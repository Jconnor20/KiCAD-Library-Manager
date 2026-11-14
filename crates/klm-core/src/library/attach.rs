//! Adding a datasheet, SPICE model or 3D model to a part already in a library.

use super::fields::{datasheet_name, same_geometry, set_footprint};
use super::{LibraryRoot, copy_file, has_ext, property, sanitize, set_property};
use crate::kicad_env::{self, write_atomic};
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::path::Path;

/// What [`LibraryRoot::attach_file`] adds to a part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AttachKind {
    Datasheet,
    Sim,
    Model,
    /// A ZIP with a footprint and/or 3D model but no symbol.
    Zip,
}

/// Extensions [`AttachKind::of`] recognises (for file pickers).
pub const ATTACH_EXTENSIONS: &[&str] = &["pdf", "lib", "cir", "sub", "sp", "spi", "ckt", "mod", "ibs", "step", "stp", "wrl", "stl", "zip"];

impl AttachKind {
    pub fn of(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_lowercase().as_str() {
            "pdf" => Some(Self::Datasheet),
            ext if crate::spice::is_spice_ext(ext) => Some(Self::Sim),
            "step" | "stp" | "wrl" | "stl" => Some(Self::Model),
            "zip" => Some(Self::Zip),
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
        let text = std::fs::read_to_string(&dest).unwrap_or_default();
        let models = crate::spice::parse_models(&text);
        let hint_mpn = property(symbol, "MPN").unwrap_or(mpn);
        let sim_name_hint = property(symbol, "Sim.Name");
        if let Some(model) = crate::spice::pick_model(&models, hint_mpn, sim_name_hint) {
            set_property(symbol, "Sim.Name", &model.name);
            let pins = crate::spice::pins::extract_pins(symbol, None);
            let sym_ref = property(symbol, "Reference").unwrap_or("");
            let (sim_pins, device) = crate::spice::pins::map_sim_pins(&pins, model, sym_ref, "");
            if !device.is_empty() {
                set_property(symbol, "Sim.Device", &device);
            }
            if let Some(sub_type) = &model.sub_type {
                set_property(symbol, "Sim.Type", sub_type);
            }
            if let Some(sp) = sim_pins {
                set_property(symbol, "Sim.Pins", &sp);
            }
        }
        Ok(())
    }

    /// Resolves an existing close SPICE model or generates one from the component's datasheet/specs and attaches it.
    /// Returns the resolution details if a model was found or generated and saved.
    pub fn resolve_and_attach_sim(
        &self,
        category: &str,
        name: &str,
        spice_dirs: &[std::path::PathBuf],
        allow_generate: bool,
    ) -> Result<Option<crate::spice::SimResolution>> {
        let cat = sanitize(category);
        let (mut lib, idx) = self.load_symbol(&cat, name)?;
        let sym_snap = lib.items()[idx].clone();

        let field = |k: &str| property(&sym_snap, k).unwrap_or("").trim().to_string();
        let mut ds_file = None;
        let ds_raw = field("Datasheet");
        if !ds_raw.is_empty() && !ds_raw.starts_with("http://") && !ds_raw.starts_with("https://") {
            ds_file = crate::spice::matcher::resolve_sim_library_path(&ds_raw, self).or_else(|| {
                let expanded = if ds_raw.contains("${KIPRJMOD}")
                    && let Some(p) = self.dir.parent()
                {
                    ds_raw.replace("${KIPRJMOD}", &p.to_string_lossy().replace('\\', "/"))
                } else {
                    ds_raw.clone()
                };
                Some(std::path::PathBuf::from(expanded))
            });
        }
        let part = crate::part::Part {
            mpn: Some(field("MPN")).filter(|m| !m.is_empty()).unwrap_or_else(|| name.to_string()),
            manufacturer: field("Manufacturer"),
            description: field("Description"),
            datasheet_file: ds_file.filter(|p| p.is_file()),
            ..Default::default()
        };
        let work = std::env::temp_dir().join("klm-sim");
        let Some(res) = crate::spice::resolve_or_generate(
            &part,
            Some(&sym_snap),
            Some(&lib),
            &cat,
            Some(self),
            spice_dirs,
            allow_generate,
            &work,
        )?
        else {
            return Ok(None);
        };

        let _guard = crate::snapshot::guard(self, &format!("attach sim for {name}"))?;
        let sym_mut = &mut lib.items_mut()[idx];
        self.attach_sim(sym_mut, &res.file, name)?;
        if let Some(sp) = &res.pins {
            set_property(sym_mut, "Sim.Pins", sp);
        }
        write_atomic(&self.symbol_lib(&cat), &sexpr::to_string(&lib))?;
        Ok(Some(res))
    }


    /// Attaches a ZIP with a footprint and/or 3D model but no symbol (see `sources::read_zip`) to a part already in
    /// the library: the footprint is filed in the category's `.pretty` and linked from the symbol, and the model is
    /// put on it. A ZIP with only a model goes on the part's current footprint.
    pub fn attach_zip(&self, category: &str, symbol_name: &str, zip: &Path) -> Result<()> {
        let work = super::import::scratch_dir()?;
        let result = self.attach_zip_in(&sanitize(category), symbol_name, zip, &work);
        let _ = std::fs::remove_dir_all(&work);
        result
    }

    fn attach_zip_in(&self, cat: &str, symbol_name: &str, zip: &Path, work: &Path) -> Result<()> {
        let part = crate::sources::zip_import::read_zip(zip, &work.join("zip"))?.with_context(|| format!("{} has no footprint or 3D model", zip.display()))?;
        if part.symbol_file.is_some() {
            bail!("{} has a symbol; import it as a new part instead", zip.display());
        }
        let model = part.model_files.iter().find(|m| has_ext(m, &["step", "stp"])).or_else(|| part.model_files.first());
        let Some(fp_src) = part.footprint_files.first() else {
            return self.attach_file(cat, symbol_name, AttachKind::Model, model.context("the ZIP holds nothing to attach")?);
        };

        crate::snapshot::ensure(self, &format!("attach footprint to {symbol_name}"))?;
        self.ensure_layout()?;
        let (mut lib, idx) = self.load_symbol(cat, symbol_name)?;
        let (nick, var) = (self.nickname(cat), self.var());

        let (fp_in, fp_out) = (work.join("in.pretty"), work.join("out.pretty"));
        std::fs::create_dir_all(&fp_in)?;
        // KiCad names a footprint after its file, so name the file after the footprint inside it.
        let text = std::fs::read_to_string(fp_src)?;
        let vendor = sexpr::parse(&text).ok().and_then(|s| s.arg(0).map(sanitize)).unwrap_or_else(|| sanitize(symbol_name));
        std::fs::write(fp_in.join(format!("{vendor}.kicad_mod")), text)?;
        kicad_env::upgrade_footprints(&fp_in, &fp_out)?;
        let mut fp = sexpr::parse(&std::fs::read_to_string(fp_out.join(format!("{vendor}.kicad_mod")))?)?;
        let ext = model.and_then(|m| m.extension()).and_then(|e| e.to_str()).unwrap_or("step").to_lowercase();
        let render = |fp: &mut Sexpr, name: &str| {
            set_footprint(fp, name, model.map(|_| format!("{var}/3dmodels/{cat}.3dshapes/{name}.{ext}")).as_deref());
            sexpr::to_string(fp)
        };
        // Same name but different geometry from another part → keep both.
        let (mut name, pretty) = (vendor.clone(), self.dir.join("footprints").join(format!("{cat}.pretty")));
        let mut text = render(&mut fp, &name);
        if let Ok(old) = std::fs::read_to_string(pretty.join(format!("{name}.kicad_mod"))) {
            if !same_geometry(&old, &text) {
                name = format!("{vendor}_{}", sanitize(symbol_name));
                text = render(&mut fp, &name);
            }
        }
        std::fs::create_dir_all(&pretty)?;
        write_atomic(&pretty.join(format!("{name}.kicad_mod")), &text)?;
        if let Some(m) = model {
            copy_file(m, &self.dir.join(format!("3dmodels/{cat}.3dshapes/{name}.{ext}")))?;
        }
        set_property(&mut lib.items_mut()[idx], "Footprint", &format!("{nick}:{name}"));
        write_atomic(&self.symbol_lib(cat), &sexpr::to_string(&lib))
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
        if kind == AttachKind::Zip {
            return self.attach_zip(&cat, symbol_name, path);
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
            AttachKind::Zip => unreachable!("handled above"),
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
    use std::path::PathBuf;

    fn zip_of(path: &Path, files: &[(&str, &str)]) {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    fn lib(name: &str) -> (LibraryRoot, PathBuf) {
        let dir = crate::test_dir(name);
        let root = LibraryRoot::global(dir.join("lib"), dir.join("config"));
        root.ensure_layout().unwrap();
        std::fs::write(root.dir.join("symbols/ICs.kicad_sym"), r#"(kicad_symbol_lib (symbol "A" (property "Footprint" "" (at 0 0 0))))"#).unwrap();
        (root, dir)
    }

    #[test]
    fn model_only_zip_goes_on_the_current_footprint() {
        let (root, dir) = lib("attach_zip_model");
        std::fs::write(root.dir.join("symbols/ICs.kicad_sym"), r#"(kicad_symbol_lib (symbol "A" (property "Footprint" "KLM_ICs:FP1" (at 0 0 0))))"#).unwrap();
        std::fs::create_dir_all(root.dir.join("footprints/ICs.pretty")).unwrap();
        std::fs::write(root.dir.join("footprints/ICs.pretty/FP1.kicad_mod"), r#"(footprint "FP1")"#).unwrap();
        let zp = dir.join("m.zip");
        zip_of(&zp, &[("x/FP1.step", "STEP")]);
        root.attach_zip("ICs", "A", &zp).unwrap();
        assert!(root.dir.join("3dmodels/ICs.3dshapes/FP1.step").is_file());
        assert!(std::fs::read_to_string(root.dir.join("footprints/ICs.pretty/FP1.kicad_mod")).unwrap().contains("ICs.3dshapes/FP1.step"));
    }

    #[test]
    fn footprint_zip_is_linked_from_the_symbol() {
        if !kicad_env::kicad_cli().is_ok_and(|c| std::process::Command::new(c).arg("version").output().is_ok_and(|o| o.status.success())) {
            eprintln!("kicad-cli not found; skipping");
            return;
        }
        let (root, dir) = lib("attach_zip_fp");
        let zp = dir.join("f.zip");
        zip_of(&zp, &[("R1.kicad_mod", r#"(footprint "R1" (layer "F.Cu") (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu")))"#), ("R1.step", "STEP")]);
        root.attach_zip("ICs", "A", &zp).unwrap();
        let sym = std::fs::read_to_string(root.dir.join("symbols/ICs.kicad_sym")).unwrap();
        assert!(sym.contains("KLM_ICs:R1"), "{sym}");
        assert!(std::fs::read_to_string(root.dir.join("footprints/ICs.pretty/R1.kicad_mod")).unwrap().contains("ICs.3dshapes/R1.step"));
        assert!(root.dir.join("3dmodels/ICs.3dshapes/R1.step").is_file());
    }

    #[test]
    fn zip_with_a_symbol_is_refused() {
        let (root, dir) = lib("attach_zip_sym");
        let zp = dir.join("s.zip");
        zip_of(&zp, &[("S.kicad_sym", "(kicad_symbol_lib)")]);
        assert!(root.attach_zip("ICs", "A", &zp).unwrap_err().to_string().contains("has a symbol"));
    }

    #[test]
    fn attach_kind_from_extension() {
        assert_eq!(AttachKind::of(Path::new("a/DS.PDF")), Some(AttachKind::Datasheet));
        assert_eq!(AttachKind::of(Path::new("x.cir")), Some(AttachKind::Sim));
        assert_eq!(AttachKind::of(Path::new("x.stp")), Some(AttachKind::Model));
        assert_eq!(AttachKind::of(Path::new("x.txt")), None);
        assert!(ATTACH_EXTENSIONS.iter().all(|e| AttachKind::of(Path::new(&format!("f.{e}"))).is_some()));
    }
}
