//! Distributor search results kept in the part store for a while, to save API calls. Any store
//! problem just means the search goes to the network; the cache is never required.

use crate::part::Part;
use crate::store::Store;
use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_TTL_MINUTES: u64 = 24 * 60;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Cached results for `(source, query)` if they are younger than `ttl_secs` at time `now`.
pub fn get(store: &Store, source: &str, query: &str, ttl_secs: i64, now: i64) -> Option<Vec<Part>> {
    let (json, at) = store.cache_get(source, &key(query)).ok()??;
    if now - at >= ttl_secs {
        return None;
    }
    serde_json::from_str(&json).ok()
}

/// Cached results, or `fetch` (then cached). A zero TTL skips the cache entirely.
pub fn search(store: Option<&Store>, source: &str, query: &str, ttl_secs: i64, fetch: impl FnOnce() -> Result<Vec<Part>>) -> Result<Vec<Part>> {
    let Some(store) = store.filter(|_| ttl_secs > 0) else { return fetch() };
    if let Some(hit) = get(store, source, query, ttl_secs, now()) {
        return Ok(hit);
    }
    let parts = fetch()?;
    if let Ok(json) = serde_json::to_string(&parts) {
        let _ = store.cache_put(source, &key(query), &json);
    }
    Ok(parts)
}

fn key(query: &str) -> String {
    query.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn part(mpn: &str) -> Part {
        Part { mpn: mpn.into(), stock: Some(5), ..Default::default() }
    }

    #[test]
    fn miss_hit_and_expiry() {
        let s = Store::open(&crate::test_dir("cache_basic").join("parts.db")).unwrap();
        let calls = Cell::new(0);
        let run = |ttl| {
            search(Some(&s), "LCSC", "LM358", ttl, || {
                calls.set(calls.get() + 1);
                Ok(vec![part("LM358")])
            })
            .unwrap()
        };
        assert_eq!(run(60)[0].stock, Some(5)); // miss
        assert_eq!(run(60)[0].mpn, "LM358"); // hit, query key is case-insensitive
        assert_eq!(calls.get(), 1);
        let (_, at) = s.cache_get("LCSC", "lm358").unwrap().unwrap();
        assert!(get(&s, "LCSC", " LM358 ", 60, at + 59).is_some());
        assert!(get(&s, "LCSC", "LM358", 60, at + 60).is_none(), "expired");
        assert!(get(&s, "Mouser", "LM358", 60, at).is_none(), "other source misses");
        run(0); // ttl 0 bypasses the cache
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn failed_fetch_is_not_cached() {
        let s = Store::open(&crate::test_dir("cache_err").join("parts.db")).unwrap();
        assert!(search(Some(&s), "LCSC", "x", 60, || anyhow::bail!("offline")).is_err());
        assert!(s.cache_get("LCSC", "x").unwrap().is_none());
    }
}
