//! Standardise a library to the KiCad Library Conventions (KLC), with each rule switchable in Settings.
//!
//! Two steps, like `batch`: [`plan`] works out every file that would change and how, without writing,
//! and [`apply`] writes exactly that, after one snapshot of the library. Symbol rules are in `symbol`,
//! footprint rules in `footprint`.
//! Naming rules (`rules.rs`) are not applied here: Standardise never renames parts, so a library's existing
//! symbol names, which may predate the name template, stay as they are.

mod footprint;
pub mod profile;

pub use footprint::{FootprintStyle, TextStyle};
pub(crate) use footprint::fix as fix_footprint;
pub(crate) mod symbol;

use crate::convert::easyeda::geom::fmt;
use crate::kicad_env::write_atomic;
use crate::library::{LibraryRoot, property, sanitize};
use crate::sexpr::{self, Sexpr};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::fmt as stdfmt;
use std::collections::HashMap;
use std::path::PathBuf;

/// Which fixes run and to what values. `None` (or `false`) switches a rule off. Defaults are KLC's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    /// Size of field, pin name, pin number and other symbol text, mm (KLC S4.3: 50 mil).
    pub symbol_text_size: Option<f64>,
    /// Shorter pins are lengthened, keeping the body end where it is (KLC S4.1: at least 100 mil).
    /// Symbols with two pins or fewer (resistors, capacitors) and power symbols are left alone.
    pub pin_length_min: Option<f64>,
    /// Stroke width of symbol graphics, mm (KLC S3.4: 10 mil).
    pub symbol_line_width: Option<f64>,
    /// Reference above the symbol body, Value below it.
    pub symbol_field_positions: bool,
    /// Footprint line widths, mm (KLC F5.1–F5.3).
    pub silk_width: Option<f64>,
    pub fab_width: Option<f64>,
    pub courtyard_width: Option<f64>,
    /// Reference above the courtyard, Value below it.
    pub footprint_ref_value: bool,
    /// SMD origin at the centre of the pads, through-hole origin at pin 1 (KLC F6.1, F7.1).
    pub footprint_origin: bool,
    /// Add a silkscreen dot beside pin 1 when the footprint has no pin-1 marker (KLC F5.1).
    pub pin1_marker: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            symbol_text_size: Some(1.27),
            pin_length_min: Some(2.54),
            symbol_line_width: Some(0.254),
            symbol_field_positions: true,
            silk_width: Some(0.12),
            fab_width: Some(0.10),
            courtyard_width: Some(0.05),
            footprint_ref_value: true,
            footprint_origin: true,
            pin1_marker: true,
        }
    }
}

/// One file that standardising would rewrite.
pub struct FileChange {
    pub path: PathBuf,
    pub label: String,
    pub changes: Vec<String>,
    old: String,
    new: String,
}

/// Everything standardising one library would change.
pub struct Plan {
    pub files: Vec<FileChange>,
    /// Files that couldn't be read; they are left alone.
    pub errors: Vec<String>,
    pub root: LibraryRoot,
}

impl stdfmt::Display for Plan {
    fn fmt(&self, f: &mut stdfmt::Formatter) -> stdfmt::Result {
        if self.files.is_empty() {
            writeln!(f, "{}: nothing to change.", self.root.dir.display())?;
        }
        for file in &self.files {
            writeln!(f, "{}", file.label)?;
            for c in &file.changes {
                writeln!(f, "  {c}")?;
            }
        }
        for e in &self.errors {
            writeln!(f, "warning: {e}")?;
        }
        Ok(())
    }
}

