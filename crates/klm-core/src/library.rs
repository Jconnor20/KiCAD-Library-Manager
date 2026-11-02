//! A KLM library on disk (global or per-project) and the import of one part into it.

use crate::kicad_env::{self, write_atomic};
use crate::part::Part;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Prefix older KLM versions wrote for global paths; KiCad lost the variable whenever it saved its settings.
const OLD_GLOBAL_VAR: &str = "${KLM_LIB}";
const SUBDIRS: &[&str] = &["symbols", "footprints", "3dmodels", "datasheets", "simulation"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    Global,
    Project,
}

/// State of one piece of a part (footprint, 3D model, datasheet, sim model).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Check {
    /// Present and the file exists.
    Ok,
    /// Points at a web page or another library, so it can't be checked locally.
    Link,
    /// Not set.
    Missing,
    /// Set, but the file it points at isn't there.
    Broken,
}

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

#[derive(Clone, Debug)]
pub struct PartStatus {
    pub category: String,
    /// Symbol name in the library.
    pub name: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub footprint: Check,
    pub model: Check,
    pub datasheet: Check,
    pub sim: Check,
    /// What "open datasheet" opens: a URL, or the local PDF if it exists.
    pub datasheet_target: Option<String>,
    /// The category library is listed in KiCad's tables.
    pub in_kicad: bool,
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

    fn symbol_lib(&self, category: &str) -> PathBuf {
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

    /// Every part in this library with what it has and lacks, sorted by category then name.
    pub fn list_parts(&self) -> Vec<PartStatus> {
        let mut cats = self.categories();
        cats.sort();
        let mut out = vec![];
        for cat in cats {
            let in_kicad = !self.needs_link(&cat);
            let Some(lib) = std::fs::read_to_string(self.symbol_lib(&cat)).ok().and_then(|t| sexpr::parse(&t).ok()) else { continue };
            for sym in lib.children("symbol") {
                let field = |k: &str| property(sym, k).unwrap_or("").trim().to_string();
                let name = sym.arg(0).unwrap_or_default().to_string();
                let (footprint, model) = self.check_footprint(&cat, &field("Footprint"));
                let datasheet = self.check_file(&field("Datasheet"));
                out.push(PartStatus {
                    category: cat.clone(),
                    mpn: Some(field("MPN")).filter(|m| !m.is_empty()).unwrap_or_else(|| name.clone()),
                    name,
                    manufacturer: field("Manufacturer"),
                    description: field("Description"),
                    footprint,
                    model,
                    datasheet_target: match datasheet {
                        Check::Ok => Some(self.resolve(&field("Datasheet"))),
                        Check::Link if field("Datasheet").starts_with("http") => Some(field("Datasheet")),
                        _ => None,
                    },
                    datasheet,
                    sim: self.check_file(&field("Sim.Library")),
                    in_kicad,
                });
            }
        }
        out
    }

    /// A path from a KiCad file with `${KIPRJMOD}` filled in.
    fn resolve(&self, value: &str) -> String {
        value.replace("${KIPRJMOD}", &self.table_dir.to_string_lossy())
    }

    fn check_file(&self, value: &str) -> Check {
        if value.is_empty() || value == "~" {
            return Check::Missing;
        }
        if value.starts_with("http") {
            return Check::Link;
        }
        let path = self.resolve(value);
        if path.contains("${") {
            Check::Link // another KiCad path variable; can't resolve it here
        } else if Path::new(&path).is_file() {
            Check::Ok
        } else {
            Check::Broken
        }
    }

    /// (footprint, 3D model) for a symbol's `Footprint` field.
    fn check_footprint(&self, cat: &str, field: &str) -> (Check, Check) {
        let Some((nick, name)) = field.split_once(':') else {
            return (if field.is_empty() { Check::Missing } else { Check::Broken }, Check::Missing);
        };
        if nick != self.nickname(cat) {
            return (Check::Link, Check::Link); // a footprint from another library
        }
        let path = self.dir.join("footprints").join(format!("{cat}.pretty")).join(format!("{name}.kicad_mod"));
        let Some(fp) = std::fs::read_to_string(path).ok().and_then(|t| sexpr::parse(&t).ok()) else {
            return (Check::Broken, Check::Missing);
        };
        (Check::Ok, fp.child("model").and_then(|m| m.arg(0)).map_or(Check::Missing, |m| self.check_file(m)))
    }

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
        self.ensure_layout()?;
        let nick = self.nickname(&cat);
        let work = scratch_dir()?;
        let result = self.add_part_in(part, &cat, &mpn, &nick, &work);
        let _ = std::fs::remove_dir_all(&work);
        result?;

        self.link(&cat)
    }

