#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod import_dialog;
mod library_view;
mod search_view;
mod settings_view;

use eframe::egui::{self, RichText};
use import_dialog::{DialogAction, ImportDialog};
use library_view::LibraryView;
use search_view::{SearchAction, SearchView};
use settings_view::{SettingsAction, SettingsView};
use klm_core::sources::Source;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use klm_core::library::LibraryRoot;
use klm_core::part::Part;
use klm_core::settings::Settings;
use klm_core::sources::{watcher::Watcher, zip_import};
use klm_core::kicad_env;
use klm_core::matching;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// Results sent back from worker threads.
enum Msg {
    Found(Part, PathBuf),
    Log(String),
    Imported { index: usize, notes: Vec<String> },
    ImportFailed(String),
    SearchDone(Vec<(usize, Part)>),
    /// A search result's files, downloaded and ready for the import dialog.
    Fetched(Part, PathBuf),
    FetchDone,
    /// A CAD page was opened for this search result; a matching ZIP may follow.
    CadOpened(Part),
    TestDone,
}

/// An action that edits KiCad's library tables, held until KiCad is closed.
enum Blocked {
    Import { index: usize, part: Part, category: String, targets: Vec<LibraryRoot> },
    Relink,
    MoveGlobal(PathBuf),
}

#[derive(PartialEq)]
enum Tab {
    Search,
    Import,
    Library,
}

struct Pending {
    part: Part,
    /// Extracted files; removed once the part is imported or dismissed.
    work: PathBuf,
}

struct App {
    settings: Settings,
    kicad_config: Result<PathBuf, String>,
    project: Option<PathBuf>,
    setup_path: String,
    pending: Vec<Pending>,
    dialog: Option<ImportDialog>,
    blocked: Option<Blocked>,
    still_open: bool,
    tab: Tab,
    library: LibraryView,
    search: SearchView,
    sources: Arc<Vec<Box<dyn Source>>>,
    busy: bool,
    settings_view: SettingsView,
    settings_open: bool,
    testing: bool,
    /// Search results whose CAD page was opened, waiting for their vendor ZIP.
    awaiting_cad: Vec<Part>,
    /// Set to stop the current Downloads watcher thread.
    watch_stop: Arc<AtomicBool>,
    log: Vec<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

fn main() -> eframe::Result {
    // `--project <file.kicad_pro or folder>`, passed by the KiCad plugin launcher.
    let args: Vec<String> = std::env::args().collect();
    let project_arg = args.iter().position(|a| a == "--project").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([820.0, 560.0]).with_title("KiCad Library Manager"),
        ..Default::default()
    };
    eframe::run_native("KiCad Library Manager", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx, project_arg)))))
}

fn project_dir(p: &Path) -> PathBuf {
    if p.is_dir() { p.to_path_buf() } else { p.parent().map(Path::to_path_buf).unwrap_or_default() }
}

fn new_work_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    std::env::temp_dir().join(format!("klm-zip-{}-{nanos}", std::process::id()))
}

/// Reads one ZIP on the calling thread and reports what it found.
fn scan_zip(zip: &Path, tx: &Sender<Msg>) {
    let work = new_work_dir();
    let name = zip.file_name().unwrap_or_default().to_string_lossy().into_owned();
    match zip_import::read_zip(zip, &work) {
        Ok(Some(part)) => {
            let _ = tx.send(Msg::Found(part, work));
        }
        Ok(None) => {
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(Msg::Log(format!("{name}: no KiCad symbol inside, ignored")));
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(Msg::Log(format!("{name}: {e:#}")));
        }
    }
}

