//! The standardise rules on the Symbols and Footprints pages of Settings. Running them (dry run, apply) is on the Library tab.

use eframe::egui;
use klm_core::settings::Settings;
use klm_core::standardise::Rules;
use klm_core::standardise::profile::{self, Profile};

/// The active standard profile: pick, save a copy under a new name, import or export. Returns an error to log.
/// Switching drops unsaved edits to the previous profile, as Cancel would.
pub fn profile(ui: &mut egui::Ui, d: &mut Settings) -> Option<String> {
    let mut err = None;
    let current = d.profile_name().to_string();
    ui.horizontal(|ui| {
        ui.label("Profile");
        egui::ComboBox::from_id_salt("standard_profile").selected_text(&current).show_ui(ui, |ui| {
            for name in profile::list() {
                if ui.selectable_label(name == current, &name).clicked() && name != current {
                    match profile::load(&name) {
                        Ok(p) => (d.profile, d.standard) = (p.name.clone(), p),
                        Err(e) => err = Some(format!("{e:#}")),
                    }
                }
            }
        });
        if ui.button("Import…").on_hover_text("Add a profile file someone shared").clicked() {
            if let Some(f) = rfd::FileDialog::new().add_filter("Standard profile", &["json"]).pick_file() {
                match profile::import(&f) {
                    Ok(p) => (d.profile, d.standard) = (p.name.clone(), p),
                    Err(e) => err = Some(format!("{e:#}")),
                }
            }
        }
        if ui.button("Export…").on_hover_text("Save this profile as a file to share").clicked() {
            if let Some(f) = rfd::FileDialog::new().add_filter("Standard profile", &["json"]).set_file_name(format!("{current}.json")).save_file() {
                let p = Profile { name: current.clone(), ..d.standard.clone() };
                err = profile::export(&p, &f).err().map(|e| format!("{e:#}"));
            }
        }
    });
    ui.horizontal(|ui| {
        let id = ui.make_persistent_id("new_profile_name");
        let mut name = ui.data_mut(|m| m.get_temp::<String>(id)).unwrap_or_default();
        ui.add(egui::TextEdit::singleline(&mut name).hint_text("new profile name").desired_width(160.0));
        if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Save as new profile")).clicked() {
            let p = Profile { name: name.trim().into(), ..d.standard.clone() };
            // Listing reads the folder, so it's only done on click, not every frame.
            let taken = profile::list().iter().any(|n| n.eq_ignore_ascii_case(&p.name));
            match if taken { Err(anyhow::anyhow!("a profile called \"{}\" already exists", p.name)) } else { profile::save(&p) } {
                Ok(()) => (d.profile, name) = (p.name, String::new()),
                Err(e) => err = Some(format!("{e:#}")),
            }
        }
        ui.data_mut(|m| m.insert_temp(id, name));
    });
    ui.weak("A profile is your house standard in one file. Copy or export it to share it with your team.");
    err
}

/// The classes with a house template of the user's own, each with a way back to the built-in.
pub fn house_templates(ui: &mut egui::Ui, p: &mut Profile) {
    let classes: std::collections::BTreeSet<String> = p.templates.keys().chain(p.footprint_styles.keys()).cloned().collect();
    if classes.is_empty() {
        ui.weak("None: every kind of part uses the built-in style. Right-click a part in the Library tab and choose \"Use as house template\" to set one.");
    }
    for class in classes {
        ui.horizontal(|ui| {
            ui.label(&class);
            if ui.button("Use built-in").on_hover_text("Forget this house symbol and footprint style").clicked() {
                p.templates.remove(&class);
                p.footprint_styles.remove(&class);
            }
        });
    }
}

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
