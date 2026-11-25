//! Search-result thumbnails, downloaded once and kept under the config dir. Blocking: call from a
//! worker thread.

use super::http;
use anyhow::{Result, bail};
use std::path::PathBuf;

const MAX_BYTES: usize = 2_000_000;

fn dir() -> Option<PathBuf> {
    Some(crate::settings::app_dir()?.join("thumbs"))
}

/// FNV-1a, because `DefaultHasher` may change between Rust versions and orphan the cache.
fn file_name(url: &str) -> String {
    let h = url.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    // The extension tells the GUI's image loader which decoder to use.
    let ext = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| matches!(e.as_str(), "jpg" | "jpeg" | "png"))
        .unwrap_or_else(|| "jpg".into());
    format!("{h:016x}.{ext}")
}

/// The local file for `url`, downloading it first if it is not cached yet.
pub fn get(url: &str) -> Result<PathBuf> {
    let Some(dir) = dir() else { bail!("no config directory") };
    let path = dir.join(file_name(url));
    if path.exists() {
        return Ok(path);
    }
    if !url.starts_with("https://") {
        bail!("not an https image link");
    }
    let resp = http::client().get(url).send().map_err(reqwest::Error::without_url)?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }
    let bytes = resp.bytes().map_err(reqwest::Error::without_url)?;
    if bytes.len() > MAX_BYTES {
        bail!("image too large");
    }
    std::fs::create_dir_all(&dir)?;
    // Written under a temp name so a half-written file is never mistaken for a cached image.
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_stable_and_keep_the_image_type() {
        assert_eq!(file_name("https://a/b.PNG?x=1"), file_name("https://a/b.PNG?x=1"));
        assert!(file_name("https://a/b.PNG?x=1").ends_with(".png"));
        assert!(file_name("https://a/image").ends_with(".jpg"));
        assert_ne!(file_name("https://a/1.jpg"), file_name("https://a/2.jpg"));
    }
}
