//! Pieces the BOM, History and Settings pages share, as the wireframes draw them: the teal primary button,
//! the mono caption, bordered cards and summary tiles.

use crate::theme;
use eframe::egui::{self, Color32, RichText, Ui};

/// The filled teal button ("Read BOM and prices", "Save", "Roll back to before this").
pub fn primary(ui: &mut Ui, enabled: bool, text: &str) -> egui::Response {
    let pal = theme::palette(ui.visuals());
    let on = if ui.visuals().dark_mode { Color32::from_rgb(0x07, 0x18, 0x1b) } else { Color32::WHITE };
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).strong().color(on)).fill(pal.accent).stroke(egui::Stroke::NONE))
}

/// The outlined destructive button ("Delete…").
pub fn danger(ui: &mut Ui, text: &str) -> egui::Response {
    let red = super::ui_kit::Level::Error.color(ui.visuals());
    ui.add(egui::Button::new(RichText::new(text).color(red)).stroke(egui::Stroke::new(1.0, red)))
}

/// A small mono uppercase caption: "BUY AS", "PRICE BREAKS · LCSC".
pub fn caption(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text.to_uppercase()).monospace().size(11.0).color(theme::palette(ui.visuals()).muted))
}

/// A bordered panel-coloured box with an optional bold title, filling the row.
pub fn card<R>(ui: &mut Ui, title: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let pal = theme::palette(ui.visuals());
    egui::Frame::new()
        .fill(pal.panel)
        .stroke(egui::Stroke::new(1.0, pal.line))
        .corner_radius(4)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(t) = title {
                ui.label(RichText::new(t).strong().size(14.0));
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

/// A summary tile: caption, big value, one muted line. `tint` colours the border, caption and value.
pub fn tile(ui: &mut Ui, label: &str, value: &str, sub: &str, tint: Option<Color32>) {
    let pal = theme::palette(ui.visuals());
    egui::Frame::new()
        .fill(pal.panel)
        .stroke(egui::Stroke::new(1.0, tint.unwrap_or(pal.line)))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(egui::Label::new(RichText::new(label.to_uppercase()).monospace().size(11.0).color(tint.unwrap_or(pal.muted))).truncate());
            ui.add(egui::Label::new(RichText::new(value).size(20.0).strong().color(tint.unwrap_or(pal.fg))).truncate());
            ui.add(egui::Label::new(RichText::new(if sub.is_empty() { " " } else { sub }).size(12.0).color(pal.muted)).truncate());
        });
}
