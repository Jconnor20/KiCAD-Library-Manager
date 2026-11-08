//! Alternatives and duplicates: suggests groups of likely-equivalent parts, stores the groups the user
//! confirms (with a preferred / substitute / obsolete role each) in the part store, and writes the
//! alternatives into the symbols as `MPN_AltN`, `Manufacturer_AltN` and `<Supplier>_AltN` fields so
//! KiCad's BOM shows them. Nothing is merged or deleted: footprints may differ between members.
//!
//! A confirmed group is stored as rows `(preferred MPN, member)` in the `alternatives` table, the
//! preferred part included, so [`alternatives_of`] can answer from any member.

use crate::kicad_env::write_atomic;
use crate::library::{LibraryRoot, property, sanitize};
use crate::part::Part;
use crate::matching::{Kind, key, package_id, package_ids, same_mpn, value_key};
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

    /// The footprint's canonical package (`chip:0603`, `sot23`): R_0603 and RESC1608X55N are the same one.
    fn package(&self) -> String {
        package_id(&self.footprint)
    }

    /// The `Value` field when it says something other than the part number ("10k", "100nF"), trimmed
    /// and lower-cased with the unit signs dropped. This is what distributors are searched for.
    fn value_text(&self) -> Option<String> {
        let v = self.value.trim();
        if v.is_empty() || same_mpn(v, &self.mpn) || same_mpn(v, &self.symbol) {
            return None;
        }
        Some(v.replace(['Ω', ' '], "").to_lowercase().replace("ohm", ""))
    }

    /// What grouping compares: the value as an exact quantity (0.1uF = 100nF = 100n), else its text.
    /// Voltage and tolerance words are ignored, so a 16 V and a 50 V capacitor can share a suggestion;
    /// the user confirms every group.
    fn value_key(&self) -> Option<String> {
        let text = self.value_text()?;
        let kind = Kind::of_footprint(&self.footprint);
        Some(value_key(&self.value, kind, false).unwrap_or_else(|| format!("t:{kind:?}:{text}")))
    }

    /// Id of this part for rejected suggestions: the MPN at its raw footprint name (no library prefix).
    /// The package is resolved when read ([`canonical_id`]), so editing the alias table keeps rejections.
    pub fn member_id(&self) -> String {
        let name = self.footprint.rsplit_once(':').map_or(self.footprint.as_str(), |(_, n)| n);
        format!("{}@{}", key(&self.mpn), name.trim().to_ascii_lowercase())
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
            keys.push((pkg, v));
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

/// A stored `mpn@footprint` id with the footprint resolved to today's package id.
fn canonical_id(id: &str) -> String {
    id.rsplit_once('@').map_or(id.to_string(), |(m, f)| format!("{m}@{}", package_id(f)))
}

/// [`suggest`] without the groups the user rejected. A group is hidden when every pair of its members
/// was rejected, so a part added to it later brings it back.
pub fn suggest_unrejected(store: &Store, parts: &[LibPart]) -> Result<Vec<Vec<usize>>> {
    let rejected: std::collections::HashSet<_> =
        store.rejected()?.iter().map(|(a, b)| pairs(&[canonical_id(a), canonical_id(b)]).remove(0)).collect();
    let mut groups = suggest(parts);
    groups.retain(|g| {
        let ids: Vec<String> = g.iter().map(|&i| canonical_id(&parts[i].member_id())).collect();
        !pairs(&ids).iter().all(|p| rejected.contains(p))
    });
    Ok(groups)
}

/// Every pair of ids in a group, each ordered, equal ids included (a plain duplicate pairs with itself).
fn pairs(ids: &[String]) -> Vec<(String, String)> {
    let mut out = vec![];
    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
            out.push(if a <= b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) });
        }
    }
    out
}

/// Remembers that `members` were not equivalent, so [`suggest_unrejected`] stops offering them.
pub fn reject(store: &Store, members: &[&LibPart]) -> Result<()> {
    let ids: Vec<String> = members.iter().map(|p| p.member_id()).collect();
    store.add_rejected(&pairs(&ids))
}

