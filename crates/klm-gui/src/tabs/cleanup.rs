use super::page_kit::{primary, tile};
use super::ui_kit::{self, Col, Level, Sort};
use crate::{AppCtx, theme};
use eframe::egui::{self, RichText};
use klm_core::cleanup::{self, Action, Entry, Issue, IssueKind, Kind, Move, Report};
use klm_core::kicad_env;
use std::path::PathBuf;

/// Standards · KiCad tables: scans KiCad's own library tables for duplicates, broken entries and unused libraries, and
/// lets the user queue "remove from table", "fix path" and "move into KLM" per library. Nothing is written until
/// the preview of the queued changes is applied; library files are never deleted.
#[derive(Default)]
pub struct CleanupTab {
    /// Extra project folders to search for references, besides the open project.
    projects: Vec<PathBuf>,
    report: Option<Report>,
    queue: Vec<Action>,
    mv: Option<(String, Move)>,
    /// Preview of `queue` (lines, warnings) or why it can't be applied; recomputed when the queue changes, not every frame.
    preview: Option<Result<(Vec<String>, Vec<String>), String>>,
}

const GROUPS: [(IssueKind, &str); 4] =
    [(IssueKind::Broken, "Broken"), (IssueKind::Unused, "Maybe unused"), (IssueKind::Duplicate, "Duplicate"), (IssueKind::Unverifiable, "Can't check")];

fn same(a: IssueKind, b: IssueKind) -> bool {
    std::mem::discriminant(&a) == std::mem::discriminant(&b)
}

/// A symbol library on disk that could be copied into KLM.
fn movable(e: &Entry) -> bool {
    e.kind == Kind::Symbol && e.path.as_ref().is_some_and(|p| p.is_file())
}

impl CleanupTab {
    fn refresh_preview(&mut self) {
        self.preview = match (&self.report, self.queue.is_empty()) {
            (Some(r), false) => Some(cleanup::plan(r, &self.queue).map(|p| (p.lines, p.warnings)).map_err(|e| format!("{e:#}"))),
            _ => None,
        };
    }

    fn scan(&mut self, cx: &mut AppCtx) {
        self.queue.clear();
        self.preview = None;
        let Ok(config) = cx.kicad_config.clone() else {
            cx.log.push("KiCad's config folder wasn't found, so there are no library tables to scan.".into());
            return;
        };
        let dirs: Vec<PathBuf> = cx.project.iter().chain(&self.projects).cloned().collect();
        self.report = Some(cleanup::scan(&config, &dirs));
        self.refresh_preview();
    }

