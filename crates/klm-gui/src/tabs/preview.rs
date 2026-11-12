//! The preview widget: symbol, footprint and 3D thumbnails side by side with the part's check warnings.
//! Shared by the import dialog (a part's files before import) and the Library tab (a part in a library).
//!
//! Renders run on threads (`klm_core::preview` takes seconds); the panel polls for results each frame.

use eframe::egui::{self, Color32, Sense, Vec2};
use klm_core::checks::{self, Issue, Severity};
use klm_core::library::LibraryRoot;
use klm_core::part::Part;
use klm_core::preview::{self, Kind, Sources};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

/// A thumbnail's size when there is room (the Library window); a narrower host such as the import dialog gets smaller ones.
pub const THUMB: Vec2 = Vec2::new(240.0, 180.0);
const ZOOM: Vec2 = Vec2::new(THUMB.x * 3.0 + 16.0, 300.0);

const KINDS: [(Kind, &str); 3] = [(Kind::Symbol, "Symbol"), (Kind::Footprint, "Footprint"), (Kind::Model, "3D")];

enum Slot {
    Loading,
    Image(PathBuf),
    /// Why there is no picture, in a sentence.
    Failed(String),
}

enum Msg {
    Image(usize, Result<PathBuf, String>),
    /// The symbols in the file, and the one drawn first.
    Symbols(Vec<String>, Option<String>),
}

pub struct PreviewPanel {
    ctx: egui::Context,
    src: Sources,
    slots: [Slot; 3],
    /// `None` while the checks run.
    issues: Option<Vec<Issue>>,
    zoomed: Option<usize>,
    /// Every symbol in the file when there is more than one, and the one shown.
    symbols: Vec<String>,
    symbol: Option<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    issues_rx: Receiver<Vec<Issue>>,
}

impl PreviewPanel {
    /// A part's downloaded files, before import. Build it again after any of the files change.
    pub fn for_part(ctx: &egui::Context, part: &Part) -> Self {
        let src = Sources::from_part(part);
        let files = src.clone();
        Self::start(ctx, src, Some(part.mpn.clone()), move || match (&files.symbol, &files.footprint) {
            (Some(s), Some(f)) => checks::check_files(s, f, files.model.as_deref()),
            _ => vec![],
        })
    }

    pub fn for_library(ctx: &egui::Context, root: &LibraryRoot, category: &str, name: &str) -> Self {
        let (root, category, name) = (root.clone(), category.to_string(), name.to_string());
        Self::start(ctx, Sources::from_library(&root, &category, &name), None, move || checks::check_part(&root, &category, &name))
    }

