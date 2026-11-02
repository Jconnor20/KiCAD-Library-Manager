//! Alternatives and duplicates: suggests groups of likely-equivalent parts, stores the groups the user
//! confirms (with a preferred / substitute / obsolete role each) in the part store, and writes the
//! alternatives into the symbols as `MPN_AltN`, `Manufacturer_AltN` and `<Supplier>_AltN` fields so
//! KiCad's BOM shows them. Nothing is merged or deleted: footprints may differ between members.
//!
//! A confirmed group is stored as rows `(preferred MPN, member)` in the `alternatives` table, the
//! preferred part included, so [`alternatives_of`] can answer from any member.

use crate::kicad_env::write_atomic;
use crate::library::{LibraryRoot, property, sanitize};
use crate::matching::{key, same_mpn};
use crate::sexpr;
use crate::store::{Alternative, Store};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;

/// Group roles, in the order members are listed and written.
pub const ROLES: &[&str] = &["preferred", "substitute", "obsolete"];

/// Supplier-number fields copied as `<Supplier>_AltN` (the ones the importer and editor write).
const SUPPLIERS: &[&str] = &["LCSC", "DigiKey", "Mouser"];

/// One symbol in a library, with what grouping looks at.
#[derive(Clone, Debug, PartialEq)]
pub struct LibPart {
    pub category: String,
    /// Symbol name in the library.
    pub symbol: String,
    pub mpn: String,
    pub manufacturer: String,
    pub value: String,
    /// The `Footprint` field, `Library:Name`.
    pub footprint: String,
    pub supplier_ids: Vec<(String, String)>,
}

impl LibPart {
    pub fn as_alternative(&self, role: &str) -> Alternative {
        Alternative { mpn: self.mpn.clone(), manufacturer: self.manufacturer.clone(), supplier_ids: self.supplier_ids.clone(), role: role.into() }
    }

    /// The footprint name without its library, normalised.
    // shortcut: vendors name the same package differently (R_0603 vs RESC1608X55N), so those stay apart; add a package parser if users miss groups.
    fn package(&self) -> String {
        let name = self.footprint.rsplit_once(':').map_or(self.footprint.as_str(), |(_, n)| n);
        key(name)
    }

    /// The `Value` field when it says something other than the part number ("10k", "100nF").
    fn value_key(&self) -> Option<String> {
        let v = self.value.trim();
        if v.is_empty() || same_mpn(v, &self.mpn) || same_mpn(v, &self.symbol) {
            return None;
        }
        Some(v.replace(['Ω', ' '], "").to_lowercase().replace("ohm", ""))
    }
}

/// Every symbol in `root`, by category, in file order.
pub fn scan(root: &LibraryRoot) -> Vec<LibPart> {
    let mut cats = root.categories();
    cats.sort();
    let mut out = vec![];
    for cat in cats {
        let path = root.dir.join("symbols").join(format!("{cat}.kicad_sym"));
        let Some(lib) = std::fs::read_to_string(path).ok().and_then(|t| sexpr::parse(&t).ok()) else { continue };
        for sym in lib.children("symbol") {
            let field = |k: &str| property(sym, k).unwrap_or("").trim().to_string();
            let symbol = sym.arg(0).unwrap_or_default().to_string();
            out.push(LibPart {
                category: cat.clone(),
                mpn: Some(field("MPN")).filter(|m| !m.is_empty()).unwrap_or_else(|| symbol.clone()),
                symbol,
                manufacturer: field("Manufacturer"),
                value: field("Value"),
                footprint: field("Footprint"),
                supplier_ids: SUPPLIERS.iter().map(|s| (s.to_string(), field(s))).filter(|(_, id)| !id.is_empty()).collect(),
            });
        }
    }
    out
}

