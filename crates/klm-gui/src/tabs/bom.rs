//! The "BOM" tab: the project's BOM matched against the libraries and priced from the distributors.

use crate::{AppCtx, Msg as AppMsg};
use eframe::egui::{self, RichText};
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
    running: bool,
}

impl BomTab {
    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        let Msg::Loaded(res) = msg;
        self.running = false;
        match res {
            Ok(l) => {
                cx.log.extend(l.notes.iter().map(|n| format!("BOM: {n}")));
                self.loaded = Some(l);
                self.reprice(cx);
            }
            Err(e) => cx.log.push(format!("BOM: {e}")),
        }
    }

    fn target(cx: &AppCtx) -> String {
        if cx.settings.display_currency.is_empty() { "USD".into() } else { cx.settings.display_currency.clone() }
    }

    fn reprice(&mut self, cx: &AppCtx) {
        let Some(l) = &self.loaded else { return };
        let store = Store::default_path().and_then(|p| Store::open(&p).ok());
        self.rows = bom::build_rows(l.lines.clone(), &l.lib, store.as_ref(), l.quotes.clone(), self.settings.boards, &Self::target(cx), l.rates.as_ref());
    }

    fn load(&mut self, cx: &mut AppCtx, project: std::path::PathBuf) {
        self.running = true;
        let region = self.settings.region(&cx.settings.shipping_region).to_string();
        let (settings, roots, tx, ctx) = (cx.settings.clone(), cx.roots(), cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<Loaded> {
                let lines = bom::export(&project)?;
                let store = Store::default_path().and_then(|p| Store::open(&p).ok());
                let (dist, mut notes) = bom::distributors(&settings, &region);
                let ttl = settings.cache_ttl_minutes.unwrap_or(cache::DEFAULT_TTL_MINUTES) as i64 * 60;
                let (quotes, problems) = bom::fetch_quotes(&lines, &dist, store.as_ref(), ttl);
                notes.extend(problems);
                if !region.is_empty() {
                    notes.push(format!("Shipping region {region}: used by DigiKey (site and currency). LCSC and Mouser prices do not change with it."));
                }
                Ok(Loaded { lines, quotes, lib: bom::lib_parts(&roots), rates: Rates::load().ok(), notes })
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppMsg::Bom(Msg::Loaded(res)));
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let Some(project) = cx.project.clone() else {
            ui.weak("Choose a project (Settings, or the KiCad plugin) to see its BOM.");
            return;
        };
        if self.project.as_ref() != Some(&project) {
            self.project = Some(project.clone());
            self.settings = ProjectBom::load(&project);
            self.loaded = None;
            self.rows.clear();
        }
        let (mut save, mut reprice) = (false, false);
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.running, egui::Button::new("Read BOM and prices")).on_hover_text("Runs kicad-cli, then asks the enabled distributors (cached results are reused)").clicked() {
                self.load(cx, project.clone());
            }
            ui.label("Boards");
            reprice |= ui.add(egui::DragValue::new(&mut self.settings.boards).range(1..=1_000_000)).changed();
            ui.label("Region");
            let hint = if cx.settings.shipping_region.is_empty() { "default".to_string() } else { format!("{} (default)", cx.settings.shipping_region) };
            save |= ui
                .add(egui::TextEdit::singleline(&mut self.settings.region).hint_text(hint).desired_width(90.0))
                .on_hover_text("Country code (US, DE, ...) for this project; empty uses the Settings default. Where you are, or where the board is made.")
                .lost_focus();
            if ui.add_enabled(!self.rows.is_empty(), egui::Button::new("Export CSV...")).clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("bom.csv").save_file() {
                    let csv = bom::to_csv(&self.rows, self.settings.boards, &Self::target(cx));
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
        if reprice {
            self.reprice(cx);
        }
        if save || reprice {
            if let Err(e) = self.settings.save(&project) {
                cx.log.push(format!("BOM: {e:#}"));
            }
        }
        if self.rows.is_empty() {
            ui.weak("Nothing read yet.");
            return;
        }
        let (t, cur) = (bom::totals(&self.rows), Self::target(cx));
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
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("bom").num_columns(9).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                for h in ["Refs", "Qty", "MPN", "Library", "Alternatives", "Source", "Stock", "Unit / extended", "Lead"] {
                    ui.strong(h);
                }
                ui.end_row();
                for r in &self.rows {
                    ui.label(r.line.refs.join(" "));
                    ui.label((r.line.qty() * boards).to_string());
                    ui.label(if r.line.mpn.is_empty() { r.line.value.clone() } else { r.line.mpn.clone() });
                    let text = RichText::new(bom::library_label(&r.status));
                    ui.label(if matches!(r.status, Status::Found { has_footprint: true, .. }) { text } else { text.color(ui.visuals().warn_fg_color) });
                    ui.label(r.alternatives.iter().map(|a| a.mpn.as_str()).collect::<Vec<_>>().join(", "));
                    match &r.best {
                        Some(b) => {
                            let q = &r.quotes[b.quote];
                            ui.label(format!("{}{}", q.source, if q.stale { " (old cache)" } else { "" }));
                            ui.label(q.stock.map_or("?".into(), |s| s.to_string()));
                            ui.label(format!("{:.4} / {:.2} ({} bought)", b.priced.unit, b.priced.ext, b.priced.bought));
                            ui.label(b.lead_days.map_or("?".into(), |d| if d == 0 { "in stock".into() } else { format!("{d} d") }));
                        }
                        None => {
                            ui.weak("no price");
                            ui.label("");
                            ui.label("");
                            ui.label("");
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }
}
