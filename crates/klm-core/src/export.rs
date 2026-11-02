//! Portable export: parts as one standalone KiCad symbol library, one `.pretty` and their files, all
//! paths relative, in a ZIP that Altium and Fusion can read through their KiCad importers.
//!
//! Layout: `symbols/klm_export.kicad_sym`, `footprints/klm_export.pretty/`, `3dmodels/`, `datasheets/`,
//! `simulation/`, plus `sym-lib-table` and `fp-lib-table` snippets to paste into KiCad.
//! Paths are relative to the file that holds them (`../3dmodels/x.step` from the symbol library).

use crate::library::{LibraryRoot, property, sanitize, set_property};
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::Path;

const NICK: &str = "klm_export";

/// Writes `parts` ((category, symbol name) pairs of `root`) to `zip_path`. Returns how many were exported.
pub fn export_zip(root: &LibraryRoot, parts: &[(String, String)], zip_path: &Path) -> Result<usize> {
    if parts.is_empty() {
        bail!("nothing to export");
    }
    let stage = std::env::temp_dir().join(format!("klm-export-{}-{}", std::process::id(), crate::cache::now()));
    let result = build(root, parts, &stage, zip_path);
    let _ = std::fs::remove_dir_all(&stage);
    result.map(|_| parts.len())
}