/// Dissolves the confirmed group `mpn` belongs to: removes the `_AltN` fields it wrote (after one
/// snapshot), forgets the group and rejects it so the scan doesn't suggest it again. Returns the symbols
/// written; 0 when `mpn` isn't grouped.
pub fn ungroup(root: &LibraryRoot, store: &Store, mpn: &str) -> Result<usize> {
    let group = alternatives_of(store, mpn)?;
    if group.is_empty() {
        return Ok(0);
    }
    // Every member "obsolete" lists no alternatives, so write_back clears the fields; write before forgetting.
    let cleared: Vec<_> = group.iter().map(|a| Alternative { role: "obsolete".into(), ..a.clone() }).collect();
    let written = write_back(root, &cleared)?;
    for (anchor, alt) in store.all_alternatives()? {
        if group.iter().any(|g| same_mpn(&g.mpn, &alt.mpn)) {
            store.remove_alternatives(&anchor)?;
        }
    }
    let parts = scan(root);
    let members: Vec<&LibPart> = parts.iter().filter(|p| group.iter().any(|g| same_mpn(&g.mpn, &p.mpn))).collect();
    reject(store, &members)?;
    Ok(written)
}

/// What to search distributors for to find equivalents of `part`: its value ("10k") when that says more
/// than the part number, else the MPN.
pub fn search_query(part: &LibPart) -> String {
    part.value_text().unwrap_or_else(|| part.mpn.clone())
}

/// The search results that could stand in for `part`: another MPN, the same package where both sides
/// name one, and the part's value somewhere in the description or MPN when it has one.
pub fn equivalents(part: &LibPart, found: Vec<Part>) -> Vec<Part> {
    equivalents_filtered(part, found, &Filters { same_package: true, at_jlcpcb: false })
}

/// What the Alternatives panel narrows online results by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filters {
    /// Both sides name a package and it is the same (the default; off shows every package).
    pub same_package: bool,
    /// An LCSC listing with stock: LCSC's search is JLCPCB's own parts search, so that is "available at JLCPCB".
    pub at_jlcpcb: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self { same_package: true, at_jlcpcb: false }
    }
}

/// `equivalents` with the panel's filters; with `at_jlcpcb`, basic parts come before preferred before extended.
pub fn equivalents_filtered(part: &LibPart, found: Vec<Part>, filters: &Filters) -> Vec<Part> {
    let (pkgs, text) = (package_ids(&part.footprint), part.value_text());
    let own_key = key(part.footprint.rsplit_once(':').map_or(part.footprint.as_str(), |(_, n)| n));
    let kind = Kind::of_footprint(&part.footprint);
    let canon = text.as_ref().and_then(|_| value_key(&part.value, kind, false));
    let aliased = |ids: &[String]| ids.iter().any(|i| !i.starts_with("raw:"));
    let mut out: Vec<Part> = found
        .into_iter()
        .filter(|f| !same_mpn(&f.mpn, &part.mpn))
        .filter(|f| !filters.at_jlcpcb || (f.supplier_ids.iter().any(|(s, _)| s == "LCSC") && f.stock.is_some_and(|s| s > 0)))
        .filter(|f| {
            let fids = package_ids(&f.package);
            if !filters.same_package || fids.is_empty() || pkgs.is_empty() {
                return true;
            }
            if aliased(&fids) && aliased(&pkgs) {
                return fids.iter().any(|i| !i.starts_with("raw:") && pkgs.contains(i));
            }
            let fp = key(&f.package);
            fp.is_empty() || own_key.is_empty() || own_key.contains(&fp) || fp.contains(&own_key)
        })
        .filter(|f| {
            let Some(v) = &text else { return true };
            let blob = format!("{} {}", f.description, f.mpn);
            key(&blob).replace(['Ω', ' '], "").replace("ohm", "").contains(v.as_str()) || canon.as_ref().is_some_and(|c| mentions_value(&blob, c))
        })
        .collect();
    if filters.at_jlcpcb {
        // Stable, so the source's own order holds within a class; unknown class last.
        out.sort_by_key(|f| ["basic", "preferred", "extended"].iter().position(|c| *c == f.jlc_type).unwrap_or(3));
    }
    out
}

