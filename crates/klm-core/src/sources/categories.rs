//! Distributor category trees (DigiKey, Nexar) for browsing the Search tab by part type. The
//! requests live with each source; this file has the flat category type, the reply parsers and the
//! store cache. Mouser's API has no category list, so it keeps the trait's default "unsupported".

use super::Source;
use crate::store::Store;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

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

/// DigiKey v4 `/search/categories`: nested `Categories[].ChildCategories[]`.
pub fn parse_digikey(v: &Value) -> Vec<Category> {
    fn walk(nodes: &Value, prefix: &str, out: &mut Vec<Category>) {
        for n in nodes.as_array().into_iter().flatten() {
            let (Some(name), Some(id)) = (n["Name"].as_str(), n["CategoryId"].as_u64()) else { continue };
            let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix} › {name}") };
            walk(&n["ChildCategories"], &path, out);
            out.push(Category { id: id.to_string(), path });
        }
    }
    let mut out = vec![];
    walk(&v["Categories"], "", &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Nexar `supCategories`: a flat list with parent ids.
pub fn parse_nexar(v: &Value) -> Vec<Category> {
    let list = v["data"]["supCategories"].as_array().map(Vec::as_slice).unwrap_or_default();
    let field = |c: &Value, snake: &str, camel: &str| c.get(snake).or_else(|| c.get(camel)).and_then(Value::as_str).map(str::to_string);
    let by_id: HashMap<String, (String, Option<String>)> = list
        .iter()
        .filter_map(|c| Some((c["id"].as_str()?.to_string(), (c["name"].as_str()?.to_string(), field(c, "parent_id", "parentId")))))
        .collect();
    let mut out: Vec<Category> = by_id
        .keys()
        .map(|id| {
            let (mut names, mut cur) = (vec![], Some(id.clone()));
            // The hop limit stops a malformed parent cycle.
            for _ in 0..16 {
                let Some((name, parent)) = cur.as_ref().and_then(|c| by_id.get(c)) else { break };
                names.push(name.as_str());
                cur = parent.clone();
            }
            names.reverse();
            Category { id: id.clone(), path: names.join(" › ") }
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shapes follow DigiKey's v4 categories reply and Nexar's supCategories reply.
    const DIGIKEY: &str = include_str!("../../tests/fixtures/digikey_categories.json");
    const NEXAR: &str = include_str!("../../tests/fixtures/nexar_categories.json");

    #[test]
    fn digikey_tree_is_flattened_with_paths() {
        let c = parse_digikey(&serde_json::from_str(DIGIKEY).unwrap());
        let paths: Vec<&str> = c.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["Capacitors", "Resistors", "Resistors › Chip Resistor - Surface Mount", "Resistors › Through Hole Resistors"]);
        assert_eq!(c[2].id, "53");
        assert!(parse_digikey(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn nexar_parents_become_paths_and_cycles_terminate() {
        let c = parse_nexar(&serde_json::from_str(NEXAR).unwrap());
        let paths: Vec<&str> = c.iter().map(|c| c.path.as_str()).collect();
        assert!(paths.contains(&"Passive Components › Resistors") && paths.contains(&"Passive Components"), "{paths:?}");
        assert_eq!(c.iter().find(|c| c.path.ends_with("Resistors")).unwrap().id, "6");
        assert!(parse_nexar(&serde_json::json!({"data": null})).is_empty());
    }
}
