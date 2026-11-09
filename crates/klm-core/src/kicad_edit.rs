//! Edit a part in KiCad's own symbol or footprint editor and notice when the file is saved.

use crate::compliance::{footprint_name, footprint_path, symbol_lib};
use crate::kicad_env;
use crate::library::LibraryRoot;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Editor {
    Symbol,
    Footprint,
}

/// The file the editor opens. KiCad can't open one symbol of a library from the command line, so the symbol
/// editor gets the category's whole `.kicad_sym` (the user picks the part in its tree); the footprint is a file.
pub fn target(root: &LibraryRoot, editor: Editor, category: &str, name: &str) -> Result<PathBuf> {
    let file = match editor {
        Editor::Symbol => symbol_lib(root, category),
        Editor::Footprint => footprint_path(root, category, &footprint_name(root, category, name).with_context(|| format!("{name} has no footprint"))?),
    };
    if !file.is_file() {
        bail!("{} does not exist", file.display());
    }
    Ok(file)
}

/// The KiCad program that opens `file` in `editor`, found next to `kicad-cli`.
/// shortcut: the standalone editors are opened by passing the file to eeschema/pcbnew, which is unverified
/// against a real KiCad; upgrade to the `kicad` project manager's own launch if a version ignores the file.
pub fn command(editor: Editor, file: &Path) -> Result<Command> {
    let exe = match editor {
        Editor::Symbol => "eeschema",
        Editor::Footprint => "pcbnew",
    };
    let mut c = Command::new(kicad_env::kicad_cli()?.with_file_name(format!("{exe}{}", std::env::consts::EXE_SUFFIX)));
    c.arg(file);
    Ok(c)
}

pub fn launch(editor: Editor, file: &Path) -> Result<()> {
    command(editor, file)?.spawn().context("could not start KiCad (set its path under Settings)")?;
    Ok(())
}

/// Remembers a file's modification time; `changed` is true once per save.
pub struct FileWatch {
    pub path: PathBuf,
    seen: Option<SystemTime>,
}

impl FileWatch {
    pub fn new(path: PathBuf) -> Self {
        let seen = mtime(&path);
        Self { path, seen }
    }

    pub fn changed(&mut self) -> bool {
        let now = mtime(&self.path);
        std::mem::replace(&mut self.seen, now) != now
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_passes_the_file_to_the_right_editor() {
        let f = Path::new("a.kicad_sym");
        let c = command(Editor::Symbol, f).unwrap();
        assert!(c.get_program().to_string_lossy().contains("eeschema"));
        assert_eq!(c.get_args().collect::<Vec<_>>(), [f.as_os_str()]);
        assert!(command(Editor::Footprint, f).unwrap().get_program().to_string_lossy().contains("pcbnew"));
    }

    #[test]
    fn a_save_is_reported_once() {
        let dir = crate::test_dir("kicad_edit");
        let p = dir.join("x.kicad_mod");
        std::fs::write(&p, "a").unwrap();
        let mut w = FileWatch::new(p.clone());
        assert!(!w.changed());
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(&p, "b").unwrap();
        assert!(w.changed());
        assert!(!w.changed());
    }

    #[test]
    fn target_is_the_category_library_or_the_footprint_file() {
        let dir = crate::test_dir("kicad_edit_target");
        let root = LibraryRoot::project(&dir);
        std::fs::create_dir_all(dir.join("lib/symbols")).unwrap();
        std::fs::create_dir_all(dir.join("lib/footprints/ICs.pretty")).unwrap();
        std::fs::write(dir.join("lib/symbols/ICs.kicad_sym"), r#"(kicad_symbol_lib (symbol "U1" (property "Footprint" "PRJ_ICs:FP1" (at 0 0 0))))"#).unwrap();
        std::fs::write(dir.join("lib/footprints/ICs.pretty/FP1.kicad_mod"), "(footprint \"FP1\")").unwrap();
        assert_eq!(target(&root, Editor::Symbol, "ICs", "U1").unwrap(), dir.join("lib/symbols/ICs.kicad_sym"));
        assert_eq!(target(&root, Editor::Footprint, "ICs", "U1").unwrap(), dir.join("lib/footprints/ICs.pretty/FP1.kicad_mod"));
        assert!(target(&root, Editor::Footprint, "ICs", "nope").is_err());
    }
}