/// Whether some token of `text` ("CAP CER 0.1UF 50V", "RES 10K OHM 1%") is the value `canon`. Bare
/// numbers never count, so a "0603" in a description is not 603 Ω.
fn mentions_value(text: &str, canon: &str) -> bool {
    let toks: Vec<&str> = text.split(|c: char| c.is_whitespace() || matches!(c, ',' | '/' | '(' | ')')).filter(|t| !t.is_empty()).collect();
    toks.iter().enumerate().any(|(i, t)| {
        let one = value_key(t, None, false);
        let two = toks.get(i + 1).and_then(|u| value_key(&format!("{t} {u}"), None, false));
        one.is_some_and(|v| v == canon) || two.is_some_and(|v| v == canon)
    })
}

impl From<&Part> for Alternative {
    /// A substitute taken from a distributor result.
    fn from(p: &Part) -> Self {
        Alternative { mpn: p.mpn.clone(), manufacturer: p.manufacturer.clone(), supplier_ids: p.supplier_ids.clone(), role: "substitute".into() }
    }
}

/// Adds `alt` to the group `base` belongs to (starting one with `base` preferred if it has none), stores
/// it and writes the `_AltN` fields into the library. Returns the symbols written.
pub fn add_alternate(root: &LibraryRoot, store: &Store, base: &LibPart, alt: Alternative) -> Result<usize> {
    if alt.mpn.trim().is_empty() {
        bail!("an alternate needs an MPN");
    }
    let mut members = alternatives_of(store, &base.mpn)?;
    if members.is_empty() {
        members.push(base.as_alternative("preferred"));
    }
    members.retain(|m| !same_mpn(&m.mpn, &alt.mpn));
    members.push(alt);
    write_back(root, &confirm(store, &members)?)
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
    // Confirming says these belong together, so an older "not equivalent" between members is stale.
    store.clear_rejected_among(&group.iter().map(|a| key(&a.mpn)).collect())?;
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

    fn lp(sym: &str, mpn: &str, value: &str, fp: &str) -> LibPart {
        LibPart { category: "X".into(), symbol: sym.into(), mpn: mpn.into(), manufacturer: String::new(), value: value.into(), footprint: fp.into(), supplier_ids: vec![] }
    }

    #[test]
    fn groups_across_units_and_package_names() {
        let parts = [
            lp("a", "CL10B104KB8NNNC", "0.1uF", "KLM:C_0603_1608Metric"),
            lp("b", "GRM188R71H104KA93D", "100nF 50V", "KLM:CAPC1608X87N"),
            lp("c", "OTHER", "100pF", "KLM:C_0603_1608Metric"),
            lp("d", "RC1", "4k7", "KLM:R_0603"),
            lp("e", "RC2", "4.7kΩ", "KLM:RESC1608X55N"),
            lp("f", "RC3", "4R7", "KLM:R_0603"),
            lp("g", "L1", "4R7", "KLM:L_0603"),
            lp("h", "L2", "4.7uH", "KLM:INDC1608X95N"),
            lp("i", "C0201", "100nF", "KLM:C_0201_0603Metric"),
        ];
        let dnp = [lp("r", "R1", "DNP", "R_0603"), lp("c", "C1", "DNP", "C_0603")];
        assert!(suggest(&dnp).is_empty(), "free-text values group only within a kind");
        assert_eq!(suggest(&parts), [vec![0, 1], vec![3, 4], vec![6, 7]], "0201 and 0603, 4.7k and 4.7 ohm, caps and inductors stay apart");
    }

    #[test]
    fn rejected_suggestions_stay_gone_until_a_new_member_joins() {
        let s = Store::open(&crate::test_dir("alt_reject").join("parts.db")).unwrap();
        let mut parts = vec![lp("a", "A1", "10k", "R_0603"), lp("b", "B1", "10k", "R_0603"), lp("c", "X", "X", "SOT-23"), lp("d", "X", "X", "SOT-23-3")];
        assert_eq!(suggest_unrejected(&s, &parts).unwrap().len(), 2);
        reject(&s, &[&parts[0], &parts[1]]).unwrap();
        reject(&s, &[&parts[2], &parts[3]]).unwrap();
        assert!(suggest_unrejected(&s, &parts).unwrap().is_empty(), "plain duplicates reject too");
        parts.push(lp("e", "C1", "10k", "RESC1608X55N"));
        assert_eq!(suggest_unrejected(&s, &parts).unwrap(), [vec![0, 1, 4]], "a new member brings the group back");
        parts.push(lp("f", "X", "X", "SOIC-8"));
        parts.push(lp("g", "X", "X", "SOIC127P600X175-8N"));
        assert_eq!(suggest_unrejected(&s, &parts).unwrap().len(), 2, "the same MPN in another package is a new suggestion");
    }

    #[test]
    fn rejections_are_stored_raw_and_resolved_when_read() {
        let s = Store::open(&crate::test_dir("alt_reject_raw").join("parts.db")).unwrap();
        let parts = [lp("a", "A1", "10k", "KLM:RESC1608X55N"), lp("b", "B1", "10k", "KLM:R_0603")];
        reject(&s, &[&parts[0], &parts[1]]).unwrap();
        let stored = s.rejected().unwrap();
        assert!(stored.iter().all(|(a, b)| a.contains("@resc1608x55n") || a.contains("@r_0603") && b.contains('@') && !a.contains("chip:")), "{stored:?}");
        // What a changed alias table does: the stored footprint names resolve to whatever the table says now,
        // so an entry written under an old package id (W8.3 data) and the raw one hide the same group.
        s.add_rejected(&[("c1@chip:0603".into(), "d1@chip:0603".into())]).unwrap();
        let more = [lp("c", "C1", "10k", "R_0603"), lp("d", "D1", "10k", "RESC1608X55N")];
        assert!(suggest_unrejected(&s, &more).unwrap().is_empty());
        assert!(suggest_unrejected(&s, &parts).unwrap().is_empty());
    }

    #[test]
    fn confirming_a_group_clears_stale_rejections_between_members() {
        let s = Store::open(&crate::test_dir("alt_reject_confirm").join("parts.db")).unwrap();
        let (a, b, c) = (lp("a", "A1", "10k", "R_0603"), lp("b", "B1", "10k", "R_0603"), lp("c", "C1", "10k", "R_0603"));
        reject(&s, &[&a, &b]).unwrap();
        reject(&s, &[&a, &c]).unwrap();
        confirm(&s, &[a.as_alternative("preferred"), b.as_alternative("substitute")]).unwrap();
        let left = s.rejected().unwrap();
        assert_eq!(left.len(), 1, "only the A1/C1 rejection survives: {left:?}");
        assert!(left.iter().all(|(x, y)| x.contains("c1@") || y.contains("c1@")));
    }

    #[test]
    fn ungroup_clears_fields_and_remembers() {
        let root = library("alt_ungroup");
        let s = Store::open(&crate::test_dir("alt_ungroup_db").join("parts.db")).unwrap();
        let parts = scan(&root);
        let by = |sym: &str| parts.iter().find(|p| p.symbol == sym).unwrap();
        assert_eq!(ungroup(&root, &s, "LM358").unwrap(), 0, "not grouped");
        let group = confirm(&s, &[by("RC0603FR-0710KL").as_alternative("preferred"), by("ERJ-3EKF1002V").as_alternative("substitute")]).unwrap();
        write_back(&root, &group).unwrap();
        assert_eq!(ungroup(&root, &s, "erj3ekf1002v").unwrap(), 3, "both Yageo copies and Panasonic");
        assert!(alternatives_of(&s, "RC0603FR-0710KL").unwrap().is_empty());
        let text = std::fs::read_to_string(root.dir.join("symbols/Resistors.kicad_sym")).unwrap();
        assert!(!text.contains("MPN_Alt"), "all _AltN fields gone");
        assert!(suggest_unrejected(&s, &parts).unwrap().iter().all(|g| !g.contains(&parts.iter().position(|p| p.symbol == "ERJ-3EKF1002V").unwrap())), "not suggested again");
    }

    #[test]
    fn equivalents_read_units_in_descriptions() {
        let part = lp("a", "CL10B104", "100nF", "KLM:C_0603_1608Metric");
        let hit = |mpn: &str, desc: &str, pkg: &str| Part { mpn: mpn.into(), description: desc.into(), package: pkg.into(), ..Part::default() };
        let found = vec![
            hit("A", "CAP CER 0.1UF 50V X7R", "0603 (1608 Metric)"),
            hit("B", "CAP CER 0.1UF 50V X7R", "0805 (2012 Metric)"),
            hit("C", "CAP CER 1UF 50V", "0603"),
            hit("D", "CAP CER 100 nF", "0603"),
        ];
        let mpns: Vec<_> = equivalents(&part, found).into_iter().map(|p| p.mpn).collect();
        assert_eq!(mpns, ["A", "D"]);
    }

    #[test]
    fn alt_field_names() {
        assert!(is_alt_field("MPN_Alt1") && is_alt_field("DigiKey_Alt12"));
        assert!(!is_alt_field("MPN") && !is_alt_field("MPN_Alt") && !is_alt_field("Foo_Alt1"));
    }

    #[test]
    fn equivalents_from_recorded_search() {
        let found: Vec<Part> = serde_json::from_str(include_str!("../tests/fixtures/alt_search_10k_0603.json")).unwrap();
        let parts = scan(&library("alt_equiv"));
        let base = parts.iter().find(|p| p.symbol == "RC0603FR-0710KL").unwrap();
        assert_eq!(search_query(base), "10k");
        let mpns: Vec<_> = equivalents(base, found).into_iter().map(|p| p.mpn).collect();
        assert_eq!(mpns, ["ERJ-3EKF1002V", "CRCW060310K0FKEA"], "own MPN, 0805 and 100k dropped");
    }

    #[test]
    fn add_alternate_writes_fields_and_reads_back() {
        let root = library("alt_add");
        let s = Store::open(&crate::test_dir("alt_add_db").join("parts.db")).unwrap();
        let parts = scan(&root);
        let base = parts.iter().find(|p| p.symbol == "RC0603FR-0710KL").unwrap();
        let alt = Alternative { mpn: "CRCW060310K0FKEA".into(), manufacturer: "Vishay".into(), supplier_ids: vec![("Mouser".into(), "71-CRCW0603-10K".into())], role: "substitute".into() };
        assert!(add_alternate(&root, &s, base, Alternative { mpn: " ".into(), ..alt.clone() }).is_err());
        assert_eq!(add_alternate(&root, &s, base, alt).unwrap(), 2, "both Yageo copies");
        assert_eq!(alternatives_of(&s, "CRCW060310K0FKEA").unwrap().len(), 2);
        let again = scan(&root);
        let back = again.iter().find(|p| p.symbol == "RC0603FR-0710KL").unwrap();
        let lib = sexpr::parse(&std::fs::read_to_string(root.dir.join("symbols/Resistors.kicad_sym")).unwrap()).unwrap();
        let sym = lib.children("symbol").find(|s| s.arg(0) == Some(back.symbol.as_str())).unwrap();
        assert_eq!(property(sym, "MPN_Alt1"), Some("CRCW060310K0FKEA"));
        assert_eq!(property(sym, "Manufacturer_Alt1"), Some("Vishay"));
        assert_eq!(property(sym, "Mouser_Alt1"), Some("71-CRCW0603-10K"));
    }

    #[test]
    fn jlcpcb_and_package_filters() {
        let part = lp("a", "CL10B104", "100nF", "KLM:C_0603_1608Metric");
        let hit = |mpn: &str, pkg: &str, id: Option<&str>, stock: u64, jlc: &str| Part {
            mpn: mpn.into(),
            description: "CAP CER 100 nF".into(),
            package: pkg.into(),
            supplier_ids: id.map(|i| ("LCSC".to_string(), i.to_string())).into_iter().collect(),
            stock: Some(stock),
            jlc_type: jlc.into(),
            ..Part::default()
        };
        let found = || {
            vec![
                hit("EXT", "0603", Some("C1"), 10, "extended"),
                hit("BIG", "0805", Some("C2"), 10, "basic"),
                hit("NOSTOCK", "0603", Some("C3"), 0, "basic"),
                hit("DK", "0603", None, 99, ""),
                hit("BASIC", "0603", Some("C4"), 5, "basic"),
            ]
        };
        let mpns = |f: &Filters| equivalents_filtered(&part, found(), f).into_iter().map(|p| p.mpn).collect::<Vec<_>>();
        assert_eq!(mpns(&Filters::default()), ["EXT", "NOSTOCK", "DK", "BASIC"], "package only");
        assert_eq!(mpns(&Filters { same_package: true, at_jlcpcb: true }), ["BASIC", "EXT"], "stocked at LCSC, basic first");
        assert_eq!(mpns(&Filters { same_package: false, at_jlcpcb: true }), ["BIG", "BASIC", "EXT"]);
        assert_eq!(mpns(&Filters { same_package: false, at_jlcpcb: false }).len(), 5);
    }
}
