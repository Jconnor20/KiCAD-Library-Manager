use eframe::egui;
use klm_core::credentials::{self, SOURCES};
use klm_core::settings::Settings;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub enum SettingsAction {
    None,
    /// Source switches or stored credentials changed: save the settings and rebuild the sources.
    Rebuild,
    /// Run a search with the stored credentials of this source.
    Test(&'static str),
    /// New Downloads folder to watch (`None` = the OS default).
    Downloads(Option<PathBuf>),
    Note(String),
}

/// Credential entry boxes only ever hold what the user is typing; stored secrets are never read back into the UI.
pub struct SettingsView {
    inputs: HashMap<(&'static str, &'static str), String>,
    stored: HashSet<(&'static str, &'static str)>,
}

impl SettingsView {
    pub fn new() -> Self {
        let mut v = Self { inputs: HashMap::new(), stored: HashSet::new() };
        v.refresh();
        v
    }

    fn refresh(&mut self) {
        self.stored = SOURCES
            .iter()
            .flat_map(|s| s.fields.iter().map(move |f| (s.name, f.0)))
            .filter(|(s, f)| credentials::get(s, f).is_some())
            .collect();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, settings: &mut Settings, testing: bool) -> SettingsAction {
        let mut action = SettingsAction::None;
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.heading("Settings");
            ui.strong("Search sources");
            ui.weak("Keys are kept in Windows Credential Manager, not in the settings file. A source is searched when it is ticked and has all its keys.");
            for info in SOURCES {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.strong(info.name);
                    if info.searchable {
                        let mut on = settings.is_enabled(info.name);
                        if ui.checkbox(&mut on, "Enabled").changed() {
                            settings.source_enabled.insert(info.name.into(), on);
                            action = SettingsAction::Rebuild;
                        }
                    } else {
                        ui.weak("stored for later, no search source yet");
                    }
                });
                egui::Grid::new(info.name).num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                    for (key, label) in info.fields {
                        ui.label(*label);
                        let text = self.inputs.entry((info.name, key)).or_default();
                        ui.add(egui::TextEdit::singleline(text).password(true).desired_width(300.0));
                        ui.weak(if self.stored.contains(&(info.name, *key)) { "stored" } else { "not set" });
                        ui.end_row();
                    }
                });
                ui.horizontal(|ui| {
                    let typed = info.fields.iter().any(|f| self.inputs.get(&(info.name, f.0)).is_some_and(|t| !t.trim().is_empty()));
                    if ui.add_enabled(typed, egui::Button::new("Save keys")).clicked() {
                        for (key, _) in info.fields {
                            let text = self.inputs.remove(&(info.name, *key)).unwrap_or_default();
                            if text.trim().is_empty() {
                                continue;
                            }
                            if let Err(e) = credentials::set(info.name, key, text.trim()) {
                                action = SettingsAction::Note(format!("{}: {e:#}", info.name));
                            }
                        }
                        self.refresh();
                        if matches!(action, SettingsAction::None) {
                            action = SettingsAction::Rebuild;
                        }
                    }
                    if ui.button("Clear keys").clicked() {
                        for (key, _) in info.fields {
                            self.inputs.remove(&(info.name, *key));
                            if let Err(e) = credentials::clear(info.name, key) {
                                action = SettingsAction::Note(format!("{}: {e:#}", info.name));
                            }
                        }
                        self.refresh();
                        if matches!(action, SettingsAction::None) {
                            action = SettingsAction::Rebuild;
                        }
                    }
                    let complete = info.fields.iter().all(|f| self.stored.contains(&(info.name, f.0)));
                    if info.searchable && ui.add_enabled(complete && !testing, egui::Button::new("Test")).on_hover_text("Runs a search for LM358").clicked() {
                        action = SettingsAction::Test(info.name);
                    }
                });
            }

            ui.add_space(16.0);
            ui.strong("Downloads folder");
            ui.weak("KLM watches this folder for vendor ZIPs (SnapMagic, Ultra Librarian, SamacSys).");
            ui.horizontal(|ui| {
                ui.label(settings.downloads().map_or("none found".into(), |d| d.display().to_string()));
                if ui.button("Browse…").clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_title("Folder to watch for vendor ZIPs").pick_folder() {
                        action = SettingsAction::Downloads(Some(d));
                    }
                }
                if settings.downloads_dir.is_some() && ui.button("Use default").clicked() {
                    action = SettingsAction::Downloads(None);
                }
            });
        });
        action
    }
}
