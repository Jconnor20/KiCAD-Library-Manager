//! The standard library KLM ships. Only part data is shipped: one manifest per category,
//! `stdlib/<Category>.csv`, with one row per orderable MPN. Rows that share a `part_id` are one
//! generic part (e.g. "10k 0603 1%"), and its makers' alternatives sit beside it with a role each.
//! [`build`] turns the manifests into `KLM_STD_<Category>` libraries, using symbols from the user's
//! own KiCad stock libraries, so no CAD is redistributed. [`check_offline`] and [`check_found`]
//! are what `klm-cli stdlib verify` runs, so no MPN gets in without a live source confirming it.

use crate::alternatives::{ROLES, set_alt_fields};
use crate::bom::{Lifecycle, parse_csv};
use crate::kicad_env::write_atomic;
use crate::library::{LibraryRoot, rename_symbol, sanitize, set_property};
use crate::matching::{package_id, same_mpn};
use crate::part::Part;
use crate::sexpr::{self, Sexpr};
use crate::store::{Alternative, Store};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Manifest columns, in the order new files should use. Part-level columns (value, symbol, footprint,
/// description, datasheet) only need filling on the preferred row of each group.
pub const COLUMNS: &[&str] =
    &["part_id", "value", "symbol", "footprint", "description", "datasheet", "mpn", "manufacturer", "lcsc", "digikey", "mouser", "role"];

/// Supplier columns and the field names KLM writes for them.
const SUPPLIERS: &[(&str, &str)] = &[("lcsc", "LCSC"), ("digikey", "DigiKey"), ("mouser", "Mouser")];

/// Library categories built from the manifest get this prefix, so their KiCad nickname is `KLM_STD_<Category>`.
pub const PREFIX: &str = "STD_";

/// One manifest row: one orderable MPN.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// Line number in the CSV, for messages.
    pub line: usize,
    /// Column name -> value.
    pub cells: HashMap<String, String>,
}

impl Row {
    pub fn get(&self, column: &str) -> &str {
        self.cells.get(column).map_or("", |v| v.trim())
    }

    /// Supplier part numbers as (field name, number), e.g. ("LCSC", "C25804").
    pub fn supplier_ids(&self) -> Vec<(String, String)> {
        SUPPLIERS.iter().filter(|(c, _)| !self.get(c).is_empty()).map(|(c, f)| (f.to_string(), self.get(c).to_string())).collect()
    }

    fn alternative(&self) -> Alternative {
        Alternative {
            mpn: self.get("mpn").into(),
            manufacturer: self.get("manufacturer").into(),
            supplier_ids: self.supplier_ids(),
            role: self.get("role").into(),
        }
    }
}

/// One category's manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub category: String,
    pub rows: Vec<Row>,
}

impl Manifest {
    /// Parses one CSV; the header must name every column in [`COLUMNS`] (any order).
    pub fn parse(category: &str, text: &str) -> Result<Self> {
        let mut lines = parse_csv(text).into_iter().enumerate();
        let (_, header) = lines.next().context("empty manifest")?;
        let header: Vec<String> = header.iter().map(|h| h.trim().to_ascii_lowercase()).collect();
        if let Some(missing) = COLUMNS.iter().find(|c| !header.iter().any(|h| h == *c)) {
            bail!("{category}.csv has no {missing} column");
        }
        let mut rows = vec![];
        for (i, cells) in lines {
            if cells.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            if cells.len() != header.len() {
                bail!("{category}.csv line {}: {} fields, the header has {}", i + 1, cells.len(), header.len());
            }
            rows.push(Row { line: i + 1, cells: header.iter().cloned().zip(cells).collect() });
        }
        Ok(Self { category: category.into(), rows })
    }

    /// Every `*.csv` in `dir`, by file name. `only` (category names, any case) limits which.
    pub fn load_dir(dir: &Path, only: &[String]) -> Result<Vec<Self>> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv")))
            .collect();
        files.sort();
        let mut out = vec![];
        for f in files {
            let cat = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(&cat)) {
                out.push(Self::parse(&cat, &std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?)?);
            }
        }
        if let Some(missing) = only.iter().find(|o| !out.iter().any(|m| m.category.eq_ignore_ascii_case(o))) {
            bail!("no {missing}.csv in {}", dir.display());
        }
        Ok(out)
    }

    /// Rows grouped by `part_id` in file order, each group's preferred row first.
    pub fn groups(&self) -> Vec<Vec<&Row>> {
        let mut groups: Vec<Vec<&Row>> = vec![];
        for r in &self.rows {
            match groups.iter_mut().find(|g| g[0].get("part_id") == r.get("part_id")) {
                Some(g) => g.push(r),
                None => groups.push(vec![r]),
            }
        }
        for g in &mut groups {
            g.sort_by_key(|r| ROLES.iter().position(|x| *x == r.get("role")).unwrap_or(ROLES.len()));
        }
        groups
    }
}

