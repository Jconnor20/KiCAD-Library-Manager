use crate::standardise::profile;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub global_root: Option<PathBuf>,
    /// Folders watched for vendor ZIPs; the OS Downloads folder when empty.
    pub watch_dirs: Vec<PathBuf>,
    /// Old single-folder setting, read once and moved into `watch_dirs` by `parse`.
    #[serde(skip_serializing)]
    downloads_dir: Option<PathBuf>,
    pub last_project: Option<PathBuf>,
    /// `kicad-cli` to use when KiCad isn't installed in the usual place; found automatically when unset.
    pub kicad_cli: Option<PathBuf>,
    /// Per-source switch by source name; a source not listed is on (it still needs credentials).
    pub source_enabled: BTreeMap<String, bool>,
    /// Currency search prices are converted to (ISO code); empty shows each source's own currency.
    pub display_currency: String,
    /// Category and naming rules.
    pub rules: crate::rules::Rules,
    /// The active standard profile (see `standardise::profile`); the built-in KLC when empty.
    pub profile: String,
    /// The active profile: loaded from and saved to its profile file, never to settings.json.
    #[serde(skip)]
    pub standard: profile::Profile,
    /// Rules from before profiles, moved into a profile of their own by `load`.
    #[serde(rename = "standards", skip_serializing)]
    legacy_standards: Option<crate::standardise::Rules>,
    /// Redraw imported symbols in the house style of their class (see `symbols`); off unless asked for.
    pub house_symbols_on_import: bool,
    /// How long distributor search results are reused, in minutes; unset is a day, 0 turns the cache off.
    pub cache_ttl_minutes: Option<u64>,
    /// Default shipping region (ISO country, e.g. "DE") for BOM stock and prices; a project can override it.
    pub shipping_region: String,
    /// Schematic field names the BOM reads part numbers from; a project can override them.
    pub bom_fields: crate::bom::BomFields,
    /// How many snapshots are kept and when versions are made on their own.
    pub history: History,
    /// SPICE simulation library folders and automatic model resolution.
    pub sim: SimSettings,
}

/// Settings for finding or generating SPICE simulation models during import.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimSettings {
    /// Automatically resolve missing SPICE models during import (exact/close match first).
    pub auto_sim: bool,
    /// Fall back to generating a SPICE model from datasheet PDF / parametric specs + similar models.
    pub auto_generate_from_datasheet: bool,
    /// Extra local folders searched for SPICE `.lib`/`.cir`/`.sub`/`.sp`/`.mod` models.
    pub spice_dirs: Vec<PathBuf>,
}

impl Default for SimSettings {
    fn default() -> Self {
        Self {
            auto_sim: true,
            auto_generate_from_datasheet: true,
            spice_dirs: Vec::new(),
        }
    }
}

/// How often a snapshot is promoted to a version without being asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoVersion {
    Off,
    Daily,
    #[default]
    Weekly,
    Monthly,
}

/// Retention of snapshots (see `snapshot`): versions are never pruned, minor snapshots are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    /// Minor snapshots kept per library.
    pub keep_minors: usize,
    /// Optional cap on the disk space of minor snapshots, in MB.
    pub max_minor_mb: Option<u64>,
    pub auto_version: AutoVersion,
}

impl Default for History {
    fn default() -> Self {
        Self { keep_minors: 10, max_minor_mb: None, auto_version: AutoVersion::default() }
    }
}

/// Name of the profile that customised pre-profile rules are moved into.
pub const MIGRATED_PROFILE: &str = "Custom";

/// Env var that moves everything KLM keeps (and KiCad's config dir lookup, and credentials) into one folder.
pub const SANDBOX_ENV: &str = "KLM_CONFIG_DIR";

/// The sandbox folder, when `KLM_CONFIG_DIR` (or `--config`) is set.
pub fn sandbox_dir() -> Option<PathBuf> {
    std::env::var_os(SANDBOX_ENV).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Makes `dir` the sandbox for this process. Call first thing in `main`, before any thread starts.
pub fn set_sandbox(dir: &Path) {
    // SAFETY: only called at startup, single-threaded.
    unsafe { std::env::set_var(SANDBOX_ENV, dir) };
}

/// Where KLM keeps settings, the part database, caches, queues and history: the one place every
/// per-user path goes through, so a sandbox redirects all of it.
pub fn app_dir() -> Option<PathBuf> {
    sandbox_dir().or_else(|| Some(dirs::config_dir()?.join("KiCAD-Library-Manager")))
}

fn path() -> Option<PathBuf> {
    Some(app_dir()?.join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        let mut s = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| Self::parse(&t))
            .unwrap_or_default();
        match s.legacy_standards.take() {
            // Customised rules from before profiles become a profile of their own, once.
            Some(old) if s.profile.is_empty() && old != crate::standardise::Rules::default() => {
                s.profile = MIGRATED_PROFILE.into();
                s.standard = profile::Profile { name: s.profile.clone(), rules: old, ..Default::default() };
                // On failure the old key stays in settings.json, so the move is tried again next start.
                let _ = s.save();
            }
            _ => match profile::load(s.profile_name()) {
                Ok(p) => s.standard = p,
                // A missing or broken profile file falls back to KLC without touching (or overwriting) the file.
                Err(_) => s.profile.clear(),
            },
        }
        s
    }

    /// Name of the active standard profile.
    pub fn profile_name(&self) -> &str {
        if self.profile.is_empty() { profile::BUILT_IN } else { &self.profile }
    }

    /// Parses a settings file, migrating the old `downloads_dir` into `watch_dirs`.
    fn parse(text: &str) -> Option<Self> {
        let mut s: Self = serde_json::from_str(text).ok()?;
        if let Some(d) = s.downloads_dir.take() {
            if !s.watch_dirs.contains(&d) {
                s.watch_dirs.push(d);
            }
        }
        Some(s)
    }

    pub fn save(&self) -> Result<()> {
        let p = path().context("no user config directory")?;
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", p.display()))?;
        profile::save(&profile::Profile { name: self.profile_name().into(), ..self.standard.clone() })
    }

    pub fn is_enabled(&self, source: &str) -> bool {
        self.source_enabled.get(source).copied().unwrap_or(true)
    }

    /// The folders to watch: the configured ones, or the OS Downloads folder.
    pub fn downloads(&self) -> Vec<PathBuf> {
        if self.watch_dirs.is_empty() { dirs::download_dir().into_iter().collect() } else { self.watch_dirs.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults_sources_on() {
        let mut s = Settings::default();
        assert!(s.is_enabled("DigiKey"));
        s.source_enabled.insert("Mouser".into(), false);
        s.watch_dirs = vec!["D:/dl".into(), "E:/x".into()];
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(!back.is_enabled("Mouser") && back.is_enabled("DigiKey"));
        assert_eq!(back.watch_dirs, s.watch_dirs);
        // An old config's single folder becomes the first watch folder.
        let old = Settings::parse(r#"{"downloads_dir":"D:/old"}"#).unwrap();
        assert_eq!(old.watch_dirs, vec![PathBuf::from("D:/old")]);
        // Old files without the new field still load.
        let old = serde_json::from_str::<Settings>(r#"{"global_root":"C:/x"}"#).unwrap();
        assert!(old.source_enabled.is_empty());
        assert_eq!((old.history.keep_minors, old.history.auto_version), (10, AutoVersion::Weekly));
    }
}
