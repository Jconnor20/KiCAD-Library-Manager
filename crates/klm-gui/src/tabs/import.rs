use super::batch_dialog::{self, BatchDialog, ImportAllSetup, Progress, SetupAction};
use super::library;
use super::preview::PreviewPanel;
use super::ui_kit::{self, Col, Level, Section, Sort};
use crate::{shell, theme};
use crate::{AppCtx, Msg as AppMsg, new_work_dir};
use eframe::egui;
use klm_core::batch::{self, Op, Target};
use klm_core::library::LibraryRoot;
use klm_core::matching;
use klm_core::part::Part;
use klm_core::sources::combine::{Combined, Kind};
use klm_core::sources::{foreign, watcher::Watcher, zip_import};
use std::collections::HashSet;
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
    Imported { id: u64, notes: Vec<String> },
    ImportFailed(String),
    /// The Downloads folder setting changed.
    RestartWatcher,
    /// Apply an "Import all" plan (KiCad is closed, or the plan doesn't touch its tables).
    Batch(Progress),
    /// "Import library…" finished reading a folder; its files are under the second path.
    Foreign(anyhow::Result<foreign::Scan>, PathBuf),
}

struct Pending {
    /// Stable across removals, unlike a position in the list.
    id: u64,
    part: Part,
    /// Extracted files; removed once the part is imported or dismissed.
    work: PathBuf,
    /// Category the naming rules give it.
    category: String,
    /// Libraries that already hold this MPN, as of when it arrived.
    existing: Vec<String>,
}

