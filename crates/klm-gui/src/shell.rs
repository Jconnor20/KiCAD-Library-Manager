//! The app frame from the wireframes: top bar (library/project chips, Ctrl+K omnibox, KiCad state), left rail,
//! page area and a status bar that opens the log as a drawer.

use crate::tabs::ui_kit::{self, Level, Section};
use crate::theme;
use crate::{App, Msg, Tab, project_dir};
use eframe::egui::{self, Align, Color32, Layout, Margin, Pos2, RichText, Sense, Stroke, Ui, Vec2};
use klm_core::kicad_env;

const TOP_H: f32 = 44.0;
const RAIL_W: f32 = 64.0;
const STATUS_H: f32 = 28.0;

/// The rail's items. Each owns one or two tabs: Import also holds "BOM to library", Standards holds Clean up and Footprint.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Search,
    Import,
    Library,
    Bom,
    Standards,
    History,
    /// Not in `ALL`: the rail draws it at the bottom.
    Settings,
}

impl Page {
    const ALL: [Page; 6] = [Page::Search, Page::Import, Page::Library, Page::Bom, Page::Standards, Page::History];

    fn title(self) -> &'static str {
        match self {
            Page::Search => "Search",
            Page::Import => "Import",
            Page::Library => "Library",
            Page::Bom => "BOM",
            Page::Standards => "Standards",
            Page::History => "History",
            Page::Settings => "Settings",
        }
    }

    fn of(tab: &Tab) -> Page {
        match tab {
            Tab::Search => Page::Search,
            Tab::Import | Tab::BomImport => Page::Import,
            Tab::Library => Page::Library,
            Tab::Bom => Page::Bom,
            Tab::Classes | Tab::Health | Tab::ProjectCheck | Tab::Cleanup | Tab::Footprint => Page::Standards,
            Tab::History => Page::History,
            Tab::Settings => Page::Settings,
        }
    }

    fn first_tab(self) -> Tab {
        match self {
            Page::Search => Tab::Search,
            Page::Import => Tab::Import,
            Page::Library => Tab::Library,
            Page::Bom => Tab::Bom,
            Page::Standards => Tab::Classes,
            Page::History => Tab::History,
            Page::Settings => Tab::Settings,
        }
    }

    /// Sub-pages shown as a segmented control above the page.
    fn subs(self) -> &'static [(&'static str, fn() -> Tab)] {
        match self {
            Page::Import => &[("Parts", || Tab::Import), ("From a BOM", || Tab::BomImport)],
            Page::Standards => &[
                ("Classes", || Tab::Classes),
                ("Create", || Tab::Footprint),
                ("Library health", || Tab::Health),
                ("Project check", || Tab::ProjectCheck),
                ("KiCad tables", || Tab::Cleanup),
            ],
            _ => &[],
        }
    }
}

/// Ctrl+K: type to find a library part, a recent search, a page or an action.
#[derive(Default)]
pub struct Omni {
    text: String,
    open: bool,
    /// Highlighted row of the result list.
    sel: usize,
    /// Library parts (MPN, manufacturer, category) and recent searches, read when the box gets focus.
    parts: Vec<(String, String, String)>,
    recent: Vec<String>,
    had_focus: bool,
    /// Where the field and its result list were last drawn, so a click in the list isn't "focus lost".
    field: Option<egui::Rect>,
    list: Option<egui::Rect>,
    /// The log drawer's filter.
    log_filter: Option<Level>,
}

enum Cmd {
    Go(Tab),
    Part(String),
    Search(String),
    Settings,
    Log,
}

impl Omni {
    /// Reads the libraries' parts and the recent searches; called when the box takes focus.
    fn load(&mut self, cx: &crate::AppCtx) {
        self.parts = cx.roots().iter().flat_map(|r| r.list_parts()).map(|p| (p.mpn, p.manufacturer, p.category)).collect();
        self.recent = klm_core::store::Store::default_path()
            .and_then(|p| klm_core::store::Store::open(&p).ok())
            .and_then(|s| s.recent_searches(8).ok())
            .unwrap_or_default();
    }
}

