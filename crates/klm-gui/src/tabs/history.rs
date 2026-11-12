use crate::{AppCtx, Msg as AppMsg};
use super::page_kit;
use super::ui_kit::{self, Col, Level, Section, Sort};
use crate::theme;
use eframe::egui::{self, RichText};
use klm_core::library::Scope;
use klm_core::snapshot::{self, Change, ChangeKind, Info, Kind};

pub enum Msg {
    /// Put the library back as it was when snapshot `id` was taken.
    Rollback { scope: Scope, id: String },
}

/// Snapshots of the global or project library, newest first, each with a "Roll back to before this" button.
pub struct HistoryTab {
    scope: Scope,
    /// Cached listing (with sizes); `None` means re-read from disk on the next frame.
    snapshots: Option<Vec<Info>>,
    /// Modified time of the history folder when `snapshots` was read: a new or deleted snapshot changes it.
    seen: Option<std::time::SystemTime>,
    /// Snapshot waiting for the user to confirm the rollback.
    confirm: Option<Info>,
    /// Snapshot waiting for the user to confirm deleting it.
    confirm_delete: Option<Info>,
    /// Name being typed for a version: of snapshot `id`, or of a new snapshot when `None`.
    naming: Option<(Option<String>, String)>,
    versions_only: bool,
    /// Snapshot whose changed files are listed below the table.
    selected: Option<String>,
    /// Files the `selected` snapshot changed; re-read with the snapshot list.
    changed: Option<(String, Result<Vec<Change>, String>)>,
}

/// What a row's menu asked for.
enum Act {
    Roll(Info),
    Delete(Info),
    Minor(String),
    Name(String),
}

fn row_menu(ui: &mut egui::Ui, s: &Info, act: &mut Option<Act>) {
    if ui.button("Roll back to before this").clicked() {
        *act = Some(Act::Roll(s.clone()));
        ui.close();
    }
    if s.kind == Kind::Major {
        if ui.button("Make minor").clicked() {
            *act = Some(Act::Minor(s.id.clone()));
            ui.close();
        }
    } else if ui.button("Mark as version…").clicked() {
        *act = Some(Act::Name(s.id.clone()));
        ui.close();
    }
    ui.separator();
    if ui.button(RichText::new("Delete…").color(Level::Error.color(ui.visuals()))).clicked() {
        *act = Some(Act::Delete(s.clone()));
        ui.close();
    }
}

impl Default for HistoryTab {
    fn default() -> Self {
        Self { scope: Scope::Global, snapshots: None, seen: None, confirm: None, confirm_delete: None, naming: None, versions_only: false, selected: None, changed: None }
    }
}

fn ago(ts: i64) -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) - ts;
    match secs {
        ..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.0} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

