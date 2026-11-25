#![allow(dead_code)] // the API for W7.1-W7.5; each item gets a caller as its tab adopts the kit

//! The look every tab shares: severity colours, status cells, badges and one table.
//!
//! - `Level` is the one severity scale (also used by the log); `Level::color` gives its colour for the current theme.
//! - `status_cell` draws a Footprint/3D/Datasheet-style cell; `badge` draws a small coloured pill.
//! - `table` draws a list with a fixed header, resizable columns and sortable headers, laying out only the visible rows,
//!   so a 2,000-row list costs the same as a 20-row one. The caller sorts its own data by the returned `Sort`.

use eframe::egui::{self, Color32, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder, TableRow};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

impl Level {
    pub fn color(self, v: &egui::Visuals) -> Color32 {
        match (self, v.dark_mode) {
            // The wireframes' --info / --ok / --warn / --err; the light set is the same hues, darkened for white.
            (Level::Info, true) => Color32::from_rgb(0x6f, 0xb3, 0xff),
            (Level::Info, false) => Color32::from_rgb(0x1f, 0x6f, 0xc9),
            (Level::Ok, true) => Color32::from_rgb(0x5c, 0xc4, 0x89),
            (Level::Ok, false) => Color32::from_rgb(0x1d, 0x7f, 0x4e),
            (Level::Warn, true) => Color32::from_rgb(0xe0, 0xa9, 0x4a),
            (Level::Warn, false) => Color32::from_rgb(0x9a, 0x65, 0x00),
            (Level::Error, true) => Color32::from_rgb(0xef, 0x6b, 0x6b),
            (Level::Error, false) => Color32::from_rgb(0xb4, 0x2c, 0x2c),
        }
    }
}

/// A settings page a tab can ask for with `cx.open_settings(Section)`. `General`, `Sources` and `Standards`
/// are the tree's groups: they open on the group's first page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    General,
    Libraries,
    WatchFolders,
    KiCad,
    Sources,
    Lcsc,
    DigiKey,
    Mouser,
    Nexar,
    SnapMagic,
    Search,
    Import,
    Standards,
    Symbols,
    Footprints,
    PartInfo,
    Bom,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Link,
    Missing,
    Broken,
    /// Not there, and that is fine (a sim model).
    Optional,
}

/// One cell of a status column. `what` names the thing for the tooltip ("Footprint").
pub fn status_cell(ui: &mut Ui, status: Status, what: &str) -> egui::Response {
    let v = ui.visuals();
    let (text, color, tip) = match status {
        Status::Ok => ("✔", Level::Ok.color(v), format!("{what} present")),
        Status::Link => ("link", Level::Info.color(v), format!("{what} points at a web page or another library")),
        Status::Missing => ("missing", Level::Warn.color(v), format!("No {what}")),
        Status::Broken => ("broken", Level::Error.color(v), format!("{what} is set but its file isn't there")),
        Status::Optional => ("—", v.weak_text_color(), format!("No {what} (optional)")),
    };
    // A filled-tint chip, as the wireframes draw SYM / FP / 3D / DS.
    // The tint is painted behind the label, so the text keeps its column's left edge.
    let bg = ui.painter().add(egui::Shape::Noop);
    let r = ui.label(RichText::new(text).color(color).small());
    ui.painter().set(bg, egui::Shape::rect_filled(r.rect.expand2(egui::vec2(5.0, 1.0)), 3.0, color.gamma_multiply(0.15)));
    r.on_hover_text(tip)
}

/// The wireframes' SYM / FP / 3D / DS chip: mono, tinted by `status`, `a11y` is its accessible name ("Datasheet: missing").
/// Hover shows `tip`; the response senses clicks, so a missing one can be made fixable by the caller.
pub fn file_chip(ui: &mut Ui, text: &str, status: Status, tip: &str, a11y: &str) -> egui::Response {
    let v = ui.visuals();
    let color = match status {
        Status::Ok => Level::Ok.color(v),
        Status::Link => Level::Info.color(v),
        Status::Missing => Level::Warn.color(v),
        Status::Broken => Level::Error.color(v),
        Status::Optional => v.weak_text_color(),
    };
    let galley = ui.painter().layout_no_wrap(text.into(), egui::FontId::monospace(11.0), color);
    let size = egui::vec2((galley.size().x + 10.0).max(28.0), galley.size().y + 6.0);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect_filled(rect, 3.0, color.gamma_multiply(0.15));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, a11y));
    r.on_hover_text(tip)
}

/// A filter chip: `label` with an optional count, filled in the accent when `on`.
pub fn chip(ui: &mut Ui, label: &str, count: Option<usize>, on: bool) -> egui::Response {
    let text = match count {
        Some(n) => format!("{label} {n}"),
        None => label.to_string(),
    };
    let accent = ui.visuals().selection.stroke.color;
    let t = if on { RichText::new(text).color(accent) } else { RichText::new(text) };
    ui.add(egui::Button::new(t).selected(on).min_size(egui::vec2(0.0, 26.0)))
}

