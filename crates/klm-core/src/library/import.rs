//! Importing one part into a library.

use super::fields::{rename_symbol, same_geometry, set_footprint, datasheet_name, merge_symbol};
use super::{LibraryRoot, copy_file, has_ext, property, sanitize, set_property};
use crate::kicad_env::{self, write_atomic};
use crate::part::Part;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

impl LibraryRoot {
    /// Imports `part` into `category`. Returns true if the category library was new to KiCad's tables.
    pub fn add_part(&self, part: &Part, category: &str, overwrite: bool) -> Result<bool> {
        let cat = sanitize(category);
        let mpn = sanitize(&part.mpn);
        if cat.is_empty() || mpn.is_empty() {
            bail!("category and MPN must not be empty");
        }
        if !overwrite && self.has_part(&cat, &mpn) {
            bail!("{mpn} is already in {}", self.nickname(&cat));
        }
        let _snapshot = crate::snapshot::guard(self, &format!("import {mpn}"))?;
        self.ensure_layout()?;
        let nick = self.nickname(&cat);
        let work = scratch_dir()?;
        let result = self.add_part_in(part, &cat, &mpn, &nick, &work);
        let _ = std::fs::remove_dir_all(&work);
        result?;

        self.link(&cat)
    }

    fn add_part_in(&self, part: &Part, cat: &str, mpn: &str, nick: &str, work: &Path) -> Result<()> {
        let var = self.var();

        // Symbol: bring to the installed KiCad's format, then pick the one we want.
        let sym_src = part.symbol_file.as_ref().context("part has no symbol file")?;
        let up_sym = work.join("sym.kicad_sym");
        kicad_env::upgrade_symbols(sym_src, &up_sym)?;
        let vendor_lib = sexpr::parse(&std::fs::read_to_string(&up_sym)?)?;
        let mut symbol = vendor_lib
            .children("symbol")
            .find(|s| s.arg(0).is_some_and(|n| n.eq_ignore_ascii_case(&part.mpn)))
            .or_else(|| vendor_lib.child("symbol"))
            .context("symbol file contains no symbols")?
            .clone();
        let wanted_fp = property(&symbol, "Footprint").and_then(|f| f.rsplit(':').next()).unwrap_or("").to_string();

        // Footprints.
        let mut main_fp = None;
        if !part.footprint_files.is_empty() {
            let fp_in = work.join("in.pretty");
            let fp_out = work.join("out.pretty");
            std::fs::create_dir_all(&fp_in)?;
            // KiCad names a footprint after its file, so name each file after the footprint inside it.
            for (i, f) in part.footprint_files.iter().enumerate() {
                let text = std::fs::read_to_string(f)?;
                let inner = sexpr::parse(&text).ok().and_then(|s| s.arg(0).map(sanitize));
                let stem = inner.unwrap_or_else(|| f.file_stem().unwrap_or_default().to_string_lossy().into());
                let mut dest = fp_in.join(format!("{stem}.kicad_mod"));
                if dest.exists() {
                    dest = fp_in.join(format!("{stem}_{i}.kicad_mod"));
                }
                std::fs::write(dest, text)?;
            }
            kicad_env::upgrade_footprints(&fp_in, &fp_out)?;
            let mut fps = vec![];
            for e in std::fs::read_dir(&fp_out)?.flatten() {
                fps.push(sexpr::parse(&std::fs::read_to_string(e.path())?)?);
            }
            let main_idx = fps.iter().position(|f| f.arg(0) == Some(&wanted_fp)).unwrap_or(0);
            let model = part
                .model_files
                .iter()
                .find(|m| has_ext(m, &["step", "stp"]))
                .or_else(|| part.model_files.first());

            let pretty = self.dir.join("footprints").join(format!("{cat}.pretty"));
            let shapes = self.dir.join("3dmodels").join(format!("{cat}.3dshapes"));
            std::fs::create_dir_all(&pretty)?;
            for (i, mut fp) in fps.into_iter().enumerate() {
                let vendor_name = sanitize(fp.arg(0).unwrap_or(mpn));
                let model = model.filter(|_| i == main_idx);
                let ext = model.and_then(|m| m.extension()).and_then(|e| e.to_str()).unwrap_or("step").to_lowercase();
                let render = |fp: &mut Sexpr, name: &str| {
                    let model_uri = model.map(|_| format!("{var}/3dmodels/{cat}.3dshapes/{name}.{ext}"));
                    set_footprint(fp, name, model_uri.as_deref());
                    sexpr::to_string(fp)
                };
                // Same name but different geometry from another part → keep both.
                let mut name = vendor_name.clone();
                let mut text = render(&mut fp, &name);
                let existing = pretty.join(format!("{name}.kicad_mod"));
                if existing.exists() && !same_geometry(&std::fs::read_to_string(&existing)?, &text) {
                    name = format!("{vendor_name}_{mpn}");
                    text = render(&mut fp, &name);
                }
                write_atomic(&pretty.join(format!("{name}.kicad_mod")), &text)?;
                if let Some(m) = model {
                    std::fs::create_dir_all(&shapes)?;
                    copy_file(m, &shapes.join(format!("{name}.{ext}")))?;
                }
                if i == main_idx {
                    main_fp = Some(name);
                }
            }
        }

        // Symbol fields.
        rename_symbol(&mut symbol, mpn);
        if let Some(fp) = &main_fp {
            set_property(&mut symbol, "Footprint", &format!("{nick}:{fp}"));
        }
        if let Some(ds) = &part.datasheet_file {
            let name = datasheet_name(&part.manufacturer, mpn);
            copy_file(ds, &self.dir.join("datasheets").join(&name))?;
            set_property(&mut symbol, "Datasheet", &format!("{var}/datasheets/{name}"));
        } else if !part.datasheet_url.is_empty() {
            set_property(&mut symbol, "Datasheet", &part.datasheet_url);
        }
        if !part.description.is_empty() {
            set_property(&mut symbol, "Description", &part.description);
        }
        if !part.manufacturer.is_empty() {
            set_property(&mut symbol, "Manufacturer", &part.manufacturer);
        }
        set_property(&mut symbol, "MPN", part.mpn.trim());
        for (k, v) in &part.supplier_ids {
            set_property(&mut symbol, k, v);
        }
        if let Some(sim) = part.sim_files.first() {
            self.attach_sim(&mut symbol, sim, mpn)?;
        }

        crate::symbols::restyle_imported(cat, &mut symbol);
        merge_symbol(&self.symbol_lib(cat), &vendor_lib, symbol)
    }
}

pub(super) fn scratch_dir() -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!("klm-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
