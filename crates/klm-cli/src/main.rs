//! Command-line front end: the same libraries and settings as the GUI, for scripts and CI.
//!
//! `KLM_SETTINGS` (a settings.json) and `KLM_KICAD_CONFIG` (KiCad's config dir) override the real
//! ones; tests use them so they never touch the user's files.

use anyhow::{Context, Result, bail};
use klm_core::batch::{self, Op, Target};
use klm_core::checks::{self, Severity};
use klm_core::cleanup;
use klm_core::library::{Check, LibraryRoot, PartStatus, Scope};
use klm_core::settings::Settings;
use klm_core::{kicad_env, snapshot, standardise};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage:
  klm-cli check [--json] [--deep] [--project <dir>]
  klm-cli relink [--project <dir>]
  klm-cli history [--json] [--project <dir>]
  klm-cli rollback <id> [--project <dir>]
  klm-cli standardise [--project <dir>] [--dry-run]
  klm-cli restyle <category> <part> [--project <dir>] [--dry-run]
  klm-cli health [--json | --csv] [--project <dir>]
  klm-cli cleanup [--project <dir>]... [--remove sym:<lib>|fp:<lib>]... [--fix sym:<lib>=<path>]... [--move sym:<lib>]... [--apply]
  klm-cli import <zip>... [--category <c>] [--to global|project|both] [--project <dir>] [--overwrite] [--dry-run]
  klm-cli export <out.zip> [<part>...] [--category <c>] [--project <dir>]
  --config <dir> (or KLM_CONFIG_DIR) keeps all of KLM's files, KiCad's library tables and API keys in <dir>";

#[derive(Debug, PartialEq)]
enum Command {
    Check { json: bool, deep: bool, project: Option<PathBuf> },
    Relink { project: Option<PathBuf> },
    History { json: bool, project: Option<PathBuf> },
    Rollback { id: String, project: Option<PathBuf> },
    Standardise { project: Option<PathBuf>, dry_run: bool },
    Restyle { category: String, part: String, project: Option<PathBuf>, dry_run: bool },
    Health { json: bool, csv: bool, project: Option<PathBuf> },
    Cleanup { projects: Vec<PathBuf>, remove: Vec<String>, fix: Vec<String>, moves: Vec<String>, apply: bool },
    Import { zips: Vec<PathBuf>, category: Option<String>, to: Target, project: Option<PathBuf>, overwrite: bool, dry_run: bool },
    Export { zip: PathBuf, parts: Vec<String>, category: Option<String>, project: Option<PathBuf> },
}

fn parse_cleanup(args: &[String]) -> Result<Command> {
    let (mut projects, mut remove, mut fix, mut moves, mut apply) = (vec![], vec![], vec![], vec![], false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--apply" => apply = true,
            "--project" => projects.push(PathBuf::from(value()?)),
            "--remove" => remove.push(value()?),
            "--fix" => fix.push(value()?),
            "--move" => moves.push(value()?),
            _ => bail!("{a} is not valid for cleanup"),
        }
    }
    Ok(Command::Cleanup { projects, remove, fix, moves, apply })
}

