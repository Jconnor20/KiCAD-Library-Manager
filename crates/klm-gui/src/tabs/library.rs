use super::alternatives::AlternativesPanel;
use super::batch_dialog::{self, BatchDialog, Progress};
use super::edit_dialog::{EditAction, EditDialog};
use super::preview::PreviewPanel;
use crate::{AppCtx, Msg as AppMsg};
use eframe::egui::{self, Color32, RichText};
use klm_core::batch::{self, Op};
use klm_core::checks::{self, Issue, Severity};
use klm_core::library::{ATTACH_EXTENSIONS, AttachKind, Check, LibraryRoot, PartEdit, PartStatus, Scope};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

pub enum Msg {
    /// Re-read the listing, e.g. after an import.
    Refresh,
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
    CopyToProject { category: String, name: String, overwrite: bool },
    FindDatasheets(LibraryRoot),
    /// Show the dry run of these operations.
    Batch(Vec<Op>),
}

/// Table of the parts in the global or project library, with what each one is missing.
pub struct LibraryTab {
    scope: Scope,
    filter: String,
    /// Category, manufacturer and status filters; empty means all.
    category: String,
    maker: String,
    status: Status,
    sort: (Col, bool),
    /// The row whose actions the detail panel shows.
    current: Option<(String, String)>,
    finder_results: Option<Vec<(String, Result<String, String>)>>,
    /// Cached listing; `None` means re-read from disk on the next frame.
    parts: Option<Vec<PartStatus>>,
    /// Part checks for `parts`, by (category, name). They read every footprint and 3D model, so they
    /// run on a thread and arrive after the listing.
    checks: Option<HashMap<(String, String), Vec<Issue>>>,
    checks_rx: Option<mpsc::Receiver<HashMap<(String, String), Vec<Issue>>>>,
    attach_dialog: Option<AttachDialog>,
    /// A worker is downloading datasheets.
    downloading: bool,
    /// Ticked rows, as (category, name).
    selected: HashSet<(String, String)>,
    edit_dialog: Option<EditDialog>,
    batch: Option<BatchDialog>,
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
            status: Status::All,
            sort: (Col::Category, true),
            current: None,
            finder_results: None,
            parts: None,
            checks: None,
            checks_rx: None,
            attach_dialog: None,
            downloading: false,
            selected: HashSet::new(),
            edit_dialog: None,
            batch: None,
            preview: None,
            alternatives: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    All,
    Incomplete,
    NoDatasheet,
    NoModel,
}

