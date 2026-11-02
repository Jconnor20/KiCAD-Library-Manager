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
