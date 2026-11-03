//! The standardise rules on the Symbols and Footprints pages of Settings. Running them (dry run, apply) is on the Library tab.

use eframe::egui;
use klm_core::standardise::Rules;

/// A rule with a value in mm; unticking it switches it off.
fn mm_rule(ui: &mut egui::Ui, label: &str, value: &mut Option<f64>, default: Option<f64>) {
    ui.horizontal(|ui| {
        let mut on = value.is_some();
        if ui.checkbox(&mut on, label).changed() {
            *value = if on { default } else { None };
        }
        if let Some(v) = value {
            ui.add(egui::DragValue::new(v).speed(0.01).range(0.01..=10.0).suffix(" mm"));
        }
    });
}

pub fn symbols(ui: &mut egui::Ui, rules: &mut Rules) {
    let d = Rules::default();
    mm_rule(ui, "Text size", &mut rules.symbol_text_size, d.symbol_text_size);
    mm_rule(ui, "Minimum pin length (not for 2-pin or power symbols)", &mut rules.pin_length_min, d.pin_length_min);
    mm_rule(ui, "Line width", &mut rules.symbol_line_width, d.symbol_line_width);
    ui.checkbox(&mut rules.symbol_field_positions, "Reference above the body, Value below");
}

pub fn reset_symbols(rules: &mut Rules) {
    let d = Rules::default();
    (rules.symbol_text_size, rules.pin_length_min, rules.symbol_line_width, rules.symbol_field_positions) = (d.symbol_text_size, d.pin_length_min, d.symbol_line_width, d.symbol_field_positions);
}

pub fn footprints(ui: &mut egui::Ui, rules: &mut Rules) {
    let d = Rules::default();
    mm_rule(ui, "Silkscreen line width", &mut rules.silk_width, d.silk_width);
    mm_rule(ui, "Fab line width", &mut rules.fab_width, d.fab_width);
    mm_rule(ui, "Courtyard line width", &mut rules.courtyard_width, d.courtyard_width);
    ui.checkbox(&mut rules.footprint_ref_value, "Reference above the courtyard, Value below");
    ui.checkbox(&mut rules.footprint_origin, "Origin at the pad centre (SMD) or pin 1 (through-hole)");
    ui.checkbox(&mut rules.pin1_marker, "Add a pin-1 marker where there is none");
}

pub fn reset_footprints(rules: &mut Rules) {
    let d = Rules::default();
    (rules.silk_width, rules.fab_width, rules.courtyard_width, rules.footprint_ref_value, rules.footprint_origin, rules.pin1_marker) = (d.silk_width, d.fab_width, d.courtyard_width, d.footprint_ref_value, d.footprint_origin, d.pin1_marker);
}
