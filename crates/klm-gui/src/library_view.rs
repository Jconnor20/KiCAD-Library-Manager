use eframe::egui::{self, Color32, RichText};
use klm_core::library::{ATTACH_EXTENSIONS, Check, LibraryRoot, PartStatus, Scope};
use std::path::PathBuf;

/// Something the user asked for on a row; `main` carries it out and refreshes the view.
pub enum LibraryAction {
    None,
    Attach { scope: Scope, category: String, name: String, path: PathBuf },
    /// Copy a global part into the project library; `overwrite` replaces an existing copy.
    CopyToProject { category: String, name: String, overwrite: bool },
}

/// Table of the parts in the global or project library, with what each one is missing.
pub struct LibraryView {
    scope: Scope,
    filter: String,
    only_incomplete: bool,
    /// Cached listing; `None` means re-read from disk on the next frame.
    parts: Option<Vec<PartStatus>>,
}

impl Default for LibraryView {
    fn default() -> Self {
        Self { scope: Scope::Global, filter: String::new(), only_incomplete: false, parts: None }
    }
}

/// Sim models are optional, so only a broken one counts against a part.
fn incomplete(p: &PartStatus) -> bool {
    let bad = |c: Check| matches!(c, Check::Missing | Check::Broken);
    bad(p.footprint) || bad(p.model) || bad(p.datasheet) || p.sim == Check::Broken || !p.in_kicad
}

fn check_cell(ui: &mut egui::Ui, c: Check, what: &str) {
    let (text, color, tip) = match c {
        Check::Ok => ("✔", Color32::from_rgb(60, 170, 90), format!("{what} present")),
        Check::Link => ("link", ui.visuals().hyperlink_color, format!("{what} points at a web page or another library")),
        Check::Missing => ("missing", ui.visuals().warn_fg_color, format!("No {what}")),
        Check::Broken => ("broken", ui.visuals().error_fg_color, format!("{what} is set but its file isn't there")),
    };
    ui.label(RichText::new(text).color(color)).on_hover_text(tip);
}

impl LibraryView {
    /// Forces a re-read, e.g. after an import.
    pub fn refresh(&mut self) {
        self.parts = None;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> LibraryAction {
        ui.horizontal(|ui| {
            let before = self.scope;
            ui.add_enabled_ui(global.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Global, "Global"));
            ui.add_enabled_ui(project.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Project, "Project"));
            if self.scope != before {
                self.parts = None;
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter by MPN, maker, category…").desired_width(220.0));
            ui.checkbox(&mut self.only_incomplete, "Only incomplete");
            if ui.button("Refresh").clicked() {
                self.parts = None;
            }
        });

        let root = match self.scope {
            Scope::Global => global,
            Scope::Project => project,
        };
        let Some(root) = root else {
            ui.label(match self.scope {
                Scope::Global => "Set up a global library first.",
                Scope::Project => "Choose a project first.",
            });
            return LibraryAction::None;
        };
        let parts = self.parts.get_or_insert_with(|| root.list_parts());

        let bad = parts.iter().filter(|p| incomplete(p)).count();
        ui.weak(format!("{} parts in {} · {bad} incomplete", parts.len(), root.dir.display()));
        ui.add_space(4.0);

        let needle = self.filter.to_lowercase();
        let shown: Vec<&PartStatus> = parts
            .iter()
            .filter(|p| !self.only_incomplete || incomplete(p))
            .filter(|p| needle.is_empty() || [&p.mpn, &p.manufacturer, &p.category, &p.description].iter().any(|f| f.to_lowercase().contains(&needle)))
            .collect();

        let mut action = LibraryAction::None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("library").striped(true).num_columns(9).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["Category", "MPN", "Manufacturer", "Footprint", "3D model", "Datasheet", "Sim model", "In KiCad", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for p in shown {
                    ui.label(&p.category);
                    let mpn = ui.label(RichText::new(&p.mpn).strong());
                    if !p.description.is_empty() {
                        mpn.on_hover_text(&p.description);
                    }
                    ui.label(&p.manufacturer);
                    check_cell(ui, p.footprint, "Footprint");
                    check_cell(ui, p.model, "3D model");
                    check_cell(ui, p.datasheet, "Datasheet");
                    check_cell(ui, p.sim, "Sim model");
                    check_cell(ui, if p.in_kicad { Check::Ok } else { Check::Missing }, "KiCad library entry");
                    ui.horizontal(|ui| {
                        if ui.small_button("Attach…").on_hover_text("Add a datasheet (PDF), SPICE model or 3D model (STEP/WRL)").clicked() {
                            let picked = rfd::FileDialog::new().add_filter("Datasheet, sim or 3D model", ATTACH_EXTENSIONS).pick_file();
                            if let Some(path) = picked {
                                action = LibraryAction::Attach { scope: self.scope, category: p.category.clone(), name: p.name.clone(), path };
                            }
                        }
                        if self.scope == Scope::Global {
                            let copy = ui.add_enabled(project.is_some(), egui::Button::new("Copy to project").small());
                            if copy.on_hover_text("Copy this part into the project's library. Shift-click to replace a copy already there.").clicked() {
                                let overwrite = ui.input(|i| i.modifiers.shift);
                                action = LibraryAction::CopyToProject { category: p.category.clone(), name: p.name.clone(), overwrite };
                            }
                        }
                        if let Some(target) = &p.datasheet_target {
                            if ui.small_button("Open datasheet").clicked() {
                                let _ = open::that(target);
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        });
        action
    }
}
