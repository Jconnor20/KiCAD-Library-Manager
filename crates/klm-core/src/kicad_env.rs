//! Finding the installed KiCad and editing its config: library tables and `kicad-cli` format upgrades.

use crate::sexpr;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Highest `x.y` sub-directory of `dir` (KiCad keeps one per major version).
fn newest_version_dir(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let (maj, min) = name.split_once('.')?;
            Some(((maj.parse::<u32>().ok()?, min.parse::<u32>().ok()?), e.path()))
        })
        .max_by_key(|(v, _)| *v)
        .map(|(_, p)| p)
}

/// KiCad's per-user config dir for the newest installed version, e.g. `%APPDATA%\kicad\10.0`.
pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::preference_dir().context("no user config directory")?.join("kicad");
    newest_version_dir(&base).with_context(|| format!("no KiCad config found in {}", base.display()))
}

pub fn kicad_cli() -> Result<PathBuf> {
    if cfg!(windows) {
        let pf = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or("C:/Program Files".into());
        if let Some(v) = newest_version_dir(&pf.join("KiCad")) {
            let p = v.join("bin/kicad-cli.exe");
            if p.exists() {
                return Ok(p);
            }
        }
    }
    let mac = PathBuf::from("/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli");
    if mac.exists() {
        return Ok(mac);
    }
    Ok("kicad-cli".into()) // shortcut: relies on PATH elsewhere; add a Settings override if users hit it.
}

fn quiet(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash from the GUI
    }
    cmd
}

/// True while any KiCad window is open. KiCad keeps its library tables and path variables in
/// memory and writes them back over any edits made meanwhile, so those edits must wait.
pub fn kicad_running() -> bool {
    let (prog, args): (&str, &[&str]) = if cfg!(windows) { ("tasklist", &["/NH", "/FO", "CSV"]) } else { ("ps", &["-A", "-o", "comm="]) };
    let Ok(out) = quiet(Path::new(prog)).args(args).output() else { return false };
    String::from_utf8_lossy(&out.stdout).lines().any(|l| {
        let name = l.split(',').next().unwrap_or("").trim_matches('"').to_lowercase();
        let name = name.rsplit('/').next().unwrap_or("").trim_end_matches(".exe");
        matches!(name, "kicad" | "eeschema" | "pcbnew")
    })
}

fn run_cli(args: &[&std::ffi::OsStr]) -> Result<()> {
    let cli = kicad_cli()?;
    let out = quiet(&cli).args(args).output().with_context(|| format!("could not run {}", cli.display()))?;
    if !out.status.success() {
        bail!("kicad-cli failed: {}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    }
    Ok(())
}

/// Rewrites any KiCad symbol library (legacy `.lib` included) in the installed KiCad's format.
pub fn upgrade_symbols(input: &Path, output: &Path) -> Result<()> {
    run_cli(&["sym".as_ref(), "upgrade".as_ref(), "--force".as_ref(), "-o".as_ref(), output.as_os_str(), input.as_os_str()])
}

/// Rewrites a `.pretty` folder of footprints in the installed KiCad's format.
pub fn upgrade_footprints(input_dir: &Path, output_dir: &Path) -> Result<()> {
    run_cli(&["fp".as_ref(), "upgrade".as_ref(), "--force".as_ref(), "-o".as_ref(), output_dir.as_os_str(), input_dir.as_os_str()])
}

/// Write via a temp file so a crash never leaves a half-written library.
pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("klm-tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

pub fn backup(path: &Path) -> Result<()> {
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        std::fs::copy(path, &bak).with_context(|| format!("backing up {}", path.display()))?;
    }
    Ok(())
}

/// Whether a `sym-lib-table` / `fp-lib-table` already lists `nickname`.
pub fn is_registered(table: &Path, nickname: &str) -> bool {
    std::fs::read_to_string(table)
        .ok()
        .and_then(|t| sexpr::parse(&t).ok())
        .is_some_and(|t| t.children("lib").any(|l| l.child("name").and_then(|n| n.arg(0)) == Some(nickname)))
}

/// Adds a library to a `sym-lib-table` / `fp-lib-table`. Returns false if the nickname is already there.
/// The new line is spliced in as text so the user's existing table is left byte-for-byte as it was.
pub fn register_lib(table: &Path, nickname: &str, uri: &str) -> Result<bool> {
    let kind = if table.file_name().is_some_and(|n| n == "fp-lib-table") { "fp_lib_table" } else { "sym_lib_table" };
    let text = if table.exists() {
        std::fs::read_to_string(table)?
    } else {
        format!("({kind}\n\t(version 7)\n)\n")
    };
    sexpr::parse(&text).with_context(|| format!("reading {}", table.display()))?;
    if is_registered(table, nickname) {
        return Ok(false);
    }
    let entry = sexpr::Sexpr::list(
        "lib",
        vec![
            sexpr::Sexpr::list("name", vec![sexpr::Sexpr::str(nickname)]),
            sexpr::Sexpr::list("type", vec![sexpr::Sexpr::str("KiCad")]),
            sexpr::Sexpr::list("uri", vec![sexpr::Sexpr::str(uri)]),
            sexpr::Sexpr::list("options", vec![sexpr::Sexpr::str("")]),
            sexpr::Sexpr::list("descr", vec![sexpr::Sexpr::str("Managed by KiCad Library Manager")]),
        ],
    );
    let close = text.rfind(')').context("library table has no closing ')'")?;
    let line = sexpr::to_string(&entry).replace("\n\t", " ").replace('\n', "");
    let new_text = format!("{}\t{}\n{}", text[..close].trim_end_matches([' ', '\t']), line, &text[close..]);
    backup(table)?;
    write_atomic(table, &new_text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_lib_appends_once_and_backs_up() {
        let dir = crate::test_dir("register");
        let table = dir.join("sym-lib-table");
        let original = "(sym_lib_table\n\t(version 7)\n\t(lib (name \"Mine\") (type \"KiCad\") (uri \"x\") (options \"\") (descr \"\"))\n)\n";
        std::fs::write(&table, original).unwrap();

        assert!(register_lib(&table, "KLM_Resistors", "${KLM_LIB}/symbols/Resistors.kicad_sym").unwrap());
        assert!(!register_lib(&table, "KLM_Resistors", "anything").unwrap());

        let text = std::fs::read_to_string(&table).unwrap();
        assert!(text.starts_with(original.trim_end_matches(")\n")), "existing lines untouched");
        let t = sexpr::parse(&text).unwrap();
        assert_eq!(t.children("lib").count(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("sym-lib-table.bak")).unwrap(), original);
    }

    #[test]
    fn register_lib_creates_missing_table() {
        let dir = crate::test_dir("register_new");
        let table = dir.join("fp-lib-table");
        assert!(register_lib(&table, "PRJ_ICs", "${KIPRJMOD}/lib/footprints/ICs.pretty").unwrap());
        let t = sexpr::parse(&std::fs::read_to_string(&table).unwrap()).unwrap();
        assert_eq!(t.head(), Some("fp_lib_table"));
        assert_eq!(t.child("lib").unwrap().child("uri").unwrap().arg(0), Some("${KIPRJMOD}/lib/footprints/ICs.pretty"));
    }

}
