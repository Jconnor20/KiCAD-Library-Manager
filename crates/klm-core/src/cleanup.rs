//! Tidying the libraries KiCad already has: reports duplicates, broken entries and unused libraries
//! across the global and project `sym-lib-table` / `fp-lib-table`, then removes entries, fixes their
//! paths or moves a library into KLM.
//!
//! This edits the user's real KiCad config, so it is cautious:
//! - [`scan`] and [`plan`] only read. [`apply`] is the one function that writes.
//! - `apply` refuses while KiCad runs (it rewrites its tables on exit) or while queued libraries wait
//!   for a table, backs each table up to its own timestamped file first, edits only the chosen
//!   `(lib ...)` text, and puts the backup back if the result isn't what the plan said.
//! - Library files are never deleted: Remove only unlinks the table entry, and moving a library into KLM
//!   copies it and leaves the old entry for the user to remove (the import may not take everything).
//! - Only plain `KiCad`/`Legacy` entries the user owns can be changed. Stock (`${KICAD*}`), KLM-managed,
//!   disabled-duplicate, database, HTTP and nested-table entries are reported but never touched.
//! - "Unused" is information only: it means no file of the chosen projects names the library.

use crate::batch::{self, Op, Target};
use crate::kicad_env;
use crate::library::{LibraryRoot, property};
use crate::rules::Rules;
use crate::sexpr;
use crate::sources::foreign;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Symbol,
    Footprint,
}

impl Kind {
    pub fn table_file(self) -> &'static str {
        if self == Kind::Symbol { "sym-lib-table" } else { "fp-lib-table" }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub table: PathBuf,
    pub kind: Kind,
    pub name: String,
    pub uri: String,
    /// `${KIPRJMOD}` resolves to this for a project's own table.
    pub project: Option<PathBuf>,
    /// The uri with its variables filled in, when they could all be.
    pub path: Option<PathBuf>,
    /// KiCad doesn't load it.
    pub disabled: bool,
    /// Whether remove / fix path are allowed on it.
    pub actionable: bool,
    /// Why it isn't, when it isn't.
    pub locked: Option<&'static str>,
    /// How many project files name this library; `None` when no project was chosen.
    pub refs: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    Broken,
    /// Can't be checked here (a KiCad-provided variable, a URL, a backslash path).
    Unverifiable,
    Unused,
    Duplicate,
}

#[derive(Clone, Debug)]
pub struct Issue {
    /// Index into [`Report::entries`].
    pub entry: usize,
    pub kind: IssueKind,
    pub message: String,
}

#[derive(Default)]
pub struct Report {
    pub entries: Vec<Entry>,
    pub issues: Vec<Issue>,
    /// Tables that couldn't be read, and other problems. These are never silently "no entries".
    pub errors: Vec<String>,
    /// Projects whose files were searched for references.
    pub projects: Vec<PathBuf>,
    /// Other KiCad version folders, which are not scanned.
    pub other_configs: Vec<PathBuf>,
}

impl Report {
    pub fn issues_of(&self, entry: usize) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |i| i.entry == entry)
    }

    pub fn find(&self, table: &Path, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.table == table && e.name == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Take the entry out of its table. The library's files stay.
    Remove { table: PathBuf, name: String },
    /// Point the entry at another file (symbols) or `.pretty` folder (footprints).
    FixPath { table: PathBuf, name: String, new_path: PathBuf },
}

impl Action {
    fn target(&self) -> (&Path, &str) {
        match self {
            Action::Remove { table, name } | Action::FixPath { table, name, .. } => (table, name),
        }
    }
}

// ---- reading tables as text ----

