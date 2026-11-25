//! Project health report: everything wrong or untidy in a project's own KLM library, in one list.
//!
//! The part checks come from `checks`, the has/lacks status from `library::status` and the table
//! lookups from `kicad_env`; this file adds what only a whole-project view can see (duplicates,
//! orphaned files, library tables) and the project's schematics: placed symbols that no library has.

use crate::checks::{self, Severity};
use crate::library::{Check, LibraryRoot, property};
use crate::sexpr;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    /// symbol, schematic-symbol, footprint, 3d-model, datasheet, sim-model, part-check, compliance, duplicate-mpn, metadata, orphan, library-table or library-reference.
    pub kind: &'static str,
    /// Where: `category/part`, a file path or a table entry.
    pub item: String,
    pub message: String,
}

pub fn has_errors(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Error)
}

fn sev(s: Severity) -> &'static str {
    if s == Severity::Error { "error" } else { "warning" }
}

pub fn to_json(findings: &[Finding]) -> String {
    let rows: Vec<_> = findings.iter().map(|f| json!({ "severity": sev(f.severity), "kind": f.kind, "item": f.item, "message": f.message })).collect();
    serde_json::to_string_pretty(&rows).unwrap_or_default()
}

pub fn to_csv(findings: &[Finding]) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut out = String::from("severity,kind,item,message\n");
    for f in findings {
        out += &format!("{},{},{},{}\n", sev(f.severity), f.kind, q(&f.item), q(&f.message));
    }
    out
}

struct Out(Vec<Finding>);

impl Out {
    fn add(&mut self, severity: Severity, kind: &'static str, item: impl Into<String>, message: impl Into<String>) {
        self.0.push(Finding { severity, kind, item: item.into(), message: message.into() });
    }
}

/// Library names and (unresolved) URIs in a `sym-lib-table` / `fp-lib-table`; empty if it can't be read.
pub(crate) fn table_entries(table: &Path) -> Vec<(String, String)> {
    let Some(t) = std::fs::read_to_string(table).ok().and_then(|t| sexpr::parse(&t).ok()) else { return vec![] };
    t.children("lib")
        .filter_map(|l| Some((l.child("name")?.arg(0)?.to_string(), l.child("uri").and_then(|u| u.arg(0)).unwrap_or_default().to_string())))
        .collect()
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { files_under(&p, out) } else { out.push(p) }
    }
}

/// The double-quoted strings of S-expression text.
fn quoted(text: &str) -> impl Iterator<Item = &str> {
    text.split('"').skip(1).step_by(2)
}

/// A path as one comparable string: `${KIPRJMOD}` filled in, relative paths taken from the project, `/` separators, no case.
fn norm(path: &str, project_dir: &Path) -> String {
    let prj = project_dir.to_string_lossy().replace('\\', "/");
    let p = path.replace("${KIPRJMOD}", &prj).replace('\\', "/");
    let p = if Path::new(&p).is_absolute() || p.starts_with('/') { p } else { format!("{prj}/{p}") };
    p.to_lowercase()
}

/// The uri with every `${VAR}` filled in the way KiCad does: `${KIPRJMOD}`, then the environment, then
/// `kicad_common.json` in `config`, then the installed KiCad's stock folders. None when one can't be
/// resolved, in which case the library is trusted rather than reported.
pub(crate) fn expand(uri: &str, project_dir: &Path, config: Option<&Path>) -> Option<String> {
    let mut s = uri.replace("${KIPRJMOD}", &project_dir.to_string_lossy());
    let user = config.map(crate::cleanup::user_vars).unwrap_or_default();
    while let Some(open) = s.find("${") {
        let len = s[open..].find('}')?;
        let var = &s[open + 2..open + len];
        let value = std::env::var(var)
            .ok()
            .or_else(|| user.get(var).cloned())
            .or_else(|| crate::kicad_env::default_var_dir(var).map(|d| d.to_string_lossy().into_owned()))?;
        s.replace_range(open..open + len + 1, &value);
    }
    Some(s)
}

