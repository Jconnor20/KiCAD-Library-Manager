//! Import a BOM (CSV) and build a library from it: detect the columns, resolve each line to a
//! standard-library part or a part from the enabled sources, and import the resolved parts through
//! the batch path. Lines with no match or several candidates are reported, never guessed.
//! XLSX isn't read (no reader dependency); save the sheet as CSV.

use crate::batch::{self, Op, Target};
use crate::bom::{Line, parse_csv};
use crate::library::{LibraryRoot, sanitize};
use crate::matching::{key, same_mpn};
use crate::part::Part;
use crate::sources::Source;
use crate::stdlib::{self, Manifest};
use anyhow::{Context, Result};
use std::path::Path;

/// What a BOM column holds. The discriminant indexes a [`Mapping`] and [`FIELDS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    Mpn,
    Manufacturer,
    Lcsc,
    DigiKey,
    Mouser,
    Qty,
    Refs,
    Value,
    Footprint,
}

/// Every field with its label and the header spellings it's detected from (lower case, letters and digits only).
pub const FIELDS: [(Field, &str, &[&str]); 9] = [
    (Field::Mpn, "MPN", &["mpn", "manufacturerpartnumber", "manufacturerpart", "manufacturerpn", "mfrpartnumber", "mfrpartno", "mfrpn", "mfgpartnumber", "mfgpn", "partnumber"]),
    (Field::Manufacturer, "Manufacturer", &["manufacturer", "manufacturername", "mfr", "mfg", "maker"]),
    (Field::Lcsc, "LCSC", &["lcsc", "lcscpart", "lcscpartnumber", "lcscpartno", "lcscpn", "jlcpcbpart"]),
    (Field::DigiKey, "DigiKey", &["digikey", "digikeypart", "digikeypartnumber", "digikeypn", "dkpn"]),
    (Field::Mouser, "Mouser", &["mouser", "mouserpart", "mouserpartnumber", "mouserpn"]),
    (Field::Qty, "Quantity", &["qty", "quantity", "count"]),
    (Field::Refs, "Designators", &["reference", "references", "refdes", "designator", "designators", "ref"]),
    (Field::Value, "Value", &["value", "comment", "val"]),
    (Field::Footprint, "Footprint", &["footprint", "package", "pattern", "footprintname"]),
];

/// Column index per [`Field`] (in `FIELDS` order); the user's override edits this.
pub type Mapping = Vec<Option<usize>>;

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// Maps each field to the first column whose header is one of its spellings; a column is used once.
pub fn detect(header: &[String]) -> Mapping {
    let heads: Vec<String> = header.iter().map(|h| norm(h)).collect();
    let mut taken = vec![false; heads.len()];
    FIELDS
        .iter()
        .map(|(_, _, names)| {
            let i = heads.iter().enumerate().position(|(i, h)| !taken[i] && names.contains(&h.as_str()))?;
            taken[i] = true;
            Some(i)
        })
        .collect()
}

/// Points `field` (a label from [`FIELDS`], any case) at a column given by header text or 1-based number.
pub fn set_column(map: &mut Mapping, header: &[String], field: &str, column: &str) -> Result<()> {
    let f = FIELDS.iter().position(|(_, l, _)| l.eq_ignore_ascii_case(field)).with_context(|| format!("unknown field {field}"))?;
    let by_name = header.iter().position(|h| h.trim().eq_ignore_ascii_case(column.trim()));
    map[f] = Some(by_name.or_else(|| column.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|n| *n < header.len())).with_context(|| format!("no column {column}"))?);
    Ok(())
}

/// One BOM row: the line to match, and how many are needed.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub line: Line,
    pub qty: u64,
}

