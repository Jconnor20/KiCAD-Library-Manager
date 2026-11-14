//! A KLM library on disk (global or per-project) and the import of one part into it.
//!
//! This file holds the shared pieces (the root, naming, KiCad table linking); each feature has its
//! own file: `import`, `status`, `attach`, `copy`, `edit`, `relocate`. `fields` has the symbol/footprint helpers.

mod attach;
mod copy;
mod edit;
mod fields;
mod import;
mod relocate;
mod remove;
mod status;

pub use attach::{ATTACH_EXTENSIONS, AttachKind};
pub use edit::{EDIT_FIELDS, Edited, PartEdit};
pub use fields::{property, set_property};
pub(crate) use fields::rename_symbol;
pub use remove::{Piece, RemovePlan};
pub use status::{Check, PartStatus};

use crate::kicad_env;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const SUBDIRS: &[&str] = &["symbols", "footprints", "3dmodels", "datasheets", "simulation"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    Global,
    Project,
}

#[derive(Clone, Debug)]
pub struct LibraryRoot {
    pub dir: PathBuf,
    pub scope: Scope,
    /// Where this root's sym-lib-table / fp-lib-table live.
    pub table_dir: PathBuf,
}

/// Characters KiCad or Windows reject in library ids and file names.
pub fn sanitize(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| if c.is_whitespace() || r#":/\{}"'<>|?*"#.contains(c) { '_' } else { c })
        .collect()
}

impl LibraryRoot {
    pub fn global(dir: PathBuf, kicad_config_dir: PathBuf) -> Self {
        Self { dir, scope: Scope::Global, table_dir: kicad_config_dir }
    }

    /// `project_dir` is the folder holding the `.kicad_pro`.
    pub fn project(project_dir: &Path) -> Self {
        Self { dir: project_dir.join("lib"), scope: Scope::Project, table_dir: project_dir.to_path_buf() }
    }

    /// Prefix for paths inside KiCad files. Global: the real folder, so nothing depends on a KiCad
    /// path variable. Project: KiCad's built-in `${KIPRJMOD}`, so the project folder can move.
    fn var(&self) -> String {
        match self.scope {
            Scope::Global => self.dir.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string(),
            Scope::Project => "${KIPRJMOD}/lib".into(),
        }
    }

    pub fn nickname(&self, category: &str) -> String {
        let prefix = if self.scope == Scope::Global { "KLM" } else { "PRJ" };
        format!("{prefix}_{}", sanitize(category))
    }

    pub fn ensure_layout(&self) -> Result<()> {
        for d in SUBDIRS {
            std::fs::create_dir_all(self.dir.join(d)).with_context(|| format!("creating {}", self.dir.join(d).display()))?;
        }
        Ok(())
    }

    pub(crate) fn symbol_lib(&self, category: &str) -> PathBuf {
        self.dir.join("symbols").join(format!("{}.kicad_sym", sanitize(category)))
    }

    /// Categories that already have a symbol library here.
    pub fn categories(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.dir.join("symbols")) else { return vec![] };
        rd.flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".kicad_sym").map(String::from))
            .collect()
    }

    pub fn has_part(&self, category: &str, mpn: &str) -> bool {
        let name = sanitize(mpn);
        std::fs::read_to_string(self.symbol_lib(category))
            .ok()
            .and_then(|t| sexpr::parse(&t).ok())
            .is_some_and(|lib| lib.children("symbol").any(|s| s.arg(0) == Some(&name)))
    }

    /// Whether KiCad's tables are missing this category's symbol or footprint library.
    pub fn needs_link(&self, category: &str) -> bool {
        let nick = self.nickname(category);
        ["sym-lib-table", "fp-lib-table"].iter().any(|t| {
            let table = self.table_dir.join(t);
            !kicad_env::is_registered(&table, &nick) && !kicad_env::is_queued(&table, &nick)
        })
    }

    /// Existing categories that KiCad's tables don't (or no longer) list.
    pub fn unlinked(&self) -> Vec<String> {
        self.categories().into_iter().filter(|c| self.needs_link(c)).collect()
    }

    /// Adds this category's libraries to KiCad's tables. Returns true if anything was added.
    pub fn link(&self, category: &str) -> Result<bool> {
        let (cat, nick, var) = (sanitize(category), self.nickname(category), self.var());
        if self.needs_link(category) {
            crate::snapshot::ensure(self, &format!("link {nick} in KiCad"))?;
        }
        let new_sym = kicad_env::register_or_queue(&self.table_dir.join("sym-lib-table"), &nick, &format!("{var}/symbols/{cat}.kicad_sym"))?;
        let new_fp = kicad_env::register_or_queue(&self.table_dir.join("fp-lib-table"), &nick, &format!("{var}/footprints/{cat}.pretty"))?;
        Ok(new_sym || new_fp)
    }

    fn load_symbol(&self, cat: &str, name: &str) -> Result<(Sexpr, usize)> {
        let text = std::fs::read_to_string(self.symbol_lib(cat)).with_context(|| format!("no library {}", self.nickname(cat)))?;
        let lib = sexpr::parse(&text)?;
        let idx = lib.items().iter().position(|c| c.head() == Some("symbol") && c.arg(0) == Some(name));
        Ok((lib, idx.with_context(|| format!("{name} is not in {}", self.nickname(cat)))?))
    }

    fn footprint_file(&self, cat: &str, name: &str) -> PathBuf {
        self.dir.join("footprints").join(format!("{cat}.pretty")).join(format!("{name}.kicad_mod"))
    }
}

/// `fs::copy` that replaces `dest`, creates the folder and does nothing when `src` already is `dest` (copying a file onto itself empties it).
pub(super) fn copy_file(src: &Path, dest: &Path) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if dest.exists() && std::fs::canonicalize(src)? == std::fs::canonicalize(dest)? {
        return Ok(());
    }
    // Through a temp file and a rename, never into `dest` itself: a snapshot may hold `dest` as a hard link.
    let tmp = dest.with_extension("klm-tmp");
    std::fs::copy(src, &tmp).with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
    std::fs::rename(&tmp, dest).with_context(|| format!("replacing {}", dest.display()))
}

pub(super) fn has_ext(p: &Path, exts: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_bad_chars() {
        assert_eq!(sanitize(" LM1085IS-12/NOPB "), "LM1085IS-12_NOPB");
        assert_eq!(sanitize("A:B C"), "A_B_C");
    }
}
