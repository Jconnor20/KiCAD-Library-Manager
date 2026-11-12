//! Standards · Library health: every part that is missing a file, differs from the house standard, points at a
//! file that isn't there or shares an MPN with another part, grouped by problem.

use super::batch_dialog;
use super::page_kit::{caption, primary, tile};
use super::standards::{profile_bar, seg, state_key};
use super::ui_kit::{self, Col, Level, Sort};
use crate::{AppCtx, theme};
use eframe::egui::{self, RichText};
use klm_core::library::{Check, LibraryRoot, Scope};
use klm_core::symbols::Class;
use klm_core::{compliance, standardise, symbols};
use std::collections::HashMap;
use std::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Files,
    Standards,
    Broken,
    Duplicates,
}

const KINDS: [(Kind, &str); 4] = [(Kind::Files, "Files missing"), (Kind::Standards, "Differs from standard"), (Kind::Broken, "Broken"), (Kind::Duplicates, "Duplicates")];

struct Issue {
    kind: Kind,
    root: usize,
    category: String,
    part: String,
    problem: String,
    fix: String,
    /// Every reason the part isn't compliant, for the detail pane.
    reasons: Vec<String>,
    class: Option<Class>,
}

struct Report {
    parts: usize,
    /// Parts with no issue at all.
    clean: usize,
    issues: Vec<Issue>,
    roots: Vec<LibraryRoot>,
}

enum Dry {
    Planning(mpsc::Receiver<standardise::Plan>),
    Plan(Box<standardise::Plan>),
}

#[derive(Default)]
pub struct HealthTab {
    report: Option<Report>,
    rx: Option<mpsc::Receiver<Report>>,
    key: String,
    filter: Option<Kind>,
    text: String,
    selected: Option<usize>,
    dry: Option<Dry>,
    applied: Option<mpsc::Receiver<(Level, String)>>,
}

fn scan(roots: Vec<LibraryRoot>, cx: &AppCtx) -> mpsc::Receiver<Report> {
    let (tx, rx) = mpsc::channel();
    let (profile, naming, ctx) = (cx.settings.standard.clone(), cx.settings.rules.clone(), cx.ctx.clone());
    std::thread::spawn(move || {
        let (mut issues, mut parts, mut dirty) = (vec![], 0, 0);
        let mut by_mpn: HashMap<String, Vec<(usize, String, String)>> = HashMap::new();
        for (ri, root) in roots.iter().enumerate() {
            let (comp, classes) = (compliance::compute(root, &profile, &naming), symbols::classes_in(root, &naming));
            for p in root.list_parts() {
                parts += 1;
                let before = issues.len();
                let mut add = |kind, problem: String, fix: &str, reasons: Vec<String>| {
                    issues.push(Issue { kind, root: ri, category: p.category.clone(), part: p.name.clone(), problem, fix: fix.into(), reasons, class: classes.get(&(p.category.clone(), p.name.clone())).copied() })
                };
                for (what, check, fix) in [("footprint", p.footprint, "Pick a footprint"), ("3D model", p.model, "Fetch from SnapMagic"), ("datasheet", p.datasheet, "Look up by MPN")] {
                    match check {
                        Check::Missing => add(Kind::Files, format!("No {what}"), fix, vec![]),
                        Check::Broken => add(Kind::Broken, format!("The {what} file is not where the part says"), "Choose another file", vec![]),
                        _ => {}
                    }
                }
                if let Some(c) = comp.get(&(p.category.clone(), p.name.clone())).filter(|c| !c.ok) {
                    let first = c.reasons.first().map(|r| r.trim_start_matches("Standardise: ").to_string()).unwrap_or_default();
                    add(Kind::Standards, first, "Restyle to the profile", c.reasons.clone());
                }
                dirty += (issues.len() > before) as usize;
                if !p.mpn.trim().is_empty() {
                    by_mpn.entry(p.mpn.trim().to_lowercase()).or_default().push((ri, p.category.clone(), p.name.clone()));
                }
            }
        }
        for (mpn, at) in by_mpn.into_iter().filter(|(_, v)| v.len() > 1) {
            let names: Vec<_> = at.iter().map(|a| a.2.as_str()).collect();
            issues.push(Issue { kind: Kind::Duplicates, root: at[0].0, category: at[0].1.clone(), part: names.join(" / "), problem: format!("Same MPN {mpn} in {} places", at.len()), fix: "Keep one, merge the fields".into(), reasons: vec![], class: None });
        }
        issues.sort_by(|a, b| (a.kind, &a.category, &a.part).cmp(&(b.kind, &b.category, &b.part)));
        let _ = tx.send(Report { parts, clean: parts - dirty, issues, roots });
        ctx.request_repaint();
    });
    rx
}