/// The item for one row, or none when it has no MPN, no supplier number and no value.
fn read_row(r: &[String], map: &Mapping) -> Option<Item> {
    let cell = |f: Field| map[f as usize].and_then(|i| r.get(i)).map(|s| s.trim().to_string()).unwrap_or_default();
    let supplier_ids: Vec<_> = [Field::Lcsc, Field::DigiKey, Field::Mouser].into_iter().filter(|f| !cell(*f).is_empty()).map(|f| (FIELDS[f as usize].1.to_string(), cell(f))).collect();
    let refs: Vec<String> = cell(Field::Refs).split([',', ';', ' ']).filter(|s| !s.is_empty()).map(String::from).collect();
    let line = Line { value: cell(Field::Value), footprint: cell(Field::Footprint), mpn: cell(Field::Mpn), manufacturer: cell(Field::Manufacturer), supplier_ids, refs };
    if line.mpn.is_empty() && line.supplier_ids.is_empty() && line.value.is_empty() {
        return None;
    }
    let qty = cell(Field::Qty).parse().ok().filter(|q| *q > 0).unwrap_or(line.refs.len().max(1) as u64);
    Some(Item { line, qty })
}

/// Items from the rows after the header, with the row number (index into `rows`) each came from.
pub fn items_of(rows: &[Vec<String>], map: &Mapping) -> (Vec<Item>, Vec<usize>) {
    rows.iter().enumerate().skip(1).filter_map(|(n, r)| read_row(r, map).map(|it| (it, n))).unzip()
}

/// Reads a BOM file as rows, header first.
pub fn load(path: &Path) -> Result<Vec<Vec<String>>> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xlsx")) {
        anyhow::bail!("XLSX isn't supported; save the sheet as CSV");
    }
    let rows = parse_csv(&std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?);
    if rows.len() < 2 {
        anyhow::bail!("{} has no BOM rows", path.display());
    }
    Ok(rows)
}

/// The standard library's manifests: the `stdlib` folder shipped beside the executable, else `./stdlib`; none if neither exists.
pub fn load_manifests() -> Vec<Manifest> {
    let shipped = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("stdlib"))).filter(|d| d.is_dir());
    Manifest::load_dir(&shipped.unwrap_or_else(|| "stdlib".into()), &[]).unwrap_or_default()
}

/// How a line resolved.
#[derive(Clone, Debug)]
pub enum Resolved {
    /// In the standard library: its category and `part_id`.
    Stdlib { category: String, part_id: String },
    Source(Part),
    /// Nothing found, plus the sources that failed.
    NotFound(Vec<String>),
    /// Several different parts match; the line needs an MPN or supplier number that picks one.
    Ambiguous(Vec<Part>),
}

impl Resolved {
    pub fn label(&self) -> &'static str {
        match self {
            Resolved::Stdlib { .. } => "stdlib",
            Resolved::Source(_) => "source",
            Resolved::NotFound(_) => "not found",
            Resolved::Ambiguous(_) => "ambiguous",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Resolved::Stdlib { category, part_id } => format!("{category}/{part_id}"),
            Resolved::Source(p) => format!("{} {} ({})", p.manufacturer, p.mpn, p.source),
            Resolved::NotFound(errors) => errors.join("; "),
            Resolved::Ambiguous(ps) => ps.iter().map(|p| format!("{} {}", p.manufacturer, p.mpn)).collect::<Vec<_>>().join(" | "),
        }
    }
}

