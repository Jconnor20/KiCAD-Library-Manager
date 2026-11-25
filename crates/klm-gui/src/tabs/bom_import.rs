//! The "BOM to library" tab: pick a BOM (CSV or spreadsheet), check the detected sheet, header row and columns, see how each line resolves
//! (standard library, a source, not found, ambiguous), then generate a library from it.

use super::ui_kit::{self, Col, Level, Sort};
use crate::{AppCtx, Msg as AppMsg, shell, theme};
use eframe::egui;
use klm_core::batch::Target;
use klm_core::bom_import::{self as bi, Generated, Item, Mapping, Resolved};
use klm_core::kicad_env;
use klm_core::part::Part;
use std::path::PathBuf;
use std::sync::Arc;

pub enum Msg {
    /// What the background work is doing: the step, how many are done, how many there are (0 when it can't count).
    Progress(String, usize, usize),
    /// The looked-up lines (by item index) and how each resolved; more follows (`Avail` or `Generated`).
    Resolved(Vec<(usize, Resolved)>),
    /// What CAD each source-resolved line comes with, and the part as downloaded; ends a lookup.
    Avail(Vec<(usize, Option<(bi::Avail, Option<Part>)>)>),
    Generated(Result<Generated, String>),
}

/// The folder lookups download into and Generate reuses.
fn work_dir() -> PathBuf {
    std::env::temp_dir().join(format!("klm-bom-{}", std::process::id()))
}

/// A progress sink that forwards to the tab and wakes the UI.
fn reporter(tx: &std::sync::mpsc::Sender<AppMsg>, ctx: &egui::Context) -> impl Fn(&str, usize, usize) + Sync {
    let (tx, ctx) = (std::sync::Mutex::new(tx.clone()), ctx.clone());
    move |step, done, total| {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(AppMsg::BomImport(Msg::Progress(step.into(), done, total)));
        ctx.request_repaint();
    }
}

impl Drop for BomImportTab {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(work_dir());
    }
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
    at: Vec<Vec<usize>>,
    /// Per item: how it resolved, or None until it is looked up (only selected lines are).
    resolved: Vec<Option<Resolved>>,
    /// Per item: whether it goes into the library.
    selected: Vec<bool>,
    avail: Vec<Option<bi::Avail>>,
    /// Per item: the part as the lookup downloaded it, so Generate doesn't download it again.
    fetched: Vec<Option<Part>>,
    /// Pauses and stops the running job; a new one for each job.
    ctl: Arc<bi::Control>,
    /// The running job is a Generate (else a lookup), which a stop can also undo.
    generating: bool,
    /// The step the background work is on, with how many are done of how many.
    progress: Option<(String, usize, usize)>,
    /// Your global and project libraries' parts, read when the BOM opens and after Generate.
    lib: Vec<klm_core::alternatives::LibPart>,
    /// Per item: the library symbol it already matches (by MPN or supplier number), checked without a lookup.
    in_lib: Vec<Option<String>>,
    generated: Option<Generated>,
    category: String,
    project: bool,
    overwrite: bool,
    /// Which part number the lines are looked up by.
    by: bi::By,
    /// Per item, per alternative it names: whether to import it too and record it as an alternative of the line's part.
    alt_on: Vec<Vec<bool>>,
    running: bool,
}

