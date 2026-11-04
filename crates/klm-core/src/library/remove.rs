//! Deleting a part: its symbol always, its footprint, 3D model, datasheet and sim model only when no other part uses them.
//!
//! This deletes user data, so anything that can't be read or resolved stops the whole removal (nothing is guessed),
//! and a snapshot is taken before the first write so History can roll it back.

use super::{LibraryRoot, property, sanitize};
use crate::kicad_env::write_atomic;
use crate::sexpr;
use anyhow::{Context, Result, bail};
use std::path::PathBuf;

/// One file of a part and what happens to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    /// "symbol", "footprint", "3D model", "datasheet" or "sim model".
    pub kind: &'static str,
    pub name: String,
    /// For a kept piece: why it stays.
    pub why: String,
}

/// What [`LibraryRoot::plan_remove`] found; [`LibraryRoot::remove_part`] carries it out.
#[derive(Clone, Debug)]
pub struct RemovePlan {
    pub category: String,
    pub name: String,
    pub mpn: String,
    pub removes: Vec<Piece>,
    pub keeps: Vec<Piece>,
    files: Vec<PathBuf>,
}

impl RemovePlan {
    /// One line per piece, for a confirm dialog or the CLI.
    pub fn lines(&self) -> Vec<String> {
        let removes = self.removes.iter().map(|p| format!("Remove {} {}", p.kind, p.name));
        let keeps = self.keeps.iter().map(|p| format!("Keep {} {} ({})", p.kind, p.name, p.why));
        removes.chain(keeps).collect()
    }
}

impl LibraryRoot {
    /// Every other symbol (category / name) with a field equal to `value`. Reads every symbol library; one that
    /// can't be read is an error, since it might hold a user.
    fn symbols_using(&self, skip: &[(String, String)], value: &str) -> Result<Vec<String>> {
        let mut users = vec![];
        for cat in self.categories() {
            let path = self.symbol_lib(&cat);
            let lib = sexpr::parse(&std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?)
                .with_context(|| format!("parsing {}", path.display()))?;
            for s in lib.children("symbol") {
                let name = s.arg(0).unwrap_or_default();
                if !skip.iter().any(|(c, n)| (c.as_str(), n.as_str()) == (cat.as_str(), name)) && s.children("property").any(|p| p.arg(1) == Some(value)) {
                    users.push(format!("{cat} / {name}"));
                }
            }
        }
        Ok(users)
    }

    /// Footprint files (other than `skip`) whose text mentions `needle`.
    fn footprints_mentioning(&self, skip: &[PathBuf], needle: &str) -> Result<Vec<String>> {
        let mut users = vec![];
        for pretty in std::fs::read_dir(self.dir.join("footprints")).into_iter().flatten().flatten() {
            for f in std::fs::read_dir(pretty.path()).into_iter().flatten().flatten().map(|e| e.path()) {
                if !skip.contains(&f) && super::has_ext(&f, &["kicad_mod"]) && std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?.contains(needle) {
                    users.push(f.file_stem().unwrap_or_default().to_string_lossy().into_owned());
                }
            }
        }
        Ok(users)
    }

    /// What deleting this part would remove and keep. Errors (and so refuses) when the part, a footprint
    /// or any library it must check for other users can't be read.
    pub fn plan_remove(&self, category: &str, symbol_name: &str) -> Result<RemovePlan> {
        self.plan_remove_after(category, symbol_name, &[])
    }

    /// Plans every part of a selection as [`LibraryRoot::remove_part`] will run them, one after the other: each plan
    /// treats the parts before it as already deleted, so a file shared by the selection is removed with the last part.
    pub fn plan_remove_all(&self, parts: &[(String, String)]) -> Vec<Result<RemovePlan>> {
        (0..parts.len()).map(|i| self.plan_remove_after(&parts[i].0, &parts[i].1, &parts[..i])).collect()
    }