impl Pending {
    fn new(cx: &AppCtx, id: u64, part: Part, work: PathBuf) -> Self {
        let category = cx.settings.rules.category_for(&part);
        let mpn = cx.settings.rules.normalise_mpn(&part.mpn);
        let existing = cx.roots().iter().filter(|r| r.has_part(&category, &mpn)).map(|r| r.nickname(&category)).collect();
        Self { id, part, work, category, existing }
    }
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
    /// The first-run page was skipped.
    setup_skipped: bool,
    /// Waiting parts left out of "Import N parts", and the one shown on the right.
    unchecked: HashSet<u64>,
    selected: Option<u64>,
    preview: Option<(u64, PreviewPanel)>,
    /// The MPN box of "No file?".
    mpn: String,
    /// The last imports, read once and again after each import.
    recent: Option<Vec<String>>,
    /// MPN of the single import running, for the status line.
    importing: Option<String>,
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
                (_, Ok((_, true))) => "an Altium project (.LibPkg) with no library files; pick the .SchLib and .PcbLib files with Import files…",
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

fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn run_import(cx: &mut AppCtx, id: u64, part: Part, category: String, targets: Vec<LibraryRoot>) {
    cx.busy = true;
    let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
    std::thread::spawn(move || {
        let mut notes = vec![];
        for root in &targets {
            let nick = root.nickname(&category);
            match root.add_part(&part, &category, true) {
                Ok(new_lib) => {
                    notes.push(format!("Imported {} into {nick}", part.mpn));
                    notes.extend(klm_core::symbols::take_note());
                    notes.extend(klm_core::shadow::take_notes());
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
        let _ = tx.send(AppMsg::Import(Msg::Imported { id, notes }));
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
            setup_skipped: false,
            unchecked: HashSet::new(),
            selected: None,
            preview: None,
            mpn: String::new(),
            recent: None,
            importing: None,
        };
        tab.start_watcher(cx);
        tab
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        self.take_drops(ui.ctx(), cx);
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
                let p = Pending::new(cx, next_id(), part, work);
                self.pending.push(p);
            }
            Msg::Gather { part, work, waiting } => {
                let mut dialog = ImportDialog::new(0, &part, categories(cx), cx.global_root().is_some(), cx.project.is_some(), &cx.settings.rules);
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
            Msg::Imported { id, notes } => {
                cx.busy = false;
                self.importing = None;
                self.recent = None;
                cx.log.extend(notes);
                cx.send(AppMsg::Library(library::Msg::Refresh));
                if let Some(i) = self.pending.iter().position(|p| p.id == id) {
                    let p = self.pending.remove(i);
                    let _ = std::fs::remove_dir_all(p.work);
                }
            }
            Msg::ImportFailed(e) => {
                cx.busy = false;
                self.importing = None;
                cx.log.push(format!("Import failed: {e}"));
            }
            Msg::RestartWatcher => {
                self.watch_stop.store(true, Ordering::Relaxed);
                self.start_watcher(cx);
            }
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
                        self.batch = Some(BatchDialog::Review(batch::plan_with(ops, cx.global_root().as_ref(), cx.project_root().as_ref(), &cx.settings.rules)));
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
                    self.batch = Some(BatchDialog::Review(batch::plan_with(ops, global.as_ref(), project.as_ref(), &cx.settings.rules)));
                }
            }
        }
        if let Some(plan) = batch_dialog::show(&mut self.batch, &cx.ctx) {
            batch_dialog::start(&mut self.batch, plan, cx, |p| AppMsg::Import(Msg::Batch(p)));
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
                    let mut id = dialog.id;
                    // A "Get…" part's files live until its import finishes, like a ZIP's.
                    if let Some(g) = dialog.gathered.take() {
                        id = next_id();
                        self.pending.push(Pending::new(cx, id, part.clone(), g.work));
                    }
                    self.dialog = None;
                    self.importing = Some(part.mpn.clone());
                    // New libraries are queued for KiCad's tables when it closes, so this never needs KiCad closed.
                    run_import(cx, id, part, category, targets);
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
        let dirs = cx.settings.downloads();
        if dirs.is_empty() {
            cx.log.push("No Downloads folder found; use \"Import ZIP…\" instead.".into());
            return;
        }
        cx.log.push(format!("Watching {} for part ZIPs", dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")));
        let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
        self.watch_stop = Arc::new(AtomicBool::new(false));
        let stop = self.watch_stop.clone();
        std::thread::spawn(move || {
            let mut w = Watcher::new(dirs);
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

    /// Scans a folder, or library files copied into one, and opens the review of what it found.
    pub fn import_library(&mut self, picked: Vec<PathBuf>, cx: &mut AppCtx) {
        cx.busy = true;
        let (tx, ctx, rules, work) = (cx.tx.clone(), cx.ctx.clone(), cx.settings.rules.clone(), new_work_dir());
        std::thread::spawn(move || {
            let scan = std::fs::create_dir_all(&work).map_err(anyhow::Error::from).and_then(|_| stage(&picked, &work)).and_then(|dir| foreign::scan(&dir, &rules, &work));
            let _ = tx.send(AppMsg::Import(Msg::Foreign(scan, work)));
            ctx.request_repaint();
        });
    }

    fn open_dialog(&mut self, i: usize, cx: &AppCtx) {
        self.dialog = Some(ImportDialog::new(self.pending[i].id, &self.pending[i].part, categories(cx), cx.global_root().is_some(), cx.project.is_some(), &cx.settings.rules));
    }

    /// True until the global library exists (or the user chose "Skip for now").
    pub fn needs_setup(&self, cx: &AppCtx) -> bool {
        cx.settings.global_root.is_none() && !self.setup_skipped
    }

    /// Parts waiting to be imported, for the rail badge.
    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    /// The first-run page: three numbered steps, nothing written until "Create global library".
    pub fn first_run(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let pal = theme::palette(ui.visuals());
        let w = ui.available_width().min(760.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(w);
            ui.add_space(24.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.label(egui::RichText::new("Set up your global library").size(22.0).strong());
                ui.label(egui::RichText::new("Parts you import go here and are available in every KiCad project. You can change any of this later in Settings.").color(pal.muted));
                ui.add_space(10.0);
                step(ui, 1, "Library location", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.setup_path).desired_width((w - 150.0).max(160.0)));
                        if ui.button("Browse…").clicked()
                            && let Some(d) = rfd::FileDialog::new().pick_folder()
                        {
                            self.setup_path = d.display().to_string();
                        }
                    });
                    ui.label(egui::RichText::new("An empty folder, or an existing KLM library to reuse.").size(12.0).color(pal.muted));
                });
                step(ui, 2, "KiCad", |ui| match &cx.kicad_config {
                    Ok(dir) => {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, Level::Ok.color(ui.visuals()));
                            ui.label("KiCad found ·");
                            ui.label(egui::RichText::new(dir.display().to_string()).monospace().color(pal.muted));
                        });
                        ui.label(egui::RichText::new("KLM adds its libraries to KiCad's global library tables.").size(12.0).color(pal.muted));
                    }
                    Err(e) => {
                        ui.colored_label(Level::Warn.color(ui.visuals()), format!("KiCad was not found: {e}"));
                        if ui.button("Open Settings · KiCad").clicked() {
                            cx.open_settings(Section::KiCad);
                        }
                    }
                });
                step(ui, 3, "Where to find parts", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for s in klm_core::credentials::SOURCES.iter().filter(|s| s.searchable) {
                            let on = cx.sources.iter().any(|k| k.name() == s.name);
                            let text = format!("{} · {}", s.name, if on { "on" } else if s.fields.is_empty() { "off" } else { "needs keys" });
                            if shell::chip(ui, &text, on).clicked() {
                                cx.open_settings(Section::Sources);
                            }
                        }
                    });
                    ui.add_space(4.0);
                    shell::caption(ui, "Folders to watch for vendor ZIPs");
                    ui.horizontal_wrapped(|ui| {
                        for d in cx.settings.downloads() {
                            shell::chip(ui, &d.display().to_string(), true);
                        }
                        if ui.button("Add folder…").clicked() {
                            cx.open_settings(Section::WatchFolders);
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Nothing is written until you press Create.").size(12.0).color(pal.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can = cx.kicad_config.is_ok() && !self.setup_path.trim().is_empty();
                        if ui.add_enabled(can, shell::primary(ui, "Create global library")).clicked() {
                            self.create_global_library(cx);
                        }
                        if ui.add(egui::Button::new("Skip for now").frame(false)).clicked() {
                            self.setup_skipped = true;
                        }
                    });
                });
            });
        });
    }

    /// Files dropped on the window: a ZIP is read like the Downloads watcher's, a folder or library files go to "Import library".
    fn take_drops(&mut self, ctx: &egui::Context, cx: &mut AppCtx) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() {
            return;
        }
        let (zips, rest): (Vec<_>, Vec<_>) = dropped.into_iter().partition(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")));
        for zip in zips {
            let (tx, ctx, rules) = (cx.tx.clone(), cx.ctx.clone(), cx.settings.rules.clone());
            std::thread::spawn(move || {
                scan_zip(&zip, &tx, Some(&rules));
                ctx.request_repaint();
            });
        }
        let (csv, rest): (Vec<_>, Vec<_>) = rest.into_iter().partition(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv")));
        if !csv.is_empty() {
            cx.log.push("A BOM CSV goes on the \"From a BOM\" page: open it there with Open BOM….".into());
        }
        if !rest.is_empty() && !cx.busy {
            self.import_library(rest, cx);
        }
    }

    fn parts_screen(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let pal = theme::palette(ui.visuals());
        let wide = ui.available_width() >= 1000.0;
        let n = self.pending.len();
        ui.horizontal(|ui| {
            if n > 0 {
                ui.label(egui::RichText::new(format!("{n} waiting")).color(pal.muted));
            }
            if cx.busy {
                ui.spinner();
                if let Some(mpn) = &self.importing {
                    ui.label(egui::RichText::new(format!("Importing {mpn}…")).color(pal.muted));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("Watch folders…").frame(false)).clicked() {
                    cx.open_settings(Section::WatchFolders);
                }
                let tip = "Eagle/Altium/EasyEDA libraries kicad-cli can convert, or KiCad libraries (.kicad_sym, .pretty, 3D models). Shows a preview first.";
                if ui.add_enabled(!cx.busy, egui::Button::new("Import files…")).on_hover_text(format!("Import library files (.lbr, .SchLib, .PcbLib, .kicad_sym…). {tip}")).clicked()
                    && let Some(files) = rfd::FileDialog::new().pick_files()
                {
                    self.import_library(files, cx);
                }
                if ui.add_enabled(!cx.busy, egui::Button::new("Import folder…")).on_hover_text(format!("Import a folder of libraries. {tip}")).clicked()
                    && let Some(d) = rfd::FileDialog::new().pick_folder()
                {
                    self.import_library(vec![d], cx);
                }
                if ui.button("Import ZIP…").clicked()
                    && let Some(f) = rfd::FileDialog::new().add_filter("Vendor ZIP", &["zip"]).pick_file()
                {
                    let (tx, ctx, rules) = (cx.tx.clone(), cx.ctx.clone(), cx.settings.rules.clone());
                    std::thread::spawn(move || {
                        scan_zip(&f, &tx, Some(&rules));
                        ctx.request_repaint();
                    });
                }
            });
        });
        ui.add_space(6.0);

        let mut open = None;
        let mut dismiss = None;
        let mut batch = false;
        let mut discard = false;
        let bar_h = if n > 0 { 52.0 } else { 0.0 };
        let (full_w, h) = (ui.available_width(), (ui.available_height() - bar_h).max(120.0));
        let detail_w = if wide && n > 0 { 470.0 } else { 0.0 };
        ui.horizontal_top(|ui| {
            let gap = ui.spacing().item_spacing.x;
            let left = if detail_w > 0.0 { full_w - detail_w - gap } else { full_w };
            ui.allocate_ui_with_layout(egui::vec2(left, h), egui::Layout::top_down(egui::Align::Min), |ui| self.list_ui(ui, cx, &mut open, &mut dismiss));
            if detail_w > 0.0 {
                ui.allocate_ui_with_layout(egui::vec2(detail_w, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                    egui::Frame::new().fill(pal.panel).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                        ui.set_min_size(egui::vec2(detail_w - 24.0, h - 24.0));
                        self.detail_ui(ui, cx, &mut open, &mut dismiss);
                    });
                });
            }
        });
        if n > 0 {
            let ready: Vec<usize> = (0..n).filter(|i| !self.unchecked.contains(&self.pending[*i].id)).collect();
            let conflicts = self.pending.iter().filter(|p| !p.existing.is_empty()).count();
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{} part{} ready", ready.len(), if ready.len() == 1 { "" } else { "s" })).strong());
                if conflicts > 0 {
                    ui.label(egui::RichText::new(format!("· {conflicts} already in your library")).color(pal.muted));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = format!("Import {} part{}", ready.len(), if ready.len() == 1 { "" } else { "s" });
                    if ui.add_enabled(!cx.busy && !ready.is_empty(), shell::primary(ui, &label)).on_hover_text("Import the checked parts in one go, after a preview").clicked() {
                        batch = true;
                    }
                    if ui.add_enabled(!cx.busy, egui::Button::new("Discard all").frame(false)).on_hover_text("Drop every waiting part (the ZIPs stay where they are)").clicked() {
                        discard = true;
                    }
                });
            });
        }
        if let Some(i) = open {
            self.open_dialog(i, cx);
        }
        if let Some(i) = dismiss {
            let p = self.pending.remove(i);
            let _ = std::fs::remove_dir_all(p.work);
        }
        if discard {
            for p in self.pending.drain(..) {
                let _ = std::fs::remove_dir_all(p.work);
            }
        }
        if batch {
            let ready: Vec<&Pending> = self.pending.iter().filter(|p| !self.unchecked.contains(&p.id)).collect();
            self.batch_works = ready.iter().map(|p| p.work.clone()).collect();
            let parts = ready.iter().map(|p| p.part.clone()).collect();
            self.batch_setup = Some(ImportAllSetup::new(parts, categories(cx), cx.global_root().is_some(), cx.project.is_some()));
        }
    }

    /// The left column: the drop target, the MPN box and the table of waiting parts.
    fn list_ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx, open: &mut Option<usize>, dismiss: &mut Option<usize>) {
        let pal = theme::palette(ui.visuals());
        let n = self.pending.len();
        self.drop_zone(ui, cx);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("No file? Search for a part by MPN").color(pal.muted));
            let r = ui.add(egui::TextEdit::singleline(&mut self.mpn).hint_text("MPN").desired_width(160.0));
            let go = (ui.button("Search").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))) && !self.mpn.trim().is_empty();
            if go {
                cx.send(AppMsg::Search(super::search::Msg::Query(self.mpn.trim().to_string())));
                cx.tab = crate::Tab::Search;
            }
        });
        ui.add_space(8.0);
        if n == 0 {
            ui.label(egui::RichText::new("Nothing waiting.").color(pal.muted));
            self.recent_line(ui, cx);
            return;
        }
        ui.label(egui::RichText::new("Waiting to import").strong().size(15.0));
        ui.add_space(4.0);
        let mut sort = Sort { col: usize::MAX, asc: true };
        let mut cols = vec![Col::new("", 28.0).no_sort(), Col::new("MPN", 120.0).no_sort()];
        cols.extend([Col::rest("Detected as").no_sort(), Col::new("Includes", 140.0).no_sort(), Col::new("Status", 118.0).no_sort(), Col::new("", 126.0).no_sort()]);
        let mut toggled = None;
        let mut picked = None;
        ui_kit::table(ui, "pending", &cols, &mut sort, n, |i, row| {
            let p = &self.pending[i];
            row.set_selected(self.selected == Some(p.id));
            row.col(|ui| {
                let mut on = !self.unchecked.contains(&p.id);
                if ui.checkbox(&mut on, "").on_hover_text("Include in \"Import N parts\"").changed() {
                    toggled = Some((p.id, on));
                }
            });
            row.col(|ui| drop(ui.label(egui::RichText::new(&p.part.mpn).monospace().strong())));
            row.col(|ui| drop(ui.label(&p.part.source)));
            row.col(|ui| { ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let have = [("SYM", p.part.symbol_file.is_some()), ("FP", !p.part.footprint_files.is_empty()), ("3D", !p.part.model_files.is_empty()), ("DS", p.part.datasheet_file.is_some() || !p.part.datasheet_url.is_empty())];
                for (t, ok) in have {
                    ui_kit::badge(ui, t, if ok { Level::Ok } else { Level::Warn });
                }
            }); });
            row.col(|ui| {
                if p.existing.is_empty() {
                    ui_kit::badge(ui, "New", Level::Ok);
                } else {
                    ui_kit::badge(ui, "Already in library", Level::Warn).on_hover_text(format!("In {}", p.existing.join(" and ")));
                }
            });
            row.col(|ui| { ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if ui.add_enabled(!cx.busy, egui::Button::new("Import…").small()).clicked() {
                    *open = Some(i);
                }
                if ui.add_enabled(!cx.busy, egui::Button::new("Dismiss").small()).clicked() {
                    *dismiss = Some(i);
                }
            }); });
            if row.response().clicked() {
                picked = Some(p.id);
            }
        });
        if let Some((id, on)) = toggled {
            if on {
                self.unchecked.remove(&id);
            } else {
                self.unchecked.insert(id);
            }
        }
        if picked.is_some() {
            self.selected = picked;
        }
    }

    /// "Drop anything here": the dashed target, which also lights up while files are dragged over the window.
    fn drop_zone(&mut self, ui: &mut egui::Ui, cx: &AppCtx) {
        let pal = theme::palette(ui.visuals());
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        let dirs = cx.settings.downloads();
        let inner = egui::Frame::new().fill(if hovering { pal.accent.gamma_multiply(0.1) } else { pal.bg }).inner_margin(egui::Margin::symmetric(16, 14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Drop anything here").strong().size(15.0));
                ui.label(egui::RichText::new("Vendor ZIPs (SnapMagic, Ultra Librarian, SamacSys), KiCad libraries and 3D models, or Altium/Eagle libraries. KLM works out what each one is.").color(pal.muted));
                let watched = match dirs.as_slice() {
                    [] => "No watch folder set: download a part's KiCad ZIP and use Import ZIP….".to_string(),
                    d => format!("New ZIPs in {} appear here on their own.", d.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")),
                };
                ui.label(egui::RichText::new(watched).monospace().size(11.5).color(pal.muted));
            });
        });
        let r = inner.response.rect;
        let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        let stroke = egui::Stroke::new(1.0, if hovering { pal.accent } else { pal.line });
        ui.painter().extend(egui::Shape::dashed_line(&c, stroke, 6.0, 4.0));
        ui.add_space(8.0);
    }

    /// The selected waiting part: previews of its files and what it would replace.
    fn detail_ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx, open: &mut Option<usize>, dismiss: &mut Option<usize>) {
        let Some(i) = self.selected.and_then(|id| self.pending.iter().position(|p| p.id == id)).or((!self.pending.is_empty()).then_some(0)) else {
            return;
        };
        let pal = theme::palette(ui.visuals());
        let p = &self.pending[i];
        if self.preview.as_ref().is_none_or(|(id, _)| *id != p.id) {
            self.preview = Some((p.id, PreviewPanel::for_part(&cx.ctx, &p.part)));
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.label(egui::RichText::new(&p.part.mpn).monospace().strong().size(16.0));
            ui.label(egui::RichText::new(format!("{} · {} (guessed from your naming rules)", if p.part.manufacturer.is_empty() { "unknown maker" } else { &p.part.manufacturer }, p.category)).color(pal.muted));
            ui.add_space(8.0);
            if let Some((_, panel)) = &mut self.preview {
                panel.show(ui);
            }
            if !p.existing.is_empty() {
                ui.add_space(8.0);
                shell::caption(ui, "Already in your library");
                ui.colored_label(Level::Warn.color(ui.visuals()), format!("{} is already in {}. Import… asks before overwriting it.", p.part.mpn, p.existing.join(" and ")));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!cx.busy, shell::primary(ui, "Import…")).clicked() {
                    *open = Some(i);
                }
                if ui.add_enabled(!cx.busy, egui::Button::new("Dismiss")).clicked() {
                    *dismiss = Some(i);
                }
            });
        });
    }

    /// "Imported earlier": the last few imports with a link to the Library.
    fn recent_line(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let recent = self.recent.get_or_insert_with(|| klm_core::store::Store::default_path().and_then(|p| klm_core::store::Store::open(&p).ok()).and_then(|s| s.recent_imports(6).ok()).unwrap_or_default());
        if recent.is_empty() {
            return;
        }
        ui.add_space(10.0);
        shell::caption(ui, "Imported earlier");
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(recent.join(" · ")).monospace().color(theme::palette(ui.visuals()).muted));
            if ui.link("Open Library").clicked() {
                cx.tab = crate::Tab::Library;
            }
        });
    }
}

