//! The look from the KLM Wireframes canvas: **Bench** (dark, neutral grey) and **Datasheet** (light, white),
//! teal accent, lined surfaces, 4 px corners. Colours are the canvas's tokens (`--bg`, `--panel`, `--rail`, `--line`…).
//!
//! `install` sets both themes once at start-up; egui picks by the OS theme (or the user's choice) after that.
//! Fonts: IBM Plex Sans for UI text and JetBrains Mono for data, if the system has them (else egui's built-ins).

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Theme};
use std::sync::Arc;

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// One theme's colours, named as in the wireframes.
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub rail: Color32,
    pub line: Color32,
    pub row: Color32,
    pub fg: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub field: Color32,
}

pub const BENCH: Palette = Palette {
    bg: rgb(0x1b1d21),
    panel: rgb(0x22252a),
    rail: rgb(0x16181b),
    line: rgb(0x33373e),
    row: rgb(0x26292e),
    fg: rgb(0xe3e5e8),
    muted: rgb(0x9ba1aa),
    accent: rgb(0x3cc2b4),
    field: rgb(0x22252a),
};

pub const DATASHEET: Palette = Palette {
    bg: rgb(0xffffff),
    panel: rgb(0xf6f7f8),
    rail: rgb(0xeceef0),
    line: rgb(0xd5d9de),
    row: rgb(0xeef0f2),
    fg: rgb(0x1d2024),
    muted: rgb(0x5c636d),
    accent: rgb(0x0a7f75),
    field: rgb(0xffffff),
};

pub fn palette(v: &egui::Visuals) -> &'static Palette {
    if v.dark_mode { &BENCH } else { &DATASHEET }
}

fn visuals(p: &Palette, dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r = CornerRadius::same(4);
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = r;
    v.extreme_bg_color = p.field; // text fields, plot backgrounds
    v.faint_bg_color = p.row; // striped rows
    v.code_bg_color = p.row;
    v.override_text_color = None;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = if dark { rgb(0xe0a94a) } else { rgb(0x9a6500) };
    v.error_fg_color = if dark { rgb(0xef6b6b) } else { rgb(0xb42c2c) };
    v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.22 } else { 0.18 });
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.striped = false; // lined, not tonal
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.fg);
    w.inactive.bg_fill = p.field;
    w.inactive.weak_bg_fill = p.field;
    w.inactive.bg_stroke = Stroke::new(1.0, p.line);
    w.inactive.fg_stroke = Stroke::new(1.0, p.fg);
    w.hovered.bg_fill = p.row;
    w.hovered.weak_bg_fill = p.row;
    w.hovered.bg_stroke = Stroke::new(1.0, p.muted);
    w.hovered.fg_stroke = Stroke::new(1.0, p.fg);
    w.active.bg_fill = p.accent.gamma_multiply(0.25);
    w.active.weak_bg_fill = p.accent.gamma_multiply(0.25);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.fg);
    w.open.bg_fill = p.row;
    w.open.weak_bg_fill = p.row;
    w.open.bg_stroke = Stroke::new(1.0, p.line);
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = r;
    }
    v
}

/// First readable font file from `paths`, if any.
fn first_font(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

/// Adds Plex Sans and JetBrains Mono from the system (nothing is bundled). Returns which were found.
fn add_fonts(fonts: &mut FontDefinitions) {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let ui = first_font(&[
        r"C:\Windows\Fonts\IBMPlexSans-Regular.ttf",
        &format!(r"{local}\Microsoft\Windows\Fonts\IBMPlexSans-Regular.ttf"),
        "/usr/share/fonts/truetype/ibm-plex/IBMPlexSans-Regular.ttf",
        "/Library/Fonts/IBMPlexSans-Regular.ttf",
    ]);
    let mono = first_font(&[
        r"C:\Windows\Fonts\JetBrainsMono-Regular.ttf",
        &format!(r"{local}\Microsoft\Windows\Fonts\JetBrainsMono-Regular.ttf"),
        "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
        "/Library/Fonts/JetBrainsMono-Regular.ttf",
    ]);
    for (name, bytes, family) in [("plex", ui, FontFamily::Proportional), ("jbmono", mono, FontFamily::Monospace)] {
        if let Some(b) = bytes {
            fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(b)));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
}

/// Call once, after the symbol fonts are installed (they sit behind these in each family).
pub fn install(ctx: &egui::Context, mut fonts: FontDefinitions) {
    add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    ctx.set_visuals_of(Theme::Dark, visuals(&BENCH, true));
    ctx.set_visuals_of(Theme::Light, visuals(&DATASHEET, false));
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |s| {
            s.text_styles = [
                (TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
                (TextStyle::Small, FontId::new(10.0, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
            ]
            .into();
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(10.0, 5.0);
            s.spacing.interact_size.y = 28.0;
        });
    }
}
