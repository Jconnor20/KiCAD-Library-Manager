//! A graphical editor for a house symbol template (Standards · Classes): the body is drawn on a grid and its
//! points, the pins and the Reference/Value text are dragged where they should be, as in KiCad's symbol editor.
//! It works on a copy and hands back a `Template`; the caller stores it in the profile.

use super::batch_dialog;
use super::ui_kit;
use crate::theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use klm_core::symbols::shapes::{Geo, Pt, Shape, arc_points};
use klm_core::symbols::{self, Class, Slot, Template};
use std::borrow::Cow;

/// KiCad's schematic grids: (mm, label).
const GRIDS: [(f64, &str); 4] = [(2.54, "100 mil (2.54 mm)"), (1.27, "50 mil (1.27 mm)"), (0.635, "25 mil (0.635 mm)"), (0.254, "10 mil (0.254 mm)")];
const DIRECTIONS: [(f64, &str); 4] = [(0.0, "Pin on the left"), (90.0, "Pin below"), (180.0, "Pin on the right"), (270.0, "Pin above")];

pub enum Outcome {
    None,
    Close,
    Save(Template),
}

/// Everything the editor changes; one of these is what Undo goes back to.
#[derive(Clone)]
struct Doc {
    shapes: Vec<Shape>,
    slots: Vec<Slot>,
    fields: Option<[(f64, f64, f64); 2]>,
    sizes: Option<[f64; 2]>,
    hide_numbers: bool,
}

