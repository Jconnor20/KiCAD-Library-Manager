//! Manufacturer (fab) capabilities and the IPC tables, and the limits a profile's fab choice gives.
//!
//! The data lives in `fab_profiles.json`, embedded at build time. W10.1 (library footprint check) and
//! W10.2 (whole-project check) read limits only through [`limits`], so they don't care where numbers come from.

use serde::{Deserialize, Serialize};

/// What a profile asks a board and its footprints to meet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FabChoice {
    /// Fab ids from `fab_profiles.json` ("jlcpcb"). Several: the strictest value of each limit wins. Empty: no fab.
    pub fabs: Vec<String>,
    /// IPC-6012 performance class, 1–3.
    pub ipc_class: u8,
    /// IPC-7351 land-pattern density: 'A' (most), 'B' (nominal) or 'C' (least).
    pub density: char,
}

impl Default for FabChoice {
    fn default() -> Self {
        Self { fabs: vec![], ipc_class: 2, density: 'B' }
    }
}

/// Minimums in mm. `None`: no limit known, so nothing is checked for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FabLimits {
    pub min_track: Option<f64>,
    /// Copper-to-copper gap (track–track, pad–pad).
    pub min_clearance: Option<f64>,
    /// Smallest finished drill for a plated hole.
    pub min_drill: Option<f64>,
    pub min_annular_ring: Option<f64>,
    /// Via: smallest drill and smallest pad diameter.
    pub min_via_drill: Option<f64>,
    pub min_via_diameter: Option<f64>,
    pub min_silk_width: Option<f64>,
    pub min_silk_height: Option<f64>,
    /// Solder-mask dam between pads.
    pub min_mask_web: Option<f64>,
    /// Copper to board edge.
    pub min_edge_clearance: Option<f64>,
    /// Courtyard excess around the part for the chosen IPC-7351 density (not a minimum of the fab).
    pub courtyard_excess: Option<f64>,
}

/// One fab as the JSON records it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fab {
    pub id: String,
    pub name: String,
    /// The capability page the numbers came from, and when they were checked (YYYY-MM-DD).
    pub source: String,
    pub checked: String,
    pub limits: FabLimits,
}

/// The whole JSON: the fabs, and the IPC tables indexed class 1..3 / density A..C.
#[derive(Deserialize)]
struct Tables {
    fabs: Vec<Fab>,
    ipc: Ipc,
}

#[derive(Deserialize)]
struct Ipc {
    annular_ring: [f64; 3],
    courtyard_excess: [f64; 3],
}

// shortcut: parsed on every call (a few KB), cache in a OnceLock if a check ever calls this per footprint.
fn tables() -> Tables {
    serde_json::from_str(include_str!("fab_profiles.json")).expect("fab_profiles.json is valid")
}

/// Every fab known, in the JSON's order.
pub fn fabs() -> Vec<Fab> {
    tables().fabs
}

/// The larger of two optional minimums; `None` only when neither is known.
fn strictest(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// The limits `choice` gives: the strictest of its fabs, combined with its IPC class and density.
pub fn limits(choice: &FabChoice) -> FabLimits {
    let t = tables();
    let mut out = FabLimits::default();
    // Unknown fab ids are skipped so a profile shared from a newer version still loads.
    for f in t.fabs.iter().filter(|f| choice.fabs.contains(&f.id)) {
        let l = &f.limits;
        out.min_track = strictest(out.min_track, l.min_track);
        out.min_clearance = strictest(out.min_clearance, l.min_clearance);
        out.min_drill = strictest(out.min_drill, l.min_drill);
        out.min_annular_ring = strictest(out.min_annular_ring, l.min_annular_ring);
        out.min_via_drill = strictest(out.min_via_drill, l.min_via_drill);
        out.min_via_diameter = strictest(out.min_via_diameter, l.min_via_diameter);
        out.min_silk_width = strictest(out.min_silk_width, l.min_silk_width);
        out.min_silk_height = strictest(out.min_silk_height, l.min_silk_height);
        out.min_mask_web = strictest(out.min_mask_web, l.min_mask_web);
        out.min_edge_clearance = strictest(out.min_edge_clearance, l.min_edge_clearance);
    }
    let class = usize::from(choice.ipc_class.clamp(1, 3)) - 1;
    out.min_annular_ring = strictest(out.min_annular_ring, Some(t.ipc.annular_ring[class]));
    out.courtyard_excess = match choice.density.to_ascii_uppercase() {
        'A' => Some(t.ipc.courtyard_excess[0]),
        'B' => Some(t.ipc.courtyard_excess[1]),
        'C' => Some(t.ipc.courtyard_excess[2]),
        _ => None,
    };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(fabs: &[&str], ipc_class: u8, density: char) -> FabChoice {
        FabChoice { fabs: fabs.iter().map(|s| s.to_string()).collect(), ipc_class, density }
    }

    #[test]
    fn every_fab_has_source_and_date() {
        let all = fabs();
        assert_eq!(all.len(), 5);
        for f in all {
            assert!(f.source.starts_with("https://"), "{}", f.id);
            assert_eq!(f.checked.len(), 10, "{}", f.id);
        }
    }

    #[test]
    fn strictest_wins_across_fabs() {
        let l = limits(&choice(&["jlcpcb", "aisler"], 2, 'B'));
        assert_eq!(l.min_track, Some(0.20)); // Aisler
        assert_eq!(l.min_mask_web, Some(0.10));
        assert_eq!(l.min_via_drill, Some(0.3));
        // Only Aisler/JLC publish this, and a fab with null doesn't erase a value.
        let l = limits(&choice(&["pcbway", "eurocircuits"], 2, 'B'));
        assert_eq!(l.min_track, Some(0.10));
        assert_eq!(l.min_mask_web, Some(0.075));
    }

    #[test]
    fn ipc_class_raises_annular_ring_only_above_fab() {
        // Eurocircuits publishes no ring, so the IPC class is all there is.
        assert_eq!(limits(&choice(&["eurocircuits"], 3, 'B')).min_annular_ring, Some(0.05));
        // JLCPCB's 0.18 already exceeds class 3.
        assert_eq!(limits(&choice(&["jlcpcb"], 3, 'B')).min_annular_ring, Some(0.18));
    }

    #[test]
    fn empty_choice_gives_only_ipc_values() {
        let l = limits(&choice(&[], 3, 'A'));
        assert_eq!(l, FabLimits { min_annular_ring: Some(0.05), courtyard_excess: Some(0.5), ..Default::default() });
        assert_eq!(limits(&choice(&["nope"], 2, 'C')).courtyard_excess, Some(0.1));
    }
}
