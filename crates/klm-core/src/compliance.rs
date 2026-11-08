//! Is a part up to the house standard? Compliant means the active profile's Standardise plan has nothing to
//! change for it and `checks::check_part` (fab check included, when a fab is chosen) finds nothing at all.
//!
//! The plan and the checks work on a whole library, so `Cache` re-runs them for the whole library when the
//! mtime of any part's symbol library, footprint or model (or the profile) differs from the last run.

use crate::checks::{self, Severity};
use crate::health::Finding;
use crate::library::{LibraryRoot, property, sanitize};
use crate::rules::Rules;
use crate::sexpr;
use crate::standardise::{self, profile::Profile};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub type Key = (String, String);

#[derive(Clone, Debug, PartialEq)]
pub struct Compliance {
    pub ok: bool,
    /// One line per plan change and per finding; empty when compliant.
    pub reasons: Vec<String>,
}

/// Compliance of every part in `root` for `profile`, keyed by (category, symbol name). Reads files only.
pub fn compute(root: &LibraryRoot, profile: &Profile, naming: &Rules) -> HashMap<Key, Compliance> {
    let plan = standardise::plan(root, profile, naming);
    let mut out = HashMap::new();
    for (key, issues) in checks::check_library(root) {
        let (cat, name) = &key;
        let mut reasons = vec![];
        let fp = footprint_name(root, cat, name);
        let prefix = format!("{name}: ");
        for f in &plan.files {
            let own_symbols = f.path == symbol_lib(root, cat);
            let own_footprint = fp.as_ref().is_some_and(|n| f.path == footprint_path(root, cat, n));
            reasons.extend(f.changes.iter().filter(|c| own_footprint || (own_symbols && c.starts_with(&prefix))).map(|c| format!("Standardise: {c}")));
        }
        reasons.extend(issues.iter().map(|i| format!("{}: {}", if i.severity == Severity::Error { "Error" } else { "Warning" }, i.message)));
        out.insert(key, Compliance { ok: reasons.is_empty(), reasons });
    }
    out
}

/// One health finding per non-compliant part, for the `health` report.
pub fn findings(results: &HashMap<Key, Compliance>) -> Vec<Finding> {
    let mut bad: Vec<_> = results.iter().filter(|(_, c)| !c.ok).collect();
    bad.sort_by(|a, b| a.0.cmp(b.0));
    bad.into_iter().map(|((cat, name), c)| Finding { severity: Severity::Warning, kind: "compliance", item: format!("{cat}/{name}"), message: c.reasons.join("; ") }).collect()
}

fn symbol_lib(root: &LibraryRoot, cat: &str) -> PathBuf {
    root.dir.join("symbols").join(format!("{}.kicad_sym", sanitize(cat)))
}

fn footprint_path(root: &LibraryRoot, cat: &str, name: &str) -> PathBuf {
    root.dir.join("footprints").join(format!("{cat}.pretty")).join(format!("{name}.kicad_mod"))
}

