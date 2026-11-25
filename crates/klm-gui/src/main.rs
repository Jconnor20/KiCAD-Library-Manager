#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The app shell: shared state, top bar, log, the "close KiCad first" prompt and tab switching.
//!
//! Everything else lives in `tabs/<name>.rs`, one file per tab, so a feature in one tab edits only that file:
//! - Each tab owns its state (`SearchTab`, `ImportTab`, …), its UI, the actions it starts and its own
//!   message enum (`tabs::<name>::Msg`). Worker threads report back by sending `Msg::<Tab>(…)` on `AppCtx::tx`;
//!   `App::handle_messages` hands each one to that tab's `handle`.
//! - State shared by all tabs (settings, KiCad config dir, project, sources, log, `busy`) is in `AppCtx`,
//!   passed to every tab method as `cx`. Tabs never reach into each other's state; to affect another tab
//!   they send it one of its messages (`cx.send(Msg::Library(library::Msg::Refresh))`).
//! - An action that edits KiCad's library tables that must not run while KiCad is open
//!   (only "Apply now" today) is requested with `cx.when_kicad_closed(msg)`: the shell delivers `msg` at
//!   once, or after the user closes KiCad. Everything else writes at once and queues a re-check
//!   (`kicad_env::defer_table_op`, `register_or_queue`).
//! - Adding a feature to a tab: add its state and `Msg` variants in that tab's file. Adding a new tab:
//!   a file in `tabs/`, a `Msg` variant, a `Tab` variant and one line each in `App` for `ui`/`handle`.

mod shell;
mod tabs;
mod theme;

// Headless UI tests live in tests/gui/ (outside src/ so the folder reads as tests); they are a child module to reach private state.
#[cfg(test)]
#[path = "../tests/gui/mod.rs"]
mod gui_tests;

use eframe::egui;
use klm_core::kicad_env;
use klm_core::library::LibraryRoot;
use klm_core::settings::Settings;
use klm_core::sources::Source;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use tabs::ui_kit::{Level, Section};
use tabs::{bom, bom_import, classes, cleanup, footprint_gen, health, history, import, library, project_check, search, settings};

/// Results sent back from worker threads, and actions held behind the "close KiCad" prompt.
enum Msg {
    /// A log line whose severity is guessed from its wording (see `Log::push`).
    Log(String),
    /// For worker threads: `tx.send(Msg::LogAt(Level::Error, …))`.
    #[allow(dead_code)]
    LogAt(Level, String),
    OpenSettings(Section),
    /// Add queued libraries to KiCad's tables, after the close-KiCad prompt if it is open.
    ApplyPending,
    MoveGlobal(PathBuf),
    /// A newer release exists: (version, release page).
    Update(String, String),
    Search(search::Msg),
    Import(import::Msg),
    Library(library::Msg),
    History(history::Msg),
    Bom(bom::Msg),
    BomImport(bom_import::Msg),
    Settings(settings::Msg),
}

#[derive(PartialEq)]
enum Tab {
    Search,
    Import,
    Library,
    Bom,
    BomImport,
    History,
    Settings,
    Cleanup,
    Classes,
    Health,
    ProjectCheck,
    Footprint,
}

/// The log: lines with a severity. `cx.log(level, text)` is the way to add one; `push`/`extend` are for older call sites.
#[derive(Default)]
struct Log {
    lines: Vec<(Level, String)>,
}

impl Log {
    /// Adds a line of unknown severity: an error if it reads like one, else info.
    fn push(&mut self, s: String) {
        let low = s.to_lowercase();
        let failed = ["could not", "failed", "can't", "cannot", "nothing was changed", "not found"].iter().any(|w| low.contains(w));
        self.lines.push((if failed { Level::Error } else { Level::Info }, s));
    }

    fn extend(&mut self, lines: impl IntoIterator<Item = String>) {
        for s in lines {
            self.push(s);
        }
    }

    fn clear(&mut self) {
        self.lines.clear();
    }

