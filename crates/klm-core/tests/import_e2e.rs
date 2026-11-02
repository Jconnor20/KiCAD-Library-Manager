//! Imports a hand-made old-format part through the real `kicad-cli`, then asks KiCad to render
//! the result. Skipped when KiCad isn't installed.

use klm_core::library::LibraryRoot;
use klm_core::part::Part;
use klm_core::{kicad_env, sexpr};
use std::path::Path;
use std::process::Command;

const SYMBOL_V6: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "TESTPART" (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "TESTPART" (id 1) (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Vendor:SOT-23-ACME" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "MF" "ACME" (id 3) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "TESTPART_0_1" (rectangle (start -2.54 1.27) (end 2.54 -1.27) (stroke (width 0.254)) (fill (type background))))
    (symbol "TESTPART_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
)"#;

const FOOTPRINT_V5: &str = r#"(module SOT-23-ACME (layer F.Cu) (tedit 5A02FF57)
  (fp_text reference REF** (at 0 -2.5) (layer F.SilkS) (effects (font (size 1 1) (thickness 0.15))))
  (fp_text value SOT-23-ACME (at 0 2.5) (layer F.Fab) (effects (font (size 1 1) (thickness 0.15))))
  (pad 1 smd rect (at -1 0.95) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask))
  (model ${OLDVENDOR}/sot23.wrl (offset (xyz 0 0 0.5)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 90)))
)"#;

fn kicad_available() -> bool {
    kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success()))
}

