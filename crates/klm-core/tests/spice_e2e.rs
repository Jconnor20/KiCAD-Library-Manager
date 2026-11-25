//! End-to-end tests for SPICE preservation on foreign import, ZIP `.mod` import,
//! close-variant matching across library parts, and datasheet + similar-model synthesis.

use flate2::{Compression, write::ZlibEncoder};
use klm_core::library::{Check, LibraryRoot, property};
use klm_core::part::Part;
use klm_core::rules::Rules;
use klm_core::sources::{foreign, zip_import};
use klm_core::spice::SimOrigin;
use std::io::Write;
use std::path::PathBuf;

fn fresh(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("klm-spice-e2e").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn make_pdf(text: &str) -> Vec<u8> {
    let payload = format!("BT /F1 10 Tf 72 720 Td ({text}) Tj ET");
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(payload.as_bytes()).unwrap();
    let compressed = enc.finish().unwrap();
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj << /Filter /FlateDecode /Length 999 >>\nstream\n");
    pdf.extend_from_slice(&compressed);
    pdf.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");
    pdf
}

fn diode_symbol(name: &str) -> String {
    format!(
        r#"(kicad_symbol_lib (version 20241209) (generator kicad_symbol_editor)
  (symbol "{name}" (in_bom yes) (on_board yes)
    (property "Reference" "D" (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "{name}" (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "{name}_1_1"
      (pin passive line (at -5.08 0 0) (length 2.54) (name "K" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 5.08 0 180) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))
    )
  )
)"#
    )
}

fn opamp_symbol(name: &str) -> String {
    format!(
        r#"(kicad_symbol_lib (version 20241209) (generator kicad_symbol_editor)
  (symbol "{name}" (in_bom yes) (on_board yes)
    (property "Reference" "U" (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "{name}" (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "{name}_1_1"
      (pin output line (at 7.62 0 180) (length 2.54) (name "OUT" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
      (pin input line (at -7.62 -2.54 0) (length 2.54) (name "IN-" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))
      (pin input line (at -7.62 2.54 0) (length 2.54) (name "IN+" (effects (font (size 1.27 1.27)))) (number "3" (effects (font (size 1.27 1.27)))))
      (pin power_in line (at 0 -5.08 90) (length 2.54) (name "VEE" (effects (font (size 1.27 1.27)))) (number "4" (effects (font (size 1.27 1.27)))))
      (pin power_in line (at 0 5.08 270) (length 2.54) (name "VCC" (effects (font (size 1.27 1.27)))) (number "8" (effects (font (size 1.27 1.27)))))
    )
  )
)"#
    )
}

#[test]
fn foreign_scan_preserves_spice_files_and_sets_device_and_pins() {
    let dir = fresh("foreign-sim");
    let kicad_dir = dir.join("vendor_lib");
    std::fs::create_dir_all(kicad_dir.join("sim")).unwrap();
    let sym = r#"(kicad_symbol_lib (version 20241209) (generator kicad_symbol_editor)
  (symbol "1N4148" (in_bom yes) (on_board yes)
    (property "Reference" "D" (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "1N4148" (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Sim.Library" "sim/diodes.mod" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Sim.Name" "1N4148" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "1N4148_1_1"
      (pin passive line (at -5.08 0 0) (length 2.54) (name "K" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 5.08 0 180) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))
    )
  )
)"#
    .to_string();
    std::fs::write(kicad_dir.join("Diodes.kicad_sym"), sym).unwrap();
    std::fs::write(kicad_dir.join("sim/diodes.mod"), ".model BAV99 D(IS=4n)\n.model 1N4148 D(IS=2.52n RS=0.568 N=1.752 BV=100)\n").unwrap();

    let scan = foreign::scan(&kicad_dir, &Rules::default(), &dir.join("work")).unwrap();
    assert_eq!(scan.parts.len(), 1);
    let found = &scan.parts[0];
    assert_eq!(found.part.sim_files.len(), 1);

    let lib_dir = dir.join("lib");
    let cfg_dir = dir.join("config");
    std::fs::create_dir_all(&lib_dir).unwrap();
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let root = LibraryRoot::global(lib_dir, cfg_dir);
    root.add_part(&found.part, &found.category, false).unwrap();

    let status = &root.list_parts()[0];
    assert_eq!(status.sim, Check::Ok);

    let lib = klm_core::sexpr::parse(&std::fs::read_to_string(root.dir.join(format!("symbols/{}.kicad_sym", status.category))).unwrap()).unwrap();
    let sym = lib.child("symbol").unwrap();
    assert_eq!(property(sym, "Sim.Name"), Some("1N4148"));
    assert_eq!(property(sym, "Sim.Device"), Some("D"));
    assert_eq!(property(sym, "Sim.Pins"), Some("1=K 2=A"));
}

#[test]
fn close_variant_matching_and_datasheet_plus_similar_generation_on_import() {
    let dir = fresh("close-and-synth");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let lib_dir = dir.join("lib");
    let cfg_dir = dir.join("config");
    std::fs::create_dir_all(&lib_dir).unwrap();
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let root = LibraryRoot::global(lib_dir, cfg_dir);

    // Seed library with an explicit LM358 subcircuit and an existing diode model.
    let lm358_sym = src.join("LM358.kicad_sym");
    let lm358_sp = src.join("LM358.lib");
    std::fs::write(&lm358_sym, opamp_symbol("LM358")).unwrap();
    std::fs::write(&lm358_sp, ".subckt LM358 IN+ IN- VCC VEE OUT\nRin IN+ IN- 2Meg\nEout OUT 0 IN+ IN- 100k\n.ends LM358\n").unwrap();
    root.add_part(
        &Part {
            mpn: "LM358".into(),
            manufacturer: "TI".into(),
            description: "Dual operational amplifier".into(),
            symbol_file: Some(lm358_sym),
            sim_files: vec![lm358_sp],
            ..Default::default()
        },
        "Amplifiers",
        false,
    )
    .unwrap();

    let base_diode_sym = src.join("1N4148.kicad_sym");
    let base_diode_sp = src.join("1N4148.mod");
    std::fs::write(&base_diode_sym, diode_symbol("1N4148")).unwrap();
    std::fs::write(&base_diode_sp, ".model 1N4148 D(IS=2.5n N=1.75 RS=0.5 CJO=4p BV=100)\n").unwrap();
    root.add_part(
        &Part {
            mpn: "1N4148".into(),
            manufacturer: "Vishay".into(),
            description: "Fast switching diode".into(),
            symbol_file: Some(base_diode_sym),
            sim_files: vec![base_diode_sp],
            ..Default::default()
        },
        "Diodes",
        false,
    )
    .unwrap();

    // 1. Import a package variant (LM358DR) WITHOUT supplying a sim file -> should close-match LM358 automatically.
    let lm358dr_sym = src.join("LM358DR.kicad_sym");
    std::fs::write(&lm358dr_sym, opamp_symbol("LM358DR")).unwrap();
    root.add_part(
        &Part {
            mpn: "LM358DR".into(),
            manufacturer: "TI".into(),
            description: "Dual operational amplifier SOIC-8".into(),
            symbol_file: Some(lm358dr_sym),
            ..Default::default()
        },
        "Amplifiers",
        false,
    )
    .unwrap();

    let parts = root.list_parts();
    let lm358dr = parts.iter().find(|p| p.mpn == "LM358DR").unwrap();
    assert_eq!(lm358dr.sim, Check::Ok, "LM358DR should automatically close-match LM358");
    let sim_text = std::fs::read_to_string(root.dir.join("simulation/LM358DR.lib")).unwrap();
    assert!(sim_text.contains(".subckt LM358"), "{sim_text}");
    assert!(sim_text.contains("Close SPICE model for LM358DR"), "{sim_text}");

    // 2. Import an unrelated Schottky diode B5819W with a datasheet PDF and NO sim file:
    //    should adapt the existing library diode model with B5819W's datasheet Vf / Vr!
    let b5819_sym = src.join("B5819W.kicad_sym");
    let b5819_pdf = src.join("B5819W.pdf");
    std::fs::write(&b5819_sym, diode_symbol("B5819W")).unwrap();
    std::fs::write(&b5819_pdf, make_pdf("B5819W 40V 1A Schottky Barrier Diode Forward Voltage Vf 0.45V Reverse Voltage VRRM 40V")).unwrap();
    root.add_part(
        &Part {
            mpn: "B5819W".into(),
            manufacturer: "Diodes Inc".into(),
            description: "Schottky rectifier diode 40V 1A".into(),
            symbol_file: Some(b5819_sym),
            datasheet_file: Some(b5819_pdf),
            ..Default::default()
        },
        "Diodes",
        false,
    )
    .unwrap();

    let b5819 = root.list_parts().into_iter().find(|p| p.mpn == "B5819W").unwrap();
    assert_eq!(b5819.sim, Check::Ok, "B5819W should synthesize from datasheet + similar library diode");
    let b5819_sim = std::fs::read_to_string(root.dir.join("simulation/B5819W.lib")).unwrap();
    assert!(b5819_sim.contains("adapted from similar model 1N4148"), "{b5819_sim}");
    assert!(b5819_sim.contains("BV=40"), "{b5819_sim}");

    // 3. Test resolve_and_attach_sim explicitly on a pre-existing part that had no sim file.
    let cap_sym = src.join("C_100n.kicad_sym");
    std::fs::write(
        &cap_sym,
        r#"(kicad_symbol_lib (version 20241209)
  (symbol "GRM188R71H104KA93D" (in_bom yes) (on_board yes)
    (property "Reference" "C" (at 0 0 0))
    (property "Value" "100nF" (at 0 0 0))
    (property "Footprint" "" (at 0 0 0))
    (property "Datasheet" "" (at 0 0 0))
    (symbol "GRM188R71H104KA93D_1_1"
      (pin passive line (at 0 2.54 270) (length 2.54) (name "~") (number "1"))
      (pin passive line (at 0 -2.54 90) (length 2.54) (name "~") (number "2"))
    )
  )
)"#,
    )
    .unwrap();
    root.add_part(
        &Part { mpn: "GRM188R71H104KA93D".into(), description: "100nF 50V X7R MLCC Capacitor".into(), symbol_file: Some(cap_sym), ..Default::default() },
        "Capacitors",
        false,
    )
    .unwrap();
    // Already synthesized during add_part! Verify it exists and parses cleanly:
    let cap_st = root.list_parts().into_iter().find(|p| p.mpn == "GRM188R71H104KA93D").unwrap();
    assert_eq!(cap_st.sim, Check::Ok);
    let cap_lib = std::fs::read_to_string(root.dir.join("simulation/GRM188R71H104KA93D.lib")).unwrap();
    assert!(cap_lib.contains("C_main"), "{cap_lib}");

    // Also verify ZIP import recognizes .mod SPICE models.
    let zp = src.join("diode.zip");
    {
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
        w.start_file("D1234.mod", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b".model D1234 D(IS=1p BV=60)\n").unwrap();
        w.finish().unwrap();
    }
    let extracted = zip_import::read_zip(&zp, &dir.join("zip_work")).unwrap().unwrap();
    assert_eq!(extracted.sim_files.len(), 1);
    assert!(extracted.sim_files[0].extension().and_then(|e| e.to_str()) == Some("mod"));

    // Verify SimOrigin summary formatting works for GeneratedFromDatasheet.
    let summary = SimOrigin::GeneratedFromDatasheet { based_on: Some("1N4148".into()), notes: vec![] }.summary();
    assert!(summary.contains("1N4148"));
}
