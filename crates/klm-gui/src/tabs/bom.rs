//! The "BOM" tab: the project's BOM matched against the libraries and priced from the distributors.

use super::search;
use super::ui_kit::{self, Col, Level, Section, Sort};
use crate::{AppCtx, Msg as AppMsg, Tab};
use eframe::egui;
use klm_core::alternatives::LibPart;
use klm_core::bom::{self, Line, ProjectBom, Quote, Row, Status};
use klm_core::cache;
use klm_core::currency::Rates;
use klm_core::store::Store;

pub enum Msg {
    Loaded(Result<Loaded, String>),
}

/// What the worker thread found; pricing for a board count is worked out from this on the UI thread.
pub struct Loaded {
    project: std::path::PathBuf,
    lines: Vec<Line>,
    quotes: Vec<Vec<Quote>>,
    lib: Vec<LibPart>,
    rates: Option<Rates>,
    notes: Vec<String>,
}

#[derive(Default)]
pub struct BomTab {
    project: Option<std::path::PathBuf>,
    settings: ProjectBom,
    loaded: Option<Loaded>,
    rows: Vec<Row>,
    /// Currency the rows were priced in.
    priced_in: String,
    running: bool,
    filter: String,
    shortfalls: bool,
    sort: Option<Sort>,
}

/// A line the order can't be filled from: not in the library, no price, or less stock than the boards need.
fn shortfall(r: &Row, boards: u64) -> bool {
    let need = r.line.qty() * boards;
    r.status.missing_cad() || r.best.as_ref().is_none_or(|b| r.quotes[b.quote].stock.is_some_and(|s| s < need))
}

fn mpn_of(r: &Row) -> &str {
    if r.line.mpn.is_empty() { &r.line.value } else { &r.line.mpn }
}

fn matches(r: &Row, needle: &str) -> bool {
    let hay = [r.line.refs.join(" "), r.line.value.clone(), r.line.mpn.clone(), r.line.manufacturer.clone(), bom::library_label(&r.status)];
    hay.iter().any(|h| h.to_lowercase().contains(needle))
}

/// Orders two rows by table column `col` (see the headers in `ui`).
fn compare(a: &Row, b: &Row, col: usize, boards: u64) -> std::cmp::Ordering {
    let unit = |r: &Row| r.best.as_ref().map(|b| b.priced.unit);
    let stock = |r: &Row| r.best.as_ref().and_then(|b| r.quotes[b.quote].stock);
    let source = |r: &Row| r.best.as_ref().map(|b| r.quotes[b.quote].source.clone());
    let lead = |r: &Row| r.best.as_ref().and_then(|b| b.lead_days);
    match col {
        0 => a.line.refs.cmp(&b.line.refs),
        1 => (a.line.qty() * boards).cmp(&(b.line.qty() * boards)),
        2 => mpn_of(a).to_lowercase().cmp(&mpn_of(b).to_lowercase()),
        3 => bom::library_label(&a.status).cmp(&bom::library_label(&b.status)),
        4 => a.alternatives.len().cmp(&b.alternatives.len()),
        5 => source(a).cmp(&source(b)),
        6 => stock(a).cmp(&stock(b)),
        7 => unit(a).partial_cmp(&unit(b)).unwrap_or(std::cmp::Ordering::Equal),
        _ => lead(a).cmp(&lead(b)),
    }
}

