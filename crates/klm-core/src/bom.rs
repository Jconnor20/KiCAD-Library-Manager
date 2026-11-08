//! BOM import and enrichment: reads a project's BOM with `kicad-cli sch export bom`, matches each line
//! against the libraries, and prices it from the distributor sources (cached results work offline).
//!
//! Matching is by MPN, then supplier number. A line is never matched on value alone, and a prefix match
//! ("LM358" vs "LM358DR") is only a hint: packaging suffixes change the part, so nothing is priced on it.

use crate::alternatives::{self, LibPart};
use crate::cache;
use crate::currency::{self, Rates};
use crate::matching::{key, same_mpn};
use crate::part::Part;
use crate::search_filter::parse_price;
use crate::sources::{self, Source};
use crate::store::{Alternative, Store};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Symbol fields read as supplier numbers.
const SUPPLIER_FIELDS: &[&str] = &["LCSC", "DigiKey", "Mouser"];

/// Which schematic fields hold each part number; parsing always uses the standard labels, so only the
/// `kicad-cli` export reads these.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BomFields {
    pub mpn: String,
    pub manufacturer: String,
    pub lcsc: String,
    pub digikey: String,
    pub mouser: String,
}

impl Default for BomFields {
    fn default() -> Self {
        let s = |x: &str| x.to_string();
        Self { mpn: s("MPN"), manufacturer: s("Manufacturer"), lcsc: s("LCSC"), digikey: s("DigiKey"), mouser: s("Mouser") }
    }
}

impl BomFields {
    /// Names in the order of the standard labels MPN, Manufacturer, LCSC, DigiKey, Mouser; a blank one is the standard.
    fn names(&self) -> [String; 5] {
        let d = Self::default();
        let pick = |v: &str, def: String| if v.trim().is_empty() { def } else { v.trim().to_string() };
        [pick(&self.mpn, d.mpn), pick(&self.manufacturer, d.manufacturer), pick(&self.lcsc, d.lcsc), pick(&self.digikey, d.digikey), pick(&self.mouser, d.mouser)]
    }
}

/// One BOM line: the placed parts that are the same part, grouped here (not by kicad-cli, which
/// would show mixed fields as a placeholder).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub refs: Vec<String>,
    pub value: String,
    pub footprint: String,
    pub mpn: String,
    pub manufacturer: String,
    pub supplier_ids: Vec<(String, String)>,
}

impl Line {
    pub fn qty(&self) -> u64 {
        self.refs.len() as u64
    }
}

/// Per-project settings, kept in `.klm-bom.json` in the project folder.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectBom {
    /// Overrides the Settings shipping region when not empty.
    pub region: String,
    pub boards: u64,
    /// This project's field names; the Settings ones when unset.
    pub fields: Option<BomFields>,
}

impl Default for ProjectBom {
    fn default() -> Self {
        Self { region: String::new(), boards: 1, fields: None }
    }
}

impl ProjectBom {
    const FILE: &str = ".klm-bom.json";

