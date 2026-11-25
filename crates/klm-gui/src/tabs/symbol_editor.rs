//! A small symbol editor: the pin table, the body rectangle and the Reference/Value text positions, next to
//! the part's preview. The edits themselves live in `klm_core::symbols::edit`; this is the form.
//! Anything graphical beyond that is done in KiCad's own editor.

use super::batch_dialog;
use super::preview::PreviewPanel;
use super::ui_kit::{self, Level};
use eframe::egui::{self, RichText};
use klm_core::library::LibraryRoot;
use klm_core::sexpr::Sexpr;
use klm_core::symbols::edit::{self, GRID, PIN_TYPES, PinRow};
use klm_core::{rules::Rules, standardise::profile::Profile, symbols};

/// What the editor wants after a frame.
pub enum Outcome {
    None,
    Close,
    /// The symbol was written; the Library tab should reload so its badges update.
    Saved,
}

const SIDES: [(f64, &str); 4] = [(0.0, "Left"), (180.0, "Right"), (270.0, "Top"), (90.0, "Bottom")];

pub struct SymbolEditor {
    root: LibraryRoot,
    category: String,
    name: String,
    /// The symbol as it is on disk (to detect outside changes), and the one the edits are applied to.
    was: Sexpr,
    base: Sexpr,
    /// (pin number before editing, or empty for a new pin; the row).
    pins: Vec<(String, PinRow)>,
    body: Option<[f64; 4]>,
    texts: Vec<(&'static str, [f64; 3])>,
    preview: PreviewPanel,
    message: Option<(bool, String)>,
}

impl SymbolEditor {
    pub fn new(ctx: &egui::Context, root: &LibraryRoot, category: &str, name: &str) -> anyhow::Result<Self> {
        let was = edit::load(root, category, name)?;
        let mut me = Self {
            root: root.clone(),
            category: category.into(),
            name: name.into(),
            base: was.clone(),
            was,
            pins: vec![],
            body: None,
            texts: vec![],
            preview: PreviewPanel::for_library(ctx, root, category, name),
            message: None,
        };
        me.load_fields();
        Ok(me)
    }

    fn load_fields(&mut self) {
        self.pins = edit::pins(&self.base).into_iter().map(|p| (p.number.clone(), p)).collect();
        self.body = edit::body(&self.base);
        self.texts = ["Reference", "Value"].into_iter().filter_map(|k| Some((k, edit::text_at(&self.base, k)?))).collect();
    }

    /// `base` with the form's edits applied.
    fn edited(&self) -> anyhow::Result<Sexpr> {
        let mut s = self.base.clone();
        for old in edit::pins(&self.base) {
            if !self.pins.iter().any(|(n, _)| *n == old.number) {
                edit::remove_pin(&mut s, &old.number)?;
            }
        }
        for (orig, row) in &self.pins {
            if orig.is_empty() { edit::add_pin(&mut s, row.clone())? } else { edit::set_pin(&mut s, orig, row.clone())? }
        }
        if let Some(b) = self.body {
            edit::set_body(&mut s, b)?;
        }
        for (key, at) in &self.texts {
            edit::set_text_at(&mut s, key, *at)?;
        }
        Ok(s)
    }

    fn save(&mut self, ctx: &egui::Context) -> bool {
        match self.edited().and_then(|new| edit::save(&self.root, &self.category, &self.name, &self.was, new)) {
            Ok(()) => match edit::load(&self.root, &self.category, &self.name) {
                Ok(now) => {
                    (self.was, self.base) = (now.clone(), now);
                    self.load_fields();
                    // A fresh panel re-renders the symbol and re-runs the checks.
                    self.preview = PreviewPanel::for_library(ctx, &self.root, &self.category, &self.name);
                    self.message = Some((true, "Saved. History can undo it.".into()));
                    true
                }
                Err(e) => {
                    self.message = Some((false, format!("{e:#}")));
                    false
                }
            },
            Err(e) => {
                self.message = Some((false, format!("{e:#}")));
                false
            }
        }
    }

    /// Redraws the working copy in the house style (not saved until Save).
    fn house_style(&mut self, naming: &Rules, profile: &Profile) {
        let r = self.edited().and_then(|s| {
            let class = symbols::class_of(&self.category, &s, naming)?;
            Ok((class, symbols::restyle(&s, class, profile)?))
        });
        self.message = Some(match r {
            Ok((class, new)) => {
                self.base = new;
                self.load_fields();
                (true, format!("Redrawn as {class:?}; Save to keep it."))
            }
            Err(e) => (false, format!("House style not applied: {e:#}")),
        });
    }