/// A problem with one manifest row. Errors fail `verify` and stop the row being built; warnings don't.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub category: String,
    pub line: usize,
    pub part_id: String,
    pub mpn: String,
    pub error: bool,
    pub message: String,
}

fn finding(m: &Manifest, r: &Row, error: bool, message: String) -> Finding {
    Finding { category: m.category.clone(), line: r.line, part_id: r.get("part_id").into(), mpn: r.get("mpn").into(), error, message }
}

/// KiCad's stock symbol and footprint folders, and the symbol libraries read so far.
pub struct Stock {
    pub symbols: PathBuf,
    pub footprints: PathBuf,
    libs: HashMap<String, Option<Sexpr>>,
}

impl Stock {
    pub fn new(symbols: PathBuf, footprints: PathBuf) -> Self {
        Self { symbols, footprints, libs: HashMap::new() }
    }

    /// The installed KiCad's stock folders; None when KiCad isn't found.
    pub fn installed() -> Option<Self> {
        Some(Self::new(crate::kicad_env::default_var_dir("KICAD9_SYMBOL_DIR")?, crate::kicad_env::default_var_dir("KICAD9_FOOTPRINT_DIR")?))
    }

    fn lib(&mut self, name: &str) -> Option<&Sexpr> {
        let path = self.symbols.join(format!("{name}.kicad_sym"));
        self.libs.entry(name.into()).or_insert_with(|| std::fs::read_to_string(path).ok().and_then(|t| sexpr::parse(&t).ok())).as_ref()
    }

    /// The stock symbol `Lib:Name`, with a derived (`extends`) symbol flattened onto its parent so it
    /// can live in another library on its own.
    pub fn symbol(&mut self, spec: &str) -> Result<Sexpr> {
        let (lib_name, name) = spec.split_once(':').with_context(|| format!("symbol {spec:?} is not Library:Name"))?;
        let lib = self.lib(lib_name).with_context(|| format!("no stock symbol library {lib_name}"))?;
        let find = |n: &str| lib.children("symbol").find(|s| s.arg(0) == Some(n));
        let sym = find(name).with_context(|| format!("no symbol {name} in {lib_name}"))?;
        let Some(parent) = sym.child("extends").and_then(|e| e.arg(0)) else { return Ok(sym.clone()) };
        // shortcut: only the child's properties override the parent; KiCad allows one level of extends.
        let mut base = find(parent).with_context(|| format!("{name} extends {parent}, which isn't in {lib_name}"))?.clone();
        rename_symbol(&mut base, name);
        for p in sym.children("property") {
            if let (Some(k), Some(v)) = (p.arg(0), p.arg(1)) {
                set_property(&mut base, k, v);
            }
        }
        Ok(base)
    }

    /// The library's header (version, generator), for a new library file.
    fn header(&mut self, spec: &str) -> Option<Vec<Sexpr>> {
        let lib = self.lib(spec.split_once(':')?.0)?;
        Some(lib.items().iter().filter(|c| c.head() != Some("symbol")).cloned().collect())
    }

    pub fn has_footprint(&self, spec: &str) -> bool {
        spec.split_once(':').is_some_and(|(lib, name)| self.footprints.join(format!("{lib}.pretty")).join(format!("{name}.kicad_mod")).is_file())
    }
}

