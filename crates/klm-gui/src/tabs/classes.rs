//! Standards · Classes: per kind of part (resistor, capacitor… IC), how many parts match the house standard,
//! what the standard is, and what happens to new parts of that class on import.

use super::page_kit::{caption, card, primary};
use super::standards::{profile_bar, seg};
use super::template_editor::{Outcome, TemplateEditor};
use super::ui_kit::{self, Col, Level, Sort};
use crate::{AppCtx, Tab, theme};
use eframe::egui::{self, Align2, FontId, Pos2, RichText, Stroke, Vec2};
use klm_core::sexpr::{self, Sexpr};
use klm_core::symbols::{self, Class, Slot, Template};
use klm_core::{compliance, library::LibraryRoot};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::mpsc;

/// (parts, parts that differ from the standard) per class.
type Counts = HashMap<Class, (usize, usize)>;

#[derive(Default)]
pub struct ClassesTab {
    selected: Option<Class>,
    /// `None` until the first scan of the global library has finished.
    counts: Option<(Counts, usize)>,
    rx: Option<mpsc::Receiver<(Counts, usize)>>,
    /// What `counts` was worked out for: the library folder and the profile and rules in force.
    key: String,
    /// The graphical template editor, and the class it is editing.
    editor: Option<(Class, TemplateEditor)>,
}

pub fn name(c: Class) -> String {
    match c {
        Class::Led => "LED".into(),
        Class::Npn => "NPN".into(),
        Class::Pnp => "PNP".into(),
        Class::Ic => "IC".into(),
        c => format!("{c:?}"),
    }
}

/// The key of a class in `Profile::classes` and the built-in field schemas.
fn field_key(c: Class) -> &'static str {
    match c {
        Class::Resistor => "Resistor",
        Class::Capacitor => "Capacitor",
        Class::Inductor => "Inductor",
        Class::Diode => "Diode",
        Class::Led => "Led",
        Class::Npn | Class::Pnp => "Transistor",
        Class::Ic => "Ic",
    }
}

fn scan(root: LibraryRoot, cx: &AppCtx) -> mpsc::Receiver<(Counts, usize)> {
    let (tx, rx) = mpsc::channel();
    let (profile, naming, ctx) = (cx.settings.standard.clone(), cx.settings.rules.clone(), cx.ctx.clone());
    std::thread::spawn(move || {
        let comp = compliance::compute(&root, &profile, &naming);
        let mut counts = Counts::new();
        for (key, class) in symbols::classes_in(&root, &naming) {
            let e = counts.entry(class).or_default();
            e.0 += 1;
            e.1 += comp.get(&key).is_some_and(|c| !c.ok) as usize;
        }
        let _ = tx.send((counts, comp.len()));
        ctx.request_repaint();
    });
    rx
}

