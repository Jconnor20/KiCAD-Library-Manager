use super::import;
use crate::{AppCtx, Msg as AppMsg, new_work_dir};
use eframe::egui::{self, RichText};
use klm_core::cache;
use klm_core::store::Store;
use klm_core::currency::{Rates, display_price, price_in};
use klm_core::part::Part;
use klm_core::search_filter::{Filter, SortKey, view};
use klm_core::sources::{categories::{self, Category}, combine, thumbs};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Thumbnail downloads running at once.
const THUMB_JOBS: usize = 6;
const THUMB: f32 = 40.0;

pub enum Msg {
    Done(Vec<(usize, Part)>),
    /// The sources were rebuilt, so the indexes in the results no longer match.
    ClearResults,
    /// Exchange rates for the display currency (`None` if they could not be loaded).
    Rates(Option<Rates>),
    /// A source's category list (or why it has none).
    Categories(usize, Result<Vec<Category>, String>),
    /// A thumbnail finished downloading (`None` if it failed).
    Thumb(String, Option<PathBuf>),
}

enum Action {
    None,
    Search(String),
    /// Fetch result `i` (an index into the unsorted, unfiltered results).
    Get(usize),
    /// Pin (true) or unpin a search.
    Save(String, bool),
    /// Star (true) or unstar a part by MPN.
    Star(String, bool),
    /// Load source `i`'s category list.
    LoadCategories(usize),
}

/// The "Browse by category" section.
#[derive(Default)]
struct Cats {
    /// Source shown in the picker (an index into the app's source list).
    source: usize,
    lists: HashMap<usize, Result<Vec<Category>, String>>,
    loading: HashSet<usize>,
    filter: String,
    /// Searches run inside this category until it is cleared.
    picked: Option<(usize, Category)>,
}

/// The side panel's lists, read from the part store.
#[derive(Default)]
struct Side {
    loaded: bool,
    recent: Vec<String>,
    saved: Vec<String>,
    favourites: Vec<String>,
    imported: Vec<String>,
}

impl Side {
    fn load(&mut self) {
        self.loaded = true;
        let Some(store) = Store::default_path().and_then(|p| Store::open(&p).ok()) else { return };
        self.recent = store.recent_searches(15).unwrap_or_default();
        self.saved = store.saved_searches().unwrap_or_default();
        self.favourites = store.favourites().unwrap_or_default();
        self.imported = store.recent_imports(15).unwrap_or_default();
    }
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
    side: Side,
    cats: Cats,
    /// Downloaded thumbnails by image URL; `None` once a download has failed.
    thumbs: HashMap<String, Option<PathBuf>>,
    thumbs_loading: HashSet<String>,
}