/// One child `(key value ...)` of an s-expression, with byte offsets into the text it was read from.
pub(crate) struct Kid {
    pub(crate) key: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// The first argument as written between its quotes (escapes are NOT interpreted: KiCad writes
    /// paths with `/`, and a hand-written `C:\x` must not silently become `C:x`), and the quoted literal's range.
    value: Option<(String, std::ops::Range<usize>)>,
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

/// Index after the quoted string starting at `i`.
fn skip_str(b: &[u8], i: usize) -> Result<usize> {
    let mut j = i + 1;
    while let Some(&c) = b.get(j) {
        match c {
            b'\\' => j += 2,
            b'"' => return Ok(j + 1),
            _ => j += 1,
        }
    }
    bail!("a quoted string is never closed")
}

/// Index after the parenthesised expression starting at `i`.
fn skip_expr(b: &[u8], mut i: usize) -> Result<usize> {
    let mut depth = 0usize;
    while let Some(&c) = b.get(i) {
        match c {
            b'"' => {
                i = skip_str(b, i)?;
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    bail!("a parenthesis is never closed")
}

fn token_end(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(|c| !c.is_ascii_whitespace() && !matches!(c, b'(' | b')' | b'"')) {
        i += 1;
    }
    i
}

/// The `(key value ...)` children of the expression `s`, which starts with `(`.
pub(crate) fn kids(s: &str) -> Result<Vec<Kid>> {
    let b = s.as_bytes();
    if b.first() != Some(&b'(') {
        bail!("expected '('");
    }
    let mut i = token_end(b, 1);
    let mut out = vec![];
    loop {
        i = skip_ws(b, i);
        match b.get(i) {
            None => bail!("a parenthesis is never closed"),
            Some(b')') => return Ok(out),
            Some(b'(') => {
                let start = i;
                let kend = token_end(b, start + 1);
                let v = skip_ws(b, kend);
                let value = match b.get(v) {
                    Some(b'"') => {
                        let e = skip_str(b, v)?;
                        Some((s[v + 1..e - 1].to_string(), v..e))
                    }
                    Some(b'(') | Some(b')') | None => None,
                    Some(_) => Some((s[v..token_end(b, v)].to_string(), v..token_end(b, v))),
                };
                out.push(Kid { key: s[start + 1..kend].to_string(), start, end: skip_expr(b, start)?, value });
                i = out[out.len() - 1].end;
            }
            Some(b'"') => i = skip_str(b, i)?,
            Some(_) => i = token_end(b, i),
        }
    }
}

/// A `(lib ...)` of a table, with where it sits in the text.
#[derive(Debug)]
struct Raw {
    name: String,
    typ: String,
    uri: String,
    descr: String,
    disabled: bool,
    start: usize,
    end: usize,
    /// The quoted uri literal, offsets into the whole text.
    uri_lit: std::ops::Range<usize>,
}

const BOM: char = '\u{feff}';

/// `text` without a leading byte-order mark, and the mark's length so offsets can be shifted back.
fn strip_bom(text: &str) -> (&str, usize) {
    text.strip_prefix(BOM).map_or((text, 0), |t| (t, BOM.len_utf8()))
}

fn parse_table(text: &str) -> Result<Vec<Raw>> {
    let (body, shift) = strip_bom(text);
    let body_trim = body.trim_start();
    let lead = body.len() - body_trim.len();
    let root = kids(body_trim)?;
    let rest = &body_trim[skip_expr(body_trim.as_bytes(), 0)?..];
    if !rest.trim().is_empty() {
        bail!("text after the end of the table");
    }
    let base = shift + lead;
    let mut out = vec![];
    for k in root.into_iter().filter(|k| k.key == "lib") {
        let span = &body_trim[k.start..k.end];
        let f = kids(span)?;
        let get = |key: &str| f.iter().find(|c| c.key == key).and_then(|c| c.value.clone());
        let uri = get("uri").context("a library has no uri")?;
        out.push(Raw {
            name: get("name").context("a library has no name")?.0,
            typ: get("type").map(|t| t.0).unwrap_or_default(),
            uri: uri.0,
            descr: get("descr").map(|t| t.0).unwrap_or_default(),
            disabled: f.iter().any(|c| c.key == "disabled"),
            start: base + k.start,
            end: base + k.end,
            uri_lit: base + k.start + uri.1.start..base + k.start + uri.1.end,
        });
    }
    Ok(out)
}

// ---- resolving and judging entries ----

/// KiCad's user-defined path variables (`environment.vars` in `kicad_common.json`).
pub(crate) fn user_vars(config: &Path) -> BTreeMap<String, String> {
    let v: Option<serde_json::Value> = std::fs::read_to_string(config.join("kicad_common.json")).ok().and_then(|t| serde_json::from_str(strip_bom(&t).0).ok());
    let vars = v.as_ref().and_then(|v| v["environment"]["vars"].as_object());
    vars.into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()
}

/// The uri with `${VAR}`s replaced. `Err` says why it can't be: that is "can't check", not "broken",
/// except for a variable that is defined nowhere, which the caller reports as broken.
enum Resolved {
    Path(PathBuf),
    Unverifiable(String),
    Undefined(String),
}

fn resolve(uri: &str, project: Option<&Path>, vars: &BTreeMap<String, String>) -> Resolved {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return Resolved::Unverifiable("a web address".into());
    }
    let mut s = uri.to_string();
    while let Some(open) = s.find("${") {
        let Some(len) = s[open..].find('}') else { return Resolved::Unverifiable("an unclosed variable".into()) };
        let var = s[open + 2..open + len].to_string();
        let value = if var == "KIPRJMOD" {
            match project {
                Some(p) => p.to_string_lossy().replace('\\', "/"),
                None => return Resolved::Unverifiable("depends on the project that opens it".into()),
            }
        } else if let Some(v) = std::env::var(&var).ok().or_else(|| vars.get(&var).cloned()) {
            v
        } else if var.to_uppercase().starts_with("KICAD") || var == "KLM_LIB" {
            return Resolved::Unverifiable(format!("{var} is set by KiCad itself"));
        } else {
            return Resolved::Undefined(var);
        };
        s.replace_range(open..open + len + 1, &value);
    }
    if s.contains('\\') && !Path::new(&s).exists() {
        return Resolved::Unverifiable("a path with backslashes, which KiCad reads differently".into());
    }
    let path = PathBuf::from(s);
    if path.is_relative() {
        // KiCad opens a relative path from the project; there is no telling from where for a global table.
        return match project {
            Some(p) => Resolved::Path(p.join(path)),
            None => Resolved::Unverifiable("a relative path".into()),
        };
    }
    Resolved::Path(path)
}

/// Whether the uri uses a variable KiCad defines itself (`${KICAD9_SYMBOL_DIR}`, `${KICAD9_3RD_PARTY}`, ...),
/// as opposed to one the user made up that merely starts with KICAD.
fn uses_kicad_var(uri: &str) -> bool {
    let up = uri.to_uppercase();
    up.match_indices("${KICAD").any(|(i, _)| {
        let rest = &up[i + 7..];
        rest.starts_with(|c: char| c.is_ascii_digit()) || rest.starts_with("_USER_TEMPLATE_DIR")
    })
}

fn lock_reason(typ: &str, raw: &Raw) -> Option<&'static str> {
    let up = raw.uri.to_uppercase();
    if uses_kicad_var(&raw.uri) {
        Some("a KiCad or plugin-manager library")
    } else if raw.descr.contains("Managed by KiCad Library Manager") || up.contains("${KLM_LIB}") {
        Some("managed by KLM (use Relink)")
    } else if !(typ.eq_ignore_ascii_case("KiCad") || typ.eq_ignore_ascii_case("Legacy")) {
        Some("not a plain KiCad library")
    } else {
        None
    }
}

/// Lists a library's part names without reading anything else about it.
fn names_in(entry: &Entry) -> Vec<String> {
    let Some(p) = &entry.path else { return vec![] };
    match entry.kind {
        Kind::Symbol => std::fs::read_to_string(p)
            .ok()
            .and_then(|t| sexpr::parse(strip_bom(&t).0).ok())
            .map(|lib| lib.children("symbol").filter_map(|s| s.arg(0).map(String::from)).collect())
            .unwrap_or_default(),
        Kind::Footprint => std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "kicad_mod"))
            .filter_map(|e| Some(e.path().file_stem()?.to_string_lossy().into_owned()))
            .collect(),
    }
}

