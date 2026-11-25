use super::alternatives::AlternativesPanel;
use super::batch_dialog::{self, BatchDialog, Progress};
use super::edit_dialog::{EditAction, EditDialog};
use super::preview::PreviewPanel;
use super::symbol_editor::{Outcome, SymbolEditor};
use super::ui_kit::{self, Col, Level, Sort, Status as Cell};
use crate::{AppCtx, Msg as AppMsg, theme};
use eframe::egui::{self, RichText};
use klm_core::batch::{self, Op};
use klm_core::checks::{self, Issue, Severity};
use klm_core::compliance;
use klm_core::health::{self, Finding};
use klm_core::kicad_edit::{self, Editor, FileWatch};
use klm_core::library::{ATTACH_EXTENSIONS, AttachKind, Check, LibraryRoot, PartEdit, PartStatus, RemovePlan, Scope};
use klm_core::standardise;
use klm_core::store::{Alternative, Store};
use klm_core::symbols::Restyled;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

type Key = (String, String);
type Checks = HashMap<Key, Vec<Issue>>;
type Compliances = HashMap<Key, compliance::Compliance>;

pub enum Msg {
    /// Re-read the listing, e.g. after an import.
    Refresh,
    /// Filter the list to this MPN, so another tab can land on the part.
    Show(String),
    /// Open Alternatives on this symbol, filtered to what JLCPCB stocks in the same package (the BOM's Swap).
    Swap(String),
    /// Apply `edit` to each (category, name) in the `scope` library.
    Edit {
        scope: Scope,
        targets: Vec<(String, String)>,
        edit: PartEdit,
    },
    /// Copy a global part into the project library; `overwrite` replaces an existing copy.
    CopyToProject {
        category: String,
        name: String,
        overwrite: bool,
    },
    Attach {
        from: LibraryRoot,
        targets: Vec<LibraryRoot>,
        category: String,
        name: String,
        path: PathBuf,
    },
    /// The datasheet finder has finished: (part, URL used or why not).
    DatasheetsDone(Vec<(String, Result<String, String>)>),
    /// SPICE close-match / synthesis finished: (part, summary or error).
    SimsDone(Vec<(String, Result<String, String>)>),
    /// Apply a batch plan (KiCad is closed, or the plan doesn't touch its tables).
    Batch(Progress),
    /// A reimport finished: the part and the sources that supplied files, or why not.
    Reimported(String, Result<Vec<String>, String>),
}

/// Something the user asked for on a row; `ui` carries it out and refreshes the view.
enum Action {
    None,
    /// Open the edit dialog for these (category, name) parts.
    Edit(Vec<(String, String)>),
    Attach {
        scope: Scope,
        category: String,
        name: String,
        path: PathBuf,
    },
    Preview {
        category: String,
        name: String,
    },
    /// Open the part's symbol library or footprint in KiCad's editor and re-check when it is saved.
    EditInKicad {
        editor: Editor,
        category: String,
        name: String,
    },
    /// Open the in-app symbol editor.
    EditSymbol {
        category: String,
        name: String,
    },
    /// Show what redrawing the symbol in the house style of its class would do.
    Restyle {
        category: String,
        name: String,
    },
    /// Make the part the active profile's house template for its class.
    HouseTemplate {
        category: String,
        name: String,
    },
    /// Ask before fetching the part's files from the sources again.
    Reimport(Key),
    CopyToProject {
        category: String,
        name: String,
        overwrite: bool,
    },
    FindDatasheets(LibraryRoot),
    FindSims {
        root: LibraryRoot,
        targets: Option<Vec<Key>>,
    },
    /// Show the dry run of these operations.
    Batch(Vec<Op>),
    Alternatives(Option<Key>),
    /// `None` is the whole library, `Some` only those parts (and the footprints they use).
    Standardise(LibraryRoot, Option<Vec<Key>>),
    Export(LibraryRoot),
    /// Show what deleting these parts would remove and keep.
    Delete(LibraryRoot, Vec<Key>),
    Health,
}

/// A dry run waiting for the user's answer, or the worker that is producing it.
enum Dry {
    Planning(mpsc::Receiver<standardise::Plan>),
    Standardise(standardise::Plan),
    Restyle {
        root: LibraryRoot,
        category: String,
        name: String,
        result: Result<Restyled, String>,
    },
    Export {
        root: LibraryRoot,
        parts: Vec<Key>,
        path: String,
    },
    Delete {
        root: LibraryRoot,
        plans: Vec<(Key, Result<RemovePlan, String>)>,
    },
    /// The worker planning a delete.
    PlanningDelete(LibraryRoot, mpsc::Receiver<Vec<(Key, Result<RemovePlan, String>)>>),
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
    /// The row the inspector shows.
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
    /// Compliance for `parts` (Standardise plan plus checks); `None` while it is being worked out. The cache is
    /// shared with the worker, so an unchanged library isn't re-read on every reload.
    compliance: Option<Compliances>,
    compliance_rx: Option<mpsc::Receiver<Compliances>>,
    compliance_cache: std::sync::Arc<std::sync::Mutex<compliance::Cache>>,
    /// Files open in KiCad; a save reloads (the compliance cache is keyed on mtimes, so it re-runs by itself).
    kicad_edits: Vec<FileWatch>,
    /// Library folder, when its files were last fingerprinted, and the fingerprint.
    /// The part waiting for the user to confirm its reimport.
    reimport: Option<Key>,
    lib_watch: Option<(PathBuf, std::time::Instant, (usize, Option<std::time::SystemTime>))>,
    /// The profile and naming rules compliance is judged by; a change reloads.
    standard: (standardise::profile::Profile, klm_core::rules::Rules),
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
    symbol_editor: Option<SymbolEditor>,
    alternatives: Option<AlternativesPanel>,
    /// The inspector's view of `current`.
    inspect: Option<Inspect>,
    /// Counts reloads, so the inspector re-reads when the files under it change.
    loads: u64,
}

