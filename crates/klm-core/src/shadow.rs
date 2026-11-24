//! A project's own `fp-lib-table` can list a `KLM_<Category>` library under the global library's nickname.
//! KiCad lets the project entry win, so a footprint imported into the global library is "not found" there.
//! This copies the missing footprints into such project libraries. It never overwrites a file.
//!
//! shortcut: symbols are not synced. A project `sym-lib-table` entry that shadows a `KLM_*` symbol
//! library would need the two `.kicad_sym` files merged, which is not a copy.

use crate::library::{LibraryRoot, Scope};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// One footprint copied into a project's own library.
#[derive(Clone, Debug, PartialEq)]
pub struct Copied {
    pub project: PathBuf,
    pub nickname: String,
    pub footprint: String,
}

#[derive(Clone, Debug, Default)]
pub struct SyncReport {
    pub copied: Vec<Copied>,
    pub warnings: Vec<String>,
}

impl SyncReport {
    /// One line per project library, plus the warnings, in the words the logs use.
    pub fn notes(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        let mut i = 0;
        while i < self.copied.len() {
            let c = &self.copied[i];
            let group: Vec<&str> = self.copied[i..].iter().take_while(|o| o.project == c.project && o.nickname == c.nickname).map(|o| o.footprint.as_str()).collect();
            out.push(format!("Copied {} footprint(s) into {} in project {} (its own library hides the global one): {}", group.len(), c.nickname, c.project.display(), group.join(", ")));
            i += group.len();
        }
        out.extend(self.warnings.iter().map(|w| format!("Warning: {w}")));
        out
    }
}

/// The folders of KiCad's recent projects (`system.file_history` in `kicad.json`) that still exist.
/// A missing or unreadable file gives an empty list.
pub fn recent_projects(config_dir: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(config_dir.join("kicad.json")).unwrap_or_default();
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
    let mut dirs: Vec<PathBuf> = vec![];
    for f in json["system"]["file_history"].as_array().into_iter().flatten().filter_map(|f| f.as_str()) {
        let file = PathBuf::from(f);
        if let Some(dir) = file.parent().filter(|_| file.is_file()) && !dirs.iter().any(|d| d == dir) {
            dirs.push(dir.to_path_buf());
        }
    }
    dirs
}

/// [`sync_in`] for the projects KiCad lists as recent, reading `kicad.json` from the global root's KiCad config folder.
pub fn sync_shadowing_projects(global: &LibraryRoot, dry_run: bool) -> SyncReport {
    if global.scope != Scope::Global {
        return SyncReport::default();
    }
    sync_in(global, &recent_projects(&global.table_dir), dry_run)
}

/// For each project folder, every `KLM_*` footprint library its `fp-lib-table` lists in another folder than the
/// global one gets the global footprints it lacks. With `dry_run` nothing is written; the report says what would be.
pub fn sync_in(global: &LibraryRoot, projects: &[PathBuf], dry_run: bool) -> SyncReport {
    let mut report = SyncReport::default();
    for project in projects {
        for (nick, uri) in crate::health::table_entries(&project.join("fp-lib-table")) {
            let Some(cat) = nick.strip_prefix("KLM_") else { continue };
            let from = global.dir.join("footprints").join(format!("{cat}.pretty"));
            let Some(to) = crate::health::expand(&uri, project, Some(&global.table_dir)).map(PathBuf::from) else { continue };
            let same = matches!((from.canonicalize(), to.canonicalize()), (Ok(a), Ok(b)) if a == b);
            if same || !from.is_dir() || !to.is_dir() {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&from) else { continue };
            let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "kicad_mod")).collect();
            files.sort();
            for src in files {
                let name = src.file_name().unwrap_or_default();
                if to.join(name).exists() {
                    continue;
                }
                if !dry_run && let Err(e) = std::fs::copy(&src, to.join(name)) {
                    report.warnings.push(format!("could not copy {} into {}: {e}", src.display(), to.display()));
                    continue;
                }
                report.copied.push(Copied { project: project.clone(), nickname: nick.clone(), footprint: src.file_stem().unwrap_or_default().to_string_lossy().into_owned() });
            }
        }
    }
    report
}