/// Standard library first (by MPN, any role), then the sources: each supplier number, then the MPN.
pub fn resolve(line: &Line, manifests: &[Manifest], sources: &[Box<dyn Source>]) -> Resolved {
    if !line.mpn.is_empty() {
        for m in manifests {
            if let Some(g) = m.groups().into_iter().find(|g| g.iter().any(|r| same_mpn(r.get("mpn"), &line.mpn))) {
                return Resolved::Stdlib { category: m.category.clone(), part_id: g[0].get("part_id").into() };
            }
        }
    }
    let mut queries: Vec<(&str, Option<&str>)> = line.supplier_ids.iter().map(|(s, id)| (id.as_str(), Some(s.as_str()))).collect();
    if !line.mpn.is_empty() {
        queries.push((&line.mpn, None));
    }
    let mut errors = vec![];
    for (query, supplier) in queries {
        let mut found: Vec<Part> = vec![];
        for s in sources {
            match s.search(query) {
                Ok(parts) => found.extend(parts.into_iter().map(|mut p| {
                    if p.source.is_empty() {
                        p.source = s.name().into();
                    }
                    p
                })),
                Err(e) => errors.push(format!("{}: {e:#}", s.name())),
            }
        }
        found.retain(|p| match supplier {
            Some(sup) => p.supplier_ids.iter().any(|(k, v)| k.eq_ignore_ascii_case(sup) && v.eq_ignore_ascii_case(query)),
            None => same_mpn(&p.mpn, query),
        });
        // Distributors name makers differently, so the BOM's maker only narrows the list when it matches something.
        let by_maker: Vec<Part> = found.iter().filter(|p| p.manufacturer.eq_ignore_ascii_case(&line.manufacturer)).cloned().collect();
        if !by_maker.is_empty() {
            found = by_maker;
        }
        // The same part from two sources is one candidate (the first source's).
        let mut distinct: Vec<Part> = vec![];
        for p in found {
            if !distinct.iter().any(|d| key(&d.mpn) == key(&p.mpn) && d.manufacturer.eq_ignore_ascii_case(&p.manufacturer)) {
                distinct.push(p);
            }
        }
        match distinct.len() {
            0 => {}
            1 => return Resolved::Source(distinct.remove(0)),
            _ => return Resolved::Ambiguous(distinct),
        }
    }
    Resolved::NotFound(errors)
}

pub struct Options {
    pub target: Target,
    /// One category (so one library) for every part; none means each part's own guess.
    pub category: Option<String>,
    pub overwrite: bool,
}

/// What generating did for one line.
#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    /// "Nickname:symbol" of the library part the line resolved to, when it has one.
    pub library_part: Option<String>,
    pub error: Option<String>,
}

pub struct Generated {
    pub lines: Vec<Done>,
    /// Parts added that the active standard has nothing / something to change in, after Standardise ran.
    pub compliant: usize,
    pub non_compliant: usize,
    /// Files Standardise changed.
    pub standardised: usize,
}