impl Default for LibraryTab {
    fn default() -> Self {
        Self {
            scope: Scope::Global,
            filter: String::new(),
            category: String::new(),
            maker: String::new(),
            status: Filter::All,
            sort: Sort { col: CATEGORY, asc: true },
            current: None,
            finder_results: None,
            parts: None,
            dir: None,
            parts_rx: None,
            checks: None,
            checks_rx: None,
            compliance: None,
            compliance_rx: None,
            compliance_cache: Default::default(),
            kicad_edits: vec![],
            reimport: None,
            lib_watch: None,
            standard: Default::default(),
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
            symbol_editor: None,
            alternatives: None,
            inspect: None,
            loads: 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Incomplete,
    NoDatasheet,
    NoModel,
    NonCompliant,
}

impl Filter {
    const ALL: [(Filter, &'static str); 5] = [
        (Filter::All, "Any status"),
        (Filter::Incomplete, "Incomplete"),
        (Filter::NoDatasheet, "No datasheet"),
        (Filter::NoModel, "No 3D model"),
        (Filter::NonCompliant, "Non-compliant"),
    ];

    fn matches(self, p: &PartStatus) -> bool {
        let no = |c: Check| matches!(c, Check::Missing | Check::Broken);
        match self {
            Filter::All => true,
            Filter::Incomplete => incomplete(p),
            Filter::NoDatasheet => no(p.datasheet),
            Filter::NoModel => no(p.model),
            Filter::NonCompliant => false, // decided in `update_view`, which has the results
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

/// Column ids; the table shows a subset of them, chosen by its width.
const TICK: usize = 0;
const MPN: usize = 1;
const MAKER: usize = 2;
const CATEGORY: usize = 3;
const FILES: usize = 4;
const KICAD: usize = 5;
const CHECKS: usize = 6;
const STANDARD: usize = 7;

/// Worst-first rank, so ascending puts the parts with problems on top.
fn rank(c: Check) -> u8 {
    match c {
        Check::Broken => 0,
        Check::Missing => 1,
        Check::Link => 2,
        Check::Ok => 3,
    }
}

/// The worst of a part's files (a missing sim model is fine).
fn files_rank(p: &PartStatus) -> u8 {
    [rank(p.footprint), rank(p.model), rank(p.datasheet), if p.sim == Check::Broken { 0 } else { 3 }].into_iter().min().unwrap_or(3)
}

fn compare(col: usize, a: &PartStatus, b: &PartStatus, issues: impl Fn(&PartStatus) -> usize) -> std::cmp::Ordering {
    match col {
        CATEGORY => a.category.to_lowercase().cmp(&b.category.to_lowercase()),
        MPN => a.mpn.to_lowercase().cmp(&b.mpn.to_lowercase()),
        MAKER => a.manufacturer.to_lowercase().cmp(&b.manufacturer.to_lowercase()),
        FILES => files_rank(a).cmp(&files_rank(b)),
        KICAD => a.in_kicad.cmp(&b.in_kicad),
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

/// What the inspector keeps about the current part, read when it becomes current and again after every reload.
struct Inspect {
    key: Key,
    /// `LibraryTab::loads` when it was read.
    load: u64,
    preview: PreviewPanel,
    fields: Vec<(&'static str, String)>,
    alternatives: Vec<Alternative>,
}

fn small(text: &str) -> egui::Button<'_> {
    egui::Button::new(text).small()
}

/// An inspector section: padded, with a rule under it.
fn section(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 8.0;
        add(ui);
    });
    ui.add(egui::Separator::default().spacing(0.0));
}

/// One SYM / FP / 3D / DS chip. A missing or broken footprint, 3D model or datasheet can be clicked to fix it:
/// the first two ask for a file, the datasheet starts the finder.
fn file_chips(ui: &mut egui::Ui, p: &PartStatus, scope: Scope, downloading: bool, root: &LibraryRoot, action: &mut Action, with_sim: bool) {
    ui.spacing_mut().item_spacing.x = 4.0;
    let chip = |ui: &mut egui::Ui, short: &str, what: &str, status: Cell| {
        let (state, tip) = match status {
            Cell::Ok => ("present", format!("{what} present")),
            Cell::Link => ("link", format!("{what} points at a web page or another library")),
            Cell::Missing => ("missing", format!("No {what}. Click to fix it.")),
            Cell::Broken => ("broken", format!("{what} is set but its file isn't there. Click to fix it.")),
            Cell::Optional => ("optional", format!("No {what} (optional)")),
        };
        ui_kit::file_chip(ui, short, status, &tip, &format!("{what}: {state}"))
    };
    let fixable = |c: Check| matches!(c, Check::Missing | Check::Broken);
    let key = (p.category.clone(), p.name.clone());
    chip(ui, "SYM", "Symbol", Cell::Ok);
    let r = chip(ui, "FP", "Footprint", cell(p.footprint));
    if fixable(p.footprint) && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        if let Some(path) = pick_attachment() {
            *action = Action::Attach { scope, category: key.0.clone(), name: key.1.clone(), path };
        }
    }
    let r = chip(ui, "3D", "3D model", cell(p.model));
    if fixable(p.model) && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        if let Some(path) = pick_attachment() {
            *action = Action::Attach { scope, category: key.0.clone(), name: key.1.clone(), path };
        }
    }
    let r = chip(ui, "DS", "Datasheet", cell(p.datasheet));
    if fixable(p.datasheet) && !downloading && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *action = Action::FindDatasheets(root.clone());
    }
    // Sim models are optional: the inspector always lists the slot, the table only when there is something to say.
    if with_sim || p.sim != Check::Missing {
        let r = chip(ui, "SIM", "Sim model", if p.sim == Check::Missing { Cell::Optional } else { cell(p.sim) });
        if p.sim != Check::Ok
            && !downloading
            && r.on_hover_text("Click to find a close SPICE model or generate one from datasheet & similar models")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        {
            *action = Action::FindSims { root: root.clone(), targets: Some(vec![(p.category.clone(), p.name.clone())]) };
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
        ui.weak("—").on_hover_text("No problems found");
        return;
    };
    let (word, level) = if worst == Severity::Error { ("error", Level::Error) } else { ("warning", Level::Warn) };
    let n = issues.len();
    let tip: Vec<_> = issues.iter().map(|i| format!("• {}", i.message)).collect();
    ui.label(RichText::new(format!("{n} {word}{}", if n == 1 { "" } else { "s" })).color(level.color(ui.visuals()))).on_hover_text(tip.join("\n"));
}

static NO_ISSUES: Vec<Issue> = Vec::new();

/// OK / No / … for the house standard, with the reasons on hover.
fn compliance_cell(ui: &mut egui::Ui, c: Option<&compliance::Compliance>) {
    let r = match c {
        None => ui.weak("…").on_hover_text("Working out whether the part follows the house standard"),
        Some(c) if c.ok => {
            ui.label(RichText::new("OK").color(ui.visuals().text_color())).on_hover_text("Standardise has nothing to change and the checks pass")
        }
        Some(c) => {
            let tip: Vec<_> = c.reasons.iter().map(|r| format!("• {r}")).collect();
            ui.label(RichText::new("No").color(ui.visuals().warn_fg_color)).on_hover_text(tip.join("\n"))
        }
    };
    let text = match c {
        None => "Compliance: computing",
        Some(c) if c.ok => "Compliant",
        Some(_) => "Not compliant",
    };
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
}

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
        if self.standard.0 != cx.settings.standard || self.standard.1 != cx.settings.rules {
            self.standard = (cx.settings.standard.clone(), cx.settings.rules.clone());
            self.stale = true;
        }
        if !self.kicad_edits.is_empty() {
            self.stale |= self.kicad_edits.iter_mut().fold(false, |a, w| w.changed() | a);
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
        // Anything saved in KiCad (or by hand) comes back: poll the shown library's files every couple of seconds.
        if let Some(root) = if self.scope == Scope::Global { &global } else { &project } {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(2));
            if self.lib_watch.as_ref().is_none_or(|(_, t, _)| t.elapsed() > std::time::Duration::from_secs(2)) {
                let sig = root.signature();
                if self.lib_watch.as_ref().is_some_and(|(d, _, s)| *d == root.dir && *s != sig) {
                    self.stale = true;
                }
                self.lib_watch = Some((root.dir.clone(), std::time::Instant::now(), sig));
            }
        }
        match self.show(ui, global.as_ref(), project.as_ref()) {
            Action::None => {}
            Action::EditInKicad { editor, category, name } => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    match kicad_edit::target(root, editor, &category, &name).and_then(|f| kicad_edit::launch(editor, &f).map(|_| f)) {
                        Ok(f) => {
                            cx.log(Level::Info, format!("Opened {} in KiCad; saving there re-checks {name}", f.display()));
                            if !self.kicad_edits.iter().any(|w| w.path == f) {
                                self.kicad_edits.push(FileWatch::new(f));
                            }
                        }
                        Err(e) => cx.log(Level::Error, format!("Could not open {name} in KiCad: {e:#}")),
                    }
                }
            }
            Action::FindDatasheets(root) => self.find_datasheets(root, cx),
            Action::FindSims { root, targets } => self.find_sims(root, targets, cx),
            Action::Batch(ops) => self.batch = Some(BatchDialog::Review(batch::plan(ops, global.as_ref(), project.as_ref()))),
            Action::Alternatives(open_on) => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    self.alternatives = Some(AlternativesPanel::new(root, &cx.ctx, open_on));
                }
            }
            Action::Standardise(root, only) => {
                let (tx, rx) = mpsc::channel();
                let (profile, naming, ctx) = (cx.settings.standard.clone(), cx.settings.rules.clone(), cx.ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(standardise::plan_for(&root, &profile, &naming, only.as_deref()));
                    ctx.request_repaint();
                });
                self.dry = Some(Dry::Planning(rx));
            }
            Action::Export(root) => {
                // A sandboxed run (tests, --config) never writes outside its sandbox.
                let path = klm_core::settings::sandbox_dir().or_else(dirs::document_dir).or_else(dirs::home_dir).unwrap_or_default().join("klm_export.zip");
                self.dry = Some(Dry::Export { root, parts: sorted(&self.selected), path: path.display().to_string() });
            }
            Action::Delete(root, targets) => self.start_delete(root, targets, &cx.ctx),
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
                    self.edit_dialog = Some(EditDialog::new(&cx.ctx, self.scope, targets, root, cx.settings.rules.clone()));
                }
            }
            Action::Preview { category, name } => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    self.preview = Some((format!("Preview: {name}"), PreviewPanel::for_library(&cx.ctx, root, &category, &name)));
                }
            }
            Action::EditSymbol { category, name } => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    match SymbolEditor::new(&cx.ctx, root, &category, &name) {
                        Ok(e) => self.symbol_editor = Some(e),
                        Err(e) => cx.log(Level::Error, format!("Can't edit {name}: {e:#}")),
                    }
                }
            }
            Action::Restyle { category, name } => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    let result =
                        klm_core::symbols::restyle_part(root, &category, &name, &cx.settings.rules, &cx.settings.standard, false).map_err(|e| format!("{e:#}"));
                    self.dry = Some(Dry::Restyle { root: root.clone(), category, name, result });
                }
            }
            Action::HouseTemplate { category, name } => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    let naming = cx.settings.rules.clone();
                    match klm_core::symbols::set_house_template(&mut cx.settings.standard, root, &category, &name, &naming)
                        .and_then(|r| cx.settings.save().map(|_| r))
                    {
                        Ok((class, footprint)) => {
                            klm_core::symbols::configure(&cx.settings);
                            let fp = if footprint { " and its footprint style" } else { " (no footprint style: its footprint isn't in this library)" };
                            cx.log(Level::Info, format!("{name}{fp} is now the {class:?} house template of profile {}", cx.settings.profile_name()));
                        }
                        Err(e) => cx.log(Level::Error, format!("{e:#}")),
                    }
                }
            }
            Action::Reimport(key) => self.reimport = Some(key),
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

    /// Plans the delete on a worker; the review dialog follows.
    fn start_delete(&mut self, root: LibraryRoot, targets: Vec<Key>, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (r, ctx) = (root.clone(), ctx.clone());
        std::thread::spawn(move || {
            let plans = r.plan_remove_all(&targets);
            let _ = tx.send(targets.into_iter().zip(plans).map(|(k, p)| (k, p.map_err(|e| format!("{e:#}")))).collect());
            ctx.request_repaint();
        });
        self.dry = Some(Dry::PlanningDelete(root, rx));
    }

    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        match msg {
            Msg::Refresh => self.stale = true,
            Msg::Swap(symbol) => {
                // The BOM matches against both libraries, so open the one that has the symbol.
                // shortcut: scans on the UI thread, one click; move to the panel's worker if large libraries stall it.
                let has = |r: &LibraryRoot| klm_core::alternatives::scan(r).iter().any(|p| p.symbol == symbol);
                if let Some(root) = [cx.project_root(), cx.global_root()].into_iter().flatten().find(has) {
                    self.alternatives = Some(AlternativesPanel::new(&root, &cx.ctx, Some((String::new(), symbol))).for_swap());
                }
            }
            Msg::Show(mpn) => (self.filter, self.category, self.maker, self.status) = (mpn, String::new(), String::new(), Filter::All),
            Msg::DatasheetsDone(results) => {
                self.downloading = false;
                self.stale = true;
                self.finder_results = Some(results);
            }
            Msg::SimsDone(results) => {
                self.downloading = false;
                self.stale = true;
                self.finder_results = Some(results);
            }
            Msg::Batch(p) => {
                if batch_dialog::progress(&mut self.batch, p, cx).is_some() {
                    self.stale = true;
                }
            }
            Msg::Reimported(name, result) => {
                cx.busy = false;
                self.stale = true;
                match result {
                    Ok(used) => cx.log(Level::Info, format!("Reimported {name} from {}", used.join(", "))),
                    Err(e) => cx.log(Level::Error, format!("Could not reimport {name}: {e}")),
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
        if let Some((category, name)) = self.reimport.clone() {
            let mut choice = None;
            egui::Window::new("Reimport part").collapsible(false).resizable(false).show(&cx.ctx, |ui| {
                ui.label(format!("Fetch {name} from the enabled sources again and replace its symbol, footprint, 3D model and datasheet?"));
                ui.weak("Edits you made to this part are replaced. Undo it in History.");
                ui.horizontal(|ui| {
                    if ui.add_enabled(!cx.busy, egui::Button::new("Reimport")).clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(false);
                    }
                });
            });
            if let Some(go) = choice {
                self.reimport = None;
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let (true, Some(root)) = (go, root) {
                    cx.busy = true;
                    cx.log(Level::Info, format!("Reimporting {name}…"));
                    let (root, sources, tx, ctx, work) = (root.clone(), cx.sources.clone(), cx.tx.clone(), cx.ctx.clone(), crate::new_work_dir());
                    std::thread::spawn(move || {
                        let result = root.reimport(&category, &name, &sources, &work).map_err(|e| format!("{e:#}"));
                        let _ = std::fs::remove_dir_all(&work);
                        let _ = tx.send(AppMsg::Library(Msg::Reimported(name, result)));
                        ctx.request_repaint();
                    });
                }
            }
        }
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
        if let Some(editor) = &mut self.symbol_editor {
            match editor.show(&cx.ctx, &cx.settings.rules, &cx.settings.standard) {
                Outcome::None => {}
                Outcome::Close => self.symbol_editor = None,
                Outcome::Saved => self.stale = true,
            }
        }
        if let Some(dialog) = &mut self.edit_dialog {
            match dialog.show(&cx.ctx) {
                EditAction::None => {}
                EditAction::Cancel => self.edit_dialog = None,
                EditAction::Delete { targets } => {
                    self.edit_dialog = None;
                    let root = if self.scope == Scope::Global { &global } else { &project };
                    if let Some(root) = root {
                        self.start_delete(root.clone(), targets, &cx.ctx);
                    }
                }
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
            Some(Dry::PlanningDelete(root, rx)) => match rx.try_recv() {
                Ok(plans) => self.dry = Some(Dry::Delete { root: root.clone(), plans }),
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
        let heading = match self.dry {
            Some(Dry::Planning(_)) => ("Standardise", "Checking every symbol and footprint…"),
            Some(Dry::PlanningDelete(..)) => ("Delete", "Checking what each part uses…"),
            _ => ("Health report", "Checking the project's library…"),
        };
        match &mut self.dry {
            None => {}
            Some(Dry::Planning(_)) | Some(Dry::PlanningDelete(..)) | Some(Dry::Scanning(_)) => {
                let m = egui::Modal::new(egui::Id::NULL.with("dry_working")).show(&ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading(heading.0);
                    ui.label(heading.1);
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
                let note = format!(
                    "{}
The rules are in Settings → Standards. A snapshot is taken first, so History can roll it back.",
                    plan.root.dir.display()
                );
                match batch_dialog::review(&ctx, "dry_standardise", "Apply", false, "Standardise", &sections, !plan.files.is_empty(), |ui| {
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
                let note =
                    "Keeps every field. A snapshot is taken first, so History can undo it. Wires on schematics that already use the symbol may need fixing.";
                match batch_dialog::review(&ctx, "dry_restyle", "Apply", false, "Restyle symbol", &sections, can_apply, |ui| {
                    ui.weak(note);
                }) {
                    Some(true) if cx.busy => cx.log(Level::Warn, "Restyle: wait for the running import or download to finish, then press Apply again"),
                    Some(true) => {
                        let line = match klm_core::symbols::restyle_part(root, category, name, &cx.settings.rules, &cx.settings.standard, true) {
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
                let answer = batch_dialog::review(&ctx, "dry_export", "Apply", false, &title, &sections, can, |ui| {
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
            Some(Dry::Delete { root, plans }) => {
                let sections: Vec<_> = plans
                    .iter()
                    .map(|((c, n), plan)| (format!("{c} / {n}"), plan.as_ref().map_or_else(|e| vec![format!("can't be deleted: {e}")], |p| p.lines())))
                    .collect();
                let can = !plans.is_empty() && plans.iter().all(|(_, p)| p.is_ok());
                let title = format!("Delete {} part(s)?", plans.len());
                let confirm = format!("Delete {} part(s)", plans.len());
                let note = "A snapshot is taken first, so History can roll it back. A footprint, 3D model, datasheet or sim model another part still uses is kept; it goes with the last part that uses it.";
                match batch_dialog::review(&ctx, "dry_delete", &confirm, true, &title, &sections, can, |ui| {
                    ui.add_space(6.0);
                    ui.colored_label(ui.visuals().warn_fg_color, "Deleted parts are removed from the library, KLM's database and alternatives.");
                    ui.weak(note);
                }) {
                    Some(true) if cx.busy => cx.log(Level::Warn, "Delete: wait for the running import or download to finish, then press Delete again"),
                    Some(true) => {
                        let (root, keys): (LibraryRoot, Vec<Key>) = (root.clone(), plans.iter().map(|(k, _)| k.clone()).collect());
                        self.dry = None;
                        let store = klm_core::store::Store::default_path().and_then(|p| klm_core::store::Store::open(&p).ok());
                        let _snapshot = match klm_core::snapshot::guard(&root, &format!("delete {} part(s)", keys.len())) {
                            Ok(g) => g,
                            Err(e) => return cx.log(Level::Error, format!("Nothing deleted: the snapshot could not be taken: {e:#}")),
                        };
                        for (c, n) in keys {
                            // Plan again: deleting the part before this one may have freed a file they shared.
                            let done = root.plan_remove(&c, &n).and_then(|plan| {
                                root.remove_part(&plan)?;
                                store.as_ref().map_or(Ok(()), |s| s.forget_part(&root, &c, &n, &plan.mpn))
                            });
                            match done {
                                Ok(()) => cx.log(Level::Ok, format!("Deleted {n} from {c}")),
                                Err(e) => cx.log(Level::Error, format!("Could not delete {n}: {e:#}")),
                            }
                            self.selected.remove(&(c.clone(), n.clone()));
                            if self.current.as_ref() == Some(&(c, n)) {
                                self.current = None;
                            }
                        }
                        self.stale = true;
                    }
                    Some(false) => self.dry = None,
                    None => {}
                }
            }
            Some(Dry::Health(findings)) => {
                let (mut copy, mut close) = (false, false);
                let sub = if findings.is_empty() { "no problems found".to_string() } else { format!("{} finding(s)", findings.len()) };
                let dismissed = batch_dialog::dialog(
                    &ctx,
                    egui::Id::NULL.with("dry_health"),
                    700.0,
                    "Health report",
                    Some(sub.as_str()),
                    |ui| {
                        if findings.is_empty() {
                            ui.label("No problems found in the project's library.");
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
                    },
                    |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close = ui.button("Close").clicked();
                            copy = ui.button("Copy as CSV").clicked();
                        });
                    },
                );
                if copy {
                    ctx.copy_text(health::to_csv(findings));
                }
                if close || dismissed {
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

    /// Finds close SPICE matches or generates SPICE models from datasheets & similar parts on a worker thread.
    fn find_sims(&mut self, root: LibraryRoot, targets: Option<Vec<Key>>, cx: &mut AppCtx) {
        self.downloading = true;
        let (tx, ctx, sim_cfg) = (cx.tx.clone(), cx.ctx.clone(), cx.settings.sim.clone());
        cx.log.push("Looking for close SPICE models or generating from datasheets…".into());
        std::thread::spawn(move || {
            let todo: Vec<(String, String)> = match targets {
                Some(t) => t,
                None => root.list_parts().into_iter().filter(|p| p.sim != Check::Ok).map(|p| (p.category, p.name)).collect(),
            };
            let mut results = Vec::with_capacity(todo.len());
            for (cat, name) in todo {
                let res = match root.resolve_and_attach_sim(&cat, &name, &sim_cfg.spice_dirs, sim_cfg.auto_generate_from_datasheet) {
                    Ok(Some(r)) => Ok(r.origin.summary()),
                    Ok(None) => Err("no matching SPICE model or datasheet specs".into()),
                    Err(e) => Err(format!("{e:#}")),
                };
                results.push((name, res));
            }
            let _ = tx.send(AppMsg::Library(Msg::SimsDone(results)));
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
        self.loads += 1;
        let (comp_tx, comp_rx) = mpsc::channel();
        let (r, c, cache, (profile, naming)) = (root.clone(), ctx.clone(), self.compliance_cache.clone(), self.standard.clone());
        std::thread::spawn(move || {
            let done = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&r, &profile, &naming).clone();
            let _ = comp_tx.send(done);
            c.request_repaint();
        });
        (self.parts_rx, self.checks, self.checks_rx) = (Some(rx), None, Some(ctx_rx));
        (self.compliance, self.compliance_rx) = (None, Some(comp_rx));
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
        if let Some(rx) = &self.compliance_rx {
            match rx.try_recv() {
                Ok(done) => {
                    (self.compliance, self.compliance_rx) = (Some(done), None);
                    self.generation += 1;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(_) => self.compliance_rx = None,
            }
        }
    }

    /// Re-filters and re-sorts when something that decides the rows changed, and drops ticks on rows that are gone.
    fn update_view(&mut self) {
        let key = ViewKey {
            generation: self.generation,
            filter: self.filter.clone(),
            category: self.category.clone(),
            maker: self.maker.clone(),
            status: self.status,
            sort: self.sort,
        };
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
                    && (self.status != Filter::NonCompliant
                        || self.compliance.as_ref().and_then(|c| c.get(&(p.category.clone(), p.name.clone()))).is_some_and(|c| !c.ok))
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

    fn current_part(&self) -> Option<&PartStatus> {
        let k = self.current.as_ref()?;
        self.parts.as_ref()?.iter().find(|p| (&p.category, &p.name) == (&k.0, &k.1))
    }

    /// Keeps the inspector's pictures, fields and alternatives for the current part; rebuilt when the part or the listing changes.
    fn sync_inspector(&mut self, ctx: &egui::Context, root: &LibraryRoot) {
        let Some(p) = self.current_part() else {
            self.inspect = None;
            return;
        };
        let (key, mpn) = ((p.category.clone(), p.name.clone()), p.mpn.clone());
        if self.inspect.as_ref().is_some_and(|i| i.key == key && i.load == self.loads) {
            return;
        }
        let fields = root.part_fields(&key.0, &key.1).unwrap_or_default();
        let alternatives =
            Store::default_path().and_then(|p| Store::open(&p).ok()).and_then(|s| klm_core::alternatives::alternatives_of(&s, &mpn).ok()).unwrap_or_default();
        self.inspect = Some(Inspect { preview: PreviewPanel::for_library(ctx, root, &key.0, &key.1), key, load: self.loads, fields, alternatives });
    }

    fn show(&mut self, ui: &mut egui::Ui, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> Action {
        let mut action = Action::None;
        let root = match self.scope {
            Scope::Global => global,
            Scope::Project => project,
        };
        let Some(root) = root else {
            (self.parts, self.parts_rx, self.checks_rx, self.compliance_rx, self.dir, self.inspect) = (None, None, None, None, None, None);
            self.header(ui, global, project, None, &mut action);
            ui.label(match self.scope {
                Scope::Global => "Set up a global library first.",
                Scope::Project => "Choose a project first.",
            });
            return action;
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
        self.sync_inspector(ui.ctx(), root);

        // The inspector goes first so it takes the full height beside the header and the table.
        if let Some(p) = self.current_part().cloned() {
            let pal = theme::palette(ui.visuals());
            let width = if ui.available_width() < 900.0 { 300.0 } else { 352.0 };
            egui::Panel::right("library_inspector")
                .resizable(true)
                .default_size(width)
                .size_range(260.0..=520.0)
                .frame(egui::Frame::new().fill(pal.panel))
                .show(ui, |ui| {
                    self.inspector(ui, &p, root, project.is_some(), &mut action);
                });
        }
        self.header(ui, global, project, Some(root), &mut action);

        if self.parts.is_none() {
            ui.weak(format!("Loading parts from {}…", root.dir.display()));
            return action;
        }
        if !self.selected.is_empty() {
            self.selection_bar(ui, project.is_some(), root, &mut action);
        }
        let Some(parts) = &self.parts else { return action };
        if parts.is_empty() {
            ui.add_space(8.0);
            ui.weak("No parts yet. Import one from Search or Import.");
            return action;
        }

        // The table's width decides how many columns fit; the sort is kept as a column id, not a position.
        let wide = ui.available_width();
        let shown: &[usize] = if wide >= 800.0 {
            &[TICK, MPN, MAKER, CATEGORY, FILES, KICAD, CHECKS, STANDARD]
        } else if wide >= 500.0 {
            &[TICK, MPN, MAKER, FILES, CHECKS]
        } else {
            &[TICK, MPN, FILES, CHECKS]
        };
        let cols: Vec<Col> = shown
            .iter()
            .map(|&c| match c {
                TICK => Col::new("", 28.0).no_sort(),
                MPN if shown.contains(&STANDARD) => Col::new("MPN", 140.0),
                MPN => Col::rest("MPN"),
                MAKER => Col::new("Manufacturer", 110.0),
                CATEGORY => Col::new("Category", 80.0),
                FILES => Col::new("Files", 168.0),
                KICAD => Col::new("KiCad", 48.0),
                CHECKS => Col::new("Checks", 84.0),
                _ => Col::rest("Standard").no_sort(),
            })
            .collect();
        let (scope, downloading) = (self.scope, self.downloading);
        let mut click = None;
        let mut toggle_all = false;
        let (view, selected, checks, current, compliance) = (&self.view, &self.selected, &self.checks, &self.current, &self.compliance);
        let all_ticked = !view.is_empty() && selected.len() == view.len();
        let before = Sort { col: shown.iter().position(|&c| c == self.sort.col).unwrap_or(usize::MAX), asc: self.sort.asc };
        let mut sort = before;
        ui.scope(|ui| {
            // A selectable label would swallow the click that selects the row.
            ui.style_mut().interaction.selectable_labels = false;
            ui_kit::table_with_head(
                ui,
                "library",
                &cols,
                &mut sort,
                view.len(),
                |i, ui| {
                    if shown[i] != TICK {
                        return false;
                    }
                    let mut on = all_ticked;
                    let r = ui.checkbox(&mut on, "");
                    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, all_ticked, if all_ticked { "Select none" } else { "Select all" }));
                    toggle_all = r.clicked();
                    true
                },
                |i, row| {
                    let p = &parts[view[i]];
                    let key = (p.category.clone(), p.name.clone());
                    row.set_selected(current.as_ref() == Some(&key));
                    for &c in shown {
                        row.col(|ui| match c {
                            TICK => {
                                let mut ticked = selected.contains(&key);
                                let r = ui.checkbox(&mut ticked, "");
                                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, ticked, format!("Select {}", p.mpn)));
                                if r.clicked() {
                                    click = Some(Click::Tick(i, ui.input(|i| i.modifiers.shift)));
                                }
                            }
                            MPN => {
                                let r = ui.label(RichText::new(&p.mpn).monospace());
                                if !p.description.is_empty() {
                                    r.on_hover_text(&p.description);
                                }
                            }
                            MAKER => {
                                ui.label(&p.manufacturer);
                            }
                            CATEGORY => {
                                ui.label(RichText::new(&p.category).weak());
                            }
                            FILES => {
                                ui.horizontal(|ui| file_chips(ui, p, scope, downloading, root, &mut action, false));
                            }
                            KICAD => {
                                if p.in_kicad {
                                    ui.label(RichText::new("✔").color(Level::Ok.color(ui.visuals())))
                                        .on_hover_text("This category's library is in KiCad's library list");
                                } else {
                                    ui.label(RichText::new("no").color(Level::Warn.color(ui.visuals())))
                                        .on_hover_text("This category's library isn't in KiCad's library list yet. Relink libraries adds it.");
                                }
                            }
                            CHECKS => issues_cell(ui, checks.as_ref().map(|c| c.get(&key).unwrap_or(&NO_ISSUES))),
                            _ => compliance_cell(ui, compliance.as_ref().and_then(|c| c.get(&key))),
                        });
                    }
                    let r = row.response();
                    if r.clicked() {
                        click = Some(Click::Row(i, r.ctx.input(|i| i.modifiers)));
                    } else if r.secondary_clicked() {
                        click = Some(Click::Menu(i));
                    }
                },
            );
        });
        if sort != before {
            if let Some(&c) = shown.get(sort.col) {
                self.sort = Sort { col: c, asc: sort.asc };
            }
        }
        if toggle_all {
            self.selected = if all_ticked { HashSet::new() } else { (0..self.view.len()).filter_map(|i| self.key_at(i)).collect() };
        }
        if let Some(c) = click {
            self.click(c);
        }
        action
    }

    fn scope_toggle(&mut self, ui: &mut egui::Ui, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) {
        ui.add_enabled_ui(global.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Global, "Global"));
        ui.add_enabled_ui(project.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Project, "Project"));
    }

    /// Title, count and the library switch; filters; the page's own buttons. One row on a wide page, two on a
    /// medium one, and on a narrow one the rarer buttons move into More.
    fn header(&mut self, ui: &mut egui::Ui, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>, root: Option<&LibraryRoot>, action: &mut Action) {
        let width = ui.available_width();
        let (narrow, one_row) = (width < 700.0, width >= 1300.0);
        let n_bad = self.parts.as_ref().map_or(0, |ps| ps.iter().filter(|p| incomplete(p)).count());
        let before = self.scope;
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Library").heading().strong());
            if let Some(ps) = &self.parts {
                let loading = self.parts_rx.is_some() || self.checks_rx.is_some() || self.compliance_rx.is_some();
                let mut text = format!("{} parts", ps.len());
                if self.view.len() != ps.len() {
                    text += &format!(" · {} shown", self.view.len());
                }
                if loading {
                    text += " · reading…";
                }
                let dir = root.map(|r| r.dir.display().to_string()).unwrap_or_default();
                ui.weak(text).on_hover_text(format!("{dir}\n{n_bad} incomplete"));
            }
            if !narrow {
                self.scope_toggle(ui, global, project);
            }
            if one_row {
                self.filters(ui, n_bad);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.tools(ui, root, project, narrow, action));
        });
        if !one_row {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                if narrow {
                    self.scope_toggle(ui, global, project);
                }
                self.filters(ui, n_bad);
            });
        }
        if self.scope != before {
            self.stale = true;
            self.current = None;
            ui.ctx().request_repaint();
        }
        ui.add_space(4.0);
    }

    fn filters(&mut self, ui: &mut egui::Ui, n_bad: usize) {
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter MPN, maker, category…").desired_width(170.0));
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
        if ui_kit::chip(ui, "Incomplete", Some(n_bad), self.status == Filter::Incomplete)
            .on_hover_text("Parts missing a footprint, 3D model or datasheet, or not in KiCad")
            .clicked()
        {
            self.status = if self.status == Filter::Incomplete { Filter::All } else { Filter::Incomplete };
        }
        let current = Filter::ALL.iter().find(|(s, _)| *s == self.status).map_or("", |(_, l)| l);
        egui::ComboBox::from_id_salt("status_filter").selected_text(current).show_ui(ui, |ui| {
            for (s, label) in Filter::ALL {
                ui.selectable_value(&mut self.status, s, label);
            }
        });
    }

    /// The buttons on the right of the header, laid out right to left: Standardise, More, then the ghosts.
    fn tools(&mut self, ui: &mut egui::Ui, root: Option<&LibraryRoot>, project: Option<&LibraryRoot>, narrow: bool, action: &mut Action) {
        let style = ui.style().clone();
        // Ticked parts narrow Standardise to them; the whole library stays one menu click away.
        let picked = sorted(&self.selected);
        let (label, hover) = if picked.is_empty() {
            (
                "Standardise…".to_string(),
                "Fix symbols and footprints to the rules in Settings → Standards. Shows what changes first. Tick parts to standardise only those.",
            )
        } else {
            (
                format!("Standardise {} selected…", picked.len()),
                "Fix only the ticked parts and the footprints they use to the rules in Settings → Standards. Shows what changes first. More → Standardise whole library does everything.",
            )
        };
        let std = ui.add_enabled(root.is_some(), ui_kit::primary(&label, &style.visuals)).on_hover_text(hover);
        if let (true, Some(root)) = (std.clicked(), root) {
            *action = Action::Standardise(root.clone(), (!picked.is_empty()).then_some(picked.clone()));
        }
        ui.menu_button("More", |ui| {
            if !picked.is_empty() && ui.add_enabled(root.is_some(), egui::Button::new("Standardise whole library…")).on_hover_text("Fix every symbol and footprint, not just the ticked parts").clicked() {
                if let Some(root) = root {
                    *action = Action::Standardise(root.clone(), None);
                }
                ui.close();
            }
            if narrow {
                if ui.add_enabled(project.is_some(), egui::Button::new("Health report…")).on_hover_text("Everything wrong or untidy in the project's library").on_disabled_hover_text("Choose a project first").clicked() {
                    *action = Action::Health;
                    ui.close();
                }
                if ui.button("Alternatives…").on_hover_text("Find duplicates and equivalent parts, and mark preferred ones").clicked() {
                    *action = self.alternatives_action();
                    ui.close();
                }
            }
            if ui.add_enabled(root.is_some() && !self.downloading, egui::Button::new("Find missing datasheets")).on_hover_text("For parts with no working datasheet, try the existing link, then search the enabled sources by MPN, and save the first real PDF").clicked() {
                if let Some(root) = root {
                    *action = Action::FindDatasheets(root.clone());
                }
                ui.close();
            }
            if ui.add_enabled(root.is_some() && !self.downloading, egui::Button::new("Find / Generate SPICE models")).on_hover_text("For parts without a simulation model, find a close match in the library/KiCad/SPICE folders or generate one from datasheet & similar models").clicked() {
                if let Some(root) = root {
                    *action = Action::FindSims { root: root.clone(), targets: None };
                }
                ui.close();
            }
            if ui.button("Relink libraries").on_hover_text("Add every KLM library missing from KiCad's library list").clicked() {
                *action = Action::Batch(vec![Op::RelinkAll]);
                ui.close();
            }
            if ui.button("Repair 3D links…").on_hover_text("Preview broken model path repairs and restore missing models on the project's KLM footprints. Close KiCad before applying; backups are kept.").clicked() {
                *action = Action::Batch(vec![Op::RepairModels]);
                ui.close();
            }
            if ui.button("Refresh").on_hover_text("Read the library again").clicked() {
                self.stale = true;
                ui.close();
            }
        });
        if narrow {
            return;
        }
        if ui
            .add_enabled(project.is_some(), ui_kit::ghost("Health report…"))
            .on_hover_text("Everything wrong or untidy in the project's library")
            .on_disabled_hover_text("Choose a project first")
            .clicked()
        {
            *action = Action::Health;
        }
        if ui.add(ui_kit::ghost("Alternatives…")).on_hover_text("Find duplicates and equivalent parts, and mark preferred ones").clicked() {
            *action = self.alternatives_action();
        }
    }

    /// Alternatives opens on the one ticked part, else the current one.
    fn alternatives_action(&self) -> Action {
        let one = (self.selected.len() == 1).then(|| self.selected.iter().next().cloned()).flatten();
        Action::Alternatives(one.or_else(|| self.current.clone()))
    }

    /// The teal bar over the table while rows are ticked: what can be done to all of them.
    fn selection_bar(&mut self, ui: &mut egui::Ui, has_project: bool, root: &LibraryRoot, action: &mut Action) {
        let targets = sorted(&self.selected);
        let style = ui.style().clone();
        egui::Frame::new().fill(ui.visuals().selection.bg_fill).corner_radius(4).inner_margin(egui::Margin::symmetric(10, 5)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{} selected", targets.len())).strong());
                if ui.add(small("Edit…")).on_hover_text("Edit category, maker, description, datasheet and supplier numbers of the ticked parts").clicked() {
                    *action = Action::Edit(targets.clone());
                }
                let can_copy = self.scope == Scope::Global && has_project;
                let copy = ui.add_enabled(can_copy, small("Copy to project"));
                if copy
                    .on_hover_text("Copy the ticked parts into the project's library. Shift-click to replace copies already there.")
                    .on_disabled_hover_text("Needs the global library and a project")
                    .clicked()
                {
                    *action = Action::Batch(vec![Op::CopyToProject { parts: targets.clone(), overwrite: ui.input(|i| i.modifiers.shift) }]);
                }
                if ui.add(small("Attach…")).on_hover_text("Add one datasheet (PDF), SPICE model or 3D model (STEP/WRL) to every ticked part").clicked() {
                    if let Some(file) = pick_attachment() {
                        *action = Action::Batch(vec![Op::Attach { scope: self.scope, file, parts: targets.clone() }]);
                    }
                }
                let fill = ui.add_enabled(!self.downloading, small("Fill missing files…"));
                if fill
                    .on_hover_text("For every part in this library with no working datasheet, try the existing link, then search the enabled sources by MPN")
                    .clicked()
                {
                    *action = Action::FindDatasheets(root.clone());
                }
                if ui
                    .add_enabled(!self.downloading, small("Find SPICE…"))
                    .on_hover_text("Find close SPICE models or generate from datasheet & similar models for the ticked parts")
                    .clicked()
                {
                    *action = Action::FindSims { root: root.clone(), targets: Some(targets.clone()) };
                }
                if ui.add(small("Export…")).on_hover_text("Write the ticked parts, with their footprints and 3D models, to one ZIP").clicked() {
                    *action = Action::Export(root.clone());
                }
                let del = ui.add(ui_kit::danger("Delete…", false, &style.visuals).small());
                if del.on_hover_text("Delete the ticked parts, with the files no other part uses. Shows what goes first.").clicked() {
                    *action = Action::Delete(root.clone(), targets.clone());
                }
                if ui.add(ui_kit::ghost("Clear selection").small()).clicked() {
                    self.selected.clear();
                }
            });
        });
        ui.add_space(4.0);
    }

    /// The right-hand pane for the current part: what it is, its pictures, fields and alternatives, and its actions.
    fn inspector(&mut self, ui: &mut egui::Ui, p: &PartStatus, root: &LibraryRoot, has_project: bool, action: &mut Action) {
        let pal = theme::palette(ui.visuals());
        let key = (p.category.clone(), p.name.clone());
        let style = ui.style().clone();
        egui::Panel::bottom("library_inspector_footer")
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.panel).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let target = p.datasheet_target.as_deref();
                    let open = ui.add_enabled(target.is_some(), egui::Button::new("Open datasheet"));
                    if open.on_hover_text("Open the datasheet in your PDF viewer or browser").on_disabled_hover_text("This part has no datasheet").clicked() {
                        if let Some(t) = target {
                            let _ = open::that(t);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (can, why) = match (self.scope, has_project) {
                            (Scope::Project, _) => (false, "This part is already in the project library"),
                            (_, false) => (false, "Choose a project first"),
                            _ => (true, ""),
                        };
                        let copy = ui.add_enabled(can, ui_kit::primary("Copy to project", &style.visuals));
                        if copy
                            .on_hover_text("Copy this part into the project's library. Shift-click to replace a copy already there.")
                            .on_disabled_hover_text(why)
                            .clicked()
                        {
                            let overwrite = ui.input(|i| i.modifiers.shift);
                            *action = Action::CopyToProject { category: key.0.clone(), name: key.1.clone(), overwrite };
                        }
                    });
                });
            });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // Identity: MPN, who makes it, which files it has.
            section(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&p.mpn).monospace().size(16.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let x = ui.add(ui_kit::ghost("✕"));
                        x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close inspector"));
                        if x.clicked() {
                            self.current = None;
                            ui.ctx().request_repaint();
                        }
                        ui.menu_button("Actions", |ui| {
                            if ui.button("Reimport…").on_hover_text("Fetch this part's symbol, footprint, 3D model and datasheet from the sources again, replacing the library copy").clicked() {
                                *action = Action::Reimport(key.clone());
                                ui.close();
                            }
                            if ui.button("Preview").on_hover_text("Show the symbol, footprint and 3D model large").clicked() {
                                *action = Action::Preview { category: key.0.clone(), name: key.1.clone() };
                                ui.close();
                            }
                            if ui.button("Use as house template for its class").on_hover_text("Make this symbol (and its footprint's text, line and pin-1 style) the active profile's house style for its kind of part. Restyle and Standardise then follow it. Settings > Symbols puts the built-in back.").clicked() {
                                *action = Action::HouseTemplate { category: key.0.clone(), name: key.1.clone() };
                                ui.close();
                            }
                            if ui.add(ui_kit::danger("Delete…", false, ui.visuals())).on_hover_text("Delete this part, with the files no other part uses. Shows what goes first.").clicked() {
                                *action = Action::Delete(root.clone(), vec![key.clone()]);
                                ui.close();
                            }
                        });
                    });
                });
                let who = [p.manufacturer.as_str(), p.category.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                ui.weak(format!("{who} · {} library", if self.scope == Scope::Global { "global" } else { "project" }));
                ui.horizontal(|ui| file_chips(ui, p, self.scope, self.downloading, root, action, true));
                ui.horizontal(|ui| {
                    ui.weak("House standard");
                    compliance_cell(ui, self.compliance.as_ref().and_then(|c| c.get(&key)));
                });
            });
            // The three pictures, and what can be done to the symbol.
            section(ui, |ui| {
                if self.inspect.as_mut().is_some_and(|i| i.preview.pictures(ui)) {
                    *action = Action::Preview { category: key.0.clone(), name: key.1.clone() };
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if ui.add(small("Edit symbol…")).on_hover_text("Edit pins, body and text positions of the symbol in the app (graphical editing: use KiCad)").clicked() {
                        *action = Action::EditSymbol { category: key.0.clone(), name: key.1.clone() };
                    }
                    if ui.add(small("Restyle symbol…")).on_hover_text("Redraw the symbol in the house style for its kind of part. Shows what changes first. Keeps every field; undo it in History. Wires on schematics that already use it may need fixing.").clicked() {
                        *action = Action::Restyle { category: key.0.clone(), name: key.1.clone() };
                    }
                    ui.menu_button("Open in KiCad", |ui| {
                        if ui.button("Symbol in KiCad's editor").on_hover_text("Open this category's symbol library in KiCad's symbol editor (pick the part in its tree). Saving there re-checks the part here.").clicked() {
                            *action = Action::EditInKicad { editor: Editor::Symbol, category: key.0.clone(), name: key.1.clone() };
                            ui.close();
                        }
                        if ui.button("Footprint in KiCad's editor").on_hover_text("Open this part's footprint in KiCad. Saving there re-checks the part here.").clicked() {
                            *action = Action::EditInKicad { editor: Editor::Footprint, category: key.0.clone(), name: key.1.clone() };
                            ui.close();
                        }
                    });
                });
            });
            // Fields.
            section(ui, |ui| {
                ui.horizontal(|ui| {
                    ui_kit::section_label(ui, "Fields");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(ui_kit::ghost("Edit…")).on_hover_text("Edit MPN, maker, description, category, supplier numbers, datasheet and tags").clicked() {
                            *action = Action::Edit(vec![key.clone()]);
                        }
                        if ui.add(ui_kit::ghost("Attach…")).on_hover_text("Add a datasheet (PDF), SPICE model, 3D model (STEP/WRL) or a footprint/3D ZIP without a symbol").clicked() {
                            if let Some(path) = pick_attachment() {
                                *action = Action::Attach { scope: self.scope, category: key.0.clone(), name: key.1.clone(), path };
                            }
                        }
                    });
                });
                let rows: Vec<_> = self.inspect.iter().flat_map(|i| i.fields.iter()).filter(|(k, v)| !v.is_empty() && !matches!(*k, "MPN" | "Manufacturer")).collect();
                if rows.is_empty() {
                    ui.weak("No fields set.");
                }
                egui::Grid::new("inspector_fields").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                    for (k, v) in rows {
                        ui.label(RichText::new(*k).weak().size(12.0));
                        let text = if matches!(*k, "Description" | "Tags") { RichText::new(v).size(12.0) } else { RichText::new(v).monospace().size(12.0) };
                        ui.add(egui::Label::new(text).truncate()).on_hover_text(v);
                        ui.end_row();
                    }
                });
            });
            // Alternatives.
            section(ui, |ui| {
                let n = self.inspect.as_ref().map_or(0, |i| i.alternatives.len());
                ui.horizontal(|ui| {
                    ui_kit::section_label(ui, "Alternatives");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(ui_kit::ghost("Find…")).on_hover_text("Find duplicates and equivalent parts, and mark preferred ones").clicked() {
                            *action = Action::Alternatives(Some(key.clone()));
                        }
                    });
                });
                if n == 0 {
                    ui.weak("None marked yet.");
                }
                for a in self.inspect.iter().flat_map(|i| i.alternatives.iter()) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&a.mpn).monospace().size(12.0));
                        ui.weak(&a.manufacturer);
                        ui_kit::badge(ui, &a.role, if a.role == "preferred" { Level::Ok } else { Level::Info });
                    });
                }
            });
            // What the part checks found.
            section(ui, |ui| {
                ui_kit::section_label(ui, "Checks");
                if let Some(i) = &self.inspect {
                    i.preview.issues_list(ui);
                }
            });
        });
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
        let file = format!("{} → {}", self.path.file_name().unwrap_or_default().to_string_lossy(), self.name);
        let targets: Vec<LibraryRoot> =
            [(self.to_global, global), (self.to_project, project)].into_iter().filter_map(|(on, r)| if on { r.cloned() } else { None }).collect();
        let dismissed = batch_dialog::dialog(
            ctx,
            egui::Id::NULL.with("attach_dialog"),
            460.0,
            "Add file",
            None,
            |ui| {
                ui.label(RichText::new(&file).monospace());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Add to");
                    ui.add_enabled(global.is_some(), egui::Checkbox::new(&mut self.to_global, "Global"));
                    ui.add_enabled(project.is_some(), egui::Checkbox::new(&mut self.to_project, "Project"));
                });
                ui.weak("The file is copied into each library's folder. A library without this part gets a copy of it first.");
            },
            |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(!targets.is_empty(), ui_kit::primary("Add", ui.visuals())).clicked() {
                        action = AttachAction::Attach { targets: targets.clone() };
                    }
                    if ui.button("Cancel").clicked() {
                        action = AttachAction::Cancel;
                    }
                });
            },
        );
        if dismissed && matches!(action, AttachAction::None) {
            action = AttachAction::Cancel;
        }
        action
    }
}
