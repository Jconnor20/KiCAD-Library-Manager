//! Import a BOM (CSV) and build a library from it: detect the columns, resolve each line to a
//! standard-library part or a part from the enabled sources, and import the resolved parts through
//! the batch path. Lines with no match or several candidates are reported, never guessed.
//! Reads CSV and spreadsheets (xlsx, xlsm, xlsb, xls, ods): the sheet and header row are detected, or given.

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

/// One sheet of a BOM file (a CSV is a single sheet), every row as read. Row and column numbers
/// match the spreadsheet's, so rows above the data (titles) are kept.
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

impl Sheet {
    /// The rows from `header` (0-based) down, header first; fails when no data row follows it.
    pub fn table(&self, header: usize) -> Result<&[Vec<String>]> {
        match self.rows.get(header..) {
            Some(t) if t.len() >= 2 => Ok(t),
            _ => anyhow::bail!("{} has no BOM rows below row {}", self.name, header + 1),
        }
    }
}

/// Reads every sheet of a CSV or spreadsheet; any other extension is read as CSV.
pub fn read_sheets(path: &Path) -> Result<Vec<Sheet>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !["xlsx", "xlsm", "xlsb", "xls", "ods"].contains(&ext.as_str()) {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        return Ok(vec![Sheet { name: "CSV".into(), rows: parse_csv(&text) }]);
    }
    let mut book = calamine::open_workbook_auto(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(calamine::Reader::worksheets(&mut book)
        .into_iter()
        .map(|(name, range)| {
            // The range starts at its first used cell; pad back to A1 so numbers match Excel's.
            let (top, left) = range.start().map_or((0, 0), |(r, c)| (r as usize, c as usize));
            let rows = std::iter::repeat_n(vec![], top)
                .chain(range.rows().map(|r| std::iter::repeat_n(String::new(), left).chain(r.iter().map(|c| c.to_string().trim().to_string())).collect()))
                .collect();
            Sheet { name, rows }
        })
        .collect())
}

/// How many BOM fields a row's cells name.
fn score(row: &[String]) -> usize {
    detect(row).iter().flatten().count()
}

/// The header row (0-based) of `rows`: the first of the top 50 that names the most fields. When none
/// names any (a headerless sheet), the first row with anything in it.
pub fn header_of(rows: &[Vec<String>]) -> usize {
    let mut best = (rows.iter().position(|r| r.iter().any(|c| !c.is_empty())).unwrap_or(0), 0);
    for (i, r) in rows.iter().enumerate().take(50) {
        if score(r) > best.1 {
            best = (i, score(r));
        }
    }
    best.0
}

/// The sheet whose header row names the most fields (the first on a tie; sheets with data beat empty ones), and that row.
pub fn auto(sheets: &[Sheet]) -> (usize, usize) {
    let pick = |(i, s): (usize, &Sheet)| {
        let h = header_of(&s.rows);
        ((s.rows.get(h).map_or(0, |r| score(r)), s.rows.len() > 1), i, h)
    };
    // `rev` so that on a tie `max_by_key`, which keeps the last, lands on the first sheet.
    sheets.iter().enumerate().rev().map(pick).max_by_key(|t| t.0).map_or((0, 0), |t| (t.1, t.2))
}

/// Reads a BOM file as a table, header first. `sheet` (a name) and `header` (1-based row) override what is detected.
pub fn load(path: &Path, sheet: Option<&str>, header: Option<usize>) -> Result<Vec<Vec<String>>> {
    let sheets = read_sheets(path)?;
    let (i, h) = match sheet {
        Some(n) => {
            let i = sheets.iter().position(|s| s.name.eq_ignore_ascii_case(n.trim())).with_context(|| format!("no sheet {n} (sheets: {})", sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")))?;
            (i, header_of(&sheets[i].rows))
        }
        None => auto(&sheets),
    };
    let h = header.map_or(h, |n| n.saturating_sub(1));
    Ok(sheets[i].table(h)?.to_vec())
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

    /// A minimal workbook: sheet "Notes" with prose, sheet "BOM" whose header is on row 3 and starts in column B.
    fn write_xlsx(path: &Path) {
        use std::io::Write;
        let cell = |r: &str, t: &str| format!(r#"<c r="{r}" t="inlineStr"><is><t>{t}</t></is></c>"#);
        let sheet = |rows: &[&[(&str, &str)]]| {
            let body: String = rows.iter().enumerate().map(|(n, r)| format!(r#"<row r="{}">{}</row>"#, n + 1, r.iter().map(|(a, t)| cell(a, t)).collect::<String>())).collect();
            format!(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{body}</sheetData></worksheet>"#)
        };
        let files = [
            ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string()),
            ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string()),
            ("xl/workbook.xml", r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Notes" sheetId="1" r:id="rId1"/><sheet name="BOM" sheetId="2" r:id="rId2"/></sheets></workbook>"#.to_string()),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#.to_string()),
            ("xl/worksheets/sheet1.xml", sheet(&[&[("A1", "Read me first")], &[("A2", "Not a BOM")]])),
            ("xl/worksheets/sheet2.xml", sheet(&[&[("B1", "Board rev C")], &[], &[("B3", "Designator"), ("C3", "MPN"), ("D3", "Quantity")], &[("B4", "R1, R2"), ("C4", "RC0603FR-0710KL"), ("D4", "2")]])),
        ];
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, xml) in files {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn reads_a_workbook_finding_the_sheet_and_header_row() {
        let path = std::env::temp_dir().join(format!("klm-bom-import-test-{}.xlsx", std::process::id()));
        write_xlsx(&path);
        let rows = load(&path, None, None).unwrap();
        // Rows 1-2 above the header are skipped; the header is Excel's row 3, and column A is padding.
        assert_eq!(rows[0], h(",Designator,MPN,Quantity"));
        let (items, at) = items_of(&rows, &detect(&rows[0]));
        assert_eq!((at, items[0].qty, items[0].line.mpn.as_str()), (vec![1], 2, "RC0603FR-0710KL"));
        // Both overrides: a named sheet, and a header row given by hand.
        assert_eq!(load(&path, Some("bom"), Some(3)).unwrap(), rows);
        assert!(load(&path, Some("Nope"), None).is_err());
        assert!(load(&path, Some("Notes"), Some(2)).is_err(), "a header on the last row leaves no data");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn headerless_sheet_falls_back_to_its_first_filled_row() {
        let rows = vec![vec![], h("foo,bar"), h("1,2")];
        assert_eq!(header_of(&rows), 1);
    }
}
