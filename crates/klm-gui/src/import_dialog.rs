use eframe::egui;
use klm_core::library::LibraryRoot;
use klm_core::part::Part;

pub enum DialogAction {
    None,
    Cancel,
    Import { part: Part, category: String, targets: Vec<LibraryRoot> },
}

/// Review a detected part, pick its category and where it goes.
pub struct ImportDialog {
    pub index: usize,
    part: Part,
    category: String,
    categories: Vec<String>,
    to_global: bool,
    to_project: bool,
    /// Names of libraries that already hold this MPN, shown once Import is pressed.
    clashes: Vec<String>,
}

impl ImportDialog {
    pub fn new(index: usize, part: &Part, categories: Vec<String>, has_global: bool, has_project: bool) -> Self {
        Self {
            index,
            category: klm_core::categories::guess(&part.category_hint).to_string(),
            part: part.clone(),
            categories,
            to_global: has_global,
            to_project: has_project && !has_global,
            clashes: vec![],
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> DialogAction {
        let mut action = DialogAction::None;
        let modal = egui::Modal::new(egui::Id::unique("import_dialog")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Import part");
            ui.weak(&self.part.source);
            ui.add_space(6.0);
            egui::Grid::new("fields").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("MPN");
                ui.text_edit_singleline(&mut self.part.mpn);
                ui.end_row();
                ui.label("Manufacturer");
                ui.text_edit_singleline(&mut self.part.manufacturer);
                ui.end_row();
                ui.label("Description");
                ui.text_edit_singleline(&mut self.part.description);
                ui.end_row();
                ui.label("Category");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("category").selected_text(&self.category).show_ui(ui, |ui| {
                        for c in &self.categories {
                            ui.selectable_value(&mut self.category, c.clone(), c);
                        }
                    });
                    ui.add(egui::TextEdit::singleline(&mut self.category).hint_text("or type a new one").desired_width(140.0));
                });
                ui.end_row();
                ui.label("Add to");
                ui.horizontal(|ui| {
                    ui.add_enabled(global.is_some(), egui::Checkbox::new(&mut self.to_global, "Global"));
                    ui.add_enabled(project.is_some(), egui::Checkbox::new(&mut self.to_project, "Project"));
                });
                ui.end_row();
            });

            let files = [
                ("symbol", self.part.symbol_file.is_some() as usize),
                ("footprint", self.part.footprint_files.len()),
                ("3D model", self.part.model_files.len().min(1)),
                ("datasheet", (self.part.datasheet_file.is_some() || !self.part.datasheet_url.is_empty()) as usize),
                ("sim model", self.part.sim_files.len().min(1)),
            ];
            let have: Vec<_> = files.iter().filter(|(_, n)| *n > 0).map(|(f, _)| *f).collect();
            let missing: Vec<_> = files.iter().filter(|(_, n)| *n == 0).map(|(f, _)| *f).collect();
            ui.add_space(6.0);
            ui.label(format!("Includes: {}", have.join(", ")));
            if !missing.is_empty() {
                ui.weak(format!("Not in this download: {}", missing.join(", ")));
            }

            let targets: Vec<LibraryRoot> = [(self.to_global, global), (self.to_project, project)]
                .into_iter()
                .filter_map(|(on, r)| if on { r.cloned() } else { None })
                .collect();
            let ready = !targets.is_empty() && !self.part.mpn.trim().is_empty() && !self.category.trim().is_empty();

            if !self.clashes.is_empty() {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().warn_fg_color, format!("{} is already in {}. Overwrite it?", self.part.mpn, self.clashes.join(" and ")));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = if self.clashes.is_empty() { "Import" } else { "Overwrite" };
                if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                    let clashes: Vec<String> = targets
                        .iter()
                        .filter(|r| r.has_part(&self.category, &self.part.mpn))
                        .map(|r| r.nickname(&self.category))
                        .collect();
                    if clashes.is_empty() || !self.clashes.is_empty() {
                        action = DialogAction::Import { part: self.part.clone(), category: self.category.trim().to_string(), targets };
                    } else {
                        self.clashes = clashes;
                    }
                }
                if ui.button("Cancel").clicked() {
                    action = DialogAction::Cancel;
                }
            });
        });
        if modal.should_close() && matches!(action, DialogAction::None) {
            action = DialogAction::Cancel;
        }
        action
    }
}
