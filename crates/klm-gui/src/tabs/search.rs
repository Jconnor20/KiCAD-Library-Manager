use super::{import, library};
use super::ui_kit::{self, Col, Level, Section, Sort};
use crate::{AppCtx, Msg as AppMsg, Tab, new_work_dir};
use crate::{shell, theme};
use eframe::egui::{self, RichText, containers::menu::{MenuButton, MenuConfig}};
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
/// Sortable table columns as (column index, key).
const SORT_KEYS: [(usize, SortKey); 7] = [(1, SortKey::Source), (2, SortKey::Mpn), (3, SortKey::Manufacturer), (4, SortKey::Description), (5, SortKey::Package), (6, SortKey::Stock), (7, SortKey::Price)];

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
    /// Search for this text now (BOM's "Search").
    Query(String),
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
    /// Open Settings on this page.
    Settings(Section),
    /// Show this MPN in the Library tab.
    OpenLibrary(String),
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
    sort: Option<Sort>,
    /// A search has finished, so "no results" is a fact rather than a blank start.
    searched: bool,
    /// Lower-case MPNs of the parts already in the libraries.
    in_library: HashSet<String>,
    was_busy: bool,
    /// The result shown in the detail pane (an index into `results`).
    selected: Option<usize>,
    /// The price column's header; leaked because `Col` wants a `&'static str` and it changes only with the currency.
    price_title: (String, &'static str),
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
        if self.was_busy && !cx.busy {
            self.refresh_library(cx);
        }
        self.was_busy = cx.busy;
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
            Action::Settings(s) => cx.open_settings(s),
            Action::OpenLibrary(mpn) => {
                cx.send(AppMsg::Library(library::Msg::Show(mpn)));
                cx.tab = Tab::Library;
            }
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
            Msg::Query(q) => {
                self.query = q.clone();
                self.run_search(q, cx);
            }
            Msg::Done(results) => {
                self.searching = false;
                self.searched = true;
                self.refresh_library(cx);
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

    fn refresh_library(&mut self, cx: &AppCtx) {
        self.in_library = cx.roots().iter().flat_map(|r| r.list_parts()).map(|p| p.mpn.to_lowercase()).collect();
    }

    fn price_title(&mut self, currency: &str) -> &'static str {
        if self.price_title.1.is_empty() || self.price_title.0 != currency {
            let text = if currency.is_empty() { "Price (as listed)".to_string() } else { format!("Price ({currency})") };
            self.price_title = (currency.to_string(), Box::leak(text.into_boxed_str()));
        }
        self.price_title.1
    }

    fn show(&mut self, ui: &mut egui::Ui, source_names: &[&str], busy: bool, currency: &mut String) -> Action {
        let mut action = Action::None;
        let pal = theme::palette(ui.visuals());
        if source_names.is_empty() {
            self.no_source(ui, &mut action);
            return action;
        }
        if !self.side.loaded {
            self.side.load();
        }
        let wide = ui.available_width() >= 1000.0;
        let target = (!currency.is_empty()).then_some(currency.clone());
        let target = target.as_deref();
        let rates = self.rates.as_ref();
        let price = |p: &Part| price_in(&p.unit_price, target, rates);
        let key = self.sort.and_then(|s| SORT_KEYS.iter().find(|(c, _)| *c == s.col).map(|(_, k)| (*k, s.asc)));
        let rows = view(&self.results, &self.filter, key, &price);
        let shown = rows.len();
        // The first row is selected until the user picks another.
        if self.selected.is_none_or(|i| i >= self.results.len()) {
            self.selected = rows.first().copied();
        }

        // Wide: the detail pane runs the full height beside the header. Narrow: it sits under the table, which gets the rest.
        let sel = self.selected.filter(|_| !self.results.is_empty());
        let fill = egui::Frame::new().fill(pal.panel).inner_margin(egui::Margin::same(12));
        match sel {
            Some(sel) if wide => {
                egui::Panel::right("search_detail").default_size(300.0).size_range(260.0..=440.0).frame(fill).show(ui, |ui| self.detail_ui(ui, sel, source_names, busy, &mut action));
                self.main_ui(ui, source_names, busy, currency, &rows, shown, wide, &mut action);
            }
            // Too short a window for both: the table keeps the room and the row buttons still work.
            Some(sel) if ui.available_height() >= 340.0 => {
                let h = 200.0f32.min(ui.available_height() - 240.0);
                let size = egui::vec2(ui.available_width(), ui.available_height() - h);
                ui.allocate_ui(size, |ui| self.main_ui(ui, source_names, busy, currency, &rows, shown, wide, &mut action));
                ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
                    fill.show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.detail_ui(ui, sel, source_names, busy, &mut action);
                    });
                });
            }
            _ => self.main_ui(ui, source_names, busy, currency, &rows, shown, wide, &mut action),
        }
        action
    }

    /// Query bar, filters and the results table.
    #[allow(clippy::too_many_arguments)]
    fn main_ui(&mut self, ui: &mut egui::Ui, source_names: &[&str], busy: bool, currency: &mut String, rows: &[usize], shown: usize, wide: bool, action: &mut Action) {
        let pal = theme::palette(ui.visuals());
        ui.horizontal(|ui| {
            ui.label(RichText::new("Search").heading().strong());
            let edit = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("MPN, LCSC/DigiKey number or keywords").desired_width(if wide { 300.0 } else { 220.0 }).margin(egui::Margin::symmetric(8, 6)));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let can = !self.searching && (!self.query.trim().is_empty() || self.cats.picked.is_some());
            if (ui.add_enabled(can, shell::primary(ui, "Search")).clicked() || (enter && can)) && can {
                *action = Action::Search(self.query.trim().to_string());
            }
            if self.searching {
                ui.spinner();
            }
            let cfg = MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
            MenuButton::new("Saved & recent ▾").config(cfg).ui(ui, |ui| {
                ui.set_min_width(240.0);
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| self.side_ui(ui, busy, action));
            });
            if wide {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let text = if self.searching { format!("Searching: {}", source_names.join(", ")) } else { format!("Sources: {}", source_names.join(", ")) };
                    ui.label(RichText::new(text).size(12.0).color(pal.muted));
                });
            }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new("Browse by category").default_open(self.cats.picked.is_some()).show(ui, |ui| self.categories_ui(ui, source_names, action));
        self.filters_ui(ui, source_names, currency, shown);
        if self.results.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                if self.searching {
                    ui.weak("Searching…");
                } else if self.searched {
                    ui.label("No results. Try fewer words, or the exact MPN or supplier number.");
                } else {
                    ui.label(RichText::new("Search for a part").heading());
                    ui.weak("Type an MPN, an LCSC or DigiKey number, or keywords, then press Enter. Results from every enabled source appear here.");
                }
            });
            return;
        }
        let target = (!currency.is_empty()).then_some(currency.clone());
        let target = target.as_deref();
        let title = self.price_title(target.unwrap_or(""));
        let rates = self.rates.as_ref();
        let w = |narrow: f32, wide_: f32| if wide { wide_ } else { narrow };
        let cols = [
            Col::new("Source", w(46.0, 54.0)),
            Col::new("MPN", w(104.0, 120.0)),
            Col::new("Manufacturer", w(84.0, 100.0)),
            Col::rest("Description"),
            Col::new("Package", w(58.0, 66.0)),
            Col::new("Stock", w(48.0, 58.0)),
            Col::new(title, w(78.0, 92.0)),
            Col::new("CAD", w(112.0, 118.0)).no_sort(),
            Col::new("", 60.0).no_sort(),
        ];
        let mut sort = self.sort.unwrap_or(Sort { col: usize::MAX, asc: true });
        // `i` indexes `self.results`, not the sorted rows, so "Import" survives any sort or filter.
        let mut picked = None;
        let rows_n = rows.len();
        ui_kit::table(ui, "results", &cols, &mut sort, rows_n, |r, row| {
            let i = rows[r];
            let (src, p) = &self.results[i];
            row.set_selected(self.selected == Some(i));
            let name = source_names.get(*src).copied().unwrap_or("?");
            row.col(|ui| drop(ui.label(RichText::new(source_tag(name)).monospace().size(10.5).background_color(pal.row)).on_hover_text(name)));
            let starred = self.side.favourites.contains(&p.mpn);
            let owned = self.in_library.contains(&p.mpn.to_lowercase());
            row.col(|ui| { ui.horizontal(|ui| {
                let mpn = ui.label(RichText::new(&p.mpn).monospace().strong());
                if owned {
                    let b = ui_kit::badge(ui, "In library", Level::Ok);
                    if ui.interact(b.rect, ui.auto_id_with(("owned", i)), egui::Sense::click()).on_hover_text("Open in Library").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        *action = Action::OpenLibrary(p.mpn.clone());
                    }
                }
                mpn.context_menu(|ui| {
                    if owned && ui.button("Open in Library").clicked() {
                        *action = Action::OpenLibrary(p.mpn.clone());
                        ui.close();
                    }
                    if ui.button(if starred { "Remove from favourites" } else { "Add to favourites" }).clicked() {
                        *action = Action::Star(p.mpn.clone(), !starred);
                        ui.close();
                    }
                });
            }); });
            row.col(|ui| drop(ui.label(&p.manufacturer)));
            row.col(|ui| drop(ui.label(&p.description).on_hover_text(&p.description)));
            row.col(|ui| drop(ui.label(&p.package)));
            row.col(|ui| drop(ui.label(RichText::new(p.stock.map_or(String::new(), group)).monospace())));
            row.col(|ui| {
                let (shown, tip) = price_cell(p, target, rates);
                let l = ui.label(RichText::new(&shown).monospace());
                if !tip.is_empty() {
                    l.on_hover_text(tip);
                }
            });
            row.col(|ui| { ui.horizontal(|ui| {
                let known = p.supplier_ids.iter().any(|(k, _)| k == "LCSC") || p.source.contains("LCSC");
                ui.spacing_mut().item_spacing.x = 3.0;
                let tip = cad_summary(p, known);
                if known {
                    for (t, lvl) in [("SYM", Level::Ok), ("FP", Level::Ok), ("3D", Level::Info)].into_iter().chain((!p.sim_files.is_empty()).then_some(("SPICE", Level::Ok))) {
                        ui_kit::badge(ui, t, lvl).on_hover_text(&tip);
                    }
                } else {
                    ui.weak("not known").on_hover_text(tip);
                }
            }); });
            row.col(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Import").small()).on_hover_text("Fetch symbol, footprint, 3D model and datasheet into your library").clicked() {
                    *action = Action::Get(i);
                }
            });
            if row.response().clicked() {
                picked = Some(i);
            }
        });
        self.sort = (sort.col != usize::MAX).then_some(sort);
        if picked.is_some() {
            self.selected = picked;
        }
    }

    /// "No search source is ready": which sources exist and what each needs.
    fn no_source(&self, ui: &mut egui::Ui, action: &mut Action) {
        let pal = theme::palette(ui.visuals());
        let warn = Level::Warn.color(ui.visuals());
        ui.label(RichText::new("Search").heading().strong());
        ui.add_space(8.0);
        egui::Frame::new().fill(warn.gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0, warn)).corner_radius(4).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width().min(560.0));
            ui.label(RichText::new("No search source is ready.").strong());
            ui.label(RichText::new("Enable at least one source, or add an API key, to search for parts here. You can still import ZIPs.").color(pal.muted));
        });
        ui.add_space(8.0);
        egui::Grid::new("sources_state").num_columns(3).spacing([24.0, 8.0]).show(ui, |ui| {
            for h in ["Source", "State", ""] {
                shell::caption(ui, h);
            }
            ui.end_row();
            for s in klm_core::credentials::SOURCES.iter().filter(|s| s.searchable) {
                ui.label(s.name);
                let needs_key = !s.fields.is_empty();
                ui_kit::badge(ui, if needs_key { "No key" } else { "Off" }, Level::Warn);
                let section = match s.name {
                    "LCSC" => Section::Lcsc,
                    "DigiKey" => Section::DigiKey,
                    "Mouser" => Section::Mouser,
                    _ => Section::Nexar,
                };
                if ui.small_button(if needs_key { "Add key…" } else { "Enable…" }).clicked() {
                    *action = Action::Settings(section);
                }
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        if ui.add(shell::primary(ui, "Open Settings · Sources")).clicked() {
            *action = Action::Settings(Section::Sources);
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Keys are kept in the system credential store, not in a file.").size(12.0).color(pal.muted));
    }

    /// The selected result: what is known about it before importing, and the two actions.
    fn detail_ui(&self, ui: &mut egui::Ui, i: usize, source_names: &[&str], busy: bool, action: &mut Action) {
        let pal = theme::palette(ui.visuals());
        let Some((src, p)) = self.results.get(i) else { return };
        let name = source_names.get(*src).copied().unwrap_or("?");
        egui::Panel::bottom("search_detail_actions").frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 8))).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Find alternatives")).on_hover_text("Search again with this part's description, to see similar parts").clicked() {
                    *action = Action::Search(p.description.split_whitespace().take(6).collect::<Vec<_>>().join(" "));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(!busy, shell::primary(ui, "Import to library")).on_hover_text("Fetch symbol, footprint, 3D model and datasheet into your library").clicked() {
                        *action = Action::Get(i);
                    }
                });
            });
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let starred = self.side.favourites.contains(&p.mpn);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&p.mpn).monospace().strong().size(16.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(if starred { "★" } else { "☆" }).frame(false)).on_hover_text(if starred { "Remove from favourites" } else { "Add to favourites" }).clicked() {
                        *action = Action::Star(p.mpn.clone(), !starred);
                    }
                });
            });
            ui.label(RichText::new([p.manufacturer.as_str(), p.description.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ")).color(pal.muted));
            let ids: Vec<String> = p.supplier_ids.iter().map(|(k, v)| format!("{k} {v}")).collect();
            if !ids.is_empty() {
                ui.label(RichText::new(ids.join(" · ")).monospace().size(11.5).color(pal.muted));
            }
            ui.add_space(8.0);
            ui.horizontal_top(|ui| {
                if let Some(Some(path)) = self.thumbs.get(&p.image_url) {
                    shell::card(ui, |ui| ui.add(egui::Image::new(format!("file://{}", path.to_string_lossy().replace('\\', "/"))).fit_to_exact_size(egui::vec2(84.0, 84.0))));
                }
                ui.vertical(|ui| {
                    shell::caption(ui, "Files on import");
                    let known = p.supplier_ids.iter().any(|(k, _)| k == "LCSC") || p.source.contains("LCSC");
                    ui.label(RichText::new(cad_summary(p, known)).size(12.0).color(pal.muted));
                    ui.label(RichText::new("Symbol, footprint and 3D previews appear in the import dialog.").size(12.0).color(pal.muted));
                });
            });
            ui.add_space(10.0);
            shell::caption(ui, &format!("Price breaks · {name}"));
            if p.price_breaks.is_empty() {
                ui.label(if p.unit_price.is_empty() { "No price listed".to_string() } else { p.unit_price.clone() });
            } else {
                egui::Grid::new("detail_breaks").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    shell::caption(ui, "Qty");
                    shell::caption(ui, &format!("Unit ({})", if p.price_currency.is_empty() { "as listed" } else { &p.price_currency }));
                    ui.end_row();
                    for (q, v) in &p.price_breaks {
                        ui.label(RichText::new(group(*q)).monospace());
                        ui.label(RichText::new(format!("{v}")).monospace());
                        ui.end_row();
                    }
                });
            }
            for (label, v) in [("Minimum order", p.moq.filter(|m| *m > 1)), ("Pack of", p.pack_qty.filter(|n| *n > 1))] {
                if let Some(v) = v {
                    ui.label(RichText::new(format!("{label} {v}")).size(12.0).color(pal.muted));
                }
            }
            if let Some(s) = p.stock {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, if s > 0 { Level::Ok } else { Level::Warn }.color(ui.visuals()));
                    ui.label(format!("{} in stock at {name}", group(s)));
                });
            }
            let mut specs: Vec<(String, String)> = vec![];
            for (k, v) in [("Package", &p.package), ("JLCPCB class", &p.jlc_type), ("Lifecycle", &p.lifecycle), ("Packaging", &p.packaging)] {
                if !v.is_empty() {
                    specs.push((k.to_string(), v.clone()));
                }
            }
            if let Some(d) = p.lead_time_days {
                specs.push(("Lead time".into(), format!("{d} days")));
            }
            specs.extend(p.params.iter().take(8).cloned());
            if !specs.is_empty() || !p.datasheet_url.is_empty() {
                ui.add_space(10.0);
                shell::caption(ui, "Specs");
                egui::Grid::new("detail_specs").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (k, v) in &specs {
                        ui.label(RichText::new(k).color(pal.muted));
                        ui.label(v);
                        ui.end_row();
                    }
                    if !p.datasheet_url.is_empty() {
                        ui.label(RichText::new("Datasheet").color(pal.muted));
                        ui.hyperlink_to("open", &p.datasheet_url);
                        ui.end_row();
                    }
                });
            }
        });
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
        ui.strong("Saved");
        if self.side.saved.is_empty() {
            ui.weak("Click ☆ on a recent search to keep it here.");
        }
        for q in &self.side.saved {
            ui.horizontal(|ui| {
                if ui.small_button("★").on_hover_text("Remove from Saved").clicked() {
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
                if ui.small_button(if saved { "★" } else { "☆" }).on_hover_text(if saved { "Remove from Saved" } else { "Save this search" }).clicked() {
                    *action = Action::Save(q.clone(), !saved);
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
                if ui.small_button(if starred { "★" } else { "☆" }).on_hover_text(if starred { "Remove from favourites" } else { "Add to favourites" }).clicked() {
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
                if shell::chip(ui, source_names.get(s).copied().unwrap_or("?"), shown).clicked() {
                    shown = !shown;
                    f.hidden_sources.retain(|h| *h != s);
                    if !shown {
                        f.hidden_sources.push(s);
                    }
                }
            }
            if shell::chip(ui, "In stock", f.in_stock).clicked() {
                f.in_stock = !f.in_stock;
            }
            egui::ComboBox::from_id_salt("manufacturer_filter").selected_text(if f.manufacturer.is_empty() { "Any manufacturer" } else { &f.manufacturer }).show_ui(ui, |ui| {
                ui.selectable_value(&mut f.manufacturer, String::new(), "Any manufacturer");
                for m in manufacturers {
                    ui.selectable_value(&mut f.manufacturer, m.to_string(), m);
                }
            });
            ui.add(egui::TextEdit::singleline(&mut f.package).hint_text("Package contains").desired_width(110.0));
            let unit = if currency.is_empty() { "as listed" } else { currency.as_str() };
            ui.add(egui::TextEdit::singleline(&mut f.max_price).hint_text(format!("Max price ({unit})")).desired_width(110.0));
            ui.add(egui::TextEdit::singleline(&mut f.text).hint_text("MPN, manufacturer, description").desired_width(190.0));
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("currency").selected_text(if currency.is_empty() { "As listed" } else { currency.as_str() }).show_ui(ui, |ui| {
                ui.selectable_value(currency, String::new(), "As listed");
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
        self.selected = None;
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
    let datasheet = if p.datasheet_url.is_empty() { "no datasheet link" } else { "datasheet available" };
    if known {
        format!("{datasheet}\nSymbol and footprint: provided by LCSC/EasyEDA\n3D model: usually provided (confirmed at import)\nSPICE model: not provided")
    } else {
        format!("{datasheet}\nSymbol, footprint, 3D and SPICE model: not known until imported (other sources are asked then)")
    }
}

/// One source's results, from the part store if fresh enough (each thread opens its own connection).
fn search_cached(src: &dyn klm_core::sources::Source, query: &str, ttl_secs: i64) -> anyhow::Result<Vec<Part>> {
    let store = Store::default_path().and_then(|p| Store::open(&p).ok());
    cache::search(store.as_ref(), src.name(), query, ttl_secs, || src.search(query))
}

/// Short name for the Source column.
fn source_tag(name: &str) -> String {
    match name {
        "DigiKey" => "DK".into(),
        "Mouser" => "MOU".into(),
        "Nexar" => "NEX".into(),
        n => n.chars().take(4).collect::<String>().to_uppercase(),
    }
}

/// 28410 -> "28,410".
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The price column's text and its tooltip (order size, pack, price breaks, why it was not converted).
fn price_cell(p: &Part, target: Option<&str>, rates: Option<&Rates>) -> (String, String) {
    let mut shown = display_price(&p.unit_price, target, rates);
    let cur = if p.price_currency.is_empty() { klm_core::currency::detect(&p.unit_price).unwrap_or_default() } else { p.price_currency.clone() };
    if shown == p.unit_price && !cur.is_empty() && !shown.contains(&cur) {
        shown = format!("{cur} {}", shown.trim_start_matches(['$', '€', '£']));
    }
    let mut tip = vec![];
    if let Some(m) = p.moq.filter(|m| *m > 1) {
        tip.push(format!("Minimum order {m}"));
    }
    if let Some(n) = p.pack_qty.filter(|n| *n > 1) {
        tip.push(format!("{} of {n}", if p.packaging.is_empty() { "Pack" } else { &p.packaging }));
    }
    if !p.price_breaks.is_empty() {
        tip.push(format!("Price breaks
{}", p.price_breaks.iter().map(|(q, v)| format!("{q}+: {v} {cur}")).collect::<Vec<_>>().join("
")));
    } else if target.is_some() && !p.unit_price.is_empty() && shown == p.unit_price {
        tip.push("Not converted: unknown currency or no exchange rates".into());
    }
    (shown, tip.join("
"))
}
