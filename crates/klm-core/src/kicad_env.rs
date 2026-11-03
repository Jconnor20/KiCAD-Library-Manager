//! Finding the installed KiCad and editing its config: library tables and `kicad-cli` format upgrades.

use crate::sexpr;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Highest `x.y` sub-directory of `dir` (KiCad keeps one per major version).
fn newest_version_dir(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let (maj, min) = name.split_once('.')?;
            Some(((maj.parse::<u32>().ok()?, min.parse::<u32>().ok()?), e.path()))
        })
        .max_by_key(|(v, _)| *v)
        .map(|(_, p)| p)
}

/// KiCad's per-user config dir for the newest installed version, e.g. `%APPDATA%\kicad\10.0`.
pub fn config_dir() -> Result<PathBuf> {
    if let Some(sb) = crate::settings::sandbox_dir() {
        return sandbox_kicad_dir(&sb.join("kicad"));
    }
    let base = dirs::preference_dir().context("no user config directory")?.join("kicad");
    newest_version_dir(&base).with_context(|| format!("no KiCad config found in {}", base.display()))
}

/// The newest version folder under a sandbox's `kicad/`, created with empty library tables if there is none,
/// so a test never falls back to the real KiCad config.
fn sandbox_kicad_dir(base: &Path) -> Result<PathBuf> {
    if let Some(d) = newest_version_dir(base) {
        return Ok(d);
    }
    let d = base.join("10.0");
    std::fs::create_dir_all(&d)?;
    for (file, root) in [("sym-lib-table", "sym_lib_table"), ("fp-lib-table", "fp_lib_table")] {
        std::fs::write(d.join(file), format!("({root}
  (version 7)
)
"))?;
    }
    Ok(d)
}

/// The Settings override for `kicad-cli`. Process-wide so the GUI, the CLI and the core all honour it
/// without every function that runs `kicad-cli` taking a parameter; set at startup and when it changes.
static CLI_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_kicad_cli_override(path: Option<PathBuf>) {
    *CLI_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

pub fn kicad_cli() -> Result<PathBuf> {
    let over = CLI_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(resolve_cli(over.as_deref()))
}

/// `over` when it points at a file, else wherever KiCad is installed, else `kicad-cli` on PATH.
fn resolve_cli(over: Option<&Path>) -> PathBuf {
    if let Some(p) = over.filter(|p| p.is_file()) {
        return p.to_path_buf();
    }
    if cfg!(windows) {
        let pf = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or("C:/Program Files".into());
        if let Some(v) = newest_version_dir(&pf.join("KiCad")) {
            let p = v.join("bin/kicad-cli.exe");
            if p.exists() {
                return p;
            }
        }
    }
    let mac = PathBuf::from("/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli");
    if mac.exists() {
        return mac;
    }
    "kicad-cli".into()
}

/// Output of `kicad-cli version`, for the Settings "Test" button.
pub fn kicad_cli_version() -> Result<String> {
    let cli = kicad_cli()?;
    let out = quiet(&cli).arg("version").output().with_context(|| format!("could not run {}", cli.display()))?;
    if !out.status.success() {
        bail!("{} failed: {}", cli.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn quiet(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    // kicad-cli rewrites its own kicad_common.json on every run; keep that inside the sandbox too.
    if let Some(sb) = crate::settings::sandbox_dir() {
        cmd.env("KICAD_CONFIG_HOME", sb.join("kicad"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash from the GUI
    }
    cmd
}

/// True while any KiCad window is open. KiCad keeps its library tables and path variables in
/// memory and writes them back over any edits made meanwhile, so those edits must wait.
static RUNNING_CACHE: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);

/// `kicad_running_now`, remembered for 5 s so UI clicks don't each spawn `tasklist`.
pub fn kicad_running() -> bool {
    let c = RUNNING_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *c
        && at.elapsed() < std::time::Duration::from_secs(5)
    {
        return v;
    }
    drop(c);
    kicad_running_now()
}

/// Uncached check (which also refreshes the cache), for "has the user closed KiCad yet?" re-checks.
pub fn kicad_running_now() -> bool {
    let v = scan_processes();
    *RUNNING_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((std::time::Instant::now(), v));
    v
}

fn scan_processes() -> bool {
    // A sandbox's tables are not the running KiCad's, so it cannot overwrite them (and a test must not be blocked by the user's KiCad).
    if crate::settings::sandbox_dir().is_some() {
        return false;
    }
    let (prog, args): (&str, &[&str]) = if cfg!(windows) { ("tasklist", &["/NH", "/FO", "CSV"]) } else { ("ps", &["-A", "-o", "comm="]) };
    let Ok(out) = quiet(Path::new(prog)).args(args).output() else { return false };
    String::from_utf8_lossy(&out.stdout).lines().any(|l| {
        let name = l.split(',').next().unwrap_or("").trim_matches('"').to_lowercase();
        let name = name.rsplit('/').next().unwrap_or("").trim_end_matches(".exe");
        matches!(name, "kicad" | "eeschema" | "pcbnew")
    })
}

pub fn run_cli(args: &[&std::ffi::OsStr]) -> Result<()> {
    let cli = kicad_cli()?;
    let out = quiet(&cli).args(args).output().with_context(|| format!("could not run {}", cli.display()))?;
    if !out.status.success() {
        bail!("kicad-cli failed: {}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    }
    Ok(())
}

/// Rewrites any KiCad symbol library (legacy `.lib` included) in the installed KiCad's format.
pub fn upgrade_symbols(input: &Path, output: &Path) -> Result<()> {
    run_cli(&["sym".as_ref(), "upgrade".as_ref(), "--force".as_ref(), "-o".as_ref(), output.as_os_str(), input.as_os_str()])
}

/// Rewrites a `.pretty` folder of footprints in the installed KiCad's format.
pub fn upgrade_footprints(input_dir: &Path, output_dir: &Path) -> Result<()> {
    run_cli(&["fp".as_ref(), "upgrade".as_ref(), "--force".as_ref(), "-o".as_ref(), output_dir.as_os_str(), input_dir.as_os_str()])
}

/// Write via a temp file so a crash never leaves a half-written library.
pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("klm-tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

pub fn backup(path: &Path) -> Result<()> {
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        std::fs::copy(path, &bak).with_context(|| format!("backing up {}", path.display()))?;
    }
    Ok(())
}

/// Whether a `sym-lib-table` / `fp-lib-table` already lists `nickname`.
pub fn is_registered(table: &Path, nickname: &str) -> bool {
    std::fs::read_to_string(table)
        .ok()
        .and_then(|t| sexpr::parse(&t).ok())
        .is_some_and(|t| t.children("lib").any(|l| l.child("name").and_then(|n| n.arg(0)) == Some(nickname)))
}

/// Adds a library to a `sym-lib-table` / `fp-lib-table`. Returns false if the nickname is already there.
/// The new line is spliced in as text so the user's existing table is left byte-for-byte as it was.
pub fn register_lib(table: &Path, nickname: &str, uri: &str) -> Result<bool> {
    let kind = if table.file_name().is_some_and(|n| n == "fp-lib-table") { "fp_lib_table" } else { "sym_lib_table" };
    let text = if table.exists() {
        std::fs::read_to_string(table)?
    } else {
        format!("({kind}\n\t(version 7)\n)\n")
    };
    sexpr::parse(&text).with_context(|| format!("reading {}", table.display()))?;
    if is_registered(table, nickname) {
        return Ok(false);
    }
    let entry = sexpr::Sexpr::list(
        "lib",
        vec![
            sexpr::Sexpr::list("name", vec![sexpr::Sexpr::str(nickname)]),
            sexpr::Sexpr::list("type", vec![sexpr::Sexpr::str("KiCad")]),
            sexpr::Sexpr::list("uri", vec![sexpr::Sexpr::str(uri)]),
            sexpr::Sexpr::list("options", vec![sexpr::Sexpr::str("")]),
            sexpr::Sexpr::list("descr", vec![sexpr::Sexpr::str("Managed by KiCad Library Manager")]),
        ],
    );
    let close = text.rfind(')').context("library table has no closing ')'")?;
    let line = sexpr::to_string(&entry).replace("\n\t", " ").replace('\n', "");
    let new_text = format!("{}\t{}\n{}", text[..close].trim_end_matches([' ', '\t']), line, &text[close..]);
    backup(table)?;
    write_atomic(table, &new_text)?;
    Ok(true)
}

/// A table entry waiting for KiCad to close, so a new library can be written while KiCad is open.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct PendingLib {
    pub table: PathBuf,
    pub nickname: String,
    pub uri: String,
}

/// Where queued entries are kept. `None` (the CLI, tests) means nothing is ever queued.
static QUEUE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where the GUI and the CLI keep the queue, so either one can queue and the GUI applies it.
pub fn default_queue_path() -> Option<PathBuf> {
    Some(crate::settings::app_dir()?.join("pending-libs.json"))
}

pub fn set_pending_queue(path: Option<PathBuf>) {
    *QUEUE.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

fn queue_path() -> Option<PathBuf> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn pending_in(queue: &Path) -> Vec<PendingLib> {
    std::fs::read_to_string(queue).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Adds `entry` to the queue file. False if the same table and nickname are already waiting.
pub fn enqueue_in(queue: &Path, entry: PendingLib) -> Result<bool> {
    let mut all = pending_in(queue);
    if all.iter().any(|e| e.table == entry.table && e.nickname == entry.nickname) {
        return Ok(false);
    }
    all.push(entry);
    if let Some(dir) = queue.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomic(queue, &serde_json::to_string_pretty(&all)?)?;
    Ok(true)
}

/// A change to an existing table, written at once and repeated when KiCad closes in case it saved over it.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct PendingTable {
    pub table: PathBuf,
    pub op: TableOp,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub enum TableOp {
    /// Path prefix `old` becomes `new`.
    Rewrite { old: String, new: String },
    /// The whole table is this text (`None`: the file does not exist).
    Restore(Option<String>),
}

fn table_queue(queue: &Path) -> PathBuf {
    queue.with_file_name("pending-tables.json")
}

pub fn pending_tables_in(queue: &Path) -> Vec<PendingTable> {
    std::fs::read_to_string(table_queue(queue)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn apply_table_op(table: &Path, op: &TableOp) -> Result<()> {
    match op {
        TableOp::Rewrite { old, new } => {
            if let Ok(text) = std::fs::read_to_string(table).map(|t| t.replace(old, new)) {
                write_atomic(table, &text)?;
            }
        }
        TableOp::Restore(Some(text)) => {
            let current = std::fs::read_to_string(table).unwrap_or_default();
            write_atomic(table, &with_outside_entries(text, &current))?;
        }
        TableOp::Restore(None) => {
            // Entries that aren't ours were added since: keep the table, minus ours.
            if table.is_file() {
                let current = std::fs::read_to_string(table).unwrap_or_default();
                if outside_entries(&current, "").is_empty() {
                    std::fs::remove_file(table)?;
                } else {
                    write_atomic(table, &with_outside_entries(&empty_table(table), &current))?;
                }
            }
        }
    }
    Ok(())
}

fn empty_table(table: &Path) -> String {
    let kind = if table.file_name().is_some_and(|n| n == "fp-lib-table") { "fp_lib_table" } else { "sym_lib_table" };
    format!("({kind}\n\t(version 7)\n)\n")
}

/// Entries of `current` that `base` doesn't list and that KLM didn't add: edits made by the user or KiCad
/// since the restore was queued. KLM's own entries are left out so a restore can still remove them.
fn outside_entries(current: &str, base: &str) -> Vec<String> {
    let Ok(cur) = sexpr::parse(current) else { return vec![] };
    let base = sexpr::parse(base).ok();
    let name = |l: &sexpr::Sexpr| l.child("name").and_then(|n| n.arg(0)).unwrap_or_default().to_string();
    let known: Vec<String> = base.iter().flat_map(|b| b.children("lib")).map(name).collect();
    cur.children("lib")
        .filter(|l| !known.contains(&name(l)) && !l.child("descr").and_then(|d| d.arg(0)).is_some_and(|d| d.contains("Managed by KiCad Library Manager")))
        .map(|l| sexpr::to_string(l).replace("\n\t", " ").replace('\n', ""))
        .collect()
}

/// `restore` with the outside entries of `current` spliced in, so applying a queued restore doesn't lose them.
fn with_outside_entries(restore: &str, current: &str) -> String {
    let extra = outside_entries(current, restore);
    let Some(close) = restore.rfind(')').filter(|_| !extra.is_empty()) else { return restore.to_string() };
    let lines: String = extra.iter().map(|e| format!("\t{e}\n")).collect();
    format!("{}{lines}{}", restore[..close].trim_end_matches([' ', '\t']), &restore[close..])
}

/// Records that `op` was just done to `table` so it is repeated once KiCad closes. Only while KiCad is open and queueing is on.
pub fn defer_table_op(table: &Path, op: TableOp) -> Result<()> {
    let Some(q) = queue_path().filter(|_| kicad_running()) else { return Ok(()) };
    let mut all = pending_tables_in(&q);
    all.push(PendingTable { table: table.to_path_buf(), op });
    if let Some(dir) = q.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomic(&table_queue(&q), &serde_json::to_string_pretty(&all)?)
}

/// Repeats the queued table changes, in order, and empties that queue. The caller makes sure KiCad is closed.
fn apply_tables_in(queue: &Path) -> Result<()> {
    let tq = table_queue(queue);
    for e in pending_tables_in(queue) {
        apply_table_op(&e.table, &e.op)?;
    }
    if tq.exists() {
        std::fs::remove_file(&tq)?;
    }
    Ok(())
}

/// Repeats queued table changes, then registers every queued entry and empties the queue; entries that fail stay queued.
/// Returns how many libraries were added. The caller makes sure KiCad is closed.
pub fn apply_in(queue: &Path) -> Result<usize> {
    apply_tables_in(queue)?;
    let (mut added, mut keep) = (0, vec![]);
    for e in pending_in(queue) {
        match register_lib(&e.table, &e.nickname, &e.uri) {
            Ok(true) => added += 1,
            Ok(false) => {}
            Err(_) => keep.push(e),
        }
    }
    if keep.is_empty() {
        let _ = std::fs::remove_file(queue);
    } else {
        write_atomic(queue, &serde_json::to_string_pretty(&keep)?)?;
    }
    Ok(added)
}

pub fn pending_count() -> usize {
    queue_path().map_or(0, |q| pending_in(&q).len() + pending_tables_in(&q).len())
}

/// Whether `nickname` is waiting to be added to `table`.
pub fn is_queued(table: &Path, nickname: &str) -> bool {
    queue_path().is_some_and(|q| pending_in(&q).iter().any(|e| e.table == table && e.nickname == nickname))
}

/// Applies the queue if KiCad is closed (cached check). Returns how many libraries were added.
pub fn apply_pending() -> Result<usize> {
    match queue_path() {
        Some(q) if (q.exists() || table_queue(&q).exists()) && !kicad_running() => apply_in(&q),
        _ => Ok(0),
    }
}

/// Like `register_lib`, but while KiCad is open (and queueing is on) the entry is also queued, to be re-checked once KiCad closes.
/// Returns true if the library was added or queued.
pub fn register_or_queue(table: &Path, nickname: &str, uri: &str) -> Result<bool> {
    match queue_path() {
        Some(q) if kicad_running() => {
            if is_registered(table, nickname) {
                return Ok(false);
            }
            // Written now so it is usable at once, and queued too: if KiCad saves over the table when it
            // closes, the queue puts the entry back.
            register_lib(table, nickname, uri)?;
            enqueue_in(&q, PendingLib { table: table.to_path_buf(), nickname: nickname.into(), uri: uri.into() })
        }
        _ => register_lib(table, nickname, uri),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_dedupes_and_applies() {
        let dir = crate::test_dir("queue");
        let (table, queue) = (dir.join("sym-lib-table"), dir.join("pending.json"));
        let e = PendingLib { table: table.clone(), nickname: "KLM_ICs".into(), uri: "x/ICs.kicad_sym".into() };
        assert!(enqueue_in(&queue, e.clone()).unwrap());
        assert!(!enqueue_in(&queue, e).unwrap());
        assert_eq!(pending_in(&queue).len(), 1);
        assert_eq!(apply_in(&queue).unwrap(), 1);
        assert!(is_registered(&table, "KLM_ICs"));
        assert!(pending_in(&queue).is_empty());
    }

    #[test]
    fn table_ops_are_repeated_after_kicad_overwrites() {
        let dir = crate::test_dir("table_ops");
        let (table, queue) = (dir.join("fp-lib-table"), dir.join("pending-libs.json"));
        // Stands in for defer_table_op, which only queues while KiCad is running.
        let defer = |op| {
            let mut all = pending_tables_in(&queue);
            all.push(PendingTable { table: table.clone(), op });
            std::fs::write(table_queue(&queue), serde_json::to_string(&all).unwrap()).unwrap();
        };
        defer(TableOp::Rewrite { old: "/old/".into(), new: "/new/".into() });
        std::fs::write(&table, "(uri \"/old/a\")").unwrap(); // KiCad saved its stale copy
        assert_eq!(apply_in(&queue).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&table).unwrap(), "(uri \"/new/a\")");
        assert!(pending_tables_in(&queue).is_empty());

        defer(TableOp::Restore(Some("restored".into())));
        apply_in(&queue).unwrap();
        assert_eq!(std::fs::read_to_string(&table).unwrap(), "restored");
        defer(TableOp::Restore(None));
        apply_in(&queue).unwrap();
        assert!(!table.exists());
    }

    #[test]
    fn register_lib_appends_once_and_backs_up() {
        let dir = crate::test_dir("register");
        let table = dir.join("sym-lib-table");
        let original = "(sym_lib_table\n\t(version 7)\n\t(lib (name \"Mine\") (type \"KiCad\") (uri \"x\") (options \"\") (descr \"\"))\n)\n";
        std::fs::write(&table, original).unwrap();

        assert!(register_lib(&table, "KLM_Resistors", "${KLM_LIB}/symbols/Resistors.kicad_sym").unwrap());
        assert!(!register_lib(&table, "KLM_Resistors", "anything").unwrap());

        let text = std::fs::read_to_string(&table).unwrap();
        assert!(text.starts_with(original.trim_end_matches(")\n")), "existing lines untouched");
        let t = sexpr::parse(&text).unwrap();
        assert_eq!(t.children("lib").count(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("sym-lib-table.bak")).unwrap(), original);
    }

    #[test]
    fn register_lib_creates_missing_table() {
        let dir = crate::test_dir("register_new");
        let table = dir.join("fp-lib-table");
        assert!(register_lib(&table, "PRJ_ICs", "${KIPRJMOD}/lib/footprints/ICs.pretty").unwrap());
        let t = sexpr::parse(&std::fs::read_to_string(&table).unwrap()).unwrap();
        assert_eq!(t.head(), Some("fp_lib_table"));
        assert_eq!(t.child("lib").unwrap().child("uri").unwrap().arg(0), Some("${KIPRJMOD}/lib/footprints/ICs.pretty"));
    }

    #[test]
    fn cli_override_is_used_only_when_the_file_exists() {
        let dir = crate::test_dir("cli_override");
        let fake = dir.join("my-kicad-cli");
        std::fs::write(&fake, "").unwrap();
        assert_eq!(resolve_cli(Some(&fake)), fake);
        // A stale setting falls back to the normal search instead of breaking every import.
        assert_eq!(resolve_cli(Some(&dir.join("gone"))), resolve_cli(None));
    }
}
