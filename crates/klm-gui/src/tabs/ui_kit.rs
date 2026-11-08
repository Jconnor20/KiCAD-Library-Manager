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
            (Level::Info, true) => Color32::from_rgb(110, 170, 240),
            (Level::Info, false) => Color32::from_rgb(30, 100, 190),
            (Level::Ok, true) => Color32::from_rgb(84, 190, 120),
            (Level::Ok, false) => Color32::from_rgb(30, 130, 60),
            (Level::Warn, true) => Color32::from_rgb(230, 160, 60),
            (Level::Warn, false) => Color32::from_rgb(170, 100, 0),
            (Level::Error, true) => Color32::from_rgb(240, 100, 100),
            (Level::Error, false) => Color32::from_rgb(190, 40, 40),
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
    ui.label(RichText::new(text).color(color)).on_hover_text(tip)
}

/// A small coloured pill, e.g. "3 errors".
pub fn badge(ui: &mut Ui, text: &str, level: Level) -> egui::Response {
    let color = level.color(ui.visuals());
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(8)
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

pub const ROW_HEIGHT: f32 = 22.0;

/// A table with a fixed header. `row` is called only for rows in view: it gets the row's index and adds one
/// `row.col(|ui| …)` per column, in order. Clicking a sortable header changes `sort`; the caller re-sorts its data.
/// Selection: `row.set_selected(…)` and `row.response()` (egui_extras' `TableRow`).
pub fn table(ui: &mut Ui, id: &str, cols: &[Col], sort: &mut Sort, rows: usize, mut row: impl FnMut(usize, &mut TableRow)) {
    let mut t = TableBuilder::new(ui).id_salt(id).striped(true).resizable(true).sense(Sense::click()).auto_shrink(false);
    for c in cols {
        t = t.column(if c.width > 0.0 { Column::initial(c.width).at_least(24.0).clip(true) } else { Column::remainder().at_least(60.0).clip(true) });
    }
    t.header(ROW_HEIGHT, |mut h| {
        for (i, c) in cols.iter().enumerate() {
            h.col(|ui| sort_header(ui, c, i, sort));
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