struct Hit {
    section: &'static str,
    title: String,
    detail: String,
    hint: &'static str,
    cmd: Cmd,
}

/// The wireframes' look for primary actions: a solid teal button, dark text.
pub(crate) fn primary(ui: &Ui, text: &str) -> egui::Button<'static> {
    let on = if ui.visuals().dark_mode { Color32::from_rgb(0x0b, 0x1f, 0x1d) } else { Color32::WHITE };
    egui::Button::new(RichText::new(text.to_string()).strong().color(on)).fill(theme::palette(ui.visuals()).accent)
}

/// A small mono caption in capitals, the wireframes' section label.
pub(crate) fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).monospace().size(10.5).color(theme::palette(ui.visuals()).muted));
}

/// A filter chip: bordered, tinted teal while on.
pub(crate) fn chip(ui: &mut Ui, text: &str, on: bool) -> egui::Response {
    let pal = theme::palette(ui.visuals());
    let (fill, line, fg) = if on { (pal.accent.gamma_multiply(0.18), pal.accent, pal.accent) } else { (pal.field, pal.line, pal.fg) };
    ui.add(egui::Button::new(RichText::new(text).size(12.5).color(fg)).fill(fill).stroke(Stroke::new(1.0, line)).corner_radius(4))
}

/// A bordered panel, as the wireframes' cards.
pub(crate) fn card<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
    let pal = theme::palette(ui.visuals());
    egui::Frame::new().fill(pal.panel).stroke(Stroke::new(1.0, pal.line)).corner_radius(4).inner_margin(Margin::same(14)).show(ui, body).inner
}

/// Paints a rail icon in a 16×16 box, as the wireframes' SVGs: 1.6 px round strokes.
fn icon(p: &egui::Painter, page: Option<Page>, c: Pos2, col: Color32) {
    let s = Stroke::new(1.6, col);
    let pt = |x: f32, y: f32| Pos2::new(c.x - 8.0 + x, c.y - 8.0 + y);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([pt(a.0, a.1), pt(b.0, b.1)], s);
    match page {
        Some(Page::Search) => {
            p.circle_stroke(pt(7.0, 7.0), 4.5, s);
            line((10.5, 10.5), (14.0, 14.0));
        }
        Some(Page::Import) => {
            line((8.0, 2.0), (8.0, 10.0));
            line((4.5, 6.5), (8.0, 10.0));
            line((11.5, 6.5), (8.0, 10.0));
            line((2.5, 13.5), (13.5, 13.5));
        }
        Some(Page::Library) => {
            p.rect_stroke(egui::Rect::from_min_size(pt(2.0, 2.5), Vec2::new(3.0, 11.0)), 0.0, s, egui::StrokeKind::Middle);
            p.rect_stroke(egui::Rect::from_min_size(pt(6.5, 2.5), Vec2::new(3.0, 11.0)), 0.0, s, egui::StrokeKind::Middle);
            line((11.0, 3.0), (14.0, 13.0));
        }
        Some(Page::Bom) => {
            for (i, w) in [10.0, 10.0, 6.0, 6.0].into_iter().enumerate() {
                let y = 3.0 + 3.5 * i as f32;
                line((3.0, y), (3.0 + w, y));
            }
        }
        Some(Page::Standards) => {
            line((2.5, 13.5), (2.5, 2.5));
            line((2.5, 2.5), (13.5, 13.5));
            line((13.5, 13.5), (2.5, 13.5));
        }
        Some(Page::History) => {
            p.circle_stroke(pt(8.0, 8.0), 5.5, s);
            line((8.0, 5.0), (8.0, 8.0));
            line((8.0, 8.0), (10.0, 9.5));
        }
        Some(Page::Settings) | None => {
            p.circle_stroke(pt(8.0, 8.0), 2.2, s);
            for (a, b) in [((8.0, 1.5), (8.0, 3.5)), ((8.0, 12.5), (8.0, 14.5)), ((1.5, 8.0), (3.5, 8.0)), ((12.5, 8.0), (14.5, 8.0))] {
                line(a, b);
            }
        }
    }
}

