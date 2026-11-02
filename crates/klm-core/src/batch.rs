//! Batch operations on libraries, in two steps: [`plan`] works out exactly what a batch would change
//! without writing to any library, and [`apply`] then does it, carrying on past items that fail.

use crate::library::{AttachKind, Check, LibraryRoot, Scope, sanitize};
use crate::part::Part;
use crate::sources::zip_import::read_zip;
use crate::{categories, kicad_env};
use anyhow::{Result, bail};
use std::fmt;
use std::path::{Path, PathBuf};

/// Which libraries an import goes into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Global,
    Project,
    Both,
}

pub enum Op {
    /// Import a vendor ZIP. No category means the one guessed from the part's own hint.
    ImportZip { zip: PathBuf, category: Option<String>, target: Target, overwrite: bool },
    /// Import a part whose ZIP was already read (the GUI's pending list).
    ImportPart { part: Part, category: Option<String>, target: Target, overwrite: bool },
    /// Add one datasheet, SPICE or 3D model file to many parts of the `scope` library, as (category, name).
    Attach { scope: Scope, file: PathBuf, parts: Vec<(String, String)> },
    /// Copy global parts, as (category, name), into the project library.
    CopyToProject { parts: Vec<(String, String)>, overwrite: bool },
    /// Re-add libraries missing from KiCad's tables and fix old paths.
    RelinkAll,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Add(String),
    Replace(String),
    /// A symbol field that gets a new value.
    Field(String),
    /// A library that KiCad's tables don't list yet.
    NewLibrary(String),
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Change::Add(s) => write!(f, "add {s}"),
            Change::Replace(s) => write!(f, "replace {s}"),
            Change::Field(s) => write!(f, "set {s}"),
            Change::NewLibrary(n) => write!(f, "add library {n} to KiCad's tables"),
        }
    }
}

enum Work {
    /// Nothing to do; the item failed while planning.
    Nothing,
    Import { part: Part, category: String, targets: Vec<LibraryRoot>, overwrite: bool },
    Attach { root: LibraryRoot, category: String, name: String, kind: AttachKind, file: PathBuf },
    Copy { from: LibraryRoot, to: LibraryRoot, category: String, name: String, overwrite: bool },
    FixPaths(LibraryRoot),
    Link(LibraryRoot, String),
}

/// What happens to one part (or one library, for a relink).
pub struct Item {
    pub label: String,
    pub changes: Vec<Change>,
    /// Things that will probably make it fail, e.g. "already exists".
    pub warnings: Vec<String>,
    /// Set when the item can't be done at all; `apply` reports it as failed.
    pub error: Option<String>,
    work: Work,
}

impl Item {
    fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), changes: vec![], warnings: vec![], error: None, work: Work::Nothing }
    }

    fn fail(mut self, msg: impl Into<String>) -> Self {
        self.error = Some(msg.into());
        self
    }

    fn run(&self, notes: &mut Vec<String>) -> Result<()> {
        match &self.work {
            Work::Nothing => bail!("nothing to do"),
            Work::Import { part, category, targets, overwrite } => {
                let mut errors = vec![];
                for root in targets {
                    let nick = root.nickname(category);
                    match root.add_part(part, category, *overwrite) {
                        Ok(new_lib) => {
                            notes.push(format!("Imported {} into {nick}", part.mpn));
                            if new_lib {
                                notes.push(format!("Added library {nick} to KiCad"));
                            }
                        }
                        Err(e) => errors.push(format!("{nick}: {e:#}")),
                    }
                }
                if !errors.is_empty() {
                    bail!("{}", errors.join("; "));
                }
            }
            Work::Attach { root, category, name, kind, file } => {
                root.attach_file(category, name, *kind, file)?;
                notes.push(format!("Added {} to {name} in {}", file.file_name().unwrap_or_default().to_string_lossy(), root.nickname(category)));
            }
            Work::Copy { from, to, category, name, overwrite } => {
                let new_lib = from.copy_part_to(category, name, to, *overwrite)?;
                notes.push(format!("Copied {name} into {}", to.nickname(category)));
                if new_lib {
                    notes.push(format!("Added library {} to KiCad", to.nickname(category)));
                }
            }
            Work::FixPaths(root) => notes.push(format!("Replaced ${{KLM_LIB}} with {} in {} files", root.dir.display(), root.fix_old_paths()?)),
            Work::Link(root, category) => {
                root.link(category)?;
                notes.push(format!("Re-linked {} in KiCad", root.nickname(category)));
            }
        }
        Ok(())
    }
}