    /// [`LibraryRoot::plan_remove`] as if the parts in `gone` were already deleted.
    fn plan_remove_after(&self, category: &str, symbol_name: &str, gone: &[(String, String)]) -> Result<RemovePlan> {
        let cat = sanitize(category);
        let (lib, idx) = self.load_symbol(&cat, symbol_name)?;
        let sym = &lib.items()[idx];
        let mut skip: Vec<(String, String)> = gone.iter().map(|(c, n)| (sanitize(c), n.clone())).collect();
        skip.push((cat.clone(), symbol_name.into()));
        let skip = &skip[..];
        // Footprint files that the gone parts take with them (nothing outside `skip` uses them).
        let mut gone_files = vec![];
        for (c, n) in &skip[..skip.len() - 1] {
            let Ok((l, i)) = self.load_symbol(c, n) else { continue };
            let field = property(&l.items()[i], "Footprint").unwrap_or("").trim().to_string();
            if let Some(fp) = field.strip_prefix(&format!("{}:", self.nickname(c))) {
                if !fp.is_empty() && !fp.contains(['/', '\\']) && !fp.contains("..") && self.symbols_using(skip, &field)?.is_empty() {
                    gone_files.push(self.footprint_file(c, fp));
                }
            }
        }
        let mut plan = RemovePlan {
            category: cat.clone(),
            name: symbol_name.into(),
            mpn: property(sym, "MPN").map(str::trim).filter(|m| !m.is_empty()).unwrap_or(symbol_name).into(),
            removes: vec![Piece { kind: "symbol", name: symbol_name.into(), why: String::new() }],
            keeps: vec![],
            files: vec![],
        };
        let var = self.var();
        let decide =|plan: &mut RemovePlan, kind: &'static str, name: String, file: Option<PathBuf>, users: Vec<String>, why_not: &str| {
            if !users.is_empty() {
                plan.keeps.push(Piece { kind, name, why: format!("also used by {}", users.join(", ")) });
            } else if let Some(file) = file.filter(|f| f.is_file()) {
                plan.removes.push(Piece { kind, name, why: String::new() });
                plan.files.push(file);
            } else {
                plan.keeps.push(Piece { kind, name, why: why_not.into() });
            }
        };
        // A path under this library, or None for a link, another library or an empty field. `..` is refused outright.
        let local = |value: &str, sub: &str| -> Result<Option<PathBuf>> {
            let Some(rel) = value.strip_prefix(&format!("{var}/{sub}/")) else { return Ok(None) };
            if rel.split(['/', '\\']).any(|c| c == ".." || c.is_empty()) {
                bail!("{value} is not a path inside the library; refusing to delete it");
            }
            Ok(Some(self.dir.join(sub).join(rel)))
        };

        for (key, sub, kind) in [("Datasheet", "datasheets", "datasheet"), ("Sim.Library", "simulation", "sim model")] {
            let value = property(sym, key).unwrap_or("").trim();
            if let Some(file) = local(value, sub)? {
                let users = self.symbols_using(skip, value)?;
                decide(&mut plan, kind, value.rsplit('/').next().unwrap_or(value).into(), Some(file), users, "file not found");
            }
        }