    fn warnings(&self) -> usize {
        self.lines.iter().filter(|(l, _)| *l == Level::Warn).count()
    }

    fn errors(&self) -> usize {
        self.lines.iter().filter(|(l, _)| *l == Level::Error).count()
    }

    fn text(&self) -> String {
        self.lines.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// Fonts for the symbols the GUI uses (✔ ✘ ★ ▲ 📄 🧊 …), which egui's own fonts lack. They come from the system, so
/// nothing is bundled; if none is found those glyphs show as empty boxes.
fn install_fonts(ctx: &egui::Context) {
    let candidates = [
        ("symbols", [r"C:\Windows\Fonts\seguisym.ttf", "/System/Library/Fonts/Apple Symbols.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"]),
        ("emoji", [r"C:\Windows\Fonts\seguiemj.ttf", "", ""]),
    ];
    let mut fonts = egui::FontDefinitions::default();
    let mut at = 1;
    for (name, paths) in candidates {
        let Some(bytes) = paths.iter().find_map(|p| std::fs::read(p).ok()) else { continue };
        fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            // After the main text font, before egui's emoji fonts, so ✔ comes out as a plain glyph in the text colour.
            fonts.families.entry(family).or_default().insert(at, name.into());
        }
        at += 1;
    }
    theme::install(ctx, fonts);
}

/// State every tab can read and change.
struct AppCtx {
    ctx: egui::Context,
    settings: Settings,
    kicad_config: Result<PathBuf, String>,
    project: Option<PathBuf>,
    sources: Arc<Vec<Box<dyn Source>>>,
    /// A download or import is running; buttons that start another are disabled.
    busy: bool,
    tab: Tab,
    log: Log,
    tx: Sender<Msg>,
    /// An action that edits KiCad's library tables, held until KiCad is closed.
    blocked: Option<Msg>,
    still_open: bool,
    /// Libraries waiting to be added to KiCad's tables once it closes.
    pending_libs: usize,
    last_poll: std::time::Instant,
}

impl AppCtx {
    /// Queues a message for the shell or a tab, handled on the next pass of `handle_messages`.
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }

    /// Adds a line to the log, coloured by `level`.
    fn log(&mut self, level: Level, text: impl Into<String>) {
        self.log.lines.push((level, text.into()));
    }

    /// Opens the Settings popup at `section`.
    fn open_settings(&self, section: Section) {
        self.send(Msg::OpenSettings(section));
    }

    /// Sends `msg` now, or holds it behind the "close KiCad" prompt if KiCad is open.
    fn when_kicad_closed(&mut self, msg: Msg) {
        if kicad_env::kicad_running() {
            self.still_open = false;
            self.blocked = Some(msg);
        } else {
            self.send(msg);
        }
    }

    fn global_root(&self) -> Option<LibraryRoot> {
        let cfg = self.kicad_config.as_ref().ok()?;
        Some(LibraryRoot::global(self.settings.global_root.clone()?, cfg.clone()))
    }

    fn project_root(&self) -> Option<LibraryRoot> {
        self.project.as_deref().map(LibraryRoot::project)
    }

    fn roots(&self) -> Vec<LibraryRoot> {
        self.global_root().into_iter().chain(self.project_root()).collect()
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.log.push(format!("Could not save settings: {e:#}"));
        }
    }
}

struct App {
    cx: AppCtx,
    rx: Receiver<Msg>,
    search_tab: search::SearchTab,
    import_tab: import::ImportTab,
    library_tab: library::LibraryTab,
    history_tab: history::HistoryTab,
    cleanup_tab: cleanup::CleanupTab,
    classes_tab: classes::ClassesTab,
    health_tab: health::HealthTab,
    project_check_tab: project_check::ProjectCheckTab,
    footprint_tab: footprint_gen::FootprintTab,
    bom_tab: bom::BomTab,
    bom_import_tab: bom_import::BomImportTab,
    settings_tab: settings::SettingsTab,
    log_open: bool,
    checklist_hidden: bool,
    omni: shell::Omni,
    /// A newer release, once the start-up check has found one.
    update: Option<(String, String)>,
}

