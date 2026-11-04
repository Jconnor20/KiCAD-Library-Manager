//! The dialogs shared by every batch: the dry-run review, the progress while a worker applies it, and
//! the "Import all" setup. A tab keeps an `Option<BatchDialog>` and calls these from `dialogs`/`handle`.

use crate::{AppCtx, Msg as AppMsg};
use eframe::egui::{self, RichText};
use klm_core::batch::{self, ItemResult, Op, Plan, Target};
use klm_core::part::Part;

/// Sent by the worker that applies a plan.
pub enum Progress {
    Step(usize, usize),
    Done(Vec<ItemResult>),
}

pub enum BatchDialog {
    /// The dry run, waiting for Apply.
    Review(Plan),
    Running { done: usize, total: usize },
}

/// Shows the dialog and returns the plan once the user presses Apply (the dialog is then gone).
pub fn show(batch: &mut Option<BatchDialog>, ctx: &egui::Context) -> Option<Plan> {
    let dialog = batch.as_ref()?;
    let (mut apply, mut close) = (false, false);
    let modal = egui::Modal::new(egui::Id::unique("batch_dialog")).show(ctx, |ui| {
        ui.set_width(520.0);
        match dialog {
            BatchDialog::Running { done, total } => {
                ui.heading("Applying…");
                ui.add(egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32).text(format!("{done} of {total}")));
            }
            BatchDialog::Review(plan) => {
                ui.heading("This will change:");
                if plan.items.is_empty() {
                    ui.label("Nothing to do.");
                }
                egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                    for item in &plan.items {
                        ui.add_space(4.0);
                        ui.label(RichText::new(&item.label).strong());
                        for c in &item.changes {
                            ui.label(format!("    {c}"));
                        }
                        for w in &item.warnings {
                            ui.colored_label(ui.visuals().warn_fg_color, format!("    {w}"));
                        }
                        if let Some(e) = &item.error {
                            ui.colored_label(ui.visuals().error_fg_color, format!("    can't be done: {e}"));
                        }
                    }
                });
                for w in &plan.warnings {
                    ui.colored_label(ui.visuals().warn_fg_color, w);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    apply = ui.add_enabled(plan.items.iter().any(|i| i.error.is_none()), egui::Button::new("Apply")).clicked();
                    close = ui.button(if plan.items.is_empty() { "Close" } else { "Cancel" }).clicked();
                });
            }
        }
    });
    if matches!(dialog, BatchDialog::Review(_)) && (close || modal.should_close()) {
        *batch = None;
    } else if apply {
        if let Some(BatchDialog::Review(plan)) = batch.take() {
            return Some(plan);
        }
    }
    None
}

/// The dry-run dialog for work that isn't a `batch::Plan` (standardise, restyle, export): a heading per item with
/// its lines under it, and whatever `extra` adds above the buttons. `Some(true)` is the `confirm` button ("Apply", "Delete"), `Some(false)` Cancel.
pub fn review(ctx: &egui::Context, id: &str, confirm: &str, title: &str, sections: &[(String, Vec<String>)], can_apply: bool, extra: impl FnOnce(&mut egui::Ui)) -> Option<bool> {
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::NULL.with(id)).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(title);
        if sections.is_empty() {
            ui.label("Nothing to do.");
        }
        egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
            for (label, lines) in sections {
                ui.add_space(4.0);
                ui.label(RichText::new(label).strong());
                for l in lines {
                    ui.label(format!("    {l}"));
                }
            }
        });
        extra(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(can_apply, egui::Button::new(confirm)).clicked() {
                answer = Some(true);
            }
            if ui.button(if can_apply { "Cancel" } else { "Close" }).clicked() {
                answer = Some(false);
            }
        });
    });
    answer.or(modal.should_close().then_some(false))
}

/// Applies `plan` on a worker, which reports back as `wrap(Progress)`.
pub fn start(batch: &mut Option<BatchDialog>, plan: Plan, cx: &mut AppCtx, wrap: fn(Progress) -> AppMsg) {
    cx.busy = true;
    *batch = Some(BatchDialog::Running { done: 0, total: plan.items.len() });
    let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
    std::thread::spawn(move || {
        let results = batch::apply(&plan, |done, total| {
            let _ = tx.send(wrap(Progress::Step(done, total)));
            ctx.request_repaint();
        });
        let _ = tx.send(wrap(Progress::Done(results)));
        ctx.request_repaint();
    });
}