/// Folders that are projects: those holding a `.kicad_pro`, searched three levels down. A folder with
/// library tables but no `.kicad_pro` counts as a project itself.
pub fn find_projects(dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(d: &Path, depth: usize, out: &mut BTreeSet<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut subs = vec![];
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 3 && !e.file_name().to_string_lossy().starts_with('.') {
                    subs.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "kicad_pro") {
                out.insert(d.to_path_buf());
            }
        }
        subs.into_iter().for_each(|s| walk(&s, depth + 1, out));
    }
    let mut out = BTreeSet::new();
    for d in dirs {
        let before = out.len();
        walk(d, 0, &mut out);
        if out.len() == before && (d.join("sym-lib-table").exists() || d.join("fp-lib-table").exists()) {
            out.insert(d.clone());
        }
    }
    out.into_iter().collect()
}

fn project_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !name.starts_with('.') && name != crate::snapshot::DIR {
                project_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "kicad_sch" || x == "kicad_pcb" || x == "kicad_sym") {
            out.push(p);
        }
    }
}

/// Reads the global tables in `config` and the tables of every project found in `project_dirs`, and
/// reports what is wrong. Writes nothing.
pub fn scan(config: &Path, project_dirs: &[PathBuf]) -> Report {
    let mut r = Report { projects: find_projects(project_dirs), ..Report::default() };
    let vars = user_vars(config);
    if let Some(parent) = config.parent() {
        r.other_configs =
            std::fs::read_dir(parent).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p != config && p.join("sym-lib-table").exists()).collect();
    }
    let mut tables: Vec<(PathBuf, Kind, Option<PathBuf>)> = vec![];
    for kind in [Kind::Symbol, Kind::Footprint] {
        tables.push((config.join(kind.table_file()), kind, None));
        tables.extend(r.projects.iter().map(|p| (p.join(kind.table_file()), kind, Some(p.clone()))));
    }
    for (table, kind, project) in tables {
        let Ok(text) = std::fs::read_to_string(&table) else {
            if table.exists() {
                r.errors.push(format!("{} can't be read", table.display()));
            }
            continue;
        };
        let raws = match parse_table(&text) {
            Ok(raws) => raws,
            Err(e) => {
                r.errors.push(format!("{} can't be understood and is left alone: {e:#}", table.display()));
                continue;
            }
        };
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        raws.iter().for_each(|x| *seen.entry(&x.name).or_default() += 1);
        for raw in &raws {
            let (path, mut note) = match resolve(&raw.uri, project.as_deref(), &vars) {
                Resolved::Path(p) => (Some(p), None),
                Resolved::Unverifiable(why) => (None, Some((IssueKind::Unverifiable, why))),
                Resolved::Undefined(v) if project.is_some() => (None, Some((IssueKind::Unverifiable, format!("{v} may be a text variable of the project")))),
                Resolved::Undefined(v) => (None, Some((IssueKind::Broken, format!("the variable {v} is not defined in KiCad's settings")))),
            };
            let mut locked = lock_reason(&raw.typ, raw);
            if seen[raw.name.as_str()] > 1 {
                locked = Some("the name is listed twice in this table");
            }
            let idx = r.entries.len();
            r.entries.push(Entry {
                table: table.clone(),
                kind,
                name: raw.name.clone(),
                uri: raw.uri.clone(),
                project: project.clone(),
                path,
                disabled: raw.disabled,
                actionable: locked.is_none(),
                locked,
                refs: None,
            });
            if raw.uri.trim().is_empty() {
                note = Some((IssueKind::Broken, "has no path".into()));
            } else if let Some(p) = &r.entries[idx].path {
                let plain = raw.typ.eq_ignore_ascii_case("KiCad");
                let ok = match kind {
                    _ if !plain => p.exists(),
                    Kind::Symbol => p.is_file(),
                    Kind::Footprint => p.is_dir(),
                };
                if !ok {
                    note = Some((IssueKind::Broken, format!("{} doesn't exist", p.display())));
                } else if plain && kind == Kind::Footprint && !p.extension().is_some_and(|x| x == "pretty") {
                    note = Some((IssueKind::Broken, "isn't a .pretty folder".into()));
                }
            }
            if let Some((kind, message)) = note {
                r.issues.push(Issue { entry: idx, kind, message });
            }
        }
    }
    duplicates(&mut r);
    unused(&mut r);
    r
}

