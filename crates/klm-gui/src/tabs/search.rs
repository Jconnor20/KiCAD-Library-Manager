use super::import;
use crate::{AppCtx, Msg as AppMsg, new_work_dir};
use eframe::egui::{self, RichText};
use klm_core::part::Part;

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
    /// Fetch result `i` (an index into the results).
    Get(usize),
}

/// One search box over every enabled source; results from all of them in one table.
#[derive(Default)]
pub struct SearchTab {
    query: String,
    searching: bool,
    /// (index into the app's source list, result)
    results: Vec<(usize, Part)>,
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
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("results").striped(true).num_columns(8).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["Source", "MPN", "Manufacturer", "Description", "Package", "Stock", "Price", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for (i, (src, p)) in self.results.iter().enumerate() {
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