/// Checks that need no network: roles, one preferred per group, part-level fields on the preferred
/// row, MPNs listed once, and (with `stock`) that the symbol and footprint exist in KiCad's stock libraries.
pub fn check_offline(manifests: &[Manifest], mut stock: Option<&mut Stock>) -> Vec<Finding> {
    let mut out = vec![];
    let mut seen: Vec<(&str, &str, usize)> = vec![];
    for m in manifests {
        for r in &m.rows {
            let mpn = r.get("mpn");
            if mpn.is_empty() {
                out.push(finding(m, r, true, "no MPN".into()));
            } else if let Some((cat, _, line)) = seen.iter().find(|(_, s, _)| same_mpn(s, mpn)) {
                out.push(finding(m, r, true, format!("{mpn} is already listed at {cat}.csv line {line}")));
            } else {
                seen.push((&m.category, mpn, r.line));
            }
            if r.get("part_id").is_empty() {
                out.push(finding(m, r, true, "no part_id".into()));
            }
            if !ROLES.contains(&r.get("role")) {
                out.push(finding(m, r, true, format!("role {:?} must be one of {}", r.get("role"), ROLES.join(", "))));
            }
        }
        for g in m.groups() {
            let preferred = g.iter().filter(|r| r.get("role") == "preferred").count();
            if preferred != 1 {
                out.push(finding(m, g[0], true, format!("{} preferred rows; a part needs exactly one", preferred)));
                continue;
            }
            let p = g[0];
            for col in ["value", "symbol", "footprint"] {
                if p.get(col).is_empty() {
                    out.push(finding(m, p, true, format!("the preferred row has no {col}")));
                }
            }
            if g.len() < 2 {
                out.push(finding(m, p, false, "no alternates".into()));
            }
            if let Some(stock) = stock.as_deref_mut() {
                if let Some(Err(e)) = (!p.get("symbol").is_empty()).then(|| stock.symbol(p.get("symbol"))) {
                    out.push(finding(m, p, true, e.to_string()));
                }
                if !p.get("footprint").is_empty() && !stock.has_footprint(p.get("footprint")) {
                    out.push(finding(m, p, true, format!("no stock footprint {}", p.get("footprint"))));
                }
            }
        }
    }
    out
}

/// Checks one row against what the sources returned for its MPN: found at all, the maker matches,
/// the lifecycle is active, supplier numbers agree, and the package matches the footprint.
pub fn check_found(m: &Manifest, r: &Row, footprint: &str, found: &[Part]) -> Vec<Finding> {
    let mpn = r.get("mpn");
    let hits: Vec<&Part> = found.iter().flat_map(|p| std::iter::once(p).chain(&p.variations)).filter(|p| same_mpn(&p.mpn, mpn)).collect();
    if hits.is_empty() {
        return vec![finding(m, r, true, "not found at any source".into())];
    }
    let mut out = vec![];
    let maker = r.get("manufacturer").to_lowercase();
    let first_word = maker.split_whitespace().next().unwrap_or("");
    if !first_word.is_empty() && !hits.iter().any(|p| p.manufacturer.to_lowercase().contains(first_word)) {
        out.push(finding(m, r, false, format!("sources say the maker is {:?}", hits[0].manufacturer)));
    }
    for p in &hits {
        if let Some(l) = Lifecycle::parse(&p.lifecycle).filter(|l| *l != Lifecycle::Active) {
            out.push(finding(m, r, true, format!("{} lists it as {l:?} ({})", p.source, p.lifecycle)));
            break;
        }
    }
    for (col, field) in SUPPLIERS {
        let ids: Vec<&str> = hits.iter().flat_map(|p| &p.supplier_ids).filter(|(k, _)| k == field).map(|(_, v)| v.as_str()).collect();
        let Some(first) = ids.first() else { continue };
        let ours = r.get(col);
        if ours.is_empty() {
            out.push(finding(m, r, false, format!("{field} can be filled: {first}")));
        } else if !ids.iter().any(|id| id.eq_ignore_ascii_case(ours)) {
            out.push(finding(m, r, true, format!("{field} {ours} doesn't match the source's {}", ids.join(" / "))));
        }
    }
    let want = package_id(footprint);
    let got: Vec<String> = hits.iter().map(|p| package_id(&p.package)).filter(|g| !g.is_empty()).collect();
    if !want.is_empty() && !got.is_empty() && !got.contains(&want) {
        out.push(finding(m, r, false, format!("sources give the package as {:?}, the footprint is {want}", hits[0].package)));
    }
    out
}

/// What [`build`] did.
#[derive(Debug, Default)]
pub struct Built {
    pub parts: usize,
    pub libraries: usize,
    /// Rows not built, and why.
    pub skipped: Vec<Finding>,
}

