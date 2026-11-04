//! KLM's own SQLite database: alternatives, tags, the search cache and history. The `.kicad_sym`
//! libraries stay the source of truth, so `parts` is a disposable index that [`Store::rebuild_from`]
//! refills; losing the file never affects a KiCad project.

use crate::library::{LibraryRoot, Scope};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

/// Bump together with a new step in [`MIGRATIONS`].
pub const SCHEMA_VERSION: u32 = 1;

/// `MIGRATIONS[n]` takes the database from version n to n + 1.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE parts (
        scope TEXT NOT NULL, root TEXT NOT NULL, category TEXT NOT NULL, symbol_name TEXT NOT NULL,
        mpn TEXT NOT NULL, manufacturer TEXT NOT NULL, description TEXT NOT NULL,
        footprint TEXT NOT NULL, model TEXT NOT NULL, datasheet TEXT NOT NULL, sim TEXT NOT NULL,
        in_kicad INTEGER NOT NULL,
        PRIMARY KEY (root, category, symbol_name)
    );
    -- Keyed by MPN, not a parts row, so they survive `parts` being rebuilt.
    CREATE TABLE alternatives (
        part_mpn TEXT NOT NULL, alt_mpn TEXT NOT NULL, manufacturer TEXT NOT NULL DEFAULT '',
        supplier_ids TEXT NOT NULL DEFAULT '[]',
        role TEXT NOT NULL CHECK (role IN ('preferred', 'substitute', 'obsolete')),
        PRIMARY KEY (part_mpn, alt_mpn)
    );
    CREATE TABLE tags (part_mpn TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (part_mpn, tag));
    CREATE TABLE search_cache (
        source TEXT NOT NULL, query TEXT NOT NULL, result TEXT NOT NULL,
        fetched_at INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY (source, query)
    );
    CREATE TABLE history (
        id INTEGER PRIMARY KEY, ts INTEGER NOT NULL DEFAULT (unixepoch()),
        action TEXT NOT NULL, details TEXT NOT NULL DEFAULT ''
    );
"];

pub struct Store {
    db: Connection,
}

