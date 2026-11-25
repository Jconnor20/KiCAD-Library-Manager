//! `klm-cli --help` and `--version`: they print and exit 0 without touching any settings.

use std::process::Command;

fn klm(args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_klm-cli")).args(args).output().unwrap();
    (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned())
}

#[test]
fn help_prints_usage_and_exits_zero() {
    for args in [&["--help"][..], &["-h"], &["help"], &["check", "--help"]] {
        let (ok, out) = klm(args);
        assert!(ok && out.contains("klm-cli bom-import"), "{args:?}: {out}");
    }
}

#[test]
fn version_prints_the_package_version() {
    for flag in ["--version", "-V"] {
        let (ok, out) = klm(&[flag]);
        assert!(ok);
        assert_eq!(out.trim(), concat!("klm-cli ", env!("CARGO_PKG_VERSION")));
    }
}