/// Builds each manifest into the `STD_<Category>` library under `root` (replacing it), one symbol per
/// part named after its `part_id`, with the preferred MPN's fields and the others as `_AltN` fields.
/// Groups are also stored in `store` as confirmed alternatives. Parts with offline errors are skipped.
pub fn build(root: &LibraryRoot, store: &Store, manifests: &[Manifest], stock: &mut Stock) -> Result<Built> {
    let errors = check_offline(manifests, Some(stock));
    let bad = |m: &Manifest, id: &str| errors.iter().any(|f| f.error && f.category == m.category && f.part_id == id);
    let _snapshot = crate::snapshot::guard(root, "build the standard library")?;
    root.ensure_layout()?;
    let mut built = Built { skipped: errors.iter().filter(|f| f.error).cloned().collect(), ..Built::default() };
    for m in manifests {
        let cat = sanitize(&format!("{PREFIX}{}", m.category));
        let (mut header, mut symbols) = (None, vec![]);
        for g in m.groups().into_iter().filter(|g| !bad(m, g[0].get("part_id"))) {
            let p = g[0];
            let mut sym = stock.symbol(p.get("symbol"))?;
            header = header.or_else(|| stock.header(p.get("symbol")));
            rename_symbol(&mut sym, &sanitize(p.get("part_id")));
            for (field, col) in [
                ("Value", "value"),
                ("Footprint", "footprint"),
                ("Datasheet", "datasheet"),
                ("Description", "description"),
                ("Manufacturer", "manufacturer"),
                ("MPN", "mpn"),
            ] {
                if !p.get(col).is_empty() {
                    set_property(&mut sym, field, p.get(col));
                }
            }
            let part = Part {
                mpn: p.get("mpn").into(),
                manufacturer: p.get("manufacturer").into(),
                description: p.get("description").into(),
                datasheet_url: p.get("datasheet").into(),
                supplier_ids: p.supplier_ids(),
                ..Part::default()
            };
            for (k, v) in &part.supplier_ids {
                set_property(&mut sym, k, v);
            }
            crate::symbols::restyle_imported(&cat, &mut sym);
            crate::fields::fix_imported(&cat, &mut sym, &part);
            if g.len() > 1 {
                let group = crate::alternatives::confirm(store, &g.iter().map(|r| r.alternative()).collect::<Vec<_>>())?;
                set_alt_fields(&mut sym, &group, &part.mpn);
            }
            symbols.push(sym);
        }
        let Some(mut items) = header else { continue };
        built.parts += symbols.len();
        items.extend(symbols);
        write_atomic(&root.dir.join("symbols").join(format!("{cat}.kicad_sym")), &sexpr::to_string(&Sexpr::List(items)))?;
        // Footprints stay in KiCad's stock libraries for now; the empty folder keeps the table entry valid.
        std::fs::create_dir_all(root.dir.join("footprints").join(format!("{cat}.pretty")))?;
        root.link(&cat)?;
        built.libraries += 1;
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "part_id,value,symbol,footprint,description,datasheet,mpn,manufacturer,lcsc,digikey,mouser,role\n";

    fn manifest(rows: &str) -> Manifest {
        Manifest::parse("Resistors", &format!("{HEADER}{rows}")).unwrap()
    }

    /// A stock library with a base symbol and one derived from it, and one footprint.
    fn stock(name: &str) -> Stock {
        let dir = crate::test_dir(name);
        std::fs::create_dir_all(dir.join("symbols")).unwrap();
        std::fs::create_dir_all(dir.join("footprints/Resistor_SMD.pretty")).unwrap();
        std::fs::write(
            dir.join("symbols/Device.kicad_sym"),
            r#"(kicad_symbol_lib (version 20241209) (generator "kicad_symbol_editor")
  (symbol "R" (property "Reference" "R" (at 0 0 0)) (property "Value" "R" (at 0 0 0)) (property "Footprint" "" (at 0 0 0))
    (symbol "R_0_1" (rectangle (start -1 -2) (end 1 2))) (symbol "R_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "1"))))
  (symbol "R_Small" (extends "R") (property "Value" "R_Small" (at 0 0 0))))"#,
        )
        .unwrap();
        std::fs::write(dir.join("footprints/Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod"), "(footprint \"R_0603_1608Metric\")").unwrap();
        Stock::new(dir.join("symbols"), dir.join("footprints"))
    }

    const GOOD: &str = "R_0603_10k_1%,10k,Device:R,Resistor_SMD:R_0603_1608Metric,\"Resistor, 10k, 1%\",,RC0603FR-0710KL,YAGEO,C98220,,,preferred\n\
                        R_0603_10k_1%,,,,,,CRCW060310K0FKEA,Vishay Dale,,,,substitute\n";

    #[test]
    fn parses_quoted_fields_and_groups_preferred_first() {
        let m = manifest(&format!("{}\n{}", GOOD.lines().nth(1).unwrap(), GOOD.lines().next().unwrap()));
        let g = m.groups();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0][0].get("mpn"), "RC0603FR-0710KL");
        assert_eq!(g[0][0].get("description"), "Resistor, 10k, 1%");
        assert_eq!(g[0][0].supplier_ids(), vec![("LCSC".to_string(), "C98220".to_string())]);
        assert!(Manifest::parse("X", "part_id,mpn\n").unwrap_err().to_string().contains("no value column"));
    }

    #[test]
    fn offline_check_catches_bad_rows() {
        let mut s = stock("stdlib_offline");
        assert!(check_offline(&[manifest(GOOD)], Some(&mut s)).is_empty());
        let bad = manifest(
            "A,10k,Device:Nope,Resistor_SMD:R_0402,,,X1,M,,,,preferred\n\
             A,,,,,,X1,M,,,,preferred\n\
             B,,Device:R,,,,X2,M,,,,spare\n",
        );
        let msgs: Vec<String> = check_offline(&[bad], Some(&mut s)).into_iter().map(|f| f.message).collect();
        let has = |t: &str| msgs.iter().any(|m| m.contains(t));
        assert!(has("already listed"), "{msgs:?}");
        assert!(has("2 preferred rows"), "{msgs:?}");
        assert!(has("role \"spare\""), "{msgs:?}");
        assert!(has("0 preferred rows"), "{msgs:?}");
    }

    #[test]
    fn found_check_compares_with_sources() {
        let m = manifest(GOOD);
        let r = &m.rows[0];
        let fp = "Resistor_SMD:R_0603_1608Metric";
        let part = |mpn: &str, lcsc: &str, life: &str| Part {
            mpn: mpn.into(),
            manufacturer: "YAGEO".into(),
            supplier_ids: vec![("LCSC".into(), lcsc.into())],
            lifecycle: life.into(),
            package: "0603".into(),
            source: "LCSC".into(),
            ..Part::default()
        };
        assert!(check_found(&m, r, fp, &[part("RC0603FR-0710KL", "C98220", "Active")]).is_empty());
        assert_eq!(check_found(&m, r, fp, &[part("OTHER", "C1", "")])[0].message, "not found at any source");
        let msgs: Vec<String> = check_found(&m, r, fp, &[part("RC0603FR-0710KL", "C1", "Obsolete")]).into_iter().map(|f| f.message).collect();
        assert!(msgs.iter().any(|m| m.contains("Obsolete")) && msgs.iter().any(|m| m.contains("doesn't match")), "{msgs:?}");
    }

    #[test]
    fn builds_a_library_with_alternates_and_flattens_derived_symbols() {
        let mut s = stock("stdlib_build_stock");
        let dir = crate::test_dir("stdlib_build");
        let root = LibraryRoot::global(dir.join("lib"), dir.join("kicad"));
        std::fs::create_dir_all(&root.table_dir).unwrap();
        let store = Store::open(&dir.join("parts.db")).unwrap();
        let m = manifest(&format!(
            "{GOOD}R_small,1k,Device:R_Small,Resistor_SMD:R_0603_1608Metric,,,RC0603FR-071KL,YAGEO,,,,preferred\nBAD,1k,Device:R,,,,X9,M,,,,preferred\n"
        ));
        let built = build(&root, &store, &[m], &mut s).unwrap();
        assert_eq!((built.parts, built.libraries, built.skipped.len()), (2, 1, 1));
        let text = std::fs::read_to_string(root.dir.join("symbols/STD_Resistors.kicad_sym")).unwrap();
        assert!(text.contains("\"R_0603_10k_1%\"") && text.contains("\"MPN_Alt1\" \"CRCW060310K0FKEA\""), "{text}");
        assert!(text.contains("\"R_small_0_1\"") && !text.contains("extends"), "derived symbol flattened: {text}");
        assert!(text.contains("\"Footprint\" \"Resistor_SMD:R_0603_1608Metric\""));
        assert_eq!(crate::alternatives::alternatives_of(&store, "CRCW060310K0FKEA").unwrap().len(), 2);
        assert!(root.nickname("STD_Resistors").ends_with("KLM_STD_Resistors"));
    }
}
