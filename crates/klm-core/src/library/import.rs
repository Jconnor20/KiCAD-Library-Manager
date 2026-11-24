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
        self.add_part_named(part, category, &part.mpn, overwrite)
    }

    /// [`add_part`](Self::add_part) with the symbol called `name` (the user's name template); the MPN field stays `part.mpn`.
    pub fn add_part_named(&self, part: &Part, category: &str, name: &str, overwrite: bool) -> Result<bool> {
        let cat = sanitize(category);
        let mpn = sanitize(name);
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

        let linked = self.link(&cat);
        if linked.is_ok() && self.scope == super::Scope::Global {
            crate::shadow::sync_after_import(self);
        }
        linked
    }

    /// Fetches the part's symbol, footprint, 3D model and datasheet from `sources` again (by its LCSC number if it has one,
    /// else its MPN) and replaces the library copy. Returns the sources that supplied files. Undo it from History.
    pub fn reimport(&self, category: &str, name: &str, sources: &[Box<dyn crate::sources::Source>], work: &Path) -> Result<Vec<String>> {
        use crate::sources::combine::{Combined, gather};
        let lib = sexpr::parse(&std::fs::read_to_string(self.symbol_lib(category))?)?;
        let sym = lib.children("symbol").find(|s| s.arg(0) == Some(name)).with_context(|| format!("{name} is not in {category}"))?;
        let field = |k: &str| property(sym, k).unwrap_or("").trim().to_string();
        let mpn = Some(field("MPN")).filter(|m| !m.is_empty()).unwrap_or_else(|| name.to_string());
        let lcsc = field("LCSC");
        let chosen = Part {
            mpn,
            manufacturer: field("Manufacturer"),
            description: field("Description"),
            supplier_ids: if lcsc.is_empty() { vec![] } else { vec![("LCSC".into(), lcsc.clone())] },
            source: if lcsc.is_empty() { String::new() } else { "LCSC".into() },
            ..Part::default()
        };
        let from = sources.iter().position(|s| !lcsc.is_empty() && s.name() == "LCSC").unwrap_or(usize::MAX);
        let state = std::sync::Mutex::new((Combined::default(), chosen.clone(), vec![], vec![]));
        gather(sources, &chosen, from, work, |source, found| {
            let mut g = state.lock().unwrap_or_else(|e| e.into_inner());
            match found {
                Ok(Some(p)) => {
                    g.3.push(p.warnings.join("; "));
                    g.2.push(source.to_string());
                    let (combined, meta, ..) = &mut *g;
                    combined.add(meta, p);
                }
                Ok(None) => {}
                Err(e) => g.3.push(format!("{source}: {e:#}")),
            }
        });
        let (combined, meta, used, why) = state.into_inner().unwrap_or_else(|e| e.into_inner());
        let part = combined.to_part(&meta);
        if part.symbol_file.is_none() {
            bail!("no source has CAD files for {}{}", chosen.mpn, if sources.is_empty() { " (no source is enabled)".into() } else { format!(": {}", why.into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join("; ")) });
        }
        self.add_part_named(&part, category, name, true)?;
        Ok(used)
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
            let name = datasheet_name(&part.manufacturer, &sanitize(&part.mpn));
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
        // Before the restyle, which sorts pins onto sides by type and lets SPICE pin mapping see filled pin types.
        crate::symbols::pintype::fill(&mut symbol, cat);

        if let Some(sim) = part.sim_files.first() {
            self.attach_sim(&mut symbol, sim, &sanitize(&part.mpn))?;
        } else {
            let settings = crate::settings::Settings::load();
            if let Some(existing_lib) = property(&symbol, "Sim.Library")
                .and_then(|raw| crate::spice::matcher::resolve_sim_library_path(raw, self))
                .filter(|p| p.is_file())
            {
                self.attach_sim(&mut symbol, &existing_lib, &sanitize(&part.mpn))?;
            } else if settings.sim.auto_sim
                && let Some(res) = crate::spice::resolve_or_generate(
                    part,
                    Some(&symbol),
                    Some(&vendor_lib),
                    cat,
                    Some(self),
                    &settings.sim.spice_dirs,
                    settings.sim.auto_generate_from_datasheet,
                    work,
                )?
            {
                self.attach_sim(&mut symbol, &res.file, &sanitize(&part.mpn))?;
                if let Some(sp) = &res.pins {
                    set_property(&mut symbol, "Sim.Pins", sp);
                }
            }
        }

        crate::symbols::restyle_imported(cat, &mut symbol);
        crate::fields::fix_imported(cat, &mut symbol, part);
        merge_symbol(&self.symbol_lib(cat), &vendor_lib, symbol)
    }
}

pub(super) fn scratch_dir() -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!("klm-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
