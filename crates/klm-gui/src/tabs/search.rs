use super::import;
use crate::{AppCtx, Msg as AppMsg, new_work_dir};
use eframe::egui::{self, RichText};
use klm_core::part::Part;
use klm_core::search_filter::{Filter, SortKey, view};

pub enum Msg {
    Done(Vec<(usize, Part)>),
    /// A download started by "Get…" has finished (or failed).
    FetchDone,
    /// The sources were rebuilt, so the indexes in the results no longer match.
    ClearResults,
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
}

impl SearchTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let names: Vec<&str> = cx.sources.iter().map(|s| s.name()).collect();
        match self.show(ui, &names, cx.busy) {
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
            Msg::FetchDone => cx.busy = false,
            Msg::ClearResults => self.results.clear(),
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, source_names: &[&str], busy: bool) -> Action {
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
        self.filters_ui(ui, source_names);
        let rows = view(&self.results, &self.filter, self.sort);
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("results").striped(true).num_columns(8).spacing([14.0, 4.0]).show(ui, |ui| {
                for (h, key) in [("Source", SortKey::Source), ("MPN", SortKey::Mpn), ("Manufacturer", SortKey::Manufacturer), ("Description", SortKey::Description), ("Package", SortKey::Package), ("Stock", SortKey::Stock), ("Price", SortKey::Price)] {
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
                    ui.label(&p.unit_price);
                    if ui.add_enabled(!busy, egui::Button::new("Get…")).clicked() {
                        action = Action::Get(i);
                    }
                    ui.end_row();
                }
            });
        });
        action
    }

    fn filters_ui(&mut self, ui: &mut egui::Ui, source_names: &[&str]) {
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
            ui.add(egui::TextEdit::singleline(&mut f.max_price).hint_text("Max unit price").desired_width(90.0));
            ui.add(egui::TextEdit::singleline(&mut f.text).hint_text("MPN, manufacturer, description").desired_width(190.0));
        });
        ui.horizontal(|ui| {
            ui.weak(format!("{} of {} results", view(&self.results, &self.filter, None).len(), self.results.len()));
            if ui.add_enabled(!self.filter.is_empty(), egui::Button::new("Clear filters")).clicked() {
                self.filter = Filter::default();
            }
        });
        ui.add_space(4.0);
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

    /// Downloads a search result's CAD, or opens its CAD page if the source has none to download.
    fn get_result(&mut self, i: usize, cx: &mut AppCtx) {
        let Some((src, part)) = self.results.get(i).cloned() else { return };
        cx.busy = true;
        let (tx, sources, ctx) = (cx.tx.clone(), cx.sources.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let source = &sources[src];
            let work = new_work_dir();
            // Sources write their files straight into `work`.
            let _ = std::fs::create_dir_all(&work);
            match source.fetch(&part, &work) {
                Ok(Some(p)) => {
                    for w in &p.warnings {
                        let _ = tx.send(AppMsg::Log(format!("{}: {}: {w}", source.name(), p.mpn)));
                    }
                    let _ = tx.send(AppMsg::Import(import::Msg::Fetched(p, work)));
                }
                Ok(None) => {
                    let _ = std::fs::remove_dir_all(&work);
                    match source.cad_page_url(&part) {
                        Some(url) => {
                            let _ = open::that(&url);
                            let _ = tx.send(AppMsg::Import(import::Msg::CadOpened(part.clone())));
                            let _ = tx.send(AppMsg::Log(format!("Opened {url}. Download the KiCad ZIP and it will appear under Import.")));
                        }
                        None => drop(tx.send(AppMsg::Log(format!("{} has no CAD files for {}", source.name(), part.mpn)))),
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&work);
                    let _ = tx.send(AppMsg::Log(format!("{}: could not get {}: {e:#}", source.name(), part.mpn)));
                }
            }
            let _ = tx.send(AppMsg::Search(Msg::FetchDone));
            ctx.request_repaint();
        });
    }
}