fn footprint_name(root: &LibraryRoot, cat: &str, symbol: &str) -> Option<String> {
    let lib = std::fs::read_to_string(symbol_lib(root, cat)).ok().and_then(|t| sexpr::parse(&t).ok())?;
    let sym = lib.children("symbol").find(|s| s.arg(0) == Some(symbol))?;
    Some(property(sym, "Footprint")?.rsplit(':').next()?.to_string())
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The first `(model "…")` path of a footprint, found as text (a full parse isn't needed for an mtime).
fn model_path(root: &LibraryRoot, footprint: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(footprint).ok()?;
    let rest = text.split_once("(model")?.1.trim_start().strip_prefix('"')?;
    Some(PathBuf::from(rest.split('"').next()?.replace("${KIPRJMOD}", &root.table_dir.to_string_lossy())))
}

type Stamp = Vec<Option<SystemTime>>;

/// mtimes of every part's symbol library, footprint and model.
fn stamps(root: &LibraryRoot) -> HashMap<Key, Stamp> {
    let mut out = HashMap::new();
    for cat in root.categories() {
        let sym_path = symbol_lib(root, &cat);
        let Some(lib) = std::fs::read_to_string(&sym_path).ok().and_then(|t| sexpr::parse(&t).ok()) else { continue };
        let sym_time = mtime(&sym_path);
        for sym in lib.children("symbol") {
            let name = sym.arg(0).unwrap_or_default().to_string();
            let fp = property(sym, "Footprint").and_then(|f| f.rsplit(':').next()).map(|n| footprint_path(root, &cat, n));
            let model = fp.as_ref().and_then(|p| model_path(root, p));
            out.insert((cat.clone(), name), vec![sym_time, fp.as_deref().and_then(mtime), model.as_deref().and_then(mtime)]);
        }
    }
    out
}

/// Results kept between runs; see the module comment for when they are dropped.
#[derive(Default)]
pub struct Cache {
    dir: PathBuf,
    profile: u64,
    stamps: HashMap<Key, Stamp>,
    results: HashMap<Key, Compliance>,
    /// How many times the plan and checks actually ran (for tests).
    pub runs: usize,
}

impl Cache {
    pub fn get(&mut self, root: &LibraryRoot, profile: &Profile, naming: &Rules) -> &HashMap<Key, Compliance> {
        // The profile's JSON stands in for its identity: it also notices edits not yet saved to its file.
        let mut h = DefaultHasher::new();
        serde_json::to_string(profile).unwrap_or_default().hash(&mut h);
        let (profile_id, now) = (h.finish(), stamps(root));
        if self.runs == 0 || self.dir != root.dir || self.profile != profile_id || self.stamps != now {
            self.results = compute(root, profile, naming);
            (self.dir, self.profile, self.stamps) = (root.dir.clone(), profile_id, now);
            self.runs += 1;
        }
        &self.results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const SYM: &str = r#"(kicad_symbol_lib (version 20251024) (generator "t")
        (symbol "P1"
            (property "Reference" "U" (at 0 0 0))
            (property "Value" "P1" (at 0 0 0))
            (property "Footprint" "PRJ_Test:FP" (at 0 0 0))
            (property "MPN" "P1" (at 0 0 0))
            (property "Manufacturer" "Acme" (at 0 0 0))
            (symbol "P1_0_1" (rectangle (start 0 0) (end 1 1)))
            (symbol "P1_1_1"
                (pin passive line (at 0 0 0) (length 2) (name "A") (number "1"))
                (pin passive line (at 0 2 0) (length 2) (name "B") (number "2"))
                (pin passive line (at 0 4 0) (length 2) (name "C") (number "3")))))"#;

    const FP: &str = r#"(footprint "FP" (layer "F.Cu")
        (model "${KIPRJMOD}/lib/3dmodels/Test.3dshapes/p.step")
        (fp_line (start -1 -1) (end 1 -1) (layer "F.CrtYd"))
        (fp_line (start -1 -1) (end 1 -1) (layer "F.SilkS"))
        (pad "1" smd rect (at -1 0) (size 1 0.5) (layers "F.Cu"))
        (pad "2" smd roundrect (at 0 0) (size 1 0.5) (layers "F.Cu"))
        (pad "3" smd roundrect (at 1 0) (size 1 0.5) (layers "F.Cu")))"#;

    const STEP: &[u8] = b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";

    fn rules() -> Rules {
        crate::settings::Settings::default().rules
    }

    /// A library that Standardise has already tidied, so only the checks can object.
    fn tidy(name: &str) -> LibraryRoot {
        let root = LibraryRoot::project(&crate::test_dir(name));
        for (sub, file, text) in [("symbols", "Test.kicad_sym", SYM.as_bytes()), ("footprints/Test.pretty", "FP.kicad_mod", FP.as_bytes()), ("3dmodels/Test.3dshapes", "p.step", STEP)] {
            std::fs::create_dir_all(root.dir.join(sub)).unwrap();
            std::fs::write(root.dir.join(sub).join(file), text).unwrap();
        }
        standardise::apply(&standardise::plan(&root, &Profile::default(), &rules())).unwrap();
        root
    }

    #[test]
    fn compliant_then_plan_not_empty_then_check_warning() {
        let key = ("Test".to_string(), "P1".to_string());
        let root = tidy("compliance_ok");
        let ok = &compute(&root, &Profile::default(), &rules())[&key];
        assert!(ok.ok, "{:?}", ok.reasons);

        // A plan that isn't empty: a profile that wants longer pins than the symbol has.
        let mut strict = Profile::default();
        strict.rules.pin_length_min = Some(3.81);
        let bad = &compute(&root, &strict, &rules())[&key];
        assert!(!bad.ok && bad.reasons.iter().any(|r| r.starts_with("Standardise: ")), "{:?}", bad.reasons);

        // A check finding: no 3D model file.
        std::fs::remove_file(root.dir.join("3dmodels/Test.3dshapes/p.step")).unwrap();
        let missing = compute(&root, &Profile::default(), &rules());
        assert!(!missing[&key].ok && missing[&key].reasons.iter().any(|r| r.contains("3D model")), "{:?}", missing[&key].reasons);
        assert_eq!(findings(&missing).len(), 1);
    }

    #[test]
    fn cache_recomputes_only_when_an_mtime_or_the_profile_changes() {
        let root = tidy("compliance_cache");
        let (p, r) = (Profile::default(), rules());
        let mut cache = Cache::default();
        cache.get(&root, &p, &r);
        cache.get(&root, &p, &r);
        assert_eq!(cache.runs, 1, "unchanged files hit the cache");
        let model = root.dir.join("3dmodels/Test.3dshapes/p.step");
        std::fs::File::options().write(true).open(&model).unwrap().set_modified(SystemTime::now() + Duration::from_secs(60)).unwrap();
        cache.get(&root, &p, &r);
        assert_eq!(cache.runs, 2, "a model mtime change recomputes");
        let mut other = p.clone();
        other.rules.pin_length_min = Some(3.81);
        cache.get(&root, &other, &r);
        assert_eq!(cache.runs, 3, "another profile recomputes");
    }
}