impl App {
    fn new(ctx: &egui::Context, project_arg: Option<PathBuf>) -> Self {
        let mut settings = Settings::load();
        if let Some(p) = &project_arg {
            settings.last_project = Some(project_dir(p));
        }
        let (tx, rx) = channel();
        let sources = Arc::new(klm_core::sources::all(&settings));
        let mut app = Self {
            kicad_config: kicad_env::config_dir().map_err(|e| format!("{e:#}")),
            project: settings.last_project.clone().filter(|p| p.is_dir()),
            setup_path: dirs::document_dir()
                .map(|d| d.join("KiCad").join("KLM-Library").display().to_string())
                .unwrap_or_default(),
            settings,
            pending: vec![],
            dialog: None,
            blocked: None,
            still_open: false,
            tab: Tab::Import,
            library: LibraryView::default(),
            search: SearchView::default(),
            sources,
            busy: false,
            settings_view: SettingsView::new(),
            settings_open: false,
            testing: false,
            awaiting_cad: vec![],
            watch_stop: Arc::new(AtomicBool::new(false)),
            log: vec![],
            tx,
            rx,
        };
        if let Err(e) = &app.kicad_config {
            app.log.push(format!("KiCad not found: {e}"));
        }
        if let Some(g) = app.settings.global_root.clone().filter(|g| !g.is_dir()) {
            app.log.push(format!("Global library not found at {}. If you moved it, click \"Change location…\" and choose its new folder.", g.display()));
        }
        app.start_watcher(ctx.clone());
        app.check_links();
        app
    }

    fn roots(&self) -> Vec<LibraryRoot> {
        self.global_root().into_iter().chain(self.project_root()).collect()
    }

    /// Re-adds KLM libraries that KiCad dropped from its tables (e.g. it saved over them while open).
    fn check_links(&mut self) {
        if self.roots().iter().all(|r| r.unlinked().is_empty() && !r.has_old_paths()) {
            return;
        }
        if kicad_env::kicad_running() {
            self.log.push("Some KLM libraries are missing from KiCad's library list. Close KiCad, then click \"Re-link libraries\".".into());
        } else {
            self.relink();
        }
    }

    fn relink(&mut self) {
        let mut any = false;
        for root in self.roots() {
            match root.fix_old_paths() {
                Ok(0) => {}
                Ok(n) => {
                    self.log.push(format!("Replaced ${{KLM_LIB}} with {} in {n} files", root.dir.display()));
                    any = true;
                }
                Err(e) => self.log.push(format!("Could not update old paths: {e:#}")),
            }
            for cat in root.unlinked() {
                match root.link(&cat) {
                    Ok(_) => self.log.push(format!("Re-linked {} in KiCad", root.nickname(&cat))),
                    Err(e) => self.log.push(format!("Could not re-link {}: {e:#}", root.nickname(&cat))),
                }
                any = true;
            }
        }
        if !any {
            self.log.push("All KLM libraries are already in KiCad's library list.".into());
        }
        self.library.refresh();
    }

    /// Runs `action` now, or holds it behind the "close KiCad" prompt if KiCad is open.
    fn when_kicad_closed(&mut self, action: Blocked, ctx: &egui::Context) {
        if kicad_env::kicad_running() {
            self.still_open = false;
            self.blocked = Some(action);
            return;
        }
        match action {
            Blocked::Import { index, part, category, targets } => self.run_import(index, part, category, targets, ctx.clone()),
            Blocked::Relink => self.relink(),
            Blocked::MoveGlobal(dir) => self.move_global(&dir),
        }
    }

    /// Moves the global library (or adopts it where the user moved it) and fixes every path.
    fn move_global(&mut self, dir: &Path) {
        let Some(root) = self.global_root() else { return };
        match root.relocate(dir) {
            Ok(moved) => {
                self.settings.global_root = Some(moved.dir.clone());
                self.save_settings();
                self.library.refresh();
                self.log.push(format!("Global library is now at {}; KiCad's library list and all paths updated.", moved.dir.display()));
                self.log.push("Boards placed before the move keep the old 3D paths until you run Tools → Update Footprints from Library.".into());
            }
            Err(e) => self.log.push(format!("Could not move the global library: {e:#}")),
        }
    }