fn main() -> eframe::Result {
    // `--project <file.kicad_pro or folder>`, passed by the KiCad plugin launcher.
    let args: Vec<String> = std::env::args().collect();
    // `--config <dir>` (or KLM_CONFIG_DIR): keep settings, library tables and keys in <dir>, for tests.
    if let Some(dir) = args.iter().position(|a| a == "--config").and_then(|i| args.get(i + 1)) {
        klm_core::settings::set_sandbox(Path::new(dir));
    }
    let project_arg = args.iter().position(|a| a == "--project").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    eframe::run_native("KiCad Library Manager", native_options(), Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx, project_arg)))))
}

/// Window size and position are remembered between runs, in the config folder (so a sandbox keeps them too).
fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]).with_title("KiCad Library Manager"),
        persistence_path: klm_core::settings::app_dir().map(|d| d.join("window.ron")),
        ..Default::default()
    }
}

fn project_dir(p: &Path) -> PathBuf {
    if p.is_dir() { p.to_path_buf() } else { p.parent().map(Path::to_path_buf).unwrap_or_default() }
}

/// A fresh path in the temp folder for extracted or downloaded part files.
fn new_work_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    std::env::temp_dir().join(format!("klm-zip-{}-{nanos}", std::process::id()))
}

impl App {
    fn new(ctx: &egui::Context, project_arg: Option<PathBuf>) -> Self {
        install_fonts(ctx);
        let mut settings = Settings::load();
        kicad_env::set_kicad_cli_override(settings.kicad_cli.clone());
        kicad_env::set_pending_queue(kicad_env::default_queue_path());
        klm_core::snapshot::set_store_path(klm_core::store::Store::default_path());
        klm_core::snapshot::set_policy(&settings.history);
        klm_core::symbols::configure(&settings);
        if let Some(p) = &project_arg {
            settings.last_project = Some(project_dir(p));
        }
        let settings_ready = settings.global_root.is_some();
        let (tx, rx) = channel();
        let sources = Arc::new(klm_core::sources::all(&settings));
        let mut cx = AppCtx {
            ctx: ctx.clone(),
            kicad_config: kicad_env::config_dir().map_err(|e| format!("{e:#}")),
            project: settings.last_project.clone().filter(|p| p.is_dir()),
            settings,
            sources,
            busy: false,
            tab: if settings_ready { Tab::Library } else { Tab::Import },
            log: Log::default(),
            tx,
            blocked: None,
            still_open: false,
            pending_libs: 0,
            last_poll: std::time::Instant::now(),
        };
        if let Err(e) = &cx.kicad_config {
            cx.log(Level::Warn, format!("KiCad not found: {e}"));
        }
        if let Some(g) = cx.settings.global_root.clone().filter(|g| !g.is_dir()) {
            cx.log.push(format!("Global library not found at {}. If you moved it, click \"Change location…\" and choose its new folder.", g.display()));
        }
        let import_tab = import::ImportTab::new(&mut cx);
        let mut app = Self {
            import_tab,
            search_tab: search::SearchTab::default(),
            library_tab: library::LibraryTab::default(),
            history_tab: history::HistoryTab::default(),
            cleanup_tab: cleanup::CleanupTab::default(),
            classes_tab: Default::default(),
            health_tab: Default::default(),
            project_check_tab: Default::default(),
            footprint_tab: footprint_gen::FootprintTab::default(),
            bom_tab: bom::BomTab::default(),
            bom_import_tab: bom_import::BomImportTab::default(),
            settings_tab: settings::SettingsTab::new(),
            log_open: false,
            checklist_hidden: false,
            omni: shell::Omni::default(),
            update: None,
            cx,
            rx,
        };
        app.apply_pending();
        app.check_links();
        app.check_update();
        app
    }

