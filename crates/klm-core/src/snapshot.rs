//! Snapshots of a library taken before it changes, and rollback to one.
//!
//! A snapshot is a folder `<library>/.klm-history/<id>/` (id = milliseconds since 1970, so folders sort by age):
//! - `manifest.json`: label, time, every file and folder the library had, and which KiCad tables existed.
//! - `files/<relative path>`: symbol and footprint files are copied in full; everything else (3D models,
//!   datasheets, simulation, ...) is *hard-linked*, so the snapshot costs no space until the library
//!   replaces or deletes that file. This is safe because every library write replaces files (temp file +
//!   rename, or `copy_file`, which does the same) instead of writing into them.
//!   If linking fails (FAT, network share) the file is copied instead.
//! - `tables/`: copies of KiCad's `sym-lib-table` and `fp-lib-table` for this library.
//!
//! Rollback restores that exact state and first takes a snapshot of the current one, so a rollback
//! can be undone like any other change. Newer snapshots are kept; each is a complete state.
//!
//! Every function that changes a library starts with [`ensure`]. A caller that makes many changes and
//! wants them undone as one (a batch) holds a [`guard`] for the whole run; [`ensure`] then does nothing.

use crate::library::LibraryRoot;
use crate::store::Store;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Folder inside each library root that holds its snapshots.
pub const DIR: &str = ".klm-history";
/// Keep at most this many snapshots per library ...
pub const MAX_SNAPSHOTS: usize = 30;
/// ... and at most this many bytes (text copies plus binaries only the snapshot still holds).
pub const MAX_BYTES: u64 = 500 * 1024 * 1024;

const TABLES: [&str; 2] = ["sym-lib-table", "fp-lib-table"];
const TEXT_EXTS: [&str; 2] = ["kicad_sym", "kicad_mod"];

#[derive(Serialize, Deserialize)]
struct Manifest {
    label: String,
    /// Unix seconds.
    ts: i64,
    /// Paths relative to the library root, `/`-separated.
    files: Vec<String>,
    dirs: Vec<String>,
    /// Which of [`TABLES`] existed.
    tables: Vec<String>,
}

/// One entry of [`list`].
#[derive(Clone, Debug)]
pub struct Info {
    pub id: String,
    pub ts: i64,
    pub label: String,
    /// Disk space this snapshot uses: copied text plus binaries the library no longer has as they were.
    pub bytes: u64,
}

thread_local! {
    /// Libraries this thread holds a [`Guard`] for.
    static HELD: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Where snapshots are also logged (the `history` table); unset in tests and tools that don't want it.
static STORE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_store_path(path: Option<PathBuf>) {
    *STORE.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

/// Takes a snapshot of `root` labelled `label` ("import LM358", "move to Power"), unless this thread holds a
/// [`guard`] for `root`. Call it before a change writes anything, and let its error stop the change.
pub fn ensure(root: &LibraryRoot, label: &str) -> Result<()> {
    if HELD.with(|h| h.borrow().contains(&root.dir)) {
        return Ok(());
    }
    take(root, label)?;
    prune(root)
}

/// Holds one snapshot open for a run of changes: takes it now and makes [`ensure`] on this thread do
/// nothing for `root` until the guard is dropped, so a batch of 50 imports is one snapshot.
/// Guards nest (an inner one on the same root takes nothing). It is per thread: work done on other
/// threads still snapshots on its own, so hold the guard on the thread that calls the library functions.
///
/// ```ignore
/// let _snapshot = snapshot::guard(&root, "batch import (50 parts)")?;
/// for part in parts { root.add_part(&part, "ICs", false)?; }
/// ```
pub fn guard(root: &LibraryRoot, label: &str) -> Result<Guard> {
    ensure(root, label)?;
    HELD.with(|h| h.borrow_mut().push(root.dir.clone()));
    Ok(Guard(root.dir.clone()))
}

#[must_use = "the snapshot guard only lasts as long as this value is kept"]
pub struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            if let Some(i) = h.iter().rposition(|d| *d == self.0) {
                h.remove(i);
            }
        });
    }
}