    pub fn show(&mut self, ctx: &egui::Context, naming: &Rules, profile: &Profile) -> Outcome {
        let (mut out, mut restyle, mut save) = (Outcome::None, false, false);
        let dirty = self.edited().map(|s| s != self.was).unwrap_or(true);
        let message = self.message.clone();
        let sub = format!("Symbol · {}", self.category);
        let n_pins = self.pins.len();
        let closed = batch_dialog::dialog(
            ctx,
            egui::Id::NULL.with("symbol_editor"),
            940.0,
            &self.name.clone(),
            Some(&sub),
            |ui| {
                ui.horizontal_top(|ui| {
                    let side = 300.0;
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - side - 24.0).max(300.0));
                        ui.horizontal(|ui| {
                            ui_kit::section_label(ui, &format!("Pins · {n_pins}"));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Add pin").clicked() {
                                    let n = (1..).map(|n| n.to_string()).find(|n| self.pins.iter().all(|(_, p)| p.number != *n)).unwrap_or_default();
                                    self.pins.push((
                                        String::new(),
                                        PinRow { number: n, name: "~".into(), etype: "passive".into(), x: 0.0, y: 0.0, rot: 0.0, length: 2.54 },
                                    ));
                                }
                            });
                        });
                        self.pin_table(ui);
                    });
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.set_width(side);
                        self.side_pane(ui, &mut restyle);
                    });
                });
            },
            |ui| {
                ui.horizontal(|ui| {
                    if dirty {
                        ui_kit::badge(ui, "Unsaved changes", Level::Warn);
                    }
                    if let Some((ok, text)) = &message {
                        let color = if *ok { ui.visuals().weak_text_color() } else { ui.visuals().error_fg_color };
                        ui.label(RichText::new(text).color(color));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        save = ui.add(ui_kit::primary("Save", ui.visuals())).clicked();
                        if ui.button("Close").clicked() {
                            out = Outcome::Close;
                        }
                    });
                });
            },
        );
        if save && self.save(ctx) {
            out = Outcome::Saved;
        }
        if restyle {
            self.house_style(naming, profile);
        }
        if closed {
            out = Outcome::Close;
        }
        out
    }

    /// One row per pin, edited in place.
    fn pin_table(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        egui::ScrollArea::both().max_height((ui.ctx().content_rect().height() - 260.0).max(200.0)).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("pin_table").striped(true).spacing([8.0, 4.0]).show(ui, |ui| {
                for h in ["No.", "Name", "Type", "Side", "x", "y", "Length", ""] {
                    ui.label(RichText::new(h.to_uppercase()).monospace().size(11.0).weak());
                }
                ui.end_row();
                for (i, (_, p)) in self.pins.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut p.number).desired_width(40.0));
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(90.0));
                    egui::ComboBox::from_id_salt(("pin_type", i)).selected_text(p.etype.clone()).show_ui(ui, |ui| {
                        for t in PIN_TYPES {
                            ui.selectable_value(&mut p.etype, t.to_string(), t);
                        }
                    });
                    let side = SIDES.iter().find(|s| s.0 == p.rot).map_or("Other", |s| s.1);
                    egui::ComboBox::from_id_salt(("pin_side", i)).selected_text(side).width(60.0).show_ui(ui, |ui| {
                        for (rot, label) in SIDES {
                            ui.selectable_value(&mut p.rot, rot, label);
                        }
                    });
                    ui.add(egui::DragValue::new(&mut p.x).speed(GRID));
                    ui.add(egui::DragValue::new(&mut p.y).speed(GRID));
                    ui.add(egui::DragValue::new(&mut p.length).speed(GRID).range(GRID..=50.0));
                    if ui.add(ui_kit::ghost("✕")).on_hover_text("Remove this pin").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(i) = remove {
            self.pins.remove(i);
        }
    }

    /// The picture, the body rectangle, the Reference/Value positions and the house style.
    fn side_pane(&mut self, ui: &mut egui::Ui, restyle: &mut bool) {
        ui_kit::section_label(ui, "Symbol");
        ui.add_space(2.0);
        self.preview.inspector(ui);
        ui.add_space(10.0);
        if let Some(b) = &mut self.body {
            ui_kit::section_label(ui, "Body");
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                for (i, label) in ["x1", "y1", "x2", "y2"].iter().enumerate() {
                    ui.weak(*label);
                    ui.add(egui::DragValue::new(&mut b[i]).speed(GRID));
                }
            });
            ui.add_space(10.0);
        }
        ui_kit::section_label(ui, "Text positions");
        ui.add_space(2.0);
        for (key, at) in &mut self.texts {
            ui.horizontal(|ui| {
                ui.label(*key);
                ui.weak("x");
                ui.add(egui::DragValue::new(&mut at[0]).speed(GRID));
                ui.weak("y");
                ui.add(egui::DragValue::new(&mut at[1]).speed(GRID));
            });
        }
        ui.add_space(10.0);
        ui_kit::section_label(ui, "House style");
        ui.add_space(2.0);
        ui.weak(format!("Positions snap to the {GRID} mm (50 mil) grid when you save. For anything graphical, use Edit in KiCad."));
        if ui
            .button("Apply house style")
            .on_hover_text("Redraw in the house style of its kind of part, like Restyle symbol. Keeps pins' numbers, names and types.")
            .clicked()
        {
            *restyle = true;
        }
    }
}
