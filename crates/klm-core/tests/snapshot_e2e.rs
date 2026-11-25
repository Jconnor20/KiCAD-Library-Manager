//! A real import (through `kicad-cli`) followed by rollback leaves the library and KiCad's tables
//! byte-identical to before. Skipped when KiCad isn't installed.

use klm_core::library::LibraryRoot;
use klm_core::part::Part;
use klm_core::{kicad_env, snapshot};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const SYMBOL: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "TESTPART" (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "TESTPART" (id 1) (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Vendor:SOT-23-ACME" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "TESTPART_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
)"#;

const FOOTPRINT: &str = r#"(module SOT-23-ACME (layer F.Cu) (tedit 5A02FF57)
  (fp_text reference REF** (at 0 -2.5) (layer F.SilkS) (effects (font (size 1 1) (thickness 0.15))))
  (pad 1 smd rect (at -1 0.95) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask))
)"#;

/// Every file under `dir` (except the history) with its bytes.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn go(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().unwrap() != snapshot::DIR {
                    go(base, &p, out);
                }
            } else {
                out.insert(p.strip_prefix(base).unwrap().to_string_lossy().into(), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    go(dir, dir, &mut out);
    out
}

#[test]
fn import_then_rollback_restores_everything() {
    if !kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success())) {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let base = std::env::temp_dir().join("klm-e2e").join("snapshot");
    let _ = std::fs::remove_dir_all(&base);
    let src = base.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("t.kicad_sym"), SYMBOL).unwrap();
    std::fs::write(src.join("fp.kicad_mod"), FOOTPRINT).unwrap();
    std::fs::write(src.join("t.step"), "ISO-10303-21; one").unwrap();
    std::fs::write(src.join("t2.step"), "ISO-10303-21; two, a different model").unwrap();
    std::fs::write(src.join("ds.pdf"), "%PDF").unwrap();
    let part = |step: &str| Part {
        mpn: "TP-100".into(),
        manufacturer: "ACME".into(),
        symbol_file: Some(src.join("t.kicad_sym")),
        footprint_files: vec![src.join("fp.kicad_mod")],
        model_files: vec![src.join(step)],
        datasheet_file: Some(src.join("ds.pdf")),
        ..Default::default()
    };
    let config = base.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let root = LibraryRoot::global(base.join("lib"), config.clone());

    // Into an empty library: rollback leaves nothing behind, and no table.
    root.add_part(&part("t.step"), "ICs", false).unwrap();
    assert_eq!(snapshot::list(&root).len(), 1, "import and its table linking are one snapshot");
    snapshot::rollback(&root, &snapshot::list(&root)[0].id).unwrap();
    assert!(tree(&root.dir).is_empty() && !config.join("sym-lib-table").exists());

    // Re-import with a different model over the first: the old model comes back.
    root.add_part(&part("t.step"), "ICs", false).unwrap();
    let (files, tables) = (tree(&root.dir), (tree(&config)));
    let started = std::time::Instant::now();
    root.add_part(&part("t2.step"), "ICs", true).unwrap();
    let latest = &snapshot::list(&root)[0];
    eprintln!("snapshot: {} bytes, import with snapshot took {:?}", latest.bytes, started.elapsed());
    assert_ne!(tree(&root.dir), files);
    snapshot::rollback(&root, &latest.id).unwrap();
    assert_eq!(tree(&root.dir), files);
    assert_eq!(
        tree(&config).into_iter().filter(|(k, _)| !k.ends_with(".bak")).collect::<BTreeMap<_, _>>(),
        tables.into_iter().filter(|(k, _)| !k.ends_with(".bak")).collect::<BTreeMap<_, _>>()
    );
}
