//! "Get…" from every source at once: each source's files for one MPN become a candidate set, and the
//! import dialog picks each file type from one of them.
//!
//! Default picks: symbol, footprint and 3D model all come from the most complete set (the first to
//! arrive wins a tie), so a symbol never meets a foreign footprint unless the user swaps one; the
//! datasheet is a real PDF from wherever there is one. A swapped symbol/footprint pair must pass the
//! pin/pad check before it can be imported.

use super::Source;
use crate::checks::{Issue, Severity, check_pin_pad_files};
use crate::download::download_pdf;
use crate::kicad_env::upgrade_symbols;
use crate::matching::same_mpn;
use crate::part::Part;
use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Symbol,
    Footprint,
    Model,
    Datasheet,
    Sim,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Symbol, Kind::Footprint, Kind::Model, Kind::Datasheet, Kind::Sim];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Symbol => "Symbol",
            Kind::Footprint => "Footprint",
            Kind::Model => "3D model",
            Kind::Datasheet => "Datasheet",
            Kind::Sim => "Sim model",
        }
    }

    fn has(self, p: &Part) -> bool {
        match self {
            Kind::Symbol => p.symbol_file.is_some(),
            Kind::Footprint => !p.footprint_files.is_empty(),
            Kind::Model => !p.model_files.is_empty(),
            Kind::Datasheet => p.datasheet_file.is_some(),
            Kind::Sim => !p.sim_files.is_empty(),
        }
    }
}

/// Every source's files for one part, and which set each file type is taken from.
#[derive(Default)]
pub struct Combined {
    /// Candidate sets in the order they arrived; `source` names each one.
    pub sets: Vec<Part>,
    pick: [Option<usize>; 5],
    /// The user changed a pick, so later arrivals no longer reset them.
    edited: bool,
    mismatch: Option<String>,
}

impl Combined {
    /// Takes one source's answer: its metadata is merged into `meta` (the part the dialog edits) and
    /// its files, if any, become a candidate set.
    pub fn add(&mut self, meta: &mut Part, found: Part) {
        merge_meta(meta, &found);
        if Kind::ALL.iter().any(|k| k.has(&found)) {
            self.sets.push(found);
            if !self.edited {
                self.reset();
            }
        }
    }

    /// Indexes into `sets` of the sets that have `kind`.
    pub fn candidates(&self, kind: Kind) -> Vec<usize> {
        (0..self.sets.len()).filter(|&i| kind.has(&self.sets[i])).collect()
    }

    pub fn picked(&self, kind: Kind) -> Option<usize> {
        self.pick[kind as usize]
    }

    pub fn choose(&mut self, kind: Kind, set: Option<usize>) {
        self.pick[kind as usize] = set.filter(|&i| self.sets.get(i).is_some_and(|p| kind.has(p)));
        self.edited = true;
        self.recheck();
    }

    /// Back to the default picks.
    pub fn reset(&mut self) {
        use Kind::*;
        // Symbol, footprint and model decide; datasheet and sim model only break a tie.
        let score = |p: &Part| 16 * Symbol.has(p) as u8 + 8 * Footprint.has(p) as u8 + 4 * Model.has(p) as u8 + Datasheet.has(p) as u8 + Sim.has(p) as u8;
        let cad = (0..self.sets.len()).filter(|&i| score(&self.sets[i]) > 0).max_by_key(|&i| (score(&self.sets[i]), std::cmp::Reverse(i)));
        let from_cad = |k: Kind| cad.filter(|&i| k.has(&self.sets[i]));
        let anywhere = |k: Kind| from_cad(k).or_else(|| self.candidates(k).first().copied());
        self.pick = [from_cad(Symbol), from_cad(Footprint), from_cad(Model), anywhere(Datasheet), anywhere(Sim)];
        self.edited = false;
        self.recheck();
    }

    /// Some set has a symbol, so the part can be imported.
    pub fn has_cad(&self) -> bool {
        !self.candidates(Kind::Symbol).is_empty()
    }

    /// Symbol and footprint come from different sets.
    fn mixed(&self) -> bool {
        matches!((self.picked(Kind::Symbol), self.picked(Kind::Footprint)), (Some(s), Some(f)) if s != f)
    }

    /// Why the chosen symbol and footprint can't be imported together, if they can't.
    pub fn mismatch(&self) -> Option<&str> {
        self.mismatch.as_deref()
    }

