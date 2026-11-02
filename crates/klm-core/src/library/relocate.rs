//! Moving a global library, and fixing paths written by older KLM versions.

use super::{LibraryRoot, Scope, has_ext};
use crate::kicad_env::{self, write_atomic};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Prefix older KLM versions wrote for global paths; KiCad lost the variable whenever it saved its settings.
const OLD_GLOBAL_VAR: &str = "${KLM_LIB}";

impl LibraryRoot {
    /// KiCad files this root writes paths into: its libraries plus KiCad's tables.
    fn files_with_paths(&self) -> Vec<PathBuf> {
        let mut libs: Vec<PathBuf> = std::fs::read_dir(self.dir.join("symbols")).into_iter().flatten().flatten().map(|e| e.path()).collect();
        for pretty in std::fs::read_dir(self.dir.join("footprints")).into_iter().flatten().flatten() {
            libs.extend(std::fs::read_dir(pretty.path()).into_iter().flatten().flatten().map(|e| e.path()));
        }
        libs.retain(|p| has_ext(p, &["kicad_sym", "kicad_mod"]));
        let mut files = vec![self.table_dir.join("sym-lib-table"), self.table_dir.join("fp-lib-table")];
        files.extend(libs);
        files
    }

    /// Whether files still use the old `${KLM_LIB}` prefix (see [`Self::fix_old_paths`]).
    pub fn has_old_paths(&self) -> bool {
        let old = format!("{OLD_GLOBAL_VAR}/");
        self.scope == Scope::Global && self.files_with_paths().iter().any(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains(&old)))
    }

    /// Rewrites `${KLM_LIB}` to the library's real path. Returns how many files changed.
    pub fn fix_old_paths(&self) -> Result<usize> {
        if self.scope != Scope::Global {
            return Ok(0);
        }
        self.rewrite_prefix(OLD_GLOBAL_VAR)
    }

    /// Replaces path prefix `old` with this root's prefix in its libraries and KiCad's tables.
    /// Returns how many files changed. KiCad must be closed (it saves over its tables).
    fn rewrite_prefix(&self, old: &str) -> Result<usize> {
        // The trailing '/' stops ".../Lib" also matching ".../Lib2".
        let (old, new) = (format!("{}/", old.trim_end_matches('/')), format!("{}/", self.var()));
        let mut changed = 0;
        for f in self.files_with_paths() {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            if text.contains(&old) {
                // Back up KiCad's tables (no extension); a .bak inside a library folder would be clutter.
                if f.extension().is_none() {
                    kicad_env::backup(&f)?;
                }
                write_atomic(&f, &text.replace(&old, &new))?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// Points a global library at `new_dir` and rewrites every path to match. If `new_dir`
    /// already holds the library (the user moved it by hand) it's used as-is; otherwise the
    /// library is moved there. Returns the new root.
    pub fn relocate(&self, new_dir: &Path) -> Result<LibraryRoot> {
        if self.scope != Scope::Global {
            bail!("only the global library can be moved");
        }
        let moved = Self { dir: new_dir.to_path_buf(), ..self.clone() };
        if !new_dir.join("symbols").is_dir() {
            if new_dir.exists() && std::fs::read_dir(new_dir)?.next().is_some() {
                bail!("{} is not empty and has no KLM library in it", new_dir.display());
            }
            if !self.dir.is_dir() {
                bail!("the library isn't at {} any more; choose the folder you moved it to", self.dir.display());
            }
            if let Some(parent) = new_dir.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // shortcut: rename only works on the same drive; across drives the user moves it by hand and picks the new folder.
            std::fs::rename(&self.dir, new_dir).with_context(|| {
                format!("could not move the library to {} (to move it to another drive, move the folder yourself, then choose its new location)", new_dir.display())
            })?;
        }
        moved.rewrite_prefix(&self.var())?;
        Ok(moved)
    }
}
