use super::alternatives::AlternativesPanel;
use super::batch_dialog::{self, BatchDialog, Progress};
use super::edit_dialog::{EditAction, EditDialog};
use super::preview::PreviewPanel;
use super::ui_kit::{self, Col, Level, Sort, Status as Cell};
use crate::{AppCtx, Msg as AppMsg};
use eframe::egui::{self, RichText, Sense};
use klm_core::batch::{self, Op};
use klm_core::checks::{self, Issue, Severity};
use klm_core::health::{self, Finding};
use klm_core::library::{ATTACH_EXTENSIONS, AttachKind, Check, LibraryRoot, PartEdit, PartStatus, Scope};
use klm_core::standardise;
use klm_core::symbols::Restyled;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

type Key = (String, String);
type Checks = HashMap<Key, Vec<Issue>>;

pub enum Msg {
    /// Re-read the listing, e.g. after an import.
    Refresh,
    /// Filter the list to this MPN, so another tab can land on the part.
    Show(String),
    /// Apply `edit` to each (category, name) in the `scope` library.
    Edit { scope: Scope, targets: Vec<(String, String)>, edit: PartEdit },
    /// Copy a global part into the project library; `overwrite` replaces an existing copy.
    CopyToProject { category: String, name: String, overwrite: bool },
    Attach { from: LibraryRoot, targets: Vec<LibraryRoot>, category: String, name: String, path: PathBuf },
    /// The datasheet finder has finished: (part, URL used or why not).
    DatasheetsDone(Vec<(String, Result<String, String>)>),
    /// Apply a batch plan (KiCad is closed, or the plan doesn't touch its tables).
    Batch(Progress),
}

/// Something the user asked for on a row; `ui` carries it out and refreshes the view.
enum Action {
    None,
    /// Open the edit dialog for these (category, name) parts.
    Edit(Vec<(String, String)>),
    Attach { scope: Scope, category: String, name: String, path: PathBuf },
    Preview { category: String, name: String },
    /// Show what redrawing the symbol in the house style of its class would do.
    Restyle { category: String, name: String },
    CopyToProject { category: String, name: String, overwrite: bool },
    FindDatasheets(LibraryRoot),
    /// Show the dry run of these operations.
    Batch(Vec<Op>),
    Alternatives(Option<Key>),
    Standardise(LibraryRoot),
    Export(LibraryRoot),
    Health,
}

/// A dry run waiting for the user's answer, or the worker that is producing it.
enum Dry {
    Planning(mpsc::Receiver<standardise::Plan>),
    Standardise(standardise::Plan),
    Restyle { root: LibraryRoot, category: String, name: String, result: Result<Restyled, String> },
    Export { root: LibraryRoot, parts: Vec<Key>, path: String },
    Scanning(mpsc::Receiver<Vec<Finding>>),
    Health(Vec<Finding>),
}

/// Table of the parts in the global or project library, with what each one is missing.
pub struct LibraryTab {
    scope: Scope,
    filter: String,
    /// Category, manufacturer and status filters; empty means all.
    category: String,
    maker: String,
    status: Filter,
    sort: Sort,
    /// The row whose actions the detail panel shows.
    current: Option<Key>,
    finder_results: Option<Vec<(String, Result<String, String>)>>,
    /// The listing; `None` until the first load arrives. A reload keeps the old rows on screen.
    parts: Option<Vec<PartStatus>>,
    /// Folder of the library `parts` and the running loads belong to.
    dir: Option<PathBuf>,
    /// The listing and the part checks are read on threads; the receivers are replaced on every reload, so a
    /// superseded load can never land in the new view.
    parts_rx: Option<mpsc::Receiver<Vec<PartStatus>>>,
    /// Part checks for `parts`; they read every footprint and 3D model, so they arrive after the listing.
    checks: Option<Checks>,
    checks_rx: Option<mpsc::Receiver<Checks>>,
    /// The next frame starts a reload.
    stale: bool,
    /// Bumped whenever `parts` or `checks` change, which rebuilds `view`.
    generation: u64,
    /// Indices into `parts` of the rows shown, filtered and sorted.
    view: Vec<usize>,
    view_key: Option<ViewKey>,
    pub(crate) attach_dialog: Option<AttachDialog>,
    /// A worker is downloading datasheets.
    downloading: bool,
    /// Ticked rows; always a subset of the rows shown.
    selected: HashSet<Key>,
    /// Where a shift-click range starts.
    anchor: Option<Key>,
    edit_dialog: Option<EditDialog>,
    batch: Option<BatchDialog>,
    dry: Option<Dry>,
    /// The open preview window and its title.
    preview: Option<(String, PreviewPanel)>,
    alternatives: Option<AlternativesPanel>,
}

