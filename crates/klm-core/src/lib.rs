//! KiCad Library Manager core: no UI code lives here, so any front end can drive it.

pub mod alternatives;
pub mod batch;
pub mod bom;
pub mod cache;
pub mod categories;
pub mod checks;
pub mod cleanup;
pub mod compliance;
pub mod convert;
pub mod credentials;
pub mod currency;
pub mod download;
pub mod export;
pub mod fab;
pub mod fab_data;
pub mod fields;
pub mod health;
pub mod kicad_edit;
pub mod kicad_env;
pub mod library;
pub mod project_check;
pub mod matching;
pub(crate) mod model3d;
pub mod part;
pub mod preview;
pub mod rules;
pub mod search_filter;
pub mod settings;
pub mod snapshot;
pub mod standardise;
pub mod stdlib;
pub mod sexpr;
pub mod sources;
pub mod store;
pub mod symbols;

/// Fresh, empty temp folder for a test.
#[cfg(test)]
pub(crate) fn test_dir(name: &str) -> std::path::PathBuf {
    // Per process, so two test runs at once (two worktrees, two terminals) never wipe each other's folders.
    let d = std::env::temp_dir().join(format!("klm-tests-{}", std::process::id())).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
