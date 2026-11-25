//! Standard profiles: a named house standard kept in one JSON file, so a team shares it by copying the file.
//!
//! Profiles live in `app_dir()/standards/<name>.json`. [`BUILT_IN`] (KLC) needs no file; a file with its
//! name overrides it. Settings names the active profile and loads its rules (see `settings`).

use super::{FootprintStyle, Rules};
use crate::fields::ClassInfo;
use crate::library::sanitize;
use crate::settings::app_dir;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The profile used when none is chosen: the KiCad Library Conventions.
// shortcut: only KLC is built in; W10 adds fab/IPC profiles once a profile can hold those settings.
pub const BUILT_IN: &str = "KLC";

/// Format version written into every profile, for later migrations.
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Empty when the file has none, so `parse` names it after the file rather than overriding KLC.
    #[serde(default = "String::new")]
    pub name: String,
    pub version: u32,
    pub rules: Rules,
    /// House symbol per class name ("Resistor"), as the s-expression text of the symbol the user drew,
    /// so a shared profile works without the library it came from.
    pub templates: BTreeMap<String, String>,
    /// Footprint text, line and pin-1 style per class name, taken from a reference footprint.
    pub footprint_styles: BTreeMap<String, FootprintStyle>,
    /// Part information per class key (see `fields`); a class left out uses the built-in default.
    pub classes: BTreeMap<String, ClassInfo>,
    /// The fabs, IPC class and density the footprint and project checks hold parts to (see `fab_data`).
    pub fab: crate::fab_data::FabChoice,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: BUILT_IN.into(),
            version: VERSION,
            rules: Rules::default(),
            templates: BTreeMap::new(),
            footprint_styles: BTreeMap::new(),
            classes: BTreeMap::new(),
            fab: Default::default(),
        }
    }
}

fn dir() -> Result<PathBuf> {
    Ok(app_dir().context("no user config directory")?.join("standards"))
}

fn file(name: &str) -> Result<PathBuf> {
    let stem = sanitize(name);
    if stem.is_empty() || stem.starts_with('.') {
        bail!("\"{name}\" can't be used as a profile name");
    }
    Ok(dir()?.join(format!("{stem}.json")))
}

fn parse(text: &str, fallback_name: &str) -> Result<Profile> {
    let mut p: Profile = serde_json::from_str(text).context("not a standard profile")?;
    if p.name.trim().is_empty() {
        p.name = fallback_name.into();
    }
    Ok(p)
}

/// Every profile's name: the built-in first, then the files in the profiles folder, sorted.
pub fn list() -> Vec<String> {
    let mut names: Vec<String> = dir()
        .ok()
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            parse(&std::fs::read_to_string(&p).ok()?, &stem).ok().map(|p| p.name)
        })
        .filter(|n| n != BUILT_IN)
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.insert(0, BUILT_IN.into());
    names
}

/// The profile called `name`: its file, or the built-in when there is no file for it.
pub fn load(name: &str) -> Result<Profile> {
    let path = file(name)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text, name).with_context(|| format!("reading {}", path.display())),
        Err(_) if name == BUILT_IN => Ok(Profile::default()),
        Err(e) => Err(e).with_context(|| format!("no profile called \"{name}\"")),
    }
}

/// Writes the profile to its file. An untouched built-in needs no file and gets none.
pub fn save(p: &Profile) -> Result<()> {
    let path = file(&p.name)?;
    if (Profile { version: VERSION, ..p.clone() }) == Profile::default() && !path.exists() {
        return Ok(());
    }
    export(p, &path)
}

/// Writes the profile anywhere, e.g. to share it.
pub fn export(p: &Profile, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let p = Profile { version: VERSION, ..p.clone() };
    std::fs::write(path, serde_json::to_string_pretty(&p)?).with_context(|| format!("writing {}", path.display()))
}

/// Copies a shared profile file into the profiles folder (replacing one of the same name) and returns it.
pub fn import(path: &Path) -> Result<Profile> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let p = parse(&text, &stem).with_context(|| format!("importing {}", path.display()))?;
    save(&p)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_files_and_names_them() {
        // A hand-written file with only one rule keeps KLC values for the rest and takes the file's name.
        let p = parse(r#"{"rules":{"silk_width":0.15}}"#, "Acme").unwrap();
        assert_eq!((p.name.as_str(), p.rules.silk_width, p.rules.fab_width), ("Acme", Some(0.15), Rules::default().fab_width));
        assert!(parse("not json", "x").is_err());
        assert!(file("  ").is_err() && file("..").is_err());
        assert!(file("My standard").unwrap().ends_with("My_standard.json"));
    }

    #[test]
    fn templates_round_trip_and_older_files_load_without_them() {
        let old = parse(r#"{"name":"Old","version":1,"rules":{"silk_width":0.15}}"#, "x").unwrap();
        assert!(old.templates.is_empty() && old.footprint_styles.is_empty());
        let mut p = Profile { name: "Acme".into(), ..Profile::default() };
        p.templates.insert("Resistor".into(), "(symbol \"R\" (property \"Reference\" \"R\"))".into());
        p.footprint_styles.insert(
            "Resistor".into(),
            FootprintStyle { widths: [("F.SilkS".to_string(), 0.15)].into(), pin1: Some("(fp_circle)".into()), ..Default::default() },
        );
        assert_eq!(parse(&serde_json::to_string_pretty(&p).unwrap(), "x").unwrap(), p);
    }
}
