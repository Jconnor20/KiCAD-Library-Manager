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

/// Every fab known, in the JSON's order.
pub fn fabs() -> Vec<Fab> {
    todo!("W10.0: parse the embedded fab_profiles.json")
}

/// The limits `choice` gives: the strictest of its fabs, combined with its IPC class and density.
pub fn limits(choice: &FabChoice) -> FabLimits {
    let _ = choice;
    todo!("W10.0: strictest of the fabs, then the IPC-6012 class and IPC-7351 density tables")
}
