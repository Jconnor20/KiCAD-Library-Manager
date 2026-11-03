use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

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
    /// Which standardise rules run, and their values; KLC's when unset.
    pub standards: crate::standardise::Rules,
    /// How long distributor search results are reused, in minutes; unset is a day, 0 turns the cache off.
    pub cache_ttl_minutes: Option<u64>,
    /// Default shipping region (ISO country, e.g. "DE") for BOM stock and prices; a project can override it.
    pub shipping_region: String,
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("KiCAD-Library-Manager").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| Self::parse(&t))
            .unwrap_or_default()
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
        std::fs::write(&p, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", p.display()))
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
        assert!(serde_json::from_str::<Settings>(r#"{"global_root":"C:/x"}"#).unwrap().source_enabled.is_empty());
    }
}
