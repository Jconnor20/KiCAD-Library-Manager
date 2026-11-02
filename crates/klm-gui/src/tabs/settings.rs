use super::{import, search, standards};
use crate::{AppCtx, Msg as AppMsg};
use eframe::egui;
use klm_core::credentials::{self, SOURCES};
use klm_core::kicad_env;
use klm_core::rules::{MpnCase, Rules, TEMPLATE_FIELDS};
use klm_core::settings::Settings;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub enum Msg {
    TestDone,
}

enum Action {
    None,
    /// Source switches or stored credentials changed: save the settings and rebuild the sources.
    Rebuild,
    /// Run a search with the stored credentials of this source.
    Test(&'static str),
    /// New Downloads folder to watch (`None` = the OS default).
    Downloads(Option<PathBuf>),
    /// New `kicad-cli` to use (`None` = find it automatically).
    KicadCli(Option<PathBuf>),
    TestCli,
    Note(String),
    /// Naming rules changed: save the settings.
    SaveNaming,
    Standards(standards::Action),
}

/// The Settings popup. Credential entry boxes only ever hold what the user is typing; stored secrets are never read back into the UI.
pub struct SettingsTab {
    open: bool,
    testing: bool,
    inputs: HashMap<(&'static str, &'static str), String>,
    stored: HashSet<(&'static str, &'static str)>,
    standards: standards::Standards,
}

impl SettingsTab {
    pub fn new() -> Self {
        let mut v = Self { open: false, testing: false, inputs: HashMap::new(), stored: HashSet::new(), standards: Default::default() };
        v.refresh();
        v
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn handle(&mut self, msg: Msg, _cx: &mut AppCtx) {
        match msg {
            Msg::TestDone => self.testing = false,
        }
    }

    /// The popup itself; shown whatever tab is selected.
    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        if !self.open {
            return;
        }
        let ctx = cx.ctx.clone();
        let mut action = Action::None;
        let modal = egui::Modal::new(egui::Id::unique("settings")).show(&ctx, |ui| {
            ui.set_width(560.0);
            ui.set_max_height(520.0);
            action = self.show(ui, &mut cx.settings, self.testing);
        });
        if modal.should_close() {
            self.open = false;
        }
        match action {
            Action::None => {}
            Action::Rebuild => self.apply_sources(cx),
            Action::Test(name) => self.test_source(name, cx),
            Action::Note(n) => cx.log.push(n),
            Action::Downloads(dir) => {
                cx.settings.downloads_dir = dir;
                cx.save_settings();
                cx.send(AppMsg::Import(import::Msg::RestartWatcher));
            }
            Action::KicadCli(path) => {
                kicad_env::set_kicad_cli_override(path.clone());
                cx.settings.kicad_cli = path;
                cx.save_settings();
            }
            Action::TestCli => self.test_cli(cx),
            Action::SaveNaming => cx.save_settings(),
            Action::Standards(a) => self.standards.act(a, cx),
        }
    }

