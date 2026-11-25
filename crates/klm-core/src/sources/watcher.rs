//! Spots ZIPs that finish downloading while the app is open.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Watcher {
    dirs: Vec<PathBuf>,
    since: SystemTime,
    seen: HashSet<PathBuf>,
}

impl Watcher {
    /// Only ZIPs modified after this call count, so old downloads are never touched.
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self { dirs, since: SystemTime::now(), seen: HashSet::new() }
    }

    /// New, fully-downloaded ZIPs in any watched folder since the last call.
    pub fn poll(&mut self) -> Vec<PathBuf> {
        let mut found = vec![];
        for dir in &self.dirs {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) || self.seen.contains(&p) {
                    continue;
                }
                let Ok(meta) = e.metadata() else { continue };
                if meta.modified().map_or(true, |m| m < self.since) {
                    continue;
                }
                // Firefox creates an empty placeholder and writes to `<name>.part` until done.
                let mut part = p.as_os_str().to_owned();
                part.push(".part");
                if meta.len() == 0 || Path::new(&part).exists() {
                    continue;
                }
                self.seen.insert(p.clone());
                found.push(p);
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_new_finished_zips_once() {
        let dir = crate::test_dir("watcher");
        std::fs::write(dir.join("old.zip"), "x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let other = crate::test_dir("watcher2");
        let mut w = Watcher::new(vec![dir.clone(), other.clone()]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(other.join("second.zip"), "x").unwrap();
        std::fs::write(dir.join("new.zip"), "x").unwrap();
        std::fs::write(dir.join("busy.zip"), "x").unwrap();
        std::fs::write(dir.join("busy.zip.part"), "x").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let mut got = w.poll();
        got.sort();
        let mut want = vec![dir.join("new.zip"), other.join("second.zip")];
        want.sort();
        assert_eq!(got, want);
        assert!(w.poll().is_empty());
        std::fs::remove_file(dir.join("busy.zip.part")).unwrap();
        assert_eq!(w.poll(), vec![dir.join("busy.zip")]);
    }
}