    fn apply(&mut self, cx: &mut AppCtx) {
        let Some(report) = &self.report else { return };
        match cleanup::plan(report, &self.queue).and_then(|p| cleanup::apply(&p, true)) {
            Ok(backups) => {
                cx.log.push(format!(
                    "Cleaned up the library tables; backups: {}",
                    backups.iter().map(|b| b.display().to_string()).collect::<Vec<_>>().join(", ")
                ));
                self.scan(cx);
            }
            Err(e) => cx.log.push(format!("Nothing was changed: {e:#}")),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let scanned = self.report.is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new("KiCad library tables").heading().strong());
            if let Some(r) = &self.report {
                ui.weak(format!("{} libraries · {} project(s) searched", r.entries.len(), r.projects.len()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary(ui, true, if scanned { "Scan again" } else { "Scan KiCad's library tables" }).clicked() {
                    self.scan(cx);
                }
                if ui.button("Add project folder…").on_hover_text("Projects whose files are searched to see which libraries are used").clicked()
                    && let Some(d) = rfd::FileDialog::new().pick_folder()
                {
                    self.projects.push(d);
                }
            });
        });
        let mut drop = None;
        for (i, p) in self.projects.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(p.display().to_string()).monospace());
                if ui.small_button("x").clicked() {
                    drop = Some(i);
                }
            });
        }
        if let Some(i) = drop {
            self.projects.remove(i);
        }
        ui.weak("Shows what is wrong in your global and project library tables. \"Maybe unused\" only means none of the chosen projects use a library; check your other projects before removing it.");
        ui.add_space(4.0);
        let (mut apply, mut discard) = (false, false);
        if !self.queue.is_empty() {
            let pal = theme::palette(ui.visuals());
            egui::Frame::new().fill(pal.panel).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.strong("Changes to apply");
                    ui.weak(format!("{} fix(es) staged", self.queue.len()));
                });
                match &self.preview {
                    Some(Ok((lines, warnings))) => {
                        lines.iter().for_each(|l| {
                            ui.label(l);
                        });
                        warnings.iter().for_each(|w| {
                            ui.colored_label(Level::Warn.color(ui.visuals()), w);
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(Level::Error.color(ui.visuals()), e);
                    }
                    None => {}
                }
                let running = kicad_env::kicad_running();
                if running {
                    ui.colored_label(Level::Warn.color(ui.visuals()), "KiCad is open and would overwrite these changes. Close it to apply.");
                }
                ui.horizontal(|ui| {
                    ui.weak("Only table entries change; library files stay. A backup of each table is made first.");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        apply = primary(ui, !running, &format!("Apply {} fixes", self.queue.len())).clicked();
                        discard = ui.button("Discard").clicked();
                    });
                });
            });
        }
        if apply {
            self.apply(cx);
        }
        if discard {
            self.queue.clear();
            self.preview = None;
        }
        let Some(report) = &self.report else { return };
        for e in &report.errors {
            ui.colored_label(Level::Error.color(ui.visuals()), e);
        }
        for c in &report.other_configs {
            ui.weak(format!("Not scanned (another KiCad version): {}", c.display()));
        }
        let rows: Vec<(usize, bool, &Issue)> =
            (0..report.entries.len()).flat_map(|i| report.issues_of(i).enumerate().map(move |(n, x)| (i, n == 0, x))).collect();
        let of = |k: IssueKind| rows.iter().filter(|r| same(r.2.kind, k)).count();
        let outside = report.entries.iter().filter(|e| movable(e)).count();
        let pal = theme::palette(ui.visuals());
        let (err, warn) = (Level::Error.color(ui.visuals()), Level::Warn.color(ui.visuals()));
        ui.columns(4, |c| {
            tile(&mut c[0], "Broken", &of(IssueKind::Broken).to_string(), "path doesn't exist", (of(IssueKind::Broken) > 0).then_some(err));
            tile(&mut c[1], "Maybe unused", &of(IssueKind::Unused).to_string(), "no chosen project uses it", (of(IssueKind::Unused) > 0).then_some(warn));
            tile(&mut c[2], "Outside KLM", &outside.to_string(), "could be moved in", None);
            tile(&mut c[3], "Can't check", &of(IssueKind::Unverifiable).to_string(), "set by KiCad itself", None);
        });
        ui.add_space(4.0);
        // A header row per kind of problem, then that kind's rows.
        let mut entries: Vec<Result<(usize, bool, &Issue), (&str, usize)>> = vec![];
        for (k, name) in GROUPS {
            let group: Vec<_> = rows.iter().filter(|r| same(r.2.kind, k)).collect();
            if !group.is_empty() {
                entries.push(Err((name, group.len())));
                entries.extend(group.into_iter().map(|r| Ok(*r)));
            }
        }
        let mut want: Option<Action> = None;
        let mut undo: Option<usize> = None;
        let mut move_of: Option<usize> = None;
        let cols = [Col::new("Library", 170.0).no_sort(), Col::new("Type", 80.0).no_sort(), Col::rest("Detail").no_sort(), Col::new("Fix", 300.0).no_sort()];
        ui_kit::table(ui, "cleanup", &cols, &mut Sort { col: usize::MAX, asc: true }, entries.len(), |n, row| {
            let (i, _, x) = match entries[n] {
                Err((name, count)) => {
                    row.col(|ui| {
                        ui.label(RichText::new(format!("{name} · {count}")).strong().color(pal.fg));
                    });
                    for _ in 1..cols.len() {
                        row.col(|_| {});
                    }
                    return;
                }
                Ok(r) => r,
            };
            let e = &report.entries[i];
            row.col(|ui| {
                ui.strong(&e.name).on_hover_text(format!("{} in {}", e.uri, e.table.display()));
            });
            row.col(|ui| {
                ui.weak(if e.kind == Kind::Symbol { "Symbol" } else { "Footprint" });
            });
            row.col(|ui| {
                ui.label(RichText::new(&x.message).monospace().size(12.0)).on_hover_text(&x.message);
            });
            row.col(|ui| {
                ui.horizontal(|ui| {
                    if let Some(why) = e.locked {
                        ui.weak("Locked").on_hover_text(format!("Can't be changed here: {why}"));
                        return;
                    }
                    let queued = self.queue.iter().position(
                        |a| matches!(a, Action::Remove { table, name } | Action::FixPath { table, name, .. } if *table == e.table && *name == e.name),
                    );
                    if let Some(q) = queued {
                        let text = match &self.queue[q] {
                            Action::FixPath { new_path, .. } => format!("Will point to {}", new_path.display()),
                            _ => "Will remove".into(),
                        };
                        ui_kit::badge(ui, &text, Level::Ok);
                        if ui.small_button("Undo").clicked() {
                            undo = Some(q);
                        }
                        return;
                    }
                    if ui.small_button("Remove from table").on_hover_text("Only the table entry goes; the library's files stay").clicked() {
                        want = Some(Action::Remove { table: e.table.clone(), name: e.name.clone() });
                    }
                    if ui.small_button("Fix path…").clicked() {
                        let picked = if e.kind == Kind::Symbol {
                            rfd::FileDialog::new().add_filter("Symbol library", &["kicad_sym", "lib"]).pick_file()
                        } else {
                            rfd::FileDialog::new().pick_folder()
                        };
                        if let Some(new_path) = picked {
                            want = Some(Action::FixPath { table: e.table.clone(), name: e.name.clone(), new_path });
                        }
                    }
                    if movable(e) && ui.small_button("Move into KLM…").clicked() {
                        move_of = Some(i);
                    }
                });
            });
        });
        if let Some(q) = undo {
            self.queue.remove(q);
            self.refresh_preview();
        }
        if let Some(a) = want {
            self.queue.push(a);
            self.refresh_preview();
        }
        if let Some(i) = move_of {
            match (cx.global_root(), &self.report) {
                (Some(g), Some(report)) => {
                    let e = &report.entries[i];
                    match cleanup::plan_move(report, e, &cx.settings.rules, &g) {
                        Ok(m) => self.mv = Some((e.name.clone(), m)),
                        Err(err) => cx.log.push(format!("Can't move {}: {err:#}", e.name)),
                    }
                }
                _ => cx.log.push("Set up a global library in Settings first.".into()),
            }
        }
    }

    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let Some((name, mv)) = &self.mv else { return };
        let mut close = false;
        egui::Window::new(format!("Move {name} into KLM")).collapsible(false).show(&cx.ctx.clone(), |ui| {
            ui.label(mv.plan.to_string());
            for c in &mv.caveats {
                ui.colored_label(Level::Warn.color(ui.visuals()), c);
            }
            ui.weak(
                "This copies the library into KLM. The old table entry and its files stay as they are; remove the entry afterwards if the copy is complete.",
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!kicad_env::kicad_running(), egui::Button::new("Import"))
                    .on_disabled_hover_text("KiCad is open; close it so the new libraries can be linked")
                    .clicked()
                {
                    for r in mv.apply() {
                        cx.log.push(match r.error {
                            Some(e) => format!("{}: {e}", r.label),
                            None => format!("{}: {}", r.label, r.notes.join("; ")),
                        });
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.mv = None;
        }
    }
}