/// Everything a batch would do. Holds the files read from ZIPs while planning; they're deleted on drop.
pub struct Plan {
    pub items: Vec<Item>,
    pub warnings: Vec<String>,
    scratch: PathBuf,
    /// Libraries the batch may change; each gets one snapshot for the whole batch.
    roots: Vec<LibraryRoot>,
}

impl Plan {
    /// Whether applying this edits KiCad's library tables, which KiCad overwrites while it is open.
    pub fn touches_kicad_tables(&self) -> bool {
        self.items.iter().any(|i| matches!(i.work, Work::FixPaths(_)) || i.changes.iter().any(|c| matches!(c, Change::NewLibrary(_))))
    }
}

impl Drop for Plan {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.items.is_empty() {
            writeln!(f, "Nothing to do.")?;
        }
        for item in &self.items {
            writeln!(f, "{}", item.label)?;
            for c in &item.changes {
                writeln!(f, "  {c}")?;
            }
            for w in &item.warnings {
                writeln!(f, "  warning: {w}")?;
            }
            if let Some(e) = &item.error {
                writeln!(f, "  cannot be done: {e}")?;
            }
        }
        for w in &self.warnings {
            writeln!(f, "warning: {w}")?;
        }
        Ok(())
    }
}

fn act(exists: bool, what: String) -> Change {
    if exists { Change::Replace(what) } else { Change::Add(what) }
}

