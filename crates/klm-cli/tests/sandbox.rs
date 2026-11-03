//! `KLM_CONFIG_DIR` / `--config` keep every file KLM writes inside the sandbox folder: settings, KiCad's library
//! tables and credentials never reach the real `%APPDATA%`.

use std::path::Path;
use std::process::{Command, Output};

fn klm(sandbox: &Path, via_flag: bool, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_klm-cli"));
    // Remove the overrides a developer may have set, so only the sandbox decides.
    c.env_remove("KLM_SETTINGS").env_remove("KLM_KICAD_CONFIG").env_remove("KLM_USE_REAL_KEYS");
    if via_flag {
        c.arg("--config").arg(sandbox);
    } else {
        c.env("KLM_CONFIG_DIR", sandbox);
    }
    c.args(args).output().unwrap()
}

fn fresh(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join("klm-cli-sandbox").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn history_and_check_read_the_sandbox_and_create_kicad_tables_there() {
    for via_flag in [false, true] {
        let sb = fresh(if via_flag { "flag" } else { "env" });
        let lib = sb.join("lib");
        std::fs::write(sb.join("settings.json"), format!(r#"{{"global_root": {:?}}}"#, lib.to_string_lossy().replace('\\', "/"))).unwrap();

        let out = klm(&sb, via_flag, &["check", "--json"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let out = klm(&sb, via_flag, &["history"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

        // KiCad's config dir lookup was redirected, with empty tables made on demand.
        let kicad = std::fs::read_dir(sb.join("kicad")).unwrap().flatten().next().expect("a kicad/<version> folder").path();
        assert!(kicad.join("sym-lib-table").is_file() && kicad.join("fp-lib-table").is_file());
    }
}

#[test]
fn config_flag_needs_a_folder() {
    let out = Command::new(env!("CARGO_BIN_EXE_klm-cli")).args(["check", "--config"]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn import_writes_only_inside_the_sandbox() {
    use klm_core::kicad_env;
    let sb = fresh("import");
    // Even `kicad-cli version` rewrites kicad_common.json, so point it at the sandbox as well.
    if !kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).env("KICAD_CONFIG_HOME", sb.join("kicad")).arg("version").output().is_ok_and(|o| o.status.success())) {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    std::fs::write(sb.join("settings.json"), format!(r#"{{"global_root": {:?}}}"#, sb.join("lib").to_string_lossy().replace('\\', "/"))).unwrap();
    let zip = sb.join("t.zip");
    {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(&zip).unwrap());
        z.start_file("t.kicad_sym", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)\n (symbol \"SBX\" (in_bom yes) (on_board yes)\n  (property \"Reference\" \"U\" (id 0) (at 0 0 0) (effects (font (size 1.27 1.27))))\n  (property \"Value\" \"SBX\" (id 1) (at 0 0 0) (effects (font (size 1.27 1.27))))\n  (symbol \"SBX_1_1\" (pin passive line (at 0 0 0) (length 2.54) (name \"A\" (effects (font (size 1.27 1.27)))) (number \"1\" (effects (font (size 1.27 1.27))))))))").unwrap();
        z.finish().unwrap();
    }
    let out = klm(&sb, true, &["import", zip.to_str().unwrap(), "--category", "ICs"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(sb.join("lib").is_dir());
    let table = std::fs::read_dir(sb.join("kicad")).unwrap().flatten().next().unwrap().path().join("sym-lib-table");
    assert!(std::fs::read_to_string(table).unwrap().contains("ICs"), "global table written in the sandbox");
}