/// One numbered card of the first-run page.
fn step(ui: &mut egui::Ui, n: usize, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    let pal = theme::palette(ui.visuals());
    shell::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.label(egui::RichText::new(format!(" {n} ")).monospace().size(11.0).color(pal.accent).background_color(pal.accent.gamma_multiply(0.16)));
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).strong().size(14.0));
                body(ui);
            });
        });
    });
    ui.add_space(8.0);
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
    id: u64,
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
    fn new(id: u64, part: &Part, categories: Vec<String>, has_global: bool, has_project: bool, rules: &klm_core::rules::Rules) -> Self {
        let mut part = part.clone();
        part.mpn = rules.normalise_mpn(&part.mpn);
        Self {
            id,
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

            let existing: Vec<String> = targets.iter().filter(|r| r.has_part(&self.category, &self.part.mpn)).map(|r| r.nickname(&self.category)).collect();
            if !existing.is_empty() {
                ui.add_space(6.0);
                let ask = if self.clashes.is_empty() { "Importing replaces it." } else { "Replace it with this import?" };
                ui.colored_label(ui.visuals().warn_fg_color, format!("Already in your library: {} is in {}. {ask}", self.part.mpn, existing.join(" and ")));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = if !self.clashes.is_empty() { "Replace" } else if existing.is_empty() { "Import" } else { "Reimport…" };
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