/// A stroke across the bottom/top/right edge of `r`, for the lined (not tonal) surfaces.
fn edge(ui: &Ui, r: egui::Rect, side: Side) {
    let line = Stroke::new(1.0, theme::palette(ui.visuals()).line);
    let (a, b) = match side {
        Side::Bottom => (r.left_bottom(), r.right_bottom()),
        Side::Top => (r.left_top(), r.right_top()),
        Side::Right => (r.right_top(), r.right_bottom()),
    };
    ui.painter().line_segment([a, b], line);
}

enum Side {
    Top,
    Bottom,
    Right,
}

impl App {
    pub(crate) fn shell(&mut self, ui: &mut Ui) {
        let pal = theme::palette(ui.visuals());
        let bar = egui::Frame::new().fill(pal.rail).inner_margin(Margin::symmetric(12, 0));

        if self.import_tab.needs_setup(&self.cx) {
            // First run: just the title bar and the set-up steps.
            let top = egui::Panel::top("top").exact_size(TOP_H).frame(bar).show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("KLM").strong().size(14.0).color(pal.accent));
                    ui.label(RichText::new("KiCad Library Manager").color(pal.muted));
                });
            });
            edge(ui, top.response.rect, Side::Bottom);
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin::symmetric(12, 10))).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.import_tab.first_run(ui, &mut self.cx));
            });
            return;
        }
        let top = egui::Panel::top("top").exact_size(TOP_H).frame(bar).show(ui, |ui| self.top(ui));
        edge(ui, top.response.rect, Side::Bottom);
        let status = egui::Panel::bottom("status").exact_size(STATUS_H).frame(bar).show(ui, |ui| self.status(ui));
        edge(ui, status.response.rect, Side::Top);
        let rail = egui::Panel::left("rail")
            .exact_size(RAIL_W)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.rail).inner_margin(Margin::symmetric(4, 8)))
            .show(ui, |ui| self.rail(ui));
        edge(ui, rail.response.rect, Side::Right);
        if self.log_open {
            let log = egui::Panel::bottom("log")
                .resizable(true)
                .default_size(120.0)
                .frame(egui::Frame::new().fill(pal.panel).inner_margin(Margin::same(8)))
                .show(ui, |ui| self.log_drawer(ui));
            edge(ui, log.response.rect, Side::Top);
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin::symmetric(12, 10))).show(ui, |ui| self.page(ui));
        self.omnibox_popup(ui);
    }

    fn top(&mut self, ui: &mut Ui) {
        let pal = theme::palette(ui.visuals());
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(RichText::new("KLM").strong().size(14.0).color(pal.accent));
            ui.add_space(4.0);
            self.library_chip(ui);
            self.project_chip(ui);
            ui.add_space(12.0);
            self.omnibox(ui);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let open = kicad_env::kicad_running();
                let color = if open { Level::Warn } else { Level::Ok }.color(ui.visuals());
                ui.label(RichText::new(if open { "KiCad open" } else { "KiCad closed" }).color(pal.muted).size(12.0));
                dot(ui, color);
            });
        });
    }

    /// "Library  Global ▾": where the global library lives and the actions on it.
    fn library_chip(&mut self, ui: &mut Ui) {
        let cx = &mut self.cx;
        chip_menu(ui, "Library", "Global", |ui| {
            match &cx.settings.global_root {
                Some(g) => ui.label(RichText::new(g.display().to_string()).monospace()),
                None => ui.weak("not set up"),
            };
            ui.separator();
            if cx.global_root().is_some() && ui.button("Change location…").on_hover_text("Move the library, or point KLM at where you moved it").clicked() {
                if let Some(d) = rfd::FileDialog::new().set_title("New location for the global library").pick_folder() {
                    cx.send(Msg::MoveGlobal(d));
                }
                ui.close();
            }
            if cx.pending_libs > 0 {
                let tip = "Written to KiCad's library list; re-checked when KiCad closes in case it saves over them";
                if ui.button(format!("Apply {} pending now", cx.pending_libs)).on_hover_text(tip).clicked() {
                    cx.when_kicad_closed(Msg::ApplyPending);
                    ui.close();
                }
            }
            if ui.button("Library settings…").clicked() {
                cx.open_settings(Section::Libraries);
                ui.close();
            }
        });
    }

    fn project_chip(&mut self, ui: &mut Ui) {
        let cx = &mut self.cx;
        let name = cx.project.as_deref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "none".into());
        chip_menu(ui, "Project", &name, |ui| {
            match &cx.project {
                Some(p) => ui.label(RichText::new(p.display().to_string()).monospace()),
                None => ui.weak("none chosen"),
            };
            ui.separator();
            if ui.button("Choose project…").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("KiCad project", &["kicad_pro"]).pick_file() {
                    cx.project = Some(project_dir(&f));
                    cx.settings.last_project = cx.project.clone();
                    cx.save_settings();
                    cx.send(Msg::Library(crate::library::Msg::Refresh));
                }
                ui.close();
            }
            if cx.project.is_some() && ui.button("Close project").clicked() {
                cx.project = None;
                cx.send(Msg::Library(crate::library::Msg::Refresh));
                ui.close();
            }
        });
    }

    fn omnibox(&mut self, ui: &mut Ui) {
        let id = egui::Id::unique("omni_field");
        let pressed = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K));
        let r = ui.add_sized(
            [420.0f32.min(ui.available_width() - 160.0).max(160.0), 30.0],
            egui::TextEdit::singleline(&mut self.omni.text).id(id).hint_text("Search parts, pages, actions…  Ctrl K").margin(Margin::symmetric(10, 6)),
        );
        if pressed {
            r.request_focus();
        }
        let o = &mut self.omni;
        o.field = Some(r.rect);
        if r.has_focus() && !o.had_focus {
            o.load(&self.cx);
        }
        o.had_focus = r.has_focus();
        let over_list = o.list.is_some_and(|l| ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| l.contains(p)));
        o.open = r.has_focus() || (o.open && over_list);
        if r.changed() {
            o.sel = 0;
        }
        if r.has_focus() {
            let n = self.hits().len();
            let o = &mut self.omni;
            let (down, up) =
                ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)));
            if down && o.sel + 1 < n {
                o.sel += 1;
            }
            if up {
                o.sel = o.sel.saturating_sub(1);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                o.text.clear();
                r.surrender_focus();
            }
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let sel = self.omni.sel;
            if let Some(h) = self.hits().into_iter().nth(sel) {
                self.run(h.cmd);
            }
            self.omni.text.clear();
        }
    }

    /// Everything the box can offer for its text, in display order.
    fn hits(&self) -> Vec<Hit> {
        let q = self.omni.text.trim().to_lowercase();
        let has = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
        let mut out = vec![];
        if !q.is_empty() {
            for (mpn, maker, cat) in self.omni.parts.iter().filter(|(m, mk, _)| has(m) || has(mk)).take(5) {
                out.push(Hit {
                    section: "In your library",
                    title: mpn.clone(),
                    detail: format!("{maker} · {cat}"),
                    hint: "Library",
                    cmd: Cmd::Part(mpn.clone()),
                });
            }
        }
        for s in self.omni.recent.iter().filter(|s| has(s)).take(3) {
            out.push(Hit { section: "Recent searches", title: s.clone(), detail: String::new(), hint: "Search", cmd: Cmd::Search(s.clone()) });
        }
        if !q.is_empty() && !self.cx.sources.is_empty() {
            let names: Vec<&str> = self.cx.sources.iter().map(|s| s.name()).collect();
            out.push(Hit {
                section: "Suppliers",
                title: format!("Search {} for \"{}\"", names.join(", "), self.omni.text.trim()),
                detail: String::new(),
                hint: "Search",
                cmd: Cmd::Search(self.omni.text.trim().to_string()),
            });
        }
        let mut pages: Vec<(String, Cmd)> = Page::ALL.iter().map(|p| (format!("Go to {}", p.title()), Cmd::Go(p.first_tab()))).collect();
        pages.push(("Go to BOM to library".into(), Cmd::Go(Tab::BomImport)));
        pages.push(("Go to Create footprint".into(), Cmd::Go(Tab::Footprint)));
        pages.push(("Open Settings".into(), Cmd::Settings));
        pages.push(("Show log".into(), Cmd::Log));
        for (title, cmd) in pages.into_iter().filter(|(n, _)| has(n)) {
            out.push(Hit { section: "Pages and actions", title, detail: String::new(), hint: "", cmd });
        }
        out
    }

    fn run(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Go(t) => self.cx.tab = t,
            Cmd::Part(mpn) => {
                self.cx.send(Msg::Library(crate::library::Msg::Show(mpn)));
                self.cx.tab = Tab::Library;
            }
            Cmd::Search(q) => {
                self.cx.send(Msg::Search(crate::search::Msg::Query(q)));
                self.cx.tab = Tab::Search;
            }
            Cmd::Settings => self.cx.open_settings(Section::General),
            Cmd::Log => self.log_open = true,
        }
    }

    /// The result list under the omnibox while it is open.
    fn omnibox_popup(&mut self, ui: &mut Ui) {
        if !self.omni.open {
            self.omni.list = None;
            return;
        }
        let hits = self.hits();
        let pal = theme::palette(ui.visuals());
        let (mut chosen, mut last) = (None, "");
        let field = self.omni.field.unwrap_or(egui::Rect::from_min_size(Pos2::new(RAIL_W + 200.0, TOP_H - 4.0), Vec2::new(420.0, 30.0)));
        let width = field.width().max(480.0).min(ui.ctx().content_rect().width() - 16.0);
        let pos = field.left_bottom() + Vec2::new(0.0, 4.0);
        let sel = self.omni.sel.min(hits.len().saturating_sub(1));
        let area = egui::Area::new(egui::Id::unique("omni_pop")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).fill(pal.panel).stroke(Stroke::new(1.0, pal.line)).inner_margin(Margin::symmetric(0, 6)).show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                if hits.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.weak("No match");
                    });
                }
                for (i, h) in hits.iter().enumerate() {
                    if h.section != last {
                        last = h.section;
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            caption(ui, h.section);
                        });
                        ui.add_space(2.0);
                    }
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &h.title));
                    if i == sel || resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, pal.accent.gamma_multiply(0.16));
                    }
                    if i == sel {
                        ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(3.0, 28.0)), 0.0, pal.accent);
                    }
                    let font = if matches!(h.cmd, Cmd::Part(_)) { egui::FontId::monospace(13.0) } else { egui::FontId::proportional(13.0) };
                    let t = ui.painter().text(rect.left_center() + Vec2::new(14.0, 0.0), egui::Align2::LEFT_CENTER, &h.title, font, pal.fg);
                    ui.painter().text(
                        egui::pos2(t.right() + 10.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &h.detail,
                        egui::FontId::proportional(12.0),
                        pal.muted,
                    );
                    let hint = if i == sel { "Enter" } else { h.hint };
                    ui.painter().text(
                        rect.right_center() - Vec2::new(14.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        hint,
                        egui::FontId::proportional(11.5),
                        pal.muted,
                    );
                    if resp.clicked() {
                        chosen = Some(i);
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new("↑ ↓ move · Enter open · Esc close").size(11.0).color(pal.muted));
                });
            });
        });
        self.omni.list = Some(area.response.rect);
        if let Some(i) = chosen {
            if let Some(h) = self.hits().into_iter().nth(i) {
                self.run(h.cmd);
            }
            self.omni.text.clear();
            self.omni.open = false;
        }
    }

    fn rail(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 2.0;
        let cur = Page::of(&self.cx.tab);
        for page in Page::ALL {
            // Import: parts waiting. Standards has no cheap issue count (the report is built on demand), so no badge.
            let badge = if page == Page::Import { self.import_tab.waiting() } else { 0 };
            if rail_item(ui, Some(page), page.title(), cur == page, badge).clicked() && cur != page {
                self.cx.tab = page.first_tab();
            }
        }
        let on_settings = self.cx.tab == Tab::Settings;
        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
            if rail_item(ui, None, "Settings", on_settings, 0).clicked() {
                self.cx.open_settings(Section::General);
            }
        });
    }

    fn page(&mut self, ui: &mut Ui) {
        let page = Page::of(&self.cx.tab);
        if !page.subs().is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(page.title()).heading().strong());
                ui.add_space(8.0);
                for (name, to) in page.subs() {
                    let on = std::mem::discriminant(&self.cx.tab) == std::mem::discriminant(&to());
                    if chip(ui, name, on).clicked() {
                        self.cx.tab = to();
                    }
                }
            });
            ui.add_space(6.0);
        }
        if self.cx.tab != Tab::Settings {
            self.checklist(ui);
        }
        match self.cx.tab {
            Tab::Search => self.search_tab.ui(ui, &mut self.cx),
            Tab::Import => self.import_tab.ui(ui, &mut self.cx),
            Tab::Library => self.library_tab.ui(ui, &mut self.cx),
            Tab::Bom => self.bom_tab.ui(ui, &mut self.cx),
            Tab::BomImport => self.bom_import_tab.ui(ui, &mut self.cx),
            Tab::History => self.history_tab.ui(ui, &mut self.cx),
            Tab::Settings => self.settings_tab.ui(ui, &mut self.cx),
            Tab::Cleanup => self.cleanup_tab.ui(ui, &mut self.cx),
            Tab::Classes => self.classes_tab.ui(ui, &mut self.cx),
            Tab::Health => self.health_tab.ui(ui, &mut self.cx),
            Tab::ProjectCheck => self.project_check_tab.ui(ui, &mut self.cx),
            Tab::Footprint => self.footprint_tab.ui(ui, &mut self.cx),
        }
    }

    fn status(&mut self, ui: &mut Ui) {
        let pal = theme::palette(ui.visuals());
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            let watching = self.cx.settings.watch_dirs.first().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned());
            match watching {
                Some(n) => {
                    dot(ui, Level::Ok.color(ui.visuals()));
                    ui.label(RichText::new(format!("Watching {n} for part ZIPs")).color(pal.muted).size(12.0));
                }
                None => {
                    ui.label(RichText::new("No watch folder").color(pal.muted).size(12.0));
                }
            }
            let waiting = self.import_tab.waiting();
            if waiting > 0 {
                ui.label(RichText::new(format!("{waiting} waiting")).color(pal.muted).size(12.0));
            }
            if self.cx.busy {
                ui.label(RichText::new("Working…").color(pal.muted).size(12.0));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (errors, warns) = (self.cx.log.errors(), self.cx.log.warnings());
                let toggle = if self.log_open { "Hide log" } else { "Show log" };
                if ui.add(egui::Button::new(RichText::new(toggle).size(12.0).color(pal.muted)).frame(false)).clicked() {
                    self.log_open = !self.log_open;
                }
                if let Some((version, url)) = self.update.as_ref().filter(|(_, u)| u.starts_with("https://")) {
                    let r = ui_kit::badge(ui, &format!("Update {version}"), Level::Ok);
                    if ui.interact(r.rect, ui.scope_id().with("update_badge"), Sense::click()).on_hover_text("Open the release page").clicked() {
                        let _ = open::that(url);
                    }
                }
                if errors > 0 {
                    let text = format!("{errors} error{}", if errors == 1 { "" } else { "s" });
                    let r = ui_kit::badge(ui, &text, Level::Error);
                    if ui.interact(r.rect, ui.scope_id().with("error_badge"), Sense::click()).on_hover_text("Show the log").clicked() {
                        self.log_open = true;
                    }
                } else if warns > 0 {
                    ui.label(
                        RichText::new(format!("⚠ {warns} warning{}", if warns == 1 { "" } else { "s" })).size(12.0).color(Level::Warn.color(ui.visuals())),
                    );
                }
            });
        });
    }

    fn log_drawer(&mut self, ui: &mut Ui) {
        let pal = theme::palette(ui.visuals());
        let (all, warns, errors) = (self.cx.log.lines.len(), self.cx.log.warnings(), self.cx.log.errors());
        ui.horizontal(|ui| {
            ui.label(RichText::new("Log").strong().size(14.0));
            for (text, level) in
                [(format!("All {all}"), None), (format!("Warnings {warns}"), Some(Level::Warn)), (format!("Errors {errors}"), Some(Level::Error))]
            {
                if chip(ui, &text, self.omni.log_filter == level).clicked() {
                    self.omni.log_filter = level;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Hide").clicked() {
                    self.log_open = false;
                }
                if ui.small_button("Clear").clicked() {
                    self.cx.log.clear();
                }
                if ui.small_button("Copy").on_hover_text("Copy all log lines to the clipboard").clicked() {
                    ui.ctx().copy_text(self.cx.log.text());
                }
            });
        });
        ui.add_space(2.0);
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
            for (level, line) in self.cx.log.lines.iter().filter(|(l, _)| self.omni.log_filter.is_none_or(|f| f == *l)) {
                let tag = match level {
                    Level::Info => "info",
                    Level::Ok => "ok",
                    Level::Warn => "warn",
                    Level::Error => "error",
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(RichText::new(tag).monospace().size(12.0).color(level.color(ui.visuals())));
                    ui.label(RichText::new(line).monospace().size(12.0).color(if *level == Level::Info { pal.fg } else { level.color(ui.visuals()) }));
                });
            }
        });
    }
}