/// Likely-equivalent parts, as groups of indices into `parts` (two or more each). Parts join a group when
/// they share a package and either the normalised MPN (a duplicate) or the value (a second source).
/// Different packages never group, so the same MPN with two footprints stays apart.
pub fn suggest(parts: &[LibPart]) -> Vec<Vec<usize>> {
    // Union-find over "first part seen with this key".
    let mut parent: Vec<usize> = (0..parts.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut first: HashMap<(String, String), usize> = HashMap::new();
    for (i, p) in parts.iter().enumerate() {
        let pkg = p.package();
        let mut keys = vec![(pkg.clone(), format!("m:{}", key(&p.mpn)))];
        if let Some(v) = p.value_key().filter(|_| !pkg.is_empty()) {
            keys.push((pkg, format!("v:{v}")));
        }
        for k in keys {
            let j = *first.entry(k).or_insert(i);
            let (a, b) = (find(&mut parent, i), find(&mut parent, j));
            parent[a] = b;
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..parts.len() {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<_> = groups.into_values().filter(|g| g.len() > 1).collect();
    out.sort();
    out
}

fn rank(role: &str) -> usize {
    ROLES.iter().position(|r| *r == role).unwrap_or(ROLES.len())
}

/// Stores a group the user confirmed, replacing any stored group that shares a member. Needs exactly one
/// preferred member and at least two distinct MPNs. Returns the group as stored: preferred first.
pub fn confirm(store: &Store, members: &[Alternative]) -> Result<Vec<Alternative>> {
    if let Some(bad) = members.iter().find(|a| !ROLES.contains(&a.role.as_str())) {
        bail!("unknown role {:?} for {}", bad.role, bad.mpn);
    }
    let mut group = members.to_vec();
    group.sort_by_key(|a| rank(&a.role));
    let mut seen: Vec<String> = vec![];
    group.retain(|a| {
        let new = !seen.iter().any(|s| same_mpn(s, &a.mpn));
        seen.push(a.mpn.clone());
        new
    });
    if group.iter().filter(|a| a.role == "preferred").count() != 1 {
        bail!("a group needs exactly one preferred part");
    }
    if group.len() < 2 {
        bail!("a group needs at least two different MPNs");
    }
    for (anchor, alt) in store.all_alternatives()? {
        if group.iter().any(|g| same_mpn(&g.mpn, &alt.mpn)) {
            store.remove_alternatives(&anchor)?;
        }
    }
    for a in &group {
        store.set_alternative(&group[0].mpn, a)?;
    }
    Ok(group)
}

/// The confirmed group `mpn` belongs to, `mpn` itself included, preferred first then substitutes and
/// obsolete parts. Empty when the user hasn't grouped it.
pub fn alternatives_of(store: &Store, mpn: &str) -> Result<Vec<Alternative>> {
    let rows = store.all_alternatives()?;
    let Some((anchor, _)) = rows.iter().find(|(_, a)| same_mpn(&a.mpn, mpn)) else { return Ok(vec![]) };
    let mut group: Vec<_> = rows.iter().filter(|(p, _)| p == anchor).map(|(_, a)| a.clone()).collect();
    group.sort_by_key(|a| rank(&a.role));
    Ok(group)
}

/// `MPN_Alt3`, `LCSC_Alt1`, …: fields this module owns and rewrites.
fn is_alt_field(name: &str) -> bool {
    name.rsplit_once("_Alt").is_some_and(|(base, n)| {
        !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && (base == "MPN" || base == "Manufacturer" || SUPPLIERS.contains(&base))
    })
}

/// Writes `group`'s alternatives into every symbol in `root` whose MPN is in it (after one snapshot):
/// each symbol lists the other members that aren't obsolete, preferred first, as `MPN_AltN`,
/// `Manufacturer_AltN` and `<Supplier>_AltN`. Old `_AltN` fields are replaced, and cleared from symbols
/// that still list a member but have left the group. Returns the symbols written.
pub fn write_back(root: &LibraryRoot, group: &[Alternative]) -> Result<usize> {
    let in_group = |mpn: &str| group.iter().any(|g| same_mpn(&g.mpn, mpn));
    let mut snapshot = None;
    let mut written = 0;
    let mut cats = root.categories();
    cats.sort();
    for cat in cats {
        let path = root.dir.join("symbols").join(format!("{}.kicad_sym", sanitize(&cat)));
        let mut lib = sexpr::parse(&std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?)?;
        let mut changed = false;
        for sym in lib.items_mut().iter_mut().filter(|s| s.head() == Some("symbol")) {
            let mpn = property(sym, "MPN").map(str::trim).filter(|m| !m.is_empty()).or(sym.arg(0)).unwrap_or_default().to_string();
            let lists_member = sym.children("property").any(|p| p.arg(0).is_some_and(|k| k.starts_with("MPN_Alt") && is_alt_field(k)) && p.arg(1).is_some_and(in_group));
            if !in_group(&mpn) && !lists_member {
                continue;
            }
            sym.items_mut().retain(|p| !(p.head() == Some("property") && p.arg(0).is_some_and(is_alt_field)));
            (changed, written) = (true, written + 1);
            let alts = group.iter().filter(|a| in_group(&mpn) && a.role != "obsolete" && !same_mpn(&a.mpn, &mpn));
            for (n, a) in alts.enumerate() {
                let n = n + 1;
                crate::library::set_property(sym, &format!("MPN_Alt{n}"), &a.mpn);
                crate::library::set_property(sym, &format!("Manufacturer_Alt{n}"), &a.manufacturer);
                for (supplier, id) in &a.supplier_ids {
                    crate::library::set_property(sym, &format!("{supplier}_Alt{n}"), id);
                }
            }
        }
        if changed {
            if snapshot.is_none() {
                snapshot = Some(crate::snapshot::guard(root, &format!("alternatives of {}", group.first().map_or("", |g| g.mpn.as_str())))?);
            }
            write_atomic(&path, &sexpr::to_string(&lib))?;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str, mpn: &str, maker: &str, value: &str, fp: &str, lcsc: &str) -> String {
        format!(
            r#"(symbol "{name}" (property "Reference" "R" (at 0 0 0)) (property "Value" "{value}" (at 0 0 0)) (property "Footprint" "{fp}" (at 0 0 0))
    (property "MPN" "{mpn}" (at 0 0 0)) (property "Manufacturer" "{maker}" (at 0 0 0)) (property "LCSC" "{lcsc}" (at 0 0 0)))"#
        )
    }

    /// Resistors: two 10k 0603 from different makers, the same Yageo MPN again in another category, and
    /// one MPN in two packages. ICs: an op-amp whose Value is its MPN.
    fn library(name: &str) -> LibraryRoot {
        let dir = crate::test_dir(name);
        std::fs::create_dir_all(dir.join("lib/symbols")).unwrap();
        let res = [
            sym("RC0603FR-0710KL", "RC0603FR-0710KL", "Yageo", "10k", "KLM_Resistors:R_0603", "C98220"),
            sym("ERJ-3EKF1002V", "ERJ-3EKF1002V", "Panasonic", "10 kΩ", "KLM_Resistors:R_0603", ""),
            sym("X-SAME", "X-SAME", "ACME", "X-SAME", "KLM_Resistors:SOT-23", ""),
            sym("X-SAME_SOIC", "X-SAME", "ACME", "X-SAME", "KLM_Resistors:SOIC-8", ""),
        ];
        let ics = [sym("RC0603FR0710KL", "rc0603fr0710kl", "Yageo", "10k", "Other:R_0603", ""), sym("LM358", "LM358", "TI", "LM358", "KLM_ICs:SOIC-8", "")];
        std::fs::write(dir.join("lib/symbols/Resistors.kicad_sym"), format!("(kicad_symbol_lib (version 20241209)\n{}\n)", res.join("\n"))).unwrap();
        std::fs::write(dir.join("lib/symbols/ICs.kicad_sym"), format!("(kicad_symbol_lib (version 20241209)\n{}\n)", ics.join("\n"))).unwrap();
        LibraryRoot::global(dir.join("lib"), dir.join("kicad"))
    }

    fn names(parts: &[LibPart], groups: &[Vec<usize>]) -> Vec<Vec<String>> {
        groups.iter().map(|g| g.iter().map(|&i| format!("{}/{}", parts[i].category, parts[i].symbol)).collect()).collect()
    }

    #[test]
    fn groups_by_mpn_or_value_within_a_package() {
        let parts = scan(&library("alt_group"));
        assert_eq!(parts.len(), 6);
        assert_eq!(parts.iter().find(|p| p.symbol == "RC0603FR-0710KL").unwrap().supplier_ids, [("LCSC".to_string(), "C98220".to_string())]);
        assert_eq!(
            names(&parts, &suggest(&parts)),
            [["ICs/RC0603FR0710KL", "Resistors/RC0603FR-0710KL", "Resistors/ERJ-3EKF1002V"]],
            "same MPN or same value in R_0603 group; X-SAME in SOT-23 and SOIC-8 stays apart, and so does LM358"
        );
    }

    #[test]
    fn same_mpn_different_footprint_stays_separate() {
        let parts = scan(&library("alt_separate"));
        let groups = suggest(&parts);
        assert!(groups.iter().all(|g| g.iter().filter(|&&i| parts[i].mpn == "X-SAME").count() < 2));
    }

    #[test]
    fn confirm_stores_roles_and_replaces_overlapping_groups() {
        let s = Store::open(&crate::test_dir("alt_confirm").join("parts.db")).unwrap();
        let a = |mpn: &str, role: &str| Alternative { mpn: mpn.into(), manufacturer: "M".into(), supplier_ids: vec![], role: role.into() };
        assert!(confirm(&s, &[a("A", "substitute"), a("B", "substitute")]).is_err(), "needs a preferred part");
        assert!(confirm(&s, &[a("A", "preferred"), a("a", "substitute")]).is_err(), "one MPN is not a group");
        assert!(confirm(&s, &[a("A", "preferred"), a("B", "best")]).is_err());

        let g = confirm(&s, &[a("B", "obsolete"), a("C", "substitute"), a("A", "preferred")]).unwrap();
        assert_eq!(g.iter().map(|x| x.mpn.as_str()).collect::<Vec<_>>(), ["A", "C", "B"]);
        assert_eq!(alternatives_of(&s, "b").unwrap(), g, "found from any member, normalised");
        assert!(alternatives_of(&s, "Z").unwrap().is_empty());

        // Re-confirming with C preferred and a new member replaces the old group.
        confirm(&s, &[a("C", "preferred"), a("D", "substitute")]).unwrap();
        assert!(alternatives_of(&s, "A").unwrap().is_empty());
        assert_eq!(alternatives_of(&s, "D").unwrap().iter().map(|x| x.role.as_str()).collect::<Vec<_>>(), ["preferred", "substitute"]);
    }

    #[test]
    fn write_back_adds_alt_fields_and_replaces_old_ones() {
        let root = library("alt_write");
        let s = Store::open(&crate::test_dir("alt_write_db").join("parts.db")).unwrap();
        let parts = scan(&root);
        let by = |sym: &str| parts.iter().find(|p| p.symbol == sym).unwrap();
        let group = confirm(&s, &[by("ERJ-3EKF1002V").as_alternative("substitute"), by("RC0603FR-0710KL").as_alternative("preferred"), by("LM358").as_alternative("obsolete")]).unwrap();
        assert_eq!(write_back(&root, &group).unwrap(), 4, "both Yageo copies, Panasonic and LM358");
        assert_eq!(crate::snapshot::list(&root).len(), 1, "one snapshot for the whole write");

        let field = |cat: &str, sym: &str, k: &str| {
            let lib = sexpr::parse(&std::fs::read_to_string(root.dir.join(format!("symbols/{cat}.kicad_sym"))).unwrap()).unwrap();
            let s = lib.children("symbol").find(|s| s.arg(0) == Some(sym)).unwrap().clone();
            property(&s, k).map(String::from)
        };
        // Panasonic lists the preferred Yageo with its LCSC number; Yageo lists Panasonic; obsolete LM358 is nobody's alternative.
        assert_eq!(field("Resistors", "ERJ-3EKF1002V", "MPN_Alt1").as_deref(), Some("RC0603FR-0710KL"));
        assert_eq!(field("Resistors", "ERJ-3EKF1002V", "Manufacturer_Alt1").as_deref(), Some("Yageo"));
        assert_eq!(field("Resistors", "ERJ-3EKF1002V", "LCSC_Alt1").as_deref(), Some("C98220"));
        assert_eq!(field("Resistors", "ERJ-3EKF1002V", "MPN_Alt2"), None);
        assert_eq!(field("ICs", "RC0603FR0710KL", "MPN_Alt1").as_deref(), Some("ERJ-3EKF1002V"));
        assert_eq!(field("ICs", "LM358", "MPN_Alt1").as_deref(), Some("RC0603FR-0710KL"));
        assert_eq!(field("ICs", "LM358", "MPN_Alt2").as_deref(), Some("ERJ-3EKF1002V"));
        assert_eq!(field("ICs", "LM358", "MPN").as_deref(), Some("LM358"), "own fields untouched");

        // A smaller group rewrites the fields, leaving no stale MPN_Alt2.
        let group = confirm(&s, &[by("LM358").as_alternative("preferred"), by("ERJ-3EKF1002V").as_alternative("substitute")]).unwrap();
        assert_eq!(write_back(&root, &group).unwrap(), 4, "the Yageo copies left the group and are cleared");
        assert_eq!(field("Resistors", "RC0603FR-0710KL", "MPN_Alt1"), None);
        assert_eq!(field("ICs", "LM358", "MPN_Alt1").as_deref(), Some("ERJ-3EKF1002V"));
        assert_eq!(field("ICs", "LM358", "MPN_Alt2"), None);
        assert_eq!(field("ICs", "LM358", "Manufacturer_Alt2"), None);
    }

    #[test]
    fn alt_field_names() {
        assert!(is_alt_field("MPN_Alt1") && is_alt_field("DigiKey_Alt12"));
        assert!(!is_alt_field("MPN") && !is_alt_field("MPN_Alt") && !is_alt_field("Foo_Alt1"));
    }
}