impl ClassesTab {
    fn refresh(&mut self, cx: &AppCtx) {
        let Some(root) = cx.global_root() else { return };
        self.key = super::standards::state_key(cx);
        self.rx = Some(scan(root, cx));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        if let Some(done) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.rx = None;
            self.selected.get_or_insert_with(|| Class::ALL.into_iter().find(|c| done.0.get(c).is_some_and(|n| n.1 > 0)).unwrap_or(Class::Resistor));
            self.counts = Some(done);
        }
        let key = super::standards::state_key(cx);
        if self.rx.is_none() && key != self.key {
            self.refresh(cx);
        }
        let loading = self.rx.is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Classes").heading().strong());
            match (&self.counts, cx.global_root()) {
                (_, None) => ui.weak("Set up a global library in Settings to see how its parts match."),
                (Some((m, n)), _) => ui.weak(format!(
                    "{n} parts · {} without a class (no house style fits them){}",
                    n - m.values().map(|c| c.0).sum::<usize>(),
                    if loading { " · checking…" } else { "" }
                )),
                (None, _) => ui.weak("checking…"),
            };
            if ui.add_enabled(!loading, egui::Button::new("Check again").frame(false)).clicked() {
                self.refresh(cx);
            }
            profile_bar(ui, cx);
        });
        ui.add_space(6.0);
        let selected = self.selected.unwrap_or(Class::Resistor);
        let counts = self.counts.as_ref().map(|c| &c.0);
        let mut pick = None;
        let pal = theme::palette(ui.visuals());
        egui::Panel::left("classes_list").exact_size(280.0).resizable(false).frame(egui::Frame::new().fill(pal.panel).stroke(Stroke::new(1.0, pal.line))).show(
            ui,
            |ui| {
                let cols =
                    [Col::new("Class", 100.0).no_sort(), Col::new("Ref", 40.0).no_sort(), Col::new("Parts", 50.0).no_sort(), Col::rest("Match").no_sort()];
                ui_kit::table(ui, "classes", &cols, &mut Sort { col: usize::MAX, asc: true }, Class::ALL.len(), |i, row| {
                    let c = Class::ALL[i];
                    row.set_selected(c == selected);
                    row.col(|ui| {
                        ui.label(name(c));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(c.prefix()).monospace());
                    });
                    let n = counts.map(|m| m.get(&c).copied().unwrap_or_default());
                    row.col(|ui| {
                        ui.label(n.map_or("–".to_string(), |n| n.0.to_string()));
                    });
                    row.col(|ui| match n {
                        Some((0, _)) | None => drop(ui.weak("")),
                        Some((_, 0)) => drop(ui_kit::badge(ui, "All", Level::Ok)),
                        Some((_, d)) => drop(ui_kit::badge(ui, &format!("{d} differ"), Level::Warn)),
                    });
                    if row.response().clicked() {
                        pick = Some(c);
                    }
                });
            },
        );
        if let Some(c) = pick {
            self.selected = Some(c);
        }
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 0))).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.detail(ui, cx, selected));
        });
        if let Some((class, ed)) = &mut self.editor {
            match ed.show(ui.ctx()) {
                Outcome::Save(t) => {
                    if let Ok(sym) = t.to_symbol(*class) {
                        cx.settings.standard.templates.insert(format!("{class:?}"), sexpr::to_string(&sym));
                        cx.save_settings();
                    }
                    self.editor = None;
                }
                Outcome::Close => self.editor = None,
                Outcome::None => {}
            }
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx, class: Class) {
        let (parts, differ) = self.counts.as_ref().and_then(|c| c.0.get(&class)).copied().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(format!("{} · {}", name(class), class.prefix())).size(16.0).strong());
                ui.weak(format!(
                    "{parts} parts · {}",
                    if differ == 0 { "all match this standard".to_string() } else { format!("{differ} don't match this standard") }
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The review dialog with Apply is on Library health; one place to change files from.
                if primary(ui, differ > 0, &format!("Review {differ} parts…")).on_disabled_hover_text("Every part of this class already matches").clicked() {
                    cx.tab = Tab::Health;
                }
            });
        });
        ui.add_space(8.0);
        // One row when there is room (as the wireframe), else the control drops under its label.
        let mut policy = |ui: &mut egui::Ui| {
            ui.label(RichText::new("When new parts come in").strong());
            let on = if cx.settings.house_symbols_on_import { 0 } else { 2 };
            let hit = seg(
                ui,
                on,
                &[
                    ("Change to match", true, ""),
                    ("Ask me", false, "Not available yet: an import either redraws the symbol or keeps the vendor's."),
                    ("Leave as imported", true, ""),
                ],
            );
            if let Some(i) = hit.filter(|i| *i != 1) {
                cx.settings.house_symbols_on_import = i == 0;
                cx.save_settings();
                symbols::configure(&cx.settings);
            }
        };
        if ui.available_width() > 900.0 {
            ui.horizontal(|ui| {
                policy(ui);
                ui.weak("Applies to every class, during Import, before parts reach the library.");
            });
        } else {
            ui.vertical(|ui| {
                policy(ui);
                ui.weak("Applies to every class, during Import, before parts reach the library.");
            });
        }
        ui.add_space(8.0);
        let key = format!("{class:?}");
        let mut open = None;
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            card(ui, None, |ui| {
                ui.horizontal(|ui| {
                    caption(ui, "Symbol template");
                });
                let house = symbols::house_template(&cx.settings.standard, class).ok().flatten();
                let t = house.clone().or_else(|| symbols::template_of(class).cloned()).unwrap_or_else(ic_box);
                preview(ui, &t, Vec2::new(ui.available_width(), 130.0), cx.settings.standard.rules.symbol_field_positions, &format!("{}?", class.prefix()));
                if symbols::template_of(class).is_some()
                    && ui
                        .button("Edit symbol…")
                        .on_hover_text("Draw the symbol: move its pins and shapes on the grid, place Reference and Value, set their text size")
                        .clicked()
                {
                    open = Some(t.clone());
                }
                ui.weak(if house.is_some() {
                    "Your house template, from a part you drew or set below."
                } else {
                    "Built-in template: pins on the 2.54 mm grid, KLC style."
                });
                if cx.settings.standard.templates.contains_key(&key) || cx.settings.standard.footprint_styles.contains_key(&key) {
                    if ui.small_button("Use built-in").on_hover_text("Forget this house symbol and footprint style").clicked() {
                        cx.settings.standard.templates.remove(&key);
                        cx.settings.standard.footprint_styles.remove(&key);
                        cx.save_settings();
                    }
                } else {
                    ui.weak("To set your own: click Edit symbol…, or right-click a part in the Library and choose \"Use as house template\".");
                }
            });
            let ui = &mut cols[1];
            card(ui, None, |ui| {
                caption(ui, "Footprint style");
                let r = &mut cx.settings.standard.rules;
                let mut changed = false;
                changed |= ui.checkbox(&mut r.footprint_ref_value, "Reference above the courtyard, Value below").changed();
                changed |= ui.checkbox(&mut r.pin1_marker, "Add a pin-1 marker where there is none").changed();
                changed |= ui.checkbox(&mut r.footprint_origin, "Origin at the pad centre (SMD) or pin 1 (through-hole)").changed();
                let font = cx.settings.standard.footprint_styles.get(&key).and_then(|s| s.texts.get("Reference")).and_then(|t| t.font.clone());
                ui.weak(match font {
                    Some(f) => format!("Text from your house footprint: {f}"),
                    None => "Text size and thickness follow KLC unless a house footprint sets them.".into(),
                });
                ui.weak("These rules apply to every class; Create uses them too.");
                if changed {
                    cx.save_settings();
                }
            });
        });
        ui.add_space(8.0);
        card(ui, None, |ui| {
            ui.horizontal(|ui| {
                caption(ui, "Fields and naming");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Edit…").clicked() {
                        cx.open_settings(ui_kit::Section::PartInfo);
                    }
                });
            });
            let info = cx.settings.standard.class(field_key(class));
            ui.horizontal_wrapped(|ui| {
                for (label, required) in [("Required:", true), ("Optional:", false)] {
                    ui.weak(label);
                    for f in info.fields.iter().filter(|f| f.required == required) {
                        ui_kit::badge(ui, &f.name, if required { Level::Ok } else { Level::Info });
                    }
                }
            });
            let r = &cx.settings.rules;
            ui.horizontal_wrapped(|ui| {
                ui.weak("Symbol name");
                ui.label(RichText::new(if r.name_template.trim().is_empty() { "{MPN}" } else { r.name_template.as_str() }).monospace());
                ui.weak(format!("· MPN case {:?}", r.mpn_case));
            });
        });
        if let Some(t) = open {
            self.editor = Some((class, TemplateEditor::new(class, t, cx.settings.standard.rules.symbol_text_size.unwrap_or(1.27))));
        }
        ui.weak("A part's class comes from its category and keywords. Change the matching in Settings → Import.");
    }
}