impl BomImportTab {
    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        // Progress and the first half of a job leave the tab busy; Avail and Generated end it.
        if matches!(msg, Msg::Avail(_) | Msg::Generated(_)) {
            (self.running, self.progress) = (false, None);
            cx.busy = false;
        }
        match msg {
            Msg::Progress(step, done, total) => self.progress = Some((step, done, total)),
            Msg::Resolved(r) => {
                let len = self.resolved.len();
                for (i, x) in r.into_iter().filter(|(i, _)| *i < len) {
                    (self.resolved[i], self.avail[i], self.fetched[i]) = (Some(x), None, None);
                }
            }
            Msg::Avail(a) => {
                for (i, x) in a {
                    if let (Some(Some(_)), Some(slot)) = (self.resolved.get(i), self.avail.get_mut(i)) {
                        (*slot, self.fetched[i]) = x.map_or((None, None), |(a, p)| (Some(a), p));
                    }
                }
            }
            Msg::Generated(Ok(g)) if g.stopped == bi::Stopped::Undone => {
                cx.log(Level::Warn, "BOM to library: stopped and undone, the library is as it was before (History keeps the snapshot)");
                self.generated = None;
                self.lib = klm_core::bom::lib_parts(&cx.roots());
                self.in_lib = self.items.iter().map(|i| self.in_library(i)).collect();
            }
            Msg::Generated(Ok(g)) => {
                let failed = g.lines.iter().filter(|d| d.error.is_some()).count();
                if g.stopped == bi::Stopped::Kept {
                    cx.log(Level::Warn, "BOM to library: stopped; the parts already imported are kept, the other lines were not imported. Generate again to continue (parts already in the library are skipped)");
                }
                cx.log(
                    if failed > 0 { Level::Warn } else { Level::Ok },
                    format!(
                        "BOM to library: {} part(s) in the library, {failed} failed; Standardise changed {} file(s); {} compliant, {} not",
                        g.lines.iter().filter(|d| d.library_part.is_some()).count(),
                        g.standardised,
                        g.compliant,
                        g.non_compliant
                    ),
                );
                self.generated = Some(g);
                self.lib = klm_core::bom::lib_parts(&cx.roots());
                self.in_lib = self.items.iter().map(|i| self.in_library(i)).collect();
                // What went in is done: a second Generate carries on with the rest.
                for (on, l) in self.selected.iter_mut().zip(&self.in_lib) {
                    *on &= l.is_none();
                }
            }
            Msg::Generated(Err(e)) => cx.log(Level::Error, format!("BOM to library: {e}")),
        }
    }

    pub(crate) fn open(&mut self, path: PathBuf, cx: &mut AppCtx) {
        match bi::read_sheets(&path) {
            Ok(sheets) => {
                let (sheet, header) = bi::auto(&sheets);
                self.sheets = sheets;
                self.path = Some(path);
                self.lib = klm_core::bom::lib_parts(&cx.roots());
                self.rows.clear();
                let _ = std::fs::remove_dir_all(work_dir());
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
                // A BOM with LCSC numbers is looked up by them; the user can switch back.
                self.by = if self.map[bi::Field::Lcsc as usize].is_some() { bi::By::Lcsc } else { bi::By::Auto };
                self.remap();
            }
            Err(e) => cx.log(Level::Error, format!("BOM to library: {e:#}")),
        }
    }

    fn remap(&mut self) {
        (self.items, self.at) = bi::items_of(&self.rows, &self.map);
        self.resolved = vec![None; self.items.len()];
        self.in_lib = self.items.iter().map(|i| self.in_library(i)).collect();
        // Flagged lines (non-parts, bogus MPNs) and parts already in the library start deselected.
        self.selected = self.items.iter().zip(&self.in_lib).map(|(i, l)| bi::flag(&i.line).is_none() && l.is_none()).collect();
        self.avail = vec![None; self.items.len()];
        self.fetched = vec![None; self.items.len()];
        self.alt_on = self.items.iter().map(|i| vec![false; i.alts.len()]).collect();
        self.generated = None;
    }

    fn in_library(&self, item: &Item) -> Option<String> {
        match klm_core::bom::match_line(&item.line, &self.lib) {
            klm_core::bom::Status::Found { symbol, .. } => Some(symbol),
            _ => None,
        }
    }

    /// The selected lines that still need a lookup: not looked up yet, or not found (a retry after a failure or a corrected MPN).
    fn todo(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|&n| self.selected[n] && matches!(self.resolved[n], None | Some(Resolved::NotFound(_)))).collect()
    }

    /// Every line's result, "not looked up" for the lines without one.
    fn resolved_all(&self) -> Vec<Resolved> {
        self.resolved.iter().map(|r| r.clone().unwrap_or_else(|| Resolved::NotFound(vec!["not looked up".into()]))).collect()
    }

    fn resolve(&mut self, cx: &mut AppCtx) {
        self.running = true;
        self.generating = false;
        self.ctl = Arc::new(bi::Control::default());
        cx.busy = true;
        let ctl = self.ctl.clone();
        let (lines, by, sources, tx, ctx) =
            (self.todo().into_iter().map(|n| (n, self.items[n].line.clone())).collect::<Vec<_>>(), self.by, cx.sources.clone(), cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            klm_core::download::set_abort(false);
            let report = reporter(&tx, &ctx);
            let manifests = bi::load_manifests();
            let resolved = bi::resolve_all(&lines, by, &manifests, &sources, &report, &ctl);
            let _ = tx.send(AppMsg::BomImport(Msg::Resolved(resolved.clone())));
            ctx.request_repaint();
            let work = work_dir();
            let avail = bi::par_report("Checking CAD", &resolved, &report, &ctl, |(n, r)| (*n, bi::fetch_cad(r, &sources, &work.join(n.to_string()))));
            let _ = tx.send(AppMsg::BomImport(Msg::Avail(avail)));
            ctx.request_repaint();
        });
    }

    /// Generate: looks up the ticked lines that were not looked up yet, then builds the library, reusing what an earlier lookup downloaded.
    fn generate(&mut self, cx: &mut AppCtx) {
        self.running = true;
        self.generating = true;
        self.ctl = Arc::new(bi::Control::default());
        cx.busy = true;
        let ctl = self.ctl.clone();
        let unlooked: Vec<(usize, klm_core::bom::Line)> =
            (0..self.items.len()).filter(|&n| self.selected[n] && self.resolved[n].is_none()).map(|n| (n, self.items[n].line.clone())).collect();
        let (by, mut all, sel) = (self.by, self.resolved_all(), self.selected.clone());
        let (sources, settings, tx, ctx) = (cx.sources.clone(), cx.settings.clone(), cx.tx.clone(), cx.ctx.clone());
        let (global, project) = (cx.global_root(), cx.project_root());
        // Only the ticked alternatives of ticked lines are imported.
        let alts: Vec<Vec<bi::Alt>> = self
            .items
            .iter()
            .zip(&self.selected)
            .zip(&self.alt_on)
            .map(|((i, &on), t)| if on { i.alts.iter().zip(t).filter(|(_, k)| **k).map(|(a, _)| a.clone()).collect() } else { vec![] })
            .collect();
        let opts = bi::Options {
            target: if self.project { Target::Project } else { Target::Global },
            category: Some(self.category.trim().to_string()).filter(|c| !c.is_empty()),
            overwrite: self.overwrite,
            alternatives: alts.iter().any(|a| !a.is_empty()),
        };
        let prefetched = self.fetched.clone();
        std::thread::spawn(move || {
            let report = reporter(&tx, &ctx);
            if !unlooked.is_empty() {
                let manifests = bi::load_manifests();
                let found = bi::resolve_all(&unlooked, by, &manifests, &sources, &report, &ctl);
                for (n, r) in &found {
                    all[*n] = r.clone();
                }
                let _ = tx.send(AppMsg::BomImport(Msg::Resolved(found)));
            }
            // Deselected lines are passed as not found, which generate leaves out.
            let resolved: Vec<Resolved> =
                all.into_iter().zip(&sel).map(|(r, &on)| if on { r } else { Resolved::NotFound(vec!["deselected".into()]) }).collect();
            let res = bi::generate(&resolved, &alts, &prefetched, &sources, global.as_ref(), project.as_ref(), &opts, &settings, &work_dir(), &report, &ctl)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppMsg::BomImport(Msg::Generated(res)));
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let pal = theme::palette(ui.visuals());
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!self.running, egui::Button::new("Open BOM…")).on_hover_text("A CSV or Excel/OpenDocument sheet with one row per line (KiCad, JLCPCB, DigiKey and similar exports). The sheet, header row and columns are detected; change them below if they are wrong.").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("BOM (CSV or spreadsheet)", &["csv", "xlsx", "xlsm", "xlsb", "xls", "ods"]).pick_file() {
                    self.open(p, cx);
                }
            if let Some(p) = &self.path {
                ui.label(p.display().to_string());
            }
            if self.running {
                match &self.progress {
                    Some((step, done, total)) if *total > 0 => drop(ui.add(egui::ProgressBar::new(*done as f32 / *total as f32).desired_width(260.0).text(format!("{step} {done} of {total}")))),
                    Some((step, ..)) => drop(ui.add(egui::ProgressBar::new(0.0).animate(true).desired_width(260.0).text(format!("{step}…")))),
                    None => drop(ui.spinner()),
                }
                let (paused, stopping) = (self.ctl.paused(), self.ctl.stopped());
                if ui.add_enabled(!stopping, egui::Button::new(if paused { "Resume" } else { "Pause" })).on_hover_text("Pause starts no new downloads; the ones in flight finish first").clicked() {
                    self.ctl.pause(!paused);
                }
                if ui.add_enabled(!stopping, egui::Button::new("Stop")).on_hover_text(if self.generating { "Stop here and keep the parts already imported" } else { "Stop looking up; the lines already looked up are kept" }).clicked() {
                    self.ctl.stop(false);
                }
                if self.generating && ui.add_enabled(!stopping, egui::Button::new("Stop and undo")).on_hover_text("Stop and put the library back as it was before this Generate, importing nothing").clicked() {
                    self.ctl.stop(true);
                }
                if stopping {
                    ui.label(egui::RichText::new("Stopping…").color(pal.muted));
                } else if paused {
                    ui.label(egui::RichText::new("Paused").color(pal.muted));
                }
                // Say when the sources are what is holding the import up.
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                if let Some((who, left, refused)) = klm_core::download::held() {
                    let secs = left.as_secs_f32().ceil() as u64;
                    let (text, color) = if refused { (format!("{who} is refusing requests (rate limit): retrying in {secs} s"), ui.visuals().warn_fg_color) } else { (format!("Rate limit: next request to {who} in {secs} s"), pal.muted) };
                    ui.label(egui::RichText::new(text).color(color));
                }
            }
        });
        if self.rows.is_empty() {
            ui.add_space(12.0);
            shell::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("Build a library from a BOM").strong().size(15.0));
                ui.label(egui::RichText::new("Open a BOM CSV. Each line is looked up in the standard library first, then at the enabled sources, and the parts found are added to your library.").color(pal.muted));
            });
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
                moved |= ui
                    .add(egui::DragValue::new(&mut header).range(1..=self.sheets[sheet].rows.len().max(1)))
                    .on_hover_text("The row (as numbered in the spreadsheet) holding the column titles. Rows above it are ignored.")
                    .changed();
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
        let looked = self.resolved.iter().any(Option::is_some);
        ui.horizontal_wrapped(|ui| {
            // Look up is optional: it previews what Generate will use. Generate looks up whatever was not looked up.
            let todo = self.todo().len();
            if ui.add_enabled(!self.running && todo > 0, egui::Button::new(format!("Look up selected ({todo})"))).on_hover_text("Optional preview: look up the ticked lines — standard library first, then each enabled source by supplier number, then MPN. Generate does this itself for lines not looked up yet. Lines already found are skipped; lines not found are tried again.").clicked() {
                self.resolve(cx);
            }
            egui::ComboBox::from_label("Look up by").selected_text(match self.by { bi::By::Auto => "Any number", bi::By::Lcsc => "LCSC #", bi::By::Mpn => "MPN" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.by, bi::By::Auto, "Any number (supplier numbers, then MPN)");
                ui.selectable_value(&mut self.by, bi::By::Lcsc, "LCSC # (MPN only when a line has none)");
                ui.selectable_value(&mut self.by, bi::By::Mpn, "MPN only");
            });
            let total = self.alt_on.iter().flatten().count();
            if total > 0 {
                let mut all = self.alt_on.iter().flatten().all(|&k| k);
                if ui.checkbox(&mut all, format!("All alternatives ({total})")).on_hover_text("Parts named in the Alternative columns (with an LCSC number) are imported too and recorded as alternatives of the line's part. Tick them here all at once, or one by one in the table.").changed() {
                    self.alt_on.iter_mut().flatten().for_each(|k| *k = all);
                }
            }
            ui.label("Library name");
            ui.add(egui::TextEdit::singleline(&mut self.category).hint_text("per-part category").desired_width(130.0))
                .on_hover_text("One category (so one KiCad library) for every part; empty guesses each part's category");
            ui.selectable_value(&mut self.project, false, "Global");
            ui.add_enabled_ui(cx.project.is_some(), |ui| ui.selectable_value(&mut self.project, true, "Project"));
            ui.checkbox(&mut self.overwrite, "Overwrite");
            // Ticked lines that are looked up and found, or not looked up yet (Generate looks those up).
            let found = self.resolved.iter().zip(&self.selected).any(|(r, &on)| on && matches!(r, None | Some(Resolved::Stdlib { .. } | Resolved::Source(_))));
            let ready = found && !self.running;
            let closed = !kicad_env::kicad_running();
            let has_root = if self.project { cx.project_root().is_some() } else { cx.global_root().is_some() };
            if ui.add_enabled(ready && closed && has_root, shell::primary(ui, "Generate")).on_hover_text("Look up the ticked lines that are not looked up yet, then build the library from the ones found").on_disabled_hover_text("Tick a line, have a library to write into, and close KiCad").clicked() {
                self.generate(cx);
            }
            let why = if self.running { Some("working…") } else if !found { Some(if self.selected.iter().any(|&on| on) { "none of the ticked lines was found" } else { "tick the lines to import" }) } else if !closed { Some("close KiCad first") } else if !has_root { Some(if self.project { "no project folder open" } else { "no global library set (Settings)" }) } else { None };
            if let Some(w) = why {
                ui.weak(format!("Generate: {w}"));
            }
            if ui.add_enabled(self.generated.is_some(), egui::Button::new("Save BOM with library parts…")).clicked()
                && let (Some(g), Some(path)) = (&self.generated, rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("bom-library.csv").save_file()) {
                    let csv = bi::write_back(&self.rows, &self.at, &self.resolved_all(), &g.lines);
                    cx.log.push(match std::fs::write(&path, csv) {
                        Ok(()) => format!("Wrote {}", path.display()),
                        Err(e) => format!("Could not write {}: {e}", path.display()),
                    });
                }
        });
        if let Some(g) = &self.generated {
            ui.label(format!(
                "Standardise changed {} file(s); {} added part(s) compliant, {} not (see the Library tab).",
                g.standardised, g.compliant, g.non_compliant
            ));
        }
        if looked {
            let count = |l: &str| self.resolved.iter().flatten().filter(|r| r.label() == l).count();
            let waiting = self.selected.iter().zip(&self.resolved).filter(|(on, r)| **on && r.is_none()).count();
            let more = if waiting > 0 { format!(" {waiting} selected line(s) are not looked up yet and are left out.") } else { String::new() };
            ui.label(format!(
                "{} standard library, {} from sources, {} not found, {} ambiguous. Lines not found or ambiguous are left out.{more}",
                count("stdlib"),
                count("source"),
                count("not found"),
                count("ambiguous")
            ));
        }
        let have = self.in_lib.iter().flatten().count();
        if have > 0 {
            ui.label(format!("{have} line(s) already in your library, so they start unticked. Tick one to import it again."));
        }
        if !self.items.is_empty() {
            ui.horizontal(|ui| {
                if ui.small_button("Select all").clicked() {
                    self.selected.fill(true);
                }
                if ui.small_button("Select none").clicked() {
                    self.selected.fill(false);
                }
                ui.weak(format!("{} of {} selected", self.selected.iter().filter(|&&s| s).count(), self.items.len()));
            });
        }
        let cols = [
            Col::new("", 28.0).no_sort(),
            Col::new("Refs", 150.0).no_sort(),
            Col::new("Qty", 48.0).no_sort(),
            Col::new("MPN (editable)", 190.0).no_sort(),
            Col::new("Alternatives", 190.0).no_sort(),
            Col::new("Check", 130.0).no_sort(),
            Col::new("Status", 90.0).no_sort(),
            Col::new("Symbol / footprint / 3D", 150.0).no_sort(),
            Col::new("", 64.0).no_sort(),
            Col::rest("Result").no_sort(),
        ];
        let mut edited = None;
        let mut find = None;
        let mut sort = Sort { col: usize::MAX, asc: true };
        ui_kit::table(ui, "bom_import", &cols, &mut sort, self.items.len(), |n, row| {
            let it = &mut self.items[n];
            row.col(|ui| drop(ui.checkbox(&mut self.selected[n], "")));
            row.col(|ui| drop(ui.label(egui::RichText::new(it.line.refs.join(" ")).monospace())));
            row.col(|ui| drop(ui.label(it.qty.to_string())));
            row.col(|ui| {
                let hint = it.line.value.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut it.line.mpn).hint_text(hint).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY))
                    .on_hover_text("Correct a wrong part number here, then look the lines up again")
                    .changed()
                {
                    edited = Some(n);
                }
            });
            row.col(|ui| {
                ui.horizontal(|ui| {
                    for (j, a) in it.alts.iter().enumerate() {
                        let what = if a.mpn.is_empty() { a.lcsc.clone() } else { format!("{} ({})", a.mpn, a.lcsc) };
                        ui.checkbox(&mut self.alt_on[n][j], &a.lcsc).on_hover_text(format!(
                            "{what}\n{}\nTick to import this alternative too and record it as an alternative of the line's part.",
                            a.text
                        ));
                    }
                });
            });
            row.col(|ui| match (&self.in_lib[n], bi::flag(&it.line)) {
                (Some(sym), _) => drop(
                    ui_kit::badge(ui, "In library", Level::Ok).on_hover_text(format!("Already in your library as {sym}. Tick the line to import it again.")),
                ),
                (None, Some(f)) => {
                    drop(ui_kit::badge(ui, &format!("{}: {}", if matches!(f, bi::Flag::NonPart(_)) { "not a part" } else { "bad MPN" }, f.why()), Level::Warn))
                }
                (None, None) => {}
            });
            match self.resolved.get(n).and_then(Option::as_ref) {
                Some(r) => {
                    let level = match r {
                        Resolved::Stdlib { .. } | Resolved::Source(_) => Level::Ok,
                        Resolved::NotFound(_) => Level::Error,
                        Resolved::Ambiguous(_) => Level::Warn,
                    };
                    row.col(|ui| drop(ui_kit::badge(ui, r.label(), level)));
                    let have = self.avail.get(n).copied().flatten();
                    row.col(|ui| match (r, have) {
                        (Resolved::Stdlib { .. }, _) => drop(ui.label("in standard library")),
                        (_, Some(a)) => {
                            // The same SYM / FP / 3D chips the Library and Import tabs draw.
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (short, what, ok) in [("SYM", "Symbol", a.symbol), ("FP", "Footprint", a.footprint), ("3D", "3D model", a.model)] {
                                let (status, state) = if ok { (ui_kit::Status::Ok, "present") } else { (ui_kit::Status::Missing, "missing") };
                                ui_kit::file_chip(
                                    ui,
                                    short,
                                    status,
                                    &if ok { format!("{what} present") } else { format!("No {what} from the source. Click Find… to look for one.") },
                                    &format!("{what}: {state}"),
                                );
                            }
                        }
                        (Resolved::Source(_), None) if self.running => drop(ui.weak("checking…")),
                        _ => {}
                    });
                    // Anything the lookup did not settle, or the source could not fully supply, can be searched for by hand.
                    let wanted = match r {
                        Resolved::Stdlib { .. } => false,
                        Resolved::Source(_) => have.is_some_and(|a| !(a.symbol && a.footprint && a.model)),
                        _ => true,
                    };
                    row.col(|ui| {
                        if wanted
                            && ui
                                .add(ui_kit::ghost("Find…"))
                                .on_hover_text("Search every source for this part (symbol, footprint, 3D model), then import it and look the line up again")
                                .clicked()
                        {
                            find = Some(n);
                        }
                    });
                    let done = self.generated.as_ref().and_then(|g| g.lines.get(n));
                    row.col(|ui| {
                        ui.label(match done {
                            Some(d) => d
                                .error
                                .clone()
                                .or(d.library_part.clone())
                                .map(|t| if d.alts.is_empty() { t } else { format!("{t}  | alternatives: {}", d.alts.join("; ")) })
                                .unwrap_or_else(|| r.detail()),
                            None => r.detail(),
                        });
                    });
                }
                None => {
                    for _ in 0..4 {
                        row.col(|_| {});
                    }
                }
            }
        });
        if let Some(n) = find {
            let l = &self.items[n].line;
            let query = l
                .supplier_ids
                .iter()
                .map(|(_, id)| id.as_str())
                .chain([l.mpn.as_str(), l.value.as_str()])
                .find(|q| !q.is_empty())
                .unwrap_or_default()
                .to_string();
            cx.send(AppMsg::Search(super::search::Msg::Query(query)));
            cx.tab = crate::Tab::Search;
        }
        if let Some(n) = edited {
            // This line's lookup no longer matches the table; it is selectable again once it is no longer flagged, and Look up selected redoes it.
            (self.resolved[n], self.avail[n], self.generated) = (None, None, None);
            self.in_lib[n] = self.in_library(&self.items[n]);
            self.selected[n] = bi::flag(&self.items[n].line).is_none() && self.in_lib[n].is_none();
        }
    }
}
