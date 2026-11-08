//! Standard profiles on disk: migration of pre-profile rules, save/load through Settings, import and export.
//! One test per process because the sandbox is an environment variable.

use klm_core::settings::{self, MIGRATED_PROFILE, Settings};
use klm_core::standardise::profile;

#[test]
fn profile_files_from_before_part_information_still_load() {
    let p: profile::Profile = serde_json::from_str(r#"{"name":"Old","version":1,"rules":{"silk_width":0.2}}"#).unwrap();
    assert!(p.classes.is_empty());
    assert_eq!(p.class("Capacitor"), klm_core::fields::default_class("Capacitor"));
}

#[test]
fn profiles_migrate_save_and_share() {
    let d = std::env::temp_dir().join(format!("klm-sandbox-profiles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    settings::set_sandbox(&d);

    // Untouched old settings stay on the built-in KLC and write no profile file.
    std::fs::write(d.join("settings.json"), r#"{"standards":{"silk_width":0.12}}"#).unwrap();
    let s = Settings::load();
    assert_eq!(s.profile_name(), profile::BUILT_IN);
    assert!(!d.join("standards").exists());

    // Customised old rules move into their own profile, and settings.json loses the old key.
    std::fs::write(d.join("settings.json"), r#"{"standards":{"silk_width":0.2,"pin1_marker":false}}"#).unwrap();
    let s = Settings::load();
    assert_eq!((s.profile_name(), s.standard.rules.silk_width, s.standard.rules.pin1_marker), (MIGRATED_PROFILE, Some(0.2), false));
    assert!(!std::fs::read_to_string(d.join("settings.json")).unwrap().contains("silk_width"));
    let again = Settings::load();
    assert_eq!((again.profile_name(), again.standard.rules.silk_width), (MIGRATED_PROFILE, Some(0.2)));
    assert_eq!(profile::list(), vec![profile::BUILT_IN.to_string(), MIGRATED_PROFILE.to_string()]);

    // Export, then import under another name: it becomes a profile Settings can switch to.
    let shared = d.join("shared.json");
    let mut p = profile::load(MIGRATED_PROFILE).unwrap();
    p.name = "Acme Medical".into();
    profile::export(&p, &shared).unwrap();
    let imported = profile::import(&shared).unwrap();
    assert_eq!(imported.rules, p.rules);
    assert!(profile::list().contains(&"Acme Medical".to_string()));

    // A broken active profile falls back to KLC and the file is left alone.
    std::fs::write(d.join("standards").join("Custom.json"), "{ broken").unwrap();
    let s = Settings::load();
    assert_eq!(s.profile_name(), profile::BUILT_IN);
    s.save().unwrap();
    assert_eq!(std::fs::read_to_string(d.join("standards").join("Custom.json")).unwrap(), "{ broken");
}