    /// One worker asks GitHub for the latest release; failures (offline, no release yet) are silent. Never under a sandbox, i.e. tests.
    fn check_update(&self) {
        if !self.cx.settings.check_updates || klm_core::settings::sandbox_dir().is_some() {
            return;
        }
        let (tx, ctx) = (self.cx.tx.clone(), self.cx.ctx.clone());
        std::thread::spawn(move || {
            if let Ok((version, url)) = klm_core::update::latest_release()
                && klm_core::update::is_newer(&version, env!("CARGO_PKG_VERSION"))
            {
                let _ = tx.send(Msg::Update(version, url));
                ctx.request_repaint();
            }
        });
    }

    /// Adds queued libraries to KiCad's tables if KiCad is closed.
    fn apply_pending(&mut self) {
        let cx = &mut self.cx;
        if kicad_env::pending_count() > 0 {
            match kicad_env::apply_pending() {
                Ok(0) => {}
                Ok(n) => {
                    cx.log.push(format!("Added {n} queued librar{} to KiCad's library list", if n == 1 { "y" } else { "ies" }));
                    cx.send(Msg::Library(library::Msg::Refresh));
                }
                Err(e) => cx.log.push(format!("Could not add queued libraries to KiCad: {e:#}")),
            }
        }
        cx.pending_libs = kicad_env::pending_count();
    }

    /// Re-adds KLM libraries that KiCad dropped from its tables (e.g. it saved over them while open).
    fn check_links(&mut self) {
        let roots = self.cx.roots();
        let (missing, old) = (roots.iter().any(|r| !r.unlinked().is_empty()), roots.iter().any(|r| r.has_old_paths()));
        if !missing && !old {
            return;
        }
        if kicad_env::kicad_running() {
            let what = match (missing, old) {
                (true, true) => "Some KLM libraries are missing from KiCad's library list and some use old ${KLM_LIB} paths",
                (true, false) => "Some KLM libraries are missing from KiCad's library list",
                _ => "Some KLM libraries use old ${KLM_LIB} paths",
            };
            self.cx.log.push(format!("{what}. Close KiCad, then click \"Relink libraries\" on the Library tab."));
        } else {
            self.relink();
        }
    }

    /// Re-adds missing libraries on a worker (it can copy footprints into projects), logging what it did.
    fn relink(&mut self) {
        let (roots, tx, ctx) = (self.cx.roots(), self.cx.tx.clone(), self.cx.ctx.clone());
        std::thread::spawn(move || {
            let r = LibraryRoot::relink_all(&roots);
            for n in r.notes {
                let _ = tx.send(Msg::Log(n));
            }
            for e in r.errors {
                let _ = tx.send(Msg::LogAt(Level::Error, e));
            }
            if r.changed && kicad_env::kicad_running() {
                let _ = tx.send(Msg::Log("KiCad is open: the changes are queued and re-checked when it closes.".into()));
            }
            let _ = tx.send(Msg::Library(library::Msg::Refresh));
            ctx.request_repaint();
        });
    }

