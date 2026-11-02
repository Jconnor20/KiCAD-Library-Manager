use eframe::egui::{self, RichText};
use klm_core::part::Part;

pub enum SearchAction {
    None,
    Search(String),
    /// Fetch result `i` (an index into the results).
    Get(usize),
}

/// One search box over every enabled source; results from all of them in one table.
#[derive(Default)]
pub struct SearchView {
    query: String,
    pub searching: bool,
    /// (index into the app's source list, result)
    pub results: Vec<(usize, Part)>,
}

impl SearchView {
    pub fn show(&mut self, ui: &mut egui::Ui, source_names: &[&str], busy: bool) -> SearchAction {
        let mut action = SearchAction::None;
        if source_names.is_empty() {
            ui.label("No search sources are enabled yet.");
            return action;
        }
        ui.horizontal(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("MPN, LCSC/DigiKey number or keywords").desired_width(320.0));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let can = !self.searching && !self.query.trim().is_empty();
            if (ui.add_enabled(can, egui::Button::new("Search")).clicked() || (enter && can)) && can {
                action = SearchAction::Search(self.query.trim().to_string());
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
                        action = SearchAction::Get(i);
                    }
                    ui.end_row();
                }
            });
        });
        action
    }
}