fn parse(args: &[String]) -> Result<Command> {
    let (name, rest) = args.split_first().context("missing command")?;
    if name == "cleanup" {
        return parse_cleanup(rest);
    }
    let (mut csv, mut json, mut deep, mut overwrite, mut dry_run) = (false, false, false, false, false);
    let (mut project, mut category, mut to, mut zips) = (None, None, None, vec![]);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().cloned().with_context(|| format!("{flag} needs a value"));
        match a.as_str() {
            "--json" => json = true,
            "--csv" => csv = true,
            "--deep" => deep = true,
            "--overwrite" => overwrite = true,
            "--dry-run" => dry_run = true,
            "--project" => project = Some(PathBuf::from(value(a)?)),
            "--category" => category = Some(value(a)?),
            "--to" => to = Some(value(a)?),
            _ if a.starts_with("--") => bail!("unknown option {a}"),
            _ => zips.push(PathBuf::from(a)),
        }
    }
    // Options only some commands take are rejected on the others, so a typo doesn't silently do nothing.
    let only_for = |ok: bool, what: &str| if ok { Ok(()) } else { bail!("{what} is not valid for {name}") };
    if csv {
        only_for(name == "health" && !json, "--csv")?;
    }
    match name.as_str() {
        "health" => {
            only_for(zips.is_empty() && category.is_none() && to.is_none() && !overwrite && !deep && !dry_run, "that argument")?;
            Ok(Command::Health { json, csv, project })
        }
        "check" | "relink" => {
            only_for(zips.is_empty() && category.is_none() && to.is_none() && !overwrite && !dry_run, "that argument")?;
            if name == "check" {
                Ok(Command::Check { json, deep, project })
            } else {
                only_for(!json && !deep, "--json or --deep")?;
                Ok(Command::Relink { project })
            }
        }
        "history" | "rollback" => {
            only_for(category.is_none() && to.is_none() && !overwrite && !deep && !dry_run, "that option")?;
            if name == "history" {
                only_for(zips.is_empty(), "that argument")?;
                Ok(Command::History { json, project })
            } else {
                only_for(!json, "--json")?;
                // The positional argument is the snapshot id here, not a ZIP.
                only_for(zips.len() <= 1, "more than one snapshot id")?;
                let id = zips.pop().context("rollback needs a snapshot id from `history`")?;
                Ok(Command::Rollback { id: id.to_string_lossy().into(), project })
            }
        }
        "standardise" => {
            only_for(zips.is_empty() && category.is_none() && to.is_none() && !overwrite && !json && !deep, "that argument")?;
            Ok(Command::Standardise { project, dry_run })
        }
        "restyle" => {
            only_for(category.is_none() && to.is_none() && !overwrite && !json && !deep, "that option")?;
            // Positional arguments: the category library, then the part.
            let [cat, part]: [PathBuf; 2] = zips.try_into().map_err(|_| anyhow::anyhow!("restyle needs a category and a part name"))?;
            Ok(Command::Restyle { category: cat.to_string_lossy().into(), part: part.to_string_lossy().into(), project, dry_run })
        }
        "import" => {
            only_for(!json && !deep, "--json or --deep")?;
            let to = match to.as_deref() {
                None | Some("global") => Target::Global,
                Some("project") => Target::Project,
                Some("both") => Target::Both,
                Some(other) => bail!("--to must be global, project or both, not {other}"),
            };
            if zips.is_empty() {
                bail!("import needs a ZIP file");
            }
            Ok(Command::Import { zips, category, to, project, overwrite, dry_run })
        }
        "export" => {
            only_for(to.is_none() && !overwrite && !json && !deep && !dry_run, "that option")?;
            let mut it = zips.into_iter();
            let zip = it.next().context("export needs an output ZIP path")?;
            Ok(Command::Export { zip, parts: it.map(|p| p.to_string_lossy().into()).collect(), category, project })
        }
        _ => bail!("unknown command {name}"),
    }
}

/// `--config <dir>` (anywhere in the arguments) is the same as setting `KLM_CONFIG_DIR`; returns the other arguments.
fn take_config_arg(args: Vec<String>) -> Result<Vec<String>> {
    let Some(i) = args.iter().position(|a| a == "--config") else { return Ok(args) };
    let dir = args.get(i + 1).context("--config needs a folder")?;
    klm_core::settings::set_sandbox(Path::new(dir));
    Ok(args.iter().enumerate().filter(|(j, _)| *j != i && *j != i + 1).map(|(_, a)| a.clone()).collect())
}

fn settings() -> Result<Settings> {
    let s: Settings = match std::env::var_os("KLM_SETTINGS") {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(&p).with_context(|| format!("reading {}", Path::new(&p).display()))?)?,
        None => {
            klm_core::snapshot::set_store_path(klm_core::store::Store::default_path());
            kicad_env::set_pending_queue(kicad_env::default_queue_path());
            Settings::load()
        }
    };
    kicad_env::set_kicad_cli_override(s.kicad_cli.clone());
    klm_core::snapshot::set_policy(&s.history);
    klm_core::symbols::configure(&s);
    Ok(s)
}

