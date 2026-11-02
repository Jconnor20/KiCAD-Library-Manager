use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub global_root: Option<PathBuf>,
    /// Folder watched for vendor ZIPs; the OS Downloads folder when unset.
    pub downloads_dir: Option<PathBuf>,
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
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let p = path().context("no user config directory")?;
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", p.display()))
    }

    pub fn is_enabled(&self, source: &str) -> bool {
        self.source_enabled.get(source).copied().unwrap_or(true)
    }

    pub fn downloads(&self) -> Option<PathBuf> {
        self.downloads_dir.clone().or_else(dirs::download_dir)
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
        s.downloads_dir = Some("D:/dl".into());
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(!back.is_enabled("Mouser") && back.is_enabled("DigiKey"));
        assert_eq!(back.downloads_dir, s.downloads_dir);
        // Old files without the new field still load.
        assert!(serde_json::from_str::<Settings>(r#"{"global_root":"C:/x"}"#).unwrap().source_enabled.is_empty());
    }
}