    /// Moves the global library (or adopts it where the user moved it) and fixes every path.
    fn move_global(&mut self, dir: &Path) {
        let cx = &mut self.cx;
        let Some(root) = cx.global_root() else { return };
        match root.relocate(dir) {
            Ok(moved) => {
                cx.settings.global_root = Some(moved.dir.clone());
                cx.save_settings();
                cx.send(Msg::Library(library::Msg::Refresh));
                cx.log.push(format!("Global library is now at {}; KiCad's library list and all paths updated.", moved.dir.display()));
                cx.log.push("Boards placed before the move keep the old 3D paths until you run Tools → Update Footprints from Library.".into());
            }
            Err(e) => cx.log.push(format!("Could not move the global library: {e:#}")),
        }
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Log(s) => self.cx.log.push(s),
                Msg::LogAt(level, s) => self.cx.log(level, s),
                Msg::OpenSettings(section) => {
                    self.settings_tab.open(section, &self.cx.settings);
                    self.cx.tab = Tab::Settings;
                }
                Msg::ApplyPending => self.apply_pending(),
                Msg::MoveGlobal(dir) => self.move_global(&dir),
                Msg::Update(version, url) => self.update = Some((version, url)),
                Msg::Search(m) => self.search_tab.handle(m, &mut self.cx),
                Msg::Import(m) => self.import_tab.handle(m, &mut self.cx),
                Msg::Library(m) => self.library_tab.handle(m, &mut self.cx),
                Msg::History(m) => self.history_tab.handle(m, &mut self.cx),
                Msg::Bom(m) => self.bom_tab.handle(m, &mut self.cx),
                Msg::BomImport(m) => self.bom_import_tab.handle(m, &mut self.cx),
                Msg::Settings(m) => self.settings_tab.handle(m, &mut self.cx),
            }
        }
    }

    fn kicad_open_prompt(&mut self) {
        let cx = &mut self.cx;
        if cx.blocked.is_none() {
            return;
        }
        let mut proceed = false;
        let mut cancel = false;
        let ctx = cx.ctx.clone();
        egui::Modal::new(egui::Id::unique("kicad_open")).show(&ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("Close KiCad first");
            ui.label("This changes KiCad's library list. KiCad keeps those in memory while it's open and writes them back over any outside changes, so please close every KiCad window, then click Continue.");
            if cx.still_open {
                ui.colored_label(ui.visuals().warn_fg_color, "KiCad is still open.");
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                proceed = ui.button("Continue").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel {
            cx.blocked = None;
        } else if proceed {
            if kicad_env::kicad_running_now() {
                cx.still_open = true;
            } else if let Some(msg) = cx.blocked.take() {
                cx.send(msg); // just checked uncached; the cached check could still say "open"
            }
        }
    }

    /// The first-run steps still open, each with a button to the Settings page for it. Hidden once all are done.
    fn checklist(&mut self, ui: &mut egui::Ui) {
        let cx = &self.cx;
        // W7.1 adds Libraries / KiCad / Sources / Watch folders pages; point each step at its own then.
        let steps = [
            ("Global library", cx.settings.global_root.is_some(), Section::Libraries),
            ("KiCad found", cx.kicad_config.is_ok(), Section::KiCad),
            ("A source enabled", !cx.sources.is_empty(), Section::Sources),
            ("A watch folder", !cx.settings.watch_dirs.is_empty(), Section::WatchFolders),
        ];
        if self.checklist_hidden || steps.iter().all(|s| s.1) {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.strong("Set up:");
            for (name, done, section) in steps {
                if done {
                    ui.colored_label(Level::Ok.color(ui.visuals()), format!("✔ {name}"));
                } else {
                    ui.colored_label(Level::Warn.color(ui.visuals()), format!("☐ {name}"));
                    if ui.small_button("Open").on_hover_text(format!("Settings: {name}")).clicked() {
                        cx.open_settings(section);
                    }
                }
            }
            if ui.small_button("Hide").clicked() {
                self.checklist_hidden = true;
            }
        });
        ui.separator();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_messages();
        if self.cx.last_poll.elapsed() >= std::time::Duration::from_secs(5) {
            self.cx.last_poll = std::time::Instant::now();
            if self.cx.pending_libs > 0 {
                self.apply_pending();
            }
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(5));

        self.shell(ui);

        self.import_tab.dialogs(&mut self.cx);
        // Floating windows are only drawn on their own tab; their state survives the switch.
        match self.cx.tab {
            Tab::Library => self.library_tab.dialogs(&mut self.cx),
            Tab::History => self.history_tab.dialogs(&mut self.cx),
            Tab::Cleanup => self.cleanup_tab.dialogs(&mut self.cx),
            _ => {}
        }
        self.settings_tab.dialogs(&mut self.cx);
        self.kicad_open_prompt();
    }
}
