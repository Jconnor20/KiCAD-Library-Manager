//! Runs the real binary against temp settings and a temp KiCad config. Skipped when `kicad-cli`
//! isn't installed, since importing upgrades the files through it.

use klm_core::kicad_env;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};

const SYMBOL_V6: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "TESTPART" (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "TESTPART" (id 1) (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Vendor:SOT-23-ACME" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "MF" "ACME" (id 3) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "TESTPART_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
)"#;

const FOOTPRINT_V5: &str = r#"(module SOT-23-ACME (layer F.Cu) (tedit 5A02FF57)
  (fp_text reference REF** (at 0 -2.5) (layer F.SilkS) (effects (font (size 1 1) (thickness 0.15))))
  (pad 1 smd rect (at -1 0.95) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask))
  (model ${OLDVENDOR}/sot23.wrl (offset (xyz 0 0 0.5)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 90)))
)"#;

fn fresh(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join("klm-cli-e2e").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn make_zip(path: &Path) {
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, body) in [("t.kicad_sym", SYMBOL_V6), ("fp.kicad_mod", FOOTPRINT_V5), ("t.step", "ISO-10303-21;"), ("testpart.pdf", "%PDF")] {
        z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(body.as_bytes()).unwrap();
    }
    z.finish().unwrap();
}