    pub fn load(project: &Path) -> Self {
        std::fs::read_to_string(project.join(Self::FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, project: &Path) -> Result<()> {
        std::fs::write(project.join(Self::FILE), serde_json::to_string_pretty(self)?).context("saving the BOM settings")
    }

    /// The project's region, else the Settings default.
    pub fn region<'a>(&'a self, default: &'a str) -> &'a str {
        if self.region.trim().is_empty() { default.trim() } else { self.region.trim() }
    }
}

// ---- reading the BOM ----

/// Runs `kicad-cli sch export bom` on the project's root schematic and returns the lines.
pub fn export(project: &Path, fields: &BomFields) -> Result<Vec<Line>> {
    let sch = root_schematic(project)?;
    let out = std::env::temp_dir().join(format!("klm-bom-{}.csv", std::process::id()));
    let [mpn, maker, lcsc, digikey, mouser] = fields.names();
    let field_list = format!("Reference,Value,Footprint,${{DNP}},{mpn},{maker},{lcsc},{digikey},{mouser}");
    let args: Vec<std::ffi::OsString> = [
        "sch",
        "export",
        "bom",
        "--output",
        &out.to_string_lossy(),
        "--fields",
        &field_list,
        "--labels",
        "Reference,Value,Footprint,DNP,MPN,Manufacturer,LCSC,DigiKey,Mouser",
        "--field-delimiter",
        ",",
        "--string-delimiter",
        "\"",
        "--ref-delimiter",
        ",",
        "--exclude-dnp",
    ]
    .iter()
    .map(Into::into)
    .chain([sch.into_os_string()])
    .collect();
    let refs: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
    crate::kicad_env::run_cli(&refs).context("exporting the BOM (needs KiCad 8 or later)")?;
    let text = std::fs::read_to_string(&out).context("reading the exported BOM");
    let _ = std::fs::remove_file(&out);
    Ok(parse_bom(&text?))
}

fn root_schematic(project: &Path) -> Result<std::path::PathBuf> {
    let dir = if project.is_dir() { project.to_path_buf() } else { project.parent().unwrap_or(Path::new(".")).to_path_buf() };
    let files: Vec<_> = std::fs::read_dir(&dir)?.flatten().map(|e| e.path()).collect();
    let ext = |p: &std::path::PathBuf, e: &str| p.extension().is_some_and(|x| x == e);
    // The root sheet shares the project's name.
    let root = files.iter().find(|p| ext(p, "kicad_pro")).map(|p| p.with_extension("kicad_sch")).filter(|p| p.exists());
    match root.or_else(|| files.iter().filter(|p| ext(p, "kicad_sch")).min().cloned()) {
        Some(p) => Ok(p),
        None => bail!("no .kicad_sch file in {}", dir.display()),
    }
}

/// RFC 4180 CSV: quoted fields, `""` escapes and newlines inside quotes; a leading UTF-8 BOM is dropped.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let text = text.trim_start_matches('\u{feff}');
    let (mut rows, mut row, mut field, mut quoted) = (vec![], vec![], String::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Lines from an ungrouped BOM export (one row per placed part, header first): grouped by MPN and
/// footprint (or value and footprint when there is no MPN). DNP rows and rows with no value and
/// footprint (logos, mounting holes) are dropped.
pub fn parse_bom(csv: &str) -> Vec<Line> {
    let rows = parse_csv(csv);
    let Some((header, body)) = rows.split_first() else { return vec![] };
    let col = |name: &str| header.iter().position(|h| h.trim().eq_ignore_ascii_case(name));
    let cell = |r: &Vec<String>, name: &str| col(name).and_then(|i| r.get(i)).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut lines: Vec<Line> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    for r in body {
        let dnp = cell(r, "DNP").to_lowercase();
        let (value, footprint) = (cell(r, "Value"), cell(r, "Footprint"));
        if matches!(dnp.as_str(), "dnp" | "yes" | "true" | "1" | "x") || (value.is_empty() && footprint.is_empty()) {
            continue;
        }
        let mpn = cell(r, "MPN");
        let maker = cell(r, "Manufacturer");
        // The same MPN from two makers is two parts.
        let group = if mpn.is_empty() { format!("v:{}|{footprint}", value.to_lowercase()) } else { format!("m:{}|{}|{footprint}", key(&mpn), maker.to_lowercase()) };
        let i = *index.entry(group).or_insert_with(|| {
            lines.push(Line { value, footprint, mpn, manufacturer: maker, ..Line::default() });
            lines.len() - 1
        });
        // A number set on only some of the placed parts still counts for the line.
        for s in SUPPLIER_FIELDS {
            let id = cell(r, s);
            if !id.is_empty() && !lines[i].supplier_ids.iter().any(|(k, _)| k == s) {
                lines[i].supplier_ids.push((s.to_string(), id));
            }
        }
        lines[i].refs.push(cell(r, "Reference"));
    }
    for l in &mut lines {
        l.refs.sort_by_key(|r| natural(r));
    }
    lines.sort_by_key(|l| l.refs.first().map(|r| natural(r)));
    lines
}

/// "R10" sorts after "R9".
fn natural(r: &str) -> (String, u64) {
    let split = r.find(|c: char| c.is_ascii_digit()).unwrap_or(r.len());
    (r[..split].to_string(), r[split..].parse().unwrap_or(0))
}

// ---- matching ----

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No MPN and no supplier number: nothing to match on (a stock `Device:R`, say).
    NoPartNumber,
    /// In the library under `symbol`; `has_footprint` is false when the symbol names no footprint.
    Found { symbol: String, has_footprint: bool },
    NotInLibrary,
    /// The MPN is in the library but only from other manufacturers than the BOM's.
    OtherMaker(String),
    /// Not in the library, but the library has a part whose MPN starts the same (or the reverse).
    Close(String),
}

impl Status {
    /// The library is missing CAD for this line.
    pub fn missing_cad(&self) -> bool {
        matches!(self, Status::NotInLibrary | Status::OtherMaker(_) | Status::Close(_) | Status::Found { has_footprint: false, .. })
    }
}

pub fn match_line(line: &Line, lib: &[LibPart]) -> Status {
    if line.mpn.is_empty() && line.supplier_ids.is_empty() {
        return Status::NoPartNumber;
    }
    let hit = |p: &&LibPart| {
        (!line.mpn.is_empty() && same_mpn(&p.mpn, &line.mpn))
            || line.supplier_ids.iter().any(|(s, id)| p.supplier_ids.iter().any(|(ps, pid)| ps == s && pid.eq_ignore_ascii_case(id)))
    };
    let found: Vec<&LibPart> = lib.iter().filter(hit).collect();
    // The same MPN from two makers is two parts (BC847, LM358): prefer the BOM's maker.
    let by_maker = found.iter().find(|p| line.manufacturer.is_empty() || p.manufacturer.eq_ignore_ascii_case(&line.manufacturer));
    match (by_maker, found.first()) {
        (Some(p), _) => Status::Found { symbol: p.symbol.clone(), has_footprint: !p.footprint.trim().is_empty() },
        (None, Some(p)) => Status::OtherMaker(p.manufacturer.clone()),
        (None, None) => {
            let k = key(&line.mpn);
            let close = lib.iter().find(|p| {
                let pk = key(&p.mpn);
                k.len() >= 5 && pk.len() >= 5 && (pk.starts_with(&k) || k.starts_with(&pk))
            });
            close.map_or(Status::NotInLibrary, |p| Status::Close(p.mpn.clone()))
        }
    }
}

// ---- quotes and pricing ----

/// What one distributor offers for a line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quote {
    pub source: String,
    pub id: String,
    pub stock: Option<u64>,
    pub breaks: Vec<(u64, f64)>,
    pub currency: String,
    pub moq: u64,
    /// Order multiple (Mouser's `Mult`): the quantity bought is a whole number of these.
    pub multiple: u64,
    /// "Cut Tape (CT)", "Tape & Reel (TR)": one DigiKey part has a quote per packaging.
    pub packaging: String,
    /// Factory lead time; used only when the stock doesn't cover the order.
    pub lead_days: Option<u32>,
    /// JLCPCB library class and lifecycle as the source reported them (empty = the source doesn't say).
    pub jlc_type: String,
    pub lifecycle: String,
    /// From a cache entry older than its lifetime, used because the network failed.
    pub stale: bool,
}

/// The result for a quote at an order quantity.
#[derive(Clone, Debug, PartialEq)]
pub struct Priced {
    /// Quantity actually bought: at least the MOQ and the first price break, or a higher break when that is cheaper.
    pub bought: u64,
    pub unit: f64,
    pub ext: f64,
    pub in_stock: bool,
}

pub fn quote_of(source: &str, p: &Part) -> Quote {
    let mut breaks = p.price_breaks.clone();
    if breaks.is_empty() {
        // Sources with one price only have the display text ("$0.0123 @100").
        let one = parse_price(&p.unit_price).zip(p.unit_price.split_once('@').and_then(|(_, q)| q.trim().parse::<u64>().ok()));
        if let Some((v, q)) = one {
            breaks.push((q, v));
        }
    }
    breaks.sort_by_key(|b| b.0);
    Quote {
        source: source.into(),
        id: p.supplier_ids.iter().find(|(s, _)| s == source).map(|(_, v)| v.clone()).unwrap_or_default(),
        stock: p.stock,
        currency: if p.price_currency.is_empty() { currency::detect(&p.unit_price).unwrap_or_default() } else { p.price_currency.clone() },
        moq: p.moq.unwrap_or(1).max(1),
        // DigiKey's pack size is a reel, not a multiple: its reel quotes already carry it as the MOQ.
        multiple: if source == "Mouser" { p.pack_qty.unwrap_or(1).max(1) } else { 1 },
        packaging: p.packaging.clone(),
        lead_days: p.lead_time_days,
        jlc_type: p.jlc_type.clone(),
        lifecycle: p.lifecycle.clone(),
        stale: false,
        breaks,
    }
}

/// The cheapest way to buy at least `need` from this quote, in the quote's currency. `None` with no price breaks.
pub fn price(q: &Quote, need: u64) -> Option<Priced> {
    let first = q.breaks.first()?.0;
    let m = q.multiple.max(1);
    let up = |n: u64| n.div_ceil(m) * m;
    let floor = up(need.max(q.moq).max(first));
    let unit_at = |n: u64| q.breaks.iter().take_while(|b| b.0 <= n).last().map_or(q.breaks[0].1, |b| b.1);
    // Buying up to the next break can cost less than the exact quantity.
    // Only round up past stock when the exact quantity is already out of stock.
    let cap = q.stock.filter(|&s| s >= floor).unwrap_or(u64::MAX);
    let bought = std::iter::once(floor)
        .chain(q.breaks.iter().map(|b| up(b.0)).filter(|&n| n > floor && n <= cap))
        .min_by(|a, b| (unit_at(*a) * *a as f64).total_cmp(&(unit_at(*b) * *b as f64)))?;
    let unit = unit_at(bought);
    Some(Priced { bought, unit, ext: unit * bought as f64, in_stock: q.stock.is_some_and(|s| s >= bought) })
}

/// A line's best offer, with prices in the target currency.
#[derive(Clone, Debug, PartialEq)]
pub struct Best {
    pub quote: usize,
    pub priced: Priced,
    /// Days until it ships: 0 in stock, else the factory lead time (`None` when unknown).
    pub lead_days: Option<u32>,
}

/// Cheapest in-stock quote, else the cheapest overall. Quotes in another currency need `rates`, and
/// are left out when there are none (never mixed into a total at face value).
pub fn best(quotes: &[Quote], need: u64, target: &str, rates: Option<&Rates>) -> Option<Best> {
    let mut options: Vec<(usize, Priced)> = quotes
        .iter()
        .enumerate()
        .filter_map(|(i, q)| {
            let mut p = price(q, need)?;
            if q.currency != target {
                let f = rates?.convert(1.0, &q.currency, target)?;
                (p.unit, p.ext) = (p.unit * f, p.ext * f);
            }
            Some((i, p))
        })
        .collect();
    options.sort_by(|a, b| (!a.1.in_stock).cmp(&!b.1.in_stock).then(a.1.ext.total_cmp(&b.1.ext)));
    let (quote, priced) = options.into_iter().next()?;
    let lead_days = if priced.in_stock { Some(0) } else { quotes[quote].lead_days };
    Some(Best { quote, priced, lead_days })
}

// ---- fetching ----

/// A distributor to ask, with the string its cache entries are filed under (the region is part of it
/// for DigiKey, and `/v2` keeps entries written before price breaks existed from being served).
pub struct Distributor {
    pub cache_name: String,
    pub source: Box<dyn Source>,
    /// Least time between two requests to it (Mouser allows about 30 a minute); cache hits don't wait.
    pub interval: std::time::Duration,
}

/// The enabled sources, and why the others are out: "DigiKey: no API key".
pub fn distributors(settings: &crate::settings::Settings, region: &str) -> (Vec<Distributor>, Vec<String>) {
    let (mut out, mut skipped) = (vec![], vec![]);
    // Nexar is left out on purpose: its quota is small and it duplicates the other three.
    for name in ["LCSC", "DigiKey", "Mouser"] {
        if !settings.is_enabled(name) {
            continue;
        }
        let interval = std::time::Duration::from_millis(match name {
            "Mouser" => 2100,
            "DigiKey" => 500,
            _ => 0,
        });
        let built: Option<(String, Box<dyn Source>)> = if name == "DigiKey" {
            sources::digikey::DigiKey::from_credentials().map(|d| {
                let (site, cur) = sources::digikey::locale(region);
                (format!("DigiKey@{site}/{cur}/v4"), Box::new(d.with_region(region)) as Box<dyn Source>)
            })
        } else {
            sources::build(name).map(|s| (format!("{name}/v3"), s))
        };
        match built {
            Some((cache_name, source)) => out.push(Distributor { cache_name, source, interval }),
            None => skipped.push(format!("{name}: no API key")),
        }
    }
    (out, skipped)
}

/// The best-matching result for a line: its supplier number when it has one for this source, else the MPN
/// from the BOM's maker (or any maker if the BOM names none).
pub fn pick<'a>(results: &'a [Part], line: &Line, source: &str) -> Option<&'a Part> {
    if let Some((_, id)) = line.supplier_ids.iter().find(|(s, _)| s == source) {
        if let Some(p) = results.iter().find(|p| p.supplier_ids.iter().any(|(s, v)| s == source && v.eq_ignore_ascii_case(id))) {
            return Some(p);
        }
    }
    let same: Vec<&Part> = results.iter().filter(|p| same_mpn(&p.mpn, &line.mpn)).collect();
    same.iter().find(|p| line.manufacturer.is_empty() || p.manufacturer.eq_ignore_ascii_case(&line.manufacturer)).or(same.first()).copied()
}