    /// Whether KiCad's tables are missing this category's symbol or footprint library.
    pub fn needs_link(&self, category: &str) -> bool {
        let nick = self.nickname(category);
        !kicad_env::is_registered(&self.table_dir.join("sym-lib-table"), &nick)
            || !kicad_env::is_registered(&self.table_dir.join("fp-lib-table"), &nick)
    }

    /// Existing categories that KiCad's tables don't (or no longer) list.
    pub fn unlinked(&self) -> Vec<String> {
        self.categories().into_iter().filter(|c| self.needs_link(c)).collect()
    }

    /// Adds this category's libraries to KiCad's tables. Returns true if anything was added.
    pub fn link(&self, category: &str) -> Result<bool> {
        let (cat, nick, var) = (sanitize(category), self.nickname(category), self.var());
        let new_sym = kicad_env::register_lib(&self.table_dir.join("sym-lib-table"), &nick, &format!("{var}/symbols/{cat}.kicad_sym"))?;
        let new_fp = kicad_env::register_lib(&self.table_dir.join("fp-lib-table"), &nick, &format!("{var}/footprints/{cat}.pretty"))?;
        Ok(new_sym || new_fp)
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
                    std::fs::copy(m, shapes.join(format!("{name}.{ext}")))?;
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

        merge_symbol(&self.symbol_lib(cat), &vendor_lib, symbol)
    }

    /// Copies a SPICE file into `simulation/` and points the symbol's `Sim.*` fields at it.
    fn attach_sim(&self, symbol: &mut Sexpr, src: &Path, mpn: &str) -> Result<()> {
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

    fn load_symbol(&self, cat: &str, name: &str) -> Result<(Sexpr, usize)> {
        let text = std::fs::read_to_string(self.symbol_lib(cat)).with_context(|| format!("no library {}", self.nickname(cat)))?;
        let lib = sexpr::parse(&text)?;
        let idx = lib.items().iter().position(|c| c.head() == Some("symbol") && c.arg(0) == Some(name));
        Ok((lib, idx.with_context(|| format!("{name} is not in {}", self.nickname(cat)))?))
    }

    fn footprint_file(&self, cat: &str, name: &str) -> PathBuf {
        self.dir.join("footprints").join(format!("{cat}.pretty")).join(format!("{name}.kicad_mod"))
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
                set_footprint(&mut fp, fp_name, Some(&format!("{var}/{shapes}")));
                return write_atomic(&fp_path, &sexpr::to_string(&fp));
            }
        }
        write_atomic(&self.symbol_lib(&cat), &sexpr::to_string(&lib))
    }

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

/// Adds `symbol` to the library file at `path`, replacing any symbol of that name. A new file takes
/// its header (version, generator) from `header_from` so formats match.
fn merge_symbol(path: &Path, header_from: &Sexpr, symbol: Sexpr) -> Result<()> {
    let mut lib = if path.exists() {
        sexpr::parse(&std::fs::read_to_string(path)?)?
    } else {
        Sexpr::List(header_from.items().iter().filter(|c| c.head() != Some("symbol")).cloned().collect())
    };
    let name = symbol.arg(0).unwrap_or_default().to_string();
    lib.items_mut().retain(|c| !(c.head() == Some("symbol") && c.arg(0) == Some(name.as_str())));
    lib.items_mut().push(symbol);
    write_atomic(path, &sexpr::to_string(&lib))
}

