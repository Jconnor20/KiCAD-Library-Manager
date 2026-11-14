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
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

/// Progress sink for the slow steps: what is being done, how many are finished, how many there are.
pub type Report<'a> = &'a (dyn Fn(&str, usize, usize) + Sync);

/// Lookups and downloads in flight at once; each source still paces its own requests (`Throttled`), so this overlaps the waiting, not the load.
const WORKERS: usize = 8;

/// Lets another thread pause, resume or stop a running lookup or generate. A pause or stop takes effect between items:
/// ones already in flight finish (a stop also cuts short any rate-limit wait).
#[derive(Default)]
pub struct Control(AtomicU8);

const RUN: u8 = 0;
const PAUSE: u8 = 1;
const STOP: u8 = 2;
const UNDO: u8 = 3;

impl Control {
    pub fn pause(&self, on: bool) {
        let _ = self.0.compare_exchange(if on { RUN } else { PAUSE }, if on { PAUSE } else { RUN }, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Stops; with `undo`, a generate also puts the library back as it was before it started.
    pub fn stop(&self, undo: bool) {
        self.0.store(if undo { UNDO } else { STOP }, Ordering::SeqCst);
        crate::download::set_abort(true);
    }

    pub fn paused(&self) -> bool {
        self.0.load(Ordering::SeqCst) == PAUSE
    }

    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst) >= STOP
    }

    fn undo(&self) -> bool {
        self.0.load(Ordering::SeqCst) == UNDO
    }

