//! `check --deep` reports the part checks. Needs no KiCad: the library is written by hand.

use std::process::Command;

const SYMBOL: &str = r#"(kicad_symbol_lib (version 20251024) (generator "t")
  (symbol "P1"
    (property "Reference" "U") (property "Value" "P1") (property "Footprint" "PRJ_Test:FP")
    (property "MPN" "P1") (property "Manufacturer" "Acme")
    (symbol "P1_1_1"
      (pin passive line (at 0 0 0) (length 2) (name "A") (number "1"))
      (pin passive line (at 0 2 0) (length 2) (name "B") (number "2")))))"#;

// Pad 2 is missing.
const FOOTPRINT: &str = r#"(footprint "FP" (layer "F.Cu") (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu")))"#;

#[test]
fn deep_adds_issues_and_their_exit_code() {
    let dir = std::env::temp_dir().join("klm-cli-e2e").join("deep");
    let _ = std::fs::remove_dir_all(&dir);
    let lib = dir.join("lib");
    std::fs::create_dir_all(lib.join("symbols")).unwrap();
    std::fs::create_dir_all(lib.join("footprints/Test.pretty")).unwrap();
    std::fs::write(lib.join("symbols/Test.kicad_sym"), SYMBOL).unwrap();
    std::fs::write(lib.join("footprints/Test.pretty/FP.kicad_mod"), FOOTPRINT).unwrap();
    let settings = dir.join("settings.json");
    std::fs::write(&settings, "{}").unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_klm-cli"))
            .env("KLM_SETTINGS", &settings)
            .env("KLM_KICAD_CONFIG", dir.join("config"))
            .args(["check", "--json", "--project", dir.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    };
    let plain: serde_json::Value = serde_json::from_slice(&run(&[]).stdout).unwrap();
    assert!(plain[0].get("issues").is_none());

    let out = run(&["--deep"]);
    assert_eq!(out.status.code(), Some(1));
    let deep: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let issues = deep[0]["issues"].as_array().unwrap();
    assert!(issues.iter().any(|i| i["severity"] == "error" && i["message"].as_str().unwrap().contains("pin(s) 2")), "{issues:?}");
}
