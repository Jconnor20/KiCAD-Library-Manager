//! Standards · Create, Symbol side: the form, a pasted pin table and the symbol it would give, placed the house way
//! (inputs left, outputs right, power top and bottom). Writing it into a library isn't in klm-core yet, so the
//! "Create in library" button says so instead of pretending.

use super::page_kit::{caption, primary};
use super::ui_kit::{self, Level};
use crate::{AppCtx, theme};
use eframe::egui::{self, Align2, FontId, Pos2, RichText, Stroke, Vec2};
use klm_core::symbols::Class;

const TYPES: [&str; 12] = ["input", "output", "bidirectional", "tri_state", "passive", "free", "unspecified", "power_in", "power_out", "open_collector", "open_emitter", "no_connect"];
const NOT_BACKED: &str = "Not available yet: KLM can restyle and edit symbols already in a library but doesn't build a new one from a pin table.";

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn label(self) -> &'static str {
        match self {
            Side::Left => "Left",
            Side::Right => "Right",
            Side::Top => "Top",
            Side::Bottom => "Bottom",
        }
    }
}

struct Pin {
    number: String,
    name: String,
    kind: String,
    side: Side,
}

pub struct SymbolForm {
    class: Class,
    library: String,
    name: String,
    value: String,
    footprint: String,
    datasheet: String,
    pins: String,
}

impl Default for SymbolForm {
    fn default() -> Self {
        Self { class: Class::Ic, library: "ICs".into(), name: String::new(), value: String::new(), footprint: String::new(), datasheet: String::new(), pins: String::new() }
    }
}

fn side_of(name: &str, kind: &str) -> Side {
    let n = name.to_uppercase();
    match kind {
        "output" | "power_out" | "open_collector" | "open_emitter" => Side::Right,
        "power_in" if n.contains("GND") || n.starts_with("VSS") || n.starts_with("VEE") => Side::Bottom,
        "power_in" => Side::Top,
        _ => Side::Left,
    }
}

/// One pin per `number,name,type` line; the lines that can't be read, as (line number, why).
fn parse(text: &str) -> (Vec<Pin>, Vec<(usize, String)>) {
    let (mut pins, mut bad) = (vec![], vec![]);
    for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let f: Vec<_> = line.split(',').map(str::trim).collect();
        match f[..] {
            [number, name, kind] if !number.is_empty() && !name.is_empty() && TYPES.contains(&kind) => pins.push(Pin { number: number.into(), name: name.into(), kind: kind.into(), side: side_of(name, kind) }),
            [_, _, kind] => bad.push((i + 1, format!("unknown pin type \"{kind}\""))),
            _ => bad.push((i + 1, "expected number, name, type".into())),
        }
    }
    (pins, bad)
}

