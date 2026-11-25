//! "Is there a newer release?": one anonymous request to GitHub, nothing sent but the app's name and version.

use crate::sources::http;
use anyhow::Result;
use serde_json::Value;

const LATEST: &str = "https://api.github.com/repos/Jconnor20/KiCAD-Library-Manager/releases/latest";
const USER_AGENT: &str = concat!("KiCad-Library-Manager/", env!("CARGO_PKG_VERSION"));

/// The newest release as (version without a leading `v`, page URL). Errors when offline or when no release exists yet (404).
pub fn latest_release() -> Result<(String, String)> {
    let v: Value = http::send(http::client_with(USER_AGENT, 5).get(LATEST))?;
    let tag = http::text(&v, "/tag_name");
    let url = http::text(&v, "/html_url");
    anyhow::ensure!(!tag.is_empty() && !url.is_empty(), "release reply has no tag or page");
    Ok((tag.trim_start_matches('v').to_string(), url))
}

/// Dotted numeric versions compared part by part; a `-pre-release` suffix is ignored and anything non-numeric is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(v: &str) -> Option<Vec<u64>> {
        v.trim_start_matches('v').split('-').next()?.split('.').map(|p| p.parse().ok()).collect()
    }
    matches!((parts(latest), parts(current)), (Some(l), Some(c)) if l > c)
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_numerically() {
        assert!(is_newer("1.0.0", "0.10.0"));
        assert!(is_newer("0.10.1", "0.10.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.10.0", "0.10.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("0.10.1-rc1", "0.10.1"));
        assert!(is_newer("0.11.0-rc1", "0.10.0"));
    }

    #[test]
    fn garbage_is_never_newer() {
        assert!(!is_newer("", "0.10.0"));
        assert!(!is_newer("nightly", "0.10.0"));
        assert!(!is_newer("1.x", "0.10.0"));
        assert!(!is_newer("1.0.0", "garbage"));
    }
}
