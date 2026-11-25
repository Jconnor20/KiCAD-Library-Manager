use super::batch_dialog;
use super::preview::PreviewPanel;
use super::ui_kit::{self, Level};
use eframe::egui::{self, RichText};
use klm_core::library::{LibraryRoot, PartEdit, Scope};
use klm_core::rules::Rules;
use klm_core::store::{Alternative, Store};

pub enum EditAction {
    None,
    Cancel,
    Apply {
        scope: Scope,
        targets: Vec<(String, String)>,
        edit: PartEdit,
    },
    /// The Delete… button: the Library tab shows what deleting these would remove.
    Delete {
        targets: Vec<(String, String)>,
    },
}

/// Fields that take a whole row; the others sit two to a row.
const WIDE: [&str; 3] = ["Description", "Datasheet", "Tags"];

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
    /// One part only: its pictures and the alternatives marked for it.
    preview: Option<PreviewPanel>,
    alternatives: Vec<Alternative>,
}

impl EditDialog {
    pub fn new(ctx: &egui::Context, scope: Scope, targets: Vec<(String, String)>, root: &LibraryRoot, rules: Rules) -> Self {
        let single = targets.len() == 1;
        let current = if single { root.part_fields(&targets[0].0, &targets[0].1).unwrap_or_default() } else { vec![] };
        let mpn = current.iter().find(|(k, _)| *k == "MPN").map(|(_, v)| v.clone()).unwrap_or_default();
        let fields = if single {
            current.into_iter().map(|(k, v)| (k, v.clone(), v)).collect()
        } else {
            klm_core::library::EDIT_FIELDS.iter().filter(|k| **k != "MPN").map(|&k| (k, String::new(), String::new())).collect()
        };
        let mut categories = root.categories();
        categories.sort();
        let alternatives = if single {
            Store::default_path().and_then(|p| Store::open(&p).ok()).and_then(|s| klm_core::alternatives::alternatives_of(&s, &mpn).ok()).unwrap_or_default()
        } else {
            vec![]
        };
        let preview = single.then(|| PreviewPanel::for_library(ctx, root, &targets[0].0, &targets[0].1));
        Self { scope, category: if single { targets[0].0.clone() } else { String::new() }, targets, categories, fields, rules, preview, alternatives }
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
            fields: self
                .fields
                .iter()
                .filter(|(_, now, was)| now.trim() != was)
                .map(|(k, now, _)| (k.to_string(), if *k == "MPN" { self.rules.normalise_mpn(now) } else { now.trim().to_string() }))
                .collect(),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> EditAction {
        let mut action = EditAction::None;
        let edit = self.edit();
        let changed = edit.category.is_some() || !edit.fields.is_empty();
        let single = self.targets.len() == 1;
        let (title, sub) = match self.targets.as_slice() {
            [(cat, name)] => (name.clone(), format!("{} · {cat}", if self.scope == Scope::Global { "Global" } else { "Project" })),
            many => (format!("Edit {} parts", many.len()), String::new()),
        };
        let width = if single { 900.0 } else { 560.0 };
        // The panes are borrowed one at a time inside the closures, so take what each needs first.
        let suggestion = self.suggested();
        let (targets, scope) = (self.targets.clone(), self.scope);
        let dismissed = batch_dialog::dialog(
            ctx,
            egui::Id::NULL.with("edit_dialog"),
            width,
            &title,
            (!sub.is_empty()).then_some(sub.as_str()),
            |ui| {
                ui.horizontal_top(|ui| {
                    let side = if single { 300.0 } else { 0.0 };
                    let left = (ui.available_width() - side - if single { 18.0 } else { 0.0 }).max(240.0);
                    ui.vertical(|ui| {
                        ui.set_width(left);
                        self.fields_pane(ui, suggestion, single);
                    });
                    if single {
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.set_width(side);
                            self.side_pane(ui);
                        });
                    }
                });
            },
            |ui| {
                ui.horizontal(|ui| {
                    if ui.add(ui_kit::danger("Delete…", false, ui.visuals())).on_hover_text("Show what deleting this part would remove").clicked() {
                        action = EditAction::Delete { targets: targets.clone() };
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(changed, ui_kit::primary("Save", ui.visuals())).clicked() {
                            action = EditAction::Apply { scope, targets: targets.clone(), edit: edit.clone() };
                        }
                        if ui.button("Cancel").clicked() {
                            action = EditAction::Cancel;
                        }
                        ui.weak("Changes are written when you save. Undo from History.");
                    });
                });
            },
        );
        if dismissed && matches!(action, EditAction::None) {
            action = EditAction::Cancel;
        }
        action
    }

    /// Category and the metadata fields, label above box, two to a row where they are short.
    fn fields_pane(&mut self, ui: &mut egui::Ui, suggestion: Option<String>, single: bool) {
        ui_kit::section_label(ui, "Fields");
        ui.add_space(2.0);
        ui.label(RichText::new("Category").weak().size(12.0));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.category).hint_text("unchanged").desired_width(ui.available_width() - 110.0));
            egui::ComboBox::from_id_salt("edit_category").selected_text("existing").show_ui(ui, |ui| {
                for c in &self.categories {
                    ui.selectable_value(&mut self.category, c.clone(), c);
                }
            });
        });
        if let Some(c) = suggestion
            && ui.button(format!("Use {c}")).on_hover_text("Category from your naming rules").clicked()
        {
            self.category = c;
        }
        let field = |ui: &mut egui::Ui, f: &mut (&'static str, String, String)| {
            ui.label(RichText::new(f.0).weak().size(12.0));
            ui.add(egui::TextEdit::singleline(&mut f.1).desired_width(f32::INFINITY));
        };
        let mut i = 0;
        while i < self.fields.len() {
            let pair = i + 1 < self.fields.len() && !WIDE.contains(&self.fields[i].0) && !WIDE.contains(&self.fields[i + 1].0);
            if pair {
                if let [a, b] = &mut self.fields[i..i + 2] {
                    ui.columns(2, |cols| {
                        field(&mut cols[0], a);
                        field(&mut cols[1], b);
                    });
                }
                i += 2;
            } else {
                field(ui, &mut self.fields[i]);
                i += 1;
            }
        }
        ui.add_space(4.0);
        ui.weak(if single {
            "A new category moves the part's symbol, footprint and 3D model there. A new MPN also renames the symbol."
        } else {
            "Only the boxes you fill in change. A new category moves each part's symbol, footprint and 3D model there."
        });
    }

    /// Symbol and footprint pictures, then the alternatives marked for the part.
    fn side_pane(&mut self, ui: &mut egui::Ui) {
        ui_kit::section_label(ui, "Symbol and footprint");
        ui.add_space(2.0);
        if let Some(p) = &mut self.preview {
            p.inspector(ui);
        }
        ui.add_space(10.0);
        ui_kit::section_label(ui, &format!("Alternatives · {}", self.alternatives.len()));
        ui.add_space(2.0);
        if self.alternatives.is_empty() {
            ui.weak("None marked yet. Alternatives… on the Library page finds some.");
        }
        for a in &self.alternatives {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&a.mpn).monospace());
                ui.weak(&a.manufacturer);
                ui_kit::badge(ui, &a.role, if a.role == "preferred" { Level::Ok } else { Level::Info });
            });
        }
    }
}
