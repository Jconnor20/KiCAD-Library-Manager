use super::batch_dialog::{self, BatchDialog, ImportAllSetup, Progress, SetupAction};
use super::library;
use super::preview::PreviewPanel;
use crate::{AppCtx, Msg as AppMsg, new_work_dir};
use eframe::egui;
use klm_core::batch::{self, Op, Target};
use klm_core::library::LibraryRoot;
use klm_core::matching;
use klm_core::part::Part;
use klm_core::sources::combine::{Combined, Kind};
use klm_core::sources::{foreign, watcher::Watcher, zip_import};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

pub enum Msg {
    /// A vendor ZIP was read (by the Downloads watcher or "Import ZIP…").
    Found(Part, PathBuf),
    /// "Get…" on a search result: open the import dialog while every source is asked for its files,
    /// which go under `work`.
    Gather { part: Part, work: PathBuf, waiting: usize },
    /// One source's answer for the "Get…" whose files go under `work`.
    Answer { work: PathBuf, source: String, found: anyhow::Result<Option<Part>> },
    /// Import `part` now (KiCad is closed, or the libraries are already in its tables).
    Run { index: usize, part: Part, category: String, targets: Vec<LibraryRoot> },
    Imported { index: usize, notes: Vec<String> },
    ImportFailed(String),
    /// The Downloads folder setting changed.
    RestartWatcher,
    /// Apply an "Import all" plan (KiCad is closed, or the plan doesn't touch its tables).
    RunBatch(batch::Plan),
    Batch(Progress),
    /// "Import library…" finished reading a folder; its files are under the second path.
    Foreign(anyhow::Result<foreign::Scan>, PathBuf),
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
    /// "Import all" setup, then its dry run and progress.
    batch_setup: Option<ImportAllSetup>,
    batch: Option<BatchDialog>,
    /// Extracted-files folder of each part in the batch, in batch order, to drop the imported ones from `pending`.
    batch_works: Vec<PathBuf>,
    /// Single-symbol files made by "Import library…"; removed once its batch has run.
    foreign_work: Option<PathBuf>,
    /// "Import library…" picks files instead of a folder.
    pick_files: bool,
}

/// Reads one ZIP on the calling thread and reports what it found.
/// Converts the Altium/Eagle files in `zip` and sends the parts, each given `loose`'s 3D models if it has none.
/// False if the ZIP has no such files.
fn convert_foreign(zip: &Path, work: &Path, rules: &klm_core::rules::Rules, loose: &Part, tx: &Sender<AppMsg>) -> bool {
    let dir = work.join("foreign");
    if !zip_import::extract_foreign(zip, &dir).is_ok_and(|(n, _)| n > 0) {
        return false;
    }
    let mut scan = foreign::scan(&dir, rules, &work.join("conv"));
    if let Ok(s) = &mut scan {
        for f in s.parts.iter_mut().filter(|f| f.part.model_files.is_empty()) {
            f.part.model_files = loose.model_files.clone();
            f.part.warnings.extend(loose.warnings.iter().filter(|w| w.contains("STL")).cloned());
        }
    }
    let _ = tx.send(AppMsg::Import(Msg::Foreign(scan, work.to_path_buf())));
    true
}

/// With `rules`, a ZIP holding only Altium/Eagle files goes to the foreign converter.
fn scan_zip(zip: &Path, tx: &Sender<AppMsg>, rules: Option<&klm_core::rules::Rules>) {
    let work = new_work_dir();
    let name = zip.file_name().unwrap_or_default().to_string_lossy().into_owned();
    match zip_import::read_zip(zip, &work) {
        // Ultra Librarian's Altium/Fusion-only ZIPs carry just a loose .stl next to the foreign files.
        Ok(Some(part)) if part.symbol_file.is_none() && part.footprint_files.is_empty() && rules.is_some_and(|r| convert_foreign(zip, &work, r, &part, tx)) => {}
        Ok(Some(part)) => {
            let _ = tx.send(AppMsg::Import(Msg::Found(part, work)));
        }
        Ok(None) => {
            let msg = match (rules, zip_import::extract_foreign(zip, &work.join("foreign"))) {
                (Some(rules), Ok((n, _))) if n > 0 => {
                    let scan = foreign::scan(&work.join("foreign"), rules, &work.join("conv"));
                    let _ = tx.send(AppMsg::Import(Msg::Foreign(scan, work)));
                    return;
                }
                (_, Ok((_, true))) => "an Altium project (.LibPkg) with no library files; pick the .SchLib and .PcbLib files with Import library…",
                (None, Ok((n, _))) if n > 0 => "only Altium/Eagle files inside; use Import ZIP… to convert them",
                _ => "no KiCad, Altium or Eagle library files inside, ignored",
            };
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(AppMsg::Log(format!("{name}: {msg}")));
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(AppMsg::Log(format!("{name}: {e:#}")));
        }
    }
}

