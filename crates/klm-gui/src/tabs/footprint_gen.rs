//! Standards · Create: an IPC-7351B footprint from datasheet dimensions (see `klm_core::footprint_gen`), written into a
//! library, with the pads drawn as you type; the Symbol side is in `create_symbol`.

use super::create_symbol::SymbolForm;
use super::page_kit::{caption, primary};
use super::standards::{profile_bar, seg};
use crate::tabs::ui_kit::Level;
use crate::{AppCtx, theme};
use eframe::egui::{self, Color32, Pos2, RichText, Stroke, Vec2};
use klm_core::footprint_gen::{self, Dims, Family, Generated, Range};
use klm_core::sexpr::{self, Sexpr};

pub struct FootprintTab {
    family: Family,
    category: String,
    dims: Dims,
    /// Body size and exposed pad as typed; zero means "derive it" / "none".
    body: (f64, f64),
    thermal: (f64, f64),
    density: char,
    project: bool,
    overwrite: bool,
    result: Option<Result<Generated, String>>,
    /// Footprint or Symbol side of the page.
    symbol: bool,
    sym: SymbolForm,
    /// Which layers the preview draws: pads, courtyard, silk, fab.
    layers: [bool; 4],
}

impl Default for FootprintTab {
    fn default() -> Self {
        Self {
            family: Family::Soic,
            category: String::new(),
            dims: Dims { pins: 8, pitch: 1.27, span: Range::new(5.8, 6.2), lead_len: Range::new(0.4, 0.7), lead_w: Range::new(0.31, 0.51), height: 1.75, rows: 4, cols: 4, ball: 0.5, ..Default::default() },
            body: (3.9, 4.9),
            thermal: (0.0, 0.0),
            density: 'B',
            project: false,
            overwrite: false,
            result: None,
            symbol: false,
            sym: SymbolForm::default(),
            layers: [true; 4],
        }
    }
}

fn num(ui: &mut egui::Ui, label: &str, v: &mut f64) {
    ui.label(label);
    ui.add(egui::DragValue::new(v).speed(0.01).range(0.0..=100.0).max_decimals(3));
}

fn range(ui: &mut egui::Ui, label: &str, r: &mut Range) {
    ui.label(label);
    ui.add(egui::DragValue::new(&mut r.min).speed(0.01).range(0.0..=100.0).max_decimals(3));
    ui.add(egui::DragValue::new(&mut r.max).speed(0.01).range(0.0..=100.0).max_decimals(3));
    ui.end_row();
}