/// Updates the progress bar; when the worker is done, closes the dialog, logs each result and returns them.
pub fn progress(batch: &mut Option<BatchDialog>, p: Progress, cx: &mut AppCtx) -> Option<Vec<ItemResult>> {
    match p {
        Progress::Step(done, total) => {
            *batch = Some(BatchDialog::Running { done, total });
            None
        }
        Progress::Done(results) => {
            *batch = None;
            cx.busy = false;
            for r in &results {
                cx.log.extend(r.notes.iter().cloned());
                if let Some(e) = &r.error {
                    cx.log.push(format!("{} failed: {e}", r.label));
                }
            }
            let failed = results.iter().filter(|r| r.error.is_some()).count();
            cx.log.push(format!("Batch finished: {} done, {failed} failed", results.len() - failed));
            Some(results)
        }
    }
}

/// "Import all" setup: one shared category and target, which each pending part can override.
pub struct ImportAllSetup {
    parts: Vec<Part>,
    /// Per part: category override (empty = use the shared one) and target override.
    rows: Vec<(String, Option<Target>)>,
    categories: Vec<String>,
    targets: Vec<Target>,
    category: String,
    target: Target,
    overwrite: bool,
}

pub enum SetupAction {
    None,
    Cancel,
    /// The ops to plan, one per part in the order given to [`ImportAllSetup::new`].
    Preview(Vec<Op>),
}

fn target_name(t: Target) -> &'static str {
    match t {
        Target::Global => "Global",
        Target::Project => "Project",
        Target::Both => "Both",
    }
}

impl ImportAllSetup {
    pub fn new(parts: Vec<Part>, categories: Vec<String>, has_global: bool, has_project: bool) -> Self {
        let targets: Vec<_> = [(has_global, Target::Global), (has_project, Target::Project), (has_global && has_project, Target::Both)]
            .into_iter()
            .filter_map(|(ok, t)| ok.then_some(t))
            .collect();
        Self {
            rows: vec![(String::new(), None); parts.len()],
            parts,
            categories,
            target: targets.first().copied().unwrap_or(Target::Global),
            targets,
            category: String::new(),
            overwrite: false,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> SetupAction {
        let mut action = SetupAction::None;
        let modal = egui::Modal::new(egui::Id::unique("import_all_setup")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("Import all");
            egui::Grid::new("import_all_shared").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Category");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("import_all_category").selected_text(&self.category).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.category, String::new(), "(guess for each part)");
                        for c in &self.categories {
                            ui.selectable_value(&mut self.category, c.clone(), c);
                        }
                    });
                    ui.add(egui::TextEdit::singleline(&mut self.category).hint_text("or type a new one").desired_width(140.0));
                });
                ui.end_row();
                ui.label("Add to");
                ui.horizontal(|ui| {
                    for t in &self.targets {
                        ui.selectable_value(&mut self.target, *t, target_name(*t));
                    }
                });
                ui.end_row();
            });
            ui.checkbox(&mut self.overwrite, "Replace parts that are already there");
            ui.add_space(6.0);
            ui.label("Per part (leave empty to use the choices above):");
            egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("import_all_rows").striped(true).num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
                    for (part, (category, target)) in self.parts.iter().zip(&mut self.rows) {
                        ui.strong(&part.mpn);
                        ui.add(egui::TextEdit::singleline(category).hint_text("category").desired_width(150.0));
                        egui::ComboBox::from_id_salt(&part.mpn).selected_text(target.map_or("Shared", target_name)).show_ui(ui, |ui| {
                            ui.selectable_value(target, None, "Shared");
                            for t in &self.targets {
                                ui.selectable_value(target, Some(*t), target_name(*t));
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.targets.is_empty(), egui::Button::new("Preview…")).clicked() {
                    let ops = self.parts.iter().zip(&self.rows).map(|(part, (category, target))| {
                        let category = Some(if category.trim().is_empty() { self.category.clone() } else { category.clone() });
                        Op::ImportPart { part: part.clone(), category, target: target.unwrap_or(self.target), overwrite: self.overwrite }
                    });
                    action = SetupAction::Preview(ops.collect());
                }
                if ui.button("Cancel").clicked() {
                    action = SetupAction::Cancel;
                }
            });
        });
        if modal.should_close() && matches!(action, SetupAction::None) {
            action = SetupAction::Cancel;
        }
        action
    }
}
