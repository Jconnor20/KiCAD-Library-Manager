//! The "BOM to library" tab: pick a BOM (CSV or spreadsheet), check the detected sheet, header row and columns, see how each line resolves
//! (standard library, a source, not found, ambiguous), then generate a library from it.

use super::ui_kit::{self, Level};
use crate::{AppCtx, Msg as AppMsg};
use eframe::egui;
use klm_core::batch::Target;
use klm_core::bom_import::{self as bi, Generated, Item, Mapping, Resolved};
use klm_core::kicad_env;
use std::path::PathBuf;

pub enum Msg {
    Resolved(Vec<Resolved>),
    Generated(Result<Generated, String>),
}

#[derive(Default)]
pub struct BomImportTab {
    path: Option<PathBuf>,
    sheets: Vec<bi::Sheet>,
    /// The sheet and header row (0-based) `rows` was cut from.
    sheet: usize,
    header: usize,
    /// The table: the rows from the header row down, header first.
    rows: Vec<Vec<String>>,
    map: Mapping,
    items: Vec<Item>,
    /// Row of each item in `rows`.
    at: Vec<usize>,
    resolved: Vec<Resolved>,
    generated: Option<Generated>,
    category: String,
    project: bool,
    overwrite: bool,
    running: bool,
}

impl BomImportTab {
    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        self.running = false;
        cx.busy = false;
        match msg {
            Msg::Resolved(r) => self.resolved = r,
            Msg::Generated(Ok(g)) => {
                let failed = g.lines.iter().filter(|d| d.error.is_some()).count();
                cx.log(if failed > 0 { Level::Warn } else { Level::Ok }, format!(
                    "BOM to library: {} part(s) in the library, {failed} failed; Standardise changed {} file(s); {} compliant, {} not",
                    g.lines.iter().filter(|d| d.library_part.is_some()).count(),
                    g.standardised,
                    g.compliant,
                    g.non_compliant
                ));
                self.generated = Some(g);
            }
            Msg::Generated(Err(e)) => cx.log(Level::Error, format!("BOM to library: {e}")),
        }
    }

    fn open(&mut self, path: PathBuf, cx: &mut AppCtx) {
        match bi::read_sheets(&path) {
            Ok(sheets) => {
                let (sheet, header) = bi::auto(&sheets);
                self.sheets = sheets;
                self.path = Some(path);
                self.rows.clear();
                self.select(sheet, header, cx);
            }
            Err(e) => cx.log(Level::Error, format!("BOM to library: {e:#}")),
        }
    }

    /// Cuts the table from `sheet` at row `header` and re-detects the columns; leaves everything as it was if that row has no data below it.
    fn select(&mut self, sheet: usize, header: usize, cx: &mut AppCtx) {
        match self.sheets[sheet].table(header) {
            Ok(t) => {
                self.rows = t.to_vec();
                (self.sheet, self.header) = (sheet, header);
                self.map = bi::detect(&self.rows[0]);
                self.remap();
            }
            Err(e) => cx.log(Level::Error, format!("BOM to library: {e:#}")),
        }
    }

    fn remap(&mut self) {
        (self.items, self.at) = bi::items_of(&self.rows, &self.map);
        self.resolved.clear();
        self.generated = None;
    }

    fn resolve(&mut self, cx: &mut AppCtx) {
        self.running = true;
        cx.busy = true;
        let (lines, sources, tx, ctx) = (self.items.iter().map(|i| i.line.clone()).collect::<Vec<_>>(), cx.sources.clone(), cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let manifests = bi::load_manifests();
            let resolved = lines.iter().map(|l| bi::resolve(l, &manifests, &sources)).collect();
            let _ = tx.send(AppMsg::BomImport(Msg::Resolved(resolved)));
            ctx.request_repaint();
        });
    }

    fn generate(&mut self, cx: &mut AppCtx) {
        self.running = true;
        cx.busy = true;
        let (resolved, sources, settings, tx, ctx) = (self.resolved.clone(), cx.sources.clone(), cx.settings.clone(), cx.tx.clone(), cx.ctx.clone());
        let (global, project) = (cx.global_root(), cx.project_root());
        let opts = bi::Options {
            target: if self.project { Target::Project } else { Target::Global },
            category: Some(self.category.trim().to_string()).filter(|c| !c.is_empty()),
            overwrite: self.overwrite,
        };
        std::thread::spawn(move || {
            let work = std::env::temp_dir().join(format!("klm-bom-import-{}", std::process::id()));
            let res = bi::generate(&resolved, &sources, global.as_ref(), project.as_ref(), &opts, &settings, &work).map_err(|e| format!("{e:#}"));
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(AppMsg::BomImport(Msg::Generated(res)));
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!self.running, egui::Button::new("Open BOM…")).on_hover_text("A CSV or Excel/OpenDocument sheet with one row per line (KiCad, JLCPCB, DigiKey and similar exports). The sheet, header row and columns are detected; change them below if they are wrong.").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("BOM (CSV or spreadsheet)", &["csv", "xlsx", "xlsm", "xlsb", "xls", "ods"]).pick_file() {
                    self.open(p, cx);
                }
            }
            if let Some(p) = &self.path {
                ui.label(p.display().to_string());
            }
            if self.running {
                ui.spinner();
            }
        });
        if self.rows.is_empty() {
            ui.weak("Open a BOM to build a library from it. Each line is looked up in the standard library first, then at the enabled sources.");
            return;
        }
        let (mut sheet, mut header, mut moved) = (self.sheet, self.header + 1, false);
        ui.add_enabled_ui(!self.running, |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.sheets.len() > 1 {
                    egui::ComboBox::from_label("Sheet").selected_text(&self.sheets[sheet].name).show_ui(ui, |ui| {
                        for (i, s) in self.sheets.iter().enumerate() {
                            if ui.selectable_value(&mut sheet, i, &s.name).changed() {
                                // A new sheet gets its own detected header row.
                                header = bi::header_of(&s.rows) + 1;
                                moved = true;
                            }
                        }
                    });
                }
                ui.label("Header row");
                moved |= ui.add(egui::DragValue::new(&mut header).range(1..=self.sheets[sheet].rows.len().max(1))).on_hover_text("The row (as numbered in the spreadsheet) holding the column titles. Rows above it are ignored.").changed();
                ui.weak(self.rows[0].iter().filter(|c| !c.is_empty()).take(8).cloned().collect::<Vec<_>>().join(" | "));
            });
        });
        if moved {
            self.select(sheet, header.saturating_sub(1), cx);
        }
        let mut changed = false;
        ui.collapsing("Columns", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (n, (_, label, _)) in bi::FIELDS.iter().enumerate() {
                    let shown = self.map[n].map_or("(none)".to_string(), |i| self.rows[0][i].clone());
                    egui::ComboBox::from_label(*label).selected_text(shown).show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut self.map[n], None, "(none)").changed();
                        for (i, h) in self.rows[0].iter().enumerate() {
                            changed |= ui.selectable_value(&mut self.map[n], Some(i), h).changed();
                        }
                    });
                }
            });
        });
        if changed {
            self.remap();
        }
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!self.running && !self.items.is_empty(), egui::Button::new("Look up lines")).on_hover_text("Standard library first, then each enabled source by supplier number, then MPN").clicked() {
                self.resolve(cx);
            }
            ui.label("Library name");
            ui.add(egui::TextEdit::singleline(&mut self.category).hint_text("per-part category").desired_width(130.0))
                .on_hover_text("One category (so one KiCad library) for every part; empty guesses each part's category");
            ui.selectable_value(&mut self.project, false, "Global");
            ui.add_enabled_ui(cx.project.is_some(), |ui| ui.selectable_value(&mut self.project, true, "Project"));
            ui.checkbox(&mut self.overwrite, "Overwrite");
            let ready = !self.resolved.is_empty() && !self.running;
            let closed = !kicad_env::kicad_running();
            let has_root = if self.project { cx.project_root().is_some() } else { cx.global_root().is_some() };
            if ui.add_enabled(ready && closed && has_root, egui::Button::new("Generate")).on_disabled_hover_text("Look up the lines first, have a library to write into, and close KiCad").clicked() {
                self.generate(cx);
            }
            if ui.add_enabled(self.generated.is_some(), egui::Button::new("Save BOM with library parts…")).clicked() {
                if let (Some(g), Some(path)) = (&self.generated, rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("bom-library.csv").save_file()) {
                    let csv = bi::write_back(&self.rows, &self.at, &self.resolved, &g.lines);
                    cx.log.push(match std::fs::write(&path, csv) {
                        Ok(()) => format!("Wrote {}", path.display()),
                        Err(e) => format!("Could not write {}: {e}", path.display()),
                    });
                }
            }
        });
        if let Some(g) = &self.generated {
            ui.label(format!("Standardise changed {} file(s); {} added part(s) compliant, {} not (see the Library tab).", g.standardised, g.compliant, g.non_compliant));
        }
        if !self.resolved.is_empty() {
            let count = |l: &str| self.resolved.iter().filter(|r| r.label() == l).count();
            ui.label(format!("{} standard library, {} from sources, {} not found, {} ambiguous. Lines not found or ambiguous are left out.", count("stdlib"), count("source"), count("not found"), count("ambiguous")));
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("bom_import").striped(true).show(ui, |ui| {
                for h in ["Refs", "Qty", "MPN / value", "Status", "Result"] {
                    ui.strong(h);
                }
                ui.end_row();
                for (n, it) in self.items.iter().enumerate() {
                    ui.label(it.line.refs.join(" "));
                    ui.label(it.qty.to_string());
                    ui.label(if it.line.mpn.is_empty() { &it.line.value } else { &it.line.mpn });
                    match self.resolved.get(n) {
                        Some(r) => {
                            let level = match r {
                                Resolved::Stdlib { .. } | Resolved::Source(_) => Level::Ok,
                                Resolved::NotFound(_) => Level::Error,
                                Resolved::Ambiguous(_) => Level::Warn,
                            };
                            ui_kit::badge(ui, r.label(), level);
                            let done = self.generated.as_ref().and_then(|g| g.lines.get(n));
                            ui.label(match done {
                                Some(d) => d.error.clone().or(d.library_part.clone()).unwrap_or_else(|| r.detail()),
                                None => r.detail(),
                            });
                        }
                        None => {
                            ui.weak("");
                            ui.weak("");
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }
}