    fn test_cli(&mut self, cx: &mut AppCtx) {
        self.testing = true;
        let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let line = match kicad_env::kicad_cli_version() {
                Ok(v) => format!("kicad-cli: test OK, version {v}"),
                Err(e) => format!("kicad-cli: test failed: {e:#}"),
            };
            let _ = tx.send(AppMsg::Log(line));
            let _ = tx.send(AppMsg::Settings(Msg::TestDone));
            ctx.request_repaint();
        });
    }

    /// Saves the settings and rebuilds the source list from them and the stored credentials.
    fn apply_sources(&mut self, cx: &mut AppCtx) {
        cx.save_settings();
        cx.sources = Arc::new(klm_core::sources::all(&cx.settings));
        cx.send(AppMsg::Search(search::Msg::ClearResults));
    }

    fn test_source(&mut self, name: &'static str, cx: &mut AppCtx) {
        self.testing = true;
        let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let result = klm_core::sources::build(name).ok_or_else(|| anyhow::anyhow!("keys missing")).and_then(|s| s.search("LM358"));
            let line = match result {
                Ok(parts) => format!("{name}: test OK, {} results for LM358", parts.len()),
                Err(e) => format!("{name}: test failed: {e:#}"),
            };
            let _ = tx.send(AppMsg::Log(line));
            let _ = tx.send(AppMsg::Settings(Msg::TestDone));
            ctx.request_repaint();
        });
    }

    fn refresh(&mut self) {
        self.stored = SOURCES
            .iter()
            .flat_map(|s| s.fields.iter().map(move |f| (s.name, f.0)))
            .filter(|(s, f)| credentials::get(s, f).is_some())
            .collect();
    }

    fn show(&mut self, ui: &mut egui::Ui, settings: &mut Settings, testing: bool) -> Action {
        let mut action = Action::None;
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
                            action = Action::Rebuild;
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
                    // LCSC needs no keys, so it only gets the Test button.
                    if !info.fields.is_empty() && ui.add_enabled(typed, egui::Button::new("Save keys")).clicked() {
                        for (key, _) in info.fields {
                            let text = self.inputs.remove(&(info.name, *key)).unwrap_or_default();
                            if text.trim().is_empty() {
                                continue;
                            }
                            if let Err(e) = credentials::set(info.name, key, text.trim()) {
                                action = Action::Note(format!("{}: {e:#}", info.name));
                            }
                        }
                        self.refresh();
                        if matches!(action, Action::None) {
                            action = Action::Rebuild;
                        }
                    }
                    if !info.fields.is_empty() && ui.button("Clear keys").clicked() {
                        for (key, _) in info.fields {
                            self.inputs.remove(&(info.name, *key));
                            if let Err(e) = credentials::clear(info.name, key) {
                                action = Action::Note(format!("{}: {e:#}", info.name));
                            }
                        }
                        self.refresh();
                        if matches!(action, Action::None) {
                            action = Action::Rebuild;
                        }
                    }
                    let complete = info.fields.iter().all(|f| self.stored.contains(&(info.name, f.0)));
                    if info.searchable && ui.add_enabled(complete && !testing, egui::Button::new("Test")).on_hover_text("Runs a search for LM358").clicked() {
                        action = Action::Test(info.name);
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
                        action = Action::Downloads(Some(d));
                    }
                }
                if settings.downloads_dir.is_some() && ui.button("Use default").clicked() {
                    action = Action::Downloads(None);
                }
            });

            ui.add_space(16.0);
            ui.strong("kicad-cli");
            ui.weak("Used to upgrade imported parts to your KiCad's format. Set it if KiCad isn't installed in the usual place.");
            ui.horizontal(|ui| {
                ui.label(kicad_env::kicad_cli().map_or("none found".into(), |p| p.display().to_string()));
                if ui.button("Browse…").clicked() {
                    if let Some(f) = rfd::FileDialog::new().set_title("kicad-cli executable").pick_file() {
                        action = Action::KicadCli(Some(f));
                    }
                }
                if settings.kicad_cli.is_some() && ui.button("Use default").clicked() {
                    action = Action::KicadCli(None);
                }
                if ui.add_enabled(!testing, egui::Button::new("Test")).on_hover_text("Runs kicad-cli version").clicked() {
                    action = Action::TestCli;
                }
            });

            ui.add_space(16.0);
            ui.strong("Shipping region");
            ui.weak("Country code (US, DE, ...) used for BOM stock and prices; each project can override it. Only DigiKey uses it so far.");
            if ui.add(egui::TextEdit::singleline(&mut settings.shipping_region).hint_text("e.g. DE").desired_width(80.0)).lost_focus() {
                action = Action::SaveNaming;
            }

            if naming_section(ui, &mut settings.rules) {
                action = Action::SaveNaming;
            }
            ui.add_space(16.0);
            match self.standards.show(ui, &mut settings.standards) {
                standards::Action::None => {}
                a => action = Action::Standards(a),
            }
        });
        action
    }
}

/// The "Naming" section; true when a rule changed.
fn naming_section(ui: &mut egui::Ui, rules: &mut Rules) -> bool {
    let before = rules.clone();
    ui.add_space(16.0);
    ui.strong("Naming");
    ui.weak("Category rules are checked in order before the built-in guess: text containing the keyword (a word or a package such as SOT-23) goes to that category.");
    let mut remove = None;
    for (i, r) in rules.categories.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut r.keyword).hint_text("keyword or package").desired_width(180.0));
            ui.label("→");
            ui.add(egui::TextEdit::singleline(&mut r.category).hint_text("category").desired_width(180.0));
            if ui.small_button("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        rules.categories.remove(i);
    }
    if ui.button("Add category rule").clicked() {
        rules.categories.push(Default::default());
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Name template");
        ui.add(egui::TextEdit::singleline(&mut rules.name_template).hint_text("{Manufacturer}_{MPN}").desired_width(240.0));
    });
    ui.weak(format!("Placeholders: {}. Empty means just the MPN.", TEMPLATE_FIELDS.iter().map(|f| format!("{{{f}}}")).collect::<Vec<_>>().join(" ")));
    ui.horizontal(|ui| {
        ui.label("MPN case");
        for (case, label) in [(MpnCase::Keep, "Keep"), (MpnCase::Upper, "UPPER"), (MpnCase::Lower, "lower")] {
            ui.selectable_value(&mut rules.mpn_case, case, label);
        }
    });
    ui.weak(format!("Example: {}", rules.name(&[("MPN", "Lm358n"), ("Manufacturer", "TI"), ("Package", "DIP-8")])));
    *rules != before
}