impl Default for LibraryTab {
    fn default() -> Self {
        Self {
            scope: Scope::Global,
            filter: String::new(),
            category: String::new(),
            maker: String::new(),
            status: Filter::All,
            sort: Sort { col: 1, asc: true },
            current: None,
            finder_results: None,
            parts: None,
            dir: None,
            parts_rx: None,
            checks: None,
            checks_rx: None,
            stale: true,
            generation: 0,
            view: Vec::new(),
            view_key: None,
            attach_dialog: None,
            downloading: false,
            selected: HashSet::new(),
            anchor: None,
            edit_dialog: None,
            batch: None,
            dry: None,
            preview: None,
            alternatives: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Incomplete,
    NoDatasheet,
    NoModel,
}

impl Filter {
    const ALL: [(Filter, &'static str); 4] = [(Filter::All, "Any status"), (Filter::Incomplete, "Incomplete"), (Filter::NoDatasheet, "No datasheet"), (Filter::NoModel, "No 3D model")];

    fn matches(self, p: &PartStatus) -> bool {
        let no = |c: Check| matches!(c, Check::Missing | Check::Broken);
        match self {
            Filter::All => true,
            Filter::Incomplete => incomplete(p),
            Filter::NoDatasheet => no(p.datasheet),
            Filter::NoModel => no(p.model),
        }
    }
}

/// What decides which rows are shown and in which order.
#[derive(Clone, PartialEq)]
struct ViewKey {
    generation: u64,
    filter: String,
    category: String,
    maker: String,
    status: Filter,
    sort: Sort,
}

/// Column positions in the table; 0 is the tick box.
const CATEGORY: usize = 1;
const MPN: usize = 2;
const MAKER: usize = 3;
const FOOTPRINT: usize = 4;
const MODEL: usize = 5;
const DATASHEET: usize = 6;
const SIM: usize = 7;
const IN_KICAD: usize = 8;
const CHECKS: usize = 9;

/// Worst-first rank, so ascending puts the parts with problems on top.
fn rank(c: Check) -> u8 {
    match c {
        Check::Broken => 0,
        Check::Missing => 1,
        Check::Link => 2,
        Check::Ok => 3,
    }
}

fn compare(col: usize, a: &PartStatus, b: &PartStatus, issues: impl Fn(&PartStatus) -> usize) -> std::cmp::Ordering {
    match col {
        CATEGORY => a.category.to_lowercase().cmp(&b.category.to_lowercase()),
        MPN => a.mpn.to_lowercase().cmp(&b.mpn.to_lowercase()),
        MAKER => a.manufacturer.to_lowercase().cmp(&b.manufacturer.to_lowercase()),
        FOOTPRINT => rank(a.footprint).cmp(&rank(b.footprint)),
        MODEL => rank(a.model).cmp(&rank(b.model)),
        DATASHEET => rank(a.datasheet).cmp(&rank(b.datasheet)),
        SIM => rank(a.sim).cmp(&rank(b.sim)),
        IN_KICAD => a.in_kicad.cmp(&b.in_kicad),
        CHECKS => issues(b).cmp(&issues(a)),
        _ => std::cmp::Ordering::Equal,
    }
}

/// The file picker for Attach; tests set the answer instead of opening a native dialog.
#[cfg(not(test))]
fn pick_attachment() -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("Datasheet, sim, 3D model or footprint ZIP", ATTACH_EXTENSIONS).pick_file()
}

#[cfg(test)]
pub(crate) static TEST_PICK: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn pick_attachment() -> Option<PathBuf> {
    let _ = ATTACH_EXTENSIONS;
    TEST_PICK.lock().unwrap().clone()
}

/// The part's actions as full-size buttons; one that doesn't apply is disabled and says why.
/// Used by the detail panel and the row's context menu (`menu` closes the menu on a click).
fn part_actions(ui: &mut egui::Ui, p: &PartStatus, scope: Scope, has_project: bool, menu: bool, action: &mut Action) {
    let key = (p.category.clone(), p.name.clone());
    let btn = |ui: &mut egui::Ui, label: &str, enabled: bool, tip: &str, why_not: &str| {
        let r = ui.add_enabled(enabled, egui::Button::new(label));
        let clicked = if enabled { r.on_hover_text(tip).clicked() } else { r.on_disabled_hover_text(why_not).clicked() };
        if clicked && menu {
            ui.close();
        }
        clicked
    };
    if btn(ui, "Edit…", true, "Edit MPN, maker, description, category, supplier numbers, datasheet and tags", "") {
        *action = Action::Edit(vec![key.clone()]);
    }
    if btn(ui, "Preview", true, "Show the symbol, footprint and 3D model", "") {
        *action = Action::Preview { category: key.0.clone(), name: key.1.clone() };
    }
    if btn(ui, "Restyle symbol…", true, "Redraw the symbol in the house style for its kind of part (resistor, capacitor, IC box…). Shows what changes first. Keeps every field; undo it in History. Wires on schematics that already use it may need fixing.", "") {
        *action = Action::Restyle { category: key.0.clone(), name: key.1.clone() };
    }
    if btn(ui, "Attach…", true, "Add a datasheet (PDF), SPICE model, 3D model (STEP/WRL) or a footprint/3D ZIP without a symbol", "") {
        if let Some(path) = pick_attachment() {
            *action = Action::Attach { scope, category: key.0.clone(), name: key.1.clone(), path };
        }
    }
    let (can_copy, why) = match (scope, has_project) {
        (Scope::Project, _) => (false, "This part is already in the project library"),
        (_, false) => (false, "Choose a project first"),
        _ => (true, ""),
    };
    if btn(ui, "Copy to project", can_copy, "Copy this part into the project's library. Shift-click to replace a copy already there.", why) {
        let overwrite = ui.input(|i| i.modifiers.shift);
        *action = Action::CopyToProject { category: key.0.clone(), name: key.1.clone(), overwrite };
    }
    let target = p.datasheet_target.as_deref();
    if btn(ui, "Open datasheet", target.is_some(), "Open the datasheet in your PDF viewer or browser", "This part has no datasheet") {
        if let Some(t) = target {
            let _ = open::that(t);
        }
    }
}

/// Sim models are optional, so only a broken one counts against a part.
fn incomplete(p: &PartStatus) -> bool {
    let bad = |c: Check| matches!(c, Check::Missing | Check::Broken);
    bad(p.footprint) || bad(p.model) || bad(p.datasheet) || p.sim == Check::Broken || !p.in_kicad
}

fn cell(c: Check) -> Cell {
    match c {
        Check::Ok => Cell::Ok,
        Check::Link => Cell::Link,
        Check::Missing => Cell::Missing,
        Check::Broken => Cell::Broken,
    }
}

fn issues_cell(ui: &mut egui::Ui, issues: Option<&Vec<Issue>>) {
    let Some(issues) = issues else {
        ui.weak("…").on_hover_text("Checking…");
        return;
    };
    let Some(worst) = issues.iter().map(|i| i.severity).max() else {
        ui.label(RichText::new("✔").color(Level::Ok.color(ui.visuals()))).on_hover_text("No problems found");
        return;
    };
    let color = if worst == Severity::Error { Level::Error } else { Level::Warn }.color(ui.visuals());
    let tip: Vec<_> = issues.iter().map(|i| format!("• {}", i.message)).collect();
    ui.label(RichText::new(issues.len().to_string()).color(color)).on_hover_text(tip.join("\n"));
}

static NO_ISSUES: Vec<Issue> = Vec::new();

/// The ticked rows in a stable order.
fn sorted(selected: &HashSet<Key>) -> Vec<Key> {
    let mut targets: Vec<_> = selected.iter().cloned().collect();
    targets.sort();
    targets
}

/// What a click on a row's tick box or body asks for.
enum Click {
    /// The tick box: toggle, or with shift extend from the anchor.
    Tick(usize, bool),
    /// The row: make it current; command toggles it, shift extends from the anchor.
    Row(usize, egui::Modifiers),
    /// A right-click: make it current without changing the ticks.
    Menu(usize),
}

impl LibraryTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
        match self.show(ui, global.as_ref(), project.as_ref()) {
            Action::None => {}
            Action::FindDatasheets(root) => self.find_datasheets(root, cx),
            Action::Batch(ops) => self.batch = Some(BatchDialog::Review(batch::plan(ops, global.as_ref(), project.as_ref()))),
            Action::Alternatives(open_on) => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    self.alternatives = Some(AlternativesPanel::new(root, &cx.ctx, open_on));
                }
            }
            Action::Standardise(root) => {
                let (tx, rx) = mpsc::channel();
                let (rules, ctx) = (cx.settings.standards.clone(), cx.ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(standardise::plan(&root, &rules));
                    ctx.request_repaint();
                });
                self.dry = Some(Dry::Planning(rx));
            }
            Action::Export(root) => {
                // A sandboxed run (tests, --config) never writes outside its sandbox.
                let path = klm_core::settings::sandbox_dir().or_else(dirs::document_dir).or_else(dirs::home_dir).unwrap_or_default().join("klm_export.zip");
                self.dry = Some(Dry::Export { root, parts: sorted(&self.selected), path: path.display().to_string() });
            }
            Action::Health => {
                if let Some(dir) = cx.project.clone() {
                    let (tx, rx) = mpsc::channel();
                    let (tables, ctx) = (cx.kicad_config.clone().ok(), cx.ctx.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send(health::scan(&dir, tables.as_deref()));
                        ctx.request_repaint();
                    });
                    self.dry = Some(Dry::Scanning(rx));
                }
            }
            Action::Edit(targets) => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    self.edit_dialog = Some(EditDialog::new(self.scope, targets, root, cx.settings.rules.clone()));
                }
            }
            Action::Preview { category, name } => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    self.preview = Some((format!("Preview: {name}"), PreviewPanel::for_library(&cx.ctx, root, &category, &name)));
                }
            }
            Action::Restyle { category, name } => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    let result = klm_core::symbols::restyle_part(root, &category, &name, &cx.settings.rules, &cx.settings.standards, false).map_err(|e| format!("{e:#}"));
                    self.dry = Some(Dry::Restyle { root: root.clone(), category, name, result });
                }
            }
            Action::Attach { scope, category, name, path } => self.attach_dialog = Some(AttachDialog::new(scope, category, name, path)),
            Action::CopyToProject { category, name, overwrite } => {
                // Only a library new to the project touches KiCad's tables.
                if project.as_ref().is_some_and(|p| p.needs_link(&category)) {
                    cx.send(AppMsg::Library(Msg::CopyToProject { category, name, overwrite }));
                } else {
                    self.copy_to_project(&category, &name, overwrite, cx);
                }
            }
        }
    }

    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        match msg {
            Msg::Refresh => self.stale = true,
            Msg::Show(mpn) => (self.filter, self.category, self.maker, self.status) = (mpn, String::new(), String::new(), Filter::All),
            Msg::DatasheetsDone(results) => {
                self.downloading = false;
                self.stale = true;
                self.finder_results = Some(results);
            }
            Msg::Batch(p) => {
                if batch_dialog::progress(&mut self.batch, p, cx).is_some() {
                    self.stale = true;
                }
            }
            Msg::Edit { scope, targets, edit } => self.edit(scope, &targets, &edit, cx),
            Msg::CopyToProject { category, name, overwrite } => self.copy_to_project(&category, &name, overwrite, cx),
            Msg::Attach { from, targets, category, name, path } => self.attach(&from, &targets, &category, &name, &path, cx),
        }
    }

    /// The dialogs; shown whatever tab is selected.
    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
        if let Some(plan) = batch_dialog::show(&mut self.batch, &cx.ctx) {
            batch_dialog::start(&mut self.batch, plan, cx, |p| AppMsg::Library(Msg::Batch(p)));
        }
        self.dry_run(cx);
        if let Some(results) = &self.finder_results {
            let mut open = true;
            let found = results.iter().filter(|(_, r)| r.is_ok()).count();
            egui::Window::new("Find missing datasheets").open(&mut open).collapsible(false).default_width(560.0).show(&cx.ctx, |ui| {
                if results.is_empty() {
                    ui.label("Every part already has a datasheet.");
                }
                ui.label(format!("{found} found, {} not found", results.len() - found));
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for (mpn, r) in results {
                        match r {
                            Ok(url) => ui.label(format!("✔ {mpn}: from {url}")),
                            Err(why) => ui.label(RichText::new(format!("✘ {mpn}: {why}")).color(ui.visuals().warn_fg_color)),
                        };
                    }
                });
            });
            if !open {
                self.finder_results = None;
            }
        }
        if self.alternatives.as_mut().is_some_and(|a| !a.window(cx)) {
            self.alternatives = None;
        }
        if let Some((title, panel)) = &mut self.preview {
            if !panel.window(&cx.ctx, title) {
                self.preview = None;
            }
        }
        if let Some(dialog) = &mut self.edit_dialog {
            match dialog.show(&cx.ctx) {
                EditAction::None => {}
                EditAction::Cancel => self.edit_dialog = None,
                EditAction::Apply { scope, targets, edit } => {
                    self.edit_dialog = None;
                    let root = if scope == Scope::Global { &global } else { &project };
                    // Only a category library new to KiCad's tables touches them.
                    let new_lib = root.as_ref().is_some_and(|r| edit.category.as_deref().is_some_and(|c| r.needs_link(c)));
                    if new_lib {
                        cx.send(AppMsg::Library(Msg::Edit { scope, targets, edit }));
                    } else {
                        self.edit(scope, &targets, &edit, cx);
                    }
                }
            }
        }
        if let Some(dialog) = &mut self.attach_dialog {
            match dialog.show(&cx.ctx, global.as_ref(), project.as_ref()) {
                AttachAction::None => {}
                AttachAction::Cancel => self.attach_dialog = None,
                AttachAction::Attach { targets } => {
                    let (scope, category, name, path) = (dialog.from, dialog.category.clone(), dialog.name.clone(), dialog.path.clone());
                    self.attach_dialog = None;
                    let from = match scope {
                        Scope::Global => global.clone(),
                        Scope::Project => project.clone(),
                    };
                    match from {
                        // Only a library that doesn't hold the part yet can be new to KiCad's tables.
                        Some(from) if targets.iter().any(|t| !t.has_part(&category, &name) && t.needs_link(&category)) => {
                            cx.send(AppMsg::Library(Msg::Attach { from, targets, category, name, path }));
                        }
                        Some(from) => self.attach(&from, &targets, &category, &name, &path, cx),
                        None => cx.log.push(format!("Could not add the file to {name}: its library is not available")),
                    }
                }
            }
        }
    }

    /// The Standardise, Restyle, Export and Health dialogs: each shows what it would do and waits for Apply.
    /// Polled here, not in `show`, so a worker's answer is picked up whatever tab is open.
    fn dry_run(&mut self, cx: &mut AppCtx) {
        match &self.dry {
            Some(Dry::Planning(rx)) => match rx.try_recv() {
                Ok(plan) => self.dry = Some(Dry::Standardise(plan)),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(_) => self.dry = None,
            },
            Some(Dry::Scanning(rx)) => match rx.try_recv() {
                Ok(findings) => self.dry = Some(Dry::Health(findings)),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(_) => self.dry = None,
            },
            _ => {}
        }
        let ctx = cx.ctx.clone();
        let working = matches!(self.dry, Some(Dry::Planning(_)));
        match &mut self.dry {
            None => {}
            Some(Dry::Planning(_)) | Some(Dry::Scanning(_)) => {
                let m = egui::Modal::new(egui::Id::NULL.with("dry_working")).show(&ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading(if working { "Standardise" } else { "Health report" });
                    ui.label(if working { "Checking every symbol and footprint…" } else { "Checking the project's library…" });
                    ui.add_space(8.0);
                    ui.button("Cancel").clicked()
                });
                if m.inner || m.should_close() {
                    self.dry = None;
                }
            }
            Some(Dry::Standardise(plan)) => {
                let mut sections: Vec<_> = plan.files.iter().map(|f| (f.label.clone(), f.changes.clone())).collect();
                sections.extend(plan.errors.iter().map(|e| ("Can't be read".to_string(), vec![e.clone()])));
                let note = format!("{}
The rules are in Settings → Standards. A snapshot is taken first, so History can roll it back.", plan.root.dir.display());
                match batch_dialog::review(&ctx, "dry_standardise", "Standardise", &sections, !plan.files.is_empty(), |ui| {
                    ui.weak(note);
                }) {
                    Some(true) if cx.busy => cx.log(Level::Warn, "Standardise: wait for the running import or download to finish, then press Apply again"),
                    Some(true) => {
                        if let Some(Dry::Standardise(plan)) = self.dry.take() {
                            {
                                let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
                                std::thread::spawn(move || {
                                    let dir = plan.root.dir.display().to_string();
                                    let _ = tx.send(match standardise::apply(&plan) {
                                        Ok(n) => AppMsg::LogAt(Level::Ok, format!("Standardised {n} file(s) in {dir}")),
                                        Err(e) => AppMsg::LogAt(Level::Error, format!("Standardise failed, nothing changed in {dir}: {e:#}")),
                                    });
                                    let _ = tx.send(AppMsg::Library(Msg::Refresh));
                                    ctx.request_repaint();
                                });
                            }
                        }
                    }
                    Some(false) => self.dry = None,
                    None => {}
                }
            }
            Some(Dry::Restyle { root, category, name, result }) => {
                let (lines, can_apply) = match result {
                    Ok(r) if r.changed => (r.changes.clone(), true),
                    Ok(r) => (r.changes.clone(), false),
                    Err(e) => (vec![format!("can't be restyled: {e}")], false),
                };
                let sections = vec![(format!("{category} / {name}"), lines)];
                let note = "Keeps every field. A snapshot is taken first, so History can undo it. Wires on schematics that already use the symbol may need fixing.";
                match batch_dialog::review(&ctx, "dry_restyle", "Restyle symbol", &sections, can_apply, |ui| {
                    ui.weak(note);
                }) {
                    Some(true) if cx.busy => cx.log(Level::Warn, "Restyle: wait for the running import or download to finish, then press Apply again"),
                    Some(true) => {
                        let line = match klm_core::symbols::restyle_part(root, category, name, &cx.settings.rules, &cx.settings.standards, true) {
                            Ok(done) => (Level::Ok, format!("{name}: {}", done.changes.join(", "))),
                            Err(e) => (Level::Error, format!("Could not restyle {name}: {e:#}")),
                        };
                        cx.log(line.0, line.1);
                        self.dry = None;
                        self.stale = true;
                    }
                    Some(false) => self.dry = None,
                    None => {}
                }
            }
            Some(Dry::Export { root, parts, path }) => {
                let sections: Vec<_> = parts.iter().map(|(c, n)| (format!("{c} / {n}"), vec![])).collect();
                let title = format!("Export {} part(s) as a ZIP", parts.len());
                let can = !parts.is_empty() && !path.trim().is_empty();
                let answer = batch_dialog::review(&ctx, "dry_export", &title, &sections, can, |ui| {
                    ui.add_space(6.0);
                    if Path::new(path.trim()).exists() {
                        ui.colored_label(ui.visuals().warn_fg_color, "That file exists and will be replaced.");
                    }
                    ui.label("One symbol library, one footprint library and their files, with relative paths, for KiCad, Altium or Fusion.");
                    ui.horizontal(|ui| {
                        ui.label("Save as");
                        ui.add(egui::TextEdit::singleline(path).desired_width(360.0));
                        if ui.button("Browse…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().add_filter("ZIP", &["zip"]).set_file_name("klm_export.zip").save_file() {
                                *path = p.display().to_string();
                            }
                        }
                    });
                });
                match answer {
                    Some(true) => {
                        let (root, parts, zip, tx, ctx) = (root.clone(), parts.clone(), PathBuf::from(path.trim()), cx.tx.clone(), cx.ctx.clone());
                        self.dry = None;
                        std::thread::spawn(move || {
                            let _ = tx.send(match klm_core::export::export_zip(&root, &parts, &zip) {
                                Ok(n) => AppMsg::LogAt(Level::Ok, format!("Exported {n} part(s) to {}", zip.display())),
                                Err(e) => AppMsg::LogAt(Level::Error, format!("Could not export: {e:#}")),
                            });
                            ctx.request_repaint();
                        });
                    }
                    Some(false) => self.dry = None,
                    None => {}
                }
            }
            Some(Dry::Health(findings)) => {
                let (mut copy, mut close) = (false, false);
                let modal = egui::Modal::new(egui::Id::NULL.with("dry_health")).show(&ctx, |ui| {
                    ui.set_width(620.0);
                    ui.heading("Health report");
                    if findings.is_empty() {
                        ui.label("No problems found in the project's library.");
                    } else {
                        ui.label(format!("{} finding(s)", findings.len()));
                    }
                    egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                        for f in findings.iter() {
                            ui.horizontal_wrapped(|ui| {
                                let level = if f.severity == Severity::Error { Level::Error } else { Level::Warn };
                                ui_kit::badge(ui, f.kind, level);
                                ui.label(RichText::new(&f.item).strong());
                                ui.label(&f.message);
                            });
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        copy = ui.button("Copy as CSV").clicked();
                        close = ui.button("Close").clicked();
                    });
                });
                if copy {
                    ctx.copy_text(health::to_csv(findings));
                }
                if close || modal.should_close() {
                    self.dry = None;
                }
            }
        }
    }

    /// Copies `path` into each target library's folder and points the part there. A target without the
    /// part gets it copied over from `from` first.
    fn attach(&mut self, from: &LibraryRoot, targets: &[LibraryRoot], category: &str, name: &str, path: &Path, cx: &mut AppCtx) {
        let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        for target in targets {
            let nick = target.nickname(category);
            let result = (|| -> anyhow::Result<()> {
                let kind = AttachKind::of(path).ok_or_else(|| anyhow::anyhow!("don't know what kind of file {} is", path.display()))?;
                if !target.has_part(category, name) {
                    if from.copy_part_to(category, name, target, false)? {
                        cx.log.push(format!("Added library {nick} to KiCad"));
                    }
                    cx.log.push(format!("Copied {name} into {nick}"));
                }
                target.attach_file(category, name, kind, path)
            })();
            match result {
                Ok(()) => cx.log(Level::Ok, format!("Added {file} to {name} in {nick}")),
                Err(e) => cx.log(Level::Error, format!("Could not add {file} to {name} in {nick}: {e:#}")),
            }
        }
        self.stale = true;
    }

    /// Finds a PDF for every part in `root` without a working local datasheet, on a worker thread.
    fn find_datasheets(&mut self, root: LibraryRoot, cx: &mut AppCtx) {
        self.downloading = true;
        let (tx, ctx, sources) = (cx.tx.clone(), cx.ctx.clone(), cx.sources.clone());
        cx.log.push("Looking for datasheets for parts without a working one…".into());
        std::thread::spawn(move || {
            let results = root.find_missing_datasheets(&sources).into_iter().map(|(mpn, r)| (mpn, r.map_err(|e| format!("{e:#}")))).collect();
            let _ = tx.send(AppMsg::Library(Msg::DatasheetsDone(results)));
            ctx.request_repaint();
        });
    }

    fn edit(&mut self, scope: Scope, targets: &[(String, String)], edit: &PartEdit, cx: &mut AppCtx) {
        let root = if scope == Scope::Global { cx.global_root() } else { cx.project_root() };
        let Some(root) = root else { return };
        for (category, name) in targets {
            match root.edit_part_with(category, name, edit, &cx.settings.rules) {
                Ok(done) => {
                    let mut line = format!("Edited {name}");
                    if done.name != *name {
                        line += &format!(", renamed {}", done.name);
                    }
                    if done.category != *category {
                        line += &format!(", moved to {}", root.nickname(&done.category));
                    }
                    cx.log.push(line);
                    if done.new_lib {
                        cx.log.push(format!("Added library {} to KiCad", root.nickname(&done.category)));
                    }
                }
                Err(e) => cx.log.push(format!("Could not edit {name}: {e:#}")),
            }
        }
        self.selected.clear();
        self.stale = true;
    }

    fn copy_to_project(&mut self, category: &str, name: &str, overwrite: bool, cx: &mut AppCtx) {
        let (Some(global), Some(project)) = (cx.global_root(), cx.project_root()) else { return };
        match global.copy_part_to(category, name, &project, overwrite) {
            Ok(new_lib) => {
                cx.log.push(format!("Copied {name} into {}", project.nickname(category)));
                if new_lib {
                    cx.log.push(format!("Added library {} to KiCad", project.nickname(category)));
                }
            }
            Err(e) => cx.log.push(format!("Could not copy {name}: {e:#}")),
        }
        self.stale = true;
    }

    /// Starts reading `root` on two threads. Replacing the receivers abandons any load still running.
    fn start_load(&mut self, root: &LibraryRoot, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (r, c) = (root.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(r.list_parts());
            c.request_repaint();
        });
        let (ctx_tx, ctx_rx) = mpsc::channel();
        let (r, c) = (root.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = ctx_tx.send(checks::check_library(&r));
            c.request_repaint();
        });
        self.dir = Some(root.dir.clone());
        (self.parts_rx, self.checks, self.checks_rx) = (Some(rx), None, Some(ctx_rx));
        self.generation += 1;
    }

    /// Picks up what the workers have read.
    fn receive(&mut self) {
        if let Some(rx) = &self.parts_rx {
            match rx.try_recv() {
                Ok(parts) => {
                    (self.parts, self.parts_rx) = (Some(parts), None);
                    self.generation += 1;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                // The worker died; show an empty list rather than "Loading…" for ever.
                Err(_) => {
                    (self.parts, self.parts_rx) = (Some(Vec::new()), None);
                    self.generation += 1;
                }
            }
        }
        if let Some(rx) = &self.checks_rx {
            match rx.try_recv() {
                Ok(done) => {
                    (self.checks, self.checks_rx) = (Some(done), None);
                    self.generation += 1;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(_) => self.checks_rx = None,
            }
        }
    }

    /// Re-filters and re-sorts when something that decides the rows changed, and drops ticks on rows that are gone.
    fn update_view(&mut self) {
        let key = ViewKey { generation: self.generation, filter: self.filter.clone(), category: self.category.clone(), maker: self.maker.clone(), status: self.status, sort: self.sort };
        if self.view_key.as_ref() == Some(&key) {
            return;
        }
        self.view_key = Some(key);
        let Some(parts) = &self.parts else { return };
        let needle = self.filter.to_lowercase();
        let mut view: Vec<usize> = (0..parts.len())
            .filter(|&i| {
                let p = &parts[i];
                self.status.matches(p)
                    && (self.category.is_empty() || p.category == self.category)
                    && (self.maker.is_empty() || p.manufacturer == self.maker)
                    && (needle.is_empty() || [&p.mpn, &p.manufacturer, &p.category, &p.description].iter().any(|f| f.to_lowercase().contains(&needle)))
            })
            .collect();
        let issue_count = |p: &PartStatus| self.checks.as_ref().and_then(|c| c.get(&(p.category.clone(), p.name.clone()))).map_or(0, Vec::len);
        let Sort { col, asc } = self.sort;
        view.sort_by(|&a, &b| {
            let o = compare(col, &parts[a], &parts[b], issue_count);
            if asc { o } else { o.reverse() }
        });
        let shown: HashSet<Key> = view.iter().map(|&i| (parts[i].category.clone(), parts[i].name.clone())).collect();
        self.selected.retain(|k| shown.contains(k));
        if self.anchor.as_ref().is_some_and(|k| !shown.contains(k)) {
            self.anchor = None;
        }
        self.view = view;
    }

    fn key_at(&self, i: usize) -> Option<Key> {
        let p = &self.parts.as_ref()?[*self.view.get(i)?];
        Some((p.category.clone(), p.name.clone()))
    }

    /// Applies a click on row `i` of the view to the selection.
    fn click(&mut self, click: Click) {
        let (i, shift, command) = match click {
            Click::Tick(i, shift) => (i, shift, true),
            Click::Row(i, m) => (i, m.shift, m.command),
            Click::Menu(i) => {
                self.current = self.key_at(i);
                return;
            }
        };
        let Some(key) = self.key_at(i) else { return };
        let from = self.anchor.as_ref().and_then(|a| (0..self.view.len()).find(|&n| self.key_at(n).as_ref() == Some(a)));
        match from {
            Some(from) if shift => {
                self.selected = (from.min(i)..=from.max(i)).filter_map(|n| self.key_at(n)).collect();
                self.current = Some(key);
            }
            _ if command => {
                if !self.selected.remove(&key) {
                    self.selected.insert(key.clone());
                }
                self.current = Some(key.clone());
                self.anchor = Some(key);
            }
            _ => {
                self.current = Some(key.clone());
                self.anchor = Some(key);
            }
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> Action {
        let mut action = Action::None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("View");
            let before = self.scope;
            ui.add_enabled_ui(global.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Global, "Global"));
            ui.add_enabled_ui(project.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Project, "Project"));
            if self.scope != before {
                self.stale = true;
                self.current = None;
            }
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter by MPN, maker, category…").desired_width(200.0));
            let all = |v: &str, what: &str| if v.is_empty() { format!("Any {what}") } else { v.to_string() };
            let (cats, makers) = self.parts.as_ref().map_or_else(Default::default, |ps| {
                let set = |f: fn(&PartStatus) -> &String| ps.iter().map(f).filter(|s| !s.is_empty()).cloned().collect::<std::collections::BTreeSet<_>>();
                (set(|p| &p.category), set(|p| &p.manufacturer))
            });
            for (id, value, options, what) in [("cat_filter", &mut self.category, cats, "library"), ("maker_filter", &mut self.maker, makers, "manufacturer")] {
                egui::ComboBox::from_id_salt(id).selected_text(all(value, what)).show_ui(ui, |ui| {
                    ui.selectable_value(value, String::new(), format!("Any {what}"));
                    for o in options {
                        ui.selectable_value(value, o.clone(), o);
                    }
                });
            }
            let current = Filter::ALL.iter().find(|(s, _)| *s == self.status).map_or("", |(_, l)| l);
            egui::ComboBox::from_id_salt("status_filter").selected_text(current).show_ui(ui, |ui| {
                for (s, label) in Filter::ALL {
                    ui.selectable_value(&mut self.status, s, label);
                }
            });
            if ui.button("Refresh").clicked() {
                self.stale = true;
            }
        });

        let root = match self.scope {
            Scope::Global => global,
            Scope::Project => project,
        };
        let Some(root) = root else {
            (self.parts, self.parts_rx, self.checks_rx, self.dir) = (None, None, None, None);
            ui.label(match self.scope {
                Scope::Global => "Set up a global library first.",
                Scope::Project => "Choose a project first.",
            });
            return Action::None;
        };
        // A different library (another scope, or the project changed) shares nothing with what is shown.
        if self.dir.as_ref() != Some(&root.dir) {
            (self.parts, self.view_key) = (None, None);
            (self.selected, self.anchor, self.current) = Default::default();
            self.stale = true;
        }
        if self.stale {
            self.stale = false;
            self.start_load(root, ui.ctx());
        }
        self.receive();
        self.update_view();

        let has_selection = !self.selected.is_empty();
        ui.horizontal_wrapped(|ui| {
            ui.strong("Library actions");
            if ui.add_enabled(!self.view.is_empty(), egui::Button::new("Select all")).on_hover_text("Tick every part shown").clicked() {
                self.selected = (0..self.view.len()).filter_map(|i| self.key_at(i)).collect();
            }
            if ui.add_enabled(has_selection, egui::Button::new("Select none")).clicked() {
                self.selected.clear();
            }
            if ui.add_enabled(has_selection, egui::Button::new("Edit selected…")).on_disabled_hover_text("Tick parts first").clicked() {
                action = Action::Edit(sorted(&self.selected));
            }
            let can_copy = self.scope == Scope::Global && project.is_some() && has_selection;
            let copy = ui.add_enabled(can_copy, egui::Button::new("Copy selected to project"));
            if copy.on_hover_text("Copy the ticked parts into the project's library. Shift-click to replace copies already there.").clicked() {
                action = Action::Batch(vec![Op::CopyToProject { parts: sorted(&self.selected), overwrite: ui.input(|i| i.modifiers.shift) }]);
            }
            let attach = ui.add_enabled(has_selection, egui::Button::new("Attach to selected…"));
            if attach.on_hover_text("Add one datasheet (PDF), SPICE model or 3D model (STEP/WRL) to every ticked part").clicked() {
                if let Some(file) = pick_attachment() {
                    action = Action::Batch(vec![Op::Attach { scope: self.scope, file, parts: sorted(&self.selected) }]);
                }
            }
            let export = ui.add_enabled(has_selection, egui::Button::new("Export selected…"));
            if export.on_hover_text("Write the ticked parts, with their footprints and 3D models, to one ZIP").on_disabled_hover_text("Tick parts first").clicked() {
                action = Action::Export(root.clone());
            }
            if ui.button("Standardise…").on_hover_text("Fix symbols and footprints to the rules in Settings → Standards. Shows what changes first.").clicked() {
                action = Action::Standardise(root.clone());
            }
            let health = ui.add_enabled(project.is_some(), egui::Button::new("Health report…"));
            if health.on_hover_text("Everything wrong or untidy in the project's library").on_disabled_hover_text("Choose a project first").clicked() {
                action = Action::Health;
            }
            if ui.button("Alternatives…").on_hover_text("Find duplicates and equivalent parts, and mark preferred ones").clicked() {
                let one = (self.selected.len() == 1).then(|| self.selected.iter().next().cloned()).flatten();
                action = Action::Alternatives(one.or_else(|| self.current.clone()));
            }
            if ui.button("Relink libraries").on_hover_text("Add every KLM library missing from KiCad's library list").clicked() {
                action = Action::Batch(vec![Op::RelinkAll]);
            }
            let find = ui.add_enabled(!self.downloading, egui::Button::new("Find missing datasheets"));
            if find.on_hover_text("For parts with no working datasheet, try the existing link, then search the enabled sources by MPN, and save the first real PDF").clicked() {
                action = Action::FindDatasheets(root.clone());
            }
        });

        let Some(parts) = &self.parts else {
            ui.weak(format!("Loading parts from {}…", root.dir.display()));
            return action;
        };
        let loading = self.parts_rx.is_some() || self.checks_rx.is_some();
        let bad = parts.iter().filter(|p| incomplete(p)).count();
        let mut summary = format!("{} parts in {} · {bad} incomplete", parts.len(), root.dir.display());
        if self.view.len() != parts.len() {
            summary += &format!(" · {} shown", self.view.len());
        }
        if has_selection {
            summary += &format!(" · {} selected", self.selected.len());
        }
        if loading {
            summary += " · reading…";
        }
        ui.weak(summary);
        ui.add_space(4.0);

        let current = self.current.as_ref().and_then(|k| parts.iter().find(|p| (&p.category, &p.name) == (&k.0, &k.1)));
        egui::Panel::bottom("library_detail").resizable(false).show(ui, |ui| match current {
            Some(p) => {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&p.mpn);
                    ui.weak(format!("{} · {}", p.manufacturer, p.category));
                });
                ui.horizontal_wrapped(|ui| part_actions(ui, p, self.scope, project.is_some(), false, &mut action));
            }
            None => {
                ui.weak("Click a part to see its actions; right-click a row for the same. Ctrl-click or shift-click to tick several. Click a missing datasheet or 3D model to fix it.");
            }
        });

        // Narrow enough that Checks is still on screen at the 820 px default.
        let cols = [
            Col::new("", 24.0).no_sort(),
            Col::new("Category", 74.0),
            Col::new("MPN", 120.0),
            Col::new("Manufacturer", 104.0),
            Col::new("Footprint", 66.0),
            Col::new("3D model", 66.0),
            Col::new("Datasheet", 72.0),
            Col::new("Sim model", 66.0),
            Col::new("In KiCad", 64.0),
            Col::rest("Checks"),
        ];
        let (scope, has_project, downloading) = (self.scope, project.is_some(), self.downloading);
        let mut click = None;
        let (view, selected, checks, current) = (&self.view, &self.selected, &self.checks, &self.current);
        let mut sort = self.sort;
        ui.scope(|ui| {
            // A selectable label would swallow the click that selects the row.
            ui.style_mut().interaction.selectable_labels = false;
            ui_kit::table(ui, "library", &cols, &mut sort, view.len(), |i, row| {
                let p = &parts[view[i]];
                let key = (p.category.clone(), p.name.clone());
                row.set_selected(current.as_ref() == Some(&key));
                row.col(|ui| {
                    let mut ticked = selected.contains(&key);
                    let r = ui.checkbox(&mut ticked, "");
                    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, ticked, format!("Select {}", p.mpn)));
                    if r.clicked() {
                        click = Some(Click::Tick(i, ui.input(|i| i.modifiers.shift)));
                    }
                });
                row.col(|ui| {
                    ui.label(&p.category);
                });
                row.col(|ui| {
                    let r = ui.label(RichText::new(&p.mpn).strong());
                    if !p.description.is_empty() {
                        r.on_hover_text(&p.description);
                    }
                });
                row.col(|ui| {
                    ui.label(&p.manufacturer);
                });
                row.col(|ui| {
                    ui_kit::status_cell(ui, cell(p.footprint), "Footprint");
                });
                row.col(|ui| {
                    let fixable = matches!(p.model, Check::Missing | Check::Broken);
                    let r = ui_kit::status_cell(ui, cell(p.model), "3D model");
                    if fixable && r.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        if let Some(path) = pick_attachment() {
                            action = Action::Attach { scope, category: key.0.clone(), name: key.1.clone(), path };
                        }
                    }
                });
                row.col(|ui| {
                    let fixable = matches!(p.datasheet, Check::Missing | Check::Broken);
                    let r = ui_kit::status_cell(ui, cell(p.datasheet), "Datasheet");
                    if fixable && !downloading && r.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        action = Action::FindDatasheets(root.clone());
                    }
                });
                row.col(|ui| {
                    ui_kit::status_cell(ui, if p.sim == Check::Missing { Cell::Optional } else { cell(p.sim) }, "Sim model");
                });
                row.col(|ui| {
                    ui_kit::status_cell(ui, if p.in_kicad { Cell::Ok } else { Cell::Missing }, "KiCad library entry");
                });
                row.col(|ui| issues_cell(ui, checks.as_ref().map(|c| c.get(&key).unwrap_or(&NO_ISSUES))));
                let r = row.response();
                if r.clicked() {
                    click = Some(Click::Row(i, r.ctx.input(|i| i.modifiers)));
                } else if r.secondary_clicked() {
                    click = Some(Click::Menu(i));
                }
                r.context_menu(|ui| part_actions(ui, p, scope, has_project, true, &mut action));
            });
        });
        self.sort = sort;
        if let Some(c) = click {
            self.click(c);
        }
        action
    }
}

