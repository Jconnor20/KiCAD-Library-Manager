//! What each part in a library has and lacks.

use super::{LibraryRoot, property};
use crate::sexpr;
use std::path::Path;

/// State of one piece of a part (footprint, 3D model, datasheet, sim model).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Check {
    /// Present and the file exists.
    Ok,
    /// Points at a web page or another library, so it can't be checked locally.
    Link,
    /// Not set.
    Missing,
    /// Set, but the file it points at isn't there.
    Broken,
}

#[derive(Clone, Debug)]
pub struct PartStatus {
    pub category: String,
    /// Symbol name in the library.
    pub name: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub footprint: Check,
    pub model: Check,
    pub datasheet: Check,
    pub sim: Check,
    /// What "open datasheet" opens: a URL, or the local PDF if it exists.
    pub datasheet_target: Option<String>,
    /// The category library is listed in KiCad's tables.
    pub in_kicad: bool,
}

/// A network (UNC) path: `\\server\share`, `//server/share` or the `\\?\UNC\` form. Touching one makes Windows try to sign in to that server.
pub fn is_unc(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with(r"\\") || s.starts_with("//")
}

/// Whether a datasheet target from a library file is safe to hand to the OS: a web link, or a local `.pdf` not on a network share.
/// A foreign `.kicad_sym` could otherwise point it at a script, a shortcut or a share.
pub fn safe_to_open(target: &str) -> bool {
    let t = target.trim();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return true;
    }
    !t.contains("://") && !is_unc(Path::new(t)) && Path::new(t).extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

impl LibraryRoot {
    /// Every part in this library with what it has and lacks, sorted by category then name.
    pub fn list_parts(&self) -> Vec<PartStatus> {
        let mut cats = self.categories();
        cats.sort();
        let mut out = vec![];
        for cat in cats {
            let in_kicad = !self.needs_link(&cat);
            let Some(lib) = std::fs::read_to_string(self.symbol_lib(&cat)).ok().and_then(|t| sexpr::parse(&t).ok()) else { continue };
            for sym in lib.children("symbol") {
                let field = |k: &str| property(sym, k).unwrap_or("").trim().to_string();
                let name = sym.arg(0).unwrap_or_default().to_string();
                let (footprint, model) = self.check_footprint(&cat, &field("Footprint"));
                let datasheet = self.check_file(&field("Datasheet"));
                out.push(PartStatus {
                    category: cat.clone(),
                    mpn: Some(field("MPN")).filter(|m| !m.is_empty()).unwrap_or_else(|| name.clone()),
                    name,
                    manufacturer: field("Manufacturer"),
                    description: field("Description"),
                    footprint,
                    model,
                    datasheet_target: match datasheet {
                        Check::Ok => Some(self.resolve(&field("Datasheet"))),
                        Check::Link if field("Datasheet").starts_with("http") => Some(field("Datasheet")),
                        _ => None,
                    },
                    datasheet,
                    sim: self.check_file(&field("Sim.Library")),
                    in_kicad,
                });
            }
        }
        out
    }

    /// A cheap fingerprint of the symbol, footprint and 3D files (count, newest mtime), so a save made in KiCad is noticed.
    pub fn signature(&self) -> (usize, Option<std::time::SystemTime>) {
        let files = |dir: std::path::PathBuf| std::fs::read_dir(dir).into_iter().flatten().flatten().collect::<Vec<_>>();
        let mut all = files(self.dir.join("symbols"));
        for sub in ["footprints", "3dmodels"] {
            for d in files(self.dir.join(sub)).into_iter().filter(|d| d.path().is_dir()) {
                all.extend(files(d.path()));
            }
        }
        (all.len(), all.iter().filter_map(|e| e.metadata().ok()?.modified().ok()).max())
    }

    /// A path from a KiCad file with `${KIPRJMOD}` filled in.
    fn resolve(&self, value: &str) -> String {
        value.replace("${KIPRJMOD}", &self.table_dir.to_string_lossy())
    }

    fn check_file(&self, value: &str) -> Check {
        if value.is_empty() || value == "~" {
            return Check::Missing;
        }
        if value.starts_with("http") {
            return Check::Link;
        }
        let path = self.resolve(value);
        if path.contains("${") {
            Check::Link // another KiCad path variable; can't resolve it here
        } else if is_unc(Path::new(&path)) {
            Check::Broken // never touched: probing a network path named by a library file would leak the user's login hash
        } else if Path::new(&path).is_file() {
            Check::Ok
        } else {
            Check::Broken
        }
    }

    /// (footprint, 3D model) for a symbol's `Footprint` field.
    fn check_footprint(&self, cat: &str, field: &str) -> (Check, Check) {
        let Some((nick, name)) = field.split_once(':') else {
            return (if field.is_empty() { Check::Missing } else { Check::Broken }, Check::Missing);
        };
        if nick != self.nickname(cat) {
            return (Check::Link, Check::Link); // a footprint from another library
        }
        let path = self.dir.join("footprints").join(format!("{cat}.pretty")).join(format!("{name}.kicad_mod"));
        let Some(fp) = std::fs::read_to_string(path).ok().and_then(|t| sexpr::parse(&t).ok()) else {
            return (Check::Broken, Check::Missing);
        };
        (Check::Ok, fp.child("model").and_then(|m| m.arg(0)).map_or(Check::Missing, |m| self.check_file(m)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_links_and_local_pdfs_are_opened_and_network_paths_never() {
        for ok in ["https://x.com/a.pdf", "HTTP://x.com/page", "C:/lib/datasheets/a.pdf", r"C:\lib\A.PDF"] {
            assert!(safe_to_open(ok), "{ok}");
        }
        let scripts = ["C:/x/run.bat", "C:/x/a.lnk", "C:/x/a.pdf.exe", "ftp://x/a.pdf", ""];
        for bad in scripts.into_iter().chain(["//srv/share/a.pdf", r"\\srv\share\a.pdf", r"\\?\UNC\srv\share\a.pdf", "file://srv/share/a.pdf"]) {
            assert!(!safe_to_open(bad), "{bad}");
        }
        assert!(is_unc(Path::new("//srv/s")) && !is_unc(Path::new("C:/srv")));
    }
}