fn dot(ui: &mut Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
}

/// A bordered "Label  Value ▾" button that opens a menu.
fn chip_menu(ui: &mut Ui, label: &str, value: &str, body: impl FnOnce(&mut Ui)) {
    let pal = theme::palette(ui.visuals());
    let mut job = egui::text::LayoutJob::default();
    let font = |size| egui::FontId::proportional(size);
    job.append(label, 0.0, egui::TextFormat { font_id: font(10.5), color: pal.muted, valign: Align::Center, ..Default::default() });
    job.append(value, 8.0, egui::TextFormat { font_id: font(13.0), color: pal.fg, valign: Align::Center, ..Default::default() });
    ui.menu_button(job, |ui| {
        ui.set_min_width(260.0);
        caption(ui, label);
        ui.add_space(2.0);
        body(ui);
    });
}

/// One rail entry: icon over a small label, a teal bar on the active one.
fn rail_item(ui: &mut Ui, page: Option<Page>, label: &str, active: bool, badge: usize) -> egui::Response {
    let pal = theme::palette(ui.visuals());
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(RAIL_W - 8.0, 46.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &if page.is_some() { format!("Go to {label}") } else { format!("{label}…") }));
    let p = ui.painter();
    if active || resp.hovered() {
        p.rect_filled(rect, 4.0, pal.accent.gamma_multiply(if active { 0.2 } else { 0.08 }));
    }
    if active {
        p.rect_filled(egui::Rect::from_min_size(rect.left_top() + Vec2::new(-4.0, 8.0), Vec2::new(3.0, rect.height() - 16.0)), 2.0, pal.accent);
    }
    let col = if active { pal.fg } else { pal.muted };
    icon(p, page, rect.center_top() + Vec2::new(0.0, 14.0), col);
    p.text(rect.center_bottom() + Vec2::new(0.0, -9.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), col);
    if badge > 0 {
        let at = rect.right_top() + Vec2::new(-9.0, 9.0);
        p.circle_filled(at, 8.0, Level::Warn.color(ui.visuals()));
        p.text(at, egui::Align2::CENTER_CENTER, badge.min(99).to_string(), egui::FontId::proportional(10.0), Color32::from_rgb(0x1b, 0x1d, 0x21));
    }
    resp
}