    /// Waits while paused; false once stopped.
    fn proceed(&self) -> bool {
        while self.paused() {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        !self.stopped()
    }
}

/// `f` over `items` on a few threads, results in order, reporting each finished item. Items not reached because `ctl` stopped are left out.
pub fn par_report<T: Sync, R: Send>(stage: &str, items: &[T], report: Report, ctl: &Control, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    report(stage, 0, items.len());
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(items.len()) {
            s.spawn(|| {
                while ctl.proceed() {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
                    report(stage, done.fetch_add(1, Ordering::SeqCst) + 1, items.len());
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter().flatten().collect()
}

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
    /// Free text naming a second-source part ("TPS62933FDRLR C5219272 - checked ..."); see [`Alt`].
    Alt1,
    Alt2,
}

/// Every field with its label and the header spellings it's detected from (lower case, letters and digits only; a trailing `*` matches any header starting so).
pub const FIELDS: [(Field, &str, &[&str]); 11] = [
    (Field::Mpn, "MPN", &["mpn", "manufacturerpartnumber", "manufacturerpart", "manufacturerpn", "mfrpartnumber", "mfrpartno", "mfrpn", "mfgpartnumber", "mfgpn", "partnumber"]),
    (Field::Manufacturer, "Manufacturer", &["manufacturer", "manufacturername", "mfr", "mfg", "maker"]),
    (Field::Lcsc, "LCSC", &["lcsc", "lcscpart", "lcscpartnumber", "lcscpartno", "lcscpn", "jlcpcbpart"]),
    (Field::DigiKey, "DigiKey", &["digikey", "digikeypart", "digikeypartnumber", "digikeypn", "dkpn"]),
    (Field::Mouser, "Mouser", &["mouser", "mouserpart", "mouserpartnumber", "mouserpn"]),
    (Field::Qty, "Quantity", &["qty", "quantity", "count"]),
    (Field::Refs, "Designators", &["reference", "references", "refdes", "designator", "designators", "ref", "proposeddesignators"]),
    (Field::Value, "Value", &["value", "comment", "val"]),
    (Field::Footprint, "Footprint", &["footprint", "package", "pattern", "footprintname", "kicadfootprint*"]),
    (Field::Alt1, "Alternative 1", &["alternative1*", "alt1*", "alternate1*"]),
    (Field::Alt2, "Alternative 2", &["alternative2*", "alt2*", "alternate2*"]),
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
            let i = heads.iter().enumerate().position(|(i, h)| !taken[i] && names.iter().any(|n| n.strip_suffix('*').map_or(h == n, |p| h.starts_with(p))))?;
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
    /// Second sources named in the alternative columns (only those that give an LCSC number).
    pub alts: Vec<Alt>,
}

/// A second-source part from a BOM cell such as `TPS62933FDRLR C5219272 - checked 2026-11-08`.
#[derive(Clone, Debug, PartialEq)]
pub struct Alt {
    pub mpn: String,
    pub lcsc: String,
    /// The cell as written, for display.
    pub text: String,
}

impl Alt {
    /// The LCSC number is the first token that is C and 4+ digits; the MPN is the first token when that isn't it. Cells without an LCSC number are notes, not parts.
    fn parse(text: &str) -> Option<Alt> {
        let toks: Vec<&str> = text.split(|c: char| c.is_whitespace() || ",;()".contains(c)).filter(|t| !t.is_empty()).collect();
        let lcsc = toks.iter().find(|t| t.len() > 4 && t.starts_with(['C', 'c']) && t[1..].chars().all(|c| c.is_ascii_digit()))?;
        let mpn = toks.first().filter(|t| *t != lcsc).map_or("", |t| *t);
        Some(Alt { mpn: mpn.to_string(), lcsc: lcsc.to_ascii_uppercase(), text: text.trim().to_string() })
    }
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
    let alts = [Field::Alt1, Field::Alt2].into_iter().filter_map(|f| Alt::parse(&cell(f))).collect();
    Some(Item { line, qty, alts })
}

/// Items from the rows after the header, with the rows (indices into `rows`) each came from.
/// Rows for the same part (same MPN, else same supplier numbers, else same value and footprint) are one item: refs joined, quantities added.
pub fn items_of(rows: &[Vec<String>], map: &Mapping) -> (Vec<Item>, Vec<Vec<usize>>) {
    let same = |a: &Line, b: &Line| {
        if !a.mpn.is_empty() || !b.mpn.is_empty() {
            key(&a.mpn) == key(&b.mpn)
        } else if !a.supplier_ids.is_empty() || !b.supplier_ids.is_empty() {
            a.supplier_ids == b.supplier_ids
        } else {
            a.value.eq_ignore_ascii_case(&b.value) && a.footprint.eq_ignore_ascii_case(&b.footprint)
        }
    };
    let (mut items, mut at): (Vec<Item>, Vec<Vec<usize>>) = (vec![], vec![]);
    for (n, it) in rows.iter().enumerate().skip(1).filter_map(|(n, r)| read_row(r, map).map(|it| (n, it))) {
        match items.iter().position(|i| same(&i.line, &it.line)) {
            Some(k) => {
                items[k].qty += it.qty;
                for a in it.alts {
                    if !items[k].alts.contains(&a) {
                        items[k].alts.push(a);
                    }
                }
                items[k].line.refs.extend(it.line.refs);
                at[k].push(n);
            }
            None => (items.push(it), at.push(vec![n])).0,
        }
    }
    (items, at)
}

/// Why a BOM line is probably not an orderable part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flag {
    /// A symbol/footprint that isn't a component (test point, fiducial, ...), with what it is.
    NonPart(&'static str),
    /// The MPN is not a real part number, with why; correct it or the line is looked up by supplier number only.
    BadMpn(&'static str),
}

impl Flag {
    pub fn why(&self) -> &'static str {
        match self {
            Flag::NonPart(w) | Flag::BadMpn(w) => w,
        }
    }
}

/// Flags a line that isn't a real part: a non-component (no MPN or supplier number, and its refs, value or footprint say test point, fiducial, ...) or a bogus MPN.
pub fn flag(l: &Line) -> Option<Flag> {
    if l.mpn.is_empty() {
        if !l.supplier_ids.is_empty() {
            return None;
        }
        let prefix = |r: &String| r.trim_end_matches(|c: char| c.is_ascii_digit()).to_ascii_uppercase();
        let by_ref = [("TP", "test point"), ("TPA", "test point"), ("FID", "fiducial"), ("MH", "mounting hole"), ("MK", "fiducial"), ("LOGO", "logo")];
        if let Some((_, w)) = by_ref.iter().find(|(p, _)| !l.refs.is_empty() && l.refs.iter().all(|r| prefix(r) == *p)) {
            return Some(Flag::NonPart(w));
        }
        let text = format!("{} {}", l.value, l.footprint).to_ascii_lowercase().replace(['_', '-'], " ");
        let by_text = [("test point", "test point"), ("testpoint", "test point"), ("fiducial", "fiducial"), ("mounting hole", "mounting hole"), ("mountinghole", "mounting hole"), ("npth", "mounting hole"), ("logo", "logo"), ("smd pad", "pad"), ("smdpad", "pad"), ("solder pad", "pad")];
        return by_text.iter().find(|(k, _)| text.contains(k)).map(|(_, w)| Flag::NonPart(w));
    }
    if !l.supplier_ids.is_empty() {
        return None;
    }
    let n = norm(&l.mpn);
    // "10k", "100nF", "4.7uH": digits, then only unit letters.
    let unit = l.mpn.trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').to_ascii_lowercase();
    let valueish = l.mpn.trim().starts_with(|c: char| c.is_ascii_digit()) && unit.len() <= 3 && unit.chars().all(|c| "pnumkgrfhvwµΩ%".contains(c));
    Some(Flag::BadMpn(if ["na", "nofit", "dnp", "dni", "nc", "tbd", "none", "null", "nopart", "notfitted", "unknown"].contains(&n.as_str()) {
        "placeholder"
    } else if n.len() < 3 {
        "too short"
    } else if l.mpn.trim().eq_ignore_ascii_case(l.value.trim()) {
        "same as the value"
    } else if valueish {
        "looks like a value"
    } else {
        return None;
    }))
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

/// Which part number a line is looked up by.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum By {
    /// Every supplier number, then the MPN.
    #[default]
    Auto,
    /// The LCSC number (the MPN only when the line has none).
    Lcsc,
    /// The MPN alone.
    Mpn,
}

/// Standard library first (by MPN, any role), then the sources: each supplier number, then the MPN.
pub fn resolve(line: &Line, manifests: &[Manifest], sources: &[Box<dyn Source>]) -> Resolved {
    resolve_by(line, By::Auto, manifests, sources)
}

/// [`resolve`], looking the line up by `by`. The standard library is matched by MPN, or by the LCSC number when `by` is Lcsc.
pub fn resolve_by(line: &Line, by: By, manifests: &[Manifest], sources: &[Box<dyn Source>]) -> Resolved {
    let lcsc = line.supplier_ids.iter().find(|(s, _)| s.eq_ignore_ascii_case("LCSC")).map(|(_, id)| id.as_str());
    let by = if by == By::Lcsc && lcsc.is_none() { By::Auto } else { by };
    let mut line = line.clone();
    match by {
        By::Mpn => line.supplier_ids.clear(),
        By::Lcsc => line.supplier_ids.retain(|(s, _)| s.eq_ignore_ascii_case("LCSC")),
        By::Auto => {}
    }
    let line = &line;
    if let Some(Flag::NonPart(what)) = flag(line) {
        return Resolved::NotFound(vec![format!("not a part ({what})")]);
    }
    if !line.mpn.is_empty() && !matches!(flag(line), Some(Flag::BadMpn(_))) || by == By::Lcsc {
        for m in manifests {
            let hit = |r: &&crate::stdlib::Row| if by == By::Lcsc { lcsc.is_some_and(|id| r.get("lcsc").eq_ignore_ascii_case(id)) } else { same_mpn(r.get("mpn"), &line.mpn) };
            if let Some(g) = m.groups().into_iter().find(|g| g.iter().any(hit)) {
                return Resolved::Stdlib { category: m.category.clone(), part_id: g[0].get("part_id").into() };
            }
        }
    }
    let mut queries: Vec<(&str, Option<&str>)> = line.supplier_ids.iter().map(|(s, id)| (id.as_str(), Some(s.as_str()))).collect();
    // A bogus MPN is not searched; the supplier numbers (if any) still are.
    if !line.mpn.is_empty() && !matches!(flag(line), Some(Flag::BadMpn(_))) && (by != By::Lcsc) {
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

/// Looks the lines up (each is `(index, line)`) on several threads, reporting progress; results come back in the same order
/// (the lines a stop kept from being looked up are left out).
pub fn resolve_all(lines: &[(usize, Line)], by: By, manifests: &[Manifest], sources: &[Box<dyn Source>], report: Report, ctl: &Control) -> Vec<(usize, Resolved)> {
    par_report("Looking up", lines, report, ctl, |(n, l)| (*n, resolve_by(l, by, manifests, sources)))
}

/// Which CAD a source can supply for a part.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Avail {
    pub symbol: bool,
    pub footprint: bool,
    pub model: bool,
}

/// What CAD the part of a source-resolved line comes with, found by downloading it into `work`, and the downloaded part itself:
/// hand it to [`generate`] as `prefetched` so the files are not downloaded twice. `work` is the caller's to delete, after generating.
/// None for lines that are not source parts.
pub fn fetch_cad(r: &Resolved, sources: &[Box<dyn Source>], work: &Path) -> Option<(Avail, Option<Part>)> {
    let Resolved::Source(found) = r else { return None };
    let fetched = sources.iter().find(|s| s.name() == found.source).and_then(|s| s.fetch(found, work).ok().flatten());
    let avail = fetched.as_ref().map_or_else(Avail::default, |p| Avail { symbol: p.symbol_file.is_some(), footprint: !p.footprint_files.is_empty(), model: !p.model_files.is_empty() });
    Some((avail, fetched))
}

pub struct Options {
    pub target: Target,
    /// One category (so one library) for every part; none means each part's own guess.
    pub category: Option<String>,
    pub overwrite: bool,
    /// Also import each line's alternatives (by LCSC number) and record them as the part's alternatives.
    pub alternatives: bool,
}

/// What generating did for one line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Done {
    /// "Nickname:symbol" of the library part the line resolved to, when it has one.
    pub library_part: Option<String>,
    pub error: Option<String>,
    /// What became of each alternative: "C123 imported", "C456: ...".
    pub alts: Vec<String>,
}

pub struct Generated {
    pub lines: Vec<Done>,
    /// Parts added that the active standard has nothing / something to change in, after Standardise ran.
    pub compliant: usize,
    pub non_compliant: usize,
    /// Files Standardise changed.
    pub standardised: usize,
    pub stopped: Stopped,
}

/// How a generate ended.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Stopped {
    #[default]
    No,
    /// Stopped early; the parts imported before that are kept.
    Kept,
    /// Stopped early and the library put back as it was.
    Undone,
}

fn target_roots(target: Target, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> Vec<LibraryRoot> {
    [(target != Target::Project, global), (target != Target::Global, project)].into_iter().filter(|(w, _)| *w).filter_map(|(_, r)| r.cloned()).collect()
}

/// Downloads CAD for the source-resolved lines (into `work`) on several threads and imports each part as soon as it arrives,
/// so a stall or a stop loses nothing already downloaded; then runs Standardise on the parts added and reports their compliance.
/// Stdlib lines are only referenced: their parts live in the `KLM_STD_*` libraries that `stdlib build` writes.
/// `prefetched[i]` is line `i`'s part when [`fetch_cad`] already downloaded it (its files must still exist).
/// The whole run is one snapshot per library (one entry in History). A stop through `ctl` keeps what was imported, or with
/// undo puts every library back from that snapshot.
#[allow(clippy::too_many_arguments)]
pub fn generate(resolved: &[Resolved], alts: &[Vec<Alt>], prefetched: &[Option<Part>], sources: &[Box<dyn Source>], global: Option<&LibraryRoot>, project: Option<&LibraryRoot>, opts: &Options, settings: &crate::settings::Settings, work: &Path, report: Report, ctl: &Control) -> Result<Generated> {
    let roots = target_roots(opts.target, global, project);
    let primary = roots.first().context("no library to write into")?;
    let mut lines: Vec<Done> = resolved.iter().map(|_| Done::default()).collect();
    crate::download::set_abort(false);
    let held = roots.iter().filter(|r| r.dir.is_dir()).map(|r| crate::snapshot::guard_id(r, "BOM import").map(|(g, id)| (g, r, id))).collect::<Result<Vec<_>>>()?;
    let mut added: Vec<(String, String)> = vec![];

    // Imports one downloaded part; runs on this thread, which holds the snapshots.
    let mut import_one = |lines: &mut Vec<Done>, i: usize, found: &Part, fetched: Result<Option<Part>>| match fetched.map(|p| p.unwrap_or_else(|| found.clone())) {
        Ok(mut p) if p.symbol_file.is_some() => {
            p.source = found.source.clone();
            p.mpn = settings.rules.normalise_mpn(&p.mpn);
            let cat = sanitize(&opts.category.clone().unwrap_or_else(|| settings.rules.category_for(&p)));
            let name = sanitize(&settings.rules.name_for(&p, &cat));
            let plan = batch::plan_with(vec![Op::ImportPart { part: p, category: opts.category.clone(), target: opts.target, overwrite: opts.overwrite }], global, project, &settings.rules);
            match batch::apply(&plan, |_, _| {}).into_iter().find_map(|res| res.error) {
                Some(e) => lines[i].error = Some(e),
                None => {
                    lines[i].library_part = Some(format!("{}:{name}", primary.nickname(&cat)));
                    added.push((cat, name));
                }
            }
        }
        Ok(_) => lines[i].error = Some(format!("{} has no CAD files for {}", found.source, found.mpn)),
        Err(e) => lines[i].error = Some(format!("{e:#}")),
    };

    let is_source = |i: usize| matches!(resolved[i], Resolved::Source(_));
    let total = (0..resolved.len()).filter(|&i| is_source(i)).count();
    let need: Vec<usize> = (0..resolved.len()).filter(|&i| is_source(i) && prefetched.get(i).is_none_or(Option::is_none)).collect();
    let (stage, mut done) = ("Downloading and importing", 0);
    report(stage, 0, total);
    for (i, r) in resolved.iter().enumerate() {
        if let Resolved::Stdlib { category, part_id } = r {
            lines[i].library_part = Some(format!("{}:{}", primary.nickname(&sanitize(&format!("{}{category}", stdlib::PREFIX))), sanitize(part_id)));
        }
    }
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<Option<Part>>)>();
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(need.len()) {
            let (tx, need, next) = (tx.clone(), &need, &next);
            s.spawn(move || {
                while ctl.proceed() {
                    let Some(&i) = need.get(next.fetch_add(1, Ordering::SeqCst)) else { break };
                    let Resolved::Source(found) = &resolved[i] else { continue };
                    let got = match sources.iter().find(|s| s.name() == found.source) {
                        Some(s) => s.fetch(found, &work.join(i.to_string())),
                        None => Ok(None),
                    };
                    if tx.send((i, got)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        // What an earlier lookup already downloaded goes in while the others download.
        for i in (0..resolved.len()).filter(|&i| is_source(i) && prefetched.get(i).is_some_and(Option::is_some)) {
            if !ctl.proceed() {
                break;
            }
            if let (Resolved::Source(found), Some(p)) = (&resolved[i], prefetched[i].clone()) {
                import_one(&mut lines, i, found, Ok(Some(p)));
                done += 1;
                report(stage, done, total);
            }
        }
        // Each download is imported as it arrives. After a stop the ones still in flight arrive too and are kept (or dropped, for an undo).
        for (i, got) in rx {
            if !ctl.undo() {
                if let Resolved::Source(found) = &resolved[i] {
                    import_one(&mut lines, i, found, got);
                }
            }
            done += 1;
            report(stage, done, total);
        }
    });
    for (i, d) in lines.iter_mut().enumerate() {
        if is_source(i) && d.library_part.is_none() && d.error.is_none() {
            d.error = Some("not imported: stopped first".into());
        }
    }
    if ctl.undo() {
        let ids: Vec<_> = held.iter().filter_map(|(_, r, id)| id.as_ref().map(|id| ((*r).clone(), id.clone()))).collect();
        drop(held);
        for (root, id) in ids {
            crate::snapshot::rollback(&root, &id)?;
        }
        crate::download::set_abort(false);
        return Ok(Generated { lines, compliant: 0, non_compliant: 0, standardised: 0, stopped: Stopped::Undone });
    }
    if opts.alternatives && !ctl.stopped() {
        import_alternatives(&mut lines, alts, sources, (global, project), primary, opts, settings, work, &mut added);
    }
    report("Standardising", 0, 0);
    // Only parts that went in are standardised and judged: the rest of the library is not read again.
    let (mut standardised, mut compliant, mut non_compliant) = (0, 0, 0);
    for root in &roots {
        standardised += crate::standardise::apply(&crate::standardise::plan_for(root, &settings.standard, &settings.rules, Some(&added)))?;
        for c in crate::compliance::compute_for(root, &settings.standard, &settings.rules, &added).values() {
            if c.ok {
                compliant += 1;
            } else {
                non_compliant += 1;
            }
        }
    }
    crate::download::set_abort(false);
    Ok(Generated { lines, compliant, non_compliant, standardised, stopped: if ctl.stopped() { Stopped::Kept } else { Stopped::No } })
}

/// The (category, symbol name) of each part imported is added to `added`.
#[allow(clippy::too_many_arguments)]
fn import_alternatives(lines: &mut [Done], alts: &[Vec<Alt>], sources: &[Box<dyn Source>], (global, project): (Option<&LibraryRoot>, Option<&LibraryRoot>), primary: &LibraryRoot, opts: &Options, settings: &crate::settings::Settings, work: &Path, added: &mut Vec<(String, String)>) {
    let (mut ops, mut pending) = (vec![], vec![]);
    for (i, list) in alts.iter().enumerate().take(lines.len()) {
        if lines[i].library_part.is_none() {
            continue;
        }
        for (j, a) in list.iter().enumerate() {
            let line = Line { mpn: a.mpn.clone(), supplier_ids: vec![("LCSC".into(), a.lcsc.clone())], ..Line::default() };
            let mut alt = crate::store::Alternative { mpn: a.mpn.clone(), manufacturer: String::new(), supplier_ids: line.supplier_ids.clone(), role: "substitute".into() };
            let mut note = format!("{} not found at the sources", a.lcsc);
            if let Resolved::Source(found) = resolve_by(&line, By::Lcsc, &[], sources) {
                alt.mpn = settings.rules.normalise_mpn(&found.mpn);
                alt.manufacturer = found.manufacturer.clone();
                note = format!("{} {} listed, no CAD", a.lcsc, found.mpn);
                let fetched = sources.iter().find(|s| s.name() == found.source).and_then(|s| s.fetch(&found, &work.join(format!("alt{i}_{j}"))).ok().flatten());
                if let Some(mut p) = fetched.filter(|p| p.symbol_file.is_some()) {
                    p.source = found.source.clone();
                    p.mpn = alt.mpn.clone();
                    let cat = sanitize(&opts.category.clone().unwrap_or_else(|| settings.rules.category_for(&p)));
                    added.push((cat.clone(), sanitize(&settings.rules.name_for(&p, &cat))));
                    ops.push(Op::ImportPart { part: p, category: opts.category.clone(), target: opts.target, overwrite: opts.overwrite });
                    note = format!("{} {} imported", a.lcsc, found.mpn);
                }
            }
            if alt.mpn.trim().is_empty() {
                lines[i].alts.push(note);
            } else {
                pending.push((i, alt, note));
            }
        }
    }
    let plan = batch::plan_with(ops, global, project, &settings.rules);
    let _ = batch::apply(&plan, |_, _| {});
    let store = crate::store::Store::default_path().and_then(|p| crate::store::Store::open(&p).ok());
    let library = crate::alternatives::scan(primary);
    for (i, alt, note) in pending {
        let want = lines[i].library_part.clone().unwrap_or_default();
        let base = library.iter().find(|p| format!("{}:{}", primary.nickname(&p.category), p.symbol) == want);
        lines[i].alts.push(match (&store, base) {
            (Some(st), Some(base)) => match crate::alternatives::add_alternate(primary, st, base, alt) {
                Ok(_) => note,
                Err(e) => format!("{note}; not recorded as an alternative: {e:#}"),
            },
            _ => format!("{note}; not recorded as an alternative (part or part store not found)"),
        });
    }
}

/// The BOM as read (`rows`, header first) with a status and a library-part column appended.
/// `at[i]` is the row of line `i`.
pub fn write_back(rows: &[Vec<String>], at: &[Vec<usize>], resolved: &[Resolved], done: &[Done]) -> String {
    let q = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let lines: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(n, r)| {
            let extra = match at.iter().position(|a| a.contains(&n)) {
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

    #[test]
    fn par_report_keeps_order_and_counts_every_item() {
        let seen = Mutex::new(vec![]);
        let items: Vec<u32> = (0..40).collect();
        let out = par_report("t", &items, &|_, d, t| seen.lock().unwrap().push((d, t)), &Control::default(), |n| n * 2);
        assert_eq!(out, items.iter().map(|n| n * 2).collect::<Vec<_>>());
        let seen = seen.into_inner().unwrap();
        assert_eq!((seen.len(), seen.iter().map(|s| s.0).max()), (41, Some(40)), "a start report plus one per item");
        let stopped = Control(AtomicU8::new(STOP));
        assert!(par_report("t", &items, &|_, _, _| {}, &stopped, |n| *n).is_empty(), "a stopped run starts nothing");
    }
    
    fn h(s: &str) -> Vec<String> {
        s.split(',').map(String::from).collect()
    }

    #[test]
    fn detects_kicad_and_jlc_headers() {
        let m = detect(&h("Reference,Value,Footprint,Qty,MPN,Manufacturer,LCSC"));
        assert_eq!(m, vec![Some(4), Some(5), Some(6), None, None, Some(3), Some(0), Some(1), Some(2), None, None]);
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
        assert_eq!(at, vec![vec![1], vec![2]]);
        assert_eq!((items[0].qty, items[0].line.refs.clone()), (2, vec!["R1".to_string(), "R2".to_string()]));
        assert_eq!(items[0].line.supplier_ids, vec![("LCSC".to_string(), "C98220".to_string())]);
        assert_eq!(items[1].qty, 5);
    }

    #[test]
    fn dropbear_style_headers_and_alternatives() {
        let m = detect(&h("Line,Current KiCad refs,Proposed designators,Qty,Value,KiCad footprint (as drawn),Manufacturer,MPN,LCSC #,Alternative 1 (unverified unless noted),Alternative 2 (unverified unless noted)"));
        let at = |f: Field| m[f as usize];
        assert_eq!((at(Field::Refs), at(Field::Footprint), at(Field::Lcsc), at(Field::Alt1), at(Field::Alt2)), (Some(2), Some(5), Some(8), Some(9), Some(10)));
        let a = Alt::parse("TPS62933FDRLR (forced-PWM) C5219272 - listing checked 2026-11-08").unwrap();
        assert_eq!((a.mpn.as_str(), a.lcsc.as_str()), ("TPS62933FDRLR", "C5219272"));
        assert!(Alt::parse("Do NOT use the P or O variants: pin 7 becomes PG").is_none());
    }

    #[test]
    fn duplicates_merge_and_non_parts_are_flagged() {
        let rows = parse_csv("Designator,Value,MPN,Quantity\nR1,10k,RC0603FR-0710KL,\nR2,10k,rc0603fr-0710kl,\nTP1,TestPoint,,\nC1,100n,100n,\nU1,x,,\n");
        let (items, at) = items_of(&rows, &detect(&rows[0]));
        assert_eq!((items.len(), items[0].qty, at[0].clone()), (4, 2, vec![1, 2]));
        assert_eq!(flag(&items[0].line), None);
        assert_eq!(flag(&items[1].line), Some(Flag::NonPart("test point")));
        assert_eq!(flag(&items[2].line), Some(Flag::BadMpn("same as the value")));
        assert_eq!(flag(&Line { mpn: "74HC00".into(), ..Line::default() }), None);
        assert_eq!(flag(&Line { mpn: "4.7uF".into(), ..Line::default() }), Some(Flag::BadMpn("looks like a value")));
        assert_eq!(flag(&Line { mpn: "N/A".into(), ..Line::default() }), Some(Flag::BadMpn("placeholder")));
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
        let done = vec![Done { library_part: Some("KLM_ICs:X1".into()), ..Done::default() }];
        let out = write_back(&rows, &[vec![1]], &[Resolved::NotFound(vec![])], &done);
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
        assert_eq!((at, items[0].qty, items[0].line.mpn.as_str()), (vec![vec![1]], 2, "RC0603FR-0710KL"));
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

    /// A source that "downloads" a one-symbol part after a short delay, and can flip a `Control` to `state` on its `at`-th fetch.
    struct Fake(AtomicUsize, Option<(std::sync::Arc<Control>, u8, usize)>);

    impl Source for Fake {
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn search(&self, _: &str) -> Result<Vec<Part>> {
            Ok(vec![])
        }
        fn fetch(&self, part: &Part, work: &Path) -> Result<Option<Part>> {
            let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some((ctl, state, at)) = &self.1 {
                if n == *at {
                    ctl.0.store(*state, Ordering::SeqCst);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
            std::fs::create_dir_all(work)?;
            let sym = work.join("p.kicad_sym");
            let name = &part.mpn;
            std::fs::write(&sym, format!(r#"(kicad_symbol_lib (version 20241209) (generator "t") (symbol "{name}" (property "Reference" "U" (at 0 0 0)) (property "Value" "{name}" (at 0 0 0)) (symbol "{name}_1_1" (rectangle (start -2 2) (end 2 -2)) (pin passive line (at -4 0 0) (length 2) (name "1") (number "1")))))"#))?;
            Ok(Some(Part { symbol_file: Some(sym), ..part.clone() }))
        }
    }

    /// Generates `count` fake parts into a fresh library; returns the result and how many of them are in the library.
    fn run_fake(test: &str, count: usize, stop: Option<(u8, usize)>) -> (Generated, usize) {
        let base = crate::test_dir(test);
        let root = LibraryRoot::global(base.join("lib"), base.join("config"));
        root.ensure_layout().unwrap();
        std::fs::create_dir_all(base.join("config")).unwrap();
        let ctl = std::sync::Arc::new(Control::default());
        let sources: Vec<Box<dyn Source>> = vec![Box::new(Fake(AtomicUsize::new(0), stop.map(|(state, at)| (ctl.clone(), state, at))))];
        let names: Vec<String> = (0..count).map(|i| format!("P{i}")).collect();
        let resolved: Vec<Resolved> = names.iter().map(|n| Resolved::Source(Part { mpn: n.clone(), source: "Fake".into(), ..Part::default() })).collect();
        let opts = Options { target: Target::Global, category: Some("Test".into()), overwrite: false, alternatives: false };
        let g = generate(&resolved, &vec![vec![]; count], &[], &sources, Some(&root), None, &opts, &crate::settings::Settings::default(), &base.join("work"), &|_, _, _| {}, &ctl).unwrap();
        let in_lib = names.iter().filter(|n| root.has_part("Test", n)).count();
        (g, in_lib)
    }

    #[test]
    fn generate_imports_each_part_as_it_arrives() {
        let (g, in_lib) = run_fake("bom_generate_all", 12, None);
        assert_eq!((g.stopped, in_lib, g.lines.iter().filter(|d| d.library_part.is_some()).count()), (Stopped::No, 12, 12), "{:?}", g.lines);
    }

    #[test]
    fn a_stop_keeps_what_was_downloaded_and_an_undo_keeps_nothing() {
        let (g, in_lib) = run_fake("bom_generate_stop_keep", 60, Some((STOP, 3)));
        let imported = g.lines.iter().filter(|d| d.library_part.is_some()).count();
        assert!(g.stopped == Stopped::Kept && imported == in_lib && (3..60).contains(&imported), "{:?} kept {imported}, library has {in_lib}", g.stopped);
        assert!(g.lines.iter().filter(|d| d.library_part.is_none()).all(|d| d.error.as_deref() == Some("not imported: stopped first")));

        let (g, in_lib) = run_fake("bom_generate_stop_undo", 60, Some((UNDO, 3)));
        assert_eq!((g.stopped, in_lib), (Stopped::Undone, 0), "the library is back as it was");
        assert!(g.lines.iter().all(|d| d.library_part.is_none()));
    }
}
