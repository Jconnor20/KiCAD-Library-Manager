use super::import;
use crate::{AppCtx, Msg as AppMsg, new_work_dir};
use eframe::egui::{self, RichText};
use klm_core::currency::{Rates, display_price, price_in};
use klm_core::part::Part;
use klm_core::search_filter::{Filter, SortKey, view};
use klm_core::sources::combine;

pub enum Msg {
    Done(Vec<(usize, Part)>),
    /// The sources were rebuilt, so the indexes in the results no longer match.
    ClearResults,
    /// Exchange rates for the display currency (`None` if they could not be loaded).
    Rates(Option<Rates>),
}

enum Action {
    None,
    Search(String),
    /// Fetch result `i` (an index into the unsorted, unfiltered results).
    Get(usize),
}

/// One search box over every enabled source; results from all of them in one table.
#[derive(Default)]
pub struct SearchTab {
    query: String,
    searching: bool,
    /// (index into the app's source list, result)
    results: Vec<(usize, Part)>,
    filter: Filter,
    sort: Option<(SortKey, bool)>,
    rates: Option<Rates>,
    /// A rates load was started since the currency was last changed; don't retry every frame.
    rates_tried: bool,
}

impl SearchTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let names: Vec<&str> = cx.sources.iter().map(|s| s.name()).collect();
        let mut currency = cx.settings.display_currency.clone();
        let action = self.show(ui, &names, cx.busy, &mut currency);
        if currency != cx.settings.display_currency {
            cx.settings.display_currency = currency;
            self.rates_tried = false;
            if let Err(e) = cx.settings.save() {
                cx.log.push(format!("Could not save settings: {e:#}"));
            }
        }
        // Loaded as soon as there are results so the currency list has codes to offer.
        if (!cx.settings.display_currency.is_empty() || !self.results.is_empty()) && self.rates.is_none() && !self.rates_tried {
            self.rates_tried = true;
            self.load_rates(cx);
        }
        match action {
            Action::None => {}
            Action::Search(q) => self.run_search(q, cx),
            Action::Get(i) => self.get_result(i, cx),
        }
    }

    pub fn handle(&mut self, msg: Msg, cx: &mut AppCtx) {
        match msg {
            Msg::Done(results) => {
                self.searching = false;
                cx.log.push(format!("{} search results", results.len()));
                self.results = results;
                if !self.results.iter().any(|(_, p)| p.manufacturer == self.filter.manufacturer) {
                    self.filter.manufacturer.clear();
                }
            }
            Msg::ClearResults => self.results.clear(),
            Msg::Rates(rates) => self.rates = rates.or(self.rates.take()),
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, source_names: &[&str], busy: bool, currency: &mut String) -> Action {
        let mut action = Action::None;
        if source_names.is_empty() {
            ui.label("No search sources are enabled yet.");
            return action;
        }
        ui.horizontal(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("MPN, LCSC/DigiKey number or keywords").desired_width(320.0));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let can = !self.searching && !self.query.trim().is_empty();
            if (ui.add_enabled(can, egui::Button::new("Search")).clicked() || (enter && can)) && can {
                action = Action::Search(self.query.trim().to_string());
            }
            if self.searching {
                ui.spinner();
            }
            ui.weak(format!("Searching: {}", source_names.join(", ")));
        });
        ui.add_space(4.0);
        self.filters_ui(ui, source_names, currency);
        let target = (!currency.is_empty()).then_some(currency.as_str());
        let rates = self.rates.as_ref();
        let rows = view(&self.results, &self.filter, self.sort, &|p| price_in(&p.unit_price, target, rates));
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("results").striped(true).num_columns(8).spacing([14.0, 4.0]).show(ui, |ui| {
                for (h, key) in [("Source", SortKey::Source), ("MPN", SortKey::Mpn), ("Manufacturer", SortKey::Manufacturer), ("Description", SortKey::Description), ("Package", SortKey::Package), ("Stock", SortKey::Stock), ("Price", SortKey::Price)] {
                    let h = if key == SortKey::Price { &format!("Price ({})", target.unwrap_or("native")) } else { h };
                    let arrow = match self.sort {
                        Some((k, asc)) if k == key => if asc { " ▲" } else { " ▼" },
                        _ => "",
                    };
                    if ui.add(egui::Button::new(RichText::new(format!("{h}{arrow}")).strong()).frame(false)).clicked() {
                        self.sort = match self.sort {
                            Some((k, asc)) if k == key => Some((key, !asc)),
                            _ => Some((key, true)),
                        };
                    }
                }
                ui.label("");
                ui.end_row();
                // `i` indexes `self.results`, not the sorted rows, so "Get…" survives any sort or filter.
                for i in rows {
                    let (src, p) = &self.results[i];
                    ui.label(source_names.get(*src).copied().unwrap_or("?"));
                    ui.label(RichText::new(&p.mpn).strong());
                    ui.label(&p.manufacturer);
                    let short: String = p.description.chars().take(60).collect();
                    ui.label(short).on_hover_text(&p.description);
                    ui.label(&p.package);
                    ui.label(p.stock.map_or(String::new(), |s| s.to_string()));
                    let shown = display_price(&p.unit_price, target, rates);
                    let price = ui.label(&shown);
                    if shown != p.unit_price {
                        price.on_hover_text(format!("Converted from {}", p.unit_price));
                    } else if target.is_some() && !p.unit_price.is_empty() {
                        price.on_hover_text("Not converted: unknown currency or no exchange rates");
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Get…")).clicked() {
                        action = Action::Get(i);
                    }
                    ui.end_row();
                }
            });
        });
        action
    }

    fn filters_ui(&mut self, ui: &mut egui::Ui, source_names: &[&str], currency: &mut String) {
        if self.results.is_empty() {
            return;
        }
        let mut sources: Vec<usize> = self.results.iter().map(|(s, _)| *s).collect();
        sources.sort_unstable();
        sources.dedup();
        let mut manufacturers: Vec<&str> = self.results.iter().map(|(_, p)| p.manufacturer.as_str()).filter(|m| !m.is_empty()).collect();
        manufacturers.sort_unstable_by_key(|m| m.to_lowercase());
        manufacturers.dedup();
        let f = &mut self.filter;
        ui.horizontal_wrapped(|ui| {
            for s in sources {
                let mut shown = !f.hidden_sources.contains(&s);
                if ui.checkbox(&mut shown, source_names.get(s).copied().unwrap_or("?")).changed() {
                    f.hidden_sources.retain(|h| *h != s);
                    if !shown {
                        f.hidden_sources.push(s);
                    }
                }
            }
            ui.checkbox(&mut f.in_stock, "In stock");
            egui::ComboBox::from_id_salt("manufacturer_filter").selected_text(if f.manufacturer.is_empty() { "Any manufacturer" } else { &f.manufacturer }).show_ui(ui, |ui| {
                ui.selectable_value(&mut f.manufacturer, String::new(), "Any manufacturer");
                for m in manufacturers {
                    ui.selectable_value(&mut f.manufacturer, m.to_string(), m);
                }
            });
            ui.add(egui::TextEdit::singleline(&mut f.package).hint_text("Package contains").desired_width(110.0));
            let unit = if currency.is_empty() { "native" } else { currency.as_str() };
            ui.add(egui::TextEdit::singleline(&mut f.max_price).hint_text(format!("Max price ({unit})")).desired_width(110.0));
            ui.add(egui::TextEdit::singleline(&mut f.text).hint_text("MPN, manufacturer, description").desired_width(190.0));
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("currency").selected_text(if currency.is_empty() { "Native currencies" } else { currency.as_str() }).show_ui(ui, |ui| {
                ui.selectable_value(currency, String::new(), "Native currencies");
                for c in self.rates.as_ref().map_or(vec![], Rates::codes) {
                    ui.selectable_value(currency, c.to_string(), c);
                }
            });
            match (&self.rates, currency.is_empty()) {
                (Some(r), false) => drop(ui.weak(format!("rates of {}", r.date))),
                (None, false) if self.rates_tried => drop(ui.weak("exchange rates unavailable; showing native prices")),
                _ => {}
            }
            let target = (!currency.is_empty()).then_some(currency.as_str());
            let shown = view(&self.results, &self.filter, None, &|p| price_in(&p.unit_price, target, self.rates.as_ref())).len();
            ui.weak(format!("{shown} of {} results", self.results.len()));
            if ui.add_enabled(!self.filter.is_empty(), egui::Button::new("Clear filters")).clicked() {
                self.filter = Filter::default();
            }
        });
        ui.add_space(4.0);
    }

    fn load_rates(&self, cx: &AppCtx) {
        let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let rates = Rates::load();
            if let Err(e) = &rates {
                let _ = tx.send(AppMsg::Log(format!("Exchange rates: {e:#}")));
            }
            let _ = tx.send(AppMsg::Search(Msg::Rates(rates.ok())));
            ctx.request_repaint();
        });
    }

    fn run_search(&mut self, query: String, cx: &mut AppCtx) {
        self.searching = true;
        self.results.clear();
        let (tx, sources, ctx) = (cx.tx.clone(), cx.sources.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let mut results = vec![];
            let query = &query;
            std::thread::scope(|s| {
                let handles: Vec<_> = sources.iter().enumerate().map(|(i, src)| s.spawn(move || (i, src.name(), src.search(&query)))).collect();
                for h in handles {
                    match h.join() {
                        Ok((i, _, Ok(parts))) => results.extend(parts.into_iter().map(|p| (i, p))),
                        Ok((_, name, Err(e))) => drop(tx.send(AppMsg::Log(format!("{name} search failed: {e:#}")))),
                        Err(_) => drop(tx.send(AppMsg::Log("A search source crashed".into()))),
                    }
                }
            });
            let _ = tx.send(AppMsg::Search(Msg::Done(results)));
            ctx.request_repaint();
        });
    }

    /// Opens the import dialog for a search result and asks every source for its files; each
    /// source's answer reaches the dialog as it arrives.
    fn get_result(&mut self, i: usize, cx: &mut AppCtx) {
        let Some((src, part)) = self.results.get(i).cloned() else { return };
        let work = new_work_dir();
        cx.send(AppMsg::Import(import::Msg::Gather { part: part.clone(), work: work.clone(), waiting: cx.sources.len() }));
        let (tx, sources, ctx) = (cx.tx.clone(), cx.sources.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            combine::gather(&sources, &part, src, &work, |source, found| {
                let _ = tx.send(AppMsg::Import(import::Msg::Answer { work: work.clone(), source: source.to_string(), found }));
                ctx.request_repaint();
            });
        });
    }
}