impl SymbolForm {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let (pins, bad) = parse(&self.pins);
        let pal = theme::palette(ui.visuals());
        let wide = ui.available_width() >= 900.0;
        let side = egui::Frame::new().fill(pal.panel).stroke(Stroke::new(1.0, pal.line)).inner_margin(egui::Margin::same(12));
        egui::Panel::left("sym_form").exact_size(if wide { 340.0 } else { 300.0 }).resizable(false).frame(side).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                self.form(ui);
                if !wide {
                    ui.separator();
                    self.facts(ui, cx, &pins, &bad);
                }
            });
        });
        if wide {
            egui::Panel::right("sym_side").exact_size(280.0).resizable(false).frame(side).show(ui, |ui| self.facts(ui, cx, &pins, &bad));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 0))).show(ui, |ui| {
            ui.horizontal(|ui| {
                caption(ui, "Preview");
                ui.weak(format!("Uses the {} · {} style", cx.settings.profile_name(), super::classes::name(self.class)));
            });
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
            let p = ui.painter_at(rect);
            p.rect(rect, 4.0, pal.bg, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
            self.paint(&p, rect, &pins, pal);
        });
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        let field = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str| {
            ui.add_space(4.0);
            ui.label(RichText::new(label).weak());
            ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(f32::INFINITY));
        };
        ui.label(RichText::new("Class").weak());
        egui::ComboBox::from_id_salt("sym_class").selected_text(format!("{} · {}", super::classes::name(self.class), self.class.prefix())).width(ui.available_width()).show_ui(ui, |ui| {
            for c in Class::ALL {
                ui.selectable_value(&mut self.class, c, format!("{} · {}", super::classes::name(c), c.prefix()));
            }
        });
        field(ui, "Library", &mut self.library, "e.g. ICs");
        field(ui, "Name", &mut self.name, "the MPN, e.g. TPS62130");
        field(ui, "Value", &mut self.value, "defaults to the name");
        field(ui, "Footprint", &mut self.footprint, "Library:Footprint");
        field(ui, "Datasheet", &mut self.datasheet, "a link or a PDF path");
        ui.add_space(8.0);
        ui.label(RichText::new("Pins").weak());
        super::standards::seg(ui, 0, &[("Paste a pin table", true, ""), ("From footprint pads", false, NOT_BACKED), ("Blank", false, NOT_BACKED)]);
        ui.add_space(4.0);
        ui.add(egui::TextEdit::multiline(&mut self.pins).hint_text("1,PGND,power_in\n2,SW,power_out\n3,VIN,power_in\n4,EN,input").font(egui::TextStyle::Monospace).desired_rows(6).desired_width(f32::INFINITY));
        ui.weak("One pin per line: number, name, type. Pins are placed by the house standard: inputs left, outputs right, power top and bottom.");
    }

    fn facts(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx, pins: &[Pin], bad: &[(usize, String)]) {
        caption(ui, &format!("Pins · {} pasted", pins.len()));
        egui::Grid::new("sym_pins").num_columns(4).spacing([12.0, 3.0]).show(ui, |ui| {
            for h in ["No.", "Name", "Type", "Side"] {
                ui.weak(h);
            }
            ui.end_row();
            for p in pins.iter().take(24) {
                ui.label(RichText::new(&p.number).monospace());
                ui.label(RichText::new(&p.name).monospace());
                ui.weak(&p.kind);
                ui.label(p.side.label());
                ui.end_row();
            }
        });
        if pins.len() > 24 {
            ui.weak(format!("… and {} more", pins.len() - 24));
        }
        ui.add_space(8.0);
        caption(ui, "Checks");
        let taken = !self.name.trim().is_empty() && cx.global_root().is_some_and(|r| r.has_part(&self.library, self.name.trim()));
        let mut seen = std::collections::HashSet::new();
        let dup = pins.iter().find(|p| !seen.insert(p.number.as_str())).map(|p| p.number.clone());
        ui.horizontal_wrapped(|ui| {
            match (self.name.trim().is_empty(), taken) {
                (true, _) => drop(ui_kit::badge(ui, "Name it", Level::Warn)),
                (_, true) => drop(ui_kit::badge(ui, "Name taken", Level::Error)),
                _ => drop(ui_kit::badge(ui, "Name free", Level::Ok)),
            }
            match &dup {
                Some(n) => drop(ui_kit::badge(ui, &format!("Pin {n} twice"), Level::Error)),
                None if pins.is_empty() => drop(ui_kit::badge(ui, "No pins", Level::Warn)),
                None => drop(ui_kit::badge(ui, "Numbers unique", Level::Ok)),
            }
            for (line, why) in bad {
                ui_kit::badge(ui, &format!("Line {line}: {why}"), Level::Error);
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_enabled(false, egui::Button::new("Open in editor")).on_disabled_hover_text(NOT_BACKED);
            primary(ui, false, "Create in library").on_disabled_hover_text(NOT_BACKED);
        });
        ui.weak("Creating the symbol isn't built into KLM yet; this shows what the pin table would give.");
    }

    /// A body sized to its pins with each pin on its side, the reference above and the value below.
    fn paint(&self, p: &egui::Painter, rect: egui::Rect, pins: &[Pin], pal: &theme::Palette) {
        let on = |s| pins.iter().filter(|x| x.side == s).count().max(1) as f32;
        let pitch = 18.0f32.min((rect.height() - 120.0) / on(Side::Left).max(on(Side::Right)).max(4.0)).max(8.0);
        let body = Vec2::new((on(Side::Top).max(on(Side::Bottom)) * pitch + pitch * 4.0).max(pitch * 8.0), (on(Side::Left).max(on(Side::Right)) * pitch + pitch).max(pitch * 5.0));
        let b = egui::Rect::from_center_size(rect.center(), body);
        let line = Stroke::new(1.4, pal.fg);
        p.rect(b, 0.0, pal.row, line, egui::StrokeKind::Middle);
        let len = pitch * 1.6;
        for s in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            let mine: Vec<_> = pins.iter().filter(|x| x.side == s).collect();
            for (i, pin) in mine.iter().enumerate() {
                let off = (i as f32 + 0.5 - mine.len() as f32 / 2.0) * pitch;
                let (a, dir) = match s {
                    Side::Left => (Pos2::new(b.left(), b.center().y + off), Vec2::new(-len, 0.0)),
                    Side::Right => (Pos2::new(b.right(), b.center().y + off), Vec2::new(len, 0.0)),
                    Side::Top => (Pos2::new(b.center().x + off, b.top()), Vec2::new(0.0, -len)),
                    Side::Bottom => (Pos2::new(b.center().x + off, b.bottom()), Vec2::new(0.0, len)),
                };
                p.line_segment([a, a + dir], line);
                let (anchor, at) = match s {
                    Side::Left => (Align2::LEFT_CENTER, a + Vec2::new(4.0, 0.0)),
                    Side::Right => (Align2::RIGHT_CENTER, a - Vec2::new(4.0, 0.0)),
                    _ => continue,
                };
                p.text(at, anchor, &pin.name, FontId::monospace(10.0), pal.muted);
            }
        }
        let value = if self.value.trim().is_empty() { self.name.trim() } else { self.value.trim() };
        p.text(b.left_top() - Vec2::new(0.0, len + 6.0), Align2::LEFT_BOTTOM, format!("{}?", self.class.prefix()), FontId::proportional(12.0), pal.accent);
        p.text(b.left_bottom() + Vec2::new(0.0, len + 6.0), Align2::LEFT_TOP, if value.is_empty() { "Value" } else { value }, FontId::proportional(12.0), pal.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_parsed_and_placed_by_type() {
        let (pins, bad) = parse("1,PGND,power_in
2,SW,power_out

3,VIN,power_in
4,EN,input
5,X,weird
nope");
        let sides: Vec<_> = pins.iter().map(|p| p.side.label()).collect();
        assert_eq!(sides, ["Bottom", "Right", "Top", "Left"]);
        assert_eq!(bad.iter().map(|b| b.0).collect::<Vec<_>>(), [6, 7]);
    }
}