thread_local! {
    static NOTES: RefCell<Vec<String>> = const { RefCell::new(vec![]) };
}

/// Called after a successful import into the global library: a failure here is a warning, never an import failure.
pub(crate) fn sync_after_import(global: &LibraryRoot) {
    let notes = sync_shadowing_projects(global, false).notes();
    NOTES.with(|n| *n.borrow_mut() = notes);
}

/// What the last import on this thread copied into projects (see [`sync_after_import`]), like `symbols::take_note`.
pub fn take_notes() -> Vec<String> {
    NOTES.with(|n| std::mem::take(&mut *n.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(libs: &[(&str, &str)]) -> String {
        let mut t = String::from("(fp_lib_table\n\t(version 7)\n");
        for (n, u) in libs {
            t += &format!("\t(lib (name \"{n}\")(type \"KiCad\")(uri \"{u}\")(options \"\")(descr \"\"))\n");
        }
        t + ")\n"
    }

    #[test]
    fn copies_only_missing_footprints_into_shadowing_project_libraries() {
        let base = crate::test_dir("shadow_sync");
        let cfg = base.join("cfg");
        let global = LibraryRoot::global(base.join("lib"), cfg.clone());
        let gpretty = |c: &str| global.dir.join("footprints").join(format!("{c}.pretty"));
        for c in ["X", "Y"] {
            std::fs::create_dir_all(gpretty(c)).unwrap();
        }
        std::fs::write(gpretty("X").join("A.kicad_mod"), "global A").unwrap();
        std::fs::write(gpretty("X").join("B.kicad_mod"), "global B").unwrap();
        std::fs::write(gpretty("Y").join("C.kicad_mod"), "global C").unwrap();

        let prj = base.join("prj");
        let local = prj.join("lib/footprints/KLM_X.pretty");
        let other = prj.join("lib/footprints/Other.pretty");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(local.join("A.kicad_mod"), "local A").unwrap();
        std::fs::write(prj.join("p.kicad_pro"), "{}").unwrap();
        let g_y = gpretty("Y").to_string_lossy().replace('\\', "/");
        std::fs::write(
            prj.join("fp-lib-table"),
            table(&[("KLM_X", "${KIPRJMOD}/lib/footprints/KLM_X.pretty"), ("Other", "${KIPRJMOD}/lib/footprints/Other.pretty"), ("KLM_Y", &g_y)]),
        )
        .unwrap();

        std::fs::create_dir_all(&cfg).unwrap();
        let hist = serde_json::json!({"system": {"file_history": [prj.join("p.kicad_pro"), base.join("gone/p.kicad_pro")]}});
        std::fs::write(cfg.join("kicad.json"), hist.to_string()).unwrap();
        assert_eq!(recent_projects(&cfg), vec![prj.clone()]);

        let dry = sync_shadowing_projects(&global, true);
        assert_eq!(dry.copied.len(), 1);
        assert!(!local.join("B.kicad_mod").exists());

        let r = sync_shadowing_projects(&global, false);
        assert_eq!(r.copied, vec![Copied { project: prj.clone(), nickname: "KLM_X".into(), footprint: "B".into() }]);
        assert!(r.warnings.is_empty());
        assert_eq!(std::fs::read_to_string(local.join("A.kicad_mod")).unwrap(), "local A");
        assert_eq!(std::fs::read_to_string(local.join("B.kicad_mod")).unwrap(), "global B");
        assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(gpretty("Y")).unwrap().count(), 1);
        assert!(r.notes()[0].contains("KLM_X") && r.notes()[0].contains('B'));
        assert!(sync_shadowing_projects(&global, false).copied.is_empty());

        // Junk or no kicad.json: nothing to do, no error.
        std::fs::write(cfg.join("kicad.json"), "not json").unwrap();
        assert!(recent_projects(&cfg).is_empty());
        std::fs::remove_file(cfg.join("kicad.json")).unwrap();
        assert!(recent_projects(&cfg).is_empty());
    }
}
