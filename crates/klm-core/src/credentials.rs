//! API credentials live in the OS credential store (Windows Credential Manager), never in
//! `settings.json`, logs or error text.

use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE: &str = "KiCAD-Library-Manager";

pub struct SourceInfo {
    pub name: &'static str,
    /// (key, label) of each credential the source needs.
    pub fields: &'static [(&'static str, &'static str)],
    /// False for slots that are only stored (SnapMagic has no source yet).
    pub searchable: bool,
}

const ID_SECRET: &[(&str, &str)] = &[("client_id", "Client ID"), ("client_secret", "Client secret")];

pub const SOURCES: &[SourceInfo] = &[
    SourceInfo { name: "LCSC", fields: &[], searchable: true },
    SourceInfo { name: "DigiKey", fields: ID_SECRET, searchable: true },
    SourceInfo { name: "Mouser", fields: &[("api_key", "API key")], searchable: true },
    SourceInfo { name: "Nexar", fields: ID_SECRET, searchable: true },
    SourceInfo { name: "SnapMagic", fields: &[("api_key", "API key")], searchable: false },
];

fn entry(source: &str, field: &str) -> Result<Entry> {
    Entry::new(SERVICE, &format!("{source}.{field}")).context("credential store unavailable")
}

// Under a sandbox (`KLM_CONFIG_DIR`) keys live in plain text in `<dir>/credentials.json`, never in the real
// Credential Manager. Reads fall back to the real store only when `KLM_USE_REAL_KEYS=1` (live API checks).
type Sandbox = std::collections::BTreeMap<String, String>;

fn sandbox_file() -> Option<std::path::PathBuf> {
    Some(crate::settings::sandbox_dir()?.join("credentials.json"))
}

fn sandbox_read(f: &std::path::Path) -> Sandbox {
    std::fs::read_to_string(f).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn sandbox_write(f: &std::path::Path, edit: impl FnOnce(&mut Sandbox)) -> Result<()> {
    let mut m = sandbox_read(f);
    edit(&mut m);
    std::fs::create_dir_all(f.parent().unwrap())?;
    std::fs::write(f, serde_json::to_string_pretty(&m)?).context("could not store credential")
}

pub fn get(source: &str, field: &str) -> Option<String> {
    if let Some(f) = sandbox_file() {
        let v = sandbox_read(&f).remove(&format!("{source}.{field}")).filter(|v| !v.is_empty());
        if v.is_some() || std::env::var_os("KLM_USE_REAL_KEYS").is_none_or(|v| v != "1") {
            return v;
        }
    }
    entry(source, field).ok()?.get_password().ok().filter(|v| !v.is_empty())
}

pub fn set(source: &str, field: &str, value: &str) -> Result<()> {
    if let Some(f) = sandbox_file() {
        return sandbox_write(&f, |m| drop(m.insert(format!("{source}.{field}"), value.into())));
    }
    entry(source, field)?.set_password(value).context("could not store credential")
}

pub fn clear(source: &str, field: &str) -> Result<()> {
    if let Some(f) = sandbox_file() {
        return sandbox_write(&f, |m| drop(m.remove(&format!("{source}.{field}"))));
    }
    match entry(source, field)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("could not clear credential"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_read_clear_round_trip() {
        let (s, f) = ("klm-test-source", "api_key");
        set(s, f, "secret-value").unwrap();
        assert_eq!(get(s, f).as_deref(), Some("secret-value"));
        clear(s, f).unwrap();
        assert_eq!(get(s, f), None);
        clear(s, f).unwrap(); // clearing an absent key is not an error
    }
}