/// Quotes for every line, one list per line, plus per-source problems ("LCSC: offline"). Results come
/// from the cache when fresh, and from an expired entry when the network fails.
pub fn fetch_quotes(lines: &[Line], dist: &[Distributor], store: Option<&Store>, ttl_secs: i64) -> (Vec<Vec<Quote>>, Vec<String>) {
    let mut problems = vec![];
    // (distributor, query) -> results and whether they came from an expired entry; one request per distinct query.
    let mut done: HashMap<(usize, String), Option<(Vec<Part>, bool)>> = HashMap::new();
    // Distributors that failed once: later queries use the cache only, so being offline costs one timeout.
    let mut failed: Vec<usize> = vec![];
    let mut last_call: HashMap<usize, std::time::Instant> = HashMap::new();
    let quotes = lines
        .iter()
        .map(|line| {
            let mut out = vec![];
            for (di, d) in dist.iter().enumerate() {
                let name = d.source.name();
                let query = line.supplier_ids.iter().find(|(s, _)| s == name).map(|(_, v)| v.clone()).unwrap_or_else(|| line.mpn.clone());
                if query.is_empty() {
                    continue;
                }
                let skip = failed.contains(&di);
                let res = done.entry((di, query.to_lowercase())).or_insert_with(|| match cache::search(store, &d.cache_name, &query, ttl_secs, || {
                    if skip {
                        bail!("skipped")
                    }
                    if let Some(wait) = last_call.get(&di).and_then(|t| d.interval.checked_sub(t.elapsed())) {
                        std::thread::sleep(wait);
                    }
                    last_call.insert(di, std::time::Instant::now());
                    d.source.search(&query)
                }) {
                    Ok(r) => Some((r, false)),
                    Err(e) => {
                        if !skip {
                            failed.push(di);
                            problems.push(format!("{name}: {e:#}; using older cached data where there is some"));
                        }
                        store.and_then(|s| cache::get(s, &d.cache_name, &query, i64::MAX, 0)).map(|old| (old, true))
                    }
                });
                if let Some((results, stale)) = res {
                    if let Some(p) = pick(results, line, name) {
                        // Every packaging the source lists is a quote of its own (cut tape, reel...).
                        for v in std::iter::once(p).chain(&p.variations) {
                            out.push(Quote { stale: *stale, ..quote_of(name, v) });
                        }
                    }
                }
            }
            out
        })
        .collect();
    (quotes, problems)
}