/// Works out what `ops` would do to the libraries, without writing to them. ZIPs are unpacked into a
/// scratch folder that lives as long as the returned plan.
pub fn plan(ops: Vec<Op>, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> Plan {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let roots = global.into_iter().chain(project).cloned().collect();
    let mut plan = Plan { items: vec![], warnings: vec![], scratch: std::env::temp_dir().join(format!("klm-batch-{}-{nanos}", std::process::id())), roots };
    for op in ops {
        match op {
            Op::ImportZip { zip, category, target, overwrite } => {
                let name = zip.file_name().unwrap_or_default().to_string_lossy().into_owned();
                // One item per import op, so the scratch folder can be named after its position.
                let item = match read_zip(&zip, &plan.scratch.join(plan.items.len().to_string())) {
                    Ok(Some(part)) => import_item(part, category, target, overwrite, global, project),
                    Ok(None) => Item::new(name).fail("holds no KiCad symbol"),
                    Err(e) => Item::new(name).fail(format!("{e:#}")),
                };
                plan.items.push(item);
            }
            Op::ImportPart { part, category, target, overwrite } => plan.items.push(import_item(part, category, target, overwrite, global, project)),
            Op::Attach { scope, file, parts } => {
                let root = if scope == Scope::Global { global } else { project };
                plan.items.extend(attach_items(root, &file, parts));
            }
            Op::CopyToProject { parts, overwrite } => plan.items.extend(copy_items(global, project, parts, overwrite)),
            Op::RelinkAll => plan.items.extend(relink_items(global.into_iter().chain(project))),
        }
    }
    if plan.touches_kicad_tables() && kicad_env::kicad_running() {
        plan.warnings.push("KiCad is running; close it before applying, since it would overwrite the changes to its library list".into());
    }
    plan
}

fn import_item(part: Part, category: Option<String>, target: Target, overwrite: bool, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> Item {
    let category = category.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).unwrap_or_else(|| categories::guess(&part.category_hint).to_string());
    let (cat, mpn) = (sanitize(&category), sanitize(&part.mpn));
    let mut item = Item::new(part.mpn.clone());
    if cat.is_empty() || mpn.is_empty() {
        return item.fail("category and MPN must not be empty");
    }
    let mut targets = vec![];
    for (wanted, root, what) in [(target != Target::Project, global, "global"), (target != Target::Global, project, "project")] {
        match (wanted, root) {
            (true, Some(r)) => targets.push(r.clone()),
            (true, None) => item.error = Some(format!("no {what} library to import into")),
            _ => {}
        }
    }
    for root in &targets {
        let (nick, has) = (root.nickname(&cat), root.has_part(&cat, &mpn));
        if has && !overwrite {
            item.warnings.push(format!("{mpn} already exists in {nick}"));
        }
        let mut add = |what: &str| item.changes.push(act(has, format!("{what} in {nick}")));
        add(&format!("symbol {mpn}"));
        match part.footprint_files.len() {
            0 => {}
            1 => add("footprint"),
            n => add(&format!("{n} footprints")),
        }
        if !part.model_files.is_empty() {
            add("3D model");
        }
        if part.datasheet_file.is_some() {
            add("datasheet");
        }
        if !part.sim_files.is_empty() {
            add("simulation model");
        }
        if part.datasheet_file.is_none() && !part.datasheet_url.is_empty() {
            item.changes.push(Change::Field(format!("Datasheet link in {nick}")));
        }
        if root.needs_link(&cat) {
            item.changes.push(Change::NewLibrary(nick));
        }
    }
    item.work = Work::Import { part, category: cat, targets, overwrite };
    item
}

fn attach_items(root: Option<&LibraryRoot>, file: &Path, parts: Vec<(String, String)>) -> Vec<Item> {
    let file_name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let status = root.map(LibraryRoot::list_parts).unwrap_or_default();
    parts
        .into_iter()
        .map(|(cat, name)| {
            let item = Item::new(format!("{cat}/{name}"));
            let Some(root) = root else { return item.fail("no library to add the file to") };
            let Some(kind) = AttachKind::of(file) else { return item.fail(format!("don't know what kind of file {file_name} is")) };
            let nick = root.nickname(&cat);
            let Some(st) = status.iter().find(|s| s.category == cat && s.name == name) else {
                return item.fail(format!("{name} is not in {nick}"));
            };
            let (what, have, field) = match kind {
                AttachKind::Datasheet => ("datasheet", st.datasheet, "Datasheet to the local copy"),
                AttachKind::Sim => ("simulation model", st.sim, "Sim.Library to the new file"),
                AttachKind::Model if st.footprint != Check::Ok => {
                    return item.fail(format!("{name} has no footprint in {nick}, so there is nothing to put the 3D model on"));
                }
                AttachKind::Model => ("3D model", st.model, "the footprint's model path to the new file"),
            };
            let mut item = item;
            item.changes = vec![act(have == Check::Ok, format!("{what} {file_name} in {nick}")), Change::Field(field.into())];
            item.work = Work::Attach { root: root.clone(), category: cat, name, kind, file: file.to_path_buf() };
            item
        })
        .collect()
}

fn copy_items(global: Option<&LibraryRoot>, project: Option<&LibraryRoot>, parts: Vec<(String, String)>, overwrite: bool) -> Vec<Item> {
    let status = global.map(LibraryRoot::list_parts).unwrap_or_default();
    parts
        .into_iter()
        .map(|(cat, name)| {
            let mut item = Item::new(format!("{cat}/{name}"));
            let (Some(from), Some(to)) = (global, project) else { return item.fail("needs both a global and a project library") };
            let Some(st) = status.iter().find(|s| s.category == cat && s.name == name) else {
                return item.fail(format!("{name} is not in {}", from.nickname(&cat)));
            };
            let (nick, has) = (to.nickname(&cat), to.has_part(&cat, &name));
            if has && !overwrite {
                item.warnings.push(format!("{name} already exists in {nick}"));
            }
            let mut add = |what: &str| item.changes.push(act(has, format!("{what} in {nick}")));
            add(&format!("symbol {name}"));
            for (what, check) in [("footprint", st.footprint), ("3D model", st.model), ("datasheet", st.datasheet), ("simulation model", st.sim)] {
                if check == Check::Ok {
                    add(what);
                }
            }
            item.changes.push(Change::Field("Footprint and file paths in the copy to point into the project".into()));
            if to.needs_link(&cat) {
                item.changes.push(Change::NewLibrary(nick));
            }
            item.work = Work::Copy { from: from.clone(), to: to.clone(), category: cat, name, overwrite };
            item
        })
        .collect()
}

fn relink_items<'a>(roots: impl Iterator<Item = &'a LibraryRoot>) -> Vec<Item> {
    let mut items = vec![];
    for root in roots {
        if root.has_old_paths() {
            let mut item = Item::new(format!("Old ${{KLM_LIB}} paths in {}", root.dir.display()));
            item.changes.push(Change::Field("old ${KLM_LIB} paths in its files and KiCad's tables to the library's real path".into()));
            item.work = Work::FixPaths(root.clone());
            items.push(item);
        }
        for cat in root.unlinked() {
            let nick = root.nickname(&cat);
            let mut item = Item::new(nick.clone());
            item.changes.push(Change::NewLibrary(nick));
            item.work = Work::Link(root.clone(), cat);
            items.push(item);
        }
    }
    items
}

