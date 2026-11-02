//! Command-line front end: the same libraries and settings as the GUI, for scripts and CI.
//!
//! `KLM_SETTINGS` (a settings.json) and `KLM_KICAD_CONFIG` (KiCad's config dir) override the real
//! ones; tests use them so they never touch the user's files.

use anyhow::{Context, Result, bail};
use klm_core::checks::{self, Severity};
use klm_core::library::{Check, LibraryRoot, PartStatus, Scope};
use klm_core::settings::Settings;
use klm_core::{categories, kicad_env, snapshot};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage:
  klm-cli check [--json] [--deep] [--project <dir>]
  klm-cli relink [--project <dir>]
  klm-cli history [--project <dir>]
  klm-cli rollback <id> [--project <dir>]
  klm-cli import <zip> [--category <c>] [--to global|project] [--project <dir>] [--overwrite]";

#[derive(Debug, PartialEq)]
enum Command {
    Check { json: bool, deep: bool, project: Option<PathBuf> },
    Relink { project: Option<PathBuf> },
    History { project: Option<PathBuf> },
    Rollback { id: String, project: Option<PathBuf> },
    Import { zip: PathBuf, category: Option<String>, to_project: bool, project: Option<PathBuf>, overwrite: bool },
}

fn parse(args: &[String]) -> Result<Command> {
    let (name, rest) = args.split_first().context("missing command")?;
    let (mut json, mut deep, mut overwrite) = (false, false, false);
    let (mut project, mut category, mut to, mut zip) = (None, None, None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().cloned().with_context(|| format!("{flag} needs a value"));
        match a.as_str() {
            "--json" => json = true,
            "--deep" => deep = true,
            "--overwrite" => overwrite = true,
            "--project" => project = Some(PathBuf::from(value(a)?)),
            "--category" => category = Some(value(a)?),
            "--to" => to = Some(value(a)?),
            _ if a.starts_with("--") => bail!("unknown option {a}"),
            _ if zip.is_none() => zip = Some(PathBuf::from(a)),
            _ => bail!("unexpected argument {a}"),
        }
    }
    // Options only some commands take are rejected on the others, so a typo doesn't silently do nothing.
    let only_for = |ok: bool, what: &str| if ok { Ok(()) } else { bail!("{what} is not valid for {name}") };
    match name.as_str() {
        "check" | "relink" => {
            only_for(zip.is_none() && category.is_none() && to.is_none() && !overwrite, "that argument")?;
            if name == "check" {
                Ok(Command::Check { json, deep, project })
            } else {
                only_for(!json && !deep, "--json or --deep")?;
                Ok(Command::Relink { project })
            }
        }
        "history" | "rollback" => {
            only_for(category.is_none() && to.is_none() && !overwrite && !json && !deep, "that option")?;
            if name == "history" {
                only_for(zip.is_none(), "that argument")?;
                Ok(Command::History { project })
            } else {
                Ok(Command::Rollback { id: zip.context("rollback needs a snapshot id from `history`")?.to_string_lossy().into(), project })
            }
        }
        "import" => {
            only_for(!json && !deep, "--json or --deep")?;
            let to_project = match to.as_deref() {
                None | Some("global") => false,
                Some("project") => true,
                Some(other) => bail!("--to must be global or project, not {other}"),
            };
            Ok(Command::Import { zip: zip.context("import needs a ZIP file")?, category, to_project, project, overwrite })
        }
        _ => bail!("unknown command {name}"),
    }
}

fn settings() -> Result<Settings> {
    let s: Settings = match std::env::var_os("KLM_SETTINGS") {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(&p).with_context(|| format!("reading {}", Path::new(&p).display()))?)?,
        None => {
            klm_core::snapshot::set_store_path(klm_core::store::Store::default_path());
            Settings::load()
        }
    };
    kicad_env::set_kicad_cli_override(s.kicad_cli.clone());
    Ok(s)
}

fn kicad_config() -> Result<PathBuf> {
    std::env::var_os("KLM_KICAD_CONFIG").map(PathBuf::from).map_or_else(kicad_env::config_dir, Ok)
}

