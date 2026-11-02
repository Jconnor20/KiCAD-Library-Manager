//! Copying a part from one library into another.

use super::fields::{merge_symbol, same_geometry};
use super::{LibraryRoot, copy_file, property, sanitize, set_property};
use crate::kicad_env::write_atomic;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

impl LibraryRoot {
    /// Where the file `value` (a path inside this root) lands in `target`, as (source, destination, new value).
    /// None for URLs, other libraries and files that aren't there.
    fn carry(&self, value: &str, target: &LibraryRoot) -> Option<(PathBuf, PathBuf, String)> {
        let rel = value.strip_prefix(&format!("{}/", self.var()))?;
        let src = self.dir.join(rel);
        src.is_file().then(|| (src, target.dir.join(rel), format!("{}/{rel}", target.var())))
    }

    /// Copies one part (symbol, footprint, 3D model, datasheet, sim model) into `target` and links the
    /// target's category libraries in KiCad. Returns true if that library was new to KiCad's tables.
    /// Refuses to replace a part, or a different footprint of the same name, unless `overwrite`.
    pub fn copy_part_to(&self, category: &str, symbol_name: &str, target: &LibraryRoot, overwrite: bool) -> Result<bool> {
        let cat = sanitize(category);
        let (lib, idx) = self.load_symbol(&cat, symbol_name)?;
        let mut symbol = lib.items()[idx].clone();
        if !overwrite && target.has_part(&cat, symbol_name) {
            bail!("{symbol_name} is already in {}", target.nickname(&cat));
        }
        let mut files = vec![];
        let mut carry = |value: &str| {
            let (src, dest, new) = self.carry(value, target)?;
            files.push((src, dest));
            Some(new)
        };

        let mut footprint = None;
        let field = property(&symbol, "Footprint").unwrap_or("").to_string();
        if let Some(fp_name) = field.strip_prefix(&format!("{}:", self.nickname(&cat))) {
            let src = self.footprint_file(&cat, fp_name);
            let mut fp = sexpr::parse(&std::fs::read_to_string(&src).with_context(|| format!("reading {}", src.display()))?)?;
            for model in fp.items_mut().iter_mut().filter(|c| c.head() == Some("model")) {
                if let Some(new) = model.arg(0).and_then(&mut carry) {
                    model.set_arg(0, Sexpr::str(&new));
                }
            }
            let (dest, text) = (target.footprint_file(&cat, fp_name), sexpr::to_string(&fp));
            if !overwrite && dest.exists() && !same_geometry(&std::fs::read_to_string(&dest)?, &text) {
                bail!("{} already has a different footprint called {fp_name}", target.nickname(&cat));
            }
            set_property(&mut symbol, "Footprint", &format!("{}:{fp_name}", target.nickname(&cat)));
            footprint = Some((dest, text));
        }
        for key in ["Datasheet", "Sim.Library"] {
            if let Some(new) = property(&symbol, key).and_then(&mut carry) {
                set_property(&mut symbol, key, &new);
            }
        }

        target.ensure_layout()?;
        for (src, dest) in files {
            copy_file(&src, &dest)?;
        }
        if let Some((dest, text)) = footprint {
            std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;
            write_atomic(&dest, &text)?;
        }
        merge_symbol(&target.symbol_lib(&cat), &lib, symbol)?;
        target.link(&cat)
    }
}
