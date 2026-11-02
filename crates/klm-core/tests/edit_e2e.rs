//! Edits an imported part and moves it to another category, then asks the real `kicad-cli` to render
//! the moved libraries. Skipped when KiCad isn't installed.

use klm_core::library::{Check, LibraryRoot, PartEdit, property};
use klm_core::part::Part;
use klm_core::{kicad_env, sexpr};
use std::path::{Path, PathBuf};
use std::process::Command;

const SYMBOL_V6: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "TESTPART" (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "TESTPART" (id 1) (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Vendor:SOT-23-ACME" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "TESTPART_0_1" (rectangle (start -2.54 1.27) (end 2.54 -1.27) (stroke (width 0.254)) (fill (type background))))
    (symbol "TESTPART_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
)"#;

const FOOTPRINT_V5: &str = r#"(module SOT-23-ACME (layer F.Cu) (tedit 5A02FF57)
  (fp_text reference REF** (at 0 -2.5) (layer F.SilkS) (effects (font (size 1 1) (thickness 0.15))))
  (fp_text value SOT-23-ACME (at 0 2.5) (layer F.Fab) (effects (font (size 1 1) (thickness 0.15))))
  (pad 1 smd rect (at -1 0.95) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask))
)"#;

fn fresh(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("klm-e2e-edit").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read(p: &Path) -> sexpr::Sexpr {
    sexpr::parse(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn recategorised_part_still_renders() {
    let cli = kicad_env::kicad_cli().ok().filter(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success()));
    let Some(cli) = cli else {
        eprintln!("kicad-cli not found; skipping");
        return;
    };
    let src = fresh("src");
    std::fs::write(src.join("t.kicad_sym"), SYMBOL_V6).unwrap();
    std::fs::write(src.join("fp.kicad_mod"), FOOTPRINT_V5).unwrap();
    std::fs::write(src.join("t.step"), "ISO-10303-21;").unwrap();
    let part = Part {
        mpn: "TP-100".into(),
        manufacturer: "ACME".into(),
        symbol_file: Some(src.join("t.kicad_sym")),
        footprint_files: vec![src.join("fp.kicad_mod")],
        model_files: vec![src.join("t.step")],
        ..Default::default()
    };
    let config = fresh("config");
    let root = LibraryRoot::global(fresh("global"), config.clone());
    root.add_part(&part, "ICs", false).unwrap();
    let g = root.dir.to_string_lossy().replace('\\', "/");

    let edit = PartEdit {
        category: Some("Power".into()),
        fields: vec![("MPN".into(), "TP-200".into()), ("Manufacturer".into(), "Globex".into()), ("Tags".into(), "ldo".into())],
    };
    let done = root.edit_part("ICs", "TP-100", &edit).unwrap();
    assert_eq!((done.name.as_str(), done.category.as_str(), done.new_lib), ("TP-200", "Power", true));

    let lib = read(&root.dir.join("symbols/Power.kicad_sym"));
    let sym = lib.child("symbol").unwrap();
    assert_eq!(sym.arg(0), Some("TP-200"));
    assert_eq!(property(sym, "Footprint"), Some("KLM_Power:SOT-23-ACME"));
    assert_eq!(property(sym, "Manufacturer"), Some("Globex"));
    let fp = read(&root.dir.join("footprints/Power.pretty/SOT-23-ACME.kicad_mod"));
    assert_eq!(fp.child("model").unwrap().arg(0), Some(format!("{g}/3dmodels/Power.3dshapes/SOT-23-ACME.step").as_str()));
    assert!(root.dir.join("3dmodels/Power.3dshapes/SOT-23-ACME.step").is_file());
    assert!(!root.dir.join("footprints/ICs.pretty/SOT-23-ACME.kicad_mod").exists());
    assert!(!root.needs_link("Power") && !root.needs_link("ICs"), "both libraries stay in KiCad's tables");

    let parts = root.list_parts();
    assert_eq!(parts.iter().map(|p| (p.category.as_str(), p.name.as_str())).collect::<Vec<_>>(), [("Power", "TP-200")]);
    let p = &parts[0];
    assert_eq!([p.footprint, p.model], [Check::Ok; 2]);

    // KiCad loads the moved library and footprint, and the now empty old library.
    let out = fresh("svg");
    for (kind, lib) in
        [("sym", "symbols/Power.kicad_sym"), ("fp", "footprints/Power.pretty"), ("sym", "symbols/ICs.kicad_sym"), ("fp", "footprints/ICs.pretty")]
    {
        let r = Command::new(&cli).args([kind, "export", "svg", "-o"]).arg(&out).arg(root.dir.join(lib)).output().unwrap();
        assert!(r.status.success(), "{lib}: {}", String::from_utf8_lossy(&r.stderr));
    }
    assert!(std::fs::read_dir(&out).unwrap().count() >= 2);
}
