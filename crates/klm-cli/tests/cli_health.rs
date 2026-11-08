//! `klm-cli health` end to end, on a project seeded in a sandbox (`KLM_CONFIG_DIR`). The project has one
//! complete part and one with no footprint and no datasheet.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn klm(sandbox: &Path, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_klm-cli"));
    // Remove the overrides a developer may have set, so only the sandbox decides.
    c.env_remove("KLM_SETTINGS").env_remove("KLM_KICAD_CONFIG").env_remove("KLM_USE_REAL_KEYS");
    c.env("KLM_CONFIG_DIR", sandbox);
    c.args(args).output().unwrap()
}

fn fresh(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("klm-cli-health").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const SYMBOLS: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "RC0603-10K" (in_bom yes) (on_board yes)
    (property "Reference" "R" (id 0) (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "Value" "10kΩ" (id 1) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Footprint" "PRJ_Resistors:R_0603" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "${KIPRJMOD}/lib/datasheets/RC.pdf" (id 3) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "MPN" "RC0603-10K" (id 4) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Manufacturer" "ACME" (id 5) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Description" "Resistor 10kΩ" (id 6) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Resistance" "10kΩ" (id 7) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "ki_keywords" "resistor res 10kΩ" (id 8) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "RC0603-10K_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "1" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 5.08 0 180) (length 2.54) (name "2" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27))))))
  )
  (symbol "RC-INCOMPLETE" (in_bom yes) (on_board yes)
    (property "Reference" "R" (id 0) (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "Value" "4k7" (id 1) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Footprint" "" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "" (id 3) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "MPN" "RC-INCOMPLETE" (id 4) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Manufacturer" "ACME" (id 5) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Description" "Missing footprint and datasheet" (id 6) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "RC-INCOMPLETE_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "1" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 5.08 0 180) (length 2.54) (name "2" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27))))))
  )
)
"#;

/// A STEP file with both markers, so the model check does not call it cut off.
const STEP: &str = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";

const FOOTPRINT: &str = r#"(footprint "R_0603" (layer "F.Cu")
  (fp_rect (start -1.6 -0.9) (end 1.6 0.9) (stroke (width 0.05) (type default)) (fill none) (layer "F.CrtYd"))
  (fp_line (start -2.2 -0.9) (end -2.2 0.9) (stroke (width 0.12) (type solid)) (layer "F.SilkS"))
  (pad "1" smd rect (at -0.8 0) (size 0.9 0.95) (layers "F.Cu" "F.Mask" "F.Paste"))
  (pad "2" smd rect (at 0.8 0) (size 0.9 0.95) (layers "F.Cu" "F.Mask" "F.Paste"))
  (model "${KIPRJMOD}/lib/3dmodels/R_0603.step" (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 0)))
)
"#;

/// A project folder with `lib/` (symbols, footprint, 3D model, datasheet) and the project's fp-lib-table.
fn project(dir: &Path) -> PathBuf {
    let lib = dir.join("lib");
    std::fs::create_dir_all(lib.join("symbols")).unwrap();
    std::fs::create_dir_all(lib.join("footprints/Resistors.pretty")).unwrap();
    std::fs::create_dir_all(lib.join("3dmodels")).unwrap();
    std::fs::create_dir_all(lib.join("datasheets")).unwrap();
    std::fs::write(lib.join("symbols/Resistors.kicad_sym"), SYMBOLS).unwrap();
    std::fs::write(lib.join("footprints/Resistors.pretty/R_0603.kicad_mod"), FOOTPRINT).unwrap();
    std::fs::write(lib.join("3dmodels/R_0603.step"), STEP).unwrap();
    std::fs::write(lib.join("datasheets/RC.pdf"), "%PDF-1.4\n").unwrap();
    std::fs::write(
        dir.join("fp-lib-table"),
        "(fp_lib_table\n  (version 7)\n  (lib (name \"PRJ_Resistors\")(type \"KiCad\")(uri \"${KIPRJMOD}/lib/footprints/Resistors.pretty\")(options \"\")(descr \"\"))\n)\n",
    )
    .unwrap();
    dir.to_path_buf()
}