/// The accent-filled call-to-action button (one per view).
pub fn primary(text: &str, v: &egui::Visuals) -> egui::Button<'static> {
    let on = if v.dark_mode { Color32::from_rgb(0x07, 0x18, 0x1b) } else { Color32::WHITE };
    egui::Button::new(RichText::new(text).strong().color(on)).fill(crate::theme::palette(v).accent).stroke(egui::Stroke::NONE)
}

/// A destructive button: red text and outline, no fill. `filled` for the confirm button of a delete dialog.
pub fn danger(text: &str, filled: bool, v: &egui::Visuals) -> egui::Button<'static> {
    let c = Level::Error.color(v);
    if filled {
        egui::Button::new(RichText::new(text).strong().color(Color32::from_rgb(0x2a, 0x0b, 0x0b))).fill(c).stroke(egui::Stroke::NONE)
    } else {
        egui::Button::new(RichText::new(text).color(c)).fill(Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, c))
    }
}

/// A button with no frame until hovered, for secondary actions.
pub fn ghost(text: &str) -> egui::Button<'static> {
    egui::Button::new(text.to_string()).frame(false).frame_when_inactive(false)
}

/// A small mono caps label that heads a section ("FIELDS").
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).monospace().size(11.0).color(ui.visuals().weak_text_color()));
}

/// A small coloured pill, e.g. "3 errors".
pub fn badge(ui: &mut Ui, text: &str, level: Level) -> egui::Response {
    let color = level.color(ui.visuals());
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(RichText::new(text).color(color).small()))
        .response
}

/// Which column a table is sorted by (an index into its `Col`s) and in which direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub col: usize,
    pub asc: bool,
}

pub struct Col {
    title: &'static str,
    width: f32,
    sortable: bool,
}

impl Col {
    /// A sortable column starting `width` points wide; the user can drag it wider or narrower.
    pub fn new(title: &'static str, width: f32) -> Self {
        Self { title, width, sortable: true }
    }

    /// A column that takes whatever width the others leave.
    pub fn rest(title: &'static str) -> Self {
        Self::new(title, 0.0)
    }

    pub fn no_sort(mut self) -> Self {
        self.sortable = false;
        self
    }
}

pub const ROW_HEIGHT: f32 = 28.0;

/// A table with a fixed header. `row` is called only for rows in view: it gets the row's index and adds one
/// `row.col(|ui| …)` per column, in order. Clicking a sortable header changes `sort`; the caller re-sorts its data.
/// Selection: `row.set_selected(…)` and `row.response()` (egui_extras' `TableRow`).
pub fn table(ui: &mut Ui, id: &str, cols: &[Col], sort: &mut Sort, rows: usize, row: impl FnMut(usize, &mut TableRow)) {
    table_with_head(ui, id, cols, sort, rows, |_, _| false, row)
}

/// [`table`] where `head(column, ui)` may draw a header cell itself (a select-all box) and return true.
pub fn table_with_head(
    ui: &mut Ui,
    id: &str,
    cols: &[Col],
    sort: &mut Sort,
    rows: usize,
    mut head: impl FnMut(usize, &mut Ui) -> bool,
    mut row: impl FnMut(usize, &mut TableRow),
) {
    let mut t = TableBuilder::new(ui).id_salt(id).striped(false).resizable(true).sense(Sense::click()).auto_shrink(false);
    for c in cols {
        t = t.column(if c.width > 0.0 { Column::initial(c.width).at_least(24.0).clip(true) } else { Column::remainder().at_least(60.0).clip(true) });
    }
    t.header(ROW_HEIGHT, |mut h| {
        for (i, c) in cols.iter().enumerate() {
            h.col(|ui| {
                if !head(i, ui) {
                    sort_header(ui, c, i, sort);
                }
            });
        }
    })
    .body(|body| {
        body.rows(ROW_HEIGHT, rows, |mut r| {
            let i = r.index();
            row(i, &mut r);
        });
    });
}

fn sort_header(ui: &mut Ui, c: &Col, i: usize, sort: &mut Sort) {
    if !c.sortable {
        ui.strong(c.title);
        return;
    }
    let active = sort.col == i;
    let arrow = match (active, sort.asc) {
        (false, _) => "",
        (true, true) => " ▲",
        (true, false) => " ▼",
    };
    let r = ui.add(egui::Label::new(RichText::new(format!("{}{arrow}", c.title)).strong()).sense(Sense::click()).selectable(false));
    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *sort = Sort { col: i, asc: !(active && sort.asc) };
    }
}