impl FootprintTab {
    fn build(&self, cx: &AppCtx) -> Result<Generated, String> {
        let mut d = self.dims.clone();
        d.body = (self.body.0 > 0.0 && self.body.1 > 0.0).then_some(self.body);
        d.thermal = (self.thermal.0 > 0.0 && self.thermal.1 > 0.0).then_some(self.thermal);
        let choice = klm_core::fab_data::FabChoice { density: self.density, ..cx.settings.standard.fab.clone() };
        footprint_gen::generate(self.family, &d, &choice, &cx.settings.standard.rules, None).map_err(|e| format!("{e:#}"))
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Create").heading().strong());
            if let Some(i) = seg(ui, self.symbol as usize, &[("Footprint", true, ""), ("Symbol", true, "")]) {
                self.symbol = i == 1;
            }
            profile_bar(ui, cx);
        });
        ui.add_space(6.0);
        if self.symbol {
            return self.sym.ui(ui, cx);
        }
        let pal = theme::palette(ui.visuals());
        let wide = ui.available_width() >= 900.0;
        let side = egui::Frame::new().fill(pal.panel).stroke(Stroke::new(1.0, pal.line)).inner_margin(egui::Margin::same(12));
        egui::Panel::left("fp_form").exact_size(if wide { 330.0 } else { 300.0 }).resizable(false).frame(side).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                self.form(ui, cx);
                if !wide {
                    ui.separator();
                    self.facts(ui, cx);
                }
            });
        });
        if wide {
            egui::Panel::right("fp_side").exact_size(260.0).resizable(false).frame(side).show(ui, |ui| self.facts(ui, cx));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 0))).show(ui, |ui| self.preview(ui));
    }

    fn form(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let f = self.family;
        caption(ui, "Family");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for x in Family::ALL {
                ui.selectable_value(&mut self.family, x, x.label().to_uppercase());
            }
        });
        ui.add_space(6.0);
        caption(ui, "Density (IPC-7351B)");
        let items = [("Most", true, ""), ("Nominal", true, ""), ("Least", true, "")];
        if let Some(i) = seg(ui, (self.density as u8 - b'A') as usize, &items) {
            self.density = (b'A' + i as u8) as char;
        }
        ui.add_space(6.0);
        egui::Grid::new("fpgen").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
            if f == Family::Bga {
                ui.label("Rows");
                ui.add(egui::DragValue::new(&mut self.dims.rows).range(1..=99));
                ui.end_row();
                ui.label("Columns");
                ui.add(egui::DragValue::new(&mut self.dims.cols).range(1..=99));
                ui.end_row();
                num(ui, "Pitch", &mut self.dims.pitch);
                ui.end_row();
                num(ui, "Ball Ø", &mut self.dims.ball);
                ui.end_row();
            } else {
                if !matches!(f, Family::Chip | Family::Sod) {
                    ui.label("Pins");
                    ui.add(egui::DragValue::new(&mut self.dims.pins).range(2..=400));
                    ui.end_row();
                    num(ui, "Pitch", &mut self.dims.pitch);
                    ui.end_row();
                }
                ui.weak("");
                ui.weak("min");
                ui.weak("max");
                ui.end_row();
                range(ui, "Lead span", &mut self.dims.span);
                if matches!(f, Family::Qfn | Family::Qfp) {
                    let mut y = self.dims.span_y.unwrap_or(self.dims.span);
                    range(ui, "Span Y", &mut y);
                    self.dims.span_y = Some(y);
                }
                range(ui, "Lead length", &mut self.dims.lead_len);
                range(ui, "Lead width", &mut self.dims.lead_w);
            }
            num(ui, "Height", &mut self.dims.height);
            ui.end_row();
            num(ui, "Body X", &mut self.body.0);
            ui.end_row();
            num(ui, "Body Y", &mut self.body.1);
            ui.end_row();
            if f == Family::Qfn {
                num(ui, "Exposed pad X", &mut self.thermal.0);
                ui.end_row();
                num(ui, "Exposed pad Y", &mut self.thermal.1);
                ui.end_row();
            }
        });
        ui.weak("Datasheet dimensions in mm. Body size is needed for all but chip and QFN; exposed pad 0 = none. Lines, text and courtyard follow the active profile.");
        ui.add_space(6.0);
        caption(ui, "Category");
        ui.add(egui::TextEdit::singleline(&mut self.category).hint_text("e.g. ICs").desired_width(f32::INFINITY));
        ui.add_enabled(cx.project_root().is_some(), egui::Checkbox::new(&mut self.project, "Project library"));
        ui.checkbox(&mut self.overwrite, "Replace if it exists");
    }

    /// "Will save as", the profile's changes, the pad sizes and the Generate / Save buttons.
    fn facts(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let root = if self.project { cx.project_root() } else { cx.global_root() };
        caption(ui, "Will save as");
        match (&self.result, &root) {
            (Some(Ok(g)), Some(r)) if !self.category.trim().is_empty() => ui.label(RichText::new(format!("{}.pretty/{}.kicad_mod", r.nickname(self.category.trim()), g.name)).monospace()),
            _ => ui.weak("Generate, name a category and set up a library to save."),
        };
        ui.add_space(8.0);
        caption(ui, "Checks");
        match &self.result {
            Some(Ok(g)) => {
                ui.label(RichText::new(&g.name).monospace());
                if g.fixes.is_empty() {
                    ui.weak("Already in the profile's style.");
                }
                for fix in &g.fixes {
                    ui.weak(format!("Profile: {fix}"));
                }
            }
            Some(Err(e)) => {
                ui.colored_label(Level::Error.color(ui.visuals()), e);
            }
            None => {
                ui.weak("Nothing generated yet.");
            }
        }
        if let Some(Ok(g)) = &self.result {
            ui.add_space(8.0);
            caption(ui, "Pads");
            let sizes = pad_sizes(&g.text);
            egui::Grid::new("fp_pads").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                for (k, v) in sizes {
                    ui.weak(k);
                    ui.label(RichText::new(v).monospace());
                    ui.end_row();
                }
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if primary(ui, true, "Generate").clicked() {
                self.result = Some(self.build(cx));
            }
            let ready = matches!(self.result, Some(Ok(_))) && root.is_some() && !self.category.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Save to library")).clicked() {
                if let (Some(Ok(g)), Some(root)) = (&self.result, &root) {
                    match footprint_gen::save(root, self.category.trim(), g, self.overwrite) {
                        Ok(p) => cx.log(Level::Info, format!("Wrote {}", p.display())),
                        Err(e) => cx.log(Level::Error, format!("Could not write the footprint: {e:#}")),
                    }
                }
            }
        });
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match &self.result {
                Some(Ok(g)) => ui.label(RichText::new(&g.name).monospace().strong()),
                _ => ui.weak("Preview"),
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (i, name) in ["Fab", "Silk", "Courtyard", "Pads"].into_iter().enumerate() {
                    ui.toggle_value(&mut self.layers[3 - i], name);
                }
            });
        });
        let pal = theme::palette(ui.visuals());
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect(rect, 4.0, pal.bg, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
        match &self.result {
            Some(Ok(g)) => paint(&p, rect.shrink(16.0), &g.text, &self.layers, pal),
            Some(Err(_)) => {}
            None => {
                p.text(rect.center(), egui::Align2::CENTER_CENTER, "Press Generate to draw the footprint", egui::FontId::proportional(13.0), pal.muted);
            }
        }
    }
}

fn nums(s: &Sexpr, head: &str) -> Option<(f64, f64)> {
    let c = s.child(head)?;
    Some((c.arg(0)?.parse().ok()?, c.arg(1)?.parse().ok()?))
}

