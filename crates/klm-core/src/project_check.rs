//! Whole-project check: the board and schematic through KiCad's own DRC and ERC, with the fab's minimums
//! added as custom design rules. KiCad does all the checking; this writes the rules and reads the reports.
//!
//! Runs on a copy in a temp folder, so the user's project (and any `.kicad_dru` of theirs) is never touched.

use crate::checks::Severity;
use crate::fab_data::{self, Fab, FabChoice, FabLimits};
use crate::health::Finding;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// How one fab did in [`check_all`]: it passes when KiCad's DRC finds no errors under its limits.
#[derive(Clone, Debug, PartialEq)]
pub struct FabResult {
    pub fab: String,
    pub name: String,
    pub errors: usize,
    pub warnings: usize,
}

impl FabResult {
    pub fn passes(&self) -> bool {
        self.errors == 0
    }
}

/// A KiCad custom design rules file (`.kicad_dru`, KiCad 7+ syntax) holding the limits that are set.
// shortcut: no rule for min_mask_web (KiCad's mask-bridge check only reads the board setup) or courtyard_excess
// (a footprint matter); min_silk_width only covers silk text, as KiCad has no line-width constraint.
pub fn dru(limits: &FabLimits) -> String {
    const VIA: &str = "(condition \"A.Type == 'Via'\") ";
    const SILK: &str = "(layer \"?.Silkscreen\") ";
    let rules = [
        ("klm_min_track", limits.min_track, "", "track_width"),
        ("klm_min_clearance", limits.min_clearance, "", "clearance"),
        ("klm_min_drill", limits.min_drill, "(condition \"A.Type != 'Via'\") ", "hole_size"),
        ("klm_min_annular_ring", limits.min_annular_ring, "", "annular_width"),
        ("klm_min_via_drill", limits.min_via_drill, VIA, "hole_size"),
        ("klm_min_via_diameter", limits.min_via_diameter, VIA, "via_diameter"),
        ("klm_min_silk_width", limits.min_silk_width, SILK, "text_thickness"),
        ("klm_min_silk_height", limits.min_silk_height, SILK, "text_height"),
        ("klm_min_edge_clearance", limits.min_edge_clearance, "", "edge_clearance"),
    ];
    let mut out = String::from("(version 1)\n");
    for (name, value, scope, constraint) in rules {
        if let Some(v) = value {
            out += &format!("(rule \"{name}\" {scope}(constraint {constraint} (min {v}mm)))\n");
        }
    }
    out
}

/// The project's KiCad files copied to a temp folder, removed again when dropped.
struct Copy {
    dir: PathBuf,
    /// The `.kicad_pro`'s name: KiCad reads `<stem>.kicad_dru` beside `<stem>.kicad_pcb`.
    stem: String,
    /// The project's own `.kicad_dru`, kept so the fab rules are added to it rather than replacing it.
    own_rules: Option<String>,
}