    /// `mpn` picks which symbol of a multi-symbol file is drawn first, as the import does; `None` when the symbol is already named.
    fn start(ctx: &egui::Context, src: Sources, mpn: Option<String>, check: impl FnOnce() -> Vec<Issue> + Send + 'static) -> Self {
        egui_extras::install_image_loaders(ctx); // idempotent
        let (tx, rx) = channel();
        for (i, (kind, _)) in KINDS.iter().enumerate() {
            let (tx, mut src, ctx, mpn) = (tx.clone(), src.clone(), ctx.clone(), mpn.clone());
            let kind = *kind;
            std::thread::spawn(move || {
                if let (Kind::Symbol, Some(file), Some(mpn), None) = (kind, &src.symbol, &mpn, &src.symbol_name) {
                    if let Ok(names) = preview::symbol_names(file) {
                        src.symbol_name = names.iter().find(|n| n.eq_ignore_ascii_case(mpn)).or(names.first()).cloned();
                        let _ = tx.send(Msg::Symbols(names, src.symbol_name.clone()));
                    }
                }
                let _ = tx.send(Msg::Image(i, preview::render(kind, &src).map_err(|e| format!("{e:#}"))));
                ctx.request_repaint();
            });
        }
        let (itx, issues_rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = itx.send(check());
            repaint.request_repaint();
        });
        Self { ctx: ctx.clone(), src, slots: [Slot::Loading, Slot::Loading, Slot::Loading], issues: None, zoomed: None, symbols: vec![], symbol: None, tx, rx, issues_rx }
    }

    /// Draws another symbol of the file.
    fn pick_symbol(&mut self, name: String) {
        self.slots[0] = Slot::Loading;
        let (tx, ctx, mut src) = (self.tx.clone(), self.ctx.clone(), self.src.clone());
        src.symbol_name = Some(name.clone());
        self.symbol = Some(name);
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Image(0, preview::render(Kind::Symbol, &src).map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// The panel in a window of its own; returns false once the user closes it.
    pub fn window(&mut self, ctx: &egui::Context, title: &str) -> bool {
        let mut open = true;
        egui::Window::new(title).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.set_width(ZOOM.x);
            // The zoomed picture must not push the window off a small screen.
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 90.0).max(200.0)).show(ui, |ui| self.show(ui));
        });
        open
    }

    /// Takes what the render and check threads have produced.
    fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Image(i, result) => {
                    self.slots[i] = match result {
                        Ok(path) => Slot::Image(path),
                        Err(reason) => Slot::Failed(reason),
                    }
                }
                Msg::Symbols(names, first) => (self.symbols, self.symbol) = (names, first),
            }
        }
        if let Ok(issues) = self.issues_rx.try_recv() {
            self.issues = Some(issues);
        }
    }

    /// The Library inspector's strip: Symbol / Footprint / 3D in equal labelled boxes, then the check warnings.
    /// True when a picture was clicked (the caller opens the big preview).
    pub fn inspector(&mut self, ui: &mut egui::Ui) -> bool {
        let clicked = self.pictures(ui);
        self.issues_list(ui);
        clicked
    }

    /// Just the three boxes of [`inspector`](Self::inspector).
    pub fn pictures(&mut self, ui: &mut egui::Ui) -> bool {
        self.poll();
        let gap = ui.spacing().item_spacing.x.min(6.0);
        let w = ((ui.available_width() - 2.0 * gap) / 3.0).floor().max(60.0);
        let size = Vec2::new(w, 120.0);
        let mut clicked = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (i, (kind, label)) in KINDS.iter().enumerate() {
                clicked |= slot(ui, &self.slots[i], *kind, size, Some(label));
            }
        });
        clicked
    }

    /// The check warnings, or "Checking…" while they run.
    pub fn issues_list(&self, ui: &mut egui::Ui) {
        match &self.issues {
            None => {
                ui.weak("Checking…");
            }
            Some(issues) => {
                for issue in issues {
                    let color = if issue.severity == Severity::Error { ui.visuals().error_fg_color } else { ui.visuals().warn_fg_color };
                    ui.colored_label(color, &issue.message);
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();
        let w = ((ui.available_width() - 2.0 * ui.spacing().item_spacing.x) / 3.0).clamp(140.0, THUMB.x).floor();
        let thumb = Vec2::new(w, (w * 0.75).floor());
        ui.horizontal(|ui| {
            for (i, (kind, label)) in KINDS.iter().enumerate() {
                ui.vertical(|ui| {
                    let mut picked = None;
                    if i == 0 && self.symbols.len() > 1 {
                        let shown = self.symbol.clone().unwrap_or_default();
                        egui::ComboBox::from_id_salt("symbol_pick").selected_text(shown).width(thumb.x).show_ui(ui, |ui| {
                            for name in &self.symbols {
                                if ui.selectable_label(self.symbol.as_ref() == Some(name), name).clicked() {
                                    picked = Some(name.clone());
                                }
                            }
                        });
                    } else {
                        ui.weak(*label);
                    }
                    if let Some(name) = picked.filter(|n| self.symbol.as_ref() != Some(n)) {
                        self.pick_symbol(name);
                    }
                    if slot(ui, &self.slots[i], *kind, thumb, None) {
                        self.zoomed = if self.zoomed == Some(i) { None } else { Some(i) };
                    }
                });
            }
        });
        if let Some(i) = self.zoomed {
            ui.add_space(4.0);
            if slot(ui, &self.slots[i], KINDS[i].0, Vec2::new(ui.available_width().min(ZOOM.x), ZOOM.y), None) {
                self.zoomed = None;
            }
        }
        self.issues_list(ui);
    }
}

/// One picture in a frame; true if it was clicked. The symbol is drawn for a light background and the
/// footprint for a dark one, as in KiCad. `label` is drawn in the top-left corner.
fn slot(ui: &mut egui::Ui, slot: &Slot, kind: Kind, size: Vec2, label: Option<&str>) -> bool {
    let bg = match kind {
        Kind::Symbol => Color32::from_rgb(0xF5, 0xF4, 0xEF),
        _ => Color32::from_rgb(0x0B, 0x10, 0x20),
    };
    let mut clicked = false;
    egui::Frame::new().fill(bg).corner_radius(4).show(ui, |ui| {
        ui.set_min_size(size);
        ui.set_max_size(size);
        if let Some(l) = label {
            let ink = Color32::from_rgb(0x70, 0x76, 0x80);
            ui.painter().text(ui.max_rect().min + Vec2::new(6.0, 5.0), egui::Align2::LEFT_TOP, l.to_uppercase(), egui::FontId::monospace(10.0), ink);
        }
        match slot {
            Slot::Loading => {
                ui.centered_and_justified(|ui| ui.spinner());
            }
            Slot::Failed(reason) => {
                let short: String = reason.chars().take(90).collect();
                ui.label(egui::RichText::new(short).small().color(Color32::GRAY));
            }
            Slot::Image(path) => {
                // The file loader reads "file:///C:/x" as a Windows path, but "file://C:/x" as a network share.
                let p = path.to_string_lossy().replace('\\', "/");
                let uri = if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") };
                let image = egui::Image::new(uri).fit_to_exact_size(size).sense(Sense::click());
                let r = ui.add(image);
                clicked = r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
            }
        }
    });
    clicked
}
