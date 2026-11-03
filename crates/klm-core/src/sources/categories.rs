//! Distributor category trees (DigiKey) for browsing the Search tab by part type. The
//! requests live with each source; this file has the flat category type, the reply parsers and the
//! store cache. Mouser's API has no category list and Nexar's isn't wired up, so they keep the trait's default "unsupported".

use super::Source;
use crate::store::Store;
use anyhow::Result;
use serde_json::Value;

/// One node of a category tree, flattened: `path` is "Resistors › Chip Resistor - Surface Mount".
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Category {
    pub id: String,
    pub path: String,
}

/// Category trees change rarely.
const TTL_SECS: i64 = 7 * 24 * 3600;
/// Not a query anyone can type into the search box, so it never collides with a cached search.
const CACHE_KEY: &str = "\u{1}categories";

/// `src`'s category list from the store if fresh, else from its API (then stored).
pub fn cached(src: &dyn Source) -> Result<Vec<Category>> {
    let store = Store::default_path().and_then(|p| Store::open(&p).ok());
    if let Some((json, at)) = store.as_ref().and_then(|s| s.cache_get(src.name(), CACHE_KEY).ok().flatten())
        && crate::cache::now() - at < TTL_SECS
        && let Ok(list) = serde_json::from_str(&json)
    {
        return Ok(list);
    }
    let list = src.categories()?;
    if let (Some(store), Ok(json)) = (&store, serde_json::to_string(&list)) {
        let _ = store.cache_put(src.name(), CACHE_KEY, &json);
    }
    Ok(list)
}

/// DigiKey v4 `/search/categories`: nested `Categories[].Children[]`.
pub fn parse_digikey(v: &Value) -> Vec<Category> {
    fn walk(nodes: &Value, prefix: &str, out: &mut Vec<Category>) {
        for n in nodes.as_array().into_iter().flatten() {
            let (Some(name), Some(id)) = (n["Name"].as_str(), n["CategoryId"].as_u64()) else { continue };
            let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix} › {name}") };
            walk(&n["Children"], &path, out);
            out.push(Category { id: id.to_string(), path });
        }
    }
    let mut out = vec![];
    walk(&v["Categories"], "", &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded from the live API (W6.1).
    const DIGIKEY: &str = include_str!("../../tests/fixtures/live/digikey_categories.json");

    #[test]
    fn digikey_tree_is_flattened_with_paths() {
        let c = parse_digikey(&serde_json::from_str(DIGIKEY).unwrap());
        assert!(c.len() > 50, "{}", c.len());
        let res = c.iter().find(|c| c.path == "Resistors").unwrap();
        assert!(res.id.parse::<u32>().is_ok());
        assert!(c.iter().any(|c| c.path.starts_with("Resistors › ")), "children are listed under their parent");
        assert!(parse_digikey(&serde_json::json!({})).is_empty());
    }
}