/// Same part name in two libraries of the user's own. Stock libraries are skipped: KiCad always names
/// the library too, so a shared name there is normal and would bury everything else.
fn duplicates(r: &mut Report) {
    let own: Vec<usize> = (0..r.entries.len())
        .filter(|&i| {
            !r.entries[i].disabled && r.entries[i].locked != Some("a KiCad or plugin-manager library") && r.entries[i].path.as_ref().is_some_and(|p| p.exists())
        })
        .collect();
    let names: BTreeMap<usize, BTreeSet<String>> = own.iter().map(|&i| (i, names_in(&r.entries[i]).into_iter().collect())).collect();
    let canon = |i: usize| r.entries[i].path.as_ref().and_then(|p| p.canonicalize().ok());
    let mut found = vec![];
    for (n, &a) in own.iter().enumerate() {
        for &b in &own[n + 1..] {
            if r.entries[a].kind != r.entries[b].kind || canon(a) == canon(b) {
                continue;
            }
            let shared: Vec<&String> = names[&a].intersection(&names[&b]).collect();
            if shared.is_empty() {
                continue;
            }
            let list = shared.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
            let more = if shared.len() > 5 { format!(" and {} more", shared.len() - 5) } else { String::new() };
            for (x, y) in [(a, b), (b, a)] {
                found.push(Issue {
                    entry: x,
                    kind: IssueKind::Duplicate,
                    message: format!("{} part(s) also in {}: {list}{more}", shared.len(), r.entries[y].name),
                });
            }
        }
    }
    r.issues.extend(found);
}

/// Counts the project files naming each library, and flags the ones nothing names.
fn unused(r: &mut Report) {
    if r.projects.is_empty() {
        return;
    }
    let mut files = vec![];
    r.projects.iter().for_each(|p| project_files(p, &mut files));
    // Footprint libraries are also used through the Footprint field of symbol libraries.
    for e in r.entries.iter().filter(|e| e.kind == Kind::Symbol && e.locked != Some("a KiCad or plugin-manager library")) {
        files.extend(e.path.clone().filter(|p| p.is_file()));
    }
    files.sort();
    files.dedup();
    let texts: Vec<String> = files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
    let mut found = vec![];
    let n = r.projects.len();
    for (i, e) in r.entries.iter_mut().enumerate() {
        let (quoted, bare) = (format!("\"{}:", e.name), format!(" {}:", e.name));
        let refs = texts.iter().filter(|t| t.contains(&quoted) || t.contains(&bare)).count();
        e.refs = Some(refs);
        if refs == 0 && e.locked.is_none() {
            found.push(Issue {
                entry: i,
                kind: IssueKind::Unused,
                message: format!("no schematic, board or symbol library of the {n} chosen project(s) uses it; other projects may"),
            });
        }
    }
    r.issues.extend(found);
}