fn is_text(rel: &str) -> bool {
    Path::new(rel).extension().and_then(|e| e.to_str()).is_some_and(|e| TEXT_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// All files and folders under `root` (not [`DIR`] at the top), `/`-separated and relative.
fn walk(root: &Path) -> Result<(Vec<String>, Vec<String>)> {
    fn go(root: &Path, rel: &str, files: &mut Vec<String>, dirs: &mut Vec<String>) -> std::io::Result<()> {
        for e in std::fs::read_dir(root.join(rel))? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if rel.is_empty() && name == DIR {
                continue;
            }
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if e.file_type()?.is_dir() {
                dirs.push(r.clone());
                go(root, &r, files, dirs)?;
            } else {
                files.push(r);
            }
        }
        Ok(())
    }
    let (mut files, mut dirs) = (vec![], vec![]);
    go(root, "", &mut files, &mut dirs).with_context(|| format!("reading {}", root.display()))?;
    Ok((files, dirs))
}

/// Copies through a temp file and a rename, so `dest` is replaced rather than written into.
fn copy_replace(src: &Path, dest: &Path) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("klm-tmp");
    std::fs::copy(src, &tmp).with_context(|| format!("copying {} to {}", src.display(), tmp.display()))?;
    std::fs::rename(&tmp, dest).with_context(|| format!("replacing {}", dest.display()))
}

fn now() -> std::time::Duration {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default()
}

/// Writes a snapshot without applying the retention limits. Returns the bytes it copied.
fn take(root: &LibraryRoot, label: &str) -> Result<u64> {
    std::fs::create_dir_all(&root.dir)?;
    let hist = root.dir.join(DIR);
    let mut millis = now().as_millis();
    while hist.join(format!("{millis:013}")).exists() {
        millis += 1;
    }
    let (id, tmp) = (format!("{millis:013}"), hist.join(format!("{millis:013}.tmp")));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    let (files, dirs) = walk(&root.dir)?;
    let mut copied = 0;
    let built = (|| -> Result<()> {
        for f in &files {
            let (src, dest) = (root.dir.join(f), tmp.join("files").join(f));
            std::fs::create_dir_all(dest.parent().unwrap_or(&tmp))?;
            if is_text(f) || std::fs::hard_link(&src, &dest).is_err() {
                copied += std::fs::copy(&src, &dest).with_context(|| format!("copying {}", src.display()))?;
            }
        }
        let mut tables = vec![];
        for t in TABLES {
            let src = root.table_dir.join(t);
            if src.is_file() {
                std::fs::create_dir_all(tmp.join("tables"))?;
                copied += std::fs::copy(&src, tmp.join("tables").join(t))?;
                tables.push(t.to_string());
            }
        }
        let manifest = Manifest { label: label.into(), ts: now().as_secs() as i64, files: files.clone(), dirs, tables };
        std::fs::write(tmp.join("manifest.json"), serde_json::to_string(&manifest)?)?;
        std::fs::rename(&tmp, hist.join(&id)).context("finishing the snapshot")
    })();
    if let Err(e) = built {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e.context("could not take a snapshot, so nothing was changed"));
    }
    log_to_store(label, &format!("{} files, {copied} bytes copied, in {}", files.len(), root.dir.display()));
    Ok(copied)
}

fn log_to_store(label: &str, details: &str) {
    let path = STORE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(store) = path.and_then(|p| Store::open(&p).ok()) {
        let _ = store.log(&format!("snapshot: {label}"), details);
    }
}

fn read_manifest(snap: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(snap.join("manifest.json")).with_context(|| format!("no snapshot at {}", snap.display()))?;
    Ok(serde_json::from_str(&text)?)
}

/// The snapshots of `root`, newest first.
pub fn list(root: &LibraryRoot) -> Vec<Info> {
    let hist = root.dir.join(DIR);
    let mut ids: Vec<String> = std::fs::read_dir(&hist).into_iter().flatten().flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| !n.ends_with(".tmp")).collect();
    ids.sort_by(|a, b| b.cmp(a));
    // Newest first, so a binary several snapshots hold is charged to the newest: that is the last to let it go.
    let mut seen = HashSet::new();
    ids.into_iter()
        .filter_map(|id| {
            let m = read_manifest(&hist.join(&id)).ok()?;
            let bytes = snapshot_bytes(root, &hist.join(&id), &mut seen);
            Some(Info { id, ts: m.ts, label: m.label, bytes })
        })
        .collect()
}

