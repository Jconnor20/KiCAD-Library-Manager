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
use std::sync::mpsc::{Receiver, channel};

const KINDS: [(Kind, &str); 3] = [(Kind::Symbol, "Symbol"), (Kind::Footprint, "Footprint"), (Kind::Model, "3D")];

enum Slot {
    Loading,
    Image(PathBuf),
    /// Why there is no picture, in a sentence.
    Failed(String),
}

pub struct PreviewPanel {
    slots: [Slot; 3],
    /// `None` while the checks run.
    issues: Option<Vec<Issue>>,
    zoomed: Option<usize>,
    rx: Receiver<(usize, Result<PathBuf, String>)>,
    issues_rx: Receiver<Vec<Issue>>,
}

impl PreviewPanel {
    /// A part's downloaded files, before import. Build it again after any of the files change.
    pub fn for_part(ctx: &egui::Context, part: &Part) -> Self {
        let src = Sources::from_part(part);
        let files = src.clone();
        Self::start(ctx, src, move || match (&files.symbol, &files.footprint) {
            (Some(s), Some(f)) => checks::check_files(s, f, files.model.as_deref()),
            _ => vec![],
        })
    }

    pub fn for_library(ctx: &egui::Context, root: &LibraryRoot, category: &str, name: &str) -> Self {
        let (root, category, name) = (root.clone(), category.to_string(), name.to_string());
        Self::start(ctx, Sources::from_library(&root, &category, &name), move || checks::check_part(&root, &category, &name))
    }

    fn start(ctx: &egui::Context, src: Sources, check: impl FnOnce() -> Vec<Issue> + Send + 'static) -> Self {
        egui_extras::install_image_loaders(ctx); // idempotent
        let (tx, rx) = channel();
        for (i, (kind, _)) in KINDS.iter().enumerate() {
            let (tx, src, ctx) = (tx.clone(), src.clone(), ctx.clone());
            let kind = *kind;
            std::thread::spawn(move || {
                let _ = tx.send((i, preview::render(kind, &src).map_err(|e| format!("{e:#}"))));
                ctx.request_repaint();
            });
        }
        let (itx, issues_rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = itx.send(check());
            ctx.request_repaint();
        });
        Self { slots: [Slot::Loading, Slot::Loading, Slot::Loading], issues: None, zoomed: None, rx, issues_rx }
    }

    /// The panel in a window of its own; returns false once the user closes it.
    pub fn window(&mut self, ctx: &egui::Context, title: &str) -> bool {
        let mut open = true;
        egui::Window::new(title).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.set_width(460.0);
            self.show(ui);
        });
        open
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        while let Ok((i, result)) = self.rx.try_recv() {
            self.slots[i] = match result {
                Ok(path) => Slot::Image(path),
                Err(reason) => Slot::Failed(reason),
            };
        }
        if let Ok(issues) = self.issues_rx.try_recv() {
            self.issues = Some(issues);
        }
        ui.horizontal(|ui| {
            for (i, (kind, label)) in KINDS.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.weak(*label);
                    if slot(ui, &self.slots[i], *kind, Vec2::new(140.0, 105.0)) {
                        self.zoomed = if self.zoomed == Some(i) { None } else { Some(i) };
                    }
                });
            }
        });
        if let Some(i) = self.zoomed {
            ui.add_space(4.0);
            if slot(ui, &self.slots[i], KINDS[i].0, Vec2::new(440.0, 300.0)) {
                self.zoomed = None;
            }
        }
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
}

/// One picture in a frame; true if it was clicked. The symbol is drawn for a light background and the
/// footprint for a dark one, as in KiCad.
fn slot(ui: &mut egui::Ui, slot: &Slot, kind: Kind, size: Vec2) -> bool {
    let bg = match kind {
        Kind::Symbol => Color32::from_rgb(0xF5, 0xF4, 0xEF),
        _ => Color32::from_rgb(0x0B, 0x10, 0x20),
    };
    let mut clicked = false;
    egui::Frame::new().fill(bg).show(ui, |ui| {
        ui.set_min_size(size);
        ui.set_max_size(size);
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