fn fresh(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join("klm-e2e").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read(p: &Path) -> sexpr::Sexpr {
    sexpr::parse(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn import_to_global_and_project() {
    if !kicad_available() {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let src = fresh("src");
    std::fs::write(src.join("t.kicad_sym"), SYMBOL_V6).unwrap();
    std::fs::write(src.join("fp.kicad_mod"), FOOTPRINT_V5).unwrap();
    std::fs::write(src.join("t.step"), "ISO-10303-21;").unwrap();
    std::fs::write(src.join("ds.pdf"), "%PDF").unwrap();
    std::fs::write(src.join("t.lib"), ".SUBCKT TESTMODEL 1 2\n.ENDS\n").unwrap();
    let part = Part {
        mpn: "TP-100/A".into(),
        manufacturer: "ACME".into(),
        description: "Test part".into(),
        supplier_ids: vec![("LCSC".into(), "C123".into())],
        symbol_file: Some(src.join("t.kicad_sym")),
        footprint_files: vec![src.join("fp.kicad_mod")],
        model_files: vec![src.join("t.step")],
        datasheet_file: Some(src.join("ds.pdf")),
        sim_files: vec![src.join("t.lib")],
        ..Default::default()
    };

    let config = fresh("config");
    let global = LibraryRoot::global(fresh("global"), config.clone());
    let project_dir = fresh("project");
    let project = LibraryRoot::project(&project_dir);

    assert!(global.add_part(&part, "ICs", false).unwrap(), "first import registers the library");
    assert!(global.add_part(&part, "ICs", false).is_err(), "duplicate refused without overwrite");
    assert!(!global.add_part(&part, "ICs", true).unwrap(), "overwrite doesn't re-register");
    project.add_part(&part, "ICs", false).unwrap();

    // Global files use the library's real path, forward slashes.
    let g = global.dir.to_string_lossy().replace('\\', "/");
    let lib = read(&global.dir.join("symbols/ICs.kicad_sym"));
    assert_eq!(lib.children("symbol").count(), 1);
    let sym = lib.child("symbol").unwrap();
    assert_eq!(sym.arg(0), Some("TP-100_A"));
    let prop = |k| klm_core::library::property(sym, k);
    assert_eq!(prop("Footprint"), Some("KLM_ICs:SOT-23-ACME"));
    assert_eq!(prop("Datasheet"), Some(format!("{g}/datasheets/ACME_TP-100_A.pdf").as_str()));
    assert_eq!(prop("MPN"), Some("TP-100/A"));
    assert_eq!(prop("LCSC"), Some("C123"));
    assert_eq!(prop("Sim.Library"), Some(format!("{g}/simulation/TP-100_A.lib").as_str()));
    assert_eq!(prop("Sim.Name"), Some("TESTMODEL"));
    let fp = read(&global.dir.join("footprints/ICs.pretty/SOT-23-ACME.kicad_mod"));
    let model = fp.child("model").unwrap();
    assert_eq!(model.arg(0), Some(format!("{g}/3dmodels/ICs.3dshapes/SOT-23-ACME.step").as_str()));
    assert!(sexpr::to_string(model).contains("xyz 0 0 90"), "vendor rotation kept");
    assert!(global.dir.join("3dmodels/ICs.3dshapes/SOT-23-ACME.step").exists());

    // Tables: global ones in the KiCad config dir, project ones next to the project.
    let gtable = read(&config.join("sym-lib-table"));
    assert_eq!(gtable.child("lib").unwrap().child("uri").unwrap().arg(0), Some(format!("{g}/symbols/ICs.kicad_sym").as_str()));
    let ptable = read(&project_dir.join("fp-lib-table"));
    assert_eq!(ptable.child("lib").unwrap().child("name").unwrap().arg(0), Some("PRJ_ICs"));
    let psym = read(&project.dir.join("symbols/ICs.kicad_sym"));
    assert_eq!(klm_core::library::property(psym.child("symbol").unwrap(), "Footprint"), Some("PRJ_ICs:SOT-23-ACME"));

    // KiCad saving its own copy of the table drops our entry; re-linking restores it.
    std::fs::write(config.join("sym-lib-table"), "(sym_lib_table\n\t(version 7)\n)\n").unwrap();
    assert_eq!(global.unlinked(), ["ICs"]);
    assert!(global.link("ICs").unwrap());
    assert!(global.unlinked().is_empty());

    // Libraries from older KLM versions used ${KLM_LIB}; it is rewritten to the real path.
    let fp_path = global.dir.join("footprints/ICs.pretty/SOT-23-ACME.kicad_mod");
    let fp_text = std::fs::read_to_string(&fp_path).unwrap();
    std::fs::write(&fp_path, fp_text.replace(&g, "${KLM_LIB}")).unwrap();
    assert!(global.has_old_paths());
    assert_eq!(global.fix_old_paths().unwrap(), 1);
    assert!(!global.has_old_paths());
    assert_eq!(std::fs::read_to_string(&fp_path).unwrap(), fp_text);

    // Status view: everything present, then a deleted 3D model shows as broken.
    use klm_core::library::Check;
    let parts = global.list_parts();
    assert_eq!(parts.len(), 1);
    let p = &parts[0];
    assert_eq!((p.category.as_str(), p.mpn.as_str(), p.manufacturer.as_str()), ("ICs", "TP-100/A", "ACME"));
    assert_eq!([p.footprint, p.model, p.datasheet, p.sim], [Check::Ok; 4]);
    assert!(p.in_kicad);
    let model_file = global.dir.join("3dmodels/ICs.3dshapes/SOT-23-ACME.step");
    std::fs::rename(&model_file, model_file.with_extension("gone")).unwrap();
    assert_eq!(global.list_parts()[0].model, Check::Broken);
    std::fs::rename(model_file.with_extension("gone"), &model_file).unwrap();
    assert_eq!(project.list_parts()[0].datasheet, Check::Ok, "${{KIPRJMOD}} resolves to the project folder");

    // Moving the global library rewrites its files and KiCad's tables.
    let moved_dir = std::env::temp_dir().join("klm-e2e").join("moved");
    let _ = std::fs::remove_dir_all(&moved_dir);
    let moved = global.relocate(&moved_dir).unwrap();
    assert!(!global.dir.exists() && moved_dir.join("symbols/ICs.kicad_sym").exists());
    let m = moved_dir.to_string_lossy().replace('\\', "/");
    let table = std::fs::read_to_string(config.join("fp-lib-table")).unwrap();
    assert!(table.contains(&format!("{m}/footprints/ICs.pretty")) && !table.contains(&format!("{g}/")));
    assert_eq!([moved.list_parts()[0].model, moved.list_parts()[0].datasheet], [Check::Ok; 2]);
    // Moved by hand: pointing at the new folder only rewrites paths.
    let back = moved.relocate(&moved_dir).unwrap();
    assert_eq!(back.list_parts()[0].footprint, Check::Ok);
    let global = moved;

    // KiCad itself must be able to load what we wrote.
    let cli = kicad_env::kicad_cli().unwrap();
    let out = fresh("svg");
    let sym_ok = Command::new(&cli)
        .args(["sym", "export", "svg", "-o"])
        .arg(&out)
        .arg(global.dir.join("symbols/ICs.kicad_sym"))
        .output()
        .unwrap();
    assert!(sym_ok.status.success(), "{}", String::from_utf8_lossy(&sym_ok.stderr));
    let fp_ok = Command::new(&cli)
        .args(["fp", "export", "svg", "-o"])
        .arg(&out)
        .arg(global.dir.join("footprints/ICs.pretty"))
        .output()
        .unwrap();
    assert!(fp_ok.status.success(), "{}", String::from_utf8_lossy(&fp_ok.stderr));
    assert!(std::fs::read_dir(&out).unwrap().count() >= 2);
}