/// Space used by a snapshot: its copies, plus linked files that the library has since replaced or deleted.
fn snapshot_bytes(root: &LibraryRoot, snap: &Path, seen: &mut HashSet<(String, u64, Option<SystemTime>)>) -> u64 {
    let Ok((files, _)) = walk(snap) else { return 0 };
    let mut total = 0;
    for f in files {
        let Ok(meta) = std::fs::metadata(snap.join(&f)) else { continue };
        let Some(rel) = f.strip_prefix("files/").filter(|r| !is_text(r)) else {
            total += meta.len();
            continue;
        };
        let live = std::fs::metadata(root.dir.join(rel)).ok();
        let shared = live.is_some_and(|l| l.len() == meta.len() && l.modified().ok() == meta.modified().ok());
        if !shared && seen.insert((rel.to_string(), meta.len(), meta.modified().ok())) {
            total += meta.len();
        }
    }
    total
}

/// Deletes snapshots over [`MAX_SNAPSHOTS`] or [`MAX_BYTES`], oldest first, always keeping the newest.
pub fn prune(root: &LibraryRoot) -> Result<()> {
    prune_to(root, MAX_SNAPSHOTS, MAX_BYTES)
}

fn prune_to(root: &LibraryRoot, max_snapshots: usize, max_bytes: u64) -> Result<()> {
    let hist = root.dir.join(DIR);
    for e in std::fs::read_dir(&hist)?.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")) {
        let _ = std::fs::remove_dir_all(e.path());
    }
    let mut total = 0;
    for (i, s) in list(root).iter().enumerate() {
        total += s.bytes;
        if i >= max_snapshots || (i > 0 && total > max_bytes) {
            std::fs::remove_dir_all(hist.join(&s.id)).with_context(|| format!("deleting snapshot {}", s.id))?;
        }
    }
    Ok(())
}