    fn kicad_open_prompt(&mut self, ctx: &egui::Context) {
        if self.blocked.is_none() {
            return;
        }
        let mut proceed = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("kicad_open")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("Close KiCad first");
            ui.label("This changes KiCad's library list. KiCad keeps those in memory while it's open and writes them back over any outside changes, so please close every KiCad window, then click Continue.");
            if self.still_open {
                ui.colored_label(ui.visuals().warn_fg_color, "KiCad is still open.");
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                proceed = ui.button("Continue").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel {
            self.blocked = None;
        } else if proceed {
            if kicad_env::kicad_running() {
                self.still_open = true;
            } else if let Some(action) = self.blocked.take() {
                self.when_kicad_closed(action, ctx);
            }
        }
    }

    fn start_watcher(&mut self, ctx: egui::Context) {
        let Some(dir) = self.settings.downloads() else {
            self.log.push("No Downloads folder found; use \"Import ZIP…\" instead.".into());
            return;
        };
        self.log.push(format!("Watching {} for part ZIPs", dir.display()));
        let tx = self.tx.clone();
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

    fn global_root(&self) -> Option<LibraryRoot> {
        let cfg = self.kicad_config.as_ref().ok()?;
        Some(LibraryRoot::global(self.settings.global_root.clone()?, cfg.clone()))
    }

    fn project_root(&self) -> Option<LibraryRoot> {
        self.project.as_deref().map(LibraryRoot::project)
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.log.push(format!("Could not save settings: {e:#}"));
        }
    }

    fn create_global_library(&mut self) {
        let dir = PathBuf::from(self.setup_path.trim());
        let result = (|| -> anyhow::Result<()> {
            let cfg = self.kicad_config.clone().map_err(anyhow::Error::msg)?;
            LibraryRoot::global(dir.clone(), cfg).ensure_layout()
        })();
        match result {
            Ok(()) => {
                self.settings.global_root = Some(dir.clone());
                self.save_settings();
                self.log.push(format!("Global library created at {}", dir.display()));
            }
            Err(e) => self.log.push(format!("Could not create global library: {e:#}")),
        }
    }

    fn run_import(&mut self, index: usize, part: Part, category: String, targets: Vec<LibraryRoot>, ctx: egui::Context) {
        self.busy = true;
        let tx = self.tx.clone();
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
                        let _ = tx.send(Msg::ImportFailed(format!("{nick}: {e:#}")));
                        ctx.request_repaint();
                        return;
                    }
                }
            }
            let _ = tx.send(Msg::Imported { index, notes });
            ctx.request_repaint();
        });
    }

    fn run_search(&mut self, query: String, ctx: egui::Context) {
        self.search.searching = true;
        self.search.results.clear();
        let (tx, sources) = (self.tx.clone(), self.sources.clone());
        std::thread::spawn(move || {
            let mut results = vec![];
            let query = &query;
            std::thread::scope(|s| {
                let handles: Vec<_> = sources.iter().enumerate().map(|(i, src)| s.spawn(move || (i, src.name(), src.search(&query)))).collect();
                for h in handles {
                    match h.join() {
                        Ok((i, _, Ok(parts))) => results.extend(parts.into_iter().map(|p| (i, p))),
                        Ok((_, name, Err(e))) => drop(tx.send(Msg::Log(format!("{name} search failed: {e:#}")))),
                        Err(_) => drop(tx.send(Msg::Log("A search source crashed".into()))),
                    }
                }
            });
            let _ = tx.send(Msg::SearchDone(results));
            ctx.request_repaint();
        });
    }

    /// Downloads a search result's CAD, or opens its CAD page if the source has none to download.
    fn get_result(&mut self, i: usize, ctx: egui::Context) {
        let Some((src, part)) = self.search.results.get(i).cloned() else { return };
        self.busy = true;
        let (tx, sources) = (self.tx.clone(), self.sources.clone());
        std::thread::spawn(move || {
            let source = &sources[src];
            let work = new_work_dir();
            // Sources write their files straight into `work`.
            let _ = std::fs::create_dir_all(&work);
            match source.fetch(&part, &work) {
                Ok(Some(p)) => drop(tx.send(Msg::Fetched(p, work))),
                Ok(None) => {
                    let _ = std::fs::remove_dir_all(&work);
                    match source.cad_page_url(&part) {
                        Some(url) => {
                            let _ = open::that(&url);
                            let _ = tx.send(Msg::CadOpened(part.clone()));
                            let _ = tx.send(Msg::Log(format!("Opened {url}. Download the KiCad ZIP and it will appear under Import.")));
                        }
                        None => drop(tx.send(Msg::Log(format!("{} has no CAD files for {}", source.name(), part.mpn)))),
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&work);
                    let _ = tx.send(Msg::Log(format!("{}: could not get {}: {e:#}", source.name(), part.mpn)));
                }
            }
            let _ = tx.send(Msg::FetchDone);
            ctx.request_repaint();
        });
    }

    /// Saves the settings and rebuilds the source list from them and the stored credentials.
    fn apply_sources(&mut self) {
        self.save_settings();
        self.sources = Arc::new(klm_core::sources::all(&self.settings));
        self.search.results.clear();
    }

    fn test_source(&mut self, name: &'static str, ctx: egui::Context) {
        self.testing = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = klm_core::sources::build(name).ok_or_else(|| anyhow::anyhow!("keys missing")).and_then(|s| s.search("LM358"));
            let line = match result {
                Ok(parts) => format!("{name}: test OK, {} results for LM358", parts.len()),
                Err(e) => format!("{name}: test failed: {e:#}"),
            };
            let _ = tx.send(Msg::Log(line));
            let _ = tx.send(Msg::TestDone);
            ctx.request_repaint();
        });
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut action = SettingsAction::None;
        let modal = egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.set_max_height(520.0);
            action = self.settings_view.show(ui, &mut self.settings, self.testing);
        });
        if modal.should_close() {
            self.settings_open = false;
        }
        match action {
            SettingsAction::None => {}
            SettingsAction::Rebuild => self.apply_sources(),
            SettingsAction::Test(name) => self.test_source(name, ctx.clone()),
            SettingsAction::Note(n) => self.log.push(n),
            SettingsAction::Downloads(dir) => {
                self.settings.downloads_dir = dir;
                self.save_settings();
                self.watch_stop.store(true, Ordering::Relaxed);
                self.start_watcher(ctx.clone());
            }
        }
    }

    fn open_dialog(&mut self, i: usize) {
        let mut categories: Vec<String> = klm_core::categories::DEFAULT.iter().map(|s| s.to_string()).collect();
        for root in self.roots() {
            categories.extend(root.categories());
        }
        categories.sort();
        categories.dedup();
        self.dialog = Some(ImportDialog::new(i, &self.pending[i].part, categories, self.global_root().is_some(), self.project.is_some()));
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Found(mut part, work) => {
                    if let Some(i) = self.awaiting_cad.iter().position(|a| matching::same_mpn(&a.mpn, &part.mpn)) {
                        matching::merge(&mut part, &self.awaiting_cad.remove(i));
                    }
                    self.log.push(format!("Found {} ({})", part.mpn, part.source));
                    self.pending.push(Pending { part, work });
                }
                Msg::Log(s) => self.log.push(s),
                Msg::Imported { index, notes } => {
                    self.busy = false;
                    self.log.extend(notes);
                    self.library.refresh();
                    if index < self.pending.len() {
                        let p = self.pending.remove(index);
                        let _ = std::fs::remove_dir_all(p.work);
                    }
                }
                Msg::ImportFailed(e) => {
                    self.busy = false;
                    self.log.push(format!("Import failed: {e}"));
                }
                Msg::SearchDone(results) => {
                    self.search.searching = false;
                    self.log.push(format!("{} search results", results.len()));
                    self.search.results = results;
                }
                Msg::Fetched(part, work) => {
                    self.pending.push(Pending { part, work });
                    self.tab = Tab::Import;
                    self.open_dialog(self.pending.len() - 1);
                }
                Msg::FetchDone => self.busy = false,
                Msg::CadOpened(part) => self.awaiting_cad.push(part),
                Msg::TestDone => self.testing = false,
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("roots").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.strong("Global library");
            match &self.settings.global_root {
                Some(g) => ui.label(g.display().to_string()),
                None => ui.weak("not set up"),
            };
            ui.horizontal(|ui| {
                if self.global_root().is_some()
                    && ui.button("Change location…").on_hover_text("Move the library, or point KLM at where you moved it").clicked()
                {
                    if let Some(d) = rfd::FileDialog::new().set_title("New location for the global library").pick_folder() {
                        self.when_kicad_closed(Blocked::MoveGlobal(d), &ui.ctx().clone());
                    }
                }
                if ui.button("Re-link libraries").on_hover_text("Add any KLM libraries missing from KiCad's library list").clicked() {
                    self.when_kicad_closed(Blocked::Relink, &ui.ctx().clone());
                }
            });
            ui.end_row();

            ui.strong("Project");
            match &self.project {
                Some(p) => ui.label(p.display().to_string()),
                None => ui.weak("none chosen"),
            };
            ui.horizontal(|ui| {
                if ui.button("Choose project…").clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("KiCad project", &["kicad_pro"]).pick_file() {
                        self.project = Some(project_dir(&f));
                        self.settings.last_project = self.project.clone();
                        self.save_settings();
                        self.library.refresh();
                    }
                }
                if self.project.is_some() && ui.button("Clear").clicked() {
                    self.project = None;
                    self.library.refresh();
                }
            });
            ui.end_row();
        });
    }

    fn setup_screen(&mut self, ui: &mut egui::Ui) {
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
        let can = self.kicad_config.is_ok() && !self.setup_path.trim().is_empty();
        if ui.add_enabled(can, egui::Button::new("Create global library")).clicked() {
            self.create_global_library();
        }
    }

    fn parts_screen(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.strong("Parts to import");
            if ui.button("Import ZIP…").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("Vendor ZIP", &["zip"]).pick_file() {
                    let tx = self.tx.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        scan_zip(&f, &tx);
                        ctx.request_repaint();
                    });
                }
            }
            if self.busy {
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
                    if ui.add_enabled(!self.busy, egui::Button::new("Import…")).clicked() {
                        open = Some(i);
                    }
                    if ui.add_enabled(!self.busy, egui::Button::new("Dismiss")).clicked() {
                        dismiss = Some(i);
                    }
                });
                ui.end_row();
            }
        });
        if let Some(i) = open {
            self.open_dialog(i);
        }
        if let Some(i) = dismiss {
            let p = self.pending.remove(i);
            let _ = std::fs::remove_dir_all(p.work);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_messages();
        let ctx = ui.ctx().clone();

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(6.0);
            self.top_bar(ui);
            ui.add_space(6.0);
        });
        egui::Panel::bottom("log").resizable(true).default_size(120.0).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                for line in &self.log {
                    ui.label(line);
                }
            });
        });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Search, RichText::new("Search").heading());
                ui.selectable_value(&mut self.tab, Tab::Import, RichText::new("Import").heading());
                ui.selectable_value(&mut self.tab, Tab::Library, RichText::new("Library").heading());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Settings…").clicked() {
                        self.settings_open = true;
                    }
                });
            });
            ui.separator();
            match self.tab {
                Tab::Search => {
                    let names: Vec<&str> = self.sources.iter().map(|s| s.name()).collect();
                    match self.search.show(ui, &names, self.busy) {
                        SearchAction::None => {}
                        SearchAction::Search(q) => self.run_search(q, ctx.clone()),
                        SearchAction::Get(i) => self.get_result(i, ctx.clone()),
                    }
                }
                Tab::Import => {
                    if self.settings.global_root.is_none() {
                        self.setup_screen(ui);
                        ui.separator();
                    }
                    self.parts_screen(ui, &ctx);
                }
                Tab::Library => {
                    let (global, project) = (self.global_root(), self.project_root());
                    self.library.show(ui, global.as_ref(), project.as_ref());
                }
            }
        });

        let (global, project) = (self.global_root(), self.project_root());
        if let Some(dialog) = &mut self.dialog {
            match dialog.show(&ctx, global.as_ref(), project.as_ref()) {
                DialogAction::None => {}
                DialogAction::Cancel => self.dialog = None,
                DialogAction::Import { part, category, targets } => {
                    let index = dialog.index;
                    self.dialog = None;
                    let action = Blocked::Import { index, part, category: category.clone(), targets: targets.clone() };
                    // Only new libraries touch KiCad's tables; adding to a linked one is safe while it's open.
                    if targets.iter().any(|r| r.needs_link(&category)) {
                        self.when_kicad_closed(action, &ctx);
                    } else if let Blocked::Import { index, part, category, targets } = action {
                        self.run_import(index, part, category, targets, ctx.clone());
                    }
                }
            }
        }
        self.settings_dialog(&ctx);
        self.kicad_open_prompt(&ctx);
    }
}