fn target_roots(target: Target, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> Vec<LibraryRoot> {
    [(target != Target::Project, global), (target != Target::Global, project)].into_iter().filter(|(w, _)| *w).filter_map(|(_, r)| r.cloned()).collect()
}

/// Downloads CAD for the source-resolved lines (into `work`), imports them, then runs Standardise on the
/// libraries written to and reports compliance of the parts added. Stdlib lines are only referenced:
/// their parts live in the `KLM_STD_*` libraries that `stdlib build` writes.
pub fn generate(resolved: &[Resolved], sources: &[Box<dyn Source>], global: Option<&LibraryRoot>, project: Option<&LibraryRoot>, opts: &Options, settings: &crate::settings::Settings, work: &Path) -> Result<Generated> {
    let roots = target_roots(opts.target, global, project);
    let primary = roots.first().context("no library to write into")?;
    let mut lines: Vec<Done> = resolved.iter().map(|_| Done { library_part: None, error: None }).collect();
    let (mut ops, mut at, mut added) = (vec![], vec![], vec![]);
    for (i, r) in resolved.iter().enumerate() {
        match r {
            Resolved::Stdlib { category, part_id } => lines[i].library_part = Some(format!("{}:{}", primary.nickname(&sanitize(&format!("{}{category}", stdlib::PREFIX))), sanitize(part_id))),
            Resolved::Source(found) => {
                let fetched = match sources.iter().find(|s| s.name() == found.source) {
                    Some(s) => s.fetch(found, &work.join(i.to_string())).map(|p| p.unwrap_or_else(|| found.clone())),
                    None => Ok(found.clone()),
                };
                match fetched {
                    Ok(mut p) if p.symbol_file.is_some() => {
                        p.source = found.source.clone();
                        p.mpn = settings.rules.normalise_mpn(&p.mpn);
                        let cat = sanitize(&opts.category.clone().unwrap_or_else(|| settings.rules.category_for(&p)));
                        let name = sanitize(&settings.rules.name_for(&p, &cat));
                        lines[i].library_part = Some(format!("{}:{name}", primary.nickname(&cat)));
                        added.push((cat, name));
                        ops.push(Op::ImportPart { part: p, category: opts.category.clone(), target: opts.target, overwrite: opts.overwrite });
                        at.push(i);
                    }
                    Ok(_) => lines[i].error = Some(format!("{} has no CAD files for {}", found.source, found.mpn)),
                    Err(e) => lines[i].error = Some(format!("{e:#}")),
                }
            }
            _ => {}
        }
    }
    let plan = batch::plan_with(ops, global, project, &settings.rules);
    for (i, res) in at.iter().zip(batch::apply(&plan, |_, _| {})) {
        if let Some(e) = res.error {
            lines[*i].library_part = None;
            lines[*i].error = Some(e);
        }
    }
    let (mut standardised, mut compliant, mut non_compliant) = (0, 0, 0);
    for root in &roots {
        standardised += crate::standardise::apply(&crate::standardise::plan(root, &settings.standard, &settings.rules))?;
        for (k, c) in crate::compliance::compute(root, &settings.standard, &settings.rules) {
            if added.contains(&k) {
                if c.ok {
                    compliant += 1;
                } else {
                    non_compliant += 1;
                }
            }
        }
    }
    Ok(Generated { lines, compliant, non_compliant, standardised })
}

/// The BOM as read (`rows`, header first) with a status and a library-part column appended.
/// `at[i]` is the row of line `i`.
pub fn write_back(rows: &[Vec<String>], at: &[usize], resolved: &[Resolved], done: &[Done]) -> String {
    let q = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let lines: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(n, r)| {
            let extra = match at.iter().position(|&a| a == n) {
                _ if n == 0 => vec!["KLM status".to_string(), "Library part".to_string()],
                Some(i) => vec![resolved[i].label().to_string(), done.get(i).and_then(|d| d.library_part.clone()).unwrap_or_default()],
                None => vec![String::new(); 2],
            };
            r.iter().cloned().chain(extra).map(|c| q(&c)).collect::<Vec<_>>().join(",")
        })
        .collect();
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn h(s: &str) -> Vec<String> {
        s.split(',').map(String::from).collect()
    }

    #[test]
    fn detects_kicad_and_jlc_headers() {
        let m = detect(&h("Reference,Value,Footprint,Qty,MPN,Manufacturer,LCSC"));
        assert_eq!(m, vec![Some(4), Some(5), Some(6), None, None, Some(3), Some(0), Some(1), Some(2)]);
        // JLCPCB's BOM: designators, comment, "LCSC Part #"; "Manufacturer Part Number" is the MPN.
        let m = detect(&h("Comment,Designator,Footprint,LCSC Part #,Manufacturer Part Number,Mouser Part Number"));
        assert_eq!(m[Field::Mpn as usize], Some(4));
        assert_eq!(m[Field::Lcsc as usize], Some(3));
        assert_eq!(m[Field::Mouser as usize], Some(5));
        assert_eq!((m[Field::Refs as usize], m[Field::Value as usize]), (Some(1), Some(0)));
    }

    #[test]
    fn reads_items_and_counts_refs() {
        let rows = parse_csv("Designator,Value,MPN,LCSC,Quantity\n\"R1, R2\",10k,RC0603FR-0710KL,C98220,\nC1,100n,,,5\n,,,,\n");
        let (items, at) = items_of(&rows, &detect(&rows[0]));
        assert_eq!(at, vec![1, 2]);
        assert_eq!((items[0].qty, items[0].line.refs.clone()), (2, vec!["R1".to_string(), "R2".to_string()]));
        assert_eq!(items[0].line.supplier_ids, vec![("LCSC".to_string(), "C98220".to_string())]);
        assert_eq!(items[1].qty, 5);
    }

    /// Answers searches from a fixed list; `fetch` has no CAD.
    struct Stub {
        parts: Vec<Part>,
    }

    impl Source for Stub {
        fn name(&self) -> &'static str {
            "Stub"
        }
        fn search(&self, q: &str) -> Result<Vec<Part>> {
            Ok(self.parts.iter().filter(|p| p.mpn.eq_ignore_ascii_case(q) || p.supplier_ids.iter().any(|(_, v)| v == q)).cloned().collect())
        }
        fn fetch(&self, _: &Part, _: &Path) -> Result<Option<Part>> {
            Ok(None)
        }
    }

    fn part(mpn: &str, maker: &str, lcsc: &str) -> Part {
        Part { mpn: mpn.into(), manufacturer: maker.into(), supplier_ids: vec![("LCSC".into(), lcsc.into())], ..Part::default() }
    }

    fn line(mpn: &str) -> Line {
        Line { mpn: mpn.into(), ..Line::default() }
    }

    #[test]
    fn stdlib_beats_sources_and_unmatched_lines_are_reported() {
        let manifest = Manifest::parse(
            "Resistors",
            "part_id,value,symbol,footprint,description,datasheet,mpn,manufacturer,lcsc,digikey,mouser,role\nR_10k,10k,Device:R,,,,RC0603FR-0710KL,YAGEO,C98220,,,preferred\nR_10k,,,,,,CRCW060310K0FKEA,Vishay,,,,substitute\n",
        )
        .unwrap();
        // The stdlib MPN also exists at the source; the stdlib must win without a search.
        let stub = Stub { parts: vec![part("CRCW060310K0FKEA", "Vishay", "C9"), part("LM358", "TI", "C7950"), part("NE555", "TI", "C1"), part("NE555", "ST", "C2")] };
        let sources: Vec<Box<dyn Source>> = vec![Box::new(stub)];
        match resolve(&line("crcw060310k0fkea"), std::slice::from_ref(&manifest), &sources) {
            Resolved::Stdlib { category, part_id } => assert_eq!((category.as_str(), part_id.as_str()), ("Resistors", "R_10k")),
            r => panic!("{r:?}"),
        }
        assert!(matches!(resolve(&line("LM358"), std::slice::from_ref(&manifest), &sources), Resolved::Source(p) if p.manufacturer == "TI"));
        assert!(matches!(resolve(&line("NE555"), &[], &sources), Resolved::Ambiguous(v) if v.len() == 2));
        let with_maker = Line { manufacturer: "st".into(), ..line("NE555") };
        assert!(matches!(resolve(&with_maker, &[], &sources), Resolved::Source(p) if p.supplier_ids[0].1 == "C2"));
        assert!(matches!(resolve(&line("NOPE"), &[manifest], &sources), Resolved::NotFound(_)));
        // A supplier number is tried before the MPN.
        let by_id = Line { supplier_ids: vec![("LCSC".into(), "C7950".into())], ..line("WRONG") };
        assert!(matches!(resolve(&by_id, &[], &sources), Resolved::Source(p) if p.mpn == "LM358"));
    }

    #[test]
    fn write_back_adds_two_columns() {
        let rows = vec![h("MPN,Value"), vec!["X1".to_string(), "1,5".to_string()], h("")];
        let done = vec![Done { library_part: Some("KLM_ICs:X1".into()), error: None }];
        let out = write_back(&rows, &[1], &[Resolved::NotFound(vec![])], &done);
        assert_eq!(out, "MPN,Value,KLM status,Library part\nX1,\"1,5\",not found,KLM_ICs:X1\n,,\n");
    }
}