/// Placed symbols (`lib_id` "Nick:Name") whose library isn't in any table or doesn't hold the symbol.
/// One finding per symbol, listing where it is placed. A library whose path uses a KiCad variable
/// that `expand` can't resolve is trusted.
fn schematic_symbols(project_dir: &Path, global_tables: Option<&Path>, out: &mut Out) {
    let mut sheets = vec![];
    files_under(project_dir, &mut sheets);
    sheets.retain(|p| p.extension().is_some_and(|e| e == "kicad_sch") && !p.to_string_lossy().contains("-backups"));
    sheets.sort();
    let mut tables = table_entries(&project_dir.join("sym-lib-table"));
    tables.extend(global_tables.map(|d| table_entries(&d.join("sym-lib-table"))).unwrap_or_default());
    // lib_id -> "U1 (power.kicad_sch)", in the order met.
    let mut placed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for sheet in &sheets {
        let Some(t) = std::fs::read_to_string(sheet).ok().and_then(|t| sexpr::parse(&t).ok()) else { continue };
        let file = sheet.file_name().unwrap_or_default().to_string_lossy().to_string();
        for sym in t.children("symbol") {
            let Some(id) = sym.child("lib_id").and_then(|l| l.arg(0)) else { continue };
            let reference = property(sym, "Reference").unwrap_or_default();
            if !reference.starts_with('#') {
                placed.entry(id.to_string()).or_default().push(format!("{reference} ({file})"));
            }
        }
    }
    for (id, at) in placed {
        let Some((nick, name)) = id.split_once(':') else { continue };
        let message = match tables.iter().find(|e| e.0 == nick) {
            None => format!("library {nick} is in neither the project's nor the global sym-lib-table; placed as {}", at.join(", ")),
            Some((_, uri)) => {
                let Some(path) = expand(uri, project_dir, global_tables) else { continue };
                let lib = std::fs::read_to_string(&path).ok().and_then(|t| sexpr::parse(&t).ok());
                if lib.is_some_and(|l| l.children("symbol").any(|s| s.arg(0) == Some(name))) {
                    continue;
                }
                format!("library {nick} has no symbol {name}; placed as {}", at.join(", "))
            }
        };
        out.add(Severity::Error, "schematic-symbol", id, message);
    }
}