impl SearchTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        egui_extras::install_image_loaders(ui.ctx()); // idempotent
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
        self.load_thumbs(cx);
        match action {
            Action::None => {}
            Action::Search(q) => self.run_search(q, cx),
            Action::LoadCategories(i) => self.load_categories(i, cx),
            Action::Get(i) => self.get_result(i, cx),
            Action::Save(q, on) => {
                self.with_store(cx, |s| s.set_saved_search(&q, on));
                self.side.load();
            }
            Action::Star(mpn, on) => {
                self.with_store(cx, |s| s.set_favourite(&mpn, on));
                self.side.load();
            }
        }
    }

    /// The store is only a convenience, so a failure is logged and nothing else happens.
    fn with_store(&self, cx: &mut AppCtx, f: impl FnOnce(&Store) -> anyhow::Result<()>) {
        let r = Store::default_path().ok_or_else(|| anyhow::anyhow!("no config directory")).and_then(|p| Store::open(&p)).and_then(|s| f(&s));
        if let Err(e) = r {
            cx.log.push(format!("Part store: {e:#}"));
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
            Msg::ClearResults => {
                self.results.clear();
                self.cats = Cats::default();
            }
            Msg::Categories(i, list) => {
                self.cats.loading.remove(&i);
                self.cats.lists.insert(i, list);
            }
            Msg::Thumb(url, path) => {
                self.thumbs_loading.remove(&url);
                self.thumbs.insert(url, path);
            }
            Msg::Rates(rates) => self.rates = rates.or(self.rates.take()),
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, source_names: &[&str], busy: bool, currency: &mut String) -> Action {
        let mut action = Action::None;
        if source_names.is_empty() {
            ui.label("No search sources are enabled yet. Enable one in Settings.");
            return action;
        }
        if !self.side.loaded {
            self.side.load();
        }
        egui::Panel::right("search_side").default_size(190.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.side_ui(ui, busy, &mut action));
        });
        ui.horizontal(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("MPN, LCSC/DigiKey number or keywords").desired_width(320.0));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let can = !self.searching && (!self.query.trim().is_empty() || self.cats.picked.is_some());
            if (ui.add_enabled(can, egui::Button::new("Search")).clicked() || (enter && can)) && can {
                action = Action::Search(self.query.trim().to_string());
            }
            if self.searching {
                ui.spinner();
            }
            if self.searching {
                ui.weak(format!("Searching: {}", source_names.join(", ")));
            }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new("Browse by category").default_open(self.cats.picked.is_some()).show(ui, |ui| self.categories_ui(ui, source_names, &mut action));
        let shown = {
            let target = (!currency.is_empty()).then_some(currency.as_str());
            view(&self.results, &self.filter, None, &|p| price_in(&p.unit_price, target, self.rates.as_ref())).len()
        };
        self.filters_ui(ui, source_names, currency, shown);
        let target = (!currency.is_empty()).then_some(currency.as_str());
        let rates = self.rates.as_ref();
        let price = |p: &Part| price_in(&p.unit_price, target, rates);
        let rows = view(&self.results, &self.filter, self.sort, &price);
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("results").striped(true).num_columns(9).spacing([14.0, 4.0]).show(ui, |ui| {
                ui.label("");
                for (h, key) in [("Source", SortKey::Source), ("MPN", SortKey::Mpn), ("Manufacturer", SortKey::Manufacturer), ("Description", SortKey::Description), ("Package", SortKey::Package), ("Stock", SortKey::Stock), ("Unit price (qty 1)", SortKey::Price)] {
                    let h = if key == SortKey::Price { &format!("{h} ({})", target.unwrap_or("native")) } else { h };
                    let arrow = match self.sort {
                        Some((k, asc)) if k == key => if asc { " ▲" } else { " ▼" },
                        _ => " ⇅",
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
                    match self.thumbs.get(&p.image_url) {
                        Some(Some(path)) => drop(ui.add(egui::Image::new(format!("file://{}", path.to_string_lossy().replace('\\', "/"))).fit_to_exact_size(egui::vec2(THUMB, THUMB)))),
                        _ => drop(ui.allocate_space(egui::vec2(THUMB, THUMB))),
                    }
                    ui.label(source_names.get(*src).copied().unwrap_or("?"));
                    let starred = self.side.favourites.contains(&p.mpn);
                    ui.label(RichText::new(&p.mpn).strong()).context_menu(|ui| {
                        if ui.button(if starred { "Remove from favourites" } else { "Add to favourites" }).clicked() {
                            action = Action::Star(p.mpn.clone(), !starred);
                            ui.close();
                        }
                    });
                    ui.label(&p.manufacturer);
                    let short: String = p.description.chars().take(60).collect();
                    ui.label(short).on_hover_text(&p.description);
                    ui.label(&p.package);
                    ui.label(p.stock.map_or(String::new(), |s| s.to_string()));
                    let mut shown = display_price(&p.unit_price, target, rates);
                    let cur = if p.price_currency.is_empty() { klm_core::currency::detect(&p.unit_price).unwrap_or_default() } else { p.price_currency.clone() };
                    if shown == p.unit_price && !cur.is_empty() && !shown.contains(&cur) {
                        shown = format!("{cur} {}", shown.trim_start_matches(['$', '€', '£']));
                    }
                    if let Some(m) = p.moq.filter(|m| *m > 1) {
                        shown += &format!("  MOQ {m}");
                    }
                    if let Some(n) = p.pack_qty.filter(|n| *n > 1) {
                        shown += &format!("  {} ×{n}", if p.packaging.is_empty() { "Pack" } else { &p.packaging });
                    }
                    let price = ui.label(&shown);
                    let breaks: Vec<String> = p.price_breaks.iter().map(|(q, v)| format!("{q}+: {v} {cur}")).collect();
                    if !breaks.is_empty() {
                        price.on_hover_text(format!("Price breaks\n{}", breaks.join("\n")));
                    } else if target.is_some() && !p.unit_price.is_empty() && shown == p.unit_price {
                        price.on_hover_text("Not converted: unknown currency or no exchange rates");
                    }
                    ui.horizontal(|ui| {
                        let known = p.supplier_ids.iter().any(|(k, _)| k == "LCSC") || p.source.contains("LCSC");
                        let icons = format!("{}{}", if p.datasheet_url.is_empty() { "" } else { "📄" }, if known { "🔣⏚🧊" } else { "" });
                        ui.label(if icons.is_empty() { "?".to_string() } else { icons }).on_hover_text(cad_summary(p, known));
                        if ui.add_enabled(!busy, egui::Button::new("Import")).on_hover_text("Fetch symbol, footprint, 3D model and datasheet into your library").clicked() {
                            action = Action::Get(i);
                        }
                    });
                    ui.end_row();
                }
            });
        });
        action
    }

    fn categories_ui(&mut self, ui: &mut egui::Ui, source_names: &[&str], action: &mut Action) {
        let c = &mut self.cats;
        c.source = c.source.min(source_names.len() - 1);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("category_source").selected_text(source_names[c.source]).show_ui(ui, |ui| {
                for (i, n) in source_names.iter().enumerate() {
                    ui.selectable_value(&mut c.source, i, *n);
                }
            });
            ui.add(egui::TextEdit::singleline(&mut c.filter).hint_text("Filter categories").desired_width(200.0));
            if let Some((_, cat)) = &c.picked {
                ui.label(RichText::new(format!("Searching in: {}", cat.path)).strong());
                if ui.small_button("✕ Clear").clicked() {
                    c.picked = None;
                }
            }
        });
        match c.lists.get(&c.source) {
            None if c.loading.contains(&c.source) => drop(ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Loading categories…");
            })),
            None => *action = Action::LoadCategories(c.source),
            Some(Err(e)) => drop(ui.weak(e)),
            Some(Ok(list)) => {
                let f = c.filter.trim().to_lowercase();
                let mut picked = None;
                egui::ScrollArea::vertical().max_height(180.0).id_salt("category_list").show(ui, |ui| {
                    for cat in list.iter().filter(|k| f.is_empty() || k.path.to_lowercase().contains(&f)).take(200) {
                        if ui.selectable_label(c.picked.as_ref().is_some_and(|(s, p)| *s == c.source && p.id == cat.id), &cat.path).clicked() {
                            picked = Some(cat.clone());
                        }
                    }
                });
                if let Some(cat) = picked {
                    c.picked = Some((c.source, cat));
                    *action = Action::Search(self.query.trim().to_string());
                }
            }
        }
    }

    fn side_ui(&mut self, ui: &mut egui::Ui, busy: bool, action: &mut Action) {
        let can = !self.searching && !busy;
        ui.strong("Saved searches");
        for q in &self.side.saved {
            ui.horizontal(|ui| {
                if ui.small_button("📌").on_hover_text("Unpin").clicked() {
                    *action = Action::Save(q.clone(), false);
                }
                if ui.add_enabled(can, egui::Link::new(q)).clicked() {
                    self.query = q.clone();
                    *action = Action::Search(q.clone());
                }
            });
        }
        ui.add_space(6.0);
        ui.strong("Recent searches");
        for q in &self.side.recent {
            ui.horizontal(|ui| {
                let saved = self.side.saved.contains(q);
                if ui.add_enabled(!saved, egui::Button::new("📌").small()).on_hover_text("Save this search").clicked() {
                    *action = Action::Save(q.clone(), true);
                }
                if ui.add_enabled(can, egui::Link::new(q)).clicked() {
                    self.query = q.clone();
                    *action = Action::Search(q.clone());
                }
            });
        }
        ui.add_space(6.0);
        ui.strong("Favourites");
        for mpn in &self.side.favourites {
            ui.horizontal(|ui| {
                if ui.small_button("★").on_hover_text("Unstar").clicked() {
                    *action = Action::Star(mpn.clone(), false);
                }
                if ui.add_enabled(can, egui::Link::new(mpn)).on_hover_text("Search for this part").clicked() {
                    self.query = mpn.clone();
                    *action = Action::Search(mpn.clone());
                }
            });
        }
        ui.add_space(6.0);
        ui.strong("Recently imported");
        for mpn in &self.side.imported {
            ui.horizontal(|ui| {
                let starred = self.side.favourites.contains(mpn);
                if ui.small_button(if starred { "★" } else { "☆" }).clicked() {
                    *action = Action::Star(mpn.clone(), !starred);
                }
                if ui.add_enabled(can, egui::Link::new(mpn)).clicked() {
                    self.query = mpn.clone();
                    *action = Action::Search(mpn.clone());
                }
            });
        }
    }

    fn filters_ui(&mut self, ui: &mut egui::Ui, source_names: &[&str], currency: &mut String, shown: usize) {
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

    fn load_categories(&mut self, i: usize, cx: &mut AppCtx) {
        if i >= cx.sources.len() {
            return;
        }
        let sources = cx.sources.clone();
        self.cats.loading.insert(i);
        let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            let list = categories::cached(sources[i].as_ref()).map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppMsg::Search(Msg::Categories(i, list)));
            ctx.request_repaint();
        });
    }

    /// Starts downloads for result thumbnails not fetched yet, a few at a time.
    fn load_thumbs(&mut self, cx: &AppCtx) {
        let wanted = self.results.iter().map(|(_, p)| &p.image_url).filter(|u| !u.is_empty() && !self.thumbs.contains_key(*u) && !self.thumbs_loading.contains(*u));
        let room = THUMB_JOBS.saturating_sub(self.thumbs_loading.len());
        let urls: Vec<String> = wanted.take(room).cloned().collect::<HashSet<_>>().into_iter().collect();
        for url in urls {
            self.thumbs_loading.insert(url.clone());
            let (tx, ctx) = (cx.tx.clone(), cx.ctx.clone());
            std::thread::spawn(move || {
                let path = thumbs::get(&url).ok();
                let _ = tx.send(AppMsg::Search(Msg::Thumb(url, path)));
                ctx.request_repaint();
            });
        }
    }

    fn run_search(&mut self, query: String, cx: &mut AppCtx) {
        self.searching = true;
        self.results.clear();
        if !query.is_empty() {
            self.with_store(cx, |s| s.log("search", &query));
        }
        self.side.load();
        let category = self.cats.picked.clone().filter(|(i, _)| *i < cx.sources.len()).map(|(i, c)| (i, c.id));
        let ttl = cx.settings.cache_ttl_minutes.unwrap_or(cache::DEFAULT_TTL_MINUTES) as i64 * 60;
        let (tx, sources, ctx) = (cx.tx.clone(), cx.sources.clone(), cx.ctx.clone());
        std::thread::spawn(move || {
            if let Some((i, id)) = category {
                let src = &sources[i];
                let key = format!("cat:{id}:{}", query.to_lowercase());
                let store = Store::default_path().and_then(|p| Store::open(&p).ok());
                match cache::search(store.as_ref(), src.name(), &key, ttl, || src.search_in_category(&id, &query)) {
                    Ok(parts) => {
                        let _ = tx.send(AppMsg::Search(Msg::Done(parts.into_iter().map(|p| (i, p)).collect())));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Log(format!("{} category search failed: {e:#}", src.name())));
                        let _ = tx.send(AppMsg::Search(Msg::Done(vec![])));
                    }
                }
                ctx.request_repaint();
                return;
            }
            let mut results = vec![];
            let query = &query;
            std::thread::scope(|s| {
                let handles: Vec<_> = sources.iter().enumerate().map(|(i, src)| s.spawn(move || (i, src.name(), search_cached(src.as_ref(), query, ttl)))).collect();
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

/// What importing `p` is known to bring, for the CAD icon tooltip. Only LCSC/EasyEDA parts are known
/// ahead of the import; a distributor-only part may still match an LCSC part when imported.
fn cad_summary(p: &Part, known: bool) -> String {
    let datasheet = if p.datasheet_url.is_empty() { "no datasheet link" } else { "📄 datasheet available" };
    if known {
        format!("{datasheet}\n🔣 symbol and ⏚ footprint: provided by LCSC/EasyEDA\n🧊 3D model: usually provided (confirmed at import)\nSPICE model: not provided")
    } else {
        format!("{datasheet}\nSymbol, footprint, 3D and SPICE model: not known until imported (other sources are asked then)")
    }
}

/// One source's results, from the part store if fresh enough (each thread opens its own connection).
fn search_cached(src: &dyn klm_core::sources::Source, query: &str, ttl_secs: i64) -> anyhow::Result<Vec<Part>> {
    let store = Store::default_path().and_then(|p| Store::open(&p).ok());
    cache::search(store.as_ref(), src.name(), query, ttl_secs, || src.search(query))
}
