//! The "BOM" tab: the project's BOM matched against the libraries and priced from the distributors.

use super::page_kit;
use super::search;
use super::ui_kit::{self, Col, Level, Section, Sort};
use crate::theme;
use crate::{AppCtx, Msg as AppMsg, Tab};
use eframe::egui::{self, RichText};
use klm_core::alternatives::LibPart;
use klm_core::bom::{self, Lifecycle, Line, ProjectBom, Quote, Row, Status};
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
    /// Quotes for the lines' alternates, by MPN.
    alt_quotes: Vec<(String, Vec<Quote>)>,
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
    not_in_library: bool,
    /// Row (index into `rows`) shown in the inspector.
    selected: Option<usize>,
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
        // JLC class, with unknown last.
        4 => (a.assembly.jlc.map_or(3, |j| j as u8), a.assembly.lifecycle).cmp(&(b.assembly.jlc.map_or(3, |j| j as u8), b.assembly.lifecycle)),
        5 => a.alternatives.len().cmp(&b.alternatives.len()),
        6 => source(a).cmp(&source(b)),
        7 => stock(a).cmp(&stock(b)),
        8 => unit(a).partial_cmp(&unit(b)).unwrap_or(std::cmp::Ordering::Equal),
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
        self.loaded = Some(Loaded { project, lines, quotes, alt_quotes: vec![], lib, rates: None, notes: vec![] });
        self.reprice(cx);
    }

    /// Prices the alternate `alt` of line `i` in place of the line's own part, from the alternates' quotes (none when
    /// they weren't read). Reading the BOM again puts the schematic's part back.
    fn swap(&mut self, i: usize, alt: &klm_core::store::Alternative, cx: &mut AppCtx) {
        let Some(l) = &mut self.loaded else { return };
        let line = &mut l.lines[i];
        cx.log(Level::Info, format!("BOM: showing {} in place of {} for {}; the schematic is unchanged", alt.mpn, line.mpn, line.refs.join(" ")));
        // Keep the old part's quotes so it can be swapped back.
        let old = std::mem::take(&mut l.quotes[i]);
        l.alt_quotes.push((line.mpn.clone(), old));
        (line.mpn, line.manufacturer, line.supplier_ids) = (alt.mpn.clone(), alt.manufacturer.clone(), alt.supplier_ids.clone());
        l.quotes[i] = l.alt_quotes.iter().rev().find(|(m, _)| klm_core::matching::same_mpn(m, &alt.mpn)).map(|(_, q)| q.clone()).unwrap_or_default();
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
        bom::align(&mut self.rows, self.settings.assembler, &l.alt_quotes);
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
                // The alternates go through the same lookup in one pass, so an offline source times out once.
                let alts = bom::alternate_lines(&lines, store.as_ref());
                let all: Vec<Line> = lines.iter().chain(&alts).cloned().collect();
                let (mut quotes, problems) = bom::fetch_quotes(&all, &dist, store.as_ref(), ttl);
                let alt_quotes = alts.into_iter().map(|a| a.mpn).zip(quotes.split_off(lines.len())).collect();
                notes.extend(problems);
                if !region.is_empty() {
                    notes.push(format!("Shipping region {region}: used by DigiKey (site and currency). LCSC and Mouser prices do not change with it."));
                }
                Ok(Loaded { project: project.clone(), lines, quotes, alt_quotes, lib: bom::lib_parts(&roots), rates: Rates::load().ok(), notes })
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
            self.selected = None;
        }
        let pal = theme::palette(ui.visuals());
        let boards = self.settings.boards;
        let (mut search, mut import, mut swap, mut swap_jlc, mut show) = (None, false, None, None, None);
        // The inspector for the selected line sits beside everything else, full height; only where there is room for it.
        if let Some(i) = self.selected.filter(|&i| i < self.rows.len()).filter(|_| ui.available_width() >= 900.0) {
            let frame = egui::Frame::new().fill(pal.panel).inner_margin(egui::Margin::symmetric(16, 12));
            let panel = egui::Panel::right("bom_inspector").resizable(false).exact_size(340.0).frame(frame).show(ui, |ui| inspector(ui, &self.rows[i], boards));
            ui.painter().line_segment([panel.response.rect.left_top(), panel.response.rect.left_bottom()], egui::Stroke::new(1.0, pal.line));
            match panel.inner {
                Some(Insp::Swap(alt)) => swap = Some((i, alt)),
                Some(Insp::Show(mpn)) => show = Some(mpn),
                Some(Insp::Close) => self.selected = None,
                None => {}
            }
        }
        let (mut save, mut reprice) = (false, false);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("BOM").heading().strong());
            ui.label(RichText::new(project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).monospace().size(12.0).color(pal.muted));
            ui.add_space(8.0);
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
            ui.label("Assembler");
            let before = self.settings.assembler;
            egui::ComboBox::from_id_salt("bom-assembler").selected_text(before.label()).show_ui(ui, |ui| {
                for a in bom::Assembler::ALL {
                    ui.selectable_value(&mut self.settings.assembler, a, a.label());
                }
            });
            let changed = self.settings.assembler != before;
            (reprice, save) = (reprice || changed, save || changed);
            if self.settings.region.is_empty()
                && cx.settings.shipping_region.is_empty()
                && ui.link("Set in Settings…").on_hover_text("No region anywhere: DigiKey prices use its default site").clicked()
            {
                cx.open_settings(Section::Bom);
            }
            if self.running {
                ui.spinner();
            }
            if ui.add_enabled(!self.rows.is_empty(), egui::Button::new("Export CSV…")).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("bom.csv").save_file()
            {
                let csv = bom::to_csv(&self.rows, self.settings.boards, &self.priced_in);
                cx.log.push(match std::fs::write(&path, csv) {
                    Ok(()) => format!("Wrote {}", path.display()),
                    Err(e) => format!("Could not write {}: {e}", path.display()),
                });
            }
            if page_kit::primary(ui, !self.running, "Read BOM and prices")
                .on_hover_text("Runs kicad-cli, then asks the enabled distributors (cached results are reused)")
                .clicked()
            {
                self.load(cx, project.clone());
            }
        });
        if reprice || (self.priced_in != Self::target(cx) && self.loaded.is_some()) {
            self.reprice(cx);
        }
        if save && let Err(e) = self.settings.save(&project) {
            cx.log.push(format!("BOM: {e:#}"));
        }
        ui.add_space(4.0);
        if self.rows.is_empty() {
            ui.weak("Nothing read yet.");
            return;
        }
        let (t, cur) = (bom::totals(&self.rows), self.priced_in.clone());
        let short: Vec<&Row> = self.rows.iter().filter(|r| shortfall(r, boards)).collect();
        let not_in_library = self.rows.iter().filter(|r| r.status.missing_cad()).count();
        let lead = t.longest_lead_days.map_or("unknown".into(), |d| format!("{d} days"));
        let warn = Level::Warn.color(ui.visuals());
        ui.columns(5, |c| {
            page_kit::tile(&mut c[0], "Lines", &t.lines.to_string(), &format!("{not_in_library} not in library"), None);
            page_kit::tile(&mut c[1], "Per board", &format!("{:.2} {cur}", t.unit_cost), &format!("{} of {} priced", t.priced, t.lines), None);
            page_kit::tile(&mut c[2], &format!("Order · {boards} boards"), &format!("{:.2} {cur}", t.extended_cost), "before shipping", None);
            let refs: Vec<String> = short.iter().take(2).map(|r| r.line.refs.join(" ")).collect();
            let sub = if short.is_empty() { String::new() } else { format!("{}{}", refs.join(" · "), if short.len() > 2 { "…" } else { "" }) };
            page_kit::tile(&mut c[3], "Shortfalls", &short.len().to_string(), &sub, (!short.is_empty()).then_some(warn));
            page_kit::tile(&mut c[4], "Longest lead", &lead, &if t.unknown_lead > 0 { format!("{} unknown", t.unknown_lead) } else { String::new() }, None);
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter refs, MPN, maker…").desired_width(200.0));
            if ui
                .add(egui::Button::new("Shortfalls only").selected(self.shortfalls))
                .on_hover_text("Lines missing from the library, without a price, or with less stock than the boards need")
                .clicked()
            {
                self.shortfalls = !self.shortfalls;
            }
            if ui
                .add(egui::Button::new(format!("Not in library {not_in_library}")).selected(self.not_in_library))
                .on_hover_text("Lines the library doesn't have yet")
                .clicked()
            {
                self.not_in_library = !self.not_in_library;
            }
            let picks: Vec<(usize, klm_core::store::Alternative)> =
                self.rows.iter().enumerate().filter_map(|(i, r)| Some((i, r.alternatives[r.pick?].clone()))).collect();
            let hint = format!(
                "Show the alternate that suits {} best on each line: {}",
                self.settings.assembler.label(),
                match self.settings.assembler {
                    bom::Assembler::Jlc => "JLC basic, then preferred, then extended (in stock at JLC), then in stock anywhere",
                    bom::Assembler::Distributor => "active and in stock at DigiKey, then at Mouser",
                }
            );
            if !picks.is_empty() && ui.button(format!("Use best alternates ({})", picks.len())).on_hover_text(hint).clicked() {
                for (i, alt) in picks {
                    self.swap(i, &alt, cx);
                }
            }
            let summary = bom::assembly_summary(&self.rows).text();
            if !summary.is_empty() {
                ui.label(RichText::new(format!("JLCPCB: {summary}.")).color(pal.muted).size(12.0)).on_hover_text(
                    "Class and stock come from LCSC's listing, lifecycle from DigiKey and Mouser. \"?\" means no source said, not that the part is missing.",
                );
            }
        });
        ui.add_space(4.0);
        let needle = self.filter.trim().to_lowercase();
        let mut view: Vec<usize> = (0..self.rows.len())
            .filter(|&i| {
                (needle.is_empty() || matches(&self.rows[i], &needle))
                    && (!self.shortfalls || shortfall(&self.rows[i], boards))
                    && (!self.not_in_library || self.rows[i].status.missing_cad())
            })
            .collect();
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
            Col::new("JLCPCB", 215.0),
            Col::rest("Alternatives"),
            Col::new("Source", 52.0),
            Col::new("Stock", 46.0),
            Col::new("Unit / extended", 96.0),
            Col::new("Lead", 46.0),
        ];
        let (rows, selected, mut pick) = (&self.rows, self.selected, None);
        ui.style_mut().interaction.selectable_labels = false; // so a click on any cell selects the row
        // The columns need about 940 px; narrower than that (small window, inspector open) the table scrolls sideways.
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_min_width(940.0);
            ui_kit::table(ui, "bom", &cols, &mut sort, view.len(), |n, row| {
                let i = view[n];
                let r = &rows[i];
                row.set_selected(selected == Some(i));
                row.col(|ui| {
                    ui.label(RichText::new(r.line.refs.join(" ")).monospace()).on_hover_text(r.line.refs.join(" "));
                });
                row.col(|ui| {
                    ui.label((r.line.qty() * boards).to_string());
                });
                row.col(|ui| {
                    ui.label(RichText::new(mpn_of(r)).monospace());
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
                    ui.horizontal(|ui| {
                        let a = &r.assembly;
                        if a.jlc.is_some() {
                            ui_kit::badge(ui, bom::jlc_label(a), if a.jlc == Some(bom::JlcType::Basic) { Level::Ok } else { Level::Info });
                        } else {
                            ui.weak("?");
                        }
                        if a.out_of_stock() {
                            ui_kit::badge(ui, "no stock", Level::Error);
                        }
                        if let Some(l) = a.lifecycle.filter(|l| *l != Lifecycle::Active) {
                            ui_kit::badge(ui, bom::lifecycle_label(Some(l)), if l == Lifecycle::Nrnd { Level::Warn } else { Level::Error });
                        }
                        if a.flagged() {
                            let found = if let Status::Found { symbol, .. } = &r.status { Some(symbol.clone()) } else { None };
                            let btn = ui
                                .add_enabled(found.is_some(), egui::Button::new("Swap").small())
                                .on_hover_text("Find a part JLCPCB stocks in the same package")
                                .on_disabled_hover_text("Needs the part in your library first");
                            if btn.clicked() {
                                swap_jlc = found;
                            }
                        }
                    });
                });
                row.col(|ui| {
                    let alts = r.alternatives.as_slice();
                    if alts.is_empty() {
                        return;
                    }
                    let text = match r.pick {
                        Some(k) => format!("better: {}", alts[k].mpn),
                        None if alts.len() == 1 => alts[0].mpn.clone(),
                        None => format!("{} alternates", alts.len()),
                    };
                    ui.menu_button(format!("{text} ▾"), |ui| {
                        for (k, a) in alts.iter().enumerate() {
                            let best = if r.pick == Some(k) { ", best for the assembler" } else { "" };
                            if ui.button(format!("{} ({}{best})", a.mpn, a.role)).on_hover_text("Show this part in place of the line's own").clicked() {
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
                            let text = RichText::new(q.stock.map_or("?".into(), |s| s.to_string()));
                            ui.label(if q.stock.is_some_and(|s| s < r.line.qty() * boards) { text.color(Level::Warn.color(ui.visuals())) } else { text });
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
                if row.response().clicked() {
                    pick = Some(i);
                }
            });
        });
        self.sort = (sort.col != usize::MAX).then_some(sort);
        if pick.is_some() {
            self.selected = pick;
        }
        if let Some(mpn) = search {
            cx.send(AppMsg::Search(search::Msg::Query(mpn)));
            cx.tab = Tab::Search;
        }
        if import {
            cx.tab = Tab::Import;
        }
        if let Some(symbol) = swap_jlc {
            cx.send(AppMsg::Library(super::library::Msg::Swap(symbol)));
            cx.tab = Tab::Library;
        }
        if let Some(mpn) = show {
            cx.send(AppMsg::Library(super::library::Msg::Show(mpn)));
            cx.tab = Tab::Library;
        }
        if let Some((i, alt)) = swap {
            self.swap(i, &alt, cx);
        }
    }
}

/// What the inspector asked for.
enum Insp {
    Swap(klm_core::store::Alternative),
    Show(String),
    Close,
}

/// The right-hand panel for the selected line: what to buy it as, and the price breaks of the quote in use.
fn inspector(ui: &mut egui::Ui, r: &Row, boards: u64) -> Option<Insp> {
    let pal = theme::palette(ui.visuals());
    let mut act = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} · {}", r.line.refs.join(" "), mpn_of(r))).monospace().size(15.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Close").on_hover_text("Hide this panel").clicked() {
                act = Some(Insp::Close);
            }
        });
    });
    let maker = if r.line.manufacturer.is_empty() { String::new() } else { format!(" · {}", r.line.manufacturer) };
    ui.label(RichText::new(format!("{} needed for {boards} boards{maker}", r.line.qty() * boards)).color(pal.muted));
    ui.add_space(8.0);
    page_kit::caption(ui, "Buy as");
    ui.label(RichText::new("The library part's own MPN, or one of its alternatives").size(12.0).color(pal.muted));
    page_kit::card(ui, None, |ui| {
        let _ = ui.radio(true, RichText::new(mpn_of(r)).monospace());
        if let Some(b) = &r.best {
            let q = &r.quotes[b.quote];
            ui.label(
                RichText::new(format!("{:.4} · {} · {}", b.priced.unit, q.source, q.stock.map_or("?".into(), |s| s.to_string()))).size(12.0).color(pal.muted),
            );
        }
    });
    for a in &r.alternatives {
        page_kit::card(ui, None, |ui| {
            let on = ui.radio(false, RichText::new(format!("{} ({})", a.mpn, a.role)).monospace()).on_hover_text("Show this part in place of the line's own");
            if on.clicked() {
                act = Some(Insp::Swap(a.clone()));
            }
        });
    }
    if let Some(b) = &r.best {
        let q = &r.quotes[b.quote];
        if !q.breaks.is_empty() {
            ui.add_space(8.0);
            page_kit::caption(ui, &format!("Price breaks · {} · {}", q.source, q.currency));
            egui::Grid::new("bom_breaks").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                for (qty, price) in &q.breaks {
                    ui.label(RichText::new(format!("{qty}+")).color(pal.muted));
                    ui.label(RichText::new(format!("{price:.4}")).monospace());
                    ui.end_row();
                }
            });
        }
    }
    ui.add_space(12.0);
    if matches!(r.status, Status::Found { .. }) && ui.button("Show in Library").clicked() {
        act = Some(Insp::Show(mpn_of(r).to_string()));
    }
    act
}