impl Status {
    const ALL: [(Status, &'static str); 4] = [(Status::All, "Any status"), (Status::Incomplete, "Incomplete"), (Status::NoDatasheet, "No datasheet"), (Status::NoModel, "No 3D model")];

    fn matches(self, p: &PartStatus) -> bool {
        let no = |c: Check| matches!(c, Check::Missing | Check::Broken);
        match self {
            Status::All => true,
            Status::Incomplete => incomplete(p),
            Status::NoDatasheet => no(p.datasheet),
            Status::NoModel => no(p.model),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Category,
    Mpn,
    Maker,
    Footprint,
    Model,
    Datasheet,
    Sim,
    InKicad,
    Checks,
}

impl Col {
    const ALL: [(Col, &'static str); 9] = [
        (Col::Category, "Category"),
        (Col::Mpn, "MPN"),
        (Col::Maker, "Manufacturer"),
        (Col::Footprint, "Footprint"),
        (Col::Model, "3D model"),
        (Col::Datasheet, "Datasheet"),
        (Col::Sim, "Sim model"),
        (Col::InKicad, "In KiCad"),
        (Col::Checks, "Checks"),
    ];
}

/// Worst-first rank, so ascending puts the parts with problems on top.
fn rank(c: Check) -> u8 {
    match c {
        Check::Broken => 0,
        Check::Missing => 1,
        Check::Link => 2,
        Check::Ok => 3,
    }
}

fn compare(col: Col, a: &PartStatus, b: &PartStatus, issues: impl Fn(&PartStatus) -> usize) -> std::cmp::Ordering {
    match col {
        Col::Category => a.category.to_lowercase().cmp(&b.category.to_lowercase()),
        Col::Mpn => a.mpn.to_lowercase().cmp(&b.mpn.to_lowercase()),
        Col::Maker => a.manufacturer.to_lowercase().cmp(&b.manufacturer.to_lowercase()),
        Col::Footprint => rank(a.footprint).cmp(&rank(b.footprint)),
        Col::Model => rank(a.model).cmp(&rank(b.model)),
        Col::Datasheet => rank(a.datasheet).cmp(&rank(b.datasheet)),
        Col::Sim => rank(a.sim).cmp(&rank(b.sim)),
        Col::InKicad => a.in_kicad.cmp(&b.in_kicad),
        Col::Checks => issues(b).cmp(&issues(a)),
    }
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
    if btn(ui, "Attach…", true, "Add a datasheet (PDF), SPICE model, 3D model (STEP/WRL) or a footprint/3D ZIP without a symbol", "") {
        let picked = rfd::FileDialog::new().add_filter("Datasheet, sim, 3D model or footprint ZIP", ATTACH_EXTENSIONS).pick_file();
        if let Some(path) = picked {
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

fn check_cell(ui: &mut egui::Ui, c: Check, what: &str) {
    let (text, color, tip) = match c {
        Check::Ok => ("✔", Color32::from_rgb(60, 170, 90), format!("{what} present")),
        Check::Link => ("link", ui.visuals().hyperlink_color, format!("{what} points at a web page or another library")),
        Check::Missing => ("missing", ui.visuals().warn_fg_color, format!("No {what}")),
        Check::Broken => ("broken", ui.visuals().error_fg_color, format!("{what} is set but its file isn't there")),
    };
    ui.label(RichText::new(text).color(color)).on_hover_text(tip);
}

static NO_ISSUES: Vec<Issue> = Vec::new();

fn issues_cell(ui: &mut egui::Ui, issues: Option<&Vec<Issue>>) {
    let Some(issues) = issues else {
        ui.weak("…").on_hover_text("Checking…");
        return;
    };
    let Some(worst) = issues.iter().map(|i| i.severity).max() else {
        ui.label(RichText::new("✔").color(Color32::from_rgb(60, 170, 90))).on_hover_text("No problems found");
        return;
    };
    let color = if worst == Severity::Error { ui.visuals().error_fg_color } else { ui.visuals().warn_fg_color };
    let tip: Vec<_> = issues.iter().map(|i| format!("• {}", i.message)).collect();
    ui.label(RichText::new(issues.len().to_string()).color(color)).on_hover_text(tip.join("
"));
}

/// The ticked rows in a stable order.
fn sorted(selected: &HashSet<(String, String)>) -> Vec<(String, String)> {
    let mut targets: Vec<_> = selected.iter().cloned().collect();
    targets.sort();
    targets
}

impl LibraryTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
        match self.show(ui, global.as_ref(), project.as_ref()) {
            Action::None => {}
            Action::FindDatasheets(root) => self.find_datasheets(root, cx),
            Action::Batch(ops) => self.batch = Some(BatchDialog::Review(batch::plan(ops, global.as_ref(), project.as_ref()))),
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
            Msg::Refresh => self.parts = None,
            Msg::DatasheetsDone(results) => {
                self.downloading = false;
                self.parts = None;
                self.finder_results = Some(results);
            }
            Msg::Batch(p) => {
                if batch_dialog::progress(&mut self.batch, p, cx).is_some() {
                    self.parts = None;
                }
            }
            Msg::Edit { scope, targets, edit } => self.edit(scope, &targets, &edit, cx),
            Msg::CopyToProject { category, name, overwrite } => self.copy_to_project(&category, &name, overwrite, cx),
            Msg::Attach { from, targets, category, name, path } => self.attach(&from, &targets, &category, &name, &path, cx),
        }
    }

    /// The "add file" popup; shown whatever tab is selected.
    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
        if let Some(plan) = batch_dialog::show(&mut self.batch, &cx.ctx) {
            batch_dialog::start(&mut self.batch, plan, cx, |p| AppMsg::Library(Msg::Batch(p)));
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
        if self.alternatives.as_mut().is_some_and(|a| !a.window(&cx.ctx, &mut cx.log)) {
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
                Ok(()) => cx.log.push(format!("Added {file} to {name} in {nick}")),
                Err(e) => cx.log.push(format!("Could not add {file} to {name} in {nick}: {e:#}")),
            }
        }
        self.parts = None;
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
            match root.edit_part(category, name, edit) {
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
        self.parts = None;
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
        self.parts = None;
    }

    fn show(&mut self, ui: &mut egui::Ui, global: Option<&LibraryRoot>, project: Option<&LibraryRoot>) -> Action {
        let mut download = false;
        let mut alternatives = false;
        let mut edit_selected = false;
        let (mut copy_selected, mut relink_all) = (None, false);
        let mut attach_to_selected = None;
        let has_selection = !self.selected.is_empty();
        ui.horizontal_wrapped(|ui| {
            ui.strong("View");
            let before = self.scope;
            ui.add_enabled_ui(global.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Global, "Global"));
            ui.add_enabled_ui(project.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Project, "Project"));
            if self.scope != before {
                self.parts = None;
                self.selected.clear();
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
            let current = Status::ALL.iter().find(|(s, _)| *s == self.status).map_or("", |(_, l)| l);
            egui::ComboBox::from_id_salt("status_filter").selected_text(current).show_ui(ui, |ui| {
                for (s, label) in Status::ALL {
                    ui.selectable_value(&mut self.status, s, label);
                }
            });
            if ui.button("Refresh").clicked() {
                self.parts = None;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.strong("Library actions");
            edit_selected = ui.add_enabled(has_selection, egui::Button::new("Edit selected…")).on_disabled_hover_text("Tick parts first").clicked();
            let can_copy = self.scope == Scope::Global && project.is_some() && has_selection;
            let copy = ui.add_enabled(can_copy, egui::Button::new("Copy selected to project"));
            if copy.on_hover_text("Copy the ticked parts into the project's library. Shift-click to replace copies already there.").clicked() {
                copy_selected = Some(ui.input(|i| i.modifiers.shift));
            }
            let attach = ui.add_enabled(has_selection, egui::Button::new("Attach to selected…"));
            if attach.on_hover_text("Add one datasheet (PDF), SPICE model or 3D model (STEP/WRL) to every ticked part").clicked() {
                attach_to_selected = rfd::FileDialog::new().add_filter("Datasheet, sim, 3D model or footprint ZIP", ATTACH_EXTENSIONS).pick_file();
            }
            alternatives = ui.button("Alternatives…").on_hover_text("Find duplicates and equivalent parts, and mark preferred ones").clicked();
            relink_all = ui.button("Relink libraries").on_hover_text("Add every KLM library missing from KiCad's library list").clicked();
            download = ui
                .add_enabled(!self.downloading, egui::Button::new("Find missing datasheets"))
                .on_hover_text("For parts with no working datasheet, try the existing link, then search the enabled sources by MPN, and save the first real PDF")
                .clicked();
        });

        let root = match self.scope {
            Scope::Global => global,
            Scope::Project => project,
        };
        let Some(root) = root else {
            ui.label(match self.scope {
                Scope::Global => "Set up a global library first.",
                Scope::Project => "Choose a project first.",
            });
            return Action::None;
        };
        if self.parts.is_none() {
            let (tx, rx) = mpsc::channel();
            let (root, ctx) = (root.clone(), ui.ctx().clone());
            std::thread::spawn(move || {
                let _ = tx.send(checks::check_library(&root));
                ctx.request_repaint();
            });
            (self.checks, self.checks_rx) = (None, Some(rx));
        }
        if let Some(done) = self.checks_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (self.checks, self.checks_rx) = (Some(done), None);
        }
        if alternatives {
            self.alternatives = Some(AlternativesPanel::new(root, ui.ctx()));
        }
        let parts = self.parts.get_or_insert_with(|| root.list_parts());

        let bad = parts.iter().filter(|p| incomplete(p)).count();
        ui.weak(format!("{} parts in {} · {bad} incomplete", parts.len(), root.dir.display()));
        ui.add_space(4.0);

        let needle = self.filter.to_lowercase();
        let mut shown: Vec<&PartStatus> = parts
            .iter()
            .filter(|p| self.status.matches(p))
            .filter(|p| self.category.is_empty() || p.category == self.category)
            .filter(|p| self.maker.is_empty() || p.manufacturer == self.maker)
            .filter(|p| needle.is_empty() || [&p.mpn, &p.manufacturer, &p.category, &p.description].iter().any(|f| f.to_lowercase().contains(&needle)))
            .collect();
        let issue_count = |p: &PartStatus| self.checks.as_ref().and_then(|c| c.get(&(p.category.clone(), p.name.clone()))).map_or(0, Vec::len);
        let (col, asc) = self.sort;
        shown.sort_by(|a, b| {
            let o = compare(col, a, b, issue_count);
            if asc { o } else { o.reverse() }
        });

        let mut action = if download { Action::FindDatasheets(root.clone()) } else { Action::None };
        if edit_selected {
            action = Action::Edit(sorted(&self.selected));
        }
        if let Some(overwrite) = copy_selected {
            action = Action::Batch(vec![Op::CopyToProject { parts: sorted(&self.selected), overwrite }]);
        }
        if let Some(file) = attach_to_selected {
            action = Action::Batch(vec![Op::Attach { scope: self.scope, file, parts: sorted(&self.selected) }]);
        }
        if relink_all {
            action = Action::Batch(vec![Op::RelinkAll]);
        }
        let current = self.current.as_ref().and_then(|k| parts.iter().find(|p| (&p.category, &p.name) == (&k.0, &k.1)));
        egui::Panel::bottom("library_detail").resizable(false).show(ui, |ui| {
            match current {
                Some(p) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&p.mpn);
                        ui.weak(format!("{} · {}", p.manufacturer, p.category));
                    });
                    ui.horizontal_wrapped(|ui| part_actions(ui, p, self.scope, project.is_some(), false, &mut action));
                }
                None => {
                    ui.weak("Click a part to see its actions. Right-click a row for the same actions.");
                }
            }
        });
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            // Selection tick box, eight status columns, then Checks.
            egui::Grid::new("library").striped(true).num_columns(10).spacing([14.0, 4.0]).show(ui, |ui| {
                ui.label("");
                for (c, h) in Col::ALL {
                    let active = self.sort.0 == c;
                    let clicked = ui
                        .horizontal(|ui| {
                            let arrow = if !active { "⇅" } else if self.sort.1 { "▲" } else { "▼" };
                            let a = ui.add(egui::Label::new(RichText::new(h).strong()).sense(egui::Sense::click()));
                            let b = ui.add(egui::Label::new(if active { RichText::new(arrow) } else { RichText::new(arrow).weak() }).sense(egui::Sense::click()));
                            a.clicked() || b.clicked()
                        })
                        .inner;
                    if clicked {
                        self.sort = (c, if active { !self.sort.1 } else { true });
                    }
                }
                ui.end_row();
                for p in shown {
                    let key = (p.category.clone(), p.name.clone());
                    let mut ticked = self.selected.contains(&key);
                    if ui.checkbox(&mut ticked, "").changed() {
                        if ticked {
                            self.selected.insert(key.clone());
                        } else {
                            self.selected.remove(&key);
                        }
                    }
                    ui.label(&p.category);
                    let is_current = self.current.as_ref() == Some(&key);
                    let mut mpn = ui.selectable_label(is_current, RichText::new(&p.mpn).strong());
                    if !p.description.is_empty() {
                        mpn = mpn.on_hover_text(&p.description);
                    }
                    if mpn.clicked() || mpn.secondary_clicked() {
                        self.current = Some(key.clone());
                    }
                    mpn.context_menu(|ui| part_actions(ui, p, self.scope, project.is_some(), true, &mut action));
                    ui.label(&p.manufacturer);
                    check_cell(ui, p.footprint, "Footprint");
                    check_cell(ui, p.model, "3D model");
                    check_cell(ui, p.datasheet, "Datasheet");
                    check_cell(ui, p.sim, "Sim model");
                    check_cell(ui, if p.in_kicad { Check::Ok } else { Check::Missing }, "KiCad library entry");
                    issues_cell(ui, self.checks.as_ref().map(|c| c.get(&key).unwrap_or(&NO_ISSUES)));
                    ui.end_row();
                }
            });
        });
        action
    }
}

enum AttachAction {
    None,
    Cancel,
    Attach { targets: Vec<LibraryRoot> },
}

/// Asks which libraries a picked datasheet, sim or 3D model file goes into.
struct AttachDialog {
    /// The library the part was listed in, which holds the part.
    from: Scope,
    category: String,
    name: String,
    path: PathBuf,
    to_global: bool,
    to_project: bool,
}

impl AttachDialog {
    fn new(from: Scope, category: String, name: String, path: PathBuf) -> Self {
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
