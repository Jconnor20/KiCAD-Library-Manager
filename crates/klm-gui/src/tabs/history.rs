use crate::{AppCtx, Msg as AppMsg};
use eframe::egui::{self, RichText};
use klm_core::library::Scope;
use klm_core::settings::AutoVersion;
use klm_core::snapshot::{self, Info, Kind};

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
    retention: bool,
}

impl Default for HistoryTab {
    fn default() -> Self {
        Self { scope: Scope::Global, snapshots: None, seen: None, confirm: None, confirm_delete: None, naming: None, versions_only: false, retention: false }
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
        ui.horizontal(|ui| {
            for (scope, name) in [(Scope::Global, "Global"), (Scope::Project, "Project")] {
                if ui.selectable_value(&mut self.scope, scope, name).clicked() {
                    self.snapshots = None;
                }
            }
            if ui.button("Refresh").clicked() {
                self.snapshots = None;
            }
        });
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
        }
        let snapshots = self.snapshots.get_or_insert_with(|| snapshot::list(&root));
        let last = snapshots.iter().find(|s| s.kind == Kind::Major);
        ui.horizontal(|ui| {
            match last {
                Some(v) => ui.label(format!("Last version: {}, {}", v.version.as_deref().unwrap_or(&v.label), ago(v.ts))),
                None => ui.colored_label(ui.visuals().warn_fg_color, "No version saved yet"),
            };
            if ui.button("Snapshot now as version…").clicked() {
                self.naming = Some((None, String::new()));
            }
            ui.checkbox(&mut self.versions_only, "Versions only");
            if ui.button("Retention…").clicked() {
                self.retention = !self.retention;
            }
        });
        if self.retention {
            let h = &mut cx.settings.history;
            let before = h.clone();
            ui.horizontal(|ui| {
                ui.label("Keep last");
                ui.add(egui::DragValue::new(&mut h.keep_minors).range(1..=500));
                ui.label("automatic snapshots (versions are never removed). Size cap in MB, 0 = none:");
                let mut cap = h.max_minor_mb.unwrap_or(0);
                ui.add(egui::DragValue::new(&mut cap).range(0..=100_000));
                h.max_minor_mb = (cap > 0).then_some(cap);
                egui::ComboBox::from_label("Make a version automatically").selected_text(format!("{:?}", h.auto_version)).show_ui(ui, |ui| {
                    for v in [AutoVersion::Off, AutoVersion::Daily, AutoVersion::Weekly, AutoVersion::Monthly] {
                        ui.selectable_value(&mut h.auto_version, v, format!("{v:?}"));
                    }
                });
            });
            if *h != before {
                snapshot::set_policy(h);
                if let Err(e) = cx.settings.save() {
                    cx.log.push(format!("Could not save settings: {e:#}"));
                }
            }
        }
        if snapshots.is_empty() {
            ui.weak("No snapshots yet. One is taken automatically before every import, edit, attach, copy or move.");
            return;
        }
        let (mut pick, mut delete, mut kind) = (None, None, None);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("history").num_columns(5).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                for h in ["When", "Change", "Size", "", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for s in snapshots.iter().filter(|s| !self.versions_only || s.kind == Kind::Major) {
                    ui.label(ago(s.ts));
                    ui.horizontal(|ui| {
                        if s.kind == Kind::Major {
                            ui.label(RichText::new(format!("Version: {}", s.version.as_deref().unwrap_or("(unnamed)"))).strong().color(ui.visuals().selection.stroke.color));
                        }
                        ui.label(&s.label);
                    });
                    ui.label(size(s.bytes));
                    if ui.button("Roll back to before this").clicked() {
                        pick = Some(s.clone());
                    }
                    ui.horizontal(|ui| {
                        if s.kind == Kind::Major {
                            if ui.button("Make minor").clicked() {
                                kind = Some(s.id.clone());
                            }
                        } else if ui.button("Mark as version…").clicked() {
                            self.naming = Some((Some(s.id.clone()), String::new()));
                        }
                        if ui.button("Delete").clicked() {
                            delete = Some(s.clone());
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if pick.is_some() {
            self.confirm = pick;
        }
        if delete.is_some() {
            self.confirm_delete = delete;
        }
        if let Some(id) = kind {
            let r = snapshot::set_kind(&root, &id, Kind::Minor, None);
            self.done(r, cx);
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
            // KiCad's tables are restored too, and KiCad writes them back over outside edits.
            cx.when_kicad_closed(AppMsg::History(Msg::Rollback { scope: self.scope, id: s.id }));
        }
        if yes || no {
            self.confirm = None;
        }
    }
}
