use crate::AppCtx;
use eframe::egui::{self, RichText};
use klm_core::cleanup::{self, Action, Entry, IssueKind, Kind, Move, Report};
use klm_core::kicad_env;
use std::path::PathBuf;

/// Scans KiCad's own library tables for duplicates, broken entries and unused libraries, and lets the
/// user queue "remove from table", "fix path" and "move into KLM" per library. Nothing is written until
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

fn tag(e: &Entry) -> String {
    format!("{}: {}", if e.kind == Kind::Symbol { "symbol" } else { "footprint" }, e.name)
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
                cx.log.push(format!("Cleaned up the library tables; backups: {}", backups.iter().map(|b| b.display().to_string()).collect::<Vec<_>>().join(", ")));
                self.scan(cx);
            }
            Err(e) => cx.log.push(format!("Nothing was changed: {e:#}")),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        ui.horizontal(|ui| {
            if ui.button("Scan KiCad's library tables").clicked() {
                self.scan(cx);
            }
            if ui.button("Add project folder…").on_hover_text("Projects whose files are searched to see which libraries are used").clicked()
                && let Some(d) = rfd::FileDialog::new().pick_folder()
            {
                self.projects.push(d);
            }
        });
        let mut drop = None;
        for (i, p) in self.projects.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(p.display().to_string());
                if ui.small_button("x").clicked() {
                    drop = Some(i);
                }
            });
        }
        if let Some(i) = drop {
            self.projects.remove(i);
        }
        ui.weak("Shows what is wrong in your global and project library tables. \"Unused\" only means none of the chosen projects use a library; check your other projects before removing it.");
        let Some(report) = &self.report else { return };
        let mut want: Option<Action> = None;
        let mut move_of: Option<usize> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for e in &report.errors {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            for c in &report.other_configs {
                ui.weak(format!("Not scanned (another KiCad version): {}", c.display()));
            }
            ui.weak(format!("{} libraries, {} project(s) searched", report.entries.len(), report.projects.len()));
            for (i, e) in report.entries.iter().enumerate() {
                let issues: Vec<_> = report.issues_of(i).collect();
                if issues.is_empty() {
                    continue;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong(tag(e));
                    ui.weak(format!("{} in {}", e.uri, e.table.display()));
                });
                for x in issues {
                    let (label, bad) = match x.kind {
                        IssueKind::Broken => ("Broken", true),
                        IssueKind::Unverifiable => ("Can't check", false),
                        IssueKind::Unused => ("Unused?", false),
                        IssueKind::Duplicate => ("Duplicate", false),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(label).color(if bad { ui.visuals().error_fg_color } else { ui.visuals().warn_fg_color }));
                        ui.label(&x.message);
                    });
                }
                if let Some(why) = e.locked {
                    ui.weak(format!("Can't be changed here: {why}"));
                    continue;
                }
                let queued = self.queue.iter().any(|a| matches!(a, Action::Remove { table, name } | Action::FixPath { table, name, .. } if *table == e.table && *name == e.name));
                ui.horizontal(|ui| {
                    if ui.add_enabled(!queued, egui::Button::new("Remove from table")).on_hover_text("Only the table entry goes; the library's files stay").clicked() {
                        want = Some(Action::Remove { table: e.table.clone(), name: e.name.clone() });
                    }
                    if ui.add_enabled(!queued, egui::Button::new("Fix path…")).clicked() {
                        let picked = if e.kind == Kind::Symbol { rfd::FileDialog::new().add_filter("Symbol library", &["kicad_sym", "lib"]).pick_file() } else { rfd::FileDialog::new().pick_folder() };
                        if let Some(new_path) = picked {
                            want = Some(Action::FixPath { table: e.table.clone(), name: e.name.clone(), new_path });
                        }
                    }
                    if e.kind == Kind::Symbol && e.path.as_ref().is_some_and(|p| p.is_file()) && ui.button("Move into KLM…").clicked() {
                        move_of = Some(i);
                    }
                    if queued {
                        ui.weak("queued");
                    }
                });
            }
        });
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
        if !self.queue.is_empty() {
            ui.separator();
            ui.strong("Changes to apply");
            match &self.preview {
                Some(Ok((lines, warnings))) => {
                    lines.iter().for_each(|l| {
                        ui.label(l);
                    });
                    warnings.iter().for_each(|w| {
                        ui.colored_label(ui.visuals().warn_fg_color, w);
                    });
                }
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                None => {}
            }
            let running = kicad_env::kicad_running();
            if running {
                ui.colored_label(ui.visuals().warn_fg_color, "KiCad is open and would overwrite these changes. Close it to apply.");
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!running, egui::Button::new("Apply (a backup of each table is made first)")).clicked() {
                    self.apply(cx);
                }
                if ui.button("Discard").clicked() {
                    self.queue.clear();
                    self.preview = None;
                }
            });
        }
    }

    pub fn dialogs(&mut self, cx: &mut AppCtx) {
        let Some((name, mv)) = &self.mv else { return };
        let mut close = false;
        egui::Window::new(format!("Move {name} into KLM")).collapsible(false).show(&cx.ctx.clone(), |ui| {
            ui.label(mv.plan.to_string());
            for c in &mv.caveats {
                ui.colored_label(ui.visuals().warn_fg_color, c);
            }
            ui.weak("This copies the library into KLM. The old table entry and its files stay as they are; remove the entry afterwards if the copy is complete.");
            ui.horizontal(|ui| {
                if ui.add_enabled(!kicad_env::kicad_running(), egui::Button::new("Import")).on_disabled_hover_text("KiCad is open; close it so the new libraries can be linked").clicked() {
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