/// KiCad rewrites its library tables when it closes, which would undo our edits.
fn ensure_kicad_closed() -> Result<()> {
    // An overridden config dir isn't the real KiCad's, so it can't be clobbered.
    if std::env::var_os("KLM_KICAD_CONFIG").is_none() && kicad_env::kicad_running() {
        bail!("KiCad is running; close it first, since it would overwrite changes to its library list");
    }
    Ok(())
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

fn relink(project: Option<&Path>) -> Result<()> {
    let roots = roots(&settings()?, project)?;
    ensure_kicad_closed()?;
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
    Ok(())
}

fn history(project: Option<&Path>) -> Result<()> {
    for root in roots(&settings()?, project)? {
        println!("{}", root.dir.display());
        for s in snapshot::list(&root) {
            println!("  {}  {:>9} bytes  {}", s.id, s.bytes, s.label);
        }
    }
    Ok(())
}

/// Restores the library that holds snapshot `id` to the state before that change.
fn rollback(id: &str, project: Option<&Path>) -> Result<()> {
    let root = roots(&settings()?, project)?.into_iter().find(|r| snapshot::list(r).iter().any(|s| s.id == id)).with_context(|| format!("no snapshot {id}; see `klm-cli history`"))?;
    ensure_kicad_closed()?;
    snapshot::rollback(&root, id)?;
    println!("Rolled {} back to snapshot {id}", root.dir.display());
    Ok(())
}

fn import(zip: &Path, category: Option<String>, to_project: bool, project: Option<PathBuf>, overwrite: bool) -> Result<()> {
    let s = settings()?;
    let root = if to_project {
        let dir = project.or(s.last_project.clone()).context("no project: pass --project <dir>")?;
        LibraryRoot::project(&dir)
    } else {
        global_root(&s)?
    };
    let work = std::env::temp_dir().join(format!("klm-cli-{}", std::process::id()));
    let result = import_in(&root, zip, category, overwrite, &work);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn import_in(root: &LibraryRoot, zip: &Path, category: Option<String>, overwrite: bool, work: &Path) -> Result<()> {
    let part = klm_core::sources::zip_import::read_zip(zip, work)?.with_context(|| format!("{} holds no KiCad symbol", zip.display()))?;
    let category = category.unwrap_or_else(|| categories::guess(&part.category_hint).to_string());
    if root.needs_link(&category) {
        ensure_kicad_closed()?;
    }
    root.add_part(&part, &category, overwrite)?;
    println!("Imported {} into {}", part.mpn, root.nickname(&category));
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse(&args).and_then(|cmd| match cmd {
        Command::Check { json, deep, project } => check(json, deep, project.as_deref()),
        Command::Relink { project } => relink(project.as_deref()).map(|_| ExitCode::SUCCESS),
        Command::History { project } => history(project.as_deref()).map(|_| ExitCode::SUCCESS),
        Command::Rollback { id, project } => rollback(&id, project.as_deref()).map(|_| ExitCode::SUCCESS),
        Command::Import { zip, category, to_project, project, overwrite } => {
            import(&zip, category, to_project, project, overwrite).map(|_| ExitCode::SUCCESS)
        }
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
        assert_eq!(parse(&args("history")).unwrap(), Command::History { project: None });
        assert_eq!(parse(&args("rollback 0001 --project D:/p")).unwrap(), Command::Rollback { id: "0001".into(), project: Some("D:/p".into()) });
        assert!(parse(&args("rollback")).is_err());
        assert_eq!(
            parse(&args("import a.zip --category ICs --to project --overwrite")).unwrap(),
            Command::Import { zip: "a.zip".into(), category: Some("ICs".into()), to_project: true, project: None, overwrite: true }
        );
        assert_eq!(
            parse(&args("import --to global a.zip")).unwrap(),
            Command::Import { zip: "a.zip".into(), category: None, to_project: false, project: None, overwrite: false }
        );
    }

    #[test]
    fn rejects_bad_arguments() {
        for bad in ["", "frobnicate", "import", "import a.zip --to nowhere", "check --bogus", "check a.zip", "relink --json", "relink --deep", "import a.zip --deep", "import a.zip --json", "check --project", "import a.zip b.zip"] {
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
