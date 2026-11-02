use crate::{AppCtx, Msg as AppMsg};
use eframe::egui::{self, RichText};
use klm_core::library::Scope;
use klm_core::snapshot::{self, Info};

pub enum Msg {
    /// Put the library back as it was when snapshot `id` was taken.
    Rollback { scope: Scope, id: String },
}

/// Snapshots of the global or project library, newest first, each with a "Roll back to before this" button.
pub struct HistoryTab {
    scope: Scope,
    /// Cached listing; `None` means re-read from disk on the next frame.
    snapshots: Option<Vec<Info>>,
    /// Snapshot waiting for the user to confirm the rollback.
    confirm: Option<Info>,
    /// When `ui` last ran; a gap means the tab was left and other tabs may have changed the library.
    last_shown: Option<std::time::Instant>,
}

impl Default for HistoryTab {
    fn default() -> Self {
        Self { scope: Scope::Global, snapshots: None, confirm: None, last_shown: None }
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
        if self.last_shown.is_none_or(|t| t.elapsed().as_millis() > 500) {
            self.snapshots = None;
        }
        self.last_shown = Some(std::time::Instant::now());
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
        let snapshots = self.snapshots.get_or_insert_with(|| snapshot::list(&root));
        if snapshots.is_empty() {
            ui.weak("No snapshots yet. One is taken automatically before every import, edit, attach, copy or move.");
            return;
        }
        let mut pick = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("history").num_columns(4).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                for h in ["When", "Change", "Size", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for s in snapshots.iter() {
                    ui.label(ago(s.ts));
                    ui.label(&s.label);
                    ui.label(size(s.bytes));
                    if ui.button("Roll back to before this").clicked() {
                        pick = Some(s.clone());
                    }
                    ui.end_row();
                }
            });
        });
        if pick.is_some() {
            self.confirm = pick;
        }
    }

    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let Some(s) = self.confirm.clone() else { return };
        let (mut yes, mut no) = (false, false);
        let ctx = cx.ctx.clone();
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
