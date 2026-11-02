use eframe::egui;
use klm_core::library::{LibraryRoot, PartEdit, Scope};
use klm_core::rules::Rules;

pub enum EditAction {
    None,
    Cancel,
    Apply { scope: Scope, targets: Vec<(String, String)>, edit: PartEdit },
}

/// Edits the metadata of one part, or of several at once. With several, every box starts empty and only
/// the ones you fill in are applied (so a field can't be cleared in bulk).
pub struct EditDialog {
    scope: Scope,
    /// (category, symbol name) of each part being edited.
    targets: Vec<(String, String)>,
    categories: Vec<String>,
    category: String,
    /// (field, text, text when the dialog opened).
    fields: Vec<(&'static str, String, String)>,
    rules: Rules,
}

impl EditDialog {
    pub fn new(scope: Scope, targets: Vec<(String, String)>, root: &LibraryRoot, rules: Rules) -> Self {
        let single = targets.len() == 1;
        let current = if single { root.part_fields(&targets[0].0, &targets[0].1).unwrap_or_default() } else { vec![] };
        let fields = if single {
            current.into_iter().map(|(k, v)| (k, v.clone(), v)).collect()
        } else {
            klm_core::library::EDIT_FIELDS.iter().filter(|k| **k != "MPN").map(|&k| (k, String::new(), String::new())).collect()
        };
        let mut categories = root.categories();
        categories.sort();
        Self { scope, category: if single { targets[0].0.clone() } else { String::new() }, targets, categories, fields, rules }
    }

    /// The category the user's rules give for the text in the boxes.
    fn suggested(&self) -> Option<String> {
        let text = self.fields.iter().map(|f| f.1.as_str()).collect::<Vec<_>>().join(" ");
        self.rules.rule_category(&text).map(str::to_string).filter(|c| *c != self.category)
    }

    fn edit(&self) -> PartEdit {
        let current = (self.targets.len() == 1).then(|| self.targets[0].0.as_str());
        PartEdit {
            category: Some(self.category.trim().to_string()).filter(|c| !c.is_empty() && Some(c.as_str()) != current),
            fields: self.fields.iter().filter(|(_, now, was)| now.trim() != was).map(|(k, now, _)| (k.to_string(), if *k == "MPN" { self.rules.normalise_mpn(now) } else { now.trim().to_string() })).collect(),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> EditAction {
        let mut action = EditAction::None;
        let modal = egui::Modal::new(egui::Id::unique("edit_dialog")).show(ctx, |ui| {
            ui.set_width(460.0);
            match self.targets.as_slice() {
                [(_, name)] => ui.heading(format!("Edit {name}")),
                many => ui.heading(format!("Edit {} parts", many.len())),
            };
            ui.add_space(6.0);
            egui::Grid::new("edit_fields").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Category");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.category).hint_text("unchanged").desired_width(200.0));
                    egui::ComboBox::from_id_salt("edit_category").selected_text("existing").show_ui(ui, |ui| {
                        for c in &self.categories {
                            ui.selectable_value(&mut self.category, c.clone(), c);
                        }
                    });
                    if let Some(c) = self.suggested() {
                        if ui.button(format!("Use {c}")).on_hover_text("Category from your naming rules").clicked() {
                            self.category = c;
                        }
                    }
                });
                ui.end_row();
                for (key, text, _) in &mut self.fields {
                    ui.label(*key);
                    ui.add(egui::TextEdit::singleline(text).desired_width(330.0));
                    ui.end_row();
                }
            });
            ui.add_space(4.0);
            ui.weak("A new category moves the part's symbol, footprint and 3D model there. A new MPN also renames the symbol.");

            let edit = self.edit();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let changed = edit.category.is_some() || !edit.fields.is_empty();
                if ui.add_enabled(changed, egui::Button::new("Apply")).clicked() {
                    action = EditAction::Apply { scope: self.scope, targets: self.targets.clone(), edit };
                }
                if ui.button("Cancel").clicked() {
                    action = EditAction::Cancel;
                }
            });
        });
        if modal.should_close() && matches!(action, EditAction::None) {
            action = EditAction::Cancel;
        }
        action
    }
}