fn datasheet_name(manufacturer: &str, mpn: &str) -> String {
    format!("{}.pdf", sanitize(&format!("{manufacturer}_{mpn}")).trim_start_matches('_'))
}

/// `fs::copy` that creates the folder and does nothing when `src` already is `dest` (copying a file onto itself empties it).
fn copy_file(src: &Path, dest: &Path) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if dest.exists() && std::fs::canonicalize(src)? == std::fs::canonicalize(dest)? {
        return Ok(());
    }
    std::fs::copy(src, dest).with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
    Ok(())
}

fn scratch_dir() -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!("klm-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn has_ext(p: &Path, exts: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

pub fn property<'a>(symbol: &'a Sexpr, key: &str) -> Option<&'a str> {
    symbol.children("property").find(|p| p.arg(0) == Some(key)).and_then(|p| p.arg(1))
}

/// Sets a symbol field, adding it as a hidden field if missing.
pub fn set_property(symbol: &mut Sexpr, key: &str, value: &str) {
    let items = symbol.items_mut();
    if let Some(p) = items.iter_mut().find(|p| p.head() == Some("property") && p.arg(0) == Some(key)) {
        p.set_arg(1, Sexpr::str(value));
        return;
    }
    let new = Sexpr::list(
        "property",
        vec![
            Sexpr::str(key),
            Sexpr::str(value),
            Sexpr::list("at", vec![Sexpr::Atom("0".into()), Sexpr::Atom("0".into()), Sexpr::Atom("0".into())]),
            Sexpr::list("hide", vec![Sexpr::Atom("yes".into())]),
            Sexpr::list(
                "effects",
                vec![Sexpr::list("font", vec![Sexpr::list("size", vec![Sexpr::Atom("1.27".into()), Sexpr::Atom("1.27".into())])])],
            ),
        ],
    );
    let after_last_prop = items.iter().rposition(|p| p.head() == Some("property")).map_or(items.len(), |i| i + 1);
    items.insert(after_last_prop, new);
}

/// Renames a symbol and its unit sub-symbols (`OLD_1_1` → `NEW_1_1`).
fn rename_symbol(symbol: &mut Sexpr, new_name: &str) {
    let old = symbol.arg(0).unwrap_or_default().to_string();
    symbol.set_arg(0, Sexpr::str(new_name));
    for unit in symbol.items_mut().iter_mut().filter(|c| c.head() == Some("symbol")) {
        if let Some(suffix) = unit.arg(0).and_then(|n| n.strip_prefix(old.as_str())).map(String::from) {
            unit.set_arg(0, Sexpr::str(&format!("{new_name}{suffix}")));
        }
    }
}

/// Renames a footprint and points its 3D model at `model_uri`, keeping the vendor's offset/rotation.
fn set_footprint(fp: &mut Sexpr, name: &str, model_uri: Option<&str>) {
    fp.set_arg(0, Sexpr::str(name));
    let Some(uri) = model_uri else { return };
    let items = fp.items_mut();
    match items.iter().position(|c| c.head() == Some("model")) {
        Some(first) => {
            items[first].set_arg(0, Sexpr::str(uri));
            let mut i = 0;
            items.retain(|c| {
                i += 1;
                i - 1 == first || c.head() != Some("model")
            });
        }
        None => {
            let xyz = |v: &str| Sexpr::list("xyz", vec![Sexpr::Atom(v.into()), Sexpr::Atom(v.into()), Sexpr::Atom(v.into())]);
            items.push(Sexpr::list(
                "model",
                vec![
                    Sexpr::str(uri),
                    Sexpr::list("offset", vec![xyz("0")]),
                    Sexpr::list("scale", vec![xyz("1")]),
                    Sexpr::list("rotate", vec![xyz("0")]),
                ],
            ));
        }
    }
}

