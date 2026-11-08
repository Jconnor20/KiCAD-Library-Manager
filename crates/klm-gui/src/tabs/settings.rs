//! The Settings popup: a search box over a tree of pages on the left, the chosen page on the right.
//! It edits a copy of `Settings` that Apply or OK saves and Cancel or ✕ throws away. API keys are the one
//! exception: "Save keys" writes to the credential store at once.

mod pages;

use super::ui_kit::{Level, Section};
use super::{import, search};
use crate::{AppCtx, Msg as AppMsg};
use eframe::egui;
use klm_core::credentials::{self, SOURCES};
use klm_core::kicad_env;
use klm_core::settings::Settings;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub enum Msg {
    /// A Test button finished: what it tested (a source name or "kicad-cli") and `Ok(what was found)` or `Err(why not)`.
    Tested(&'static str, Result<String, String>),
}

/// Something a page asks the popup to do that needs the app.
enum Cmd {
    /// Stored credentials changed: rebuild the sources from them.
    Rebuild,
    Test(&'static str),
    MoveLibrary(PathBuf),
    Log(Level, String),
}

enum Footer {
    None,
    Ok,
    Apply,
    Cancel,
}

const KICAD_CLI: &str = "kicad-cli";

const TREE: &[(&str, &[(Section, &str)])] = &[
    ("General", &[(Section::Libraries, "Libraries"), (Section::WatchFolders, "Watch folders"), (Section::KiCad, "KiCad")]),
    ("Sources", &[(Section::Lcsc, "LCSC"), (Section::DigiKey, "DigiKey"), (Section::Mouser, "Mouser"), (Section::Nexar, "Nexar"), (Section::SnapMagic, "SnapMagic")]),
    ("Search", &[(Section::Search, "Search")]),
    ("Import", &[(Section::Import, "Import")]),
    ("Standards", &[(Section::Symbols, "Symbols"), (Section::Footprints, "Footprints"), (Section::PartInfo, "Part information")]),
    ("BOM", &[(Section::Bom, "BOM")]),
    ("History", &[(Section::History, "History")]),
];

/// The page a section opens: a group opens on its first page.
fn page_of(s: Section) -> Section {
    match s {
        Section::General => Section::Libraries,
        Section::Sources => Section::Lcsc,
        Section::Standards => Section::Symbols,
        s => s,
    }
}

/// The Settings popup. Credential entry boxes only ever hold what the user is typing; stored secrets are never read back into the UI.
#[derive(Default)]
pub struct SettingsTab {
    /// The copy being edited; `None` while the popup is closed.
    draft: Option<Settings>,
    section: Section,
    /// The class shown on the Part information page (an index into `part_info::CLASSES`).
    info_class: usize,
    query: String,
    /// Tests running, and the last result of each, shown on the page.
    testing: HashSet<&'static str>,
    results: HashMap<&'static str, Result<String, String>>,
    inputs: HashMap<(&'static str, &'static str), String>,
    stored: HashSet<(&'static str, &'static str)>,
    /// Key boxes whose typed text is shown instead of dots.
    shown: HashSet<(&'static str, &'static str)>,
}

impl SettingsTab {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, section: Section, live: &Settings) {
        self.draft = Some(live.clone());
        self.section = page_of(section);
        self.query.clear();
        self.inputs.clear();
        self.shown.clear();
        self.refresh();
    }

    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        let Msg::Tested(what, result) = msg;
        self.testing.remove(what);
        let (level, line) = match &result {
            Ok(found) => (Level::Ok, format!("{what}: test OK, {found}")),
            Err(e) => (Level::Error, format!("{what}: test failed: {e}")),
        };
        cx.log(level, line);
        self.results.insert(what, result);
    }

    /// The popup itself; shown whatever tab is selected.
    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let Some(mut draft) = self.draft.take() else { return };
        let ctx = cx.ctx.clone();
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("settings_backdrop")).order(egui::Order::Middle).fixed_pos(screen.min).show(&ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
            ui.painter().rect_filled(rect, 0.0, egui::Color32::from_black_alpha(110));
        });
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut open = true;
        let (mut footer, mut cmd) = (Footer::None, None);
        egui::Window::new("Settings")
            .order(egui::Order::Foreground)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .default_size(screen.size() * 0.8)
            .min_size(egui::vec2(520.0, 320.0))
            .max_size(screen.size())
            .show(&ctx, |ui| (footer, cmd) = self.contents(ui, &mut draft, &cx.settings));
        match footer {
            Footer::Ok => {
                self.apply(draft, cx);
            }
            Footer::Apply => {
                self.apply(draft, cx);
                self.draft = Some(cx.settings.clone());
            }
            Footer::Cancel => self.discard(cx),
            Footer::None if !open || escape => self.discard(cx),
            Footer::None => self.draft = Some(draft),
        }
        match cmd {
            None => {}
            Some(Cmd::Rebuild) => {
                cx.sources = Arc::new(klm_core::sources::all(&cx.settings));
                cx.send(AppMsg::Search(search::Msg::ClearResults));
            }
            Some(Cmd::Test(what)) => self.test(what, cx),
            Some(Cmd::MoveLibrary(dir)) => cx.send(AppMsg::MoveGlobal(dir)),
            Some(Cmd::Log(level, text)) => cx.log(level, text),
        }
    }

    fn discard(&mut self, cx: &AppCtx) {
        // A kicad-cli test may have tried the draft's path.
        kicad_env::set_kicad_cli_override(cx.settings.kicad_cli.clone());
        self.draft = None;
    }

    /// Makes `draft` the live settings and saves it. The library's location and the open project are changed
    /// elsewhere while the popup is open, so those two keep their live values.
    fn apply(&mut self, mut draft: Settings, cx: &mut AppCtx) {
        draft.global_root = cx.settings.global_root.clone();
        draft.last_project = cx.settings.last_project.clone();
        draft.display_currency = draft.display_currency.trim().to_uppercase();
        let old = std::mem::replace(&mut cx.settings, draft);
        cx.save_settings();
        let s = &cx.settings;
        kicad_env::set_kicad_cli_override(s.kicad_cli.clone());
        klm_core::snapshot::set_policy(&s.history);
        klm_core::symbols::configure(s);
        if s.source_enabled != old.source_enabled {
            cx.sources = Arc::new(klm_core::sources::all(&cx.settings));
            cx.send(AppMsg::Search(search::Msg::ClearResults));
        }
        if cx.settings.watch_dirs != old.watch_dirs {
            cx.send(AppMsg::Import(import::Msg::RestartWatcher));
        }
    }

    fn test(&mut self, what: &'static str, cx: &mut AppCtx) {
        self.testing.insert(what);
        self.results.remove(what);
        if what == KICAD_CLI {
            kicad_env::set_kicad_cli_override(self.draft.as_ref().map_or(cx.settings.kicad_cli.clone(), |d| d.kicad_cli.clone()));
        }
        let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let result = if what == KICAD_CLI {
                kicad_env::kicad_cli_version().map(|v| format!("version {v}"))
            } else {
                klm_core::sources::build(what).ok_or_else(|| anyhow::anyhow!("keys missing")).and_then(|s| s.search("LM358")).map(|p| format!("{} results", p.len()))
            };
            let _ = tx.send(AppMsg::Settings(Msg::Tested(what, result.map_err(|e| format!("{e:#}")))));
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

    fn contents(&mut self, ui: &mut egui::Ui, d: &mut Settings, live: &Settings) -> (Footer, Option<Cmd>) {
        let mut footer = Footer::None;
        egui::Panel::bottom("settings_footer").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(pages::has_defaults(self.section), egui::Button::new("Restore defaults")).on_hover_text("Puts this page's options back to their defaults; Apply or OK saves them").clicked() {
                    pages::restore(d, self.section);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        footer = Footer::Ok;
                    }
                    if ui.button("Cancel").clicked() {
                        footer = Footer::Cancel;
                    }
                    if ui.button("Apply").clicked() {
                        footer = Footer::Apply;
                    }
                });
            });
            ui.add_space(2.0);
        });
        egui::Panel::left("settings_tree").resizable(true).default_size(170.0).size_range(120.0..=300.0).show(ui, |ui| self.tree(ui));
        let mut cmd = None;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(pages::title(self.section));
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| cmd = self.page(ui, d, live));
        });
        (footer, cmd)
    }

    fn tree(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search settings").desired_width(f32::INFINITY));
        ui.add_space(4.0);
        let q = self.query.trim().to_lowercase();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // Compact, so the whole tree fits at the default window size.
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.spacing_mut().button_padding.y = 0.0;
            let mut any = false;
            for (group, pages) in TREE {
                let group_hit = group.to_lowercase().contains(&q);
                let shown: Vec<_> = pages.iter().filter(|(_, label)| group_hit || label.to_lowercase().contains(&q)).collect();
                if shown.is_empty() {
                    continue;
                }
                any = true;
                let nested = pages.len() > 1;
                if nested {
                    ui.add_space(4.0);
                    ui.strong(*group);
                }
                for (section, label) in shown {
                    ui.horizontal(|ui| {
                        if nested {
                            ui.add_space(10.0);
                        }
                        if ui.selectable_label(self.section == *section, *label).clicked() {
                            self.section = *section;
                        }
                    });
                }
            }
            if !any {
                ui.weak("No page matches");
            }
        });
    }
}