fn klm(settings: &Path, config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klm-cli"))
        .env("KLM_SETTINGS", settings)
        .env("KLM_KICAD_CONFIG", config)
        .env("KLM_CONFIG_DIR", config) // keeps parts.db out of the real config folder
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn import_then_check_and_relink() {
    if !kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success())) {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let dir = fresh("run");
    let (config, lib, settings, zip) = (dir.join("config"), dir.join("lib"), dir.join("settings.json"), dir.join("t.zip"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(&settings, format!(r#"{{"global_root": {:?}}}"#, lib.to_string_lossy().replace('\\', "/"))).unwrap();
    make_zip(&zip);

    let out = klm(&settings, &config, &["import", zip.to_str().unwrap(), "--category", "ICs"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let again = klm(&settings, &config, &["import", zip.to_str().unwrap(), "--category", "ICs"]);
    assert!(!again.status.success(), "duplicate refused without --overwrite");
    assert!(klm(&settings, &config, &["import", zip.to_str().unwrap(), "--category", "ICs", "--overwrite"]).status.success());

    let out = klm(&settings, &config, &["check", "--json"]);
    assert!(out.status.success(), "complete library exits 0: {}", String::from_utf8_lossy(&out.stdout));
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    let p = &rows[0];
    assert_eq!((p["scope"].as_str(), p["category"].as_str(), p["mpn"].as_str()), (Some("global"), Some("ICs"), Some("TESTPART")));
    assert_eq!([&p["footprint"], &p["model"], &p["datasheet"]], ["ok"; 3]);
    assert_eq!(p["in_kicad"], true);

    // KiCad dropping our table entries makes the part incomplete until relink restores them.
    std::fs::write(config.join("sym-lib-table"), "(sym_lib_table\n\t(version 7)\n)\n").unwrap();
    let out = klm(&settings, &config, &["check"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("incomplete"));
    assert!(klm(&settings, &config, &["relink"]).status.success());
    assert!(klm(&settings, &config, &["check"]).status.success());
}

#[test]
fn remove_is_a_dry_run_until_yes_and_keeps_shared_files() {
    let dir = fresh("remove");
    let (config, lib, settings) = (dir.join("config"), dir.join("lib"), dir.join("settings.json"));
    let g = lib.to_string_lossy().replace('\\', "/");
    std::fs::create_dir_all(lib.join("symbols")).unwrap();
    std::fs::create_dir_all(lib.join("footprints/ICs.pretty")).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(&settings, format!(r#"{{"global_root": {g:?}}}"#)).unwrap();
    let part = |n: &str| format!(r#"(symbol "{n}" (property "Footprint" "KLM_ICs:FP" (at 0 0 0)) (property "MPN" "{n}" (at 0 0 0)))"#);
    std::fs::write(lib.join("symbols/ICs.kicad_sym"), format!("(kicad_symbol_lib (version 20251024) {} {})", part("A"), part("B"))).unwrap();
    std::fs::write(lib.join("footprints/ICs.pretty/FP.kicad_mod"), r#"(footprint "FP")"#).unwrap();
    let sym = || std::fs::read_to_string(lib.join("symbols/ICs.kicad_sym")).unwrap();
    let before = sym();

    let out = klm(&settings, &config, &["remove", "A"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && text.contains("Remove symbol A") && text.contains("Keep footprint FP") && text.contains("Dry run"), "{text}");
    assert_eq!(sym(), before, "a dry run writes nothing");

    let out = klm(&settings, &config, &["remove", "A", "--yes"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!sym().contains("\"A\"") && sym().contains("\"B\""));
    assert!(lib.join("footprints/ICs.pretty/FP.kicad_mod").is_file(), "B still uses the footprint");
    assert!(!klm(&settings, &config, &["remove", "A", "--yes"]).status.success(), "already gone");
    assert!(lib.join(".klm-history").is_dir(), "no snapshot");
}

#[test]
fn bad_usage_fails() {
    let dir = fresh("usage");
    let out = klm(&dir.join("none.json"), &dir, &["check"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
}

#[test]
fn dry_run_prints_the_plan_and_writes_nothing() {
    let dir = fresh("dry_run");
    let (config, lib, settings, zip) = (dir.join("config"), dir.join("lib"), dir.join("settings.json"), dir.join("t.zip"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(&settings, format!(r#"{{"global_root": {:?}}}"#, lib.to_string_lossy().replace('\\', "/"))).unwrap();
    make_zip(&zip);
    let second = dir.join("u.zip");
    std::fs::copy(&zip, &second).unwrap();

    // Planning only reads the ZIPs, so this needs no kicad-cli.
    let out = klm(&settings, &config, &["import", zip.to_str().unwrap(), second.to_str().unwrap(), "--category", "ICs", "--dry-run"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.matches("TESTPART
").count(), 2, "one entry per ZIP: {text}");
    assert!(text.contains("add symbol TESTPART in KLM_ICs") && text.contains("add library KLM_ICs to KiCad's tables"), "{text}");
    assert!(!lib.exists() && std::fs::read_dir(&config).unwrap().count() == 0, "a dry run writes nothing");
}

#[test]
fn standardise_dry_run_then_apply_keeps_parts_loadable() {
    let Ok(cli) = kicad_env::kicad_cli() else { return eprintln!("kicad-cli not found; skipping") };
    if !Command::new(&cli).arg("version").output().is_ok_and(|o| o.status.success()) {
        return eprintln!("kicad-cli not working; skipping");
    }
    let dir = fresh("standardise");
    let (config, lib, settings, zip) = (dir.join("config"), dir.join("lib"), dir.join("settings.json"), dir.join("t.zip"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(&settings, format!(r#"{{"global_root": {:?}}}"#, lib.to_string_lossy().replace('\\', "/"))).unwrap();
    make_zip(&zip);
    assert!(klm(&settings, &config, &["import", zip.to_str().unwrap(), "--category", "ICs"]).status.success());
    let (sym, pretty) = (lib.join("symbols/ICs.kicad_sym"), lib.join("footprints/ICs.pretty"));
    let fp = std::fs::read_dir(&pretty).unwrap().flatten().next().unwrap().path();
    let read = || (std::fs::read_to_string(&sym).unwrap(), std::fs::read_to_string(&fp).unwrap());
    let before = read();

    let out = klm(&settings, &config, &["standardise", "--dry-run"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("origin moved") && text.contains("Dry run: nothing was changed."), "{text}");
    assert_eq!(read(), before, "a dry run writes nothing");

    let history = |s: &Path| String::from_utf8_lossy(&klm(s, &config, &["history"]).stdout).matches("standardise (").count();
    assert_eq!(history(&settings), 0);
    assert!(klm(&settings, &config, &["standardise"]).status.success());
    assert_ne!(read(), before);
    assert_eq!(history(&settings), 1, "a snapshot is taken before writing");

    // KiCad still loads both files.
    let svg = dir.join("svg");
    let name = fp.file_stem().unwrap().to_str().unwrap();
    for args in [vec!["sym", "export", "svg", "--symbol", "TESTPART", sym.to_str().unwrap()], vec!["fp", "export", "svg", "--footprint", name, pretty.to_str().unwrap()]] {
        let o = Command::new(&cli).args(&args).arg("-o").arg(&svg).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    }
}

#[test]
fn import_applies_the_rules_from_settings() {
    let dir = fresh("rules");
    let (config, lib, settings, zip) = (dir.join("config"), dir.join("lib"), dir.join("settings.json"), dir.join("t.zip"));
    std::fs::create_dir_all(&config).unwrap();
    let rules = r#"{"categories": [{"keyword": "testpart", "category": "Custom"}], "name_template": "Z_{MPN}", "mpn_case": "Lower"}"#;
    std::fs::write(&settings, format!(r#"{{"global_root": {:?}, "rules": {rules}}}"#, lib.to_string_lossy().replace('\\', "/"))).unwrap();
    make_zip(&zip);

    let out = klm(&settings, &config, &["import", zip.to_str().unwrap(), "--dry-run"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("add symbol Z_testpart in KLM_Custom"), "{text}");
}

#[test]
fn import_writes_rules_and_template_to_disk() {
    if !kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success())) {
        panic!("kicad-cli not found or not runnable; this test must really run");
    }
    let dir = fresh("rules_disk");
    let (config, lib, settings, zip) = (dir.join("config"), dir.join("lib"), dir.join("settings.json"), dir.join("t.zip"));
    std::fs::create_dir_all(&config).unwrap();
    let rules = r#"{"categories": [{"keyword": "testpart", "category": "Custom"}], "name_template": "Z_{MPN}"}"#;
    std::fs::write(&settings, format!(r#"{{"global_root": {:?}, "rules": {rules}}}"#, lib.to_string_lossy().replace('\\', "/"))).unwrap();
    make_zip(&zip);

    let out = klm(&settings, &config, &["import", zip.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(lib.join("symbols/Custom.kicad_sym")).expect("category library written");
    assert!(text.contains("(symbol \"Z_TESTPART\""), "{text}");
    assert!(text.contains("(property \"MPN\" \"TESTPART\""), "MPN must stay untouched: {text}");
}
