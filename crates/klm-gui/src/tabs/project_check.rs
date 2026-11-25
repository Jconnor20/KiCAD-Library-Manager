//! Standards · Project check: the open project's board and schematic through KiCad's DRC and ERC, with the active
//! profile's fab minimums as extra rules, or the board once per known fab to see which could make it.

use super::page_kit::primary;
use super::standards::profile_bar;
use super::ui_kit::{self, Col, Level, Section, Sort};
use crate::AppCtx;
use eframe::egui::{self, RichText};
use klm_core::checks::Severity;
use klm_core::fab_data;
use klm_core::health::Finding;
use klm_core::kicad_env;
use klm_core::project_check::{self, FabResult};
use std::path::PathBuf;
use std::sync::mpsc;

enum Output {
    Findings(Vec<Finding>),
    Fabs(Vec<FabResult>),
}

struct Done {
    project: PathBuf,
    /// What it was checked against, e.g. "JLCPCB, IPC class 2, density B".
    against: String,
    output: Result<Output, String>,
}

#[derive(Default)]
pub struct ProjectCheckTab {
    all_fabs: bool,
    rx: Option<mpsc::Receiver<Done>>,
    done: Option<Done>,
}

impl ProjectCheckTab {
    fn run(&mut self, project: PathBuf, cx: &AppCtx) {
        let (tx, rx) = mpsc::channel();
        let (choice, all, ctx) = (cx.settings.standard.fab.clone(), self.all_fabs, cx.ctx.clone());
        std::thread::spawn(move || {
            let ipc = format!("IPC class {}, density {}", choice.ipc_class, choice.density);
            let against = if all {
                format!("every known fab, {ipc}")
            } else {
                format!("{}, {ipc}", if choice.fabs.is_empty() { "no fab".to_string() } else { choice.fabs.join(" + ") })
            };
            let output = (|| {
                let cli = kicad_env::kicad_cli()?;
                if all {
                    project_check::check_all(&project, &cli, &fab_data::fabs(), &choice).map(Output::Fabs)
                } else {
                    project_check::check(&project, &cli, &fab_data::limits(&choice)).map(Output::Findings)
                }
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Done { project, against, output });
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        if let Some(d) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (self.done, self.rx) = (Some(d), None);
        }
        let running = self.rx.is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Project check").heading().strong());
            if running {
                ui.spinner();
                ui.weak("checking…");
            }
            profile_bar(ui, cx);
        });
        ui.add_space(6.0);
        let Some(project) = cx.project.clone() else {
            ui.weak("Choose a project (top bar, or the KiCad plugin) to run KiCad's DRC and ERC on it.");
            return;
        };
        ui.horizontal(|ui| {
            let tip = "Runs kicad-cli's DRC and ERC on a copy of the project; your files are not touched";
            if primary(ui, !running, "Run check").on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                self.run(project.clone(), cx);
            }
            ui.add_enabled(!running, egui::Checkbox::new(&mut self.all_fabs, "All fabs"))
                .on_hover_text("Board DRC once per known fab instead of once with the profile's limits");
            ui.weak(project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        });
        ui.add_space(6.0);
        // A result for another project is stale once the project changes.
        let Some(done) = self.done.as_ref().filter(|d| d.project == project) else {
            ui.weak("Not run yet. Uses the active profile's fab limits; tick All fabs to compare every fab.");
            return;
        };
        ui.label(RichText::new(format!("Checked against {}", done.against)).size(12.0).color(crate::theme::palette(ui.visuals()).muted));
        ui.add_space(4.0);
        match &done.output {
            Err(e) => {
                ui.colored_label(Level::Error.color(ui.visuals()), e);
                if e.contains("could not run") && ui.link("Set the kicad-cli path in Settings · KiCad").clicked() {
                    cx.open_settings(Section::KiCad);
                }
            }
            Ok(Output::Findings(found)) => findings_table(ui, found),
            Ok(Output::Fabs(results)) => fabs_table(ui, results),
        }
    }
}

fn level(s: Severity) -> Level {
    if s == Severity::Error { Level::Error } else { Level::Warn }
}

fn findings_table(ui: &mut egui::Ui, found: &[Finding]) {
    let count = |s| found.iter().filter(|f| f.severity == s).count();
    ui.horizontal(|ui| {
        ui_kit::badge(ui, &format!("{} errors", count(Severity::Error)), Level::Error);
        ui_kit::badge(ui, &format!("{} warnings", count(Severity::Warning)), Level::Warn);
    });
    ui.add_space(4.0);
    if found.is_empty() {
        ui.colored_label(Level::Ok.color(ui.visuals()), "No violations.");
        return;
    }
    let cols = [Col::new("Severity", 80.0).no_sort(), Col::new("Check", 60.0).no_sort(), Col::rest("Message").no_sort(), Col::new("Location", 260.0).no_sort()];
    ui_kit::table(ui, "project_check", &cols, &mut Sort { col: usize::MAX, asc: true }, found.len(), |n, row| {
        let f = &found[n];
        row.col(|ui| {
            ui_kit::badge(ui, if f.severity == Severity::Error { "error" } else { "warning" }, level(f.severity));
        });
        row.col(|ui| {
            ui.label(RichText::new(f.kind.to_uppercase()).monospace());
        });
        row.col(|ui| {
            ui.label(&f.message).on_hover_text(&f.message);
        });
        row.col(|ui| {
            ui.label(RichText::new(&f.item).monospace()).on_hover_text(&f.item);
        });
    });
}

fn fabs_table(ui: &mut egui::Ui, results: &[FabResult]) {
    let passing = results.iter().filter(|r| r.passes()).count();
    ui.label(format!("{passing} of {} fabs pass", results.len()));
    ui.add_space(4.0);
    let cols = [Col::rest("Fab").no_sort(), Col::new("Result", 80.0).no_sort(), Col::new("Errors", 80.0).no_sort(), Col::new("Warnings", 80.0).no_sort()];
    ui_kit::table(ui, "project_check_fabs", &cols, &mut Sort { col: usize::MAX, asc: true }, results.len(), |n, row| {
        let r = &results[n];
        row.col(|ui| {
            ui.label(&r.name).on_hover_text(&r.fab);
        });
        row.col(|ui| {
            let ok = r.passes();
            ui_kit::badge(ui, if ok { "pass" } else { "fail" }, if ok { Level::Ok } else { Level::Error });
        });
        row.col(|ui| {
            ui.label(r.errors.to_string());
        });
        row.col(|ui| {
            ui.label(r.warnings.to_string());
        });
    });
}
