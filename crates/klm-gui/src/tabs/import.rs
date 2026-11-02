use super::library;
use super::preview::PreviewPanel;
use crate::{AppCtx, Msg as AppMsg, Tab, new_work_dir};
use eframe::egui;
use klm_core::library::LibraryRoot;
use klm_core::matching;
use klm_core::part::Part;
use klm_core::sources::{watcher::Watcher, zip_import};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

pub enum Msg {
    /// A vendor ZIP was read (by the Downloads watcher or "Import ZIP…").
    Found(Part, PathBuf),
    /// A search result's files, downloaded and ready for the import dialog.
    Fetched(Part, PathBuf),
    /// A CAD page was opened for this search result; a matching ZIP may follow.
    CadOpened(Part),
    /// Import `part` now (KiCad is closed, or the libraries are already in its tables).
    Run { index: usize, part: Part, category: String, targets: Vec<LibraryRoot> },
    Imported { index: usize, notes: Vec<String> },
    ImportFailed(String),
    /// The Downloads folder setting changed.
    RestartWatcher,
}

struct Pending {
    part: Part,
    /// Extracted files; removed once the part is imported or dismissed.
    work: PathBuf,
}

/// Parts waiting to be imported (from the Downloads watcher, "Import ZIP…" or a search), and the import dialog.
pub struct ImportTab {
    pending: Vec<Pending>,
    dialog: Option<ImportDialog>,
    setup_path: String,
    /// Search results whose CAD page was opened, waiting for their vendor ZIP.
    awaiting_cad: Vec<Part>,
    /// Set to stop the current Downloads watcher thread.
    watch_stop: Arc<AtomicBool>,
}

/// Reads one ZIP on the calling thread and reports what it found.
fn scan_zip(zip: &Path, tx: &Sender<AppMsg>) {
    let work = new_work_dir();
    let name = zip.file_name().unwrap_or_default().to_string_lossy().into_owned();
    match zip_import::read_zip(zip, &work) {
        Ok(Some(part)) => {
            let _ = tx.send(AppMsg::Import(Msg::Found(part, work)));
        }
        Ok(None) => {
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(AppMsg::Log(format!("{name}: no KiCad symbol inside, ignored")));
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(AppMsg::Log(format!("{name}: {e:#}")));
        }
    }
}

fn run_import(cx: &mut AppCtx, index: usize, part: Part, category: String, targets: Vec<LibraryRoot>) {
    cx.busy = true;
    let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
    std::thread::spawn(move || {
        let mut notes = vec![];
        for root in &targets {
            let nick = root.nickname(&category);
            match root.add_part(&part, &category, true) {
                Ok(new_lib) => {
                    notes.push(format!("Imported {} into {nick}", part.mpn));
                    if new_lib {
                        notes.push(format!("Added library {nick} to KiCad"));
                    }
                }
                Err(e) => {
                    let _ = tx.send(AppMsg::Import(Msg::ImportFailed(format!("{nick}: {e:#}"))));
                    ctx.request_repaint();
                    return;
                }
            }
        }
        let _ = tx.send(AppMsg::Import(Msg::Imported { index, notes }));
        ctx.request_repaint();
    });
}