/// Works out what the profile's rules (and its per-class footprint styles) would change in `root`, reading
/// every symbol library and footprint but writing nothing. `naming` tells which class a part is.
pub fn plan(root: &LibraryRoot, profile: &profile::Profile, naming: &crate::rules::Rules) -> Plan {
    let rules = &profile.rules;
    // Fab fixes only once a fab is chosen: the density defaults to B in every profile, so growing courtyards
    // without one would rewrite every footprint of every library the first time it is standardised.
    let limits = (!profile.fab.fabs.is_empty()).then(|| crate::fab_data::limits(&profile.fab));
    // Footprint name -> the house style of the class of the part that uses it.
    let mut styles: HashMap<String, &FootprintStyle> = HashMap::new();
    for cat in root.categories().into_iter().filter(|_| !profile.footprint_styles.is_empty()) {
        let path = root.dir.join("symbols").join(format!("{}.kicad_sym", sanitize(&cat)));
        let Ok(lib) = std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|t| sexpr::parse(&t)) else { continue };
        for sym in lib.children("symbol") {
            let style = crate::symbols::class_of(&cat, sym, naming).ok().and_then(|c| profile.footprint_styles.get(&format!("{c:?}")));
            if let (Some(fp), Some(style)) = (property(sym, "Footprint"), style) {
                styles.insert(fp.rsplit(':').next().unwrap_or(fp).to_string(), style);
            }
        }
    }
    let mut plan = Plan { files: vec![], errors: vec![], root: root.clone() };
    let mut consider = |path: PathBuf, label: String, fix: &dyn Fn(&mut Sexpr) -> Vec<String>| {
        let parsed = std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|t| Ok((sexpr::parse(&t)?, t)));
        match parsed {
            Ok((mut x, old)) => {
                let changes = fix(&mut x);
                if !changes.is_empty() {
                    plan.files.push(FileChange { path, label, changes, old, new: sexpr::to_string(&x) });
                }
            }
            Err(e) => plan.errors.push(format!("can't read {}: {e:#}", path.display())),
        }
    };
    for cat in root.categories() {
        let path = root.dir.join("symbols").join(format!("{}.kicad_sym", sanitize(&cat)));
        consider(path, format!("symbols in {}", root.nickname(&cat)), &|lib| {
            let mut out = vec![];
            for sym in lib.items_mut().iter_mut().filter(|c| c.head() == Some("symbol")) {
                let name = sym.arg(0).unwrap_or_default().to_string();
                // A house template places Reference and Value itself, so the generic above/below rule leaves them.
                let templated = crate::symbols::class_of(&cat, sym, naming).is_ok_and(|c| profile.templates.contains_key(&format!("{c:?}")));
                let rules = Rules { symbol_field_positions: rules.symbol_field_positions && !templated, ..rules.clone() };
                out.extend(symbol::fix(sym, &rules).into_iter().map(|c| format!("{name}: {c}")));
                out.extend(crate::fields::fix(sym, &cat, profile, None).into_iter().map(|c| format!("{name}: {c}")));
            }
            out
        });
    }
    let mut footprints: Vec<PathBuf> = std::fs::read_dir(root.dir.join("footprints"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "pretty"))
        .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten().map(|f| f.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "kicad_mod"))
        .collect();
    footprints.sort();
    for path in footprints {
        let cat = path.parent().and_then(|p| p.file_stem()).unwrap_or_default().to_string_lossy().into_owned();
        let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        consider(path, format!("footprint {name} in {}", root.nickname(&cat)), &|fp| {
            let mut out = footprint::fix(fp, rules, styles.get(&name).copied(), limits.as_ref());
            out.extend(crate::model3d::fix(root, fp));
            out
        });
    }
    plan
}

/// Writes what `plan` said, after one snapshot of the library. Refuses, writing nothing, if any of
/// the files changed since the plan was made. Returns the number of files written.
pub fn apply(plan: &Plan) -> Result<usize> {
    if plan.files.is_empty() {
        return Ok(0);
    }
    for f in &plan.files {
        if std::fs::read_to_string(&f.path).ok().as_deref() != Some(f.old.as_str()) {
            bail!("{} changed since the dry run; run it again", f.path.display());
        }
    }
    let _snapshot = crate::snapshot::guard(&plan.root, &format!("standardise ({} files)", plan.files.len()))?;
    for f in &plan.files {
        write_atomic(&f.path, &f.new)?;
    }
    Ok(plan.files.len())
}

fn num(x: &Sexpr, i: usize) -> Option<f64> {
    x.arg(i)?.parse().ok()
}

fn set_num(x: &mut Sexpr, i: usize, v: f64) {
    x.set_arg(i, Sexpr::Atom(fmt(v)));
}