fn kicad_config() -> Result<PathBuf> {
    std::env::var_os("KLM_KICAD_CONFIG").map(PathBuf::from).map_or_else(kicad_env::config_dir, Ok)
}

/// For commands that add libraries; relink and rollback instead queue a re-check for the GUI to run once KiCad closes.
/// KiCad rewrites its library tables when it closes, which would undo our edits.
fn ensure_kicad_closed() -> Result<()> {
    // An overridden config dir isn't the real KiCad's, so it can't be clobbered.
    if std::env::var_os("KLM_KICAD_CONFIG").is_none() && kicad_env::kicad_running() {
        bail!("KiCad is running; close it first, since it would overwrite changes to its library list");
    }
    Ok(())
}

/// Table edits made with KiCad open are queued; the GUI repeats them once KiCad has closed.
fn note_if_kicad_open() {
    if kicad_env::pending_count() > 0 && kicad_env::kicad_running() {
        println!("KiCad is open: its library list is re-checked the next time KLM's window runs after KiCad closes.");
    }
}

fn global_root(s: &Settings) -> Result<LibraryRoot> {
    let dir = s.global_root.clone().context("no global library set; choose one in the GUI's Settings")?;
    Ok(LibraryRoot::global(dir, kicad_config()?))
}

/// The global library when one is set, plus the project's if given.
fn roots(s: &Settings, project: Option<&Path>) -> Result<Vec<LibraryRoot>> {
    let mut roots: Vec<_> = s.global_root.is_some().then(|| global_root(s)).transpose()?.into_iter().collect();
    roots.extend(project.map(LibraryRoot::project));
    if roots.is_empty() {
        bail!("no global library set; choose one in the GUI's Settings or pass --project");
    }
    Ok(roots)
}

// Same rule as the GUI's Library tab ("Only incomplete").
fn incomplete(p: &PartStatus) -> bool {
    let bad = |c: Check| matches!(c, Check::Missing | Check::Broken);
    bad(p.footprint) || bad(p.model) || bad(p.datasheet) || p.sim == Check::Broken || !p.in_kicad
}

fn word(c: Check) -> String {
    format!("{c:?}").to_lowercase()
}