impl ImportTab {
    pub fn new(cx: &mut AppCtx) -> Self {
        let mut tab = Self {
            pending: vec![],
            dialog: None,
            setup_path: dirs::document_dir()
                .map(|d| d.join("KiCad").join("KLM-Library").display().to_string())
                .unwrap_or_default(),
            awaiting_cad: vec![],
            watch_stop: Arc::new(AtomicBool::new(false)),
        };
        tab.start_watcher(cx);
        tab
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        if cx.settings.global_root.is_none() {
            self.setup_screen(ui, cx);
            ui.separator();
        }
        self.parts_screen(ui, cx);
    }

    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        match msg {
            Msg::Found(mut part, work) => {
                if let Some(i) = self.awaiting_cad.iter().position(|a| matching::same_mpn(&a.mpn, &part.mpn)) {
                    matching::merge(&mut part, &self.awaiting_cad.remove(i));
                }
                cx.log.push(format!("Found {} ({})", part.mpn, part.source));
                self.pending.push(Pending { part, work });
            }
            Msg::Fetched(part, work) => {
                self.pending.push(Pending { part, work });
                cx.tab = Tab::Import;
                self.open_dialog(self.pending.len() - 1, cx);
            }
            Msg::CadOpened(part) => self.awaiting_cad.push(part),
            Msg::Run { index, part, category, targets } => run_import(cx, index, part, category, targets),
            Msg::Imported { index, notes } => {
                cx.busy = false;
                cx.log.extend(notes);
                cx.send(AppMsg::Library(library::Msg::Refresh));
                if index < self.pending.len() {
                    let p = self.pending.remove(index);
                    let _ = std::fs::remove_dir_all(p.work);
                }
            }
            Msg::ImportFailed(e) => {
                cx.busy = false;
                cx.log.push(format!("Import failed: {e}"));
            }
            Msg::RestartWatcher => {
                self.watch_stop.store(true, Ordering::Relaxed);
                self.start_watcher(cx);
            }
        }
    }

    /// The import dialog; shown whatever tab is selected.
    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
        if let Some(dialog) = &mut self.dialog {
            match dialog.show(&cx.ctx, global.as_ref(), project.as_ref()) {
                DialogAction::None => {}
                DialogAction::Cancel => self.dialog = None,
                DialogAction::Import { part, category, targets } => {
                    let index = dialog.index;
                    self.dialog = None;
                    // Only new libraries touch KiCad's tables; adding to a linked one is safe while it's open.
                    if targets.iter().any(|r| r.needs_link(&category)) {
                        cx.when_kicad_closed(AppMsg::Import(Msg::Run { index, part, category, targets }));
                    } else {
                        run_import(cx, index, part, category, targets);
                    }
                }
            }
        }
    }

    fn start_watcher(&mut self, cx: &mut AppCtx) {
        let Some(dir) = cx.settings.downloads() else {
            cx.log.push("No Downloads folder found; use \"Import ZIP…\" instead.".into());
            return;
        };
        cx.log.push(format!("Watching {} for part ZIPs", dir.display()));
        let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
        self.watch_stop = Arc::new(AtomicBool::new(false));
        let stop = self.watch_stop.clone();
        std::thread::spawn(move || {
            let mut w = Watcher::new(dir);
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_secs(2));
                for zip in w.poll() {
                    scan_zip(&zip, &tx);
                    ctx.request_repaint();
                }
            }
        });
    }

    fn create_global_library(&mut self, cx: &mut AppCtx) {
        let dir = PathBuf::from(self.setup_path.trim());
        let result = (|| -> anyhow::Result<()> {
            let cfg = cx.kicad_config.clone().map_err(anyhow::Error::msg)?;
            LibraryRoot::global(dir.clone(), cfg).ensure_layout()
        })();
        match result {
            Ok(()) => {
                cx.settings.global_root = Some(dir.clone());
                cx.save_settings();
                cx.log.push(format!("Global library created at {}", dir.display()));
            }
            Err(e) => cx.log.push(format!("Could not create global library: {e:#}")),
        }
    }

    fn open_dialog(&mut self, i: usize, cx: &AppCtx) {
        let mut categories: Vec<String> = klm_core::categories::DEFAULT.iter().map(|s| s.to_string()).collect();
        for root in cx.roots() {
            categories.extend(root.categories());
        }
        categories.sort();
        categories.dedup();
        self.dialog = Some(ImportDialog::new(i, &self.pending[i].part, categories, cx.global_root().is_some(), cx.project.is_some()));
    }

    fn setup_screen(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        ui.heading("Set up your global library");
        ui.label("Parts imported to \"Global\" go here, available in every KiCad project.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.setup_path).desired_width(480.0));
            if ui.button("Browse…").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    self.setup_path = d.display().to_string();
                }
            }
        });
        ui.add_space(8.0);
        let can = cx.kicad_config.is_ok() && !self.setup_path.trim().is_empty();
        if ui.add_enabled(can, egui::Button::new("Create global library")).clicked() {
            self.create_global_library(cx);
        }
    }

    fn parts_screen(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        ui.horizontal(|ui| {
            ui.strong("Parts to import");
            if ui.button("Import ZIP…").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("Vendor ZIP", &["zip"]).pick_file() {
                    let tx = cx.tx.clone();
                    let ctx = cx.ctx.clone();
                    std::thread::spawn(move || {
                        scan_zip(&f, &tx);
                        ctx.request_repaint();
                    });
                }
            }
            if cx.busy {
                ui.spinner();
            }
        });
        ui.weak("Download a part's KiCad ZIP from SnapMagic, Ultra Librarian or SamacSys and it appears here.");
        ui.add_space(6.0);
        if self.pending.is_empty() {
            ui.label("Nothing waiting.");
        }
        let mut open = None;
        let mut dismiss = None;
        egui::Grid::new("pending").striped(true).num_columns(4).spacing([16.0, 6.0]).show(ui, |ui| {
            for (i, p) in self.pending.iter().enumerate() {
                ui.strong(&p.part.mpn);
                ui.label(&p.part.manufacturer);
                ui.weak(&p.part.source);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!cx.busy, egui::Button::new("Import…")).clicked() {
                        open = Some(i);
                    }
                    if ui.add_enabled(!cx.busy, egui::Button::new("Dismiss")).clicked() {
                        dismiss = Some(i);
                    }
                });
                ui.end_row();
            }
        });
        if let Some(i) = open {
            self.open_dialog(i, cx);
        }
        if let Some(i) = dismiss {
            let p = self.pending.remove(i);
            let _ = std::fs::remove_dir_all(p.work);
        }
    }
}

enum DialogAction {
    None,
    Cancel,
    Import { part: Part, category: String, targets: Vec<LibraryRoot> },
}

/// Review a detected part, pick its category and where it goes.
struct ImportDialog {
    index: usize,
    part: Part,
    category: String,
    categories: Vec<String>,
    to_global: bool,
    to_project: bool,
    /// Names of libraries that already hold this MPN, shown once Import is pressed.
    clashes: Vec<String>,
    /// Built on first show; set to `None` after changing `part`'s files so it is drawn again.
    preview: Option<PreviewPanel>,
}

impl ImportDialog {
    fn new(index: usize, part: &Part, categories: Vec<String>, has_global: bool, has_project: bool) -> Self {
        Self {
            index,
            category: klm_core::categories::guess(&part.category_hint).to_string(),
            part: part.clone(),
            categories,
            to_global: has_global,
            to_project: has_project && !has_global,
            clashes: vec![],
            preview: None,
        }
    }

    fn show(&mut self, ctx: &egui::Context, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> DialogAction {
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

            ui.add_space(6.0);
            self.preview.get_or_insert_with(|| PreviewPanel::for_part(ctx, &self.part)).show(ui);

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