fn where_chip(r: &Report, root: usize) -> &'static str {
    if r.roots.get(root).is_some_and(|r| r.scope == Scope::Project) { "P" } else { "G" }
}

impl HealthTab {
    fn refresh(&mut self, cx: &AppCtx) {
        self.key = state_key(cx);
        self.selected = None;
        self.rx = Some(scan(cx.roots(), cx));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        if let Some(r) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (self.report, self.rx) = (Some(r), None);
        }
        if let Some((level, line)) = self.applied.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.applied = None;
            cx.log(level, line);
            cx.send(crate::Msg::Library(crate::library::Msg::Refresh));
            self.refresh(cx);
        }
        if self.rx.is_none() && self.applied.is_none() && self.key != state_key(cx) {
            self.refresh(cx);
        }
        let loading = self.rx.is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Library health").heading().strong());
            if cx.roots().is_empty() {
                ui.weak("Set up a global library in Settings to check its parts.");
            } else if loading {
                ui.weak("checking…");
            }
            if ui.add_enabled(!loading, egui::Button::new("Check again")).clicked() {
                self.refresh(cx);
            }
            profile_bar(ui, cx);
        });
        ui.add_space(6.0);
        let Some(report) = &self.report else { return };
        let count = |k: Kind| report.issues.iter().filter(|i| i.kind == k).count();
        let pal = theme::palette(ui.visuals());
        let (warn, err, info) = (Level::Warn.color(ui.visuals()), Level::Error.color(ui.visuals()), Level::Info.color(ui.visuals()));
        let tint = |n: usize, c| (n > 0).then_some(c);
        ui.columns(5, |c| {
            tile(&mut c[0], "Parts checked", &report.parts.to_string(), &format!("{} fully clean", report.clean), None);
            tile(&mut c[1], "Files missing", &count(Kind::Files).to_string(), "footprint, 3D, datasheet", tint(count(Kind::Files), warn));
            tile(&mut c[2], "Differs from standard", &count(Kind::Standards).to_string(), "restyle to match", tint(count(Kind::Standards), warn));
            tile(&mut c[3], "Broken", &count(Kind::Broken).to_string(), "file not found", tint(count(Kind::Broken), err));
            tile(&mut c[4], "Duplicates", &count(Kind::Duplicates).to_string(), "same MPN twice", tint(count(Kind::Duplicates), info));
        });
        ui.add_space(6.0);
        let differ = count(Kind::Standards);
        let (mut pick, mut review) = (None, false);
        let labels: Vec<String> = std::iter::once(format!("All {}", report.issues.len())).chain(KINDS.iter().map(|(k, n)| format!("{} {}", n.split(' ').next().unwrap_or(n), count(*k)))).collect();
        let on = self.filter.map_or(0, |f| 1 + KINDS.iter().position(|k| k.0 == f).unwrap_or(0));
        ui.horizontal(|ui| {
            let items: Vec<(&str, bool, &str)> = labels.iter().map(|l| (l.as_str(), true, "")).collect();
            if let Some(i) = seg(ui, on, &items) {
                self.filter = (i > 0).then(|| KINDS[i - 1].0);
            }
            ui.add(egui::TextEdit::singleline(&mut self.text).hint_text("Filter by part…").desired_width(180.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let tip = "Shows what the profile would change in the whole library, then applies it after a snapshot. Missing files are fixed from the Library tab.";
                review = primary(ui, differ > 0 && self.dry.is_none(), "Review and apply…").on_hover_text(tip).on_disabled_hover_text(tip).clicked();
            });
        });
        ui.add_space(4.0);
        let q = self.text.trim().to_lowercase();
        let show: Vec<usize> = (0..report.issues.len()).filter(|&i| self.filter.is_none_or(|f| f == report.issues[i].kind) && (q.is_empty() || report.issues[i].part.to_lowercase().contains(&q))).collect();
        // Group headers are rows too: (group, count) or an issue.
        let mut entries: Vec<Result<usize, (Kind, usize)>> = vec![];
        for (k, _) in KINDS {
            let of: Vec<_> = show.iter().copied().filter(|&i| report.issues[i].kind == k).collect();
            if !of.is_empty() {
                entries.push(Err((k, of.len())));
                entries.extend(of.into_iter().map(Ok));
            }
        }
        let wide = ui.available_width() >= 900.0;
        if wide {
            egui::Panel::right("health_detail").exact_size(300.0).resizable(false).frame(egui::Frame::new().fill(pal.panel).stroke(egui::Stroke::new(1.0, pal.line)).inner_margin(egui::Margin::same(12))).show(ui, |ui| {
                self.detail(ui);
            });
        }
        let report = self.report.as_ref().expect("checked above");
        let cols = [Col::new("Part", 170.0).no_sort(), Col::rest("Problem").no_sort(), Col::new("Suggested fix", 170.0).no_sort(), Col::new("Where", 50.0).no_sort()];
        let selected = self.selected;
        ui_kit::table(ui, "health", &cols, &mut Sort { col: usize::MAX, asc: true }, entries.len(), |n, row| match entries[n] {
            Err((k, c)) => {
                let name = KINDS.iter().find(|x| x.0 == k).map_or("", |x| x.1);
                row.col(|ui| {
                    ui.horizontal(|ui| {
                        ui_kit::badge(ui, &c.to_string(), if k == Kind::Broken { Level::Error } else { Level::Warn });
                        ui.strong(name);
                    });
                });
                for _ in 1..cols.len() {
                    row.col(|_| {});
                }
            }
            Ok(i) => {
                let x = &report.issues[i];
                row.set_selected(selected == Some(i));
                row.col(|ui| {
                    ui.label(RichText::new(&x.part).monospace()).on_hover_text(&x.category);
                });
                row.col(|ui| {
                    ui.label(&x.problem).on_hover_text(if x.reasons.is_empty() { x.problem.clone() } else { x.reasons.join("\n") });
                });
                row.col(|ui| {
                    ui.weak(&x.fix);
                });
                row.col(|ui| {
                    ui.weak(where_chip(report, x.root));
                });
                if row.response().clicked() {
                    pick = Some(i);
                }
            }
        });
        if pick.is_some() {
            self.selected = pick;
        }
        if review && let Some(root) = self.report.as_ref().and_then(|r| r.roots.get(self.selected.map_or(0, |i| r.issues[i].root))) {
            let (tx, rx) = mpsc::channel();
            let (root, profile, naming, ctx) = (root.clone(), cx.settings.standard.clone(), cx.settings.rules.clone(), cx.ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(standardise::plan(&root, &profile, &naming));
                ctx.request_repaint();
            });
            self.dry = Some(Dry::Planning(rx));
        }
        self.dialog(cx);
    }

    fn detail(&self, ui: &mut egui::Ui) {
        let Some(x) = self.selected.and_then(|i| self.report.as_ref()?.issues.get(i)) else {
            ui.weak("Select a part to see why it is listed.");
            return;
        };
        ui.label(RichText::new(&x.part).monospace().strong().size(14.0));
        ui.add_space(4.0);
        ui.label(&x.problem);
        ui.add_space(8.0);
        egui::Grid::new("health_facts").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.weak("Library");
            ui.label(&x.category);
            ui.end_row();
            if let Some(c) = x.class {
                ui.weak("Class");
                ui.label(format!("{c:?} · {}", c.prefix()));
                ui.end_row();
            }
        });
        if !x.reasons.is_empty() {
            ui.add_space(8.0);
            caption(ui, "What differs");
            for r in &x.reasons {
                ui.label(r);
            }
        }
    }

    fn dialog(&mut self, cx: &mut AppCtx) {
        if let Some(Dry::Planning(rx)) = &self.dry
            && let Ok(plan) = rx.try_recv()
        {
            self.dry = Some(Dry::Plan(Box::new(plan)));
        }
        let Some(Dry::Plan(plan)) = &self.dry else { return };
        let mut sections: Vec<_> = plan.files.iter().map(|f| (f.label.clone(), f.changes.clone())).collect();
        sections.extend(plan.errors.iter().map(|e| ("Can't be read".to_string(), vec![e.clone()])));
        let note = format!("{}\nThe rules are in Settings → Standards. A snapshot is taken first, so History can roll it back.", plan.root.dir.display());
        match batch_dialog::review(&cx.ctx.clone(), "health_standardise", "Apply", false, "Standardise", &sections, !plan.files.is_empty() && !cx.busy, |ui| {
            ui.weak(note);
        }) {
            Some(true) => {
                if let Some(Dry::Plan(plan)) = self.dry.take() {
                    let (tx, rx) = mpsc::channel();
                    let ctx = cx.ctx.clone();
                    self.applied = Some(rx);
                    std::thread::spawn(move || {
                        let dir = plan.root.dir.display().to_string();
                        let _ = tx.send(match standardise::apply(&plan) {
                            Ok(n) => (Level::Ok, format!("Standardised {n} file(s) in {dir}")),
                            Err(e) => (Level::Error, format!("Standardise failed, nothing changed in {dir}: {e:#}")),
                        });
                        ctx.request_repaint();
                    });
                }
            }
            Some(false) => self.dry = None,
            None => {}
        }
    }
}