/// Stages through `copy_part_to`, which already carries the files and rewrites their paths; then makes them relative.
fn build(root: &LibraryRoot, parts: &[(String, String)], stage: &Path, zip_path: &Path) -> Result<()> {
    let staging = LibraryRoot::global(stage.join("lib"), stage.join("tables"));
    std::fs::create_dir_all(&staging.table_dir)?;
    for (cat, name) in parts {
        root.copy_part_to(cat, name, &staging, true).with_context(|| format!("exporting {name}"))?;
    }
    let prefix = format!("{}/", staging.dir.to_string_lossy().replace('\\', "/").trim_end_matches('/'));

    let mut out = zip::ZipWriter::new(std::fs::File::create(zip_path).with_context(|| format!("creating {}", zip_path.display()))?);
    let opts = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, bytes: &[u8]| -> Result<()> {
        out.start_file(name, opts)?;
        Ok(out.write_all(bytes)?)
    };

    let mut lib: Option<Sexpr> = None;
    let mut cats: Vec<String> = parts.iter().map(|(c, _)| sanitize(c)).collect();
    cats.sort();
    cats.dedup();
    for cat in cats {
        let mut part_lib = sexpr::parse(&std::fs::read_to_string(staging.dir.join(format!("symbols/{cat}.kicad_sym")))?)?;
        for sym in part_lib.items_mut().iter_mut().filter(|c| c.head() == Some("symbol")) {
            let fp = property(sym, "Footprint").and_then(|f| f.strip_prefix(&format!("{}:", staging.nickname(&cat)))).map(|f| format!("{NICK}:{f}"));
            if let Some(fp) = fp {
                set_property(sym, "Footprint", &fp);
            }
            for key in ["Datasheet", "Sim.Library"] {
                let rel = property(sym, key).and_then(|v| v.strip_prefix(&prefix)).map(|r| format!("../{r}"));
                if let Some(rel) = rel {
                    set_property(sym, key, &rel);
                }
            }
        }
        match &mut lib {
            None => lib = Some(part_lib),
            Some(all) => {
                let symbols = part_lib.items().iter().filter(|c| c.head() == Some("symbol")).cloned();
                all.items_mut().extend(symbols);
            }
        }

        let pretty = staging.dir.join(format!("footprints/{cat}.pretty"));
        for e in std::fs::read_dir(&pretty).into_iter().flatten().flatten() {
            let mut fp = sexpr::parse(&std::fs::read_to_string(e.path())?)?;
            for model in fp.items_mut().iter_mut().filter(|c| c.head() == Some("model")) {
                if let Some(rel) = model.arg(0).and_then(|m| m.strip_prefix(&prefix)).map(String::from) {
                    model.set_arg(0, Sexpr::str(&format!("../../{rel}")));
                }
            }
            put(&format!("footprints/{NICK}.pretty/{}", e.file_name().to_string_lossy()), sexpr::to_string(&fp).as_bytes())?;
        }
    }
    put("symbols/klm_export.kicad_sym", sexpr::to_string(&lib.context("nothing to export")?).as_bytes())?;

    for dir in ["3dmodels", "datasheets", "simulation"] {
        let mut todo = vec![staging.dir.join(dir)];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    todo.push(e.path());
                } else {
                    let rel = e.path().strip_prefix(&staging.dir)?.to_string_lossy().replace('\\', "/");
                    put(&rel, &std::fs::read(e.path())?)?;
                }
            }
        }
    }

    let table = |kind: &str, uri: &str| format!("({kind}_lib_table\n\t(version 7)\n\t(lib (name \"{NICK}\")(type \"KiCad\")(uri \"{uri}\")(options \"\")(descr \"Exported from KLM\"))\n)\n");
    put("sym-lib-table", table("sym", "${KIPRJMOD}/symbols/klm_export.kicad_sym").as_bytes())?;
    put("fp-lib-table", table("fp", "${KIPRJMOD}/footprints/klm_export.pretty").as_bytes())?;
    out.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_paths_resolve_inside_the_bundle() {
        let base = crate::test_dir("export_zip");
        std::fs::create_dir_all(base.join("config")).unwrap();
        let root = LibraryRoot::global(base.join("global"), base.join("config"));
        root.ensure_layout().unwrap();
        let d = root.dir.to_string_lossy().replace('\\', "/");
        let sym = |n: &str| format!("(symbol \"{n}\" (property \"Footprint\" \"KLM_ICs:FP_{n}\" (at 0 0 0)) (property \"Datasheet\" \"{d}/datasheets/{n}.pdf\" (at 0 0 0)))");
        std::fs::write(root.dir.join("symbols/ICs.kicad_sym"), format!("(kicad_symbol_lib (version 20241209) (generator test)\n{}{})", sym("A"), sym("B"))).unwrap();
        std::fs::create_dir_all(root.dir.join("footprints/ICs.pretty")).unwrap();
        for n in ["A", "B"] {
            std::fs::write(root.dir.join(format!("footprints/ICs.pretty/FP_{n}.kicad_mod")), format!("(footprint \"FP_{n}\" (model \"{d}/3dmodels/{n}.step\"))")).unwrap();
            std::fs::write(root.dir.join(format!("3dmodels/{n}.step")), n).unwrap();
            std::fs::write(root.dir.join(format!("datasheets/{n}.pdf")), n).unwrap();
        }
        let parts = vec![("ICs".to_string(), "A".to_string()), ("ICs".to_string(), "B".to_string())];
        let zip = base.join("out.zip");
        assert_eq!(export_zip(&root, &parts, &zip).unwrap(), 2);

        let out = base.join("unzipped");
        zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap().extract(&out).unwrap();
        let text = std::fs::read_to_string(out.join("symbols/klm_export.kicad_sym")).unwrap();
        let lib = sexpr::parse(&text).unwrap();
        assert_eq!(lib.children("symbol").count(), 2);
        for s in lib.children("symbol") {
            let fp = property(s, "Footprint").unwrap().strip_prefix("klm_export:").unwrap();
            let fp_file = out.join(format!("footprints/klm_export.pretty/{fp}.kicad_mod"));
            let ds = out.join("symbols").join(property(s, "Datasheet").unwrap());
            assert!(ds.is_file(), "{}", ds.display());
            let footprint = sexpr::parse(&std::fs::read_to_string(&fp_file).unwrap()).unwrap();
            let model = footprint.child("model").unwrap().arg(0).unwrap();
            assert!(!model.contains(':') && !model.contains("${") && !model.starts_with('/'), "{model}");
            assert!(fp_file.parent().unwrap().join(model).is_file(), "{model}");
        }
        for t in ["sym-lib-table", "fp-lib-table"] {
            assert!(std::fs::read_to_string(out.join(t)).unwrap().contains("klm_export"));
        }
        assert!(!text.contains(&d) && !text.contains("${KLM"));

        // With kicad-cli installed, KiCad itself must be able to read what we wrote.
        if let Ok(cli) = crate::kicad_env::kicad_cli() {
            let ok = |args: &[&std::ffi::OsStr]| std::process::Command::new(&cli).args(args).output().unwrap().status.success();
            let up = base.join("up.kicad_sym");
            assert!(ok(&["sym".as_ref(), "upgrade".as_ref(), "-o".as_ref(), up.as_os_str(), out.join("symbols/klm_export.kicad_sym").as_os_str()]));
            assert!(ok(&["fp".as_ref(), "upgrade".as_ref(), out.join("footprints/klm_export.pretty").as_os_str()]));
        }
    }
}