/// Puts `root` (and KiCad's two tables for it) back exactly as they were when snapshot `id` was taken, which
/// undoes every change made since, newer snapshots' included. KiCad must be closed: the tables are
/// restored whole. The current state is snapshotted first, so this can be rolled back too.
pub fn rollback(root: &LibraryRoot, id: &str) -> Result<()> {
    let snap = root.dir.join(DIR).join(id);
    if id.contains(['/', '\\']) || id.ends_with(".tmp") {
        bail!("{id} is not a snapshot");
    }
    let m = read_manifest(&snap)?;
    take(root, &format!("before rolling back to \"{}\"", m.label))?;

    let want: HashSet<&String> = m.files.iter().collect();
    let (live_files, live_dirs) = walk(&root.dir)?;
    for f in live_files.iter().filter(|f| !want.contains(f)) {
        std::fs::remove_file(root.dir.join(f)).with_context(|| format!("deleting {f}"))?;
    }
    for f in &m.files {
        let (src, dest) = (snap.join("files").join(f), root.dir.join(f));
        let same = match (std::fs::metadata(&src), std::fs::metadata(&dest)) {
            (Ok(s), Ok(d)) if s.len() == d.len() => {
                if is_text(f) { std::fs::read(&src)? == std::fs::read(&dest)? } else { s.modified()? == d.modified()? }
            }
            _ => false,
        };
        if !same {
            copy_replace(&src, &dest).with_context(|| format!("restoring {f}"))?;
        }
    }
    for d in &m.dirs {
        std::fs::create_dir_all(root.dir.join(d))?;
    }
    // Folders made since, deepest first; one that still holds something we don't own stays.
    let keep: HashSet<&String> = m.dirs.iter().collect();
    let mut gone: Vec<&String> = live_dirs.iter().filter(|d| !keep.contains(d)).collect();
    gone.sort_by_key(|d| std::cmp::Reverse(d.len()));
    for d in gone {
        let _ = std::fs::remove_dir(root.dir.join(d));
    }
    for t in TABLES {
        let dest = root.table_dir.join(t);
        if m.tables.iter().any(|x| x == t) {
            copy_replace(&snap.join("tables").join(t), &dest).with_context(|| format!("restoring {t}"))?;
        } else if dest.is_file() {
            std::fs::remove_file(&dest)?;
        }
    }
    prune(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::AttachKind;
    use std::collections::BTreeMap;

    /// Every file under `dir` (and the folders) with its bytes, except the history itself.
    fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        let (files, dirs) = walk(dir).unwrap();
        let mut t: BTreeMap<String, Vec<u8>> = files.into_iter().map(|f| (f.clone(), std::fs::read(dir.join(&f)).unwrap())).collect();
        t.extend(dirs.into_iter().map(|d| (format!("{d}/"), vec![])));
        t
    }

    /// Library `ICs` with two parts (one with a 3D model and datasheet) and KiCad tables listing it.
    fn fixture(name: &str) -> LibraryRoot {
        let dir = crate::test_dir(name);
        let root = LibraryRoot::global(dir.join("lib"), dir.join("config"));
        root.ensure_layout().unwrap();
        std::fs::create_dir_all(&root.table_dir).unwrap();
        let g = root.dir.to_string_lossy().replace('\\', "/");
        let part = |n: &str, fp: &str| {
            format!(
                r#"(symbol "{n}" (property "Value" "{n}" (at 0 0 0)) (property "Footprint" "KLM_ICs:{fp}" (at 0 0 0)) (property "Manufacturer" "ACME" (at 0 0 0)))"#
            )
        };
        std::fs::write(root.dir.join("symbols/ICs.kicad_sym"), format!("(kicad_symbol_lib (version 20251024) {} {})", part("A", "FP1"), part("B", "FP2"))).unwrap();
        let pretty = root.dir.join("footprints/ICs.pretty");
        std::fs::create_dir_all(&pretty).unwrap();
        std::fs::write(pretty.join("FP1.kicad_mod"), r#"(footprint "FP1" (pad "1" smd rect (at 0 0) (size 1 1)))"#).unwrap();
        let fp2 = format!(r#"(footprint "FP2" (pad "1" smd rect (at 0 0) (size 2 2)) (model "{g}/3dmodels/ICs.3dshapes/FP2.step" (offset (xyz 0 0 1))))"#);
        std::fs::write(pretty.join("FP2.kicad_mod"), fp2).unwrap();
        std::fs::create_dir_all(root.dir.join("3dmodels/ICs.3dshapes")).unwrap();
        std::fs::write(root.dir.join("3dmodels/ICs.3dshapes/FP2.step"), "OLD MODEL").unwrap();
        std::fs::write(root.dir.join("datasheets/ACME-B.pdf"), "%PDF old").unwrap();
        root.link("ICs").unwrap();
        std::fs::remove_dir_all(root.dir.join(DIR)).unwrap(); // linking snapshotted; tests start with none
        root
    }

    #[test]
    fn replaced_model_comes_back() {
        let root = fixture("snap_attach");
        let before = tree(&root.dir);
        let tables = (std::fs::read(root.table_dir.join("sym-lib-table")).unwrap(), std::fs::read(root.table_dir.join("fp-lib-table")).unwrap());
        let outside = root.dir.parent().unwrap();
        let new_model = outside.join("new.step");
        std::fs::write(&new_model, "NEW MODEL").unwrap();
        // Different format too, so the old .step is deleted rather than overwritten.
        let wrl = outside.join("new.wrl");
        std::fs::write(&wrl, "#VRML").unwrap();

        root.attach_file("ICs", "B", AttachKind::Model, &new_model).unwrap();
        assert_eq!(std::fs::read_to_string(root.dir.join("3dmodels/ICs.3dshapes/FP2.step")).unwrap(), "NEW MODEL");
        root.attach_file("ICs", "B", AttachKind::Model, &wrl).unwrap();
        assert!(!root.dir.join("3dmodels/ICs.3dshapes/FP2.step").exists());
        assert_eq!(list(&root).len(), 2);

        let oldest = list(&root).pop().unwrap();
        rollback(&root, &oldest.id).unwrap();
        assert_eq!(tree(&root.dir), before);
        assert_eq!(std::fs::read_to_string(root.dir.join("3dmodels/ICs.3dshapes/FP2.step")).unwrap(), "OLD MODEL");
        assert_eq!(tables, (std::fs::read(root.table_dir.join("sym-lib-table")).unwrap(), std::fs::read(root.table_dir.join("fp-lib-table")).unwrap()));
    }

    #[test]
    fn category_move_is_undone_and_so_are_the_tables() {
        use crate::library::PartEdit;
        let root = fixture("snap_move");
        let before = tree(&root.dir);
        let table = std::fs::read(root.table_dir.join("fp-lib-table")).unwrap();

        let edit = PartEdit { category: Some("Power".into()), fields: vec![] };
        root.edit_part("ICs", "B", &edit).unwrap();
        assert!(root.dir.join("symbols/Power.kicad_sym").exists());
        assert_ne!(table, std::fs::read(root.table_dir.join("fp-lib-table")).unwrap(), "the move linked the new library");

        rollback(&root, &list(&root)[0].id).unwrap();
        assert_eq!(tree(&root.dir), before, "no Power library, model and footprint back in ICs");
        assert_eq!(table, std::fs::read(root.table_dir.join("fp-lib-table")).unwrap());
    }

    #[test]
    fn rollback_to_an_older_snapshot_undoes_the_newer_ones_and_can_be_undone() {
        let root = fixture("snap_older");
        let before = tree(&root.dir);
        let pdf = root.dir.parent().unwrap().join("a.pdf");
        std::fs::write(&pdf, "%PDF new").unwrap();
        root.attach_file("ICs", "B", AttachKind::Datasheet, &pdf).unwrap();
        let after_first = tree(&root.dir);
        root.attach_file("ICs", "A", AttachKind::Datasheet, &pdf).unwrap();
        let after_second = tree(&root.dir);
        assert_ne!(after_first, after_second);
        assert_eq!(list(&root).len(), 2);

        let oldest = list(&root).pop().unwrap();
        rollback(&root, &oldest.id).unwrap();
        assert_eq!(tree(&root.dir), before);

        // The rollback is itself a snapshot: go back to where we were.
        let undo = list(&root).into_iter().find(|s| s.label.starts_with("before rolling back")).unwrap();
        rollback(&root, &undo.id).unwrap();
        assert_eq!(tree(&root.dir), after_second);
    }

    #[test]
    fn a_guard_makes_many_changes_one_snapshot() {
        let root = fixture("snap_guard");
        let pdf = root.dir.parent().unwrap().join("a.pdf");
        std::fs::write(&pdf, "%PDF new").unwrap();
        {
            let _outer = guard(&root, "batch").unwrap();
            let _inner = guard(&root, "nested").unwrap();
            for part in ["A", "B"] {
                root.attach_file("ICs", part, AttachKind::Datasheet, &pdf).unwrap();
            }
        }
        let l = list(&root);
        assert_eq!((l.len(), l[0].label.as_str()), (1, "batch"));
        // Released: the next change snapshots again.
        root.attach_file("ICs", "A", AttachKind::Datasheet, &pdf).unwrap();
        assert_eq!(list(&root).len(), 2);
    }

    #[test]
    fn a_new_library_rolls_back_to_nothing() {
        let dir = crate::test_dir("snap_import");
        let root = LibraryRoot::global(dir.join("lib"), dir.join("config"));
        std::fs::create_dir_all(&root.table_dir).unwrap();
        let _snapshot = guard(&root, "first").unwrap();
        root.ensure_layout().unwrap();
        root.link("ICs").unwrap();
        assert!(root.table_dir.join("sym-lib-table").exists());
        rollback(&root, &list(&root)[0].id).unwrap();
        assert!(tree(&root.dir).is_empty() && !root.table_dir.join("sym-lib-table").exists());
    }

    #[test]
    fn retention_limits_count_and_size() {
        let root = fixture("snap_retention");
        for i in 0..5 {
            take(&root, &format!("s{i}")).unwrap();
        }
        prune_to(&root, 3, u64::MAX).unwrap();
        let labels: Vec<_> = list(&root).into_iter().map(|s| s.label).collect();
        assert_eq!(labels, ["s4", "s3", "s2"]);

        // Each snapshot copies ~700 bytes of text; room for one and a half keeps only the newest.
        prune_to(&root, 30, list(&root)[0].bytes * 3 / 2).unwrap();
        assert_eq!(list(&root).len(), 1);
        prune_to(&root, 30, 0).unwrap();
        assert_eq!(list(&root).len(), 1, "the newest is never pruned");
    }

    #[test]
    fn a_replaced_binary_is_charged_to_the_snapshot_that_holds_it() {
        let root = fixture("snap_size");
        take(&root, "s").unwrap();
        let text_only = list(&root)[0].bytes;
        let model = root.dir.join("3dmodels/ICs.3dshapes/FP2.step");
        std::fs::write(root.dir.join("m.tmp"), "A MUCH LONGER MODEL").unwrap();
        std::fs::rename(root.dir.join("m.tmp"), &model).unwrap();
        assert_eq!(list(&root)[0].bytes, text_only + "OLD MODEL".len() as u64);
    }
}
