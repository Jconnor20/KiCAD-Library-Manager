use super::edit_dialog::{EditAction, EditDialog};
use crate::{AppCtx, Msg as AppMsg};
use eframe::egui::{self, Color32, RichText};
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
    /// The datasheet download worker has finished.
    DownloadDone,
}

/// Something the user asked for on a row; `ui` carries it out and refreshes the view.
enum Action {
    None,
    /// Open the edit dialog for these (category, name) parts.
    Edit(Vec<(String, String)>),
    Attach { scope: Scope, category: String, name: String, path: PathBuf },
    CopyToProject { category: String, name: String, overwrite: bool },
    DownloadDatasheets(LibraryRoot),
}

/// Table of the parts in the global or project library, with what each one is missing.
pub struct LibraryTab {
    scope: Scope,
    filter: String,
    only_incomplete: bool,
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
}

impl Default for LibraryTab {
    fn default() -> Self {
        Self {
            scope: Scope::Global,
            filter: String::new(),
            only_incomplete: false,
            parts: None,
            checks: None,
            checks_rx: None,
            attach_dialog: None,
            downloading: false,
            selected: HashSet::new(),
            edit_dialog: None,
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

impl LibraryTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
        match self.show(ui, global.as_ref(), project.as_ref()) {
            Action::None => {}
            Action::DownloadDatasheets(root) => self.download_datasheets(root, cx),
            Action::Edit(targets) => {
                let root = if self.scope == Scope::Global { &global } else { &project };
                if let Some(root) = root {
                    self.edit_dialog = Some(EditDialog::new(self.scope, targets, root));
                }
            }
            Action::Attach { scope, category, name, path } => self.attach_dialog = Some(AttachDialog::new(scope, category, name, path)),
            Action::CopyToProject { category, name, overwrite } => {
                // Only a library new to the project touches KiCad's tables.
                if project.as_ref().is_some_and(|p| p.needs_link(&category)) {
                    cx.when_kicad_closed(AppMsg::Library(Msg::CopyToProject { category, name, overwrite }));
                } else {
                    self.copy_to_project(&category, &name, overwrite, cx);
                }
            }
        }
    }

    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        match msg {
            Msg::Refresh => self.parts = None,
            Msg::DownloadDone => {
                self.downloading = false;
                self.parts = None;
            }
            Msg::Edit { scope, targets, edit } => self.edit(scope, &targets, &edit, cx),
            Msg::CopyToProject { category, name, overwrite } => self.copy_to_project(&category, &name, overwrite, cx),
            Msg::Attach { from, targets, category, name, path } => self.attach(&from, &targets, &category, &name, &path, cx),
        }
    }

    /// The "add file" popup; shown whatever tab is selected.
    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
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
                        cx.when_kicad_closed(AppMsg::Library(Msg::Edit { scope, targets, edit }));
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
                            cx.when_kicad_closed(AppMsg::Library(Msg::Attach { from, targets, category, name, path }));
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

