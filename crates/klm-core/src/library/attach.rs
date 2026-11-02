//! Adding a datasheet, SPICE model or 3D model to a part already in a library.

use super::fields::{datasheet_name, set_footprint, spice_model_name};
use super::{LibraryRoot, copy_file, property, sanitize, set_property};
use crate::kicad_env::write_atomic;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::path::Path;

/// What [`LibraryRoot::attach_file`] adds to a part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AttachKind {
    Datasheet,
    Sim,
    Model,
}

/// Extensions [`AttachKind::of`] recognises (for file pickers).
pub const ATTACH_EXTENSIONS: &[&str] = &["pdf", "lib", "cir", "sub", "sp", "spi", "ibs", "step", "stp", "wrl"];

impl AttachKind {
    pub fn of(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_lowercase().as_str() {
            "pdf" => Some(Self::Datasheet),
            "lib" | "cir" | "sub" | "sp" | "spi" | "ibs" => Some(Self::Sim),
            "step" | "stp" | "wrl" => Some(Self::Model),
            _ => None,
        }
    }
}

impl LibraryRoot {
    /// Copies a SPICE file into `simulation/` and points the symbol's `Sim.*` fields at it.
    pub(super) fn attach_sim(&self, symbol: &mut Sexpr, src: &Path, mpn: &str) -> Result<()> {
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("lib");
        let name = format!("{mpn}.{ext}");
        let dest = self.dir.join("simulation").join(&name);
        copy_file(src, &dest)?;
        set_property(symbol, "Sim.Library", &format!("{}/simulation/{name}", self.var()));
        if let Some((kind, model)) = spice_model_name(&std::fs::read_to_string(&dest).unwrap_or_default()) {
            set_property(symbol, "Sim.Name", &model);
            if kind == "subckt" {
                set_property(symbol, "Sim.Device", "SUBCKT");
            }
        }
        Ok(())
    }


    /// Adds a datasheet, SPICE model or 3D model to a part already in the library, filed like an import would.
    /// A 3D model goes on the part's footprint, so parts sharing that footprint get it too.
    pub fn attach_file(&self, category: &str, symbol_name: &str, kind: AttachKind, path: &Path) -> Result<()> {
        let cat = sanitize(category);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
        // Sim files may have any extension; the other two kinds must match what KiCad can read.
        if AttachKind::of(path).map_or(kind != AttachKind::Sim, |k| k != kind) {
            bail!("{} can't be added as that kind of file", path.display());
        }
        crate::snapshot::ensure(self, &format!("attach to {symbol_name}"))?;
        self.ensure_layout()?;
        let (mut lib, idx) = self.load_symbol(&cat, symbol_name)?;
        let symbol = &mut lib.items_mut()[idx];
        let var = self.var();
        match kind {
            AttachKind::Datasheet => {
                let name = datasheet_name(property(symbol, "Manufacturer").unwrap_or(""), symbol_name);
                copy_file(path, &self.dir.join("datasheets").join(&name))?;
                set_property(symbol, "Datasheet", &format!("{var}/datasheets/{name}"));
            }
            AttachKind::Sim => self.attach_sim(symbol, path, symbol_name)?,
            AttachKind::Model => {
                let nick = self.nickname(&cat);
                let fp_name = property(symbol, "Footprint")
                    .and_then(|f| f.strip_prefix(&format!("{nick}:")))
                    .with_context(|| format!("{symbol_name} has no footprint in {nick}, so there is nothing to put the 3D model on"))?;
                let fp_path = self.footprint_file(&cat, fp_name);
                let mut fp = sexpr::parse(&std::fs::read_to_string(&fp_path).with_context(|| format!("reading {}", fp_path.display()))?)?;
                let shapes = format!("3dmodels/{cat}.3dshapes/{fp_name}.{ext}");
                copy_file(path, &self.dir.join(&shapes))?;
                // A model re-attached in another format replaces the old one instead of leaving it behind.
                for other in ["step", "stp", "wrl"].into_iter().filter(|e| *e != ext) {
                    let _ = std::fs::remove_file(self.dir.join(format!("3dmodels/{cat}.3dshapes/{fp_name}.{other}")));
                }
                set_footprint(&mut fp, fp_name, Some(&format!("{var}/{shapes}")));
                return write_atomic(&fp_path, &sexpr::to_string(&fp));
            }
        }
        write_atomic(&self.symbol_lib(&cat), &sexpr::to_string(&lib))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_kind_from_extension() {
        assert_eq!(AttachKind::of(Path::new("a/DS.PDF")), Some(AttachKind::Datasheet));
        assert_eq!(AttachKind::of(Path::new("x.cir")), Some(AttachKind::Sim));
        assert_eq!(AttachKind::of(Path::new("x.stp")), Some(AttachKind::Model));
        assert_eq!(AttachKind::of(Path::new("x.txt")), None);
        assert!(ATTACH_EXTENSIONS.iter().all(|e| AttachKind::of(Path::new(&format!("f.{e}"))).is_some()));
    }
}