/// Scans the project in `project_dir` (the folder with the `.kicad_pro`). `global_tables` is KiCad's
/// config folder, whose tables the project's are compared with.
pub fn scan(project_dir: &Path, global_tables: Option<&Path>) -> Vec<Finding> {
    let root = LibraryRoot::project(project_dir);
    let mut out = Out(vec![]);

    // Library tables: broken URIs, and names the global tables also use.
    let mut fp_nicks: BTreeSet<String> = BTreeSet::new();
    for (file, is_fp) in [("sym-lib-table", false), ("fp-lib-table", true)] {
        let global: BTreeSet<String> = global_tables.map(|d| table_entries(&d.join(file))).unwrap_or_default().into_iter().map(|e| e.0).collect();
        if is_fp {
            fp_nicks.extend(global.iter().cloned());
        }
        for (name, uri) in table_entries(&project_dir.join(file)) {
            let item = format!("{file}: {name}");
            let path = expand(&uri, project_dir, global_tables);
            if path.as_ref().is_some_and(|p| !p.starts_with("http") && !Path::new(p).exists()) {
                out.add(Severity::Error, "library-table", &item, format!("{uri} doesn't exist"));
            }
            if global.contains(&name) {
                out.add(Severity::Warning, "library-table", &item, "the global table has a library with the same name, which this one overrides");
            }
            if is_fp {
                fp_nicks.insert(name);
            }
        }
    }

    // Symbol libraries: unreadable ones, and references from their footprint fields.
    let mut cats = root.categories();
    cats.sort();
    let mut symbol_text = String::new();
    for cat in &cats {
        let path = root.dir.join("symbols").join(format!("{cat}.kicad_sym"));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let Ok(lib) = sexpr::parse(&text) else {
            out.add(Severity::Error, "symbol", path.display().to_string(), "the symbol library can't be read");
            continue;
        };
        for sym in lib.children("symbol") {
            let name = sym.arg(0).unwrap_or_default();
            if let Some((nick, _)) = property(sym, "Footprint").and_then(|f| f.split_once(':'))
                && !fp_nicks.contains(nick)
            {
                out.add(
                    Severity::Error,
                    "library-reference",
                    format!("{cat}/{name}"),
                    format!("Footprint library {nick} is in neither the project's nor the global fp-lib-table"),
                );
            }
        }
        symbol_text += &text;
    }

    // What each part has and lacks.
    let parts = root.list_parts();
    for p in &parts {
        let item = format!("{}/{}", p.category, p.name);
        let (e, w) = (Severity::Error, Severity::Warning);
        match p.footprint {
            Check::Missing => out.add(e, "footprint", &item, "no footprint is set"),
            Check::Broken => out.add(e, "footprint", &item, "the footprint is set but its file can't be read"),
            _ => {}
        }
        match p.model {
            Check::Missing if p.footprint == Check::Ok => out.add(w, "3d-model", &item, "the footprint has no 3D model"),
            Check::Broken => out.add(e, "3d-model", &item, "the 3D model file is missing"),
            _ => {}
        }
        match p.datasheet {
            Check::Missing => out.add(w, "datasheet", &item, "no datasheet is set"),
            Check::Broken => out.add(e, "datasheet", &item, "the datasheet file is missing"),
            _ => {}
        }
        if p.sim == Check::Broken {
            out.add(e, "sim-model", &item, "the simulation model file is missing");
        }
    }
    // A part with no footprint is already reported above; the field check would say it again.
    let no_footprint: BTreeSet<(String, String)> =
        parts.iter().filter(|p| p.footprint == Check::Missing).map(|p| (p.category.clone(), p.name.clone())).collect();
    let mut part_issues: Vec<_> = checks::check_library(&root).into_iter().collect();
    part_issues.sort_by(|a, b| a.0.cmp(&b.0));
    for ((cat, name), issues) in part_issues {
        for i in issues.into_iter().filter(|i| !(no_footprint.contains(&(cat.clone(), name.clone())) && i.message.starts_with("Required field Footprint"))) {
            out.add(i.severity, "part-check", format!("{cat}/{name}"), i.message);
        }
    }

    // Duplicates and inconsistent metadata.
    let mut by_mpn: BTreeMap<String, Vec<&crate::library::PartStatus>> = BTreeMap::new();
    let mut spellings: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for p in &parts {
        by_mpn.entry(p.mpn.to_lowercase()).or_default().push(p);
        if !p.manufacturer.is_empty() {
            spellings.entry(p.manufacturer.to_lowercase()).or_default().insert(&p.manufacturer);
        }
    }
    for (mpn, group) in &by_mpn {
        let at = group.iter().map(|p| format!("{}/{}", p.category, p.name)).collect::<Vec<_>>().join(", ");
        if group.len() > 1 {
            out.add(Severity::Warning, "duplicate-mpn", mpn, format!("used by {} parts: {at}", group.len()));
        }
        let makers: BTreeSet<_> = group.iter().map(|p| p.manufacturer.to_lowercase()).filter(|m| !m.is_empty()).collect();
        if makers.len() > 1 {
            out.add(Severity::Warning, "metadata", mpn, format!("the same MPN has different manufacturers: {at}"));
        }
    }
    for set in spellings.values().filter(|s| s.len() > 1) {
        out.add(Severity::Warning, "metadata", set.iter().copied().collect::<Vec<_>>().join(" / "), "one manufacturer is spelled in different ways");
    }

    // Orphans: files nothing refers to, matched by full path (a same-named file elsewhere doesn't cover).
    let (mut fps, mut footprint_text) = (vec![], String::new());
    files_under(&root.dir.join("footprints"), &mut fps);
    fps.retain(|p| p.extension().is_some_and(|e| e == "kicad_mod"));
    for f in &fps {
        footprint_text += &std::fs::read_to_string(f).unwrap_or_default();
    }
    let fp_table = table_entries(&project_dir.join("fp-lib-table"));
    let used_footprints: BTreeSet<String> = quoted(&symbol_text)
        .filter_map(|s| s.split_once(':'))
        .flat_map(|(nick, name)| fp_table.iter().filter(move |e| e.0 == nick).map(move |e| norm(&format!("{}/{name}.kicad_mod", e.1), project_dir)))
        .collect();
    for f in &fps {
        if !used_footprints.contains(&norm(&f.to_string_lossy(), project_dir)) {
            out.add(Severity::Warning, "orphan", f.display().to_string(), "no symbol uses this footprint");
        }
    }
    for (dir, text, who) in [("3dmodels", &footprint_text, "footprint"), ("datasheets", &symbol_text, "symbol"), ("simulation", &symbol_text, "symbol")] {
        let used: BTreeSet<String> = quoted(text).map(|s| norm(s, project_dir)).collect();
        let mut files = vec![];
        files_under(&root.dir.join(dir), &mut files);
        files.sort();
        for f in files {
            if !used.contains(&norm(&f.to_string_lossy(), project_dir)) {
                out.add(Severity::Warning, "orphan", f.display().to_string(), format!("no {who} refers to this file"));
            }
        }
    }

    schematic_symbols(project_dir, global_tables, &mut out);
    out.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYM: &str = r#"(kicad_symbol_lib (version 20251024) (generator "t")
        (symbol "P1"
            (property "Reference" "U" (at 0 0 0))
            (property "Value" "P1" (at 0 0 0))
            (property "Footprint" "PRJ_Test:FP" (at 0 0 0))
            (property "Datasheet" "${KIPRJMOD}/lib/datasheets/p1.pdf" (at 0 0 0))
            (property "MPN" "P1" (at 0 0 0))
            (property "Manufacturer" "Acme" (at 0 0 0))
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

    const STEP: &str = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";

    fn table(kind: &str, uri: &str) -> String {
        format!("({kind}_lib_table (version 7) (lib (name \"PRJ_Test\") (type \"KiCad\") (uri \"{uri}\") (options \"\") (descr \"\")))")
    }

    /// A healthy project in a fresh folder.
    fn project(name: &str) -> PathBuf {
        let dir = crate::test_dir(name);
        let write = |rel: &str, text: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write("lib/symbols/Test.kicad_sym", SYM);
        write("lib/footprints/Test.pretty/FP.kicad_mod", FP);
        write("lib/3dmodels/Test.3dshapes/p.step", STEP);
        write("lib/datasheets/p1.pdf", "%PDF");
        write("sym-lib-table", &table("sym", "${KIPRJMOD}/lib/symbols/Test.kicad_sym"));
        write("fp-lib-table", &table("fp", "${KIPRJMOD}/lib/footprints/Test.pretty"));
        dir
    }

    /// `kind: message` of the findings.
    fn found(dir: &Path) -> Vec<String> {
        scan(dir, None).into_iter().map(|f| format!("{:?} {}: {}", f.severity, f.kind, f.message)).collect()
    }

    fn assert_one(dir: &Path, needle: &str) {
        let f = found(dir);
        assert!(f.len() == 1 && f[0].contains(needle), "{f:?} should be only {needle:?}");
    }

    #[test]
    fn healthy_project_has_no_findings() {
        assert_eq!(found(&project("health_ok")), Vec::<String>::new());
    }

    #[test]
    fn broken_symbol_library() {
        let dir = project("health_symbol");
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), "(kicad_symbol_lib (symbol").unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.starts_with("Error symbol")), "{f:?}");
    }

    #[test]
    fn missing_footprint_and_model() {
        let dir = project("health_fp");
        std::fs::remove_file(dir.join("lib/3dmodels/Test.3dshapes/p.step")).unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.starts_with("Error 3d-model")), "{f:?}");
        assert!(f.iter().any(|m| m.contains("p.step is missing")), "the part checks are reused: {f:?}");

        let dir = project("health_fp_gone");
        std::fs::remove_file(dir.join("lib/footprints/Test.pretty/FP.kicad_mod")).unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.starts_with("Error footprint")), "{f:?}");
        assert!(!f.iter().any(|m| m.contains("no symbol uses")), "a footprint that is gone isn't an orphan: {f:?}");

        let dir = project("health_fp_unset");
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), SYM.replace("PRJ_Test:FP", "")).unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.starts_with("Error footprint") && m.contains("no footprint")), "{f:?}");
    }

    #[test]
    fn datasheet_and_sim_model() {
        let dir = project("health_ds");
        std::fs::remove_file(dir.join("lib/datasheets/p1.pdf")).unwrap();
        assert_one(&dir, "Error datasheet: the datasheet file is missing");

        let dir = project("health_ds_unset");
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), SYM.replace("${KIPRJMOD}/lib/datasheets/p1.pdf", "~")).unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.starts_with("Warning datasheet")), "{f:?}");

        let dir = project("health_sim");
        let sim = SYM.replace(r#"(property "MPN""#, r#"(property "Sim.Library" "${KIPRJMOD}/lib/simulation/p1.lib" (at 0 0 0)) (property "MPN""#);
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), sim).unwrap();
        assert_one(&dir, "Error sim-model");
        std::fs::create_dir_all(dir.join("lib/simulation")).unwrap();
        std::fs::write(dir.join("lib/simulation/p1.lib"), ".end").unwrap();
        assert!(found(&dir).is_empty());
    }

    #[test]
    fn part_check_errors_are_reported() {
        let dir = project("health_partcheck");
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), SYM.replace(r#"(property "Manufacturer" "Acme" (at 0 0 0))"#, "")).unwrap();
        assert_one(&dir, "Error part-check: Required field Manufacturer");
    }

    #[test]
    fn duplicate_mpn_and_inconsistent_metadata() {
        // A second symbol with the same MPN, whose manufacturer is `maker`.
        let two = |maker: &str| {
            let body = SYM.split_once("(symbol \"P1\"").unwrap().1.replace("P1_1_1", "P1b_1_1").replace("Acme", maker);
            format!("{}(symbol \"P1b\"{body}", &SYM[..SYM.len() - 1])
        };
        let dir = project("health_dup");
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), two("ACME")).unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.starts_with("Warning duplicate-mpn")), "{f:?}");
        assert!(f.iter().any(|m| m.starts_with("Warning metadata") && m.contains("spelled")), "{f:?}");

        let dir = project("health_dup_maker");
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), two("Other")).unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.contains("different manufacturers")), "{f:?}");
    }

    #[test]
    fn orphaned_files() {
        let dir = project("health_orphan");
        for rel in ["lib/footprints/Test.pretty/Old.kicad_mod", "lib/3dmodels/Test.3dshapes/old.step", "lib/datasheets/old.pdf", "lib/simulation/old.lib"] {
            std::fs::create_dir_all(dir.join(rel).parent().unwrap()).unwrap();
            std::fs::write(dir.join(rel), if rel.ends_with("kicad_mod") { FP } else { "x" }).unwrap();
        }
        let f = found(&dir);
        assert_eq!(f.iter().filter(|m| m.starts_with("Warning orphan")).count(), 4, "{f:?}");
        assert_eq!(f.len(), 4, "{f:?}");
    }

    fn sch(refs: &[(&str, &str)]) -> String {
        let body: String = refs.iter().map(|(id, r)| format!("(symbol (lib_id \"{id}\") (at 0 0 0) (property \"Reference\" \"{r}\" (at 0 0 0)))")).collect();
        format!("(kicad_sch (version 20250114) {body})")
    }

    #[test]
    fn schematic_symbols_missing_from_every_library() {
        let dir = project("health_sch");
        std::fs::write(
            dir.join("a.kicad_sch"),
            sch(&[
                ("PRJ_Test:P1", "U1"),
                ("PRJ_Test:Gone", "U2"),
                ("PRJ_Test:Gone", "U3"),
                ("Nowhere:X", "U4"),
                ("power:GND", "#PWR01"),
                ("PRJ_Test:Gone", "#FLG1"),
            ]),
        )
        .unwrap();
        let f: Vec<_> = scan(&dir, None).into_iter().filter(|f| f.kind == "schematic-symbol").collect();
        assert_eq!(f.len(), 2, "one per symbol, not per placement: {f:?}");
        assert!(f[0].item == "Nowhere:X" && f[0].message.contains("neither"));
        assert!(f[1].item == "PRJ_Test:Gone" && f[1].message.contains("U2 (a.kicad_sch), U3 (a.kicad_sch)") && !f[1].message.contains("#FLG"), "{f:?}");
        // A library on an unresolvable path variable is trusted; a healthy placement is clean.
        let g = crate::test_dir("health_sch_global");
        std::fs::write(g.join("sym-lib-table"), "(sym_lib_table (lib (name \"Device\") (type \"KiCad\") (uri \"${KLM_TEST_UNSET_VAR}/Device.kicad_sym\")))")
            .unwrap();
        std::fs::write(dir.join("a.kicad_sch"), sch(&[("PRJ_Test:P1", "U1"), ("Device:R", "R1")])).unwrap();
        assert!(!scan(&dir, Some(&g)).iter().any(|f| f.kind == "schematic-symbol"));
    }

    #[test]
    fn variable_libraries_are_resolved_and_checked() {
        let dir = project("health_vars");
        let g = crate::test_dir("health_vars_global");
        let libs = g.join("mylibs");
        std::fs::create_dir_all(&libs).unwrap();
        std::fs::write(libs.join("Device.kicad_sym"), SYM).unwrap();
        // The variable comes from kicad_common.json, as KiCad's Configure Paths dialog stores it.
        std::fs::write(
            g.join("kicad_common.json"),
            format!(r#"{{"environment":{{"vars":{{"KLM_TEST_LIBS":"{}"}}}}}}"#, libs.to_string_lossy().replace('\\', "/")),
        )
        .unwrap();
        std::fs::write(g.join("sym-lib-table"), "(sym_lib_table (lib (name \"Device\") (type \"KiCad\") (uri \"${KLM_TEST_LIBS}/Device.kicad_sym\")))")
            .unwrap();
        std::fs::write(dir.join("a.kicad_sch"), sch(&[("Device:P1", "U1"), ("Device:Gone", "U2")])).unwrap();
        let f: Vec<_> = scan(&dir, Some(&g)).into_iter().filter(|f| f.kind == "schematic-symbol").collect();
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].item == "Device:Gone" && f[0].message.contains("has no symbol"));
        // A project table pointing at a missing folder under a resolvable variable is reported too.
        std::fs::write(dir.join("sym-lib-table"), "(sym_lib_table (lib (name \"X\") (type \"KiCad\") (uri \"${KLM_TEST_LIBS}/Nope.kicad_sym\")))").unwrap();
        assert!(scan(&dir, Some(&g)).iter().any(|f| f.kind == "library-table" && f.message.contains("doesn't exist")));
    }

    #[test]
    fn orphans_match_by_full_path_not_file_name() {
        let dir = project("health_orphan_path");
        // Same name as the used footprint but in another folder, and a datasheet named like the used one.
        for rel in ["lib/footprints/Other.pretty/FP.kicad_mod", "lib/datasheets/old/p1.pdf"] {
            std::fs::create_dir_all(dir.join(rel).parent().unwrap()).unwrap();
            std::fs::write(dir.join(rel), if rel.ends_with("kicad_mod") { FP } else { "x" }).unwrap();
        }
        let f = found(&dir);
        assert_eq!(f.iter().filter(|m| m.starts_with("Warning orphan")).count(), 2, "{f:?}");
    }

    #[test]
    fn a_part_without_footprint_is_reported_once() {
        let dir = project("health_nofp_once");
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), SYM.replace("PRJ_Test:FP", "")).unwrap();
        let f = found(&dir);
        assert_eq!(f.iter().filter(|m| m.to_lowercase().contains("footprint") && m.starts_with("Error")).count(), 1, "{f:?}");
    }

    #[test]
    fn library_tables_and_references() {
        let dir = project("health_tables");
        std::fs::write(dir.join("fp-lib-table"), table("fp", "${KIPRJMOD}/lib/footprints/Nope.pretty")).unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.starts_with("Error library-table") && m.contains("Nope.pretty")), "{f:?}");

        // A footprint from a library no table lists, then one the global table lists.
        let dir = project("health_ref");
        std::fs::write(dir.join("lib/symbols/Test.kicad_sym"), SYM.replace("PRJ_Test:FP", "Ghost:FP")).unwrap();
        let f = found(&dir);
        assert!(f.iter().any(|m| m.starts_with("Error library-reference") && m.contains("Ghost")), "{f:?}");
        let global = crate::test_dir("health_global");
        std::fs::write(
            global.join("fp-lib-table"),
            "(fp_lib_table (version 7) (lib (name \"Ghost\") (type \"KiCad\") (uri \"x\") (options \"\") (descr \"\")))",
        )
        .unwrap();
        assert!(!scan(&dir, Some(&global)).iter().any(|f| f.kind == "library-reference"));

        // A project library with the global one's name shadows it.
        let global = crate::test_dir("health_global_same");
        std::fs::write(global.join("fp-lib-table"), table("fp", "x")).unwrap();
        let f = scan(&project("health_shadow"), Some(&global));
        assert!(f.iter().any(|f| f.kind == "library-table" && f.severity == Severity::Warning && f.message.contains("global")), "{f:?}");
    }

    #[test]
    fn exports() {
        let dir = project("health_export");
        std::fs::remove_file(dir.join("lib/datasheets/p1.pdf")).unwrap();
        let f = scan(&dir, None);
        assert!(has_errors(&f));
        assert!(to_csv(&f).starts_with("severity,kind,item,message\nerror,datasheet,\"Test/P1\""));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&to_json(&f)).unwrap()[0]["kind"], "datasheet");
        assert!(!has_errors(&scan(&project("health_export_ok"), None)));
    }
}