// ---- changing tables ----

/// What one table becomes.
pub struct TablePlan {
    pub file: PathBuf,
    before: String,
    after: String,
}

pub struct Plan {
    pub tables: Vec<TablePlan>,
    /// One line per change, for the preview.
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
}

fn uri_literal(path: &Path, project: Option<&Path>) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if let Some(p) = project {
        let p = p.to_string_lossy().replace('\\', "/");
        if let Some(rel) = s.strip_prefix(&format!("{}/", p.trim_end_matches('/'))) {
            s = format!("${{KIPRJMOD}}/{rel}");
        }
    }
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// Works out the new text of every table the actions touch, reading but not writing anything. Fails
/// without partial results if an entry is gone, locked, named twice or given a bad path.
pub fn plan(report: &Report, actions: &[Action]) -> Result<Plan> {
    let mut by_table: BTreeMap<&Path, Vec<&Action>> = BTreeMap::new();
    for a in actions {
        by_table.entry(a.target().0).or_default().push(a);
    }
    let mut out = Plan { tables: vec![], lines: vec![], warnings: vec![] };
    for (table, acts) in by_table {
        let before = std::fs::read_to_string(table).with_context(|| format!("reading {}", table.display()))?;
        let raws = parse_table(&before).with_context(|| format!("{} can't be understood", table.display()))?;
        let mut edits: Vec<(std::ops::Range<usize>, String)> = vec![];
        let mut touched = BTreeSet::new();
        for a in &acts {
            let (_, name) = a.target();
            let entry = report.find(table, name).with_context(|| format!("{name} is not in {}", table.display()))?;
            if let Some(why) = entry.locked {
                bail!("{name} can't be changed: {why}");
            }
            if !touched.insert(name) {
                bail!("{name} has two actions");
            }
            let raw = raws
                .iter()
                .find(|r| r.name == name && r.uri == entry.uri)
                .with_context(|| format!("{name} changed in {} since it was scanned; scan again", table.display()))?;
            match a {
                Action::Remove { .. } => {
                    let (mut s, mut e) = (raw.start, raw.end);
                    let line_start = before[..s].rfind('\n').map_or(0, |i| i + 1);
                    let tail = before[e..].find('\n').map_or(before.len(), |i| e + i + 1);
                    // Take the whole line (with its line ending) when the entry is alone on it.
                    if before[line_start..s].trim().is_empty() && before[e..tail].trim().is_empty() {
                        (s, e) = (line_start, tail);
                    }
                    edits.push((s..e, String::new()));
                    out.lines.push(format!("{}: remove {name} ({})", table.display(), entry.uri));
                    if entry.refs.is_some_and(|n| n > 0) {
                        out.warnings.push(format!(
                            "{name} is used by {} project file(s); they keep working from their own copies, but \"Update from library\" will not find it",
                            entry.refs.unwrap_or(0)
                        ));
                    }
                }
                Action::FixPath { new_path, .. } => {
                    let new_path = &std::path::absolute(new_path).unwrap_or_else(|_| new_path.clone());
                    let ok = match entry.kind {
                        Kind::Symbol => new_path.is_file() && new_path.extension().is_some_and(|x| x == "kicad_sym" || x == "lib"),
                        Kind::Footprint => new_path.is_dir() && new_path.extension().is_some_and(|x| x == "pretty"),
                    };
                    if !ok {
                        bail!("{} is not {}", new_path.display(), if entry.kind == Kind::Symbol { "a .kicad_sym file" } else { "a .pretty folder" });
                    }
                    let lit = uri_literal(new_path, entry.project.as_deref());
                    out.lines.push(format!("{}: {name}: path {} -> {}", table.display(), entry.uri, lit.trim_matches('"')));
                    edits.push((raw.uri_lit.clone(), lit));
                }
            }
        }
        edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
        let mut after = before.clone();
        for (range, text) in edits {
            after.replace_range(range, &text);
        }
        // The result must still be a table, with exactly the removed entries gone.
        let now = parse_table(&after).context("the edited table would not be valid; nothing was changed")?;
        let gone: BTreeSet<&str> = acts.iter().filter(|a| matches!(a, Action::Remove { .. })).map(|a| a.target().1).collect();
        let want: Vec<&str> = raws.iter().map(|r| r.name.as_str()).filter(|n| !gone.contains(n)).collect();
        if now.iter().map(|r| r.name.as_str()).collect::<Vec<_>>() != want {
            bail!("the edit of {} would change more than was chosen; nothing was changed", table.display());
        }
        out.tables.push(TablePlan { file: table.to_path_buf(), before, after });
    }
    Ok(out)
}