/// "Signal 0.25 × 0.80 mm" lines for the Pads panel: pad 1, the largest other pad (exposed), and the span of all pads.
fn pad_sizes(text: &str) -> Vec<(&'static str, String)> {
    let Ok(fp) = sexpr::parse(text) else { return vec![] };
    let pads: Vec<_> = fp.children("pad").filter_map(|p| Some((p.arg(0)?.to_string(), nums(p, "at")?, nums(p, "size")?))).collect();
    let mut out = vec![];
    if let Some(p) = pads.first() {
        out.push(("Signal", format!("{:.2} × {:.2} mm", p.2.0, p.2.1)));
    }
    if let Some(p) = pads.iter().max_by(|a, b| (a.2.0 * a.2.1).total_cmp(&(b.2.0 * b.2.1))).filter(|p| pads.len() > 1 && p.2.0 * p.2.1 > 2.0 * pads[0].2.0 * pads[0].2.1) {
        out.push(("Exposed", format!("{:.2} × {:.2} mm", p.2.0, p.2.1)));
    }
    let span = |f: fn(&(String, (f64, f64), (f64, f64))) -> (f64, f64)| pads.iter().map(f).fold((f64::MAX, f64::MIN), |a, v| (a.0.min(v.0), a.1.max(v.1)));
    if !pads.is_empty() {
        let x = span(|p| (p.1.0 - p.2.0 / 2.0, p.1.0 + p.2.0 / 2.0));
        out.push(("Span", format!("{:.2} mm", x.1 - x.0)));
    }
    out
}

/// Draws the footprint's pads, courtyard, silk and fab lines scaled into `area`; KiCad's y runs down, as on screen.
fn paint(p: &egui::Painter, area: egui::Rect, text: &str, layers: &[bool; 4], pal: &theme::Palette) {
    let Ok(fp) = sexpr::parse(text) else { return };
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    let mut grow = |x: f64, y: f64| {
        (lo, hi) = (lo.min(Vec2::new(x as f32, y as f32)), hi.max(Vec2::new(x as f32, y as f32)));
    };
    for pad in fp.children("pad") {
        if let (Some((x, y)), Some((w, h))) = (nums(pad, "at"), nums(pad, "size")) {
            grow(x - w / 2.0, y - h / 2.0);
            grow(x + w / 2.0, y + h / 2.0);
        }
    }
    for g in fp.items().iter().filter(|g| g.child("layer").and_then(|l| l.arg(0)).is_some()) {
        for k in ["start", "end", "center"] {
            if let Some((x, y)) = nums(g, k) {
                grow(x, y);
            }
        }
    }
    if lo.x > hi.x {
        return;
    }
    let size = (hi - lo).max(Vec2::splat(0.1));
    let s = (area.width() / size.x).min((area.height() - 24.0) / size.y);
    let mid = (lo + hi) / 2.0;
    let at = |(x, y): (f64, f64)| Pos2::new(area.center().x + (x as f32 - mid.x) * s, area.center().y - 8.0 + (y as f32 - mid.y) * s);
    let stroke = |layer: &str| match layer {
        "F.CrtYd" if layers[1] => Some(Stroke::new(1.0, pal.accent)),
        "F.SilkS" if layers[2] => Some(Stroke::new(1.2, pal.fg)),
        "F.Fab" if layers[3] => Some(Stroke::new(1.0, pal.muted)),
        _ => None,
    };
    for g in fp.items() {
        let Some(st) = g.child("layer").and_then(|l| l.arg(0)).and_then(stroke) else { continue };
        let _ = match (g.head(), nums(g, "start"), nums(g, "end"), nums(g, "center")) {
            (Some("fp_line"), Some(a), Some(b), _) => p.line_segment([at(a), at(b)], st),
            (Some("fp_rect"), Some(a), Some(b), _) => p.rect_stroke(egui::Rect::from_two_pos(at(a), at(b)), 0.0, st, egui::StrokeKind::Middle),
            (Some("fp_circle"), _, Some(e), Some(c)) => p.circle_stroke(at(c), ((e.0 - c.0).hypot(e.1 - c.1) as f32 * s).max(1.5), st),
            _ => continue,
        };
    }
    if layers[0] {
        let gold = Color32::from_rgb(0xc9, 0xa2, 0x4a);
        for pad in fp.children("pad") {
            let (Some(c), Some((w, h))) = (nums(pad, "at"), nums(pad, "size")) else { continue };
            if pad.arg(2) == Some("circle") {
                p.circle_filled(at(c), (w as f32 * s / 2.0).max(1.0), gold);
            } else {
                p.rect_filled(egui::Rect::from_center_size(at(c), Vec2::new(w as f32 * s, h as f32 * s)), 1.0, gold);
            }
        }
    }
    let courtyard = (hi.x - lo.x) as f64;
    p.text(Pos2::new(area.center().x, area.bottom() - 4.0), egui::Align2::CENTER_BOTTOM, format!("{:.2} × {:.2} mm", courtyard, hi.y - lo.y), egui::FontId::monospace(11.0), pal.muted);
}