impl BomTab {
    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        let Msg::Loaded(res) = msg;
        self.running = false;
        match res {
            Ok(l) if self.project.as_ref() != Some(&l.project) => cx.log.push("BOM: dropped the result for another project".into()),
            Ok(l) => {
                cx.log.extend(l.notes.iter().map(|n| format!("BOM: {n}")));
                self.loaded = Some(l);
                self.reprice(cx);
            }
            Err(e) => cx.log.push(format!("BOM: {e}")),
        }
    }

    /// Fills the tab as if "Read BOM and prices" had just returned, without kicad-cli or the network.
    #[cfg(test)]
    pub fn seed(&mut self, cx: &AppCtx, project: std::path::PathBuf, lines: Vec<Line>, quotes: Vec<Vec<Quote>>, lib: Vec<LibPart>) {
        self.settings = ProjectBom::load(&project);
        self.project = Some(project.clone());
        self.loaded = Some(Loaded { project, lines, quotes, lib, rates: None, notes: vec![] });
        self.reprice(cx);
    }

    /// Prices the alternate `alt` of line `i` in place of the line's own part. The quotes were for the old part, so the
    /// line shows no price until the BOM is read again, which also puts the schematic's part back.
    fn swap(&mut self, i: usize, alt: &klm_core::store::Alternative, cx: &mut AppCtx) {
        let Some(l) = &mut self.loaded else { return };
        let line = &mut l.lines[i];
        cx.log(Level::Info, format!("BOM: showing {} in place of {} for {}; the schematic is unchanged", alt.mpn, line.mpn, line.refs.join(" ")));
        (line.mpn, line.manufacturer, line.supplier_ids) = (alt.mpn.clone(), alt.manufacturer.clone(), alt.supplier_ids.clone());
        l.quotes[i].clear();
        self.reprice(cx);
    }

    fn target(cx: &AppCtx) -> String {
        if cx.settings.display_currency.is_empty() { "USD".into() } else { cx.settings.display_currency.clone() }
    }

    fn reprice(&mut self, cx: &AppCtx) {
        let Some(l) = &self.loaded else { return };
        let store = Store::default_path().and_then(|p| Store::open(&p).ok());
        self.priced_in = Self::target(cx);
        self.rows = bom::build_rows(l.lines.clone(), &l.lib, store.as_ref(), l.quotes.clone(), self.settings.boards, &self.priced_in, l.rates.as_ref());
    }

    fn load(&mut self, cx: &mut AppCtx, project: std::path::PathBuf) {
        self.running = true;
        let region = self.settings.region(&cx.settings.shipping_region).to_string();
        let fields = self.settings.fields.clone().unwrap_or_else(|| cx.settings.bom_fields.clone());
        let (settings, roots, tx, ctx) = (cx.settings.clone(), cx.roots(), cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<Loaded> {
                let lines = bom::export(&project, &fields)?;
                let store = Store::default_path().and_then(|p| Store::open(&p).ok());
                let (dist, mut notes) = bom::distributors(&settings, &region);
                let ttl = settings.cache_ttl_minutes.unwrap_or(cache::DEFAULT_TTL_MINUTES) as i64 * 60;
                let (quotes, problems) = bom::fetch_quotes(&lines, &dist, store.as_ref(), ttl);
                notes.extend(problems);
                if !region.is_empty() {
                    notes.push(format!("Shipping region {region}: used by DigiKey (site and currency). LCSC and Mouser prices do not change with it."));
                }
                Ok(Loaded { project: project.clone(), lines, quotes, lib: bom::lib_parts(&roots), rates: Rates::load().ok(), notes })
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppMsg::Bom(Msg::Loaded(res)));
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let Some(project) = cx.project.clone() else {
            ui.weak("Choose a project (top bar, or the KiCad plugin) to see its BOM.");
            return;
        };
        if self.project.as_ref() != Some(&project) {
            self.project = Some(project.clone());
            self.settings = ProjectBom::load(&project);
            self.loaded = None;
            self.rows.clear();
        }
        let (mut save, mut reprice) = (false, false);
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!self.running, egui::Button::new("Read BOM and prices")).on_hover_text("Runs kicad-cli, then asks the enabled distributors (cached results are reused)").clicked() {
                self.load(cx, project.clone());
            }
            ui.label("Boards");
            let boards = ui.add(egui::DragValue::new(&mut self.settings.boards).range(1..=1_000_000));
            reprice |= boards.changed();
            save |= boards.drag_stopped() || boards.lost_focus();
            ui.label("Region");
            let hint = if cx.settings.shipping_region.is_empty() { "default".to_string() } else { format!("{} (default)", cx.settings.shipping_region) };
            save |= ui
                .add(egui::TextEdit::singleline(&mut self.settings.region).hint_text(hint).desired_width(90.0))
                .on_hover_text("Country code (US, DE, ...) for this project; empty uses the Settings default. Where you are, or where the board is made.")
                .lost_focus();
            if self.settings.region.is_empty() && cx.settings.shipping_region.is_empty() && ui.link("Set in Settings…").on_hover_text("No region anywhere: DigiKey prices use its default site").clicked() {
                cx.open_settings(Section::Bom);
            }
            if ui.add_enabled(!self.rows.is_empty(), egui::Button::new("Export CSV…")).clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("bom.csv").save_file() {
                    let csv = bom::to_csv(&self.rows, self.settings.boards, &self.priced_in);
                    cx.log.push(match std::fs::write(&path, csv) {
                        Ok(()) => format!("Wrote {}", path.display()),
                        Err(e) => format!("Could not write {}: {e}", path.display()),
                    });
                }
            }
            if self.running {
                ui.spinner();
            }
        });
        if reprice || (self.priced_in != Self::target(cx) && self.loaded.is_some()) {
            self.reprice(cx);
        }
        if save {
            if let Err(e) = self.settings.save(&project) {
                cx.log.push(format!("BOM: {e:#}"));
            }
        }
        if self.rows.is_empty() {
            ui.weak("Nothing read yet.");
            return;
        }
        let (t, cur) = (bom::totals(&self.rows), self.priced_in.clone());
        let lead = t.longest_lead_days.map_or("unknown".into(), |d| format!("{d} days")) + &if t.unknown_lead > 0 { format!(" (+{} unknown)", t.unknown_lead) } else { String::new() };
        ui.label(format!(
            "{} lines, {} priced ({} without a price). Per board {:.2} {cur}, order {:.2} {cur}. Longest lead time: {lead}.",
            t.lines,
            t.priced,
            t.lines - t.priced,
            t.unit_cost,
            t.extended_cost,
        ));
        let boards = self.settings.boards;
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("refs, MPN, maker…").desired_width(160.0));
            ui.checkbox(&mut self.shortfalls, "Shortfalls only").on_hover_text("Lines missing from the library, without a price, or with less stock than the boards need");
        });
        let needle = self.filter.trim().to_lowercase();
        let mut view: Vec<usize> = (0..self.rows.len()).filter(|&i| (needle.is_empty() || matches(&self.rows[i], &needle)) && (!self.shortfalls || shortfall(&self.rows[i], boards))).collect();
        let mut sort = self.sort.unwrap_or(Sort { col: usize::MAX, asc: true });
        if sort.col != usize::MAX {
            view.sort_by(|&a, &b| {
                let o = compare(&self.rows[a], &self.rows[b], sort.col, boards);
                if sort.asc { o } else { o.reverse() }
            });
        }
        if view.is_empty() {
            ui.weak("No line matches.");
        }
        let cols = [
            Col::new("Refs", 60.0),
            Col::new("Qty", 32.0),
            Col::new("MPN", 160.0),
            Col::new("Library", 170.0),
            Col::rest("Alternatives"),
            Col::new("Source", 52.0),
            Col::new("Stock", 46.0),
            Col::new("Unit / extended", 96.0),
            Col::new("Lead", 46.0),
        ];
        let (mut search, mut import, mut swap) = (None, false, None);
        let rows = &self.rows;
        ui_kit::table(ui, "bom", &cols, &mut sort, view.len(), |n, row| {
            let i = view[n];
            let r = &rows[i];
            row.col(|ui| {
                ui.label(r.line.refs.join(" ")).on_hover_text(r.line.refs.join(" "));
            });
            row.col(|ui| {
                ui.label((r.line.qty() * boards).to_string());
            });
            row.col(|ui| {
                ui.label(mpn_of(r));
            });
            row.col(|ui| {
                ui.horizontal(|ui| {
                    let ok = matches!(r.status, Status::Found { has_footprint: true, .. });
                    ui_kit::badge(ui, &bom::library_label(&r.status), if ok { Level::Ok } else { Level::Warn });
                    if r.status.missing_cad() && !mpn_of(r).is_empty() {
                        if ui.small_button("Search").on_hover_text("Open Search with this MPN (copied to the clipboard)").clicked() {
                            search = Some(mpn_of(r).to_string());
                        }
                        import |= ui.small_button("Import").on_hover_text("Open Import to add a downloaded ZIP or library for it").clicked();
                    }
                });
            });
            row.col(|ui| {
                let alts = r.alternatives.as_slice();
                if alts.is_empty() {
                    return;
                }
                let text = if let [a] = alts { a.mpn.clone() } else { format!("{} alternates", alts.len()) };
                ui.menu_button(format!("{text} ▾"), |ui| {
                    for a in alts {
                        if ui.button(format!("{} ({})", a.mpn, a.role)).on_hover_text("Show this part in place of the line's own").clicked() {
                            swap = Some((i, a.clone()));
                            ui.close();
                        }
                    }
                });
            });
            match &r.best {
                Some(b) => {
                    let q = &r.quotes[b.quote];
                    row.col(|ui| {
                        ui.label(format!("{}{}", q.source, if q.stale { " (old)" } else { "" }));
                    });
                    row.col(|ui| {
                        ui.label(q.stock.map_or("?".into(), |s| s.to_string()));
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.4} / {:.2}", b.priced.unit, b.priced.ext)).on_hover_text(format!("{} bought", b.priced.bought));
                    });
                    row.col(|ui| {
                        ui.label(b.lead_days.map_or("?".into(), |d| if d == 0 { "in stock".into() } else { format!("{d} d") }));
                    });
                }
                None => {
                    row.col(|ui| {
                        ui.weak("no price");
                    });
                    for _ in 0..3 {
                        row.col(|_| {});
                    }
                }
            }
        });
        self.sort = (sort.col != usize::MAX).then_some(sort);
        if let Some(mpn) = search {
            cx.send(AppMsg::Search(search::Msg::Query(mpn)));
            cx.tab = Tab::Search;
        }
        if import {
            cx.tab = Tab::Import;
        }
        if let Some((i, alt)) = swap {
            self.swap(i, &alt, cx);
        }
    }
}
