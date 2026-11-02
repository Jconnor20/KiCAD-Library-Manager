//! KiCad Library Manager core: no UI code lives here, so any front end can drive it.

pub mod batch;
pub mod categories;
pub mod checks;
pub mod convert;
pub mod credentials;
pub mod currency;
pub mod download;
pub mod kicad_env;
pub mod library;
pub mod matching;
pub mod part;
pub mod preview;
pub mod search_filter;
pub mod settings;
pub mod snapshot;
pub mod sexpr;
pub mod sources;
pub mod store;

/// Fresh, empty temp folder for a test.
#[cfg(test)]
pub(crate) fn test_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join("klm-tests").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
