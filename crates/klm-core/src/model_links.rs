//! Repair model paths in KLM footprints and restore missing model assignments on placed footprints.
//! Only model expressions are edited; the rest of a board stays byte-for-byte unchanged.

use crate::{
    cleanup::kids,
    kicad_env,
    library::{LibraryRoot, Scope, property},
    sexpr::{self, Sexpr},
};
use anyhow::{Context, Result, bail};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
};

pub struct Repair {
    pub path: PathBuf,
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
    before: String,
    after: String,
    models: Vec<PathBuf>,
    root: Option<LibraryRoot>,
}

impl Repair {
    /// Refuse stale previews or an open KiCad. Keep a unique backup before replacing the file.
    pub fn apply(&self) -> Result<PathBuf> {
        if kicad_env::kicad_running_now() {
            bail!("close KiCad before repairing 3D links, then reopen the project afterwards");
        }
        if std::fs::read_to_string(&self.path)? != self.before {
            bail!("{} changed since the preview; scan again", self.path.display());
        }
        if self.models.iter().any(|p| !p.is_file()) {
            bail!("a replacement model is no longer available; scan again");
        }
        if let Some(root) = &self.root {
            crate::snapshot::ensure(root, "repair 3D links")?;
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let backup = self.path.with_file_name(format!("{}.klm-3d-{stamp}.bak", self.path.file_name().unwrap_or_default().to_string_lossy()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&backup)?;
        file.write_all(self.before.as_bytes())?;
        file.sync_all()?;
        kicad_env::write_atomic(&self.path, &self.after)?;
        Ok(backup)
    }
}

fn resolve(uri: &str, root: &LibraryRoot) -> Option<PathBuf> {
    let mut path = uri.replace("${KIPRJMOD}", &root.table_dir.to_string_lossy()).replace("${KLM_LIB}", &root.dir.to_string_lossy());
    let vars = crate::cleanup::user_vars(&root.table_dir);
    // A bounded expansion also handles cycles in user-defined variables.
    for _ in 0..16 {
        let Some(start) = path.find("${") else { break };
        let end = start + path[start..].find('}')?;
        let var = &path[start + 2..end];
        let value =
            std::env::var(var).ok().or_else(|| vars.get(var).cloned()).or_else(|| kicad_env::default_var_dir(var).map(|p| p.to_string_lossy().into_owned()))?;
        path.replace_range(start..=end, &value);
    }
    if path.contains("${") || path.starts_with("http") || path.starts_with("kicad-embed:") {
        return None;
    }
    let path = PathBuf::from(path.replace('\\', "/"));
    Some(if path.is_absolute() { path } else { root.table_dir.join(path) })
}

/// Existing files only, matched by the exact filename. Ambiguous matches are left for the user.
fn model_path(uri: &str, root: &LibraryRoot, models: &[PathBuf]) -> Option<PathBuf> {
    if let Some(path) = resolve(uri, root).filter(|p| p.is_file()) {
        return Some(path);
    }
    let normalized = uri.replace('\\', "/");
    let name = normalized.rsplit('/').next()?;
    let mut matches = models.iter().filter(|p| p.file_name().is_some_and(|n| n == name));
    let found = matches.next()?;
    matches.next().is_none().then(|| found.clone())
}

fn files(dir: &Path, extension: Option<&str>) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() && extension.is_none_or(|e| path.extension().is_some_and(|x| x == e)) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn repair(path: PathBuf, root: Option<LibraryRoot>) -> Result<Repair> {
    let before = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    sexpr::parse(&before).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Repair { path, after: before.clone(), before, changes: vec![], warnings: vec![], models: vec![], root })
}

fn restored_models(source: &Sexpr, root: &LibraryRoot, models: &[PathBuf]) -> Option<(Vec<Sexpr>, Vec<PathBuf>)> {
    let mut restored = vec![];
    let mut paths = vec![];
    for model in source.children("model") {
        let path = model_path(model.arg(0)?, root, models)?;
        let mut model = model.clone();
        model.set_arg(0, Sexpr::str(&path.to_string_lossy().replace('\\', "/")));
        restored.push(model);
        paths.push(path);
    }
    (!restored.is_empty()).then_some((restored, paths))
}

fn model_text(models: &[Sexpr], indent: &str) -> String {
    models.iter().map(|m| sexpr::to_string(m).trim_end().replace('\n', &format!("\n{indent}"))).collect::<Vec<_>>().join(&format!("\n{indent}"))
}

/// Read-only preview. Boards in the project folder use exact KLM/PRJ library IDs, never fuzzy matches.
pub fn plan(roots: &[LibraryRoot], project: Option<&Path>) -> Result<Vec<Repair>> {
    let mut ordered = roots.to_vec();
    ordered.sort_by_key(|r| r.scope == Scope::Project);
    let roots = &ordered;
    let mut repairs = vec![];
    let mut footprints: HashMap<String, (Sexpr, usize, PathBuf)> = HashMap::new();
    let table = project.map(|p| p.join("fp-lib-table")).filter(|p| p.exists()).map(|p| sexpr::parse(&std::fs::read_to_string(p)?)).transpose()?;
    let mut model_lists = vec![];
    for (index, root) in roots.iter().enumerate() {
        let mut models = vec![];
        if root.dir.join("3dmodels").is_dir() {
            for entry in std::fs::read_dir(root.dir.join("3dmodels"))? {
                let dir = entry?.path();
                if dir.is_dir() {
                    models.extend(files(&dir, None)?);
                }
            }
        }
        model_lists.push(models);
        let models = &model_lists[index];
        let dir = root.dir.join("footprints");
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(dir)? {
            let pretty = entry?.path();
            let Some(cat) = pretty.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".pretty")) else { continue };
            let mut nicks = vec![root.nickname(cat)];
            if root.scope == Scope::Project
                && let Some(table) = &table
            {
                for lib in table.children("lib") {
                    let path = lib.child("uri").and_then(|u| u.arg(0)).and_then(|u| resolve(u, root));
                    if path.and_then(|p| p.canonicalize().ok()) == pretty.canonicalize().ok()
                        && let Some(nick) = lib.child("name").and_then(|n| n.arg(0))
                    {
                        nicks.push(nick.to_string());
                    }
                }
            }
            for path in files(&pretty, Some("kicad_mod"))? {
                let mut fix = repair(path, Some(root.clone()))?;
                let mut replacements = vec![];
                for kid in kids(fix.before.trim_start())?.into_iter().filter(|k| k.key == "model") {
                    let leading = fix.before.len() - fix.before.trim_start().len();
                    let range = kid.start + leading..kid.end + leading;
                    let mut model = sexpr::parse(&fix.before[range.clone()])?;
                    let Some(uri) = model.arg(0) else { continue };
                    if let Some(path) = model_path(uri, root, models) {
                        let current = resolve(uri, root);
                        if current.as_ref().is_none_or(|p| !p.is_file()) {
                            let new = path.to_string_lossy().replace('\\', "/");
                            fix.changes.push(format!("3D path {uri} → {new}"));
                            model.set_arg(0, Sexpr::str(&new));
                            replacements.push((range, sexpr::to_string(&model).trim_end().to_string()));
                            fix.models.push(path);
                        }
                    } else {
                        fix.warnings.push(format!("Cannot uniquely locate model {uri}"));
                    }
                }
                for (range, new) in replacements.into_iter().rev() {
                    fix.after.replace_range(range, &new);
                }
                let name = fix.path.file_stem().unwrap_or_default().to_string_lossy();
                let mut source = sexpr::parse(&fix.after)?;
                if source.child("model").is_none() && root.scope == Scope::Project {
                    for nick in &nicks {
                        if let Some((global, source_index, _)) = footprints.get(&format!("{nick}:{name}")) {
                            if roots[*source_index].scope != Scope::Global {
                                continue;
                            }
                            if let Some((restored, paths)) = restored_models(global, &roots[*source_index], &model_lists[*source_index]) {
                                let end = fix.after.rfind(')').context("footprint has no closing parenthesis")?;
                                fix.after.insert_str(end, &format!("\n\t{}\n", model_text(&restored, "\t")));
                                fix.changes.push(format!("restore {} 3D model(s) from {nick}:{name}", restored.len()));
                                fix.models.extend(paths);
                                source = sexpr::parse(&fix.after)?;
                            } else if global.child("model").is_some() {
                                fix.warnings.push(format!("Cannot restore models from {nick}:{name}: missing or ambiguous model files"));
                            }
                            break;
                        }
                    }
                }
                for nick in &nicks {
                    footprints.insert(format!("{nick}:{name}"), (source.clone(), index, pretty.clone()));
                }
                if !fix.changes.is_empty() || !fix.warnings.is_empty() {
                    repairs.push(fix);
                }
            }
        }
    }
    if let Some(project) = project {
        for path in files(project, Some("kicad_pcb"))? {
            let mut fix = repair(path, None)?;
            let mut replacements = vec![];
            let leading = fix.before.len() - fix.before.trim_start().len();
            for kid in kids(fix.before.trim_start())?.into_iter().filter(|k| k.key == "footprint") {
                let start = kid.start + leading;
                let end = kid.end + leading;
                let text = &fix.before[start..end];
                let fp = sexpr::parse(text)?;
                let id = fp.arg(0).unwrap_or_default();
                let Some((source, index, pretty)) = footprints.get(id) else { continue };
                let root = &roots[*index];
                let models = &model_lists[*index];
                let reference =
                    property(&fp, "Reference").or_else(|| fp.children("fp_text").find(|t| t.arg(0) == Some("reference")).and_then(|t| t.arg(1))).unwrap_or(id);
                // A project may override a global nickname with a library outside KLM's managed files.
                let nick = id.split_once(':').map(|p| p.0).unwrap_or_default();
                if let Some(entry) = table.as_ref().and_then(|t| t.children("lib").find(|l| l.child("name").and_then(|n| n.arg(0)) == Some(nick))) {
                    let board_root = LibraryRoot { table_dir: project.to_path_buf(), ..root.clone() };
                    let path = entry.child("uri").and_then(|u| u.arg(0)).and_then(|u| resolve(u, &board_root));
                    if path.and_then(|p| p.canonicalize().ok()) != pretty.canonicalize().ok() {
                        fix.warnings.push(format!("{reference}: project overrides library {nick} outside KLM; repair it manually"));
                        continue;
                    }
                }
                if fp.child("model").is_none() {
                    if let Some((restored, paths)) = restored_models(source, root, models) {
                        replacements.push((end - 1..end - 1, format!("\n\t\t{}\n\t", model_text(&restored, "\t\t"))));
                        fix.changes.push(format!("{reference}: restore {} 3D model(s) from {id}", restored.len()));
                        fix.models.extend(paths);
                    } else if source.child("model").is_some() {
                        fix.warnings.push(format!("{reference}: library models are missing or ambiguous"));
                    }
                } else {
                    for model in kids(text)?.into_iter().filter(|k| k.key == "model") {
                        let mut value = sexpr::parse(&text[model.start..model.end])?;
                        let Some(uri) = value.arg(0) else { continue };
                        // Board-relative paths belong to this project, even for a global footprint.
                        let board_root = LibraryRoot { table_dir: project.to_path_buf(), ..root.clone() };
                        if resolve(uri, &board_root).is_some_and(|p| p.is_file()) || uri.starts_with("kicad-embed:") {
                            continue;
                        }
                        if let Some(path) = model_path(uri, &board_root, models) {
                            value.set_arg(0, Sexpr::str(&path.to_string_lossy().replace('\\', "/")));
                            replacements.push((start + model.start..start + model.end, sexpr::to_string(&value).trim_end().to_string()));
                            fix.changes.push(format!("{reference}: repair 3D model path"));
                            fix.models.push(path);
                        } else {
                            fix.warnings.push(format!("{reference}: cannot uniquely locate model {uri}"));
                        }
                    }
                }
            }
            for (range, new) in replacements.into_iter().rev() {
                fix.after.replace_range(range, &new);
            }
            if !fix.changes.is_empty() || !fix.warnings.is_empty() {
                repairs.push(fix);
            }
        }
    }
    Ok(repairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> (PathBuf, LibraryRoot, PathBuf) {
        let dir = crate::test_dir(name);
        let root = LibraryRoot::global(dir.join("global"), dir.join("config"));
        root.ensure_layout().unwrap();
        std::fs::create_dir_all(root.dir.join("footprints/ICs.pretty")).unwrap();
        std::fs::create_dir_all(root.dir.join("3dmodels/ICs.3dshapes")).unwrap();
        let model = root.dir.join("3dmodels/ICs.3dshapes/part.step");
        std::fs::write(&model, "STEP fixture").unwrap();
        let fp = root.dir.join("footprints/ICs.pretty/PART.kicad_mod");
        let uri = model.to_string_lossy().replace('\\', "/");
        std::fs::write(
            &fp,
            format!(r#"(footprint "PART" (pad "1" smd rect (at 2 3)) (model "{uri}" (offset (xyz 1 2 3)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 90))))"#),
        )
        .unwrap();
        (dir, root, model)
    }

    fn without_models(mut board: Sexpr) -> Sexpr {
        for fp in board.items_mut().iter_mut().filter(|n| n.head() == Some("footprint")) {
            fp.items_mut().retain(|n| n.head() != Some("model"));
        }
        board
    }

    #[test]
    fn restores_only_model_nodes_and_preserves_existing_assignments() {
        let (dir, root, model) = setup("model_links_restore");
        let board = dir.join("test.kicad_pcb");
        let good_uri = model.to_string_lossy().replace('\\', "/");
        let before = format!(
            r#"(kicad_pcb (version 20260101)
 (footprint "KLM_ICs:PART" (at 10 20 45) (layer "B.Cu") (property "Reference" "U1") (pad "1" smd rect (at 9 8) (net 2 "GND")))
 (footprint "KLM_ICs:PART" (property "Reference" "U2") (model "{good_uri}" (offset (xyz 7 8 9))))
 (footprint "External:PART" (property "Reference" "U3"))
 (segment (start 1 2) (end 3 4) (width 0.2) (net 2)))
"#
        );
        std::fs::write(&board, &before).unwrap();
        let fixes = plan(std::slice::from_ref(&root), Some(&dir)).unwrap();
        assert_eq!(std::fs::read_to_string(&board).unwrap(), before, "preview must write nothing");
        assert_eq!(fixes.len(), 1);
        let fix = &fixes[0];
        assert_eq!(fix.changes.len(), 1);
        assert!(fix.changes[0].starts_with("U1:"));
        let original = sexpr::parse(&before).unwrap();
        let after = sexpr::parse(&fix.after).unwrap();
        assert_eq!(without_models(original.clone()), without_models(after.clone()));
        let fps: Vec<_> = after.children("footprint").collect();
        let source = sexpr::parse(&std::fs::read_to_string(root.dir.join("footprints/ICs.pretty/PART.kicad_mod")).unwrap()).unwrap();
        assert_eq!(fps[0].child("model"), source.child("model"));
        assert_eq!(fps[1], original.children("footprint").nth(1).unwrap());
        assert!(fps[2].child("model").is_none());
        // Inserting at the closing parenthesis leaves every original byte on either side intact.
        let at = before.find("\n (footprint \"KLM_ICs:PART\" (property").unwrap() - 1;
        assert!(fix.after.starts_with(&before[..at]));
        assert!(fix.after.ends_with(&before[at..]));
        let backup = fix.apply().unwrap();
        assert_eq!(std::fs::read_to_string(backup).unwrap(), before);
        assert!(plan(&[root], Some(&dir)).unwrap().is_empty(), "repair is idempotent");
    }

    #[test]
    fn fixes_stale_paths_keeps_transforms_and_refuses_stale_preview() {
        let (dir, root, _) = setup("model_links_stale");
        let fp = root.dir.join("footprints/ICs.pretty/PART.kicad_mod");
        let stale = r#"(footprint "PART" (model "Z:/old/part.step" (offset (xyz 7 8 9)) (rotate (xyz 10 20 30))))"#;
        std::fs::write(&fp, stale).unwrap();
        let board = dir.join("test.kicad_pcb");
        std::fs::write(&board, r#"(kicad_pcb (footprint "KLM_ICs:PART" (property "Reference" "U1") (model "Z:/old/part.step" (offset (xyz 4 5 6)))))"#)
            .unwrap();
        let fixes = plan(&[root], Some(&dir)).unwrap();
        assert_eq!(fixes.len(), 2);
        assert!(fixes[0].after.contains("(xyz 7 8 9)"));
        assert!(fixes[1].after.contains("(xyz 4 5 6)"));
        std::fs::write(&board, "edited after preview").unwrap();
        assert!(fixes[1].apply().unwrap_err().to_string().contains("changed since the preview"));
        assert_eq!(std::fs::read_to_string(&fp).unwrap(), stale);
    }

    #[test]
    fn ambiguous_or_missing_models_are_reported_and_never_guessed() {
        let (dir, root, _) = setup("model_links_ambiguous");
        std::fs::create_dir_all(root.dir.join("3dmodels/Other.3dshapes")).unwrap();
        std::fs::write(root.dir.join("3dmodels/Other.3dshapes/part.step"), "another model").unwrap();
        let fp = root.dir.join("footprints/ICs.pretty/PART.kicad_mod");
        std::fs::write(&fp, r#"(footprint "PART" (model "Z:/missing/part.step"))"#).unwrap();
        let board = dir.join("test.kicad_pcb");
        std::fs::write(&board, r#"(kicad_pcb (footprint "KLM_ICs:PART"))"#).unwrap();
        let fixes = plan(&[root], Some(&dir)).unwrap();
        assert_eq!(fixes.len(), 2);
        assert!(fixes.iter().all(|f| f.changes.is_empty() && !f.warnings.is_empty() && f.before == f.after));
    }

    #[test]
    fn restores_extracted_project_library_before_the_board_and_respects_overrides() {
        let (dir, global, _) = setup("model_links_project_copy");
        let project = LibraryRoot::project(&dir);
        let pretty = project.dir.join("footprints/KLM_ICs.pretty");
        std::fs::create_dir_all(&pretty).unwrap();
        let local = pretty.join("PART.kicad_mod");
        let before = r#"(footprint "PART" (pad "1" smd rect (at 4 5)))"#;
        std::fs::write(&local, before).unwrap();
        std::fs::write(dir.join("test.kicad_pcb"), r#"(kicad_pcb (footprint "KLM_ICs:PART" (property "Reference" "U1")))"#).unwrap();
        std::fs::write(dir.join("fp-lib-table"), r#"(fp_lib_table (lib (name "KLM_ICs") (uri "${KIPRJMOD}/lib/footprints/KLM_ICs.pretty")))"#).unwrap();
        let roots = [project, global];
        let fixes = plan(&roots, Some(&dir)).unwrap();
        assert_eq!(fixes.len(), 2);
        assert_eq!(fixes[0].path, local);
        assert_eq!(std::fs::read_to_string(&local).unwrap(), before);
        assert!(fixes.iter().all(|f| f.changes.len() == 1 && f.warnings.is_empty()));
        let mut after = sexpr::parse(&fixes[0].after).unwrap();
        after.items_mut().retain(|n| n.head() != Some("model"));
        assert_eq!(after, sexpr::parse(before).unwrap());
        // An override outside the managed project library must not use the global namesake.
        std::fs::write(dir.join("fp-lib-table"), r#"(fp_lib_table (lib (name "KLM_ICs") (uri "${KIPRJMOD}/other.pretty")))"#).unwrap();
        let fixes = plan(&roots, Some(&dir)).unwrap();
        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].changes.is_empty());
        assert!(fixes[0].warnings[0].contains("outside KLM"));
    }
}