    fn recheck(&mut self) {
        self.mismatch = None;
        if !self.mixed() {
            return;
        }
        let (sym, fp) = (&self.sets[self.pick[0].unwrap()], &self.sets[self.pick[1].unwrap()]);
        let issues = match readable_symbol(sym.symbol_file.as_ref().unwrap()) {
            Ok(symbol) => check_pin_pad_files(&symbol, &fp.footprint_files[0]),
            Err(e) => vec![Issue { severity: Severity::Error, message: format!("The symbol can't be checked: {e:#}") }],
        };
        if !issues.is_empty() {
            let why: Vec<_> = issues.into_iter().map(|i| i.message).collect();
            self.mismatch = Some(format!("The symbol from {} and the footprint from {} can't be used together. {}.", sym.source, fp.source, why.join(". ")));
        }
    }

    /// `meta` with the picked files, ready for `LibraryRoot::add_part`.
    pub fn to_part(&self, meta: &Part) -> Part {
        let set = |k: Kind| self.picked(k).map(|i| &self.sets[i]);
        let mut p = meta.clone();
        p.symbol_file = set(Kind::Symbol).and_then(|s| s.symbol_file.clone());
        p.footprint_files = set(Kind::Footprint).map(|s| s.footprint_files.clone()).unwrap_or_default();
        // Only the first footprint was checked against the foreign symbol, so only it goes in.
        if self.mixed() {
            p.footprint_files.truncate(1);
        }
        p.model_files = set(Kind::Model).map(|s| s.model_files.clone()).unwrap_or_default();
        p.datasheet_file = set(Kind::Datasheet).and_then(|s| s.datasheet_file.clone());
        p.sim_files = set(Kind::Sim).map(|s| s.sim_files.clone()).unwrap_or_default();
        let mut sources: Vec<&str> = vec![];
        for s in Kind::ALL.into_iter().filter_map(set) {
            if !sources.contains(&s.source.as_str()) {
                sources.push(&s.source);
            }
        }
        p.source = sources.join(" + ");
        p
    }

    /// SnapMagic's page for the part, where the user can download CAD none of the sources had.
    pub fn cad_page_url(meta: &Part) -> Option<String> {
        super::snapmagic_url(meta)
    }
}

/// The symbol file in a format the checks can read: a legacy `.lib` is converted next to itself.
fn readable_symbol(symbol: &Path) -> Result<PathBuf> {
    if !symbol.extension().is_some_and(|e| e.eq_ignore_ascii_case("lib")) {
        return Ok(symbol.to_path_buf());
    }
    let upgraded = symbol.with_extension("checked.kicad_sym");
    upgrade_symbols(symbol, &upgraded)?;
    Ok(upgraded)
}

/// Supplier numbers from every source; otherwise the first value found, except that a distributor's
/// longer description beats a shorter one (ZIP authors' text only fills a gap).
fn merge_meta(meta: &mut Part, found: &Part) {
    for (to, from) in
        [(&mut meta.manufacturer, &found.manufacturer), (&mut meta.datasheet_url, &found.datasheet_url), (&mut meta.category_hint, &found.category_hint)]
    {
        if to.is_empty() {
            to.clone_from(from);
        }
    }
    let from_zip = found.source.starts_with("ZIP");
    if meta.description.is_empty() || (!from_zip && found.description.len() > meta.description.len()) {
        meta.description.clone_from(&found.description);
    }
    for id in &found.supplier_ids {
        if !meta.supplier_ids.iter().any(|(s, _)| *s == id.0) {
            meta.supplier_ids.push(id.clone());
        }
    }
}

/// Asks every source for `chosen`'s MPN at once and calls `answer` as each replies: its part, with
/// any files under `work/<source name>`, or `None` if it has no part with that MPN. `from` is the
/// source `chosen` came from; that one is fetched directly instead of searched again.
pub fn gather(sources: &[Box<dyn Source>], chosen: &Part, from: usize, work: &Path, answer: impl Fn(&str, Result<Option<Part>>) + Sync) {
    std::thread::scope(|s| {
        let answer = &answer;
        let handles: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, src)| (src.name(), s.spawn(move || answer(src.name(), ask(src.as_ref(), chosen, i == from, &work.join(src.name()))))))
            .collect();
        for (name, h) in handles {
            if h.join().is_err() {
                answer(name, Err(anyhow!("crashed")));
            }
        }
    });
}