impl Copy {
    // shortcut: top-level files only, so sub-sheets kept in subfolders are missing from ERC; copy those too if anyone does that.
    fn new(project_dir: &Path) -> Result<Self> {
        static N: AtomicUsize = AtomicUsize::new(0);
        let files: Vec<PathBuf> = std::fs::read_dir(project_dir)
            .with_context(|| format!("reading {}", project_dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        let pro =
            files.iter().find(|p| p.extension().is_some_and(|e| e == "kicad_pro")).with_context(|| format!("no .kicad_pro in {}", project_dir.display()))?;
        let stem = pro.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let dir = std::env::temp_dir().join(format!("klm-project-check-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let mut copy = Self { dir, stem, own_rules: None };
        let prj = std::path::absolute(project_dir)?.to_string_lossy().replace('\\', "/");
        for p in &files {
            let name = p.file_name().unwrap_or_default();
            let n = name.to_string_lossy();
            if n.ends_with("lib-table") {
                // In the copy ${KIPRJMOD} would be the temp folder, where the project's own libraries aren't.
                std::fs::write(copy.dir.join(name), std::fs::read_to_string(p)?.replace("${KIPRJMOD}", &prj))?;
            } else if n.ends_with(".kicad_dru") {
                copy.own_rules = Some(std::fs::read_to_string(p)?);
            } else if p.extension().is_some_and(|e| e.to_string_lossy().starts_with("kicad_")) {
                std::fs::copy(p, copy.dir.join(name))?;
            }
        }
        Ok(copy)
    }

    fn file(&self, ext: &str) -> Option<PathBuf> {
        let p = self.dir.join(format!("{}.{ext}", self.stem));
        p.is_file().then_some(p)
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Runs kicad-cli and reads the JSON report it wrote to `report`.
fn run(kicad_cli: &Path, args: &[&OsStr], report: &Path) -> Result<Value> {
    let out = crate::kicad_env::quiet(kicad_cli)
        .args(args)
        .output()
        .with_context(|| format!("could not run kicad-cli ({}); install KiCad or set kicad-cli's path in Settings", kicad_cli.display()))?;
    if !out.status.success() {
        bail!("kicad-cli failed: {}{}", String::from_utf8_lossy(&out.stderr).trim(), String::from_utf8_lossy(&out.stdout).trim());
    }
    let text = std::fs::read_to_string(report).with_context(|| format!("kicad-cli wrote no report to {}", report.display()))?;
    serde_json::from_str(&text).context("kicad-cli's report is not JSON")
}

fn drc(copy: &Copy, kicad_cli: &Path, limits: &FabLimits) -> Result<Vec<Finding>> {
    let pcb = copy.file("kicad_pcb").with_context(|| format!("the project has no board ({}.kicad_pcb)", copy.stem))?;
    let ours = dru(limits);
    // KiCad lets later rules win, so the fab's minimums go after the project's own (without a second version line).
    let rules = match &copy.own_rules {
        Some(own) => format!("{own}\n{}", ours.split_once('\n').map_or("", |(_, r)| r)),
        None => ours,
    };
    std::fs::write(copy.dir.join(format!("{}.kicad_dru", copy.stem)), rules)?;
    let report = copy.dir.join("drc.json");
    let args =
        ["pcb".as_ref(), "drc".as_ref(), "--format".as_ref(), "json".as_ref(), "--severity-all".as_ref(), "-o".as_ref(), report.as_os_str(), pcb.as_os_str()];
    Ok(findings(&run(kicad_cli, &args, &report)?, "drc"))
}

fn erc(copy: &Copy, kicad_cli: &Path) -> Result<Vec<Finding>> {
    let sch = copy.file("kicad_sch").with_context(|| format!("the project has no schematic ({}.kicad_sch)", copy.stem))?;
    let report = copy.dir.join("erc.json");
    let args =
        ["sch".as_ref(), "erc".as_ref(), "--format".as_ref(), "json".as_ref(), "--severity-all".as_ref(), "-o".as_ref(), report.as_os_str(), sch.as_os_str()];
    Ok(findings(&run(kicad_cli, &args, &report)?, "erc"))
}

/// KiCad's DRC and ERC on the project in `project_dir` (the folder with the `.kicad_pro`), with `limits` as extra rules.
/// A project with only a board or only a schematic gets the one check it can.
pub fn check(project_dir: &Path, kicad_cli: &Path, limits: &FabLimits) -> Result<Vec<Finding>> {
    let copy = Copy::new(project_dir)?;
    let (pcb, sch) = (copy.file("kicad_pcb").is_some(), copy.file("kicad_sch").is_some());
    if !pcb && !sch {
        bail!("no {0}.kicad_pcb or {0}.kicad_sch beside the .kicad_pro", copy.stem);
    }
    let mut out = if pcb { drc(&copy, kicad_cli, limits)? } else { vec![] };
    if sch {
        out.extend(erc(&copy, kicad_cli)?);
    }
    Ok(out)
}

/// DRC once per fab (with `ipc`'s class and density), so the user sees which fabs could make the board as it is.
pub fn check_all(project_dir: &Path, kicad_cli: &Path, fabs: &[Fab], ipc: &FabChoice) -> Result<Vec<FabResult>> {
    let copy = Copy::new(project_dir)?;
    fabs.iter()
        .map(|fab| {
            let limits = fab_data::limits(&FabChoice { fabs: vec![fab.id.clone()], ..ipc.clone() });
            let found = drc(&copy, kicad_cli, &limits)?;
            let count = |s| found.iter().filter(|f| f.severity == s).count();
            Ok(FabResult { fab: fab.id.clone(), name: fab.name.clone(), errors: count(Severity::Error), warnings: count(Severity::Warning) })
        })
        .collect()
}

/// A kicad-cli DRC or ERC JSON report (KiCad 8–10) as findings of `kind`. Exclusions are left out: the user waived them in KiCad.
pub fn findings(report: &Value, kind: &'static str) -> Vec<Finding> {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let mut rows = list(&report["violations"]);
    rows.extend(list(&report["unconnected_items"]));
    for sheet in list(&report["sheets"]) {
        rows.extend(list(&sheet["violations"]));
    }
    rows.iter()
        .filter_map(|v| {
            let severity = match v["severity"].as_str()? {
                "error" => Severity::Error,
                "warning" => Severity::Warning,
                _ => return None,
            };
            Some(Finding { severity, kind, item: item(&list(&v["items"])), message: v["description"].as_str().unwrap_or_default().to_string() })
        })
        .collect()
}

/// The references KiCad's item descriptions name ("PTH pad 1 [GND] of R2", "Footprint C1", "Symbol P3 [CONN_2]"),
/// or, when none do, the first item and where it is.
fn item(items: &[Value]) -> String {
    let desc = |i: &Value| i["description"].as_str().unwrap_or_default().to_string();
    let mut refs: Vec<String> = vec![];
    for d in items.iter().map(desc) {
        let r = d.rsplit_once(" of ").map(|(_, r)| r).or_else(|| d.strip_prefix("Footprint ")).or_else(|| d.strip_prefix("Symbol "));
        if let Some(r) = r.and_then(|r| r.split_whitespace().next()).filter(|r| !refs.iter().any(|x| x == r)) {
            refs.push(r.to_string());
        }
    }
    if !refs.is_empty() {
        return refs.join(", ");
    }
    let Some(first) = items.first() else { return String::new() };
    format!("{} at ({}, {}) mm", desc(first), first["pos"]["x"], first["pos"]["y"])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(f: &Finding) -> (Severity, &str, &str) {
        (f.severity, f.kind, f.item.as_str())
    }

    #[test]
    fn dru_holds_only_the_limits_set() {
        let limits = FabLimits { min_track: Some(0.127), min_via_drill: Some(0.3), min_silk_height: Some(1.0), min_mask_web: Some(0.1), ..Default::default() };
        assert_eq!(
            dru(&limits),
            "(version 1)\n\
             (rule \"klm_min_track\" (constraint track_width (min 0.127mm)))\n\
             (rule \"klm_min_via_drill\" (condition \"A.Type == 'Via'\") (constraint hole_size (min 0.3mm)))\n\
             (rule \"klm_min_silk_height\" (layer \"?.Silkscreen\") (constraint text_height (min 1mm)))\n"
        );
        assert_eq!(dru(&FabLimits::default()), "(version 1)\n");
    }

    #[test]
    fn drc_report_becomes_findings() {
        let report = serde_json::from_str(include_str!("../tests/fixtures/drc_kicad10.json")).unwrap();
        let found = findings(&report, "drc");
        let got: Vec<_> = found.iter().map(short).collect();
        assert_eq!(
            got,
            [
                (Severity::Error, "drc", "C2"),
                (Severity::Error, "drc", "C2"),
                (Severity::Error, "drc", "Segment on Edge.Cuts at (173.355, 90.17) mm"),
                (Severity::Warning, "drc", "C1"),
                (Severity::Error, "drc", "R2, R3"),
            ]
        );
        assert!(found[0].message.starts_with("Annular width (rule 'klm_min_annular_ring'"));
        assert_eq!(found[4].message, "Missing connection between items");
    }

    #[test]
    fn erc_report_becomes_findings() {
        let report = serde_json::from_str(include_str!("../tests/fixtures/erc_kicad10.json")).unwrap();
        let got: Vec<_> = findings(&report, "erc").iter().map(|f| (f.severity, f.item.clone(), f.message.clone())).collect();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], (Severity::Error, "Label 'DANGLING' at (50.8, 50.8) mm".into(), "Label not connected".into()));
        assert_eq!(got[2].1, "P3");
    }

    const PCB: &str = r#"(kicad_pcb (version 20240108) (generator "klm")
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (setup)
  (net 0 "")
  (gr_rect (start 0 0) (end 20 10) (stroke (width 0.1) (type solid)) (fill none) (layer "Edge.Cuts"))
  (segment (start 5 5) (end 15 5) (width 0.1) (layer "F.Cu") (net 0))
)
"#;
    const SCH: &str = r#"(kicad_sch (version 20231120) (generator "klm") (uuid "0b1c2d3e-0000-4000-8000-000000000001") (paper "A4")
  (lib_symbols)
  (label "DANGLING" (at 50.8 50.8 0) (effects (font (size 1.27 1.27))) (uuid "0b1c2d3e-0000-4000-8000-000000000003"))
)
"#;

    /// A board with one 0.1 mm track and a schematic with one dangling label.
    fn project(name: &str) -> PathBuf {
        let d = crate::test_dir(name);
        std::fs::write(d.join("mini.kicad_pro"), "{}").unwrap();
        std::fs::write(d.join("mini.kicad_pcb"), PCB).unwrap();
        std::fs::write(d.join("mini.kicad_sch"), SCH).unwrap();
        d
    }

    #[test]
    fn missing_kicad_cli_is_a_clear_error() {
        let d = project("project-check-no-cli");
        let e = check(&d, &d.join("no-such-kicad-cli.exe"), &FabLimits::default()).unwrap_err();
        assert!(format!("{e:#}").contains("could not run kicad-cli"), "{e:#}");
        let e = check(&d.join("nowhere"), Path::new("kicad-cli"), &FabLimits::default()).unwrap_err();
        assert!(format!("{e:#}").contains("reading"), "{e:#}");
        std::fs::remove_file(d.join("mini.kicad_pro")).unwrap();
        assert!(format!("{:#}", check(&d, Path::new("kicad-cli"), &FabLimits::default()).unwrap_err()).contains("no .kicad_pro"));
    }

    /// The real kicad-cli accepts our rules file (it silently ignores one it can't parse) and the project is left alone.
    #[test]
    fn real_kicad_cli_applies_the_rules() {
        let Ok(cli) = crate::kicad_env::kicad_cli() else { return eprintln!("kicad-cli not found; skipping") };
        if !std::process::Command::new(&cli).arg("version").output().is_ok_and(|o| o.status.success()) {
            return eprintln!("kicad-cli not working; skipping");
        }
        let d = project("project-check-real");
        let found = check(&d, &cli, &FabLimits { min_track: Some(0.3), ..Default::default() }).unwrap();
        let track = found.iter().find(|f| f.kind == "drc" && f.message.starts_with("Track width")).expect("track width finding");
        assert!(track.message.contains("0.3000 mm") && track.severity == Severity::Error, "{}", track.message);
        assert!(found.iter().any(|f| f.kind == "erc" && f.item.starts_with("Label 'DANGLING'")), "{found:?}");
        assert!(!d.join("mini.kicad_dru").exists() && std::fs::read_to_string(d.join("mini.kicad_pcb")).unwrap() == PCB);
    }
}