/// A row of `parts`: [`crate::library::PartStatus`] plus where it lives. Checks are the variant
/// names of [`crate::library::Check`] ("Ok", "Link", "Missing", "Broken").
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPart {
    pub global: bool,
    pub root: String,
    pub category: String,
    pub symbol_name: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub footprint: String,
    pub model: String,
    pub datasheet: String,
    pub sim: String,
    pub in_kicad: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Alternative {
    pub mpn: String,
    pub manufacturer: String,
    /// (supplier, id) pairs, stored as JSON.
    pub supplier_ids: Vec<(String, String)>,
    /// "preferred", "substitute" or "obsolete".
    pub role: String,
}

impl Store {
    /// Next to `settings.json`.
    pub fn default_path() -> Option<PathBuf> {
        Some(crate::settings::app_dir()?.join("parts.db"))
    }

    /// Opens (creating and migrating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let db = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        let store = Self { db };
        store.migrate()?;
        Ok(store)
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.db.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    fn migrate(&self) -> Result<()> {
        let version = self.schema_version()?;
        if version > SCHEMA_VERSION {
            bail!("parts database is version {version}, newer than this KLM understands ({SCHEMA_VERSION})");
        }
        for (n, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            // One transaction per step; user_version is set inside it so a failed step leaves the old version.
            self.db.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version = {}; COMMIT;", n + 1))?;
        }
        Ok(())
    }

    /// Wipes `parts` and refills it from the libraries. Alternatives, tags, cache and history stay.
    pub fn rebuild_from(&mut self, roots: &[LibraryRoot]) -> Result<usize> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM parts", [])?;
        let mut n = 0;
        {
            let mut insert = tx.prepare("INSERT OR REPLACE INTO parts VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)")?;
            for root in roots {
                let scope = if root.scope == Scope::Global { "global" } else { "project" };
                for p in root.list_parts() {
                    insert.execute(params![
                        scope, root.dir.to_string_lossy(), p.category, p.name, p.mpn, p.manufacturer, p.description,
                        format!("{:?}", p.footprint), format!("{:?}", p.model), format!("{:?}", p.datasheet), format!("{:?}", p.sim),
                        p.in_kicad,
                    ])?;
                    n += 1;
                }
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn parts(&self) -> Result<Vec<StoredPart>> {
        let mut q = self.db.prepare(
            "SELECT scope, root, category, symbol_name, mpn, manufacturer, description, footprint, model, datasheet, sim, in_kicad
             FROM parts ORDER BY scope, root, category, symbol_name",
        )?;
        let rows = q.query_map([], |r| {
            Ok(StoredPart {
                global: r.get::<_, String>(0)? == "global",
                root: r.get(1)?,
                category: r.get(2)?,
                symbol_name: r.get(3)?,
                mpn: r.get(4)?,
                manufacturer: r.get(5)?,
                description: r.get(6)?,
                footprint: r.get(7)?,
                model: r.get(8)?,
                datasheet: r.get(9)?,
                sim: r.get(10)?,
                in_kicad: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds or updates the alternative `alt.mpn` of `part_mpn`.
    pub fn set_alternative(&self, part_mpn: &str, alt: &Alternative) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO alternatives (part_mpn, alt_mpn, manufacturer, supplier_ids, role) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![part_mpn, alt.mpn, alt.manufacturer, serde_json::to_string(&alt.supplier_ids)?, alt.role],
        )?;
        Ok(())
    }

    pub fn alternatives(&self, part_mpn: &str) -> Result<Vec<Alternative>> {
        let mut q = self.db.prepare("SELECT alt_mpn, manufacturer, supplier_ids, role FROM alternatives WHERE part_mpn = ?1 ORDER BY alt_mpn")?;
        let rows = q.query_map([part_mpn], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?, r.get(3)?)))?;
        rows.map(|row| {
            let (mpn, manufacturer, ids, role) = row?;
            Ok(Alternative { mpn, manufacturer, supplier_ids: serde_json::from_str(&ids)?, role })
        })
        .collect()
    }

    pub fn add_tag(&self, part_mpn: &str, tag: &str) -> Result<()> {
        self.db.execute("INSERT OR IGNORE INTO tags VALUES (?1, ?2)", [part_mpn, tag])?;
        Ok(())
    }

    pub fn tags(&self, part_mpn: &str) -> Result<Vec<String>> {
        let mut q = self.db.prepare("SELECT tag FROM tags WHERE part_mpn = ?1 ORDER BY tag")?;
        Ok(q.query_map([part_mpn], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn cache_put(&self, source: &str, query: &str, result: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO search_cache (source, query, result, fetched_at) VALUES (?1, ?2, ?3, unixepoch())",
            [source, query, result],
        )?;
        Ok(())
    }

    /// (JSON result, fetched-at unix seconds)
    pub fn cache_get(&self, source: &str, query: &str) -> Result<Option<(String, i64)>> {
        let mut q = self.db.prepare("SELECT result, fetched_at FROM search_cache WHERE source = ?1 AND query = ?2")?;
        let mut rows = q.query_map([source, query], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.next().transpose()?)
    }

    // --- Search history, saved searches, favourites, recent imports (W3.4). They reuse `history` and
    // `tags`, so no schema change; `parts` can be rebuilt without touching them.

    /// Newest first, each query once.
    pub fn recent_searches(&self, limit: usize) -> Result<Vec<String>> {
        let mut q = self.db.prepare(
            "SELECT details FROM history WHERE action = 'search' GROUP BY details ORDER BY MAX(id) DESC LIMIT ?1",
        )?;
        Ok(q.query_map(params![limit as i64], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_saved_search(&self, query: &str, saved: bool) -> Result<()> {
        self.db.execute("DELETE FROM history WHERE action = 'saved_search' AND details = ?1", [query])?;
        if saved {
            self.log("saved_search", query)?;
        }
        Ok(())
    }

    /// Oldest saved first.
    pub fn saved_searches(&self) -> Result<Vec<String>> {
        let mut q = self.db.prepare("SELECT details FROM history WHERE action = 'saved_search' ORDER BY id")?;
        Ok(q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_favourite(&self, mpn: &str, on: bool) -> Result<()> {
        if on {
            self.add_tag(mpn, "favourite")
        } else {
            self.db.execute("DELETE FROM tags WHERE part_mpn = ?1 AND tag = 'favourite'", [mpn])?;
            Ok(())
        }
    }

    pub fn favourites(&self) -> Result<Vec<String>> {
        let mut q = self.db.prepare("SELECT part_mpn FROM tags WHERE tag = 'favourite' ORDER BY part_mpn")?;
        Ok(q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    /// MPNs logged with `log("import", mpn)`, newest first, each once.
    pub fn recent_imports(&self, limit: usize) -> Result<Vec<String>> {
        let mut q = self.db.prepare(
            "SELECT details FROM history WHERE action = 'import' GROUP BY details ORDER BY MAX(id) DESC LIMIT ?1",
        )?;
        Ok(q.query_map(params![limit as i64], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn log(&self, action: &str, details: &str) -> Result<()> {
        self.db.execute("INSERT INTO history (action, details) VALUES (?1, ?2)", [action, details])?;
        Ok(())
    }

    /// Newest first: (unix seconds, action, details).
    pub fn history(&self) -> Result<Vec<(i64, String, String)>> {
        let mut q = self.db.prepare("SELECT ts, action, details FROM history ORDER BY id DESC")?;
        Ok(q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?)
    }

    // --- Alternative groups (W3.2, used by `crate::alternatives`) ---

    /// Every (part_mpn, alternative) row.
    pub fn all_alternatives(&self) -> Result<Vec<(String, Alternative)>> {
        let mut q = self.db.prepare("SELECT DISTINCT part_mpn FROM alternatives ORDER BY part_mpn")?;
        let parts: Vec<String> = q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let mut out = vec![];
        for p in parts {
            out.extend(self.alternatives(&p)?.into_iter().map(|a| (p.clone(), a)));
        }
        Ok(out)
    }

    /// Forgets a deleted part: its `parts` row, and, unless another library still has a part with that MPN,
    /// its alternatives (as the part or as someone's alternative), tags and favourite. Logs the deletion.
    pub fn forget_part(&self, root: &LibraryRoot, category: &str, symbol_name: &str, mpn: &str) -> Result<()> {
        let root = root.dir.to_string_lossy();
        self.db.execute("DELETE FROM parts WHERE root = ?1 AND category = ?2 AND symbol_name = ?3", [root.as_ref(), category, symbol_name])?;
        let still_there: i64 = self.db.query_row("SELECT COUNT(*) FROM parts WHERE mpn = ?1", [mpn], |r| r.get(0))?;
        if still_there == 0 {
            self.db.execute("DELETE FROM alternatives WHERE part_mpn = ?1 OR alt_mpn = ?1", [mpn])?;
            self.db.execute("DELETE FROM tags WHERE part_mpn = ?1", [mpn])?;
        }
        self.log("delete", mpn)
    }

    /// Deletes every alternative of `part_mpn`.
    pub fn remove_alternatives(&self, part_mpn: &str) -> Result<()> {
        self.db.execute("DELETE FROM alternatives WHERE part_mpn = ?1", [part_mpn])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYM: &str = r#"(kicad_symbol_lib (version 20241209)
  (symbol "TP-1" (property "Reference" "U" (at 0 0 0)) (property "MPN" "TP-1" (at 0 0 0))
    (property "Manufacturer" "ACME" (at 0 0 0)) (property "Footprint" "" (at 0 0 0)) (property "Datasheet" "https://x.test/d.pdf" (at 0 0 0)))
)"#;

    /// A global library holding one hand-written part, with KiCad's tables in a temp dir.
    fn library(name: &str) -> LibraryRoot {
        let dir = crate::test_dir(name);
        std::fs::create_dir_all(dir.join("lib/symbols")).unwrap();
        std::fs::write(dir.join("lib/symbols/ICs.kicad_sym"), SYM).unwrap();
        LibraryRoot::global(dir.join("lib"), dir.join("kicad"))
    }

    #[test]
    fn creates_at_current_version_and_reopens() {
        let path = crate::test_dir("store_create").join("sub/parts.db");
        assert_eq!(Store::open(&path).unwrap().schema_version().unwrap(), SCHEMA_VERSION);
        Store::open(&path).unwrap().log("x", "").unwrap();
        assert_eq!(Store::open(&path).unwrap().history().unwrap().len(), 1, "reopening keeps data");
    }

    #[test]
    fn refuses_a_newer_database() {
        let path = crate::test_dir("store_newer").join("parts.db");
        Store::open(&path).unwrap().db.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1)).unwrap();
        assert!(Store::open(&path).is_err());
    }

    #[test]
    fn migrates_an_empty_old_database() {
        let path = crate::test_dir("store_migrate").join("parts.db");
        Connection::open(&path).unwrap(); // version 0, no tables
        let s = Store::open(&path).unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        assert!(s.parts().unwrap().is_empty());
    }

    #[test]
    fn rebuild_refills_parts_and_keeps_the_rest() {
        let lib = library("store_rebuild");
        let mut s = Store::open(&crate::test_dir("store_rebuild_db").join("parts.db")).unwrap();
        assert_eq!(s.rebuild_from(std::slice::from_ref(&lib)).unwrap(), 1);
        let p = &s.parts().unwrap()[0];
        assert_eq!((p.category.as_str(), p.mpn.as_str(), p.manufacturer.as_str()), ("ICs", "TP-1", "ACME"));
        assert!(p.global && !p.in_kicad);
        assert_eq!((p.footprint.as_str(), p.datasheet.as_str()), ("Missing", "Link"));

        let alt = Alternative { mpn: "TP-2".into(), manufacturer: "Other".into(), supplier_ids: vec![("LCSC".into(), "C1".into())], role: "preferred".into() };
        s.set_alternative("TP-1", &alt).unwrap();
        s.add_tag("TP-1", "favourite").unwrap();
        s.cache_put("LCSC", "tp-1", "{}").unwrap();
        s.log("import", "TP-1").unwrap();
        assert!(s.set_alternative("TP-1", &Alternative { role: "bogus".into(), ..alt.clone() }).is_err());

        // The library changes on disk: the rebuild follows it, the rest is untouched.
        std::fs::remove_file(lib.dir.join("symbols/ICs.kicad_sym")).unwrap();
        assert_eq!(s.rebuild_from(&[lib]).unwrap(), 0);
        assert!(s.parts().unwrap().is_empty());
        assert_eq!(s.alternatives("TP-1").unwrap(), [alt]);
        assert_eq!(s.tags("TP-1").unwrap(), ["favourite"]);
        assert_eq!(s.cache_get("LCSC", "tp-1").unwrap().unwrap().0, "{}");
        assert_eq!(s.history().unwrap()[0].1, "import");
    }

    #[test]
    fn forgetting_a_part_drops_its_alternatives_and_tags() {
        let lib = library("store_forget");
        let mut s = Store::open(&crate::test_dir("store_forget_db").join("parts.db")).unwrap();
        s.rebuild_from(std::slice::from_ref(&lib)).unwrap();
        let alt = |mpn: &str| Alternative { mpn: mpn.into(), manufacturer: String::new(), supplier_ids: vec![], role: "substitute".into() };
        s.set_alternative("TP-1", &alt("TP-2")).unwrap();
        s.set_alternative("TP-3", &alt("TP-1")).unwrap();
        s.set_alternative("TP-3", &alt("TP-4")).unwrap();
        s.add_tag("TP-1", "x").unwrap();
        s.forget_part(&lib, "ICs", "TP-1", "TP-1").unwrap();
        assert!(s.parts().unwrap().is_empty() && s.tags("TP-1").unwrap().is_empty() && s.alternatives("TP-1").unwrap().is_empty());
        assert_eq!(s.alternatives("TP-3").unwrap().iter().map(|a| a.mpn.as_str()).collect::<Vec<_>>(), ["TP-4"]);
        assert_eq!(s.history().unwrap()[0].1, "delete");
    }

    #[test]
    fn searches_and_favourites_round_trip() {
        let path = crate::test_dir("store_w34").join("parts.db");
        let s = Store::open(&path).unwrap();
        for q in ["a", "b", "a"] {
            s.log("search", q).unwrap();
        }
        s.log("import", "X1").unwrap();
        s.set_saved_search("b", true).unwrap();
        s.set_saved_search("b", true).unwrap();
        s.set_favourite("X1", true).unwrap();
        s.set_favourite("X2", true).unwrap();
        s.set_favourite("X2", false).unwrap();
        let s = Store::open(&path).unwrap(); // survives reopening
        assert_eq!(s.recent_searches(10).unwrap(), ["a", "b"]);
        assert_eq!(s.saved_searches().unwrap(), ["b"]);
        assert_eq!(s.favourites().unwrap(), ["X1"]);
        assert_eq!(s.recent_imports(10).unwrap(), ["X1"]);
        s.set_saved_search("b", false).unwrap();
        assert!(s.saved_searches().unwrap().is_empty());
    }
}
