//! Imports a fixture KiCad library (two categories' worth of symbols, footprints, 3D models) through the
//! scan, a dry run and a real apply. Skipped when KiCad isn't installed.

use klm_core::batch::{self, Op, Target};
use klm_core::library::{LibraryRoot, property};
use klm_core::rules::Rules;
use klm_core::sources::foreign;
use klm_core::{kicad_env, sexpr};
use std::path::Path;
use std::process::Command;

fn sym(name: &str, desc: &str, fp: &str, extra: &str) -> String {
    format!(
        r#"(symbol "{name}" {extra} (in_bom yes) (on_board yes)
    (property "Reference" "U" (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "{name}" (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "{fp}" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Description" "{desc}" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "{name}_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )"#
    )
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
fn kicad_library_is_split_and_paths_resolve() {
    if !kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success())) {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let src = fresh("foreign_src");
    let lib = format!(
        "(kicad_symbol_lib (version 20231120) (generator test)\n  {}\n  {}\n  {}\n  {}\n)",
        sym("R_0603", "Resistor 10k", "OldLib:R_0603", ""),
        sym("LM358", "Dual operational amplifier", "OldLib:SOIC-8", ""),
        sym("R_0603_alias", "Resistor alias", "OldLib:R_0603", "(extends \"R_0603\")"),
        sym("Mystery", "Frobnicator", "OldLib:Nowhere", ""),
    );
    std::fs::create_dir_all(src.join("sym")).unwrap();
    std::fs::write(src.join("sym/Device.kicad_sym"), lib).unwrap();
    let pretty = src.join("fp/OldLib.pretty");
    std::fs::create_dir_all(&pretty).unwrap();
    std::fs::create_dir_all(src.join("models")).unwrap();
    for (name, model) in [("R_0603", "${MYLIB}/models/r.step"), ("SOIC-8", "${MYLIB}/models/missing.step")] {
        std::fs::write(
            pretty.join(format!("{name}.kicad_mod")),
            format!(
                "(footprint \"{name}\" (version 20240108) (generator test) (layer \"F.Cu\")\n (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n (model \"{model}\" (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 0))))"
            ),
        )
        .unwrap();
    }
    std::fs::write(src.join("models/r.step"), "ISO-10303-21;").unwrap();
    std::fs::write(src.join("Parts.SchLib"), "x").unwrap(); // not a real Altium file: reported, not guessed
    std::fs::copy("tests/fixtures/eagle_r0603.lbr", src.join("eagle.lbr")).unwrap();

    let work = fresh("foreign_work");
    let scan = foreign::scan(&src, &Rules::default(), &work).unwrap();
    assert_eq!(scan.parts.len(), 5, "4 symbols (the derived one flattened) + the converted Eagle part");
    assert!(scan.unsupported.iter().any(|s| s.contains("Parts.SchLib") && s.contains("could not convert")), "{:?}", scan.unsupported);
    let alias = &scan.parts.iter().find(|f| f.part.mpn == "R_0603_alias").unwrap().part;
    assert!(std::fs::read_to_string(alias.symbol_file.as_ref().unwrap()).unwrap().contains("R_0603_alias_1_1"));
    let cat = |m: &str| scan.parts.iter().find(|f| f.part.mpn == m).unwrap().category.clone();
    assert_eq!((cat("R_0603").as_str(), cat("LM358").as_str(), cat("Mystery").as_str()), ("Resistors", "ICs", "Misc"));
    let soic = &scan.parts.iter().find(|f| f.part.mpn == "LM358").unwrap().part;
    assert!(soic.warnings.iter().any(|w| w.contains("3D model")));

    let global = LibraryRoot::global(fresh("foreign_global"), fresh("foreign_config"));
    let ops = scan.parts.into_iter().map(|f| Op::ImportPart { part: f.part, category: Some(f.category), target: Target::Global, overwrite: false }).collect();
    let plan = batch::plan(ops, Some(&global), None);
    assert!(global.categories().is_empty(), "dry run writes nothing");
    assert!(plan.to_string().contains("3D model of SOIC-8 not found"));
    let results = batch::apply(&plan, |_, _| {});
    assert!(results.iter().all(|r| r.error.is_none()), "{:?}", results.iter().map(|r| &r.error).collect::<Vec<_>>());

    let g = global.dir.to_string_lossy().replace('\\', "/");
    let r = read(&global.dir.join("symbols/Resistors.kicad_sym"));
    assert_eq!(property(r.child("symbol").unwrap(), "Footprint"), Some("KLM_Resistors:R_0603"));
    let fp = read(&global.dir.join("footprints/Resistors.pretty/R_0603.kicad_mod"));
    let model = fp.child("model").unwrap().arg(0).unwrap().to_string();
    assert_eq!(model, format!("{g}/3dmodels/Resistors.3dshapes/R_0603.step"));
    assert!(Path::new(&model).is_file(), "model path resolves");
    assert!(global.dir.join("symbols/ICs.kicad_sym").is_file() && global.dir.join("symbols/Misc.kicad_sym").is_file());

    // Importing again is flagged in the dry run and refused, never a silent overwrite.
    let again = foreign::scan(&src, &Rules::default(), &work).unwrap();
    let ops = again.parts.into_iter().map(|f| Op::ImportPart { part: f.part, category: Some(f.category), target: Target::Global, overwrite: false }).collect();
    assert!(batch::plan(ops, Some(&global), None).to_string().contains("already exists"));
}