/// Sandbox plus project, with the project passed as `--project`.
fn setup(name: &str) -> (PathBuf, PathBuf) {
    let sb = fresh(name);
    let proj = project(&sb.join("proj"));
    (sb, proj)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn health_text_flags_the_incomplete_part_and_exits_1() {
    let (sb, proj) = setup("text");
    let out = klm(&sb, &["health", "--project", proj.to_str().unwrap()]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "stdout:\n{text}\nstderr:\n{}", String::from_utf8_lossy(&out.stderr));

    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.last().is_some_and(|l| l.ends_with("findings")), "{text}");
    let incomplete: Vec<&&str> = lines.iter().filter(|l| l.contains("Resistors/RC-INCOMPLETE")).collect();
    assert!(incomplete.iter().any(|l| l.starts_with("error") && l.contains("footprint")), "{text}");
    assert!(incomplete.iter().any(|l| l.starts_with("warning") && l.contains("datasheet")), "{text}");
    assert!(!lines.iter().any(|l| l.contains("Resistors/RC0603-10K") && (l.contains("footprint") || l.contains("datasheet") || l.contains("3d-model"))), "complete part flagged:\n{text}");
    // The sandbox's KiCad tables were looked up there, not in the real %APPDATA%.
    assert!(sb.join("kicad").is_dir());
}

#[test]
fn health_json_parses_and_flags_the_part() {
    let (sb, proj) = setup("json");
    let out = klm(&sb, &["health", "--json", "--project", proj.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let rows: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("health --json prints JSON");
    let rows = rows.as_array().expect("an array of findings");
    let for_part = |item: &str| rows.iter().filter(|r| r["item"] == item).collect::<Vec<_>>();

    let incomplete = for_part("Resistors/RC-INCOMPLETE");
    assert!(incomplete.iter().any(|r| r["kind"] == "footprint" && r["severity"] == "error"), "{rows:?}");
    assert!(incomplete.iter().any(|r| r["kind"] == "datasheet" && r["severity"] == "warning"), "{rows:?}");
    assert!(incomplete.iter().all(|r| r["message"].is_string()));

    let complete = for_part("Resistors/RC0603-10K");
    assert!(complete.iter().all(|r| !matches!(r["kind"].as_str(), Some("footprint" | "datasheet" | "3d-model"))), "{rows:?}");
}

#[test]
fn health_csv_has_a_header_and_one_row_per_finding() {
    let (sb, proj) = setup("csv");
    let p = proj.to_str().unwrap();
    let csv = stdout(&klm(&sb, &["health", "--csv", "--project", p]));
    let json = stdout(&klm(&sb, &["health", "--json", "--project", p]));

    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("severity,kind,item,message"));
    let rows: Vec<&str> = lines.collect();
    let findings = serde_json::from_str::<serde_json::Value>(&json).unwrap().as_array().unwrap().len();
    assert_eq!(rows.len(), findings, "CSV and JSON disagree:\n{csv}");
    assert!(rows.iter().any(|r| r.starts_with("error,footprint,\"Resistors/RC-INCOMPLETE\",")), "{csv}");
    assert!(rows.iter().all(|r| r.split(',').count() >= 4), "{csv}");
}

#[test]
fn health_on_a_complete_project_exits_0() {
    let sb = fresh("clean");
    let proj = project(&sb.join("proj"));
    std::fs::write(
        proj.join("lib/symbols/Resistors.kicad_sym"),
        SYMBOLS.split("  (symbol \"RC-INCOMPLETE\"").next().unwrap().to_string() + ")\n",
    )
    .unwrap();
    let out = klm(&sb, &["health", "--project", proj.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "stdout:\n{}\nstderr:\n{}", stdout(&out), String::from_utf8_lossy(&out.stderr));
    assert!(stdout(&out).ends_with("0 findings\n"), "{}", stdout(&out));
}

#[test]
fn health_without_a_project_says_so() {
    let sb = fresh("noproject");
    let out = klm(&sb, &["health"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no project"), "{}", String::from_utf8_lossy(&out.stderr));
}