// ---- assembly status (JLCPCB) ----

/// JLCPCB's assembly library classes. Preferred parts are extended ones JLC also flags as preferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JlcType {
    Basic,
    Preferred,
    Extended,
}

/// Worst last, so `max` over a line's quotes is the one to warn about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lifecycle {
    Active,
    Nrnd,
    LastTimeBuy,
    Obsolete,
}

impl Lifecycle {
    /// Reads a distributor's status text. `None` for wording that isn't a manufacturer lifecycle:
    /// DigiKey's "Discontinued at Digi-Key" only says DigiKey stopped selling it.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        // shortcut: Mouser's exact values were never saved to a fixture, so these are matched by substring; upgrade when a live reply is captured.
        match s.as_str() {
            _ if s.contains("digi-key") => None,
            _ if s.contains("last time") => Some(Self::LastTimeBuy),
            _ if s.contains("not for new") || s.contains("not recommended") || s.contains("nrnd") => Some(Self::Nrnd),
            _ if s.contains("obsolete") || s.contains("end of life") || s.contains("discontinued") => Some(Self::Obsolete),
            _ if s.contains("inactive") => None,
            _ if s.contains("active") || s.contains("new product") => Some(Self::Active),
            _ => None,
        }
    }
}

/// What the sources say about building a line at JLCPCB; `None` means unknown, never "no".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Assembly {
    pub jlc: Option<JlcType>,
    /// Parts JLC holds (from the LCSC quote); `Some(0)` is out of stock.
    pub jlc_stock: Option<u64>,
    pub lifecycle: Option<Lifecycle>,
}

impl Assembly {
    pub fn out_of_stock(&self) -> bool {
        self.jlc_stock == Some(0)
    }

    /// Worth a Swap: out of stock at JLC, or past active in its lifecycle.
    pub fn flagged(&self) -> bool {
        self.out_of_stock() || self.lifecycle.is_some_and(|l| l != Lifecycle::Active)
    }
}

pub fn assembly(quotes: &[Quote]) -> Assembly {
    let lcsc = || quotes.iter().filter(|q| q.source == "LCSC");
    let jlc = lcsc().find_map(|q| match q.jlc_type.as_str() {
        "basic" => Some(JlcType::Basic),
        "preferred" => Some(JlcType::Preferred),
        "extended" => Some(JlcType::Extended),
        _ => None,
    });
    Assembly { jlc, jlc_stock: lcsc().filter_map(|q| q.stock).max(), lifecycle: quotes.iter().filter_map(|q| Lifecycle::parse(&q.lifecycle)).max() }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssemblySummary {
    pub basic: usize,
    pub preferred: usize,
    pub extended: usize,
    pub out_of_stock: usize,
    /// Lines with no JLC class: not looked up, or not found at JLC.
    pub unknown: usize,
    pub obsolete: usize,
    pub nrnd: usize,
}

pub fn assembly_summary(rows: &[Row]) -> AssemblySummary {
    let mut s = AssemblySummary::default();
    for a in rows.iter().map(|r| &r.assembly) {
        match a.jlc {
            Some(JlcType::Basic) => s.basic += 1,
            Some(JlcType::Preferred) => s.preferred += 1,
            Some(JlcType::Extended) => s.extended += 1,
            None => s.unknown += 1,
        }
        s.out_of_stock += a.out_of_stock() as usize;
        match a.lifecycle {
            Some(Lifecycle::Obsolete | Lifecycle::LastTimeBuy) => s.obsolete += 1,
            Some(Lifecycle::Nrnd) => s.nrnd += 1,
            _ => {}
        }
    }
    s
}

impl AssemblySummary {
    /// "12 basic, 3 extended, 1 out of stock at JLC, 1 obsolete"; zero counts are left out. Obsolete
    /// includes last-time-buy. No setup fee is quoted: JLC's changes and the app has no source for it.
    pub fn text(&self) -> String {
        let parts = [
            (self.basic, "basic"),
            (self.preferred, "preferred"),
            (self.extended, "extended"),
            (self.out_of_stock, "out of stock at JLC"),
            (self.obsolete, "obsolete or last-time-buy"),
            (self.nrnd, "NRND"),
            (self.unknown, "not known at JLC"),
        ];
        parts.iter().filter(|p| p.0 > 0).map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(", ")
    }
}

pub fn jlc_label(a: &Assembly) -> &'static str {
    match a.jlc {
        Some(JlcType::Basic) => "basic",
        Some(JlcType::Preferred) => "preferred",
        Some(JlcType::Extended) => "extended",
        None => "",
    }
}

