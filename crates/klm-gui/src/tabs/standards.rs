//! The "Standards" section of Settings: the standardise rules, a dry run of them, and applying it.

use super::library;
use crate::{AppCtx, Msg as AppMsg};
use eframe::egui;
use klm_core::standardise::{self, Plan, Rules};
use std::sync::mpsc::{Receiver, channel};

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
    /// The dry run or apply on a worker, and where it reports back.
    working: Option<(&'static str, Receiver<Option<Vec<Plan>>>)>,
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
        if let Some((_, rx)) = &self.working {
            if let Ok(done) = rx.try_recv() {
                self.plans.extend(done.into_iter().flatten());
                self.working = None;
            } else {
                ui.ctx().request_repaint();
            }
        }
        let idle = self.working.is_none();
        let mut action = Action::None;
        ui.horizontal(|ui| {
            if ui.add_enabled(idle, egui::Button::new("Dry run")).on_hover_text("Shows what would change in the global and project libraries, without changing anything").clicked() {
                action = Action::Preview;
            }
            let any = self.plans.iter().any(|p| !p.files.is_empty());
            if ui.add_enabled(any && idle, egui::Button::new("Apply")).on_hover_text("Takes a snapshot of each library first, so it can be rolled back from History").clicked() {
                action = Action::Apply;
            }
            if let Some((what, _)) = &self.working {
                ui.spinner();
                ui.label(*what);
            }
        });
        if !self.plans.is_empty() {
            let text: String = self.plans.iter().map(|p| p.to_string()).collect();
            egui::ScrollArea::vertical().id_salt("standards_plan").max_height(200.0).show(ui, |ui| ui.monospace(text));
        }
        action
    }

    pub fn act(&mut self, action: Action, cx: &mut AppCtx) {
        match action {
            Action::None => {}
            Action::Save => cx.save_settings(),
            Action::Preview => {
                let roots = cx.roots();
                if roots.is_empty() {
                    cx.log.push("Standardise: no library to check; choose a global library or open a project".into());
                    return;
                }
                let (rules, ctx) = (cx.settings.standards.clone(), cx.ctx.clone());
                let (tx2, rx) = channel();
                std::thread::spawn(move || {
                    let _ = tx2.send(Some(roots.iter().map(|r| standardise::plan(r, &rules)).collect()));
                    ctx.request_repaint();
                });
                self.working = Some(("Checking the libraries…", rx));
            }
            // An import running at the same time could write a file between the check and the write.
            Action::Apply if cx.busy => cx.log.push("Standardise: wait for the running import or download to finish".into()),
            Action::Apply => {
                let plans = std::mem::take(&mut self.plans);
                let (app_tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    for plan in plans {
                        let line = match standardise::apply(&plan) {
                            Ok(n) => format!("Standardised {n} file(s) in {}", plan.root.dir.display()),
                            Err(e) => format!("Standardise failed, nothing changed in {}: {e:#}", plan.root.dir.display()),
                        };
                        let _ = app_tx.send(AppMsg::Log(line));
                    }
                    let _ = app_tx.send(AppMsg::Library(library::Msg::Refresh));
                    let _ = tx.send(None);
                    ctx.request_repaint();
                });
                self.working = Some(("Applying…", rx));
            }
        }
    }
}