/// Writes the plan. Returns the backup files made. Nothing is written if KiCad is running, libraries
/// are queued for its tables, or a table has changed since the plan was made. `check_kicad` is false
/// only when the tables aren't the real KiCad's (the CLI's `KLM_KICAD_CONFIG` override, tests).
pub fn apply(plan: &Plan, check_kicad: bool) -> Result<Vec<PathBuf>> {
    if check_kicad && kicad_env::kicad_running_now() {
        bail!("KiCad is running; close it first, since it would overwrite changes to its library lists");
    }
    if check_kicad && kicad_env::pending_count() > 0 {
        bail!("libraries are queued to be added to KiCad's tables; relink first so they don't undo this cleanup");
    }
    for t in &plan.tables {
        if std::fs::read_to_string(&t.file).ok().as_deref() != Some(&t.before) {
            bail!("{} changed since the preview; scan again", t.file.display());
        }
    }
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut backups: Vec<PathBuf> = vec![];
    let mut written: Vec<&TablePlan> = vec![];
    for t in &plan.tables {
        let mut bak = PathBuf::from(format!("{}.klm-cleanup-{ts}.bak", t.file.display()));
        for n in 2.. {
            if !bak.exists() {
                break;
            }
            bak = PathBuf::from(format!("{}.klm-cleanup-{ts}-{n}.bak", t.file.display()));
        }
        std::fs::write(&bak, &t.before).with_context(|| format!("backing up {}", t.file.display()))?;
        backups.push(bak.clone());
        // A table that is a link (a dotfiles repo) is written through, so the link survives.
        let write = |text: &str| {
            if std::fs::symlink_metadata(&t.file).is_ok_and(|m| m.file_type().is_symlink()) {
                std::fs::write(&t.file, text).map_err(anyhow::Error::from)
            } else {
                kicad_env::write_atomic(&t.file, text)
            }
        };
        let done = write(&t.after)
            .and_then(|()| if std::fs::read_to_string(&t.file)? == t.after { Ok(()) } else { bail!("{} doesn't read back as written", t.file.display()) });
        if let Err(e) = done {
            // Put back this table and every one already written, so a failed run changes nothing.
            for w in written.iter().copied().chain([t]) {
                let _ = std::fs::write(&w.file, &w.before);
            }
            return Err(e.context(format!(
                "every table was put back as it was; backups: {}",
                backups.iter().map(|b| b.display().to_string()).collect::<Vec<_>>().join(", ")
            )));
        }
        written.push(t);
    }
    Ok(backups)
}

// ---- moving a library into KLM ----

/// A symbol library staged for import into KLM. Dropping it deletes the staged copy, never the original.
pub struct Move {
    pub plan: batch::Plan,
    /// Why the import may not take everything; empty when it covers the whole library.
    pub caveats: Vec<String>,
    work: PathBuf,
}

impl Move {
    /// Whether every symbol of the library is in the import. Still never removes the old entry.
    pub fn complete(&self) -> bool {
        self.caveats.is_empty()
    }

    pub fn apply(&self) -> Vec<batch::ItemResult> {
        batch::apply(&self.plan, |_, _| {})
    }
}

impl Drop for Move {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.work);
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() { copy_dir(&e.path(), &dest)? } else { std::fs::copy(e.path(), dest).map(|_| ())? }
    }
    Ok(())
}

