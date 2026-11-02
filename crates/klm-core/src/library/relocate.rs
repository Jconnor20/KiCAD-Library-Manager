//! Moving a global library, and fixing paths written by older KLM versions.

use super::{LibraryRoot, Scope, has_ext};
use crate::kicad_env::{self, write_atomic};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
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
        crate::snapshot::ensure(self, "rewrite library paths")?;
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
            match std::fs::rename(&self.dir, new_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => move_across(&self.dir, new_dir, copy_tree)?,
                Err(e) => return Err(e).with_context(|| format!("could not move the library to {}", new_dir.display())),
            }
        }
        moved.rewrite_prefix(&self.var())?;
        Ok(moved)
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        // The snapshots would be copied whole (hard links become copies) and are stale after a move anyway.
        if e.file_name() == crate::snapshot::DIR {
            continue;
        }
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}

/// Every file under `dir` with its size, keyed by path relative to `dir`.
fn file_sizes(dir: &Path) -> std::io::Result<BTreeMap<PathBuf, u64>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, u64>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            if e.file_name() == crate::snapshot::DIR {
                continue;
            }
            if e.file_type()?.is_dir() {
                walk(root, &e.path(), out)?;
            } else {
                out.insert(e.path().strip_prefix(root).unwrap_or(&e.path()).to_path_buf(), e.metadata()?.len());
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Moves `from` to `to` by copying (rename can't cross drives), deleting the original only once the
/// copy has the same files and sizes. On any failure the original stays and the partial copy goes.
/// `to` must not hold anything worth keeping: the caller has checked it is new or empty.
fn move_across(from: &Path, to: &Path, copy: impl Fn(&Path, &Path) -> std::io::Result<()>) -> Result<()> {
    let copied = copy(from, to).context("copying the library").and_then(|()| {
        if file_sizes(from)? == file_sizes(to)? { Ok(()) } else { bail!("the copy doesn't match the original") }
    });
    if let Err(e) = copied {
        let _ = std::fs::remove_dir_all(to);
        return Err(e.context(format!("could not copy the library to {}; the original is untouched", to.display())));
    }
    std::fs::remove_dir_all(from).with_context(|| {
        format!("the library was copied to {}, but the original at {} could not be fully deleted; delete it yourself", to.display(), from.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> (PathBuf, PathBuf) {
        let dir = crate::test_dir(name);
        let from = dir.join("old");
        std::fs::create_dir_all(from.join("symbols")).unwrap();
        std::fs::create_dir_all(from.join("footprints/R.pretty")).unwrap();
        std::fs::write(from.join("symbols/R.kicad_sym"), "sym").unwrap();
        std::fs::write(from.join("footprints/R.pretty/R.kicad_mod"), "footprint").unwrap();
        (from, dir.join("new"))
    }

    #[test]
    fn move_across_copies_verifies_then_deletes_original() {
        let (from, to) = sample("move_across");
        move_across(&from, &to, copy_tree).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(to.join("footprints/R.pretty/R.kicad_mod")).unwrap(), "footprint");
        assert_eq!(std::fs::read_to_string(to.join("symbols/R.kicad_sym")).unwrap(), "sym");
    }

    #[test]
    fn failed_copy_leaves_original_and_removes_partial() {
        let (from, to) = sample("move_across_fail");
        let err = move_across(&from, &to, |a, b| {
            copy_tree(a, b)?;
            Err(std::io::Error::other("disk full"))
        });
        assert!(err.is_err());
        assert!(from.join("symbols/R.kicad_sym").is_file() && from.join("footprints/R.pretty/R.kicad_mod").is_file());
        assert!(!to.exists());
    }

    #[test]
    fn copy_that_does_not_match_is_not_trusted() {
        let (from, to) = sample("move_across_short");
        let err = move_across(&from, &to, |a, b| {
            copy_tree(a, b)?;
            std::fs::remove_file(b.join("symbols/R.kicad_sym"))
        });
        assert!(format!("{:#}", err.unwrap_err()).contains("doesn't match"));
        assert!(from.join("symbols/R.kicad_sym").is_file());
        assert!(!to.exists());
    }
}