fn child_mut<'a>(x: &'a mut Sexpr, head: &str) -> Option<&'a mut Sexpr> {
    match x {
        Sexpr::List(v) => v.iter_mut().find(|c| c.head() == Some(head)),
        _ => None,
    }
}

fn differs(a: f64, b: f64) -> bool {
    (a - b).abs() > 1e-4
}

/// Sets the (x, y) arguments of `at`, returning whether they changed.
fn move_to(at: &mut Sexpr, (x, y): (f64, f64)) -> bool {
    let changed = num(at, 0).is_none_or(|v| differs(v, x)) || num(at, 1).is_none_or(|v| differs(v, y));
    if changed && at.items().len() >= 3 {
        set_num(at, 0, x);
        set_num(at, 1, y);
    }
    changed
}

/// Bounding box (min x, min y, max x, max y) of `pts`.
fn bbox(pts: &[(f64, f64)]) -> Option<(f64, f64, f64, f64)> {
    let first = *pts.first()?;
    Some(pts.iter().fold((first.0, first.1, first.0, first.1), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use profile::Profile;

    const SYMBOLS: &str = r#"(kicad_symbol_lib (version 20241209) (generator "t")
        (symbol "U1"
            (property "Reference" "U" (at 9 9 0) (effects (font (size 2 2))))
            (property "Value" "U1" (at 9 9 0) (effects (font (size 1.27 1.27))))
            (symbol "U1_0_1" (rectangle (start -5 5) (end 5 -5) (stroke (width 0.1) (type default)) (fill (type none))))
            (symbol "U1_1_1"
                (pin input line (at -6.27 2.54 0) (length 1.27) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
                (pin input line (at -7.54 0 0) (length 2.54) (name "B" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))
                (pin output line (at 7.54 0 180) (length 2.54) (name "Y" (effects (font (size 1.27 1.27)))) (number "3" (effects (font (size 1.27 1.27))))))))"#;

    const FOOTPRINT: &str = r#"(footprint "FP_U1" (layer "F.Cu")
        (property "Reference" "REF**" (at 0 0 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15))))
        (property "Value" "FP_U1" (at 0 0 0) (layer "F.Fab") (effects (font (size 1 1) (thickness 0.15))))
        (attr smd)
        (fp_line (start -2 -2) (end 2 -2) (stroke (width 0.2) (type solid)) (layer "F.SilkS"))
        (fp_rect (start -3 -3) (end 3 3) (stroke (width 0.1) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd rect (at -1 -1) (size 0.5 0.5) (layers "F.Cu"))
        (pad "2" smd rect (at -1 1) (size 0.5 0.5) (layers "F.Cu"))
        (pad "3" smd rect (at 1 1) (size 0.5 0.5) (layers "F.Cu"))
        (pad "4" smd rect (at 1 -1) (size 0.5 0.5) (layers "F.Cu")))"#;

    fn library(name: &str) -> (PathBuf, LibraryRoot) {
        let base = crate::test_dir(name);
        let root = LibraryRoot::global(base.join("lib"), base.join("config"));
        root.ensure_layout().unwrap();
        std::fs::write(root.dir.join("symbols/ICs.kicad_sym"), SYMBOLS).unwrap();
        std::fs::create_dir_all(root.dir.join("footprints/ICs.pretty")).unwrap();
        std::fs::write(root.dir.join("footprints/ICs.pretty/FP_U1.kicad_mod"), FOOTPRINT).unwrap();
        (base, root)
    }

    fn contents(root: &LibraryRoot) -> Vec<String> {
        ["symbols/ICs.kicad_sym", "footprints/ICs.pretty/FP_U1.kicad_mod"].iter().map(|p| std::fs::read_to_string(root.dir.join(p)).unwrap()).collect()
    }

    #[test]
    fn dry_run_writes_nothing_and_apply_snapshots_then_fixes() {
        let (_base, root) = library("standardise_apply");
        let before = contents(&root);
        let p = plan(&root, &profile::Profile::default(), &Default::default());
        assert_eq!(contents(&root), before, "planning must not touch the library");
        assert!(crate::snapshot::list(&root).is_empty());
        assert_eq!(p.files.len(), 2, "{p}");
        assert!(p.to_string().contains("U1: 1 text(s) resized to 1.27 mm"), "{p}");

        assert_eq!(apply(&p).unwrap(), 2);
        assert_eq!(crate::snapshot::list(&root).len(), 1, "one snapshot before writing");
        assert_ne!(contents(&root), before);
        // The fixed library conforms, so a second run has nothing to do.
        let again = plan(&root, &profile::Profile::default(), &Default::default());
        assert!(again.files.is_empty(), "{again}");
    }

    #[test]
    fn rules_switched_off_change_nothing() {
        let (_base, root) = library("standardise_off");
        let off = Rules {
            symbol_text_size: None,
            pin_length_min: None,
            symbol_line_width: None,
            symbol_field_positions: false,
            silk_width: None,
            fab_width: None,
            courtyard_width: None,
            footprint_ref_value: false,
            footprint_origin: false,
            pin1_marker: false,
        };
        assert!(plan(&root, &profile::Profile { rules: off, ..Default::default() }, &Default::default()).files.is_empty());
    }

    #[test]
    fn apply_refuses_a_file_changed_since_the_plan() {
        let (_base, root) = library("standardise_stale");
        let p = plan(&root, &profile::Profile::default(), &Default::default());
        std::fs::write(root.dir.join("symbols/ICs.kicad_sym"), SYMBOLS.replace("U1", "U2")).unwrap();
        assert!(apply(&p).unwrap_err().to_string().contains("changed since the dry run"));
        assert!(crate::snapshot::list(&root).is_empty());
    }

    #[test]
    fn plan_normalises_and_rebuilds_part_information() {
        let base = crate::test_dir("standardise_fields");
        let root = LibraryRoot::global(base.join("lib"), base.join("config"));
        root.ensure_layout().unwrap();
        std::fs::write(
            root.dir.join("symbols/Capacitors.kicad_sym"),
            r#"(kicad_symbol_lib (version 20241209) (generator "t") (symbol "C1" (property "Value" "C1") (property "Capacitance" "0.1uF") (property "Voltage" "16V")))"#,
        )
        .unwrap();
        let p = plan(&root, &Profile::default(), &Default::default());
        let text = p.to_string();
        assert!(text.contains("C1: Capacitance: 0.1uF -> 100nF") && text.contains("C1: Description: \"\" -> \"Capacitor 100nF 16V\""), "{text}");
        apply(&p).unwrap();
        assert!(plan(&root, &Profile::default(), &Default::default()).files.is_empty());
    }

    #[test]
    fn house_template_keeps_its_field_positions() {
        let base = crate::test_dir("standardise_templated");
        let root = LibraryRoot::global(base.join("lib"), base.join("config"));
        root.ensure_layout().unwrap();
        let sym = r#"(symbol "R1" (property "Reference" "R" (at 5 5 0)) (property "Value" "R1" (at 5 -5 0)) (symbol "R1_1_1" (rectangle (start -1 2.5) (end 1 -2.5)) (pin passive line (at 0 3.81 270) (length 1.27) (name "~") (number "1")) (pin passive line (at 0 -3.81 90) (length 1.27) (name "~") (number "2"))))"#;
        std::fs::write(root.dir.join("symbols/Resistors.kicad_sym"), format!(r#"(kicad_symbol_lib (version 20241209) (generator "t") {sym})"#)).unwrap();
        let moves = |p: &Profile| plan(&root, p, &Default::default()).to_string().contains("Reference");
        assert!(moves(&Profile::default()));
        let mut p = Profile::default();
        p.templates.insert("Resistor".into(), sym.into());
        assert!(!moves(&p), "{}", plan(&root, &p, &Default::default()));
    }

    #[test]
    fn old_settings_files_get_the_klc_defaults() {
        let r: Rules = serde_json::from_str(r#"{"pin_length_min": null}"#).unwrap();
        assert_eq!(r, Rules { pin_length_min: None, ..Rules::default() });
    }
}