/// Stages the symbol library `entry` and the footprint libraries its symbols name, and plans importing
/// their parts into the global library. Writes nothing to it. Only symbol libraries can move: there is
/// no footprint-only import yet.
pub fn plan_move(report: &Report, entry: &Entry, rules: &Rules, global: &LibraryRoot) -> Result<Move> {
    if entry.kind != Kind::Symbol {
        bail!("only symbol libraries can be moved into KLM so far; footprints come along with the symbols that use them");
    }
    let src = entry.path.as_ref().filter(|p| p.is_file()).context("the library file is missing; fix its path first")?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let work = std::env::temp_dir().join(format!("klm-cleanup-{}-{nanos}", std::process::id()));
    let mut mv = Move { plan: batch::plan(vec![], None, None), caveats: vec![], work };
    let stage = mv.work.join("stage");
    std::fs::create_dir_all(&stage)?;
    let text = std::fs::read_to_string(src)?;
    let lib = sexpr::parse(strip_bom(&text).0).context("reading the symbol library")?;
    std::fs::write(stage.join(format!("{}.kicad_sym", crate::library::sanitize(&entry.name))), &text)?;
    let nicks: BTreeSet<&str> = lib.children("symbol").filter_map(|s| property(s, "Footprint")?.split_once(':').map(|(n, _)| n)).collect();
    for nick in nicks {
        // The symbol's own project table overrides the global one of the same name.
        let usable = |e: &&Entry| e.kind == Kind::Footprint && e.name == nick && !e.disabled && e.path.as_ref().is_some_and(|p| p.is_dir());
        let fp = report
            .entries
            .iter()
            .filter(usable)
            .find(|e| e.project == entry.project)
            .or_else(|| report.entries.iter().filter(usable).find(|e| e.project.is_none()));
        match fp.and_then(|e| e.path.as_ref()) {
            Some(dir) => copy_dir(dir, &stage.join(format!("{nick}.pretty"))).with_context(|| format!("copying {}", dir.display()))?,
            None => mv.caveats.push(format!("footprint library {nick} isn't in any scanned table, so its footprints can't come along")),
        }
    }
    let scan = foreign::scan(&stage, rules, &mv.work.join("conv"))?;
    let total = lib.children("symbol").count();
    mv.caveats.extend(scan.skipped.iter().chain(&scan.unsupported).cloned());
    if scan.parts.len() < total && scan.skipped.is_empty() {
        mv.caveats.push(format!("{} of {total} symbols are in the import", scan.parts.len()));
    }
    for f in &scan.parts {
        mv.caveats.extend(f.part.warnings.iter().map(|w| format!("{}: {w}", f.part.mpn)));
    }
    let ops = scan.parts.into_iter().map(|f| Op::ImportPart { part: f.part, category: Some(f.category), target: Target::Global, overwrite: false }).collect();
    mv.plan = batch::plan(ops, Some(global), None);
    Ok(mv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(kind: &str, libs: &[(&str, &str)]) -> String {
        let mut t = format!("({kind}_lib_table\r\n\t(version 7)\r\n");
        for (n, u) in libs {
            t += &format!("\t(lib (name \"{n}\")(type \"KiCad\")(uri \"{u}\")(options \"\")(descr \"\"))\r\n");
        }
        t + ")\r\n"
    }

    const SYM: &str = "(kicad_symbol_lib (version 20241209) (generator t)\n(symbol \"R1\" (property \"Reference\" \"R\" (at 0 0 0)) (property \"Value\" \"R1\" (at 0 0 0)) (property \"Footprint\" \"MyFp:R\" (at 0 0 0))))";

    /// A config dir with global tables, and a project with its own, holding every kind of finding.
    fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = crate::test_dir(name);
        let (cfg, prj, libs) = (base.join("cfg"), base.join("prj"), base.join("libs"));
        for d in [&cfg, &prj, &libs, &libs.join("MyFp.pretty"), &libs.join("Other.pretty")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(libs.join("good.kicad_sym"), SYM).unwrap();
        std::fs::write(libs.join("same.kicad_sym"), SYM).unwrap();
        std::fs::write(libs.join("MyFp.pretty/R.kicad_mod"), "(footprint \"R\")").unwrap();
        std::fs::write(libs.join("Other.pretty/R.kicad_mod"), "(footprint \"R\")").unwrap();
        let l = libs.to_string_lossy().replace('\\', "/");
        let mut sym = table(
            "sym",
            &[
                ("Good", &format!("{l}/good.kicad_sym")),
                ("Same", &format!("{l}/same.kicad_sym")),
                ("Gone", &format!("{l}/gone.kicad_sym")),
                ("Env", "${NOPE_VAR}/x.kicad_sym"),
                ("Stock", "${KICAD9_SYMBOL_DIR}/Device.kicad_sym"),
            ],
        );
        sym.insert(0, BOM);
        std::fs::write(cfg.join("sym-lib-table"), sym).unwrap();
        std::fs::write(cfg.join("fp-lib-table"), table("fp", &[("MyFp", &format!("{l}/MyFp.pretty")), ("Other", &format!("{l}/Other.pretty"))])).unwrap();
        std::fs::write(prj.join("p.kicad_pro"), "{}").unwrap();
        std::fs::write(prj.join("p.kicad_sch"), "(kicad_sch (lib_id \"Good:R1\"))").unwrap();
        (cfg, prj, libs)
    }

    #[test]
    fn report_finds_broken_duplicate_and_unused() {
        let (cfg, prj, _) = setup("cleanup_report");
        let r = scan(&cfg, &[prj]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let of = |name: &str, k: IssueKind| r.entries.iter().position(|e| e.name == name).is_some_and(|i| r.issues_of(i).any(|x| x.kind == k));
        assert!(of("Gone", IssueKind::Broken) && of("Env", IssueKind::Broken));
        assert!(of("Good", IssueKind::Duplicate) && of("Same", IssueKind::Duplicate) && of("MyFp", IssueKind::Duplicate));
        assert!(of("Same", IssueKind::Unused) && !of("Good", IssueKind::Unused));
        assert!(of("Stock", IssueKind::Unverifiable) || !of("Stock", IssueKind::Broken));
        assert!(!of("Stock", IssueKind::Unused) && !r.entries.iter().find(|e| e.name == "Stock").unwrap().actionable);
    }

    #[test]
    fn remove_and_fix_change_only_the_chosen_entries() {
        let (cfg, prj, libs) = setup("cleanup_apply");
        let r = scan(&cfg, &[prj]);
        let table = cfg.join("sym-lib-table");
        let before = std::fs::read_to_string(&table).unwrap();
        let acts = vec![
            Action::Remove { table: table.clone(), name: "Gone".into() },
            Action::FixPath { table: table.clone(), name: "Env".into(), new_path: libs.join("good.kicad_sym") },
        ];
        let p = plan(&r, &acts).unwrap();
        assert_eq!(std::fs::read_to_string(&table).unwrap(), before, "planning writes nothing");
        assert_eq!(p.lines.len(), 2);
        apply(&p, false).unwrap();
        let after = std::fs::read_to_string(&table).unwrap();
        assert!(after.starts_with(BOM) && after.contains("\r\n"), "BOM and CRLF kept");
        assert!(!after.contains("Gone") && !after.contains("NOPE_VAR"));
        let want = before
            .lines()
            .filter(|l| !l.contains("\"Gone\""))
            .map(|l| {
                if l.contains("\"Env\"") {
                    l.replace("${NOPE_VAR}/x.kicad_sym", &libs.join("good.kicad_sym").to_string_lossy().replace('\\', "/"))
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\r\n")
            + "\r\n";
        assert_eq!(after, want, "every other byte is as it was");
        assert!(std::fs::read_dir(&cfg).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains("klm-cleanup")), "backup made");
        assert!(libs.join("good.kicad_sym").exists(), "library files stay");
        assert!(!std::fs::read_to_string(cfg.join("fp-lib-table")).unwrap().is_empty());
    }

    #[test]
    fn working_entries_are_not_called_broken() {
        let base = crate::test_dir("cleanup_false_broken");
        let (cfg, prj) = (base.join("cfg"), base.join("prj"));
        std::fs::create_dir_all(prj.join("libs/My.pretty")).unwrap();
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(prj.join("p.kicad_pro"), "{}").unwrap();
        std::fs::write(prj.join("old.mod"), "x").unwrap();
        std::fs::write(prj.join("fp-lib-table"), "(fp_lib_table (lib (name \"Rel\")(type \"KiCad\")(uri \"libs/My.pretty\")(options \"\")(descr \"\")) (lib (name \"Leg\")(type \"Legacy\")(uri \"${KIPRJMOD}/old.mod\")(options \"\")(descr \"\")) (lib (name \"Txt\")(type \"KiCad\")(uri \"${PARTS}/x.pretty\")(options \"\")(descr \"\")) (lib (name \"Mine\")(type \"KiCad\")(uri \"${KICAD_USER_LIBS}/x.pretty\")(options \"\")(descr \"\")))").unwrap();
        let r = scan(&cfg, &[prj]);
        assert!(!r.issues.iter().any(|i| i.kind == IssueKind::Broken && i.entry < 3), "{:?}", r.issues);
        assert!(r.entries.iter().find(|e| e.name == "Mine").unwrap().actionable, "a user's own KICAD_* variable is theirs");
    }

    #[test]
    fn locked_stale_and_bad_requests_are_refused() {
        let (cfg, prj, libs) = setup("cleanup_refuse");
        let r = scan(&cfg, &[prj]);
        let table = cfg.join("sym-lib-table");
        assert!(plan(&r, &[Action::Remove { table: table.clone(), name: "Stock".into() }]).is_err());
        assert!(plan(&r, &[Action::FixPath { table: table.clone(), name: "Gone".into(), new_path: libs.join("nope.kicad_sym") }]).is_err());
        let p = plan(&r, &[Action::Remove { table: table.clone(), name: "Gone".into() }]).unwrap();
        std::fs::write(&table, std::fs::read_to_string(&table).unwrap() + " ").unwrap();
        assert!(apply(&p, false).is_err_and(|e| format!("{e:#}").contains("changed since")));
        std::fs::write(&table, "(sym_lib_table (lib").unwrap();
        assert!(!scan(&cfg, &[]).errors.is_empty(), "an unreadable table is an error, not an empty one");
    }
}