/// A box with pins left and right, as the IC template draws it (the real one is sized to the part's pins).
fn ic_box() -> Template {
    let pins = |x: f64, rot: f64| (0..5).map(move |i| Slot(x, 5.08 - 2.54 * i as f64, rot, 2.54));
    Template {
        body: Cow::Borrowed("(rectangle (start -5.08 -7.62) (end 5.08 7.62))"),
        slots: Cow::Owned(pins(-7.62, 0.0).chain(pins(7.62, 180.0)).collect()),
        roles: &[],
        hide_numbers: false,
        fields: None,
        field_sizes: None,
    }
}

enum Shape {
    Line(Vec<(f64, f64)>),
    Rect((f64, f64), (f64, f64)),
    Circle((f64, f64), f64),
}

fn shapes(body: &str) -> Vec<Shape> {
    let Ok(doc) = sexpr::parse(&format!("(b {body})")) else { return vec![] };
    let num = |x: &Sexpr, i| x.arg(i).and_then(|v| v.parse::<f64>().ok());
    let xy = |x: &Sexpr| Some((num(x, 0)?, num(x, 1)?));
    doc.items()
        .iter()
        .filter_map(|g| {
            Some(match g.head()? {
                "polyline" => Shape::Line(g.child("pts")?.children("xy").filter_map(xy).collect()),
                "arc" => Shape::Line(["start", "mid", "end"].iter().filter_map(|k| xy(g.child(k)?)).collect()),
                "rectangle" => Shape::Rect(xy(g.child("start")?)?, xy(g.child("end")?)?),
                "circle" => Shape::Circle(xy(g.child("center")?)?, num(g.child("radius")?, 0)?),
                _ => return None,
            })
        })
        .collect()
}

