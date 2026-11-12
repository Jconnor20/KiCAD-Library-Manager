//! The dialogs shared by every batch: the dry-run review, the progress while a worker applies it, and
//! the "Import all" setup. A tab keeps an `Option<BatchDialog>` and calls these from `dialogs`/`handle`.

use crate::{AppCtx, Msg as AppMsg};
use super::ui_kit::{Level, badge};
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

/// A modal in the wireframes' shape: a title row (with `sub` beside it and a close ✕), the body, then a footer bar.
/// True when the user dismissed it with ✕, Esc or a click outside.
pub fn dialog(ctx: &egui::Context, id: egui::Id, width: f32, title: &str, sub: Option<&str>, body: impl FnOnce(&mut egui::Ui), footer: impl FnOnce(&mut egui::Ui)) -> bool {
    let line = crate::theme::palette(&ctx.global_style().visuals).line;
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(0);
    let mut closed = false;
    let modal = egui::Modal::new(id).frame(frame).show(ctx, |ui| {
        ui.set_width(width.min(ctx.content_rect().width() - 40.0));
        let pad = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                add(ui)
            });
        };
        pad(ui, &mut |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).heading().strong());
                if let Some(sub) = sub {
                    ui.weak(sub);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let x = ui.add(super::ui_kit::ghost("✕"));
                    x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close dialog"));
                    closed = x.clicked();
                });
            });
        });
        ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, line));
        let mut body = Some(body);
        pad(ui, &mut |ui| (body.take().unwrap())(ui));
        ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, line));
        let mut footer = Some(footer);
        pad(ui, &mut |ui| (footer.take().unwrap())(ui));
    });
    closed || modal.should_close()
}

/// How a line of a review reads: the verb it starts with decides its chip.
#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Plain,
    Warn,
    Error,
}

/// The chip for a line, from its first word; only the verbs whose meaning is certain get one.
fn chip_for(text: &str, tone: Tone) -> Option<(&'static str, Level)> {
    match tone {
        Tone::Warn => return Some(("Warning", Level::Warn)),
        Tone::Error => return Some(("Can't", Level::Error)),
        Tone::Plain => {}
    }
    match text.split_whitespace().next()?.to_lowercase().as_str() {
        "remove" | "delete" => Some(("Delete", Level::Error)),
        "keep" => Some(("Keep", Level::Ok)),
        "add" => Some(("Add", Level::Ok)),
        "replace" => Some(("Replace", Level::Warn)),
        "set" => Some(("Set", Level::Info)),
        "can't" => Some(("Can't", Level::Error)),
        _ => None,
    }
}

/// The list every review shows: a shaded heading per item, then a row per line with its chip.
fn review_sections(ui: &mut egui::Ui, sections: &[(String, Vec<(String, Tone)>)]) {
    let pal = crate::theme::palette(ui.visuals());
    for (label, lines) in sections {
        egui::Frame::new().fill(pal.panel).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(label).monospace().strong());
        });
        for (text, tone) in lines {
            ui.horizontal_top(|ui| {
                ui.add_space(10.0);
                ui.allocate_ui_with_layout(egui::vec2(64.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if let Some((chip, level)) = chip_for(text, *tone) {
                        badge(ui, chip, level);
                    }
                });
                ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0)).wrap());
            });
        }
    }
}

/// Cancel and the confirm button, right-aligned. `Some(true)` confirm, `Some(false)` cancel.
fn buttons(ui: &mut egui::Ui, cancel: &str, confirm: egui::Button<'static>, can: bool) -> Option<bool> {
    let mut answer = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled(can, confirm).clicked() {
            answer = Some(true);
        }
        if ui.button(cancel).clicked() {
            answer = Some(false);
        }
    });
    answer
}

/// Shows the dialog and returns the plan once the user presses Apply (the dialog is then gone).
pub fn show(batch: &mut Option<BatchDialog>, ctx: &egui::Context) -> Option<Plan> {
    let state = batch.as_ref()?;
    let (mut apply, mut close) = (false, false);
    let id = egui::Id::NULL.with("batch_dialog");
    let dismissed = match state {
        BatchDialog::Running { done, total } => dialog(
            ctx,
            id,
            420.0,
            "Applying…",
            None,
            |ui| {
                ui.add(egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32).text(format!("{done} of {total}")));
            },
            |ui| {
                ui.weak("Please wait until this finishes.");
            },
        ),
        BatchDialog::Review(plan) => {
            let sections: Vec<_> = plan
                .items
                .iter()
                .map(|item| {
                    let lines = item.changes.iter().map(|c| (c.to_string(), Tone::Plain)).chain(item.warnings.iter().map(|w| (w.to_string(), Tone::Warn)));
                    let error = item.error.iter().map(|e| (format!("can't be done: {e}"), Tone::Error));
                    (item.label.clone(), lines.chain(error).collect())
                })
                .collect();
            let can = plan.items.iter().any(|i| i.error.is_none());
            let cancel = if plan.items.is_empty() { "Close" } else { "Cancel" };
            dialog(
                ctx,
                id,
                640.0,
                "This will change:",
                None,
                |ui| {
                    if plan.items.is_empty() {
                        ui.label("Nothing to do.");
                    }
                    egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| review_sections(ui, &sections));
                    for w in &plan.warnings {
                        ui.colored_label(ui.visuals().warn_fg_color, w);
                    }
                },
                |ui| match buttons(ui, cancel, super::ui_kit::primary("Apply", ui.visuals()), can) {
                    Some(true) => apply = true,
                    Some(false) => close = true,
                    None => {}
                },
            )
        }
    };
    if matches!(state, BatchDialog::Review(_)) && (close || dismissed) {
        *batch = None;
    } else if apply {
        if let Some(BatchDialog::Review(plan)) = batch.take() {
            return Some(plan);
        }
    }
    None
}

/// The dry-run dialog for work that isn't a `batch::Plan` (standardise, restyle, export, delete): a heading per item with
/// its lines under it, and whatever `extra` adds below. `Some(true)` is the `confirm` button, `Some(false)` Cancel.
/// `danger` paints the confirm button red.
pub fn review(ctx: &egui::Context, id: &str, confirm: &str, danger: bool, title: &str, sections: &[(String, Vec<String>)], can_apply: bool, extra: impl FnOnce(&mut egui::Ui)) -> Option<bool> {
    let mut answer = None;
    let rows: Vec<_> = sections.iter().map(|(l, lines)| (l.clone(), lines.iter().map(|t| (t.clone(), Tone::Plain)).collect())).collect();
    let dismissed = dialog(
        ctx,
        egui::Id::NULL.with(id),
        640.0,
        title,
        None,
        |ui| {
            if sections.is_empty() {
                ui.label("Nothing to do.");
            }
            egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| review_sections(ui, &rows));
            extra(ui);
        },
        |ui| {
            let v = ui.visuals();
            let button = if danger { super::ui_kit::danger(confirm, true, v) } else { super::ui_kit::primary(confirm, v) };
            answer = buttons(ui, if can_apply { "Cancel" } else { "Close" }, button, can_apply);
        },
    );
    answer.or(dismissed.then_some(false))
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
