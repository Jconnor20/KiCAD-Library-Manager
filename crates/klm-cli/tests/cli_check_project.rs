//! `klm-cli check-project` end to end. The DRC/ERC mapping is tested from saved reports in `klm_core::project_check`;
//! here only what fails before kicad-cli runs, since the fab limits come from W10.0's `fab_data`.

use std::path::Path;
use std::process::{Command, Output};

fn klm(sandbox: &Path, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_klm-cli"));
    c.env_remove("KLM_SETTINGS").env_remove("KLM_KICAD_CONFIG").env_remove("KLM_USE_REAL_KEYS");
    c.env("KLM_CONFIG_DIR", sandbox);
    c.args(args).output().unwrap()
}

#[test]
fn bad_project_folder_fails_clearly() {
    let sb = std::env::temp_dir().join("klm-cli-check-project");
    let _ = std::fs::remove_dir_all(&sb);
    std::fs::create_dir_all(&sb).unwrap();
    let missing = sb.join("no-such-project");
    let out = klm(&sb, &["check-project", missing.to_str().unwrap()]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && err.contains("is not a folder") && err.contains("check-project <dir>"), "{err}");
    let out = klm(&sb, &["check-project", sb.to_str().unwrap(), "--format", "xml"]);
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("--format must be"));
}