        let field = property(sym, "Footprint").unwrap_or("").trim();
        if let Some(fp_name) = field.strip_prefix(&format!("{}:", self.nickname(&cat))) {
            if fp_name.is_empty() || fp_name.contains(['/', '\\']) || fp_name.contains("..") {
                bail!("footprint {field} is not a valid name; refusing to delete it");
            }
            let file = self.footprint_file(&cat, fp_name);
            let users = self.symbols_using(skip, field)?;
            // Read it even when shared or missing-model-free, so a footprint we can't understand stops the delete.
            let text = file.is_file().then(|| std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))).transpose()?;
            let fp = text.map(|t| sexpr::parse(&t).with_context(|| format!("parsing {}", file.display()))).transpose()?;
            let shared = !users.is_empty();
            decide(&mut plan, "footprint", fp_name.into(), Some(file.clone()), users, "file not found");
            if let (Some(fp), false) = (fp, shared) {
                for model in fp.children("model").filter_map(|m| m.arg(0)) {
                    match local(model, "3dmodels")? {
                        Some(path) => {
                            let mut skip_fp = gone_files.clone();
                            skip_fp.push(file.clone());
                            let users = self.footprints_mentioning(&skip_fp, model)?;
                            decide(&mut plan, "3D model", model.rsplit('/').next().unwrap_or(model).into(), Some(path), users, "file not found");
                        }
                        None => plan.keeps.push(Piece { kind: "3D model", name: model.into(), why: "not stored in this library".into() }),
                    }
                }
            }
        }
        Ok(plan)
    }

    /// Deletes the part `plan` describes after a snapshot (one already held by a [`crate::snapshot::guard`] covers it).
    /// The symbol library is rewritten first; the other files are removed after, and a failure there is reported
    /// with what is left. The symbol library file stays even when it's now empty.
    pub fn remove_part(&self, plan: &RemovePlan) -> Result<()> {
        // Plan again: the library may have changed since the plan was shown.
        let fresh = self.plan_remove(&plan.category, &plan.name)?;
        if fresh.files != plan.files || fresh.removes != plan.removes {
            bail!("{} changed since it was reviewed; look at it again", plan.name);
        }
        crate::snapshot::ensure(self, &format!("delete {}", plan.name))?;
        let (mut lib, idx) = self.load_symbol(&plan.category, &plan.name)?;
        lib.items_mut().remove(idx);
        write_atomic(&self.symbol_lib(&plan.category), &sexpr::to_string(&lib))?;
        for f in &plan.files {
            match std::fs::remove_file(f) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("{} was deleted but {} could not be removed; the snapshot has it", plan.name, f.display())),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Category ICs: A and B share footprint FP1 (with a model) and datasheet; C has its own FP2, model, datasheet, sim.
    fn fixture(name: &str) -> LibraryRoot {
        let dir = crate::test_dir(name);
        let root = LibraryRoot::global(dir.join("lib"), dir.join("config"));
        root.ensure_layout().unwrap();
        let g = root.var();
        let part = |n: &str, fp: &str, ds: &str, sim: &str| {
            format!(
                r#"(symbol "{n}" (property "Footprint" "KLM_ICs:{fp}" (at 0 0 0)) (property "Datasheet" "{ds}" (at 0 0 0)) (property "Sim.Library" "{sim}" (at 0 0 0)) (symbol "{n}_1_1"))"#
            )
        };
        let shared = format!("{g}/datasheets/shared.pdf");
        let sym = format!(
            "(kicad_symbol_lib (version 20251024) {} {} {})",
            part("A", "FP1", &shared, ""),
            part("B", "FP1", &shared, ""),
            part("C", "FP2", &format!("{g}/datasheets/c.pdf"), &format!("{g}/simulation/c.lib"))
        );
        std::fs::write(root.symbol_lib("ICs"), sym).unwrap();
        let pretty = root.dir.join("footprints/ICs.pretty");
        std::fs::create_dir_all(&pretty).unwrap();
        for (fp, model) in [("FP1", "FP1"), ("FP2", "FP2")] {
            let text = format!(r#"(footprint "{fp}" (model "{g}/3dmodels/ICs.3dshapes/{model}.step"))"#);
            std::fs::write(pretty.join(format!("{fp}.kicad_mod")), text).unwrap();
        }
        std::fs::create_dir_all(root.dir.join("3dmodels/ICs.3dshapes")).unwrap();
        for f in ["3dmodels/ICs.3dshapes/FP1.step", "3dmodels/ICs.3dshapes/FP2.step", "datasheets/shared.pdf", "datasheets/c.pdf", "simulation/c.lib"] {
            std::fs::write(root.dir.join(f), "x").unwrap();
        }
        root
    }

    fn exists(root: &LibraryRoot, f: &str) -> bool {
        root.dir.join(f).exists()
    }

    #[test]
    fn shared_footprint_model_and_datasheet_are_kept() {
        let root = fixture("remove_shared");
        let plan = root.plan_remove("ICs", "A").unwrap();
        assert_eq!(plan.removes.iter().map(|p| p.kind).collect::<Vec<_>>(), ["symbol"]);
        assert!(plan.keeps.iter().any(|p| p.kind == "footprint" && p.why.contains("ICs / B")), "{:?}", plan.lines());
        root.remove_part(&plan).unwrap();
        assert!(!root.has_part("ICs", "A") && root.has_part("ICs", "B"));
        assert!(exists(&root, "footprints/ICs.pretty/FP1.kicad_mod") && exists(&root, "3dmodels/ICs.3dshapes/FP1.step") && exists(&root, "datasheets/shared.pdf"));
        assert!(!crate::snapshot::list(&root).is_empty(), "no snapshot taken");

        // Now B is the last user, so everything of it goes.
        let plan = root.plan_remove("ICs", "B").unwrap();
        root.remove_part(&plan).unwrap();
        assert!(!exists(&root, "footprints/ICs.pretty/FP1.kicad_mod") && !exists(&root, "3dmodels/ICs.3dshapes/FP1.step") && !exists(&root, "datasheets/shared.pdf"));
    }

    #[test]
    fn deleting_every_sharer_plans_the_shared_files_with_the_last() {
        let root = fixture("remove_many");
        let keys = vec![("ICs".to_string(), "A".to_string()), ("ICs".to_string(), "B".to_string())];
        let plans: Vec<_> = root.plan_remove_all(&keys).into_iter().map(Result::unwrap).collect();
        assert!(plans[0].keeps.iter().any(|p| p.kind == "footprint" && p.why.contains("ICs / B")), "{:?}", plans[0].lines());
        let last: Vec<_> = plans[1].removes.iter().map(|p| p.kind).collect();
        assert_eq!(last, ["symbol", "datasheet", "footprint", "3D model"], "{:?}", plans[1].lines());
        assert!(plans[1].keeps.is_empty());
        for p in &plans {
            root.remove_part(&root.plan_remove(&p.category, &p.name).unwrap()).unwrap();
        }
        assert!(!exists(&root, "footprints/ICs.pretty/FP1.kicad_mod") && !exists(&root, "datasheets/shared.pdf"));
    }

    #[test]
    fn unshared_files_go_with_the_part() {
        let root = fixture("remove_own");
        let plan = root.plan_remove("ICs", "C").unwrap();
        assert_eq!(plan.removes.len(), 5, "{:?}", plan.lines());
        assert!(plan.keeps.is_empty());
        root.remove_part(&plan).unwrap();
        for f in ["footprints/ICs.pretty/FP2.kicad_mod", "3dmodels/ICs.3dshapes/FP2.step", "datasheets/c.pdf", "simulation/c.lib"] {
            assert!(!exists(&root, f), "{f} left behind");
        }
        assert!(exists(&root, "footprints/ICs.pretty/FP1.kicad_mod") && root.has_part("ICs", "A"));
    }

    #[test]
    fn a_model_shared_by_two_footprints_is_kept() {
        let root = fixture("remove_model");
        let g = root.var();
        std::fs::write(root.dir.join("footprints/ICs.pretty/FP3.kicad_mod"), format!(r#"(footprint "FP3" (model "{g}/3dmodels/ICs.3dshapes/FP2.step"))"#)).unwrap();
        let plan = root.plan_remove("ICs", "C").unwrap();
        assert!(plan.keeps.iter().any(|p| p.kind == "3D model" && p.why.contains("FP3")), "{:?}", plan.lines());
        root.remove_part(&plan).unwrap();
        assert!(exists(&root, "3dmodels/ICs.3dshapes/FP2.step") && !exists(&root, "footprints/ICs.pretty/FP2.kicad_mod"));
    }

    #[test]
    fn refuses_when_it_cannot_resolve_references() {
        let root = fixture("remove_refuse");
        let sym = std::fs::read_to_string(root.symbol_lib("ICs")).unwrap();
        // An unparseable footprint: its models are unknown.
        std::fs::write(root.dir.join("footprints/ICs.pretty/FP2.kicad_mod"), "(footprint \"FP2\" (model").unwrap();
        assert!(root.plan_remove("ICs", "C").is_err());
        // An unreadable other library could hold a user.
        std::fs::write(root.symbol_lib("Other"), "(((").unwrap();
        assert!(root.plan_remove("ICs", "A").is_err());
        std::fs::remove_file(root.symbol_lib("Other")).unwrap();
        // A path escaping the library.
        let g = root.var();
        std::fs::write(root.symbol_lib("ICs"), sym.replace(&format!("{g}/datasheets/c.pdf"), &format!("{g}/datasheets/../../c.pdf"))).unwrap();
        assert!(root.plan_remove("ICs", "C").is_err());
        assert!(root.plan_remove("ICs", "NOPE").is_err());
        assert!(exists(&root, "datasheets/c.pdf") && root.has_part("ICs", "C"), "a refusal must change nothing");
    }
}