    /// Downloads a PDF for every part in `root` whose datasheet is only a web link, logging each result.
    fn download_datasheets(&mut self, root: LibraryRoot, cx: &mut AppCtx) {
        self.downloading = true;
        let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
        cx.log.push("Downloading datasheets for parts that only have a link…".into());
        std::thread::spawn(move || {
            let results = root.download_missing_datasheets();
            if results.is_empty() {
                let _ = tx.send(AppMsg::Log("No parts with a link-only datasheet".into()));
            }
            for (mpn, result) in results {
                let line = match result {
                    Ok(()) => format!("Downloaded the datasheet for {mpn}"),
                    Err(e) => format!("Could not download the datasheet for {mpn} (link kept): {e:#}"),
                };
                let _ = tx.send(AppMsg::Log(line));
            }
            let _ = tx.send(AppMsg::Library(Msg::DownloadDone));
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
        let mut edit_selected = false;
        ui.horizontal(|ui| {
            let before = self.scope;
            ui.add_enabled_ui(global.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Global, "Global"));
            ui.add_enabled_ui(project.is_some(), |ui| ui.selectable_value(&mut self.scope, Scope::Project, "Project"));
            if self.scope != before {
                self.parts = None;
                self.selected.clear();
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter by MPN, maker, category…").desired_width(220.0));
            edit_selected = ui.add_enabled(!self.selected.is_empty(), egui::Button::new("Edit selected…")).clicked();
            ui.checkbox(&mut self.only_incomplete, "Only incomplete");
            if ui.button("Refresh").clicked() {
                self.parts = None;
            }
            download = ui
                .add_enabled(!self.downloading, egui::Button::new("Download missing datasheets"))
                .on_hover_text("Save a local PDF for every part whose datasheet is only a web link")
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
        let parts = self.parts.get_or_insert_with(|| root.list_parts());

        let bad = parts.iter().filter(|p| incomplete(p)).count();
        ui.weak(format!("{} parts in {} · {bad} incomplete", parts.len(), root.dir.display()));
        ui.add_space(4.0);

        let needle = self.filter.to_lowercase();
        let shown: Vec<&PartStatus> = parts
            .iter()
            .filter(|p| !self.only_incomplete || incomplete(p))
            .filter(|p| needle.is_empty() || [&p.mpn, &p.manufacturer, &p.category, &p.description].iter().any(|f| f.to_lowercase().contains(&needle)))
            .collect();

        let mut action = if download { Action::DownloadDatasheets(root.clone()) } else { Action::None };
        if edit_selected {
            let mut targets: Vec<_> = self.selected.iter().cloned().collect();
            targets.sort();
            action = Action::Edit(targets);
        }
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            // Selection tick box, eight status columns, Checks, then the row buttons.
            egui::Grid::new("library").striped(true).num_columns(11).spacing([14.0, 4.0]).show(ui, |ui| {
                ui.label("");
                for h in ["Category", "MPN", "Manufacturer", "Footprint", "3D model", "Datasheet", "Sim model", "In KiCad", "Checks", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for p in shown {
                    let key = (p.category.clone(), p.name.clone());
                    let mut ticked = self.selected.contains(&key);
                    if ui.checkbox(&mut ticked, "").changed() {
                        if ticked {
                            self.selected.insert(key);
                        } else {
                            self.selected.remove(&key);
                        }
                    }
                    ui.label(&p.category);
                    let mpn = ui.label(RichText::new(&p.mpn).strong());
                    if !p.description.is_empty() {
                        mpn.on_hover_text(&p.description);
                    }
                    ui.label(&p.manufacturer);
                    check_cell(ui, p.footprint, "Footprint");
                    check_cell(ui, p.model, "3D model");
                    check_cell(ui, p.datasheet, "Datasheet");
                    check_cell(ui, p.sim, "Sim model");
                    check_cell(ui, if p.in_kicad { Check::Ok } else { Check::Missing }, "KiCad library entry");
                    issues_cell(ui, self.checks.as_ref().map(|c| c.get(&(p.category.clone(), p.name.clone())).unwrap_or(&NO_ISSUES)));
                    ui.horizontal(|ui| {
                        if ui.small_button("Edit…").on_hover_text("Edit MPN, maker, description, category, supplier numbers, datasheet and tags").clicked() {
                            action = Action::Edit(vec![(p.category.clone(), p.name.clone())]);
                        }
                        if ui.small_button("Attach…").on_hover_text("Add a datasheet (PDF), SPICE model or 3D model (STEP/WRL)").clicked() {
                            let picked = rfd::FileDialog::new().add_filter("Datasheet, sim or 3D model", ATTACH_EXTENSIONS).pick_file();
                            if let Some(path) = picked {
                                action = Action::Attach { scope: self.scope, category: p.category.clone(), name: p.name.clone(), path };
                            }
                        }
                        if self.scope == Scope::Global {
                            let copy = ui.add_enabled(project.is_some(), egui::Button::new("Copy to project").small());
                            if copy.on_hover_text("Copy this part into the project's library. Shift-click to replace a copy already there.").clicked() {
                                let overwrite = ui.input(|i| i.modifiers.shift);
                                action = Action::CopyToProject { category: p.category.clone(), name: p.name.clone(), overwrite };
                            }
                        }
                        if let Some(target) = &p.datasheet_target {
                            if ui.small_button("Open datasheet").clicked() {
                                let _ = open::that(target);
                            }
                        }
                    });
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
