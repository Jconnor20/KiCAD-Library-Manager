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

pub fn get(source: &str, field: &str) -> Option<String> {
    entry(source, field).ok()?.get_password().ok().filter(|v| !v.is_empty())
}

pub fn set(source: &str, field: &str, value: &str) -> Result<()> {
    entry(source, field)?.set_password(value).context("could not store credential")
}

pub fn clear(source: &str, field: &str) -> Result<()> {
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