pub fn lifecycle_label(l: Option<Lifecycle>) -> &'static str {
    match l {
        Some(Lifecycle::Active) => "active",
        Some(Lifecycle::Nrnd) => "NRND",
        Some(Lifecycle::LastTimeBuy) => "last-time buy",
        Some(Lifecycle::Obsolete) => "obsolete",
        None => "",
    }
}

// ---- table, totals, CSV ----

#[derive(Clone, Debug)]
pub struct Row {
    pub line: Line,
    pub status: Status,
    pub alternatives: Vec<Alternative>,
    pub quotes: Vec<Quote>,
    pub best: Option<Best>,
    pub assembly: Assembly,
}

pub fn build_rows(lines: Vec<Line>, lib: &[LibPart], store: Option<&Store>, quotes: Vec<Vec<Quote>>, boards: u64, target: &str, rates: Option<&Rates>) -> Vec<Row> {
    lines
        .into_iter()
        .zip(quotes)
        .map(|(line, quotes)| Row {
            status: match_line(&line, lib),
            alternatives: store
                .and_then(|s| alternatives::alternatives_of(s, &line.mpn).ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|a| !same_mpn(&a.mpn, &line.mpn))
                .collect(),
            best: best(&quotes, line.qty() * boards, target, rates),
            assembly: assembly(&quotes),
            line,
            quotes,
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub lines: usize,
    /// Lines with at least one usable price.
    pub priced: usize,
    /// Cost of one board's parts at the order volume's prices.
    pub unit_cost: f64,
    /// Everything bought for the order, including MOQ and price-break extras.
    pub extended_cost: f64,
    pub longest_lead_days: Option<u32>,
    /// Priced lines whose lead time the source didn't say.
    pub unknown_lead: usize,
}

pub fn totals(rows: &[Row]) -> Totals {
    let mut t = Totals { lines: rows.len(), ..Totals::default() };
    for r in rows {
        let Some(b) = &r.best else { continue };
        t.priced += 1;
        t.unit_cost += b.priced.unit * r.line.qty() as f64;
        t.extended_cost += b.priced.ext;
        match b.lead_days {
            Some(d) => t.longest_lead_days = t.longest_lead_days.max(Some(d)),
            None => t.unknown_lead += 1,
        }
    }
    t
}

/// Text cell safe to open in a spreadsheet: a leading `=`, `+`, `-`, `@`, tab or CR would run as a formula.
fn text_cell(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

pub fn library_label(s: &Status) -> String {
    match s {
        Status::NoPartNumber => "no part number".into(),
        Status::Found { has_footprint: true, .. } => "in library".into(),
        Status::Found { .. } => "no footprint".into(),
        Status::NotInLibrary => "missing".into(),
        Status::OtherMaker(m) => format!("only from {m}"),
        Status::Close(m) => format!("close to {m}"),
    }
}

pub fn to_csv(rows: &[Row], boards: u64, target: &str) -> String {
    let mut out = format!("Refs,Qty,Value,Footprint,MPN,Manufacturer,Library,Alternatives,Source,Supplier no.,Stock,Unit price ({target}),Extended ({target}),Quantity bought,Lead time (days),JLC class,JLC stock,Lifecycle\n");
    for r in rows {
        let (src, id, stock, unit, ext, bought, lead) = match &r.best {
            Some(b) => {
                let q = &r.quotes[b.quote];
                (
                    q.source.clone(),
                    q.id.clone(),
                    q.stock.map_or(String::new(), |s| s.to_string()),
                    format!("{:.4}", b.priced.unit),
                    format!("{:.2}", b.priced.ext),
                    b.priced.bought.to_string(),
                    b.lead_days.map_or(String::new(), |d| d.to_string()),
                )
            }
            None => Default::default(),
        };
        let alts: Vec<String> = r.alternatives.iter().map(|a| format!("{} ({})", a.mpn, a.role)).collect();
        let cells = [
            text_cell(&r.line.refs.join(" ")),
            (r.line.qty() * boards).to_string(),
            text_cell(&r.line.value),
            text_cell(&r.line.footprint),
            text_cell(&r.line.mpn),
            text_cell(&r.line.manufacturer),
            text_cell(&library_label(&r.status)),
            text_cell(&alts.join("; ")),
            text_cell(&src),
            text_cell(&id),
            stock,
            unit,
            ext,
            bought,
            lead,
            jlc_label(&r.assembly).into(),
            r.assembly.jlc_stock.map_or(String::new(), |s| s.to_string()),
            lifecycle_label(r.assembly.lifecycle).into(),
        ];
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

pub fn lib_parts(roots: &[crate::library::LibraryRoot]) -> Vec<LibPart> {
    roots.iter().flat_map(alternatives::scan).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "\u{feff}Reference,Value,Footprint,DNP,MPN,Manufacturer,LCSC,DigiKey,Mouser\n\
R1,10k,R_0603,,RC0603FR-0710KL,Yageo,C25804,,\n\
R10,10k,R_0603,,RC0603FR-0710KL,Yageo,C25804,,\n\
R2,10k,R_0603,,rc0603fr0710kl,Yageo,,,\n\
R3,1k,R_0603,DNP,,,,,\n\
U1,LM358,SOIC-8,,LM358DR,\"Texas, Instruments\",,296-1395-5-ND,\n\
H1,,,,,,,,\n\
C1,100n,C_0603,,,,,,\n";

    #[test]
    fn groups_ungrouped_export_and_skips_dnp_and_holes() {
        let l = parse_bom(CSV);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].refs, ["C1"]);
        assert_eq!(l[1].refs, ["R1", "R2", "R10"]);
        assert_eq!((l[1].qty(), l[1].supplier_ids.clone()), (3, vec![("LCSC".to_string(), "C25804".to_string())]));
        assert_eq!(l[2].manufacturer, "Texas, Instruments");
        // A supplier number on only the later placed part still counts; another maker is another line.
        let g = parse_bom("Reference,Value,Footprint,MPN,Manufacturer,LCSC\nR1,a,f,X1,A,\nR2,a,f,X1,A,C9\nR3,a,f,X1,B,\n");
        assert_eq!((g.len(), g[0].supplier_ids.clone()), (2, vec![("LCSC".to_string(), "C9".to_string())]));
        assert_eq!(l[2].supplier_ids, vec![("DigiKey".to_string(), "296-1395-5-ND".to_string())]);
    }

    #[test]
    fn csv_handles_quotes_and_newlines() {
        assert_eq!(parse_csv("a,\"b \"\"q\"\"\nx\",c\r\n1,2,3"), vec![vec!["a", "b \"q\"\nx", "c"], vec!["1", "2", "3"]]);
    }

    fn lib(mpn: &str, maker: &str, fp: &str, ids: &[(&str, &str)]) -> LibPart {
        LibPart {
            category: "R".into(),
            symbol: mpn.into(),
            mpn: mpn.into(),
            manufacturer: maker.into(),
            value: String::new(),
            footprint: fp.into(),
            supplier_ids: ids.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        }
    }

    #[test]
    fn matching() {
        let l = vec![lib("RC0603FR-0710KL", "Yageo", "R:R_0603", &[]), lib("BC847", "Nexperia", "", &[]), lib("LM358", "TI", "S:SOIC", &[("LCSC", "C7950")])];
        let line = |mpn: &str, maker: &str, ids: &[(&str, &str)]| Line {
            mpn: mpn.into(),
            manufacturer: maker.into(),
            supplier_ids: ids.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            ..Line::default()
        };
        assert_eq!(match_line(&line("rc0603fr0710kl", "", &[]), &l), Status::Found { symbol: "RC0603FR-0710KL".into(), has_footprint: true });
        assert!(match_line(&line("BC847", "", &[]), &l).missing_cad(), "symbol without footprint");
        assert_eq!(match_line(&line("BC847", "onsemi", &[]), &l), Status::OtherMaker("Nexperia".into()));
        assert!(matches!(match_line(&line("", "", &[("LCSC", "c7950")]), &l), Status::Found { .. }), "by supplier number");
        assert_eq!(match_line(&line("LM358DR", "", &[]), &l), Status::Close("LM358".into()));
        assert_eq!(match_line(&line("XYZ12345", "", &[]), &l), Status::NotInLibrary);
        assert_eq!(match_line(&line("", "", &[]), &l), Status::NoPartNumber);
    }

    fn quote(src: &str, breaks: &[(u64, f64)], cur: &str, stock: u64, moq: u64, lead: Option<u32>) -> Quote {
        Quote { source: src.into(), breaks: breaks.to_vec(), currency: cur.into(), stock: Some(stock), moq, lead_days: lead, ..Quote::default() }
    }

    #[test]
    fn price_breaks_moq_and_next_break() {
        let q = quote("D", &[(1, 0.10), (10, 0.05), (100, 0.01)], "USD", 1_000_000, 1, None);
        assert_eq!(price(&q, 5).map(|p| (p.bought, p.unit)), Some((5, 0.10)));
        assert_eq!(price(&q, 10).map(|p| (p.bought, p.ext)), Some((10, 0.5)));
        // 60 at 0.05 = 3.00, but 100 at 0.01 = 1.00.
        assert_eq!(price(&q, 60).map(|p| (p.bought, p.unit)), Some((100, 0.01)));
        let reel = quote("D", &[(5000, 0.002)], "USD", 9, 5000, None);
        let p = price(&reel, 20).unwrap();
        assert_eq!((p.bought, p.in_stock), (5000, false), "MOQ is bought, and stock 9 doesn't cover it");
        assert!((p.ext - 10.0).abs() < 1e-9);
        assert!(price(&Quote::default(), 1).is_none());
    }

    #[test]
    fn rounding_up_to_a_break_stays_within_stock() {
        let q = quote("D", &[(1, 0.10), (100, 0.01)], "USD", 80, 1, None);
        let p = price(&q, 60).unwrap();
        assert_eq!((p.bought, p.in_stock), (60, true), "100 would exceed the 80 in stock");
    }

    #[test]
    fn best_prefers_stock_converts_and_drops_unconvertible() {
        let rates = Rates::new("d", &[("USD", 1.0), ("EUR", 0.5)]);
        let q = vec![
            quote("A", &[(1, 0.01)], "USD", 0, 1, Some(70)), // cheapest, no stock
            quote("B", &[(1, 0.10)], "EUR", 100, 1, None),   // 0.20 USD, in stock
            quote("C", &[(1, 0.001)], "CHF", 100, 1, None),  // no rate
        ];
        let b = best(&q, 10, "USD", Some(&rates)).unwrap();
        assert_eq!(b.quote, 1);
        assert!((b.priced.unit - 0.20).abs() < 1e-9);
        assert_eq!(b.lead_days, Some(0));
        let b = best(&q, 10, "USD", None).unwrap();
        assert_eq!((b.quote, b.lead_days), (0, Some(70)), "EUR needs rates; offline falls back to USD");
        assert!(best(&q[2..], 10, "USD", Some(&rates)).is_none());
    }

    #[test]
    fn totals_and_csv() {
        let rows = build_rows(
            parse_bom(CSV),
            &[lib("RC0603FR-0710KL", "Yageo", "R:R_0603", &[])],
            None,
            vec![vec![], vec![quote("LCSC", &[(1, 0.10), (10, 0.05)], "USD", 5000, 1, None)], vec![quote("DigiKey", &[(1, 1.0)], "USD", 0, 1, Some(14))]],
            10,
            "USD",
            None,
        );
        let t = totals(&rows);
        assert_eq!((t.lines, t.priced, t.unknown_lead, t.longest_lead_days), (3, 2, 0, Some(14)));
        assert!((t.unit_cost - (3.0 * 0.05 + 1.0)).abs() < 1e-9, "{t:?}");
        assert!((t.extended_cost - (30.0 * 0.05 + 10.0)).abs() < 1e-9);
        let csv = to_csv(&rows, 10, "USD");
        assert!(csv.lines().next().unwrap().starts_with("Refs,Qty"));
        assert!(csv.contains("R1 R2 R10,30,10k,R_0603,RC0603FR-0710KL,Yageo,in library,,LCSC,"), "{csv}");
        assert!(csv.contains("\"Texas, Instruments\""));
        assert_eq!(text_cell("=1+1"), "'=1+1");
        assert_eq!(text_cell("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn picks_exact_mpn_and_maker() {
        let p = |mpn: &str, maker: &str, ids: &[(&str, &str)]| Part {
            mpn: mpn.into(),
            manufacturer: maker.into(),
            supplier_ids: ids.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            ..Part::default()
        };
        let r = vec![p("LM358DR", "TI", &[]), p("LM358", "onsemi", &[]), p("LM358", "TI", &[])];
        let line = Line { mpn: "lm358".into(), manufacturer: "ti".into(), ..Line::default() };
        let got = pick(&r, &line, "Mouser").unwrap();
        assert_eq!((got.mpn.as_str(), got.manufacturer.as_str()), ("LM358", "TI"));
        let by_id = Line { supplier_ids: vec![("LCSC".into(), "C1".into())], ..Line::default() };
        assert!(pick(&[p("X", "", &[("LCSC", "c1")])], &by_id, "LCSC").is_some());
    }

    struct Fake(std::sync::atomic::AtomicUsize, bool);
    impl Source for Fake {
        fn name(&self) -> &'static str {
            "LCSC"
        }
        fn search(&self, q: &str) -> Result<Vec<Part>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.1 {
                bail!("offline");
            }
            Ok(vec![Part { mpn: q.into(), stock: Some(9), unit_price: "$0.5 @1".into(), ..Part::default() }])
        }
        fn fetch(&self, _: &Part, _: &Path) -> Result<Option<Part>> {
            Ok(None)
        }
    }

    #[test]
    fn fetch_dedupes_and_falls_back_to_stale_cache() {
        let store = Store::open(&crate::test_dir("bom_fetch").join("parts.db")).unwrap();
        let line = |r: &str| Line { refs: vec![r.into()], mpn: "ABC12345".into(), ..Line::default() };
        let lines = [line("R1"), Line { footprint: "other".into(), ..line("R2") }];
        let d = |fail| vec![Distributor { cache_name: "LCSC/v2".into(), source: Box::new(Fake(Default::default(), fail)), interval: Default::default() }];
        let (q, problems) = fetch_quotes(&lines, &d(false), Some(&store), 60);
        assert!(problems.is_empty() && q[0][0].breaks == [(1, 0.5)] && q[1].len() == 1);
        // Expired (ttl 0 skips the cache) and offline: the old entry is used and marked stale.
        let (q, problems) = fetch_quotes(&lines, &d(true), Some(&store), 0);
        assert!(q[0][0].stale && problems.len() == 1, "{problems:?}");
        assert!(q[1][0].stale, "second line shares the cached query");
        // Offline with nothing cached: no quote, a problem, no panic.
        let (q, problems) = fetch_quotes(&lines, &d(true), None, 60);
        assert!(q[0].is_empty() && problems.len() == 1);
    }

    #[test]
    fn mouser_order_multiple_rounds_up() {
        let mut q = quote("Mouser", &[(1, 0.10), (100, 0.05)], "USD", 10_000, 1, None);
        q.multiple = 50;
        assert_eq!(price(&q, 7).map(|p| p.bought), Some(50));
        // 120 rounds to 150; 150 x 0.05 = 7.5 beats nothing cheaper.
        assert_eq!(price(&q, 120).map(|p| (p.bought, p.unit)), Some((150, 0.05)));
        let p = Part { pack_qty: Some(50), ..Part::default() };
        assert_eq!((quote_of("Mouser", &p).multiple, quote_of("DigiKey", &p).multiple), (50, 1));
    }

    #[test]
    fn requests_are_spaced_by_the_interval_but_cache_hits_are_not() {
        let store = Store::open(&crate::test_dir("bom_rate").join("parts.db")).unwrap();
        let line = |m: &str| Line { refs: vec!["R1".into()], mpn: m.into(), ..Line::default() };
        let lines = [line("AAAAA1"), line("BBBBB2"), line("AAAAA1")];
        let d = vec![Distributor { cache_name: "LCSC/v2".into(), source: Box::new(Fake(Default::default(), false)), interval: std::time::Duration::from_millis(150) }];
        let t = std::time::Instant::now();
        fetch_quotes(&lines, &d, Some(&store), 60);
        let e = t.elapsed();
        assert!(e >= std::time::Duration::from_millis(150) && e < std::time::Duration::from_millis(290), "two requests, one wait: {e:?}");
    }

    #[test]
    fn every_packaging_is_quoted() {
        let reel = Part { packaging: "Reel".into(), moq: Some(5000), price_breaks: vec![(5000, 0.002)], ..Part::default() };
        let cut = Part { packaging: "Cut Tape".into(), price_breaks: vec![(1, 0.01)], variations: vec![reel], ..Part::default() };
        let store = Store::open(&crate::test_dir("bom_pack").join("parts.db")).unwrap();
        struct One(Part);
        impl Source for One {
            fn name(&self) -> &'static str {
                "LCSC"
            }
            fn search(&self, _: &str) -> Result<Vec<Part>> {
                Ok(vec![Part { mpn: "ABC12345".into(), ..self.0.clone() }])
            }
            fn fetch(&self, _: &Part, _: &Path) -> Result<Option<Part>> {
                Ok(None)
            }
        }
        let d = vec![Distributor { cache_name: "x".into(), source: Box::new(One(cut)), interval: Default::default() }];
        let (q, _) = fetch_quotes(&[Line { refs: vec!["R1".into()], mpn: "ABC12345".into(), ..Line::default() }], &d, Some(&store), 60);
        assert_eq!(q[0].iter().map(|q| q.packaging.as_str()).collect::<Vec<_>>(), ["Cut Tape", "Reel"]);
        // For 10 parts the cut tape wins; best() weighs every packaging.
        assert_eq!(best(&q[0], 10, "", None).map(|b| b.quote), Some(0));
    }

    #[test]
    fn field_names_are_configurable_with_standard_defaults() {
        let f = BomFields { mpn: "Part Number".into(), lcsc: " ".into(), ..BomFields::default() };
        assert_eq!(f.names(), ["Part Number", "Manufacturer", "LCSC", "DigiKey", "Mouser"]);
        let p: ProjectBom = serde_json::from_str(r#"{"fields":{"mpn":"PN"}}"#).unwrap();
        assert_eq!(p.fields.unwrap().names()[..2], ["PN", "Manufacturer"]);
        assert!(ProjectBom::default().fields.is_none());
    }

    #[test]
    fn project_region_overrides_default() {
        let mut p = ProjectBom::default();
        assert_eq!(p.region("DE"), "DE");
        p.region = "US".into();
        assert_eq!(p.region("DE"), "US");
    }

    fn parts_of(json: &str, f: fn(&serde_json::Value, &str) -> Vec<Part>, q: &str) -> Vec<Part> {
        f(&serde_json::from_str(json).unwrap(), q)
    }

    #[test]
    fn assembly_from_saved_lcsc_and_digikey_replies() {
        let lcsc = parts_of(include_str!("../tests/fixtures/live/jlcpcb_search_LM358.json"), |b, q| sources::lcsc::parse_search(b, q), "LM358");
        let quote_for = |id: &str| quote_of("LCSC", lcsc.iter().find(|p| p.supplier_ids[0].1 == id).unwrap());
        // C5213: extended with 4 in stock. The same search has a basic part too.
        let a = assembly(&[quote_for("C5213")]);
        assert_eq!((a.jlc, a.jlc_stock, a.lifecycle), (Some(JlcType::Extended), Some(4), None));
        let basic = lcsc.iter().find(|p| p.jlc_type == "basic").expect("the reply has a basic part");
        assert_eq!(assembly(&[quote_of("LCSC", basic)]).jlc, Some(JlcType::Basic));
        // Out of stock and preferred come from the same two fields.
        let mut oos = quote_for("C5213");
        (oos.stock, oos.jlc_type) = (Some(0), "preferred".into());
        let a = assembly(&[oos]);
        assert!(a.out_of_stock() && a.flagged() && a.jlc == Some(JlcType::Preferred));

        let dk = parts_of(include_str!("../tests/fixtures/live/digikey_search_LM358.json"), |b, _| sources::digikey::parse(b), "");
        assert_eq!(assembly(&[quote_of("DigiKey", &dk[0])]).lifecycle, Some(Lifecycle::Active));
        // DigiKey's lifecycle text, set the way its reply would for the other states.
        let status = |s: &str| assembly(&[Quote { source: "DigiKey".into(), lifecycle: s.into(), ..Quote::default() }]).lifecycle;
        assert_eq!(status("Obsolete"), Some(Lifecycle::Obsolete));
        assert_eq!(status("Not For New Designs"), Some(Lifecycle::Nrnd));
        assert_eq!(status("Last Time Buy"), Some(Lifecycle::LastTimeBuy));
        assert_eq!(status("Discontinued at Digi-Key"), None, "DigiKey dropping it says nothing about the maker");
        assert_eq!(status(""), None);
        assert_eq!(status("Inactive"), None, "not read as active");
        // No source knows: unknown, and not flagged.
        let none = assembly(&[]);
        assert_eq!((none.jlc, none.jlc_stock, none.lifecycle, none.flagged()), (None, None, None, false));
        // The worst lifecycle across sources wins.
        let q = |s: &str| Quote { lifecycle: s.into(), ..Quote::default() };
        assert_eq!(assembly(&[q("Active"), q("Obsolete")]).lifecycle, Some(Lifecycle::Obsolete));
    }

    #[test]
    fn assembly_summary_counts_and_text() {
        let row = |jlc, stock, lifecycle| Row {
            line: Line::default(),
            status: Status::NoPartNumber,
            alternatives: vec![],
            quotes: vec![],
            best: None,
            assembly: Assembly { jlc, jlc_stock: stock, lifecycle },
        };
        let rows = [
            row(Some(JlcType::Basic), Some(100), Some(Lifecycle::Active)),
            row(Some(JlcType::Basic), Some(5), None),
            row(Some(JlcType::Extended), Some(0), Some(Lifecycle::Obsolete)),
            row(Some(JlcType::Preferred), Some(9), Some(Lifecycle::Nrnd)),
            row(None, None, Some(Lifecycle::LastTimeBuy)),
        ];
        let s = assembly_summary(&rows);
        assert_eq!((s.basic, s.preferred, s.extended, s.out_of_stock, s.unknown, s.obsolete, s.nrnd), (2, 1, 1, 1, 1, 2, 1));
        assert_eq!(s.text(), "2 basic, 1 preferred, 1 extended, 1 out of stock at JLC, 2 obsolete or last-time-buy, 1 NRND, 1 not known at JLC");
        assert_eq!(assembly_summary(&[]).text(), "");
    }
}
