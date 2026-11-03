//! Under a sandbox, API keys go to `<dir>/credentials.json`, not the Credential Manager. One test per process
//! because the sandbox is an environment variable.

use klm_core::{credentials, settings};

#[test]
fn keys_stay_in_the_sandbox_folder() {
    let d = std::env::temp_dir().join("klm-sandbox-creds");
    let _ = std::fs::remove_dir_all(&d);
    settings::set_sandbox(&d);
    credentials::set("klm-sandbox-test", "api_key", "fake-value").unwrap();
    assert_eq!(credentials::get("klm-sandbox-test", "api_key").as_deref(), Some("fake-value"));
    assert!(std::fs::read_to_string(d.join("credentials.json")).unwrap().contains("klm-sandbox-test.api_key"));
    credentials::clear("klm-sandbox-test", "api_key").unwrap();
    assert_eq!(credentials::get("klm-sandbox-test", "api_key"), None);
}