fn ask(src: &dyn Source, chosen: &Part, is_chosen: bool, work: &Path) -> Result<Option<Part>> {
    let found = if is_chosen {
        chosen.clone()
    } else {
        match src.search(&chosen.mpn)?.into_iter().find(|p| same_mpn(&p.mpn, &chosen.mpn)) {
            Some(p) => p,
            None => return Ok(None),
        }
    };
    let mut part = match src.fetch(&found, work) {
        Ok(Some(p)) => p,
        Ok(None) => {
            let mut p = found;
            // Distributors only link the datasheet; a PDF behind the link makes them a datasheet candidate.
            if p.datasheet_file.is_none() && !p.datasheet_url.is_empty() {
                let pdf = work.join("datasheet.pdf");
                if std::fs::create_dir_all(work).is_ok() && download_pdf(&p.datasheet_url, &pdf).is_ok() {
                    p.datasheet_file = Some(pdf);
                }
            }
            p
        }
        Err(e) => {
            let mut p = found;
            p.warnings.push(format!("no CAD files: {e:#}"));
            p
        }
    };
    if part.source.is_empty() {
        part.source = src.name().into();
    }
    Ok(Some(part))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const SYM: &str = r#"(kicad_symbol_lib (symbol "P1"
        (symbol "P1_1_1"
            (pin passive line (at 0 0 0) (length 2) (name "A") (number "1"))
            (pin passive line (at 0 2 0) (length 2) (name "B") (number "2"))
            (pin passive line (at 0 4 0) (length 2) (name "C") (number "3")))))"#;
    const FP: &str = r#"(footprint "FP" (layer "F.Cu")
        (pad "1" smd rect (at -1 0) (size 1 0.5) (layers "F.Cu"))
        (pad "2" smd rect (at 0 0) (size 1 0.5) (layers "F.Cu"))
        (pad "3" smd rect (at 1 0) (size 1 0.5) (layers "F.Cu")))"#;

    /// A candidate set from `source` with files for the kinds in `kinds` ("sfmds"), written to `dir`.
    fn set(dir: &Path, source: &str, kinds: &str, fp: &str) -> Part {
        let d = dir.join(source.replace([':', ' '], "_"));
        std::fs::create_dir_all(&d).unwrap();
        let file = |name: &str, text: &str| {
            let f = d.join(name);
            std::fs::write(&f, text).unwrap();
            f
        };
        Part {
            mpn: "P1".into(),
            source: source.into(),
            symbol_file: kinds.contains('s').then(|| file("p.kicad_sym", SYM)),
            footprint_files: if kinds.contains('f') { vec![file("p.kicad_mod", fp)] } else { vec![] },
            model_files: if kinds.contains('m') { vec![file("p.step", "ISO-10303-21;")] } else { vec![] },
            datasheet_file: kinds.contains('d').then(|| file("p.pdf", "%PDF")),
            sim_files: if kinds.contains('x') { vec![file("p.cir", ".subckt")] } else { vec![] },
            ..Part::default()
        }
    }

    fn picks(c: &Combined) -> Vec<Option<&str>> {
        Kind::ALL.iter().map(|&k| c.picked(k).map(|i| c.sets[i].source.as_str())).collect()
    }

    #[test]
    fn complete_set_wins_and_datasheet_comes_from_a_pdf() {
        let dir = crate::test_dir("combine_pick");
        let (mut c, mut meta) = (Combined::default(), Part { mpn: "P1".into(), ..Part::default() });
        c.add(&mut meta, set(&dir, "DigiKey", "d", FP));
        c.add(&mut meta, set(&dir, "LCSC", "sf", FP));
        assert_eq!(picks(&c), [Some("LCSC"), Some("LCSC"), None, Some("DigiKey"), None]);
        // A later, more complete set takes over symbol, footprint and model together.
        c.add(&mut meta, set(&dir, "ZIP: P1.zip", "sfmx", FP));
        assert_eq!(picks(&c), [Some("ZIP: P1.zip"), Some("ZIP: P1.zip"), Some("ZIP: P1.zip"), Some("DigiKey"), Some("ZIP: P1.zip")]);
        // An equally complete set arriving later doesn't, and the CAD set's own PDF beats another one.
        c.add(&mut meta, set(&dir, "Other", "sfmd", FP));
        assert_eq!(picks(&c)[0], Some("ZIP: P1.zip"));
        c.add(&mut meta, set(&dir, "Best", "sfmdx", FP));
        assert_eq!(picks(&c), [Some("Best"); 5]);
        assert!(c.mismatch().is_none());

        let p = c.to_part(&meta);
        assert_eq!(p.source, "Best");
        assert!(p.symbol_file.unwrap().starts_with(dir.join("Best")));
        assert_eq!(c.candidates(Kind::Symbol).len(), 4);
    }

    #[test]
    fn metadata_merges_from_every_source() {
        let mut meta = Part {
            mpn: "P1".into(),
            manufacturer: "TI".into(),
            description: "Op amp".into(),
            supplier_ids: vec![("DigiKey".into(), "296-1".into())],
            ..Part::default()
        };
        let mut c = Combined::default();
        let lcsc = Part {
            manufacturer: "Texas Instruments".into(),
            description: "Dual op amp, 8-SOIC".into(),
            supplier_ids: vec![("LCSC".into(), "C7950".into())],
            datasheet_url: "https://x/ds.pdf".into(),
            source: "LCSC".into(),
            ..Part::default()
        };
        let zip = Part {
            description: "OPERATIONAL AMPLIFIER, DUAL, VERY LONG VENDOR TEXT".into(),
            supplier_ids: vec![("DigiKey".into(), "other".into())],
            source: "ZIP: P1.zip".into(),
            ..Part::default()
        };
        c.add(&mut meta, lcsc);
        c.add(&mut meta, zip);
        assert_eq!(meta.manufacturer, "TI", "the chosen result's manufacturer stays");
        assert_eq!(meta.description, "Dual op amp, 8-SOIC", "longer distributor text wins, ZIP text doesn't");
        assert_eq!(meta.datasheet_url, "https://x/ds.pdf");
        assert_eq!(meta.supplier_ids, vec![("DigiKey".into(), "296-1".into()), ("LCSC".into(), "C7950".into())]);
        assert!(c.sets.is_empty(), "metadata-only answers are not candidate sets");
    }

    #[test]
    fn swapped_footprint_that_mismatches_is_refused() {
        let dir = crate::test_dir("combine_mismatch");
        let other_fp = FP.replace(r#"(pad "3""#, r#"(pad "4""#);
        let (mut c, mut meta) = (Combined::default(), Part::default());
        c.add(&mut meta, set(&dir, "LCSC", "sfm", FP));
        c.add(&mut meta, set(&dir, "ZIP: P1.zip", "f", &other_fp));
        assert!(c.mismatch().is_none());

        c.choose(Kind::Footprint, Some(1));
        let why = c.mismatch().expect("pins 3 and pad 4 don't line up");
        assert!(why.contains("LCSC") && why.contains("ZIP: P1.zip"), "{why}");
        assert!(why.contains("pin(s) 3 have no pad") && why.contains("pad(s) 4 have no pin"), "{why}");

        // A later set doesn't undo the user's swap; "Switch back" does.
        c.add(&mut meta, set(&dir, "Other", "sfm", FP));
        assert_eq!(c.picked(Kind::Footprint), Some(1));
        c.reset();
        assert!(c.mismatch().is_none());
        assert_eq!(picks(&c)[..2], [Some("LCSC"), Some("LCSC")]);
    }

    #[test]
    fn unconvertible_legacy_symbol_is_refused_with_the_reason() {
        let dir = crate::test_dir("combine_legacy");
        let (mut c, mut meta) = (Combined::default(), Part::default());
        let mut zip = set(&dir, "ZIP: P1.zip", "s", FP);
        let lib = dir.join("ZIP__P1.zip/p.lib");
        std::fs::write(
            &lib,
            "EESCHEMA-LIBRARY Version 2.4
not a library
",
        )
        .unwrap();
        zip.symbol_file = Some(lib);
        c.add(&mut meta, zip);
        c.add(&mut meta, set(&dir, "LCSC", "f", FP));
        c.choose(Kind::Footprint, Some(1));
        assert!(c.mismatch().is_some_and(|m| m.contains("symbol can't be checked")), "{:?}", c.mismatch());
    }

    #[test]
    fn swapped_footprint_that_matches_is_allowed_alone() {
        let dir = crate::test_dir("combine_mix_ok");
        let (mut c, mut meta) = (Combined::default(), Part::default());
        c.add(&mut meta, set(&dir, "LCSC", "sf", FP));
        let mut zip = set(&dir, "ZIP: P1.zip", "f", FP);
        zip.footprint_files.push(PathBuf::from("variant.kicad_mod"));
        c.add(&mut meta, zip);
        c.choose(Kind::Footprint, Some(1));
        assert!(c.mismatch().is_none());
        assert_eq!(c.to_part(&meta).footprint_files.len(), 1, "only the checked footprint is imported");
        assert_eq!(c.to_part(&meta).source, "LCSC + ZIP: P1.zip");
    }

    /// Answers like a source would: `hits` are its search results, `cad` the kinds its fetch returns.
    struct Fake {
        name: &'static str,
        hits: Vec<Part>,
        cad: &'static str,
    }

    impl Source for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn search(&self, _: &str) -> Result<Vec<Part>> {
            Ok(self.hits.clone())
        }
        fn fetch(&self, part: &Part, work: &Path) -> Result<Option<Part>> {
            if self.cad.is_empty() {
                return Ok(None);
            }
            // Sources leave `source` empty for gather to fill in.
            Ok(Some(Part { source: String::new(), supplier_ids: part.supplier_ids.clone(), ..set(work, self.name, self.cad, FP) }))
        }
    }

    fn fake(name: &'static str, hits: &[(&str, &str)], cad: &'static str) -> Box<dyn Source> {
        let hits = hits.iter().map(|(mpn, id)| Part { mpn: (*mpn).into(), supplier_ids: vec![(name.into(), (*id).into())], ..Part::default() }).collect();
        Box::new(Fake { name, hits, cad })
    }

    #[test]
    fn gather_asks_every_source_and_a_late_zip_joins() {
        let dir = crate::test_dir("combine_gather");
        let sources = vec![fake("Mouser", &[("P1", "M-1")], ""), fake("LCSC", &[("P10", "C9"), ("p-1", "C1")], "sfm"), fake("Nexar", &[("Q2", "N")], "sfm")];
        let chosen = Part { mpn: "P1".into(), manufacturer: "Acme".into(), supplier_ids: vec![("Mouser".into(), "M-1".into())], ..Part::default() };
        let answers = Mutex::new(vec![]);
        gather(&sources, &chosen, 0, &dir, |name, found| answers.lock().unwrap().push((name.to_string(), found.unwrap())));

        let mut answers = answers.into_inner().unwrap();
        answers.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(answers.iter().map(|a| (a.0.as_str(), a.1.is_some())).collect::<Vec<_>>(), [("LCSC", true), ("Mouser", true), ("Nexar", false)]);
        let (mut c, mut meta) = (Combined::default(), chosen.clone());
        for (_, found) in answers {
            c.add(&mut meta, found.unwrap_or_default());
        }
        assert_eq!(c.sets.len(), 1, "Mouser had no files");
        assert_eq!(picks(&c)[..3], [Some("LCSC"); 3]);
        assert!(c.sets[0].symbol_file.as_ref().unwrap().starts_with(dir.join("LCSC")));
        assert_eq!(meta.supplier_ids, vec![("Mouser".into(), "M-1".into()), ("LCSC".into(), "C1".into())], "matched p-1, not P10");

        // A vendor ZIP downloaded afterwards is one more set, and its fuller set takes over.
        c.add(&mut meta, set(&dir, "ZIP: P1.zip", "sfmdx", FP));
        assert_eq!(c.sets.len(), 2);
        assert_eq!(picks(&c), [Some("ZIP: P1.zip"); 5]);
    }

    #[test]
    fn chosen_source_is_not_searched_again() {
        let dir = crate::test_dir("combine_chosen");
        // LCSC finds nothing when searched, so an answer with files means it was fetched directly.
        let sources = vec![fake("LCSC", &[], "sf"), fake("DigiKey", &[], "")];
        let chosen = Part { mpn: "P1".into(), ..Part::default() };
        let got = Mutex::new(vec![]);
        gather(&sources, &chosen, 0, &dir, |n, f| got.lock().unwrap().push((n.to_string(), f.unwrap().is_some())));
        let mut got = got.into_inner().unwrap();
        got.sort();
        assert_eq!(got, [("DigiKey".to_string(), false), ("LCSC".to_string(), true)]);
    }
}
