//! `klm-cli cleanup` against a temp KiCad config: report-only by default, and `--apply` changes only what was chosen.

use std::process::{Command, Output};

fn klm(settings: &std::path::Path, config: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klm-cli")).env("KLM_SETTINGS", settings).env("KLM_KICAD_CONFIG", config).args(args).output().unwrap()
}

#[test]
fn report_then_preview_then_apply() {
    let d = std::env::temp_dir().join("klm-cli-e2e").join("cleanup");
    let _ = std::fs::remove_dir_all(&d);
    let cfg = d.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let settings = d.join("settings.json");
    std::fs::write(&settings, "{}").unwrap();
    let lib = d.join("ok.kicad_sym");
    std::fs::write(&lib, "(kicad_symbol_lib (symbol \"A\"))").unwrap();
    let table = cfg.join("sym-lib-table");
    let text = format!(
        "(sym_lib_table\n\t(version 7)\n\t(lib (name \"Ok\")(type \"KiCad\")(uri \"{}\")(options \"\")(descr \"\"))\n\t(lib (name \"Gone\")(type \"KiCad\")(uri \"{}/gone.kicad_sym\")(options \"\")(descr \"\"))\n)\n",
        lib.to_string_lossy().replace('\\', "/"),
        d.to_string_lossy().replace('\\', "/")
    );
    std::fs::write(&table, &text).unwrap();

    let out = klm(&settings, &cfg, &["cleanup"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && stdout.contains("broken") && stdout.contains("sym:Gone"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&table).unwrap(), text, "a report changes nothing");

    let out = klm(&settings, &cfg, &["cleanup", "--remove", "sym:Gone"]);
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("Preview only"));
    assert_eq!(std::fs::read_to_string(&table).unwrap(), text, "without --apply nothing changes");

    let out = klm(&settings, &cfg, &["cleanup", "--remove", "sym:Gone", "--apply"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let after = std::fs::read_to_string(&table).unwrap();
    assert_eq!(after, text.lines().filter(|l| !l.contains("Gone")).collect::<Vec<_>>().join("\n") + "\n");
    assert!(std::fs::read_dir(&cfg).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".klm-cleanup-")));
    assert!(lib.exists());

    assert!(!klm(&settings, &cfg, &["cleanup", "--remove", "sym:Nope"]).status.success());
}
