//! Footprint tab: an IPC-7351B land pattern from datasheet dimensions (see `klm_core::footprint_gen`), written into a library.

use crate::AppCtx;
use crate::tabs::ui_kit::Level;
use eframe::egui;
use klm_core::footprint_gen::{self, Dims, Family, Generated, Range};

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
        }
    }
}

fn num(ui: &mut egui::Ui, label: &str, v: &mut f64) {
    ui.label(label);
    ui.add(egui::DragValue::new(v).speed(0.01).range(0.0..=100.0).max_decimals(3));
}

fn range(ui: &mut egui::Ui, label: &str, r: &mut Range) {
    num(ui, &format!("{label} min"), &mut r.min);
    num(ui, "max", &mut r.max);
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
        let f = self.family;
        egui::Grid::new("fpgen").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Family");
            egui::ComboBox::from_id_salt("fpgen_family").selected_text(f.label().to_uppercase()).show_ui(ui, |ui| {
                for x in Family::ALL {
                    ui.selectable_value(&mut self.family, x, x.label().to_uppercase());
                }
            });
            ui.label("Density");
            egui::ComboBox::from_id_salt("fpgen_density").selected_text(["A: most", "B: nominal", "C: least"][(self.density as u8 - b'A') as usize]).show_ui(ui, |ui| {
                for (c, t) in [('A', "A: most"), ('B', "B: nominal"), ('C', "C: least")] {
                    ui.selectable_value(&mut self.density, c, t);
                }
            });
            ui.end_row();
            if f == Family::Bga {
                for (l, v) in [("Rows", &mut self.dims.rows), ("Columns", &mut self.dims.cols)] {
                    ui.label(l);
                    ui.add(egui::DragValue::new(v).range(1..=99));
                }
                ui.end_row();
                num(ui, "Pitch", &mut self.dims.pitch);
                num(ui, "Ball Ø", &mut self.dims.ball);
                ui.end_row();
            } else {
                if !matches!(f, Family::Chip | Family::Sod) {
                    ui.label("Pins");
                    ui.add(egui::DragValue::new(&mut self.dims.pins).range(2..=400));
                    num(ui, "Pitch", &mut self.dims.pitch);
                    ui.end_row();
                }
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
            num(ui, "Body Y", &mut self.body.1);
            ui.end_row();
            if f == Family::Qfn {
                num(ui, "Exposed pad X", &mut self.thermal.0);
                num(ui, "Y", &mut self.thermal.1);
                ui.end_row();
            }
        });
        ui.weak("Datasheet dimensions in mm. Body size is needed for all but chip and QFN; exposed pad 0 = none. Lines, text and courtyard follow the active profile.");
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Category");
            ui.add(egui::TextEdit::singleline(&mut self.category).hint_text("e.g. ICs").desired_width(140.0));
            ui.add_enabled(cx.project_root().is_some(), egui::Checkbox::new(&mut self.project, "Project library"));
            ui.checkbox(&mut self.overwrite, "Replace if it exists");
        });
        ui.horizontal(|ui| {
            if ui.button("Generate").clicked() {
                self.result = Some(self.build(cx));
            }
            let root = if self.project { cx.project_root() } else { cx.global_root() };
            let ready = matches!(self.result, Some(Ok(_))) && root.is_some() && !self.category.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Save to library")).clicked() {
                if let (Some(Ok(g)), Some(root)) = (&self.result, root) {
                    match footprint_gen::save(&root, self.category.trim(), g, self.overwrite) {
                        Ok(p) => cx.log(Level::Info, format!("Wrote {}", p.display())),
                        Err(e) => cx.log(Level::Error, format!("Could not write the footprint: {e:#}")),
                    }
                }
            }
        });
        match &self.result {
            Some(Ok(g)) => {
                ui.strong(&g.name);
                for fix in &g.fixes {
                    ui.weak(format!("Profile: {fix}"));
                }
            }
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            None => {}
        }
    }
}