enum AttachAction {
    None,
    Cancel,
    Attach { targets: Vec<LibraryRoot> },
}

/// Asks which libraries a picked datasheet, sim or 3D model file goes into.
pub struct AttachDialog {
    /// The library the part was listed in, which holds the part.
    from: Scope,
    category: String,
    name: String,
    path: PathBuf,
    to_global: bool,
    to_project: bool,
}

impl AttachDialog {
    pub fn new(from: Scope, category: String, name: String, path: PathBuf) -> Self {
        Self { from, category, name, path, to_global: from == Scope::Global, to_project: from == Scope::Project }
    }

    fn show(&mut self, ctx: &egui::Context, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> AttachAction {
        let mut action = AttachAction::None;
        let modal = egui::Modal::new(egui::Id::unique("attach_dialog")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Add file");
            ui.label(format!("{} → {}", self.path.file_name().unwrap_or_default().to_string_lossy(), self.name));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Add to");
                ui.add_enabled(global.is_some(), egui::Checkbox::new(&mut self.to_global, "Global"));
                ui.add_enabled(project.is_some(), egui::Checkbox::new(&mut self.to_project, "Project"));
            });
            ui.weak("The file is copied into each library's folder. A library without this part gets a copy of it first.");

            let targets: Vec<LibraryRoot> = [(self.to_global, global), (self.to_project, project)]
                .into_iter()
                .filter_map(|(on, r)| if on { r.cloned() } else { None })
                .collect();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!targets.is_empty(), egui::Button::new("Add")).clicked() {
                    action = AttachAction::Attach { targets };
                }
                if ui.button("Cancel").clicked() {
                    action = AttachAction::Cancel;
                }
            });
        });
        if modal.should_close() && matches!(action, AttachAction::None) {
            action = AttachAction::Cancel;
        }
        action
    }
}