fn check(json: bool, deep: bool, project: Option<&Path>) -> Result<ExitCode> {
    let mut rows = vec![];
    let mut failed = false;
    for root in roots(&settings()?, project)? {
        let mut issues = if deep { checks::check_library(&root) } else { Default::default() };
        for p in root.list_parts() {
            let issues = issues.remove(&(p.category.clone(), p.name.clone())).unwrap_or_default();
            let bad = incomplete(&p);
            // Warnings from the heuristics don't fail a CI run; errors do.
            failed |= bad || issues.iter().any(|i| i.severity == Severity::Error);
            rows.push((root.scope, p, bad, issues));
        }
    }
    let scope = |s: Scope| if s == Scope::Global { "global" } else { "project" };
    let severity = |i: &checks::Issue| if i.severity == Severity::Error { "error" } else { "warning" };
    if json {
        let out: Vec<_> = rows
            .iter()
            .map(|(s, p, bad, issues)| {
                let mut row = json!({
                    "scope": scope(*s), "category": p.category, "name": p.name, "mpn": p.mpn, "manufacturer": p.manufacturer,
                    "footprint": word(p.footprint), "model": word(p.model), "datasheet": word(p.datasheet), "sim": word(p.sim),
                    "in_kicad": p.in_kicad, "incomplete": bad,
                });
                if deep {
                    row["issues"] = issues.iter().map(|i| json!({ "severity": severity(i), "message": i.message })).collect();
                }
                row
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        for (s, p, bad, issues) in &rows {
            println!(
                "{:<8} {}/{}: footprint {}, 3D {}, datasheet {}, sim {}, in KiCad {}{}",
                scope(*s), p.category, p.mpn, word(p.footprint), word(p.model), word(p.datasheet), word(p.sim),
                if p.in_kicad { "yes" } else { "no" }, if *bad { "  <- incomplete" } else { "" },
            );
            for i in issues {
                println!("           {}: {}", severity(i), i.message);
            }
        }
        println!("{} parts, {} incomplete", rows.len(), rows.iter().filter(|r| r.2).count());
        if deep {
            let count = |sev| rows.iter().flat_map(|r| &r.3).filter(|i| i.severity == sev).count();
            println!("{} errors, {} warnings from the part checks", count(Severity::Error), count(Severity::Warning));
        }
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Prints the health report of the project (text, JSON or CSV). Exits non-zero when it has an error.
fn health(json: bool, csv: bool, project: Option<PathBuf>) -> Result<ExitCode> {
    let s = settings()?;
    let dir = project.or(s.last_project.clone()).context("no project: pass --project <dir>")?;
    let findings = klm_core::health::scan(&dir, kicad_config().ok().as_deref());
    if json {
        println!("{}", klm_core::health::to_json(&findings));
    } else if csv {
        print!("{}", klm_core::health::to_csv(&findings));
    } else {
        for f in &findings {
            println!("{:<8} {:<17} {}: {}", if f.severity == Severity::Error { "error" } else { "warning" }, f.kind, f.item, f.message);
        }
        println!("{} findings", findings.len());
    }
    Ok(if klm_core::health::has_errors(&findings) { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

fn relink(project: Option<&Path>) -> Result<()> {
    let roots = roots(&settings()?, project)?;
    let mut any = false;
    for root in roots {
        let fixed = root.fix_old_paths()?;
        if fixed > 0 {
            println!("Replaced ${{KLM_LIB}} with {} in {fixed} files", root.dir.display());
            any = true;
        }
        for cat in root.unlinked() {
            root.link(&cat)?;
            println!("Re-linked {} in KiCad", root.nickname(&cat));
            any = true;
        }
    }
    if !any {
        println!("All KLM libraries are already in KiCad's library list.");
    }
    note_if_kicad_open();
    Ok(())
}

fn history(json: bool, project: Option<&Path>) -> Result<()> {
    let mut out = vec![];
    for root in roots(&settings()?, project)? {
        let list = snapshot::list(&root);
        if json {
            let snaps: Vec<_> = list.iter().map(|s| json!({ "id": s.id, "kind": s.kind_name(), "version": s.version, "bytes": s.bytes, "label": s.label, "time": s.ts })).collect();
            out.push(json!({ "library": root.dir.display().to_string(), "snapshots": snaps }));
        } else {
            println!("{}", root.dir.display());
            for s in list {
                println!("  {}  {:<8}  {:>9} bytes  {}", s.id, s.kind_name(), s.bytes, s.label);
            }
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    }
    Ok(())
}

/// Restores the library that holds snapshot `id` to the state before that change.
fn rollback(id: &str, project: Option<&Path>) -> Result<()> {
    let root = roots(&settings()?, project)?.into_iter().find(|r| snapshot::list(r).iter().any(|s| s.id == id)).with_context(|| format!("no snapshot {id}; see `klm-cli history`"))?;
    snapshot::rollback(&root, id)?;
    println!("Rolled {} back to snapshot {id}", root.dir.display());
    note_if_kicad_open();
    Ok(())
}

/// Prints what the Settings' standards rules would change in each library, then applies it unless `dry_run`.
fn standardise(project: Option<&Path>, dry_run: bool) -> Result<()> {
    let s = settings()?;
    for root in roots(&s, project)? {
        let plan = standardise::plan(&root, &s.standards);
        print!("{plan}");
        if !dry_run {
            println!("Standardised {} file(s) in {}", standardise::apply(&plan)?, root.dir.display());
        }
    }
    if dry_run {
        println!("Dry run: nothing was changed.");
    }
    Ok(())
}

/// Redraws one part's symbol in the house style (after a snapshot), or with `dry_run` only says what would happen.
fn restyle(category: &str, part: &str, project: Option<&Path>, dry_run: bool) -> Result<()> {
    let s = settings()?;
    let root = roots(&s, project)?.into_iter().find(|r| r.has_part(category, part)).with_context(|| format!("{part} is not in category {category}"))?;
    if !dry_run {
        ensure_kicad_closed()?;
    }
    let done = klm_core::symbols::restyle_part(&root, category, part, &s.rules, &s.standards, !dry_run)?;
    println!("{part} in {}: {}{}", root.nickname(category), done.changes.join(", "), if dry_run && done.changed { " (dry run, nothing written)" } else { "" });
    Ok(())
}

/// `sym:Name` / `fp:Name` → the one scanned entry of that name.
fn pick<'a>(report: &'a cleanup::Report, sel: &str) -> Result<&'a cleanup::Entry> {
    let (kind, name) = sel.split_once(':').context("name a library as sym:<name> or fp:<name>")?;
    let kind = match kind {
        "sym" => cleanup::Kind::Symbol,
        "fp" => cleanup::Kind::Footprint,
        _ => bail!("{sel}: use sym: or fp:"),
    };
    let mut found = report.entries.iter().filter(|e| e.kind == kind && e.name == name);
    match (found.next(), found.next()) {
        (Some(e), None) => Ok(e),
        (None, _) => bail!("{sel}: no such library in the scanned tables"),
        _ => bail!("{sel}: listed in more than one table, so it can't be picked by name"),
    }
}

/// Reports duplicate, broken and unused library-table entries. Chosen actions are only previewed
/// unless `apply`; every table edited is backed up first and library files are never deleted.
fn cleanup_cmd(projects: Vec<PathBuf>, remove: Vec<String>, fix: Vec<String>, moves: Vec<String>, apply: bool) -> Result<ExitCode> {
    let s = settings()?;
    let projects = if projects.is_empty() { s.last_project.clone().into_iter().collect() } else { projects };
    let projects: Vec<PathBuf> = projects.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect();
    let config = kicad_config()?;
    let report = cleanup::scan(&config, &projects);
    let acting = !(remove.is_empty() && fix.is_empty() && moves.is_empty());
    if !acting {
        for (i, e) in report.entries.iter().enumerate() {
            let tag = format!("{}:{}", if e.kind == cleanup::Kind::Symbol { "sym" } else { "fp" }, e.name);
            for x in report.issues_of(i) {
                println!("{:<12} {tag}: {} [{}]", format!("{:?}", x.kind).to_lowercase(), x.message, e.locked.unwrap_or("can be changed"));
            }
        }
        report.errors.iter().for_each(|e| println!("error: {e}"));
        report.other_configs.iter().for_each(|c| println!("not scanned: {}", c.display()));
        println!("{} libraries in {} projects, {} findings", report.entries.len(), report.projects.len(), report.issues.len());
        return Ok(if report.errors.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE });
    }
    let mut actions = vec![];
    for sel in &remove {
        let e = pick(&report, sel)?;
        actions.push(cleanup::Action::Remove { table: e.table.clone(), name: e.name.clone() });
    }
    for spec in &fix {
        let (sel, path) = spec.split_once('=').context("--fix needs <sym:name>=<path>")?;
        let e = pick(&report, sel)?;
        actions.push(cleanup::Action::FixPath { table: e.table.clone(), name: e.name.clone(), new_path: PathBuf::from(path) });
    }
    let plan = cleanup::plan(&report, &actions)?;
    plan.lines.iter().for_each(|l| println!("{l}"));
    plan.warnings.iter().for_each(|w| println!("warning: {w}"));
    let global = if moves.is_empty() { None } else { Some(global_root(&s)?) };
    let mut staged = vec![];
    for sel in &moves {
        let (e, g) = (pick(&report, sel)?, global.as_ref().context("no global library")?);
        let mv = cleanup::plan_move(&report, e, &s.rules, g)?;
        print!("move {sel} into KLM:
{}", mv.plan);
        mv.caveats.iter().for_each(|c| println!("  not everything comes along: {c}"));
        staged.push(mv);
    }
    if !apply {
        println!("Preview only; nothing was changed. Add --apply to do it.");
        return Ok(ExitCode::SUCCESS);
    }
    // An overridden config dir isn't the real KiCad's, but a project's tables still are.
    let real = std::env::var_os("KLM_KICAD_CONFIG").is_none() || plan.tables.iter().any(|t| !t.file.starts_with(&config));
    if !plan.tables.is_empty() {
        for b in cleanup::apply(&plan, real)? {
            println!("backup: {}", b.display());
        }
    }
    let mut failed = false;
    for mv in &staged {
        for r in mv.apply() {
            r.notes.iter().for_each(|n| println!("{n}"));
            if let Some(e) = &r.error {
                eprintln!("error: {}: {e}", r.label);
                failed = true;
            }
        }
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Imports each ZIP, or with `dry_run` only prints what that would change. Exits non-zero if any ZIP fails.
fn import(zips: Vec<PathBuf>, category: Option<String>, to: Target, project: Option<PathBuf>, overwrite: bool, dry_run: bool) -> Result<ExitCode> {
    let s = settings()?;
    let global = (to != Target::Project).then(|| global_root(&s)).transpose()?;
    let project = (to != Target::Global)
        .then(|| project.or(s.last_project.clone()).map(|dir| LibraryRoot::project(&dir)).context("no project: pass --project <dir>"))
        .transpose()?;
    let ops = zips.into_iter().map(|zip| Op::ImportZip { zip, category: category.clone(), target: to, overwrite }).collect();
    let plan = batch::plan_with(ops, global.as_ref(), project.as_ref(), &s.rules);
    if dry_run {
        print!("{plan}");
        return Ok(ExitCode::SUCCESS);
    }
    if plan.adds_libraries() {
        ensure_kicad_closed()?;
    }
    let mut failed = false;
    for r in batch::apply(&plan, |_, _| {}) {
        r.notes.iter().for_each(|n| println!("{n}"));
        if let Some(e) = &r.error {
            eprintln!("error: {}: {e}", r.label);
            failed = true;
        }
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Exports the named parts (all of them, or all in `category`, when none are named) from the project's library if one is given, else the global one.
fn export(zip: &Path, names: Vec<String>, category: Option<String>, project: Option<&Path>) -> Result<()> {
    let root = roots(&settings()?, project)?.pop().context("no library")?;
    let parts: Vec<_> = root
        .list_parts()
        .into_iter()
        .filter(|p| category.as_deref().is_none_or(|c| c == p.category) && (names.is_empty() || names.contains(&p.name)))
        .map(|p| (p.category, p.name))
        .collect();
    let n = klm_core::export::export_zip(&root, &parts, zip)?;
    println!("Exported {n} part(s) to {}", zip.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = take_config_arg(args).and_then(|args| parse(&args)).and_then(|cmd| match cmd {
        Command::Check { json, deep, project } => check(json, deep, project.as_deref()),
        Command::Relink { project } => relink(project.as_deref()).map(|_| ExitCode::SUCCESS),
        Command::History { json, project } => history(json, project.as_deref()).map(|_| ExitCode::SUCCESS),
        Command::Rollback { id, project } => rollback(&id, project.as_deref()).map(|_| ExitCode::SUCCESS),
        Command::Standardise { project, dry_run } => standardise(project.as_deref(), dry_run).map(|_| ExitCode::SUCCESS),
        Command::Restyle { category, part, project, dry_run } => restyle(&category, &part, project.as_deref(), dry_run).map(|_| ExitCode::SUCCESS),
        Command::Health { json, csv, project } => health(json, csv, project),
        Command::Cleanup { projects, remove, fix, moves, apply } => cleanup_cmd(projects, remove, fix, moves, apply),
        Command::Import { zips, category, to, project, overwrite, dry_run } => import(zips, category, to, project, overwrite, dry_run),
        Command::Export { zip, parts, category, project } => export(&zip, parts, category, project.as_deref()).map(|_| ExitCode::SUCCESS),
    });
    result.unwrap_or_else(|e| {
        eprintln!("error: {e:#}\n{USAGE}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse(&args("check")).unwrap(), Command::Check { json: false, deep: false, project: None });
        assert_eq!(
            parse(&args("check --json --deep --project D:/p")).unwrap(),
            Command::Check { json: true, deep: true, project: Some("D:/p".into()) }
        );
        assert_eq!(parse(&args("relink --project D:/p")).unwrap(), Command::Relink { project: Some("D:/p".into()) });
        assert_eq!(parse(&args("health --csv --project D:/p")).unwrap(), Command::Health { json: false, csv: true, project: Some("D:/p".into()) });
        assert!(parse(&args("health --csv --json")).is_err() && parse(&args("check --csv")).is_err());
        assert_eq!(parse(&args("history")).unwrap(), Command::History { json: false, project: None });
        assert_eq!(parse(&args("history --json")).unwrap(), Command::History { json: true, project: None });
        assert!(parse(&args("rollback 0001 --json")).is_err());
        assert_eq!(parse(&args("rollback 0001 --project D:/p")).unwrap(), Command::Rollback { id: "0001".into(), project: Some("D:/p".into()) });
        assert!(parse(&args("rollback")).is_err());
        assert_eq!(parse(&args("standardise --dry-run --project D:/p")).unwrap(), Command::Standardise { project: Some("D:/p".into()), dry_run: true });
        assert_eq!(
            parse(&args("import a.zip --category ICs --to project --overwrite")).unwrap(),
            Command::Import { zips: vec!["a.zip".into()], category: Some("ICs".into()), to: Target::Project, project: None, overwrite: true, dry_run: false }
        );
        assert_eq!(
            parse(&args("import --to global a.zip")).unwrap(),
            Command::Import { zips: vec!["a.zip".into()], category: None, to: Target::Global, project: None, overwrite: false, dry_run: false }
        );
        assert_eq!(
            parse(&args("export out.zip A B --category ICs")).unwrap(),
            Command::Export { zip: "out.zip".into(), parts: vec!["A".into(), "B".into()], category: Some("ICs".into()), project: None }
        );
        assert_eq!(
            parse(&args("import a.zip b.zip --to both --dry-run")).unwrap(),
            Command::Import { zips: vec!["a.zip".into(), "b.zip".into()], category: None, to: Target::Both, project: None, overwrite: false, dry_run: true }
        );
    }

    #[test]
    fn rejects_bad_arguments() {
        assert_eq!(parse(&args("restyle Resistors RC0603 --dry-run")).unwrap(), Command::Restyle { category: "Resistors".into(), part: "RC0603".into(), project: None, dry_run: true });
        for bad in ["", "restyle Resistors", "restyle a b c", "restyle a b --json", "frobnicate", "import", "import a.zip --to nowhere", "check --bogus", "check a.zip", "relink --json", "relink --deep", "import a.zip --deep", "import a.zip --json", "check --project", "check --dry-run", "relink --dry-run", "standardise a.zip", "standardise --json"] {
            assert!(parse(&args(bad)).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn incomplete_matches_the_gui_rule() {
        let mut p = PartStatus {
            category: "ICs".into(), name: "A".into(), mpn: "A".into(), manufacturer: String::new(), description: String::new(),
            footprint: Check::Ok, model: Check::Link, datasheet: Check::Ok, sim: Check::Missing, datasheet_target: None, in_kicad: true,
        };
        assert!(!incomplete(&p), "a missing sim model alone is fine");
        p.model = Check::Broken;
        assert!(incomplete(&p));
    }
}