pub struct ItemResult {
    pub label: String,
    /// What was done, in the words the GUI logs.
    pub notes: Vec<String>,
    pub error: Option<String>,
}

/// Does the plan, one item at a time, calling `progress(done, total)` before each. A failed item is
/// reported in its result and the rest still run.
pub fn apply(plan: &Plan, mut progress: impl FnMut(usize, usize)) -> Vec<ItemResult> {
    let total = plan.items.len();
    // One snapshot per library for the whole batch; without it there'd be no way back, so don't start.
    // A library that doesn't exist yet is skipped so a global-only batch doesn't create the project's;
    // if the batch does create it, each change there is snapshotted on its own.
    let label = format!("batch of {total}");
    let guards: anyhow::Result<Vec<_>> = plan.roots.iter().filter(|r| r.dir.is_dir()).map(|r| crate::snapshot::guard(r, &label)).collect();
    let _guards = match guards {
        Ok(g) => g,
        Err(e) => {
            return vec![ItemResult { label, notes: vec![], error: Some(format!("could not take a snapshot first, so nothing was changed: {e:#}")) }];
        }
    };
    let results = plan
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            progress(i, total);
            let mut notes = vec![];
            let outcome = match &item.error {
                Some(e) => Err(anyhow::anyhow!("{e}")),
                None => item.run(&mut notes),
            };
            ItemResult { label: item.label.clone(), notes, error: outcome.err().map(|e| format!("{e:#}")) }
        })
        .collect();
    progress(total, total);
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn symbol(name: &str) -> String {
        format!(
            r#"(symbol "{name}" (in_bom yes) (on_board yes)
  (property "Reference" "U" (at 0 0 0))
  (property "Value" "{name}" (at 0 0 0))
  (property "Footprint" "KLM_ICs:FP_{name}" (at 0 0 0))
  (property "Datasheet" "" (at 0 0 0))
  (property "MPN" "{name}" (at 0 0 0)))"#
        )
    }

    /// A global library with the given parts in "ICs", linked in a KiCad config dir beside it, plus an empty project.
    fn setup(name: &str, parts: &[&str]) -> (PathBuf, LibraryRoot, LibraryRoot) {
        let base = crate::test_dir(name);
        std::fs::create_dir_all(base.join("config")).unwrap();
        let global = LibraryRoot::global(base.join("global"), base.join("config"));
        global.ensure_layout().unwrap();
        let symbols: String = parts.iter().map(|p| symbol(p)).collect();
        std::fs::write(global.dir.join("symbols/ICs.kicad_sym"), format!("(kicad_symbol_lib (version 20241209) (generator test)\n{symbols})")).unwrap();
        std::fs::create_dir_all(global.dir.join("footprints/ICs.pretty")).unwrap();
        for p in parts {
            std::fs::write(global.dir.join(format!("footprints/ICs.pretty/FP_{p}.kicad_mod")), format!("(footprint \"FP_{p}\" (layer \"F.Cu\"))")).unwrap();
        }
        global.link("ICs").unwrap();
        std::fs::create_dir_all(base.join("project")).unwrap();
        let project = LibraryRoot::project(&base.join("project"));
        (base, global, project)
    }

    fn make_zip(path: &Path) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        z.start_file("zipped.kicad_sym", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(format!("(kicad_symbol_lib (version 20241209) {})", symbol("ZIPPED")).as_bytes()).unwrap();
        z.finish().unwrap();
    }

    /// Every file under `dir` with its contents.
    fn listing(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = vec![];
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                out.extend(listing(&e.path()));
            } else {
                out.push((e.path(), std::fs::read(e.path()).unwrap()));
            }
        }
        out.sort();
        out
    }

    fn pairs(names: &[&str]) -> Vec<(String, String)> {
        names.iter().map(|n| ("ICs".to_string(), n.to_string())).collect()
    }

    #[test]
    fn plan_writes_nothing_and_lists_the_changes() {
        let (base, global, project) = setup("batch_plan", &["A", "B"]);
        let (pdf, zip) = (base.join("ds.pdf"), base.join("z.zip"));
        std::fs::write(&pdf, "%PDF").unwrap();
        make_zip(&zip);
        // Leave the global library out of KiCad's tables so the relink has something to do.
        std::fs::write(base.join("config/sym-lib-table"), "(sym_lib_table\n\t(version 7)\n)\n").unwrap();
        let before = listing(&base);

        let ops = vec![
            Op::CopyToProject { parts: pairs(&["A", "B"]), overwrite: false },
            Op::Attach { scope: Scope::Global, file: pdf, parts: pairs(&["A"]) },
            Op::ImportZip { zip, category: Some("ICs".into()), target: Target::Both, overwrite: false },
            Op::RelinkAll,
        ];
        let plan = plan(ops, Some(&global), Some(&project));
        assert_eq!(listing(&base), before, "planning must not touch the libraries");

        assert_eq!(plan.items.len(), 5);
        assert_eq!(plan.items[0].changes[0], Change::Add("symbol A in PRJ_ICs".into()));
        assert!(plan.items[0].changes.contains(&Change::NewLibrary("PRJ_ICs".into())));
        assert_eq!(plan.items[2].changes[0], Change::Add("datasheet ds.pdf in KLM_ICs".into()));
        assert_eq!(plan.items[3].label, "ZIPPED");
        assert!(plan.items[3].changes.contains(&Change::Add("symbol ZIPPED in PRJ_ICs".into())));
        assert_eq!(plan.items[4].changes, [Change::NewLibrary("KLM_ICs".into())]);
        assert!(plan.touches_kicad_tables());
        assert!(plan.to_string().contains("add library PRJ_ICs to KiCad's tables"));
    }

    #[test]
    fn apply_does_what_the_plan_said() {
        let (base, global, project) = setup("batch_apply", &["A", "B", "C"]);
        let pdf = base.join("ds.pdf");
        std::fs::write(&pdf, "%PDF").unwrap();
        std::fs::write(base.join("config/sym-lib-table"), "(sym_lib_table\n\t(version 7)\n)\n").unwrap();
        let ops = vec![
            Op::CopyToProject { parts: pairs(&["A", "B"]), overwrite: false },
            Op::Attach { scope: Scope::Global, file: pdf, parts: pairs(&["A", "C"]) },
            Op::RelinkAll,
        ];
        let plan = plan(ops, Some(&global), Some(&project));
        let snapshots_before = crate::snapshot::list(&global).len();
        let mut steps = vec![];
        let results = apply(&plan, |done, total| steps.push((done, total)));
        assert_eq!(crate::snapshot::list(&global).len(), snapshots_before + 1, "the whole batch is one snapshot of the global library");

        assert!(results.iter().all(|r| r.error.is_none()), "{:?}", results.iter().map(|r| &r.error).collect::<Vec<_>>());
        assert_eq!(results.iter().map(|r| &r.label).collect::<Vec<_>>(), plan.items.iter().map(|i| &i.label).collect::<Vec<_>>());
        assert_eq!(steps.first(), Some(&(0, 5)));
        assert_eq!(steps.last(), Some(&(5, 5)));
        assert!(project.has_part("ICs", "A") && project.has_part("ICs", "B") && !project.has_part("ICs", "C"));
        assert!(!project.needs_link("ICs") && global.unlinked().is_empty(), "the planned new libraries are in KiCad's tables");
        let ds: Vec<_> = global.list_parts().iter().map(|p| p.datasheet).collect();
        assert_eq!(ds, [Check::Ok, Check::Missing, Check::Ok]);
    }

    #[test]
    fn one_failure_does_not_stop_the_rest() {
        let (base, global, project) = setup("batch_failure", &["A", "B", "C"]);
        global.copy_part_to("ICs", "B", &project, false).unwrap();
        let broken = base.join("broken.zip");
        std::fs::write(&broken, "not a zip").unwrap();
        let ops = vec![
            Op::CopyToProject { parts: pairs(&["A", "NOPE", "B", "C"]), overwrite: false },
            Op::ImportZip { zip: broken, category: None, target: Target::Global, overwrite: false },
        ];
        let plan = plan(ops, Some(&global), Some(&project));
        assert!(plan.items[2].warnings[0].contains("already exists"));
        let results = apply(&plan, |_, _| {});

        let failed: Vec<_> = results.iter().map(|r| r.error.is_some()).collect();
        assert_eq!(failed, [false, true, true, false, true]);
        assert!(results[1].error.as_ref().unwrap().contains("not in KLM_ICs"));
        assert!(project.has_part("ICs", "A") && project.has_part("ICs", "C"));
    }
}
