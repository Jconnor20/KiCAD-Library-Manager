//! The Settings page: a search box over a tree of pages on the left, the chosen page on the right.
//! It edits a copy of `Settings` that Save writes and Discard (or leaving the page) throws away. API keys are the one
//! exception: "Save keys" writes to the credential store at once, and the theme choice is applied at once too.

mod pages;

use super::page_kit;
use super::ui_kit::{Level, Section};
use super::{import, search};
use crate::theme;
use crate::{AppCtx, Msg as AppMsg, Tab};
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

/// Something a page asks the page to do that needs the app.
enum Cmd {
    /// Stored credentials changed: rebuild the sources from them.
    Rebuild,
    Test(&'static str),
    MoveLibrary(PathBuf),
    Log(Level, String),
}

enum Footer {
    None,
    Save,
    Discard,
}

const KICAD_CLI: &str = "kicad-cli";

const TREE: &[(&str, &[(Section, &str)])] = &[
    ("General", &[(Section::Libraries, "Libraries"), (Section::WatchFolders, "Watch folders"), (Section::KiCad, "KiCad")]),
    ("Sources", &[(Section::Lcsc, "LCSC"), (Section::DigiKey, "DigiKey"), (Section::Mouser, "Mouser"), (Section::Nexar, "Nexar"), (Section::SnapMagic, "SnapMagic")]),
    ("Workflow", &[(Section::Search, "Search"), (Section::Import, "Import"), (Section::Bom, "BOM"), (Section::History, "History")]),
    ("Standards", &[(Section::Symbols, "Symbols"), (Section::Footprints, "Footprints"), (Section::PartInfo, "Part information")]),
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

/// The Settings page. Credential entry boxes only ever hold what the user is typing; stored secrets are never read back into the UI.
#[derive(Default)]
pub struct SettingsTab {
    /// The copy being edited; `None` while the page isn't showing.
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

    /// Starts editing a copy of `live` at `section`. Asking for the General page while already editing (the rail's
    /// Settings item) keeps what is on screen.
    pub fn open(&mut self, section: Section, live: &Settings) {
        if self.draft.is_some() && section == Section::General {
            return;
        }
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

    /// Called every frame whatever tab is showing: leaving the page drops the edits Save didn't take.
    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        if cx.tab != Tab::Settings && self.draft.is_some() {
            self.discard(cx);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        if self.draft.is_none() {
            self.open(Section::General, &cx.settings);
        }
        let mut draft = self.draft.take().expect("opened above");
        let (footer, cmd) = self.contents(ui, &mut draft, &cx.settings);
        match footer {
            Footer::Save => {
                self.apply(draft, cx);
                self.draft = Some(cx.settings.clone());
            }
            Footer::Discard => {
                // A kicad-cli test may have tried the draft's path.
                kicad_env::set_kicad_cli_override(cx.settings.kicad_cli.clone());
                self.draft = Some(cx.settings.clone());
                self.inputs.clear();
            }
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
    /// elsewhere while the page is open, so those two keep their live values.
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
        let pal = theme::palette(ui.visuals());
        let mut footer = Footer::None;
        let footer_frame = egui::Frame::new().fill(pal.panel).inner_margin(egui::Margin::symmetric(16, 8));
        let bar = egui::Panel::bottom("settings_footer").frame(footer_frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                // The library's place and the open project change elsewhere while this is open, so they don't count.
                let mut compare = live.clone();
                (compare.global_root, compare.last_project) = (d.global_root.clone(), d.last_project.clone());
                if format!("{compare:?}") != format!("{d:?}") {
                    ui.label(egui::RichText::new("unsaved changes").monospace().size(11.0).color(Level::Warn.color(ui.visuals())));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if page_kit::primary(ui, true, "Save").on_hover_text("Saves every page, not just this one").clicked() {
                        footer = Footer::Save;
                    }
                    if ui.button("Discard").on_hover_text("Puts every page back to what is saved").clicked() {
                        footer = Footer::Discard;
                    }
                    if ui.add_enabled(pages::has_defaults(self.section), egui::Button::new("Restore defaults").frame(false)).on_hover_text("Puts this page's options back to their defaults; Save keeps them").clicked() {
                        pages::restore(d, self.section);
                    }
                });
            });
        });
        let line = egui::Stroke::new(1.0, pal.line);
        ui.painter().line_segment([bar.response.rect.left_top(), bar.response.rect.right_top()], line);
        let tree_frame = egui::Frame::new().inner_margin(egui::Margin { right: 12, ..Default::default() });
        let tree = egui::Panel::left("settings_tree").resizable(true).default_size(210.0).size_range(150.0..=300.0).frame(tree_frame).show(ui, |ui| self.tree(ui));
        ui.painter().line_segment([tree.response.rect.right_top(), tree.response.rect.right_bottom()], line);
        let mut cmd = None;
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin { left: 4, ..Default::default() })).show(ui, |ui| {
            self.breadcrumb(ui);
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| cmd = self.page(ui, d, live));
        });
        (footer, cmd)
    }

    /// "Sources › DigiKey" with the group muted, and whether the page's keys are stored.
    fn breadcrumb(&self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.visuals());
        let title = pages::title(self.section);
        let (group, page) = title.split_once(" › ").map_or(("", title), |(g, p)| (g, p));
        let mut job = egui::text::LayoutJob::default();
        let font = |size| egui::FontId::proportional(size);
        if !group.is_empty() {
            job.append(&format!("{group} › "), 0.0, egui::TextFormat { font_id: font(14.0), color: pal.muted, ..Default::default() });
        }
        job.append(page, 0.0, egui::TextFormat { font_id: font(17.0), color: pal.fg, ..Default::default() });
        ui.horizontal(|ui| {
            ui.label(job);
            if let Some(info) = pages::source_info(self.section).filter(|i| !i.fields.is_empty()) {
                let complete = info.fields.iter().all(|f| self.stored.contains(&(info.name, f.0)));
                super::ui_kit::badge(ui, if complete { "keys stored" } else { "no keys" }, if complete { Level::Ok } else { Level::Warn });
            }
        });
        ui.separator();
    }

    fn tree(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search settings").desired_width(f32::INFINITY));
        ui.add_space(4.0);
        let q = self.query.trim().to_lowercase();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let mut any = false;
            for (group, pages) in TREE {
                let group_hit = group.to_lowercase().contains(&q);
                let shown: Vec<_> = pages.iter().filter(|(_, label)| group_hit || label.to_lowercase().contains(&q)).collect();
                if shown.is_empty() {
                    continue;
                }
                any = true;
                ui.add_space(8.0);
                page_kit::caption(ui, group);
                ui.add_space(2.0);
                for (section, label) in shown {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
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