/// The folder to scan for `picked`: a single folder as it is, otherwise the picked files copied into one under `work`.
fn stage(picked: &[PathBuf], work: &Path) -> anyhow::Result<PathBuf> {
    if let [dir] = picked && dir.is_dir() {
        return Ok(dir.clone());
    }
    let dir = work.join("picked");
    std::fs::create_dir_all(&dir)?;
    for (i, f) in picked.iter().enumerate() {
        let sub = dir.join(i.to_string());
        std::fs::create_dir_all(&sub)?;
        std::fs::copy(f, sub.join(f.file_name().unwrap_or_default()))?;
    }
    Ok(dir)
}

/// The built-in categories plus those already in a library.
fn categories(cx: &AppCtx) -> Vec<String> {
    let mut categories: Vec<String> = klm_core::categories::DEFAULT.iter().map(|s| s.to_string()).collect();
    for root in cx.roots() {
        categories.extend(root.categories());
    }
    categories.sort();
    categories.dedup();
    categories
}

/// Moves a ZIP's extracted files from `from` to `to` (inside an open "Get…" dialog's folder, so they're
/// cleaned up with it) and repoints `part` at them. False if they couldn't be moved.
fn adopt(part: &mut Part, from: &Path, to: &Path) -> bool {
    if to.parent().is_some_and(|d| std::fs::create_dir_all(d).is_err()) || std::fs::rename(from, to).is_err() {
        return false;
    }
    let files = part.symbol_file.iter_mut().chain(&mut part.footprint_files).chain(&mut part.model_files).chain(part.datasheet_file.iter_mut()).chain(&mut part.sim_files);
    for f in files {
        if let Ok(rest) = f.strip_prefix(from) {
            *f = to.join(rest);
        }
    }
    true
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
                    if let Some(store) = klm_core::store::Store::default_path().and_then(|p| klm_core::store::Store::open(&p).ok()) {
                        let _ = store.log("import", &part.mpn);
                    }
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
            batch_setup: None,
            batch: None,
            batch_works: vec![],
            foreign_work: None,
            pick_files: false,
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
                if let Some(ImportDialog { gathered: Some(g), part: meta, .. }) = &mut self.dialog
                    && matching::same_mpn(&meta.mpn, &part.mpn)
                    && adopt(&mut part, &work, &g.work.join(format!("zip-{}", g.files.sets.len())))
                {
                    cx.log.push(format!("Added {} to the open import dialog", part.source));
                    g.add(meta, part);
                    return;
                }
                if let Some(i) = self.awaiting_cad.iter().position(|a| matching::same_mpn(&a.mpn, &part.mpn)) {
                    matching::merge(&mut part, &self.awaiting_cad.remove(i));
                }
                cx.log.push(format!("Found {} ({})", part.mpn, part.source));
                self.pending.push(Pending { part, work });
            }
            Msg::Gather { part, work, waiting } => {
                let mut dialog = ImportDialog::new(usize::MAX, &part, categories(cx), cx.global_root().is_some(), cx.project.is_some(), &cx.settings.rules);
                let auto = (part.manufacturer.clone(), part.description.clone());
                dialog.gathered = Some(Gathered { files: Combined::default(), work, waiting, auto, edited: (false, false) });
                self.dialog = Some(dialog);
            }
            Msg::Answer { work, source, found } => {
                match &found {
                    Ok(Some(p)) => cx.log.extend(p.warnings.iter().map(|w| format!("{source}: {}: {w}", p.mpn))),
                    Ok(None) => {}
                    Err(e) => cx.log.push(format!("{source}: {e:#}")),
                }
                if let Some(ImportDialog { gathered: Some(g), part: meta, .. }) = &mut self.dialog
                    && g.work == work
                {
                    g.waiting = g.waiting.saturating_sub(1);
                    if let Ok(Some(p)) = found {
                        g.add(meta, p);
                    }
                } else if !self.pending.iter().any(|p| p.work == work) {
                    // The dialog was cancelled before this source answered.
                    let _ = std::fs::remove_dir_all(&work);
                }
            }
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
            Msg::RunBatch(plan) => batch_dialog::start(&mut self.batch, plan, cx, |p| AppMsg::Import(Msg::Batch(p))),
            Msg::Foreign(scan, work) => {
                cx.busy = false;
                self.batch_works.clear(); // left over from a cancelled "Import all…"; they must not be matched to these results
                self.drop_foreign_work();
                match scan {
                    Err(e) => {
                        cx.log.push(format!("Import library: {e:#}"));
                        let _ = std::fs::remove_dir_all(work);
                    }
                    Ok(scan) => {
                        cx.log.extend(scan.skipped.iter().map(|s| format!("Import library: skipped {s}")));
                        cx.log.extend(scan.unsupported.iter().map(|s| format!("Import library: not converted: {s}")));
                        if scan.parts.is_empty() {
                            cx.log.push("Import library: nothing could be imported (reasons above)".into());
                            let _ = std::fs::remove_dir_all(work);
                            return;
                        }
                        let target = if cx.global_root().is_some() { Target::Global } else { Target::Project };
                        let ops = scan.parts.into_iter().map(|f| Op::ImportPart { part: f.part, category: Some(f.category), target, overwrite: false }).collect();
                        self.foreign_work = Some(work);
                        self.batch = Some(BatchDialog::Review(batch::plan(ops, cx.global_root().as_ref(), cx.project_root().as_ref())));
                    }
                }
            }
            Msg::Batch(p) => {
                let Some(results) = batch_dialog::progress(&mut self.batch, p, cx) else { return };
                self.drop_foreign_work();
                for (work, r) in std::mem::take(&mut self.batch_works).into_iter().zip(&results) {
                    if r.error.is_none() {
                        self.pending.retain(|p| p.work != work);
                        let _ = std::fs::remove_dir_all(work);
                    }
                }
                cx.send(AppMsg::Library(library::Msg::Refresh));
            }
        }
    }

    /// The import dialog; shown whatever tab is selected.
    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
        if let Some(setup) = &mut self.batch_setup {
            match setup.show(&cx.ctx) {
                SetupAction::None => {}
                SetupAction::Cancel => self.batch_setup = None,
                SetupAction::Preview(ops) => {
                    self.batch_setup = None;
                    self.batch = Some(BatchDialog::Review(batch::plan(ops, global.as_ref(), project.as_ref())));
                }
            }
        }
        if let Some(plan) = batch_dialog::show(&mut self.batch, &cx.ctx) {
            if plan.touches_kicad_tables() {
                cx.when_kicad_closed(AppMsg::Import(Msg::RunBatch(plan)));
            } else {
                batch_dialog::start(&mut self.batch, plan, cx, |p| AppMsg::Import(Msg::Batch(p)));
            }
        }
        if let Some(dialog) = &mut self.dialog {
            match dialog.show(&cx.ctx, global.as_ref(), project.as_ref()) {
                DialogAction::None => {}
                DialogAction::Cancel => {
                    if let Some(g) = &dialog.gathered {
                        let _ = std::fs::remove_dir_all(&g.work);
                    }
                    self.dialog = None;
                }
                DialogAction::FetchCad(url) => {
                    let _ = open::that(&url);
                    cx.log.push(format!("Opened {url}. Download the KiCad ZIP and it will be added to the import dialog."));
                    self.awaiting_cad.push(dialog.part.clone());
                }
                DialogAction::Import { part, category, targets } => {
                    let mut index = dialog.index;
                    // A "Get…" part's files live until its import finishes, like a ZIP's.
                    if let Some(g) = dialog.gathered.take() {
                        self.pending.push(Pending { part: part.clone(), work: g.work });
                        index = self.pending.len() - 1;
                    }
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

    fn drop_foreign_work(&mut self) {
        if let Some(w) = self.foreign_work.take() {
            let _ = std::fs::remove_dir_all(w);
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
                    scan_zip(&zip, &tx, None);
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
        self.dialog = Some(ImportDialog::new(i, &self.pending[i].part, categories(cx), cx.global_root().is_some(), cx.project.is_some(), &cx.settings.rules));
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
                    let rules = cx.settings.rules.clone();
                    std::thread::spawn(move || {
                        scan_zip(&f, &tx, Some(&rules));
                        ctx.request_repaint();
                    });
                }
            }
            ui.checkbox(&mut self.pick_files, "files").on_hover_text("Pick library files (.lbr, .SchLib, .PcbLib, .kicad_sym…) instead of a folder");
            if ui.add_enabled(!cx.busy, egui::Button::new("Import library…")).on_hover_text("Import a folder of KiCad libraries (.kicad_sym, .pretty, 3D models), or Eagle/Altium/EasyEDA libraries kicad-cli can convert. Shows a preview first.").clicked() {
                // A folder, or one or more library files (.lbr, .SchLib, .PcbLib, .kicad_sym, ...).
                let picked = if self.pick_files { rfd::FileDialog::new().pick_files() } else { rfd::FileDialog::new().pick_folder().map(|d| vec![d]) };
                if let Some(picked) = picked {
                    cx.busy = true;
                    let (tx, ctx, rules, work) = (cx.tx.clone(), cx.ctx.clone(), cx.settings.rules.clone(), new_work_dir());
                    std::thread::spawn(move || {
                        let scan = std::fs::create_dir_all(&work).map_err(anyhow::Error::from).and_then(|_| stage(&picked, &work)).and_then(|dir| foreign::scan(&dir, &rules, &work));
                        let _ = tx.send(AppMsg::Import(Msg::Foreign(scan, work)));
                        ctx.request_repaint();
                    });
                }
            }
            if ui.add_enabled(!cx.busy && !self.pending.is_empty(), egui::Button::new("Import all…")).on_hover_text("Import every part waiting here in one go, after a preview").clicked() {
                self.batch_works = self.pending.iter().map(|p| p.work.clone()).collect();
                let parts = self.pending.iter().map(|p| p.part.clone()).collect();
                self.batch_setup = Some(ImportAllSetup::new(parts, categories(cx), cx.global_root().is_some(), cx.project.is_some()));
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
    /// Open this CAD download page.
    FetchCad(String),
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
    /// The preview and the files it was drawn from; redrawn when the picked files change.
    preview: Option<(Vec<PathBuf>, PreviewPanel)>,
    /// Set for "Get…": the files every source offered, picked per file type.
    gathered: Option<Gathered>,
}

struct Gathered {
    files: Combined,
    /// Holds every candidate's files.
    work: PathBuf,
    /// Sources that haven't answered yet.
    waiting: usize,
    /// Manufacturer and description as the merge last left them, to tell them from the user's edits.
    auto: (String, String),
    /// The user edited (manufacturer, description), so later answers leave it alone.
    edited: (bool, bool),
}

impl Gathered {
    /// Adds a source's answer to the candidates and the metadata, except for fields the user has edited.
    fn add(&mut self, meta: &mut Part, found: Part) {
        self.edited.0 |= meta.manufacturer != self.auto.0;
        self.edited.1 |= meta.description != self.auto.1;
        let kept = (meta.manufacturer.clone(), meta.description.clone());
        self.files.add(meta, found);
        if self.edited.0 {
            meta.manufacturer = kept.0;
        }
        if self.edited.1 {
            meta.description = kept.1;
        }
        self.auto = (meta.manufacturer.clone(), meta.description.clone());
    }
}

impl ImportDialog {
    fn new(index: usize, part: &Part, categories: Vec<String>, has_global: bool, has_project: bool, rules: &klm_core::rules::Rules) -> Self {
        let mut part = part.clone();
        part.mpn = rules.normalise_mpn(&part.mpn);
        Self {
            index,
            category: rules.category_for(&part),
            part,
            categories,
            to_global: has_global,
            to_project: has_project && !has_global,
            clashes: vec![],
            preview: None,
            gathered: None,
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
            // The preview follows the files picked per source, so swapping one redraws it.
            let shown = self.gathered.as_ref().map_or_else(|| self.part.clone(), |g| g.files.to_part(&self.part));
            let key: Vec<PathBuf> = shown.symbol_file.iter().chain(shown.footprint_files.first()).chain(shown.model_files.first()).cloned().collect();
            if self.preview.as_ref().is_none_or(|(k, _)| *k != key) {
                self.preview = Some((key, PreviewPanel::for_part(ctx, &shown)));
            }
            if let Some((_, panel)) = &mut self.preview {
                panel.show(ui);
            }
            ui.add_space(6.0);
            match &mut self.gathered {
                Some(g) => {
                    if let Some(url) = source_picker(ui, g, &self.part) {
                        action = DialogAction::FetchCad(url);
                    }
                }
                None => {
                    let files = [
                        ("symbol", self.part.symbol_file.is_some() as usize),
                        ("footprint", self.part.footprint_files.len()),
                        ("3D model", self.part.model_files.len().min(1)),
                        ("datasheet", (self.part.datasheet_file.is_some() || !self.part.datasheet_url.is_empty()) as usize),
                        ("sim model", self.part.sim_files.len().min(1)),
                    ];
                    let have: Vec<_> = files.iter().filter(|(_, n)| *n > 0).map(|(f, _)| *f).collect();
                    let missing: Vec<_> = files.iter().filter(|(_, n)| *n == 0).map(|(f, _)| *f).collect();
                    ui.label(format!("Includes: {}", have.join(", ")));
                    if !missing.is_empty() {
                        ui.weak(format!("Not in this download: {}", missing.join(", ")));
                    }
                }
            }
            let part = self.gathered.as_ref().map_or_else(|| self.part.clone(), |g| g.files.to_part(&self.part));

            let targets: Vec<LibraryRoot> = [(self.to_global, global), (self.to_project, project)]
                .into_iter()
                .filter_map(|(on, r)| if on { r.cloned() } else { None })
                .collect();
            let ready = !targets.is_empty()
                && !self.part.mpn.trim().is_empty()
                && !self.category.trim().is_empty()
                && part.symbol_file.is_some()
                && self.gathered.as_ref().is_none_or(|g| g.files.mismatch().is_none());

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
                        action = DialogAction::Import { part: part.clone(), category: self.category.trim().to_string(), targets };
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

/// The per-file-type source picker of a "Get…" dialog. Returns a CAD page to open when asked to.
fn source_picker(ui: &mut egui::Ui, g: &mut Gathered, meta: &Part) -> Option<String> {
    let mut changed = None;
    egui::Grid::new("from").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        for kind in Kind::ALL {
            ui.label(kind.label());
            let options = g.files.candidates(kind);
            let name = |i: Option<usize>| i.map_or("none", |i| g.files.sets[i].source.as_str());
            if options.is_empty() {
                let link = kind == Kind::Datasheet && !meta.datasheet_url.is_empty();
                ui.weak(if link { "web link only" } else { "(missing)" });
            } else {
                let mut pick = g.files.picked(kind);
                egui::ComboBox::from_id_salt(kind.label()).selected_text(name(pick)).show_ui(ui, |ui| {
                    ui.selectable_value(&mut pick, None, "none");
                    for i in options {
                        ui.selectable_value(&mut pick, Some(i), name(Some(i)));
                    }
                });
                if pick != g.files.picked(kind) {
                    changed = Some((kind, pick));
                }
            }
            ui.end_row();
        }
    });
    if let Some((kind, pick)) = changed {
        g.files.choose(kind, pick);
    }
    if g.waiting > 0 {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak(format!("Waiting for {} more source(s)…", g.waiting));
        });
    }
    let mut open = None;
    if !g.files.has_cad() {
        ui.horizontal(|ui| {
            ui.weak("No source has a symbol.");
            if ui.small_button("Fetch from SnapMagic…").clicked() {
                open = Combined::cad_page_url(meta);
            }
        });
    }
    if let Some(why) = g.files.mismatch() {
        ui.colored_label(ui.visuals().error_fg_color, why);
        if ui.button("Switch back").clicked() {
            g.files.reset();
        }
    }
    open
}