/// Draws a template: body graphics, pins as lines, Reference above and Value below.
fn preview(ui: &mut egui::Ui, t: &Template, size: Vec2, fields_above_below: bool, reference: &str) {
    let pal = theme::palette(ui.visuals());
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect(rect, 4.0, pal.bg, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
    let sh = shapes(&t.body);
    let ends = t.slots.iter().flat_map(|s| {
        let a = s.2.to_radians();
        [(s.0, s.1), (s.0 + s.3 * a.cos(), s.1 + s.3 * a.sin())]
    });
    let pts: Vec<(f64, f64)> = sh
        .iter()
        .flat_map(|s| match s {
            Shape::Line(v) => v.clone(),
            Shape::Rect(a, b) => vec![*a, *b],
            Shape::Circle(c, r) => vec![(c.0 - r, c.1 - r), (c.0 + r, c.1 + r)],
        })
        .chain(ends)
        .collect();
    let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (y0, y1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    if pts.is_empty() {
        return;
    }
    let s = (((rect.width() - 24.0) as f64 / (x1 - x0).max(1.0)).min(((rect.height() - 44.0) as f64 / (y1 - y0).max(1.0)) * 1.0)).min(14.0) as f32;
    let at = |(x, y): (f64, f64)| Pos2::new(rect.center().x + (x - (x0 + x1) / 2.0) as f32 * s, rect.center().y - (y - (y0 + y1) / 2.0) as f32 * s);
    let line = Stroke::new(1.4, pal.fg);
    for shape in &sh {
        match shape {
            Shape::Line(v) => {
                p.add(egui::Shape::line(v.iter().map(|q| at(*q)).collect(), line));
            }
            Shape::Rect(a, b) => {
                p.rect(egui::Rect::from_two_pos(at(*a), at(*b)), 0.0, pal.row, line, egui::StrokeKind::Middle);
            }
            Shape::Circle(c, r) => {
                p.circle_stroke(at(*c), *r as f32 * s, line);
            }
        }
    }
    for sl in t.slots.iter() {
        let a = sl.2.to_radians();
        p.line_segment([at((sl.0, sl.1)), at((sl.0 + sl.3 * a.cos(), sl.1 + sl.3 * a.sin()))], line);
    }
    let font = FontId::proportional(11.0);
    let (top, bottom) = (at(((x0 + x1) / 2.0, y1)), at(((x0 + x1) / 2.0, y0)));
    if let Some([r, v]) = t.fields {
        let align = |f: (f64, f64, f64)| if f.0 > 0.0 { Align2::LEFT_CENTER } else { Align2::CENTER_CENTER };
        p.text(at((r.0, r.1)), align(r), reference, font.clone(), pal.accent);
        p.text(at((v.0, v.1)), align(v), "Value", font, pal.accent);
    } else if fields_above_below {
        p.text(top - Vec2::new(0.0, 6.0), Align2::CENTER_BOTTOM, reference, font.clone(), pal.accent);
        p.text(bottom + Vec2::new(0.0, 6.0), Align2::CENTER_TOP, "Value", font, pal.accent);
    }
}
