//! The "Standards" section of Settings: the standardise rules, a dry run of them, and applying it.

use super::library;
use crate::{AppCtx, Msg as AppMsg};
use eframe::egui;
use klm_core::standardise::{self, Plan, Rules};

pub enum Action {
    None,
    /// A rule changed: save the settings.
    Save,
    Preview,
    Apply,
}

#[derive(Default)]
pub struct Standards {
    /// The last dry run, one per library; applying needs one.
    plans: Vec<Plan>,
}

/// A rule with a value in mm; unticking it switches it off.
fn mm_rule(ui: &mut egui::Ui, label: &str, value: &mut Option<f64>, default: Option<f64>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut on = value.is_some();
        if ui.checkbox(&mut on, label).changed() {
            *value = if on { default } else { None };
            changed = true;
        }
        if let Some(v) = value {
            changed |= ui.add(egui::DragValue::new(v).speed(0.01).range(0.01..=10.0).suffix(" mm")).changed();
        }
    });
    changed
}

impl Standards {
    pub fn show(&mut self, ui: &mut egui::Ui, rules: &mut Rules) -> Action {
        let d = Rules::default();
        let mut changed = false;
        ui.strong("Standards");
        ui.weak("Fixes that make parts follow the KiCad Library Conventions. Untick a rule to leave that alone, or change its value.");
        ui.label("Symbols");
        changed |= mm_rule(ui, "Text size", &mut rules.symbol_text_size, d.symbol_text_size);
        changed |= mm_rule(ui, "Minimum pin length (not for 2-pin or power symbols)", &mut rules.pin_length_min, d.pin_length_min);
        changed |= mm_rule(ui, "Line width", &mut rules.symbol_line_width, d.symbol_line_width);
        changed |= ui.checkbox(&mut rules.symbol_field_positions, "Reference above the body, Value below").changed();
        ui.label("Footprints");
        changed |= mm_rule(ui, "Silkscreen line width", &mut rules.silk_width, d.silk_width);
        changed |= mm_rule(ui, "Fab line width", &mut rules.fab_width, d.fab_width);
        changed |= mm_rule(ui, "Courtyard line width", &mut rules.courtyard_width, d.courtyard_width);
        changed |= ui.checkbox(&mut rules.footprint_ref_value, "Reference above the courtyard, Value below").changed();
        changed |= ui.checkbox(&mut rules.footprint_origin, "Origin at the pad centre (SMD) or pin 1 (through-hole)").changed();
        changed |= ui.checkbox(&mut rules.pin1_marker, "Add a pin-1 marker where there is none").changed();
        if changed {
            // The dry run no longer matches the rules.
            self.plans.clear();
            return Action::Save;
        }
        let mut action = Action::None;
        ui.horizontal(|ui| {
            if ui.button("Dry run").on_hover_text("Shows what would change in the global and project libraries, without changing anything").clicked() {
                action = Action::Preview;
            }
            let any = self.plans.iter().any(|p| !p.files.is_empty());
            if ui.add_enabled(any, egui::Button::new("Apply")).on_hover_text("Takes a snapshot of each library first, so it can be rolled back from History").clicked() {
                action = Action::Apply;
            }
        });
        if !self.plans.is_empty() {
            let text: String = self.plans.iter().map(|p| p.to_string()).collect();
            egui::ScrollArea::vertical().id_salt("standards_plan").max_height(200.0).show(ui, |ui| ui.monospace(text));
        }
        action
    }

    // shortcut: plan and apply run on the UI thread; move them to a worker if large libraries make the window stall.
    pub fn act(&mut self, action: Action, cx: &mut AppCtx) {
        match action {
            Action::None => {}
            Action::Save => cx.save_settings(),
            Action::Preview => {
                self.plans = cx.roots().iter().map(|r| standardise::plan(r, &cx.settings.standards)).collect();
                if self.plans.is_empty() {
                    cx.log.push("Standardise: no library to check; choose a global library or open a project".into());
                }
            }
            // An import running at the same time could write a file between the check and the write.
            Action::Apply if cx.busy => cx.log.push("Standardise: wait for the running import or download to finish".into()),
            Action::Apply => {
                for plan in std::mem::take(&mut self.plans) {
                    match standardise::apply(&plan) {
                        Ok(n) => cx.log.push(format!("Standardised {n} file(s) in {}", plan.root.dir.display())),
                        Err(e) => cx.log.push(format!("Standardise failed, nothing changed in {}: {e:#}", plan.root.dir.display())),
                    }
                }
                cx.send(AppMsg::Library(library::Msg::Refresh));
            }
        }
    }
}
