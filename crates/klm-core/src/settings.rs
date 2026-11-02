use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub global_root: Option<PathBuf>,
    /// Folder watched for vendor ZIPs; the OS Downloads folder when unset.
    pub downloads_dir: Option<PathBuf>,
    pub last_project: Option<PathBuf>,
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

    pub fn downloads(&self) -> Option<PathBuf> {
        self.downloads_dir.clone().or_else(dirs::download_dir)
    }
}