/// Equal apart from UUIDs, which `kicad-cli fp upgrade` regenerates on every run.
fn same_geometry(a: &str, b: &str) -> bool {
    fn strip(x: &mut Sexpr) {
        if let Sexpr::List(v) = x {
            v.retain(|c| !matches!(c.head(), Some("uuid" | "tstamp")));
            v.iter_mut().for_each(strip);
        }
    }
    match (sexpr::parse(a), sexpr::parse(b)) {
        (Ok(mut a), Ok(mut b)) => {
            strip(&mut a);
            strip(&mut b);
            a == b
        }
        _ => false,
    }
}

/// First `.subckt NAME` or `.model NAME` in a SPICE file.
fn spice_model_name(text: &str) -> Option<(&'static str, String)> {
    text.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        let kw = w.next()?.to_lowercase();
        let name = w.next()?.to_string();
        match kw.as_str() {
            ".subckt" => Some(("subckt", name)),
            ".model" => Some(("model", name)),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_bad_chars() {
        assert_eq!(sanitize(" LM1085IS-12/NOPB "), "LM1085IS-12_NOPB");
        assert_eq!(sanitize("A:B C"), "A_B_C");
    }

    #[test]
    fn rename_and_fields() {
        let mut s = sexpr::parse(r#"(symbol "old" (property "Value" "old") (symbol "old_0_1") (symbol "old_1_1"))"#).unwrap();
        rename_symbol(&mut s, "NEW");
        set_property(&mut s, "Value", "NEW");
        set_property(&mut s, "MPN", "NEW");
        assert_eq!(s.arg(0), Some("NEW"));
        let units: Vec<_> = s.children("symbol").filter_map(|u| u.arg(0)).collect();
        assert_eq!(units, ["NEW_0_1", "NEW_1_1"]);
        assert_eq!(property(&s, "Value"), Some("NEW"));
        assert_eq!(property(&s, "MPN"), Some("NEW"));
        // New field sits after the existing properties, before the units.
        assert_eq!(s.items()[3].head(), Some("property"));
    }

    #[test]
    fn footprint_model_path_keeps_offsets() {
        let mut fp = sexpr::parse(r#"(footprint "X" (model "a.wrl" (offset (xyz 0 0 1))) (model "b.step"))"#).unwrap();
        set_footprint(&mut fp, "Y", Some("${KLM_LIB}/3dmodels/ICs.3dshapes/Y.step"));
        let models: Vec<_> = fp.children("model").collect();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].arg(0), Some("${KLM_LIB}/3dmodels/ICs.3dshapes/Y.step"));
        assert!(sexpr::to_string(models[0]).contains("xyz 0 0 1"));
        assert_eq!(fp.arg(0), Some("Y"));
    }

    #[test]
    fn attach_kind_from_extension() {
        assert_eq!(AttachKind::of(Path::new("a/DS.PDF")), Some(AttachKind::Datasheet));
        assert_eq!(AttachKind::of(Path::new("x.cir")), Some(AttachKind::Sim));
        assert_eq!(AttachKind::of(Path::new("x.stp")), Some(AttachKind::Model));
        assert_eq!(AttachKind::of(Path::new("x.txt")), None);
        assert!(ATTACH_EXTENSIONS.iter().all(|e| AttachKind::of(Path::new(&format!("f.{e}"))).is_some()));
    }

    #[test]
    fn spice_names() {
        assert_eq!(spice_model_name("* hi\n.SUBCKT LM358 1 2 3\n"), Some(("subckt", "LM358".into())));
        assert_eq!(spice_model_name(".model D1N4148 D(Is=1n)"), Some(("model", "D1N4148".into())));
    }
}