impl HistoryTab {
    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        let Msg::Rollback { scope, id } = msg;
        let root = if scope == Scope::Global { cx.global_root() } else { cx.project_root() };
        let Some(root) = root else { return };
        match snapshot::rollback(&root, &id) {
            Ok(()) => cx.log.push("Rolled the library back; the state before the rollback is the newest snapshot, in case you want it back.".into()),
            Err(e) => cx.log.push(format!("Could not roll back: {e:#}")),
        }
        self.snapshots = None;
        cx.send(AppMsg::Library(super::library::Msg::Refresh));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let (global, project) = (cx.global_root(), cx.project_root());
        let pal = theme::palette(ui.visuals());
        ui.horizontal(|ui| {
            ui.label(RichText::new("History").heading().strong());
            ui.add_space(8.0);
            for (scope, name) in [(Scope::Global, "Global"), (Scope::Project, "Project")] {
                if ui.selectable_value(&mut self.scope, scope, name).clicked() {
                    self.snapshots = None;
                }
            }
            if ui.button("Refresh").clicked() {
                self.snapshots = None;
            }
        });
        ui.add_space(4.0);
        let root = if self.scope == Scope::Global { global } else { project };
        let Some(root) = root else {
            ui.weak("This library isn't set up.");
            return;
        };
        // One stat per frame; the listing walks every snapshot, so it only reruns after a change.
        let stamp = std::fs::metadata(root.dir.join(snapshot::DIR)).and_then(|m| m.modified()).ok();
        if stamp != self.seen {
            self.snapshots = None;
        }
        if self.snapshots.is_none() {
            self.seen = stamp;
            self.changed = None;
        }
        let snapshots = self.snapshots.get_or_insert_with(|| snapshot::list(&root));
        let last = snapshots.iter().find(|s| s.kind == Kind::Major);
        ui.horizontal_wrapped(|ui| {
            match last {
                Some(v) => ui.label(RichText::new(format!("Last version: {}, {}", v.version.as_deref().unwrap_or(&v.label), ago(v.ts))).color(pal.muted)),
                None => ui.colored_label(ui.visuals().warn_fg_color, "No version saved yet"),
            };
            if ui.button("Snapshot now as version…").clicked() {
                self.naming = Some((None, String::new()));
            }
            if ui.add(egui::Button::new("Versions only").selected(self.versions_only)).clicked() {
                self.versions_only = !self.versions_only;
            }
            if ui.link("Retention settings…").clicked() {
                cx.open_settings(Section::History);
            }
        });
        ui.add_space(6.0);
        if snapshots.is_empty() {
            ui.weak("No snapshots yet. One is taken automatically before every import, edit, attach, copy or move.");
            return;
        }
        let shown: Vec<&Info> = snapshots.iter().filter(|s| !self.versions_only || s.kind == Kind::Major).collect();
        if self.selected.as_ref().is_some_and(|id| !shown.iter().any(|s| s.id == *id)) {
            self.selected = None;
        }
        if let Some(id) = &self.selected
            && self.changed.as_ref().is_none_or(|(c, _)| c != id)
        {
            self.changed = Some((id.clone(), snapshot::changes(&root, id).map_err(|e| format!("{e:#}"))));
        }
        let selected = self.selected.clone();
        let (mut act, mut pick) = (None, None);
        let (width, max) = ((ui.available_width() * 0.5).clamp(320.0, 460.0), ui.available_width());
        let list = egui::Panel::left("history_list").resizable(true).default_size(width).size_range(280.0..=(max - 260.0).max(280.0)).frame(egui::Frame::new().inner_margin(egui::Margin { right: 10, ..Default::default() })).show(ui, |ui| {
            ui.style_mut().interaction.selectable_labels = false; // so a click on any cell selects the row
            let cols = [Col::new("When", 80.0).no_sort(), Col::rest("Change").no_sort(), Col::new("", 62.0).no_sort(), Col::new("", 34.0).no_sort()];
            ui_kit::table(ui, "history", &cols, &mut Sort { col: usize::MAX, asc: false }, shown.len(), |i, row| {
                let s = shown[i];
                row.set_selected(selected.as_deref() == Some(s.id.as_str()));
                row.col(|ui| {
                    ui.label(RichText::new(ago(s.ts)).color(pal.muted));
                });
                row.col(|ui| {
                    if s.kind == Kind::Major {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("Version: {}", s.version.as_deref().unwrap_or("(unnamed)"))).strong().color(ui.visuals().selection.stroke.color));
                            ui.label(&s.label);
                        });
                    } else {
                        ui.label(&s.label);
                    }
                });
                row.col(|ui| {
                    let (text, level) = if s.kind == Kind::Major { ("Version", Level::Ok) } else { ("Auto", Level::Info) };
                    ui_kit::badge(ui, text, level);
                });
                row.col(|ui| {
                    ui.menu_button("⋯", |ui| row_menu(ui, s, &mut act)).response.on_hover_text("Actions for this snapshot");
                });
                let r = row.response();
                if r.clicked() {
                    pick = Some(s.id.clone());
                }
                r.context_menu(|ui| row_menu(ui, s, &mut act));
            });
        });
        ui.painter().line_segment([list.response.rect.right_top(), list.response.rect.right_bottom()], egui::Stroke::new(1.0, pal.line));
        ui.add_space(2.0);
        let current = selected.as_deref().and_then(|id| shown.iter().find(|s| s.id == id)).zip(self.changed.as_ref());
        match current {
            Some((s, (_, res))) => {
                let kind = match (s.kind, s.version.as_deref()) {
                    (Kind::Minor, _) => "Automatic".to_string(),
                    (_, Some(v)) => format!("Version {v}"),
                    _ => "Version".to_string(),
                };
                ui.label(RichText::new(&s.label).strong().size(16.0));
                ui.label(RichText::new(format!("{kind} · {} · {}", ago(s.ts), size(s.bytes))).color(pal.muted));
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if page_kit::primary(ui, true, "Roll back to before this").clicked() {
                        act = Some(Act::Roll((*s).clone()));
                    }
                    if s.kind == Kind::Major {
                        if ui.button("Make minor").clicked() {
                            act = Some(Act::Minor(s.id.clone()));
                        }
                    } else if ui.button("Mark as version…").clicked() {
                        act = Some(Act::Name(s.id.clone()));
                    }
                    if page_kit::danger(ui, "Delete…").clicked() {
                        act = Some(Act::Delete((*s).clone()));
                    }
                });
                ui.add_space(8.0);
                match res {
                    Ok(list) => {
                        page_kit::caption(ui, &format!("Changed since this snapshot · {} file{}", list.len(), if list.len() == 1 { "" } else { "s" }));
                        ui.add_space(2.0);
                        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                            for c in list.iter().take(300) {
                                ui.horizontal(|ui| {
                                    let (text, level) = match c.kind {
                                        ChangeKind::Added => ("added", Level::Ok),
                                        ChangeKind::Removed => ("removed", Level::Error),
                                        ChangeKind::Modified => ("modified", Level::Warn),
                                    };
                                    ui_kit::badge(ui, text, level);
                                    ui.label(RichText::new(&c.path).monospace());
                                });
                            }
                            if list.len() > 300 {
                                ui.weak(format!("…and {} more", list.len() - 300));
                            }
                            ui.add_space(12.0);
                            ui.label(RichText::new("Rolling back snapshots the current state first, so the rollback can itself be undone. Versions are never removed automatically.").color(pal.muted));
                        });
                    }
                    Err(e) => {
                        ui.colored_label(Level::Error.color(ui.visuals()), format!("Could not work out what changed: {e}"));
                    }
                }
            }
            None => {
                ui.weak("Click a row to see which files that change touched.");
            }
        }
        if pick.is_some() {
            self.selected = pick;
        }
        match act {
            Some(Act::Roll(s)) => self.confirm = Some(s),
            Some(Act::Delete(s)) => self.confirm_delete = Some(s),
            Some(Act::Name(id)) => self.naming = Some((Some(id), String::new())),
            Some(Act::Minor(id)) => {
                let r = snapshot::set_kind(&root, &id, Kind::Minor, None);
                self.done(r, cx);
            }
            None => {}
        }
    }

    /// Logs a failure and re-reads the list after any change to the snapshots.
    fn done(&mut self, r: anyhow::Result<()>, cx: &mut AppCtx) {
        if let Err(e) = r {
            cx.log.push(format!("Could not change the snapshot: {e:#}"));
        }
        self.snapshots = None;
    }

    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let ctx = cx.ctx.clone();
        let root = if self.scope == Scope::Global { cx.global_root() } else { cx.project_root() };
        if let (Some((id, mut text)), Some(root)) = (self.naming.clone(), root.clone()) {
            let (mut yes, mut no) = (false, false);
            egui::Modal::new(egui::Id::unique("version")).show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.heading("Mark as version");
                ui.label("A version is never removed automatically. Name it, e.g. \"Release 1\".");
                ui.text_edit_singleline(&mut text);
                ui.horizontal(|ui| {
                    yes = ui.add_enabled(!text.trim().is_empty(), egui::Button::new("Save version")).clicked();
                    no = ui.button("Cancel").clicked();
                });
            });
            if yes {
                let r = match &id {
                    Some(id) => snapshot::set_kind(&root, id, Kind::Major, Some(text.trim().into())),
                    None => snapshot::snapshot_as_version(&root, text.trim()),
                };
                self.done(r, cx);
            }
            self.naming = if yes || no { None } else { Some((id, text)) };
        }
        if let (Some(s), Some(root)) = (self.confirm_delete.clone(), root) {
            let (mut yes, mut no) = (false, false);
            egui::Modal::new(egui::Id::unique("delete_snapshot")).show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.heading("Delete this snapshot?");
                ui.label(format!("\"{}\" ({}) is removed for good{}.", s.label, ago(s.ts), if s.kind == Kind::Major { ", although it is a version" } else { "" }));
                ui.horizontal(|ui| {
                    yes = ui.button("Delete").clicked();
                    no = ui.button("Cancel").clicked();
                });
            });
            if yes {
                let r = snapshot::delete(&root, &s.id);
                self.done(r, cx);
            }
            if yes || no {
                self.confirm_delete = None;
            }
        }
        let Some(s) = self.confirm.clone() else { return };
        let (mut yes, mut no) = (false, false);
        egui::Modal::new(egui::Id::unique("rollback")).show(&ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("Roll back?");
            ui.label(format!("This puts the library and KiCad's library list back as they were before \"{}\" ({}), undoing it and every change after it.", s.label, ago(s.ts)));
            ui.label(RichText::new("The current state is snapshotted first, so you can undo the rollback.").weak());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                yes = ui.button("Roll back").clicked();
                no = ui.button("Cancel").clicked();
            });
        });
        if yes {
            // With KiCad open the restored tables are re-checked once it closes.
            cx.send(AppMsg::History(Msg::Rollback { scope: self.scope, id: s.id }));
        }
        if yes || no {
            self.confirm = None;
        }
    }
}