impl Doc {
    fn of(t: &Template) -> Doc {
        Doc { shapes: t.shapes(), slots: t.slots.to_vec(), fields: t.fields, sizes: t.field_sizes, hide_numbers: t.hide_numbers }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Sel {
    None,
    Shape(usize),
    /// A shape's point.
    Point(usize, usize),
    /// A circle's radius handle.
    Radius(usize),
    /// A pin's connection point (where wires meet).
    Pin(usize),
    /// A pin's end at the body, which sets its length.
    PinEnd(usize),
    Field(usize),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tool {
    Select,
    Line,
    Rect,
    Circle,
    Arc,
}

impl Tool {
    fn clicks(self) -> usize {
        match self {
            Tool::Select => 0,
            Tool::Line => usize::MAX,
            Tool::Rect | Tool::Circle => 2,
            Tool::Arc => 3,
        }
    }
}

pub struct TemplateEditor {
    class: Class,
    base: Template,
    doc: Doc,
    undo: Vec<Doc>,
    sel: Sel,
    tool: Tool,
    /// Points placed so far by the drawing tool.
    draft: Vec<Pt>,
    drag: Option<(Sel, Pt)>,
    /// Where the pointer is, not snapped (a pin's length snaps on its own, finer than the grid).
    raw: Pt,
    grid: f64,
    /// Pixels per mm, and the screen position of the symbol origin relative to the canvas centre.
    scale: f32,
    pan: Vec2,
    fitted: bool,
    /// Text height when the standard's rules decide it.
    default_size: f64,
    changed: bool,
}

impl TemplateEditor {
    /// `template` is the class's current house or built-in template; `default_size` the text height the rules give.
    pub fn new(class: Class, template: Template, default_size: f64) -> Self {
        Self {
            class,
            doc: Doc::of(&template),
            base: template,
            undo: vec![],
            sel: Sel::None,
            tool: Tool::Select,
            draft: vec![],
            drag: None,
            raw: (0.0, 0.0),
            grid: 2.54,
            scale: 16.0,
            pan: Vec2::ZERO,
            fitted: false,
            default_size,
            changed: false,
        }
    }

    fn template(&self) -> Template {
        let mut t = self.base.clone();
        t.set_shapes(&self.doc.shapes);
        t.slots = Cow::Owned(self.doc.slots.clone());
        t.fields = self.doc.fields;
        t.field_sizes = self.doc.sizes;
        t.hide_numbers = self.doc.hide_numbers;
        t
    }

    fn push_undo(&mut self) {
        self.undo.push(self.doc.clone());
        self.changed = true;
    }

    fn snap(&self, v: f64) -> f64 {
        (v / self.grid).round() * self.grid
    }

    fn sizes(&self) -> [f64; 2] {
        self.doc.sizes.unwrap_or([self.default_size; 2])
    }

    /// (left, bottom, right, top) of the drawing and the pins.
    fn bounds(&self) -> (f64, f64, f64, f64) {
        let mut pts: Vec<Pt> = vec![];
        for s in &self.doc.shapes {
            match &s.geo {
                Geo::Circle(c, r) => pts.extend([(c.0 - r, c.1 - r), (c.0 + r, c.1 + r)]),
                Geo::Other(_) => {}
                _ => pts.extend(s.points()),
            }
        }
        for s in &self.doc.slots {
            let a = s.2.to_radians();
            pts.extend([(s.0, s.1), (s.0 + s.3 * a.cos(), s.1 + s.3 * a.sin())]);
        }
        if pts.is_empty() {
            return (-2.54, -2.54, 2.54, 2.54);
        }
        let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
        let (y0, y1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
        (x0, y0, x1, y1)
    }

    /// Reference and Value where the generic rule puts them: above and below the drawing.
    fn default_fields(&self) -> [(f64, f64, f64); 2] {
        let (x0, y0, x1, y1) = self.bounds();
        let x = (x0 + x1) / 2.0;
        [(x, y1 + 1.27, 0.0), (x, y0 - 1.27, 0.0)]
    }

    fn fields(&self) -> [(f64, f64, f64); 2] {
        self.doc.fields.unwrap_or_else(|| self.default_fields())
    }

    /// Fixes the text positions so that editing one of them leaves the other where it is drawn.
    fn own_fields(&mut self) -> &mut [(f64, f64, f64); 2] {
        let d = self.fields();
        self.doc.fields.get_or_insert(d)
    }

    fn fit(&mut self, size: Vec2) {
        let (x0, y0, x1, y1) = self.bounds();
        let f = self.fields();
        let (y0, y1) = (y0.min(f[1].1 - 1.0), y1.max(f[0].1 + 1.0));
        let (w, h) = ((x1 - x0 + 10.0).max(8.0), (y1 - y0 + 4.0).max(8.0));
        self.scale = ((size.x as f64 / w).min(size.y as f64 / h)).clamp(5.0, 40.0) as f32;
        self.pan = Vec2::new(-((x0 + x1) / 2.0) as f32 * self.scale, ((y0 + y1) / 2.0) as f32 * self.scale);
        self.fitted = true;
    }

    fn to_screen(&self, rect: Rect, p: Pt) -> Pos2 {
        rect.center() + self.pan + Vec2::new(p.0 as f32 * self.scale, -(p.1 as f32) * self.scale)
    }

    fn to_sym(&self, rect: Rect, p: Pos2) -> Pt {
        let d = p - rect.center() - self.pan;
        ((d.x / self.scale) as f64, -(d.y / self.scale) as f64)
    }

    fn snapped(&self, rect: Rect, p: Pos2) -> Pt {
        let q = self.to_sym(rect, p);
        (self.snap(q.0), self.snap(q.1))
    }

    /// The screen box of a text, for clicking it.
    fn text_rect(&self, rect: Rect, i: usize) -> Rect {
        let f = self.fields()[i];
        let size = self.sizes()[i] as f32 * self.scale * 0.8;
        let chars = if i == 0 { format!("{}?", self.class.prefix()).len() } else { 5 } as f32;
        let (w, h) = (chars * size * 0.6, size);
        let (w, h) = if f.2 == 90.0 || f.2 == 270.0 { (h, w) } else { (w, h) };
        let at = self.to_screen(rect, (f.0, f.1));
        if f.0 > 0.0 && f.2 == 0.0 { Rect::from_min_size(at - Vec2::new(0.0, h / 2.0), Vec2::new(w, h)) } else { Rect::from_center_size(at, Vec2::new(w, h)) }
    }

    /// What the pointer is over, nearest handle first, then text, pins and shapes.
    fn hit(&self, rect: Rect, pos: Pos2) -> Sel {
        let near = |p: Pt| self.to_screen(rect, p).distance(pos);
        let mut best: Option<(f32, Sel)> = None;
        let mut offer = |d: f32, s: Sel| {
            if d < 9.0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, s));
            }
        };
        for (i, sh) in self.doc.shapes.iter().enumerate() {
            for (j, p) in sh.points().into_iter().enumerate() {
                offer(near(p), Sel::Point(i, j));
            }
            if let Geo::Circle(c, r) = sh.geo {
                offer(near((c.0 + r, c.1)), Sel::Radius(i));
            }
        }
        for (i, s) in self.doc.slots.iter().enumerate() {
            let a = s.2.to_radians();
            offer(near((s.0, s.1)), Sel::Pin(i));
            offer(near((s.0 + s.3 * a.cos(), s.1 + s.3 * a.sin())), Sel::PinEnd(i));
        }
        if let Some((_, s)) = best {
            return s;
        }
        if let Some(i) = (0..2).find(|&i| self.text_rect(rect, i).contains(pos)) {
            return Sel::Field(i);
        }
        for (i, s) in self.doc.slots.iter().enumerate() {
            let a = s.2.to_radians();
            let (p, q) = (self.to_screen(rect, (s.0, s.1)), self.to_screen(rect, (s.0 + s.3 * a.cos(), s.1 + s.3 * a.sin())));
            if seg_dist(pos, p, q) < 6.0 {
                return Sel::Pin(i);
            }
        }
        for (i, sh) in self.doc.shapes.iter().enumerate().rev() {
            let outline: Vec<Pos2> = match &sh.geo {
                Geo::Line(v) => v.iter().map(|p| self.to_screen(rect, *p)).collect(),
                Geo::Rect(a, b) => [*a, (b.0, a.1), *b, (a.0, b.1), *a].iter().map(|p| self.to_screen(rect, *p)).collect(),
                Geo::Circle(c, r) => {
                    (0..=32).map(|k| k as f64 * std::f64::consts::TAU / 32.0).map(|t| self.to_screen(rect, (c.0 + r * t.cos(), c.1 + r * t.sin()))).collect()
                }
                Geo::Arc([a, m, e]) => arc_points(*a, *m, *e).iter().map(|p| self.to_screen(rect, *p)).collect(),
                Geo::Other(_) => vec![],
            };
            if outline.windows(2).any(|w| seg_dist(pos, w[0], w[1]) < 6.0) {
                return Sel::Shape(i);
            }
        }
        Sel::None
    }

    /// A finished set of drawing-tool clicks becomes a shape.
    fn finish_draft(&mut self) {
        let d = std::mem::take(&mut self.draft);
        let geo = match (self.tool, d.as_slice()) {
            (Tool::Line, [_, _, ..]) => Geo::Line(d),
            (Tool::Rect, [a, b]) if a != b => Geo::Rect(*a, *b),
            (Tool::Circle, [c, e]) if c != e => Geo::Circle(*c, (e.0 - c.0).hypot(e.1 - c.1)),
            (Tool::Arc, [s, e, m]) => Geo::Arc([*s, *m, *e]),
            _ => return,
        };
        self.push_undo();
        self.doc.shapes.push(Shape::new(geo));
        self.sel = Sel::Shape(self.doc.shapes.len() - 1);
        self.tool = Tool::Select;
    }

    fn delete_selected(&mut self) {
        let (Sel::Shape(i) | Sel::Point(i, _) | Sel::Radius(i)) = self.sel else { return };
        self.push_undo();
        self.doc.shapes.remove(i);
        self.sel = Sel::None;
    }

    fn rotate_selected(&mut self) {
        match self.sel {
            Sel::Pin(i) | Sel::PinEnd(i) => {
                self.push_undo();
                self.doc.slots[i].2 = (self.doc.slots[i].2 + 90.0) % 360.0;
            }
            Sel::Field(i) => {
                self.push_undo();
                let f = &mut self.own_fields()[i];
                f.2 = if f.2 == 0.0 { 90.0 } else { 0.0 };
            }
            _ => {}
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let (mut save, mut cancel) = (false, false);
        let title = format!("Edit the {} symbol", super::classes::name(self.class));
        let closed = batch_dialog::dialog(
            ctx,
            egui::Id::NULL.with("template_editor"),
            1040.0,
            &title,
            Some("Standards"),
            |ui| {
                ui.horizontal_top(|ui| {
                    let side = 270.0;
                    ui.vertical(|ui| {
                        let w = (ui.available_width() - side - 16.0).max(300.0);
                        self.canvas(ui, Vec2::new(w, (ctx.content_rect().height() - 250.0).clamp(280.0, 560.0)));
                    });
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.set_width(side);
                        self.side(ui);
                    });
                });
            },
            |ui| {
                ui.horizontal(|ui| {
                    ui.weak("Drag a handle to move it · R turns a pin · Delete removes a shape · scroll zooms · middle-drag pans · Ctrl+Z undoes");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(ui_kit::primary("Save to standard", ui.visuals())).clicked() {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            },
        );
        if save {
            Outcome::Save(self.template())
        } else if closed || cancel {
            Outcome::Close
        } else {
            Outcome::None
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui, size: Vec2) {
        let pal = theme::palette(ui.visuals());
        let danger = ui.visuals().error_fg_color;
        let (resp, p) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = resp.rect;
        if !self.fitted {
            self.fit(rect.size());
        }
        // Pan, zoom about the pointer.
        if resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) {
            self.pan += resp.drag_delta();
        }
        if let (true, Some(h)) = (resp.hovered(), resp.hover_pos()) {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let before = self.to_sym(rect, h);
                self.scale = (self.scale * (1.0 + scroll * 0.002)).clamp(4.0, 80.0);
                let after = self.to_screen(rect, before);
                self.pan += h - after;
            }
        }
        let typing = ui.ctx().egui_wants_keyboard_input();
        let hover = resp.hover_pos().map(|h| self.snapped(rect, h));

        // Pointer: picking and dragging.
        if self.tool == Tool::Select {
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let hit = self.hit(rect, pos);
                    self.sel = hit;
                    if hit != Sel::None {
                        self.push_undo();
                        self.drag = Some((hit, self.snapped(rect, pos)));
                    }
                }
            } else if resp.clicked() {
                self.sel = resp.interact_pointer_pos().map_or(Sel::None, |pos| self.hit(rect, pos));
            }
            if resp.dragged_by(egui::PointerButton::Primary) {
                if let (Some((sel, last)), Some(pos)) = (self.drag, resp.interact_pointer_pos()) {
                    let now = self.snapped(rect, pos);
                    self.raw = self.to_sym(rect, pos);
                    self.apply_drag(sel, last, now);
                    self.drag = Some((sel, now));
                }
            }
            if resp.drag_stopped() {
                self.drag = None;
            }
        } else if let Some(at) = hover {
            if resp.clicked_by(egui::PointerButton::Primary) {
                if self.draft.last() != Some(&at) {
                    self.draft.push(at);
                }
                if self.draft.len() >= self.tool.clicks() {
                    self.finish_draft();
                }
            }
            if resp.double_clicked() && self.tool == Tool::Line {
                self.finish_draft();
            }
        }
        if !typing {
            let pressed = |k| ui.input(|i| i.key_pressed(k));
            if pressed(egui::Key::Escape) {
                (self.draft, self.tool) = (vec![], Tool::Select);
            }
            if pressed(egui::Key::Enter) && self.tool == Tool::Line {
                self.finish_draft();
            }
            if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
                self.delete_selected();
            }
            if pressed(egui::Key::R) {
                self.rotate_selected();
            }
            if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
                self.undo_once();
            }
        }

        // Drawing.
        p.rect(rect, 4.0, pal.bg, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
        let p = p.with_clip_rect(rect);
        self.paint_grid(&p, rect, &pal);
        let accent = pal.accent;
        let color = |sel: bool| if sel { accent } else { pal.fg };
        for (i, sh) in self.doc.shapes.iter().enumerate() {
            let on = matches!(self.sel, Sel::Shape(j) | Sel::Point(j, _) | Sel::Radius(j) if j == i);
            self.paint_shape(&p, rect, sh, color(on), &pal);
        }
        let hide = self.doc.hide_numbers;
        for (i, s) in self.doc.slots.iter().enumerate() {
            let on = matches!(self.sel, Sel::Pin(j) | Sel::PinEnd(j) if j == i);
            let a = s.2.to_radians();
            let (c, e) = (self.to_screen(rect, (s.0, s.1)), self.to_screen(rect, (s.0 + s.3 * a.cos(), s.1 + s.3 * a.sin())));
            p.line_segment([c, e], Stroke::new((0.254 * self.scale).max(1.2), color(on)));
            p.circle_stroke(c, 3.0, Stroke::new(1.2, if on { accent } else { danger }));
            if !hide {
                p.text(c.lerp(e, 0.5) + Vec2::new(0.0, -6.0), Align2::CENTER_BOTTOM, (i + 1).to_string(), FontId::monospace(10.0), pal.muted);
            }
        }
        self.paint_texts(&p, rect, &pal);
        self.paint_handles(&p, rect, &pal);
        // The shape being drawn.
        if let Some(at) = hover.filter(|_| self.tool != Tool::Select) {
            let mut pts = self.draft.clone();
            pts.push(at);
            let stroke = Stroke::new(1.4, accent);
            let sh = match (self.tool, pts.as_slice()) {
                (Tool::Line, _) if pts.len() >= 2 => Some(Geo::Line(pts)),
                (Tool::Rect, [a, b]) => Some(Geo::Rect(*a, *b)),
                (Tool::Circle, [c, e]) => Some(Geo::Circle(*c, (e.0 - c.0).hypot(e.1 - c.1))),
                (Tool::Arc, [s, e]) => Some(Geo::Line(vec![*s, *e])),
                (Tool::Arc, [s, e, m]) => Some(Geo::Arc([*s, *m, *e])),
                _ => None,
            };
            if let Some(geo) = sh {
                self.paint_shape(&p, rect, &Shape::new(geo), stroke.color, &pal);
            }
            p.circle_filled(self.to_screen(rect, at), 3.0, accent);
        }
        if let Some(at) = hover {
            p.text(
                rect.left_bottom() + Vec2::new(8.0, -6.0),
                Align2::LEFT_BOTTOM,
                format!("x {}  y {} mm", fmt(at.0), fmt(at.1)),
                FontId::monospace(11.0),
                pal.muted,
            );
        }
    }

    fn undo_once(&mut self) {
        if let Some(d) = self.undo.pop() {
            self.doc = d;
            self.sel = Sel::None;
        }
    }

    fn apply_drag(&mut self, sel: Sel, last: Pt, now: Pt) {
        match sel {
            Sel::Shape(i) => self.doc.shapes[i].translate((now.0 - last.0, now.1 - last.1)),
            Sel::Point(i, j) => self.doc.shapes[i].set_point(j, now),
            Sel::Radius(i) => {
                if let Geo::Circle(c, r) = &mut self.doc.shapes[i].geo {
                    *r = (now.0 - c.0).hypot(now.1 - c.1).max(self.grid / 2.0);
                }
            }
            Sel::Pin(i) => (self.doc.slots[i].0, self.doc.slots[i].1) = now,
            Sel::PinEnd(i) => {
                let s = &mut self.doc.slots[i];
                let a = s.2.to_radians();
                // Along the pin only: its length, never its direction, in steps of 50 mil or the finer grid.
                let step = self.grid.min(1.27);
                let along = (self.raw.0 - s.0) * a.cos() + (self.raw.1 - s.1) * a.sin();
                s.3 = ((along / step).round() * step).max(step);
            }
            Sel::Field(i) => {
                let f = &mut self.own_fields()[i];
                (f.0, f.1) = now;
            }
            Sel::None => {}
        }
    }

    fn paint_grid(&self, p: &egui::Painter, rect: Rect, pal: &theme::Palette) {
        let step = self.grid as f32 * self.scale;
        // Never denser than a dot every 7 px; the rest is still snapped to.
        let step = if step < 7.0 { step * (7.0 / step).ceil() } else { step };
        let origin = self.to_screen(rect, (0.0, 0.0));
        let (nx0, nx1) = (((rect.left() - origin.x) / step).floor() as i32, ((rect.right() - origin.x) / step).ceil() as i32);
        let (ny0, ny1) = (((rect.top() - origin.y) / step).floor() as i32, ((rect.bottom() - origin.y) / step).ceil() as i32);
        for ix in nx0..=nx1 {
            for iy in ny0..=ny1 {
                p.circle_filled(origin + Vec2::new(ix as f32 * step, iy as f32 * step), 0.9, pal.line);
            }
        }
        let axis = Stroke::new(1.0, pal.line);
        p.hline(rect.x_range(), origin.y, axis);
        p.vline(origin.x, rect.y_range(), axis);
    }

    fn paint_shape(&self, p: &egui::Painter, rect: Rect, sh: &Shape, color: Color32, pal: &theme::Palette) {
        let stroke = Stroke::new((sh.width as f32 * self.scale).max(1.2), color);
        let at = |q: Pt| self.to_screen(rect, q);
        let fill = match sh.fill.as_str() {
            "outline" => color,
            "background" => pal.row,
            _ => Color32::TRANSPARENT,
        };
        match &sh.geo {
            Geo::Line(v) => {
                let pts: Vec<Pos2> = v.iter().map(|q| at(*q)).collect();
                if fill != Color32::TRANSPARENT && pts.len() >= 3 {
                    p.add(egui::Shape::convex_polygon(pts, fill, stroke));
                } else {
                    p.add(egui::Shape::line(pts, stroke));
                }
            }
            Geo::Rect(a, b) => {
                p.rect(Rect::from_two_pos(at(*a), at(*b)), 0.0, fill, stroke, egui::StrokeKind::Middle);
            }
            Geo::Circle(c, r) => {
                p.add(egui::Shape::circle_filled(at(*c), *r as f32 * self.scale, fill));
                p.circle_stroke(at(*c), *r as f32 * self.scale, stroke);
            }
            Geo::Arc([a, m, e]) => {
                p.add(egui::Shape::line(arc_points(*a, *m, *e).iter().map(|q| at(*q)).collect(), stroke));
            }
            Geo::Other(_) => {}
        }
    }

    fn paint_texts(&self, p: &egui::Painter, rect: Rect, pal: &theme::Palette) {
        for (i, (text, f)) in [(format!("{}?", self.class.prefix()), self.fields()[0]), ("Value".to_string(), self.fields()[1])].into_iter().enumerate() {
            let size = (self.sizes()[i] as f32 * self.scale * 0.8).max(6.0);
            let color = if self.sel == Sel::Field(i) { pal.accent } else { pal.fg };
            let galley = p.layout_no_wrap(text, FontId::proportional(size), color);
            let left = f.0 > 0.0 && f.2 == 0.0;
            let local = if left { Vec2::new(0.0, galley.size().y / 2.0) } else { galley.size() / 2.0 };
            let angle = -(f.2 as f32).to_radians();
            let rot = |v: Vec2| Vec2::new(v.x * angle.cos() - v.y * angle.sin(), v.x * angle.sin() + v.y * angle.cos());
            let top_left = self.to_screen(rect, (f.0, f.1)) - rot(local);
            p.add(egui::epaint::TextShape::new(top_left, galley, color).with_angle(angle));
            let at = self.to_screen(rect, (f.0, f.1));
            p.line_segment([at - Vec2::new(3.0, 0.0), at + Vec2::new(3.0, 0.0)], Stroke::new(1.0, pal.muted));
            p.line_segment([at - Vec2::new(0.0, 3.0), at + Vec2::new(0.0, 3.0)], Stroke::new(1.0, pal.muted));
        }
    }

    /// Handles of whatever is selected.
    fn paint_handles(&self, p: &egui::Painter, rect: Rect, pal: &theme::Palette) {
        let sq = |pt: Pt| {
            let c = self.to_screen(rect, pt);
            p.rect(Rect::from_center_size(c, Vec2::splat(7.0)), 0.0, pal.bg, Stroke::new(1.4, pal.accent), egui::StrokeKind::Middle);
        };
        match self.sel {
            Sel::Shape(i) | Sel::Point(i, _) | Sel::Radius(i) => {
                for pt in self.doc.shapes[i].points() {
                    sq(pt);
                }
                if let Geo::Circle(c, r) = self.doc.shapes[i].geo {
                    sq((c.0 + r, c.1));
                }
            }
            Sel::Pin(i) | Sel::PinEnd(i) => {
                let s = self.doc.slots[i];
                let a = s.2.to_radians();
                sq((s.0, s.1));
                sq((s.0 + s.3 * a.cos(), s.1 + s.3 * a.sin()));
            }
            _ => {}
        }
    }

    fn side(&mut self, ui: &mut egui::Ui) {
        ui_kit::section_label(ui, "Tools");
        ui.horizontal_wrapped(|ui| {
            for (t, label) in [(Tool::Select, "Select"), (Tool::Line, "Line"), (Tool::Rect, "Rectangle"), (Tool::Circle, "Circle"), (Tool::Arc, "Arc")] {
                if ui.selectable_label(self.tool == t, label).clicked() {
                    (self.tool, self.draft) = (t, vec![]);
                }
            }
        });
        ui.weak(match self.tool {
            Tool::Select => "Click or drag handles, text and shapes.",
            Tool::Line => "Click each point; Enter or double-click finishes.",
            Tool::Rect => "Click two opposite corners.",
            Tool::Circle => "Click the centre, then a point on the edge.",
            Tool::Arc => "Click the start, the end, then a point the arc passes through.",
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Grid");
            let now = GRIDS.iter().find(|g| g.0 == self.grid).map_or("", |g| g.1);
            egui::ComboBox::from_id_salt("tpl_grid").selected_text(now).show_ui(ui, |ui| {
                for (g, label) in GRIDS {
                    ui.selectable_value(&mut self.grid, g, label);
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).clicked() {
                self.undo_once();
            }
            if ui.button("Fit").clicked() {
                self.fitted = false;
            }
            if ui.button("Reset to default").on_hover_text("Go back to the built-in drawing. Nothing is kept until you Save to standard.").clicked() {
                if let Some(b) = symbols::template_of(self.class) {
                    self.push_undo();
                    self.doc = Doc::of(b);
                    self.sel = Sel::None;
                }
            }
        });
        let mut hide = !self.doc.hide_numbers;
        if ui.checkbox(&mut hide, "Show pin numbers").changed() {
            self.push_undo();
            self.doc.hide_numbers = !hide;
        }
        ui.add_space(8.0);
        ui_kit::section_label(ui, "Selected");
        self.properties(ui);
        ui.add_space(8.0);
        ui_kit::section_label(ui, "Text size");
        let mut sizes = self.sizes();
        let mut edited = false;
        for (label, v) in ["Reference", "Value"].into_iter().zip(sizes.iter_mut()) {
            ui.horizontal(|ui| {
                ui.label(label);
                edited |= ui.add(egui::DragValue::new(v).speed(0.05).range(0.5..=5.0).suffix(" mm")).changed();
            });
        }
        if edited {
            self.push_undo();
            self.doc.sizes = Some(sizes);
        }
        ui.weak("KiCad's default is 1.27 mm (50 mil).");
        if self.doc.fields.is_some()
            && ui.small_button("Put text above and below").on_hover_text("Forget the text positions: Reference above the body, Value below").clicked()
        {
            self.push_undo();
            self.doc.fields = None;
        }
    }

    /// Numbers for the selected thing, for exactness the mouse can't give.
    fn properties(&mut self, ui: &mut egui::Ui) {
        let grid = self.grid;
        let mut change = false;
        match self.sel {
            Sel::None => {
                ui.weak("Nothing selected.");
            }
            Sel::Shape(i) | Sel::Point(i, _) | Sel::Radius(i) => {
                let mut sh = self.doc.shapes[i].clone();
                ui.horizontal(|ui| {
                    ui.label("Line width");
                    change |= ui.add(egui::DragValue::new(&mut sh.width).speed(0.01).range(0.0..=2.0).suffix(" mm")).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Fill");
                    egui::ComboBox::from_id_salt("tpl_fill").selected_text(sh.fill.clone()).show_ui(ui, |ui| {
                        for f in ["none", "outline", "background"] {
                            change |= ui.selectable_value(&mut sh.fill, f.to_string(), f).changed();
                        }
                    });
                });
                if let Geo::Circle(_, r) = &mut sh.geo {
                    ui.horizontal(|ui| {
                        ui.label("Radius");
                        change |= ui.add(egui::DragValue::new(r).speed(grid).range(0.1..=40.0).suffix(" mm")).changed();
                    });
                }
                if let Sel::Point(_, j) = self.sel {
                    let mut pt = sh.points()[j];
                    ui.horizontal(|ui| {
                        change |= ui.add(egui::DragValue::new(&mut pt.0).speed(grid).prefix("X ").suffix(" mm")).changed();
                        change |= ui.add(egui::DragValue::new(&mut pt.1).speed(grid).prefix("Y ").suffix(" mm")).changed();
                    });
                    sh.set_point(j, pt);
                }
                if change {
                    self.push_undo();
                    self.doc.shapes[i] = sh;
                }
                if ui.button("Delete shape").clicked() {
                    self.delete_selected();
                }
            }
            Sel::Pin(i) | Sel::PinEnd(i) => {
                let mut s = self.doc.slots[i];
                ui.label(format!("Pin {}", i + 1));
                ui.horizontal(|ui| {
                    change |= ui.add(egui::DragValue::new(&mut s.0).speed(grid).prefix("X ").suffix(" mm")).changed();
                    change |= ui.add(egui::DragValue::new(&mut s.1).speed(grid).prefix("Y ").suffix(" mm")).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Length");
                    change |= ui.add(egui::DragValue::new(&mut s.3).speed(grid).range(0.254..=20.0).suffix(" mm")).changed();
                });
                egui::ComboBox::from_id_salt("tpl_pin_dir")
                    .selected_text(DIRECTIONS.iter().find(|d| d.0 == s.2).map_or(format!("{}°", s.2), |d| d.1.to_string()))
                    .show_ui(ui, |ui| {
                        for (v, label) in DIRECTIONS {
                            change |= ui.selectable_value(&mut s.2, v, label).changed();
                        }
                    });
                let on = |v: f64| ((v / 2.54).round() * 2.54 - v).abs() < 1e-3;
                if !(on(s.0) && on(s.1)) {
                    ui.colored_label(ui.visuals().warn_fg_color, "Off the 100 mil grid: wires meeting it may miss")
                        .on_hover_text("KLC wants every pin end on the 2.54 mm grid.");
                }
                if change {
                    self.push_undo();
                    self.doc.slots[i] = s;
                }
            }
            Sel::Field(i) => {
                let mut f = self.fields()[i];
                ui.label(["Reference", "Value"][i]);
                ui.horizontal(|ui| {
                    change |= ui.add(egui::DragValue::new(&mut f.0).speed(grid).prefix("X ").suffix(" mm")).changed();
                    change |= ui.add(egui::DragValue::new(&mut f.1).speed(grid).prefix("Y ").suffix(" mm")).changed();
                });
                let horizontal = f.2 == 0.0;
                if ui.selectable_label(!horizontal, "Vertical text (R)").clicked() {
                    f.2 = if horizontal { 90.0 } else { 0.0 };
                    change = true;
                }
                if change {
                    self.push_undo();
                    self.own_fields()[i] = f;
                }
            }
        }
    }
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.into() }
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() < 1e-6 { 0.0 } else { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) };
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> TemplateEditor {
        TemplateEditor::new(Class::Resistor, symbols::template_of(Class::Resistor).unwrap().clone(), 1.27)
    }

    #[test]
    fn dragging_changes_the_template_and_undo_restores_it() {
        let mut e = editor();
        let before = e.template();
        let text_above = e.fields()[0];
        e.push_undo();
        e.apply_drag(Sel::Pin(0), (0.0, 5.08), (0.0, 7.62));
        e.raw = (0.0, 5.3);
        e.apply_drag(Sel::PinEnd(0), (0.0, 0.0), (0.0, 5.08));
        e.apply_drag(Sel::Field(1), (0.0, 0.0), (5.08, -2.54));
        e.doc.sizes = Some([1.0, 1.5]);
        let t = e.template();
        assert_eq!((t.slots[0].0, t.slots[0].1), (0.0, 7.62));
        assert!((t.slots[0].3 - 2.54).abs() < 1e-9, "the pin end moved along the pin, so only its length changed: {:?}", t.slots[0]);
        e.raw = (0.0, 6.5);
        e.apply_drag(Sel::PinEnd(0), (0.0, 0.0), (0.0, 7.62));
        assert!((e.doc.slots[0].3 - 1.27).abs() < 1e-9, "a pin can be made shorter than the grid: {:?}", e.doc.slots[0]);
        e.raw = (0.0, 5.08);
        e.apply_drag(Sel::PinEnd(0), (0.0, 0.0), (0.0, 5.08));
        assert_eq!(t.fields.unwrap()[1], (5.08, -2.54, 0.0));
        assert_eq!(t.fields.unwrap()[0], text_above, "the other text stays where it was drawn");
        assert_eq!(t.field_sizes, Some([1.0, 1.5]));
        e.undo_once();
        assert_eq!(e.template().slots.to_vec(), before.slots.to_vec());
    }

    #[test]
    fn drawing_tools_make_shapes() {
        let mut e = editor();
        let n = e.doc.shapes.len();
        e.tool = Tool::Rect;
        e.draft = vec![(0.0, 0.0), (2.54, 2.54)];
        e.finish_draft();
        e.tool = Tool::Arc;
        e.draft = vec![(0.0, 0.0), (0.0, 2.54), (1.27, 1.27)];
        e.finish_draft();
        assert_eq!(e.doc.shapes.len(), n + 2);
        assert!(matches!(e.doc.shapes[n + 1].geo, Geo::Arc([(0.0, 0.0), (1.27, 1.27), (0.0, 2.54)])));
        assert_eq!(e.tool, Tool::Select);
        e.delete_selected();
        assert_eq!(e.doc.shapes.len(), n + 1);
    }
}
