//! W7.0 shared look: glyph coverage, status colours, the table, the log levels and the settings hook.
//! Test IDs (K1…) are the rows of the W7.0 test plan.

use super::harness::*;
use crate::tabs::ui_kit::{self, Col, Level, Section, Sort, Status};
use crate::{Msg, Tab};
use eframe::egui::{self, Rect, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

struct Demo {
    sort: Sort,
    rows: usize,
    drawn: usize,
}

/// A table of `rows` rows, with the kit's fonts, rendered offscreen.
fn demo<'a>(rows: usize, size: Vec2) -> Harness<'a, Demo> {
    let mut h = Harness::builder().with_size(size).allow_missing_glyphs().wgpu().build_ui_state(
        |ui, s: &mut Demo| {
            let cols = [Col::new("Name", 120.0), Col::new("State", 100.0), Col::rest("Notes")];
            s.drawn = 0;
            ui_kit::table(ui, "demo", &cols, &mut s.sort, s.rows, |i, row| {
                s.drawn += 1;
                row.col(|ui| {
                    ui.label(format!("row {i}"));
                });
                row.col(|ui| {
                    ui_kit::status_cell(ui, [Status::Ok, Status::Missing, Status::Broken, Status::Link, Status::Optional][i % 5], "Thing");
                });
                row.col(|ui| {
                    ui.label("note");
                });
            });
        },
        Demo { sort: Sort { col: 1, asc: true }, rows, drawn: 0 },
    );
    crate::install_fonts(&h.ctx);
    h.run_ok();
    h.run_ok();
    h
}

/// The average colour of the "ink" (pixels clearly brighter than the dark panel) inside `rect`.
fn ink(img: &image::RgbaImage, rect: Rect) -> [f32; 3] {
    let (mut sum, mut n) = ([0.0f32; 3], 0.0);
    for y in rect.min.y as u32..rect.max.y.ceil() as u32 {
        for x in rect.min.x as u32..rect.max.x.ceil() as u32 {
            let p = img.get_pixel(x, y).0;
            if p[..3].iter().map(|c| *c as u32).max().unwrap() > 70 {
                (0..3).for_each(|i| sum[i] += p[i] as f32);
                n += 1.0;
            }
        }
    }
    assert!(n > 0.0, "nothing drawn in {rect:?}");
    sum.map(|s| s / n)
}

fn ink_of(h: &mut Harness<Demo>, label: &str) -> [f32; 3] {
    let rect = h.get_all_by_label(label).next().unwrap_or_else(|| panic!("no {label:?} on screen")).rect();
    ink(&h.render().unwrap(), rect)
}

#[test]
fn k1_no_missing_glyphs_on_any_tab() {
    let _env = setup();
    for size in SIZES {
        for (tab, name) in [
            (Tab::Search, "search"),
            (Tab::Import, "import"),
            (Tab::Library, "library"),
            (Tab::Bom, "bom"),
            (Tab::History, "history"),
            (Tab::Cleanup, "cleanup"),
        ] {
            let mut h = app(size, tab);
            if name == "search" {
                with_probe(&mut h);
            }
            let m = snap(&mut h, &format!("tab-{name}"));
            assert!(m.is_empty(), "{name} at {size:?} draws {m:?} as empty boxes");
        }
        let mut h = app(size, Tab::Library);
        click(&mut h, "Settings…");
        let m = snap(&mut h, "settings");
        assert!(m.is_empty(), "Settings at {size:?} draws {m:?} as empty boxes");
    }
}

#[test]
fn k1b_every_glyph_in_the_source_is_covered() {
    let _env = setup();
    let h = app(SIZES[0], Tab::Library);
    let src: String =
        ["main.rs", "tabs/search.rs", "tabs/library.rs", "tabs/ui_kit.rs", "tabs/settings.rs", "tabs/preview.rs", "tabs/alternatives.rs", "tabs/import.rs"]
            .iter()
            .map(|f| std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(f)).unwrap())
            .collect();
    let missing: std::collections::BTreeSet<char> = h.ctx.fonts_mut(|f| {
        let id = egui::FontId::proportional(14.0);
        src.chars().filter(|c| !c.is_ascii() && !f.has_glyph(&id, *c)).collect()
    });
    assert!(missing.is_empty(), "the source uses {missing:?}, which no font draws");
}

#[test]
fn k3_library_ok_cell_is_green() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    settle(&mut h);
    let rect = h.get_all_by_label("✔").next().expect("no ✔ cell in the Library table").rect();
    let [r, g, b] = ink(&h.render().unwrap(), rect);
    assert!(g > r + 30.0 && g > b + 20.0, "✔ is not green: rgb({r:.0}, {g:.0}, {b:.0})");
}

#[test]
fn k4_status_cells() {
    let mut h = demo(5, Vec2::new(420.0, 220.0));
    let [r, g, b] = ink_of(&mut h, "✔");
    assert!(g > r + 30.0 && g > b + 20.0, "ok is not green: {:?}", [r, g, b]);
    let [r, g, b] = ink_of(&mut h, "missing");
    assert!(r > b + 60.0 && r > g, "missing is not warn-coloured: {:?}", [r, g, b]);
    let [r, g, b] = ink_of(&mut h, "broken");
    assert!(r > g + 60.0 && r > b + 60.0, "broken is not red: {:?}", [r, g, b]);
    let [r, g, b] = ink_of(&mut h, "link");
    assert!(b > r + 40.0, "link is not info-blue: {:?}", [r, g, b]);
    let [r, _, b] = ink_of(&mut h, "—");
    assert!((r - b).abs() < 25.0, "optional is not grey");
    h.render().unwrap().save(snap_dir().join("kit-status-cells.png")).unwrap();
}

fn luminance(c: egui::Color32) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

#[test]
fn k6_level_colours_are_distinct_and_readable_on_both_themes() {
    let levels = [Level::Info, Level::Ok, Level::Warn, Level::Error];
    for v in [egui::Visuals::dark(), egui::Visuals::light()] {
        let bg = luminance(v.panel_fill);
        for (i, a) in levels.iter().enumerate() {
            let c = a.color(&v);
            let (hi, lo) = if luminance(c) > bg { (luminance(c), bg) } else { (bg, luminance(c)) };
            assert!((hi + 0.05) / (lo + 0.05) >= 3.0, "{a:?} on dark={}: contrast too low", v.dark_mode);
            for b in &levels[i + 1..] {
                assert_ne!(c, b.color(&v), "{a:?} and {b:?} share a colour");
            }
        }
    }
}

#[test]
fn k7_sortable_header() {
    let mut h = demo(5, Vec2::new(420.0, 220.0));
    h.get_by_label("State ▲").hover();
    h.get_by_label("Name").click();
    h.run_ok();
    assert_eq!(h.state().sort, Sort { col: 0, asc: true });
    h.get_by_label("Name ▲").click();
    h.run_ok();
    assert_eq!(h.state().sort, Sort { col: 0, asc: false });
    assert!(h.query_by_label("Name ▼").is_some() && h.query_by_label("State ▲").is_none());
    h.render().unwrap().save(snap_dir().join("kit-table.png")).unwrap();
}

#[test]
fn k8_table_lays_out_only_visible_rows() {
    let mut h = demo(2000, Vec2::new(420.0, 400.0));
    let drawn = h.state().drawn;
    assert!((10..=30).contains(&drawn), "{drawn} of 2000 rows were laid out in a 400 px table");
    assert!(h.query_by_label("row 0").is_some() && h.query_by_label("row 1999").is_none());
    h.render().unwrap().save(snap_dir().join("kit-table-2000.png")).unwrap();
}

#[test]
fn k9_header_stays_while_scrolling_and_columns_resize() {
    let mut h = demo(2000, Vec2::new(420.0, 300.0));
    let header = h.get_by_label("Name").rect();
    h.hover_at(egui::pos2(200.0, 150.0));
    for _ in 0..5 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -100.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_ok();
    }
    assert!(h.query_by_label("row 0").is_none(), "the body did not scroll");
    assert_eq!(h.get_by_label("Name").rect(), header, "the header moved with the body");

    // Drag the border between "Name" and "State" 40 px to the right.
    let border = egui::pos2(h.get_by_label("State ▲").rect().min.x - 4.0, header.center().y);
    let before = h.get_by_label("State ▲").rect().min.x;
    h.hover_at(border);
    h.run_ok();
    h.drag_at(border);
    h.run_ok();
    h.hover_at(border + egui::vec2(40.0, 0.0));
    h.run_ok();
    h.drop_at(border + egui::vec2(40.0, 0.0));
    h.run_ok();
    let after = h.get_by_label("State ▲").rect().min.x;
    assert!(after > before + 20.0, "column did not widen: {before} -> {after}");
}

#[test]
fn k10_log_levels() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    let (lines, errors) = (h.state().cx.log.lines.len(), h.state().cx.log.errors());
    let cx = &mut h.state_mut().cx;
    cx.log(Level::Warn, "careful");
    cx.log(Level::Error, "explicit error");
    cx.log.push("Could not save settings: disk full".into());
    cx.log.push("Added LM358".into());
    cx.log.extend(["Search failed: timeout".to_string()]);
    cx.send(Msg::LogAt(Level::Ok, "all good".into()));
    h.run_ok();
    let log = &h.state().cx.log;
    assert_eq!(log.lines.len(), lines + 6);
    assert_eq!(log.errors(), errors + 3, "explicit error + two legacy lines that read as errors");
    let level = |t: &str| log.lines.iter().find(|(_, s)| s == t).unwrap().0;
    assert_eq!((level("careful"), level("all good"), level("Added LM358")), (Level::Warn, Level::Ok, Level::Info));
}

#[test]
fn k11_log_is_coloured_by_level() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    h.state_mut().cx.log.lines.clear();
    for (l, t) in [(Level::Info, "info line"), (Level::Ok, "ok line"), (Level::Warn, "warn line"), (Level::Error, "error line")] {
        h.state_mut().cx.log(l, t);
    }
    settle(&mut h);
    let img = h.render().unwrap();
    img.save(snap_dir().join("kit-log-levels.png")).unwrap();
    let g = |t: &str| ink(&img, h.get_by_label(t).rect());
    let (ok, warn, err) = (g("ok line"), g("warn line"), g("error line"));
    assert!(ok[1] > ok[0] + 30.0 && warn[0] > warn[2] + 60.0 && err[0] > err[1] + 60.0, "ok {ok:?} warn {warn:?} error {err:?}");
}

#[test]
fn k12_open_settings_hook() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    assert!(h.state().cx.tab != Tab::Settings);
    h.state().cx.open_settings(Section::General);
    h.run_ok();
    assert!(h.state().cx.tab == Tab::Settings && h.query_by_label("General › Libraries").is_some(), "Settings did not open");
}

/// Every non-ASCII symbol the GUI draws, on one sheet for eyeballing (kit-glyphs.png).
#[test]
fn k2_symbol_sheet_has_no_tofu() {
    let sheet = "✔ ✘ ✕ ★ ☆ ▲ ▼ ⇅ 📌 📄 🔣 ⏚ 🧊 👁 → … · — € £ ×";
    let mut h = Harness::builder().with_size(Vec2::new(420.0, 60.0)).allow_missing_glyphs().wgpu().build_ui(|ui| {
        ui.label(egui::RichText::new(sheet).size(20.0));
    });
    crate::install_fonts(&h.ctx);
    h.run_ok();
    h.run_ok();
    let missing: Vec<char> = h.ctx.fonts_mut(|f| sheet.chars().filter(|c| *c != ' ' && !f.has_glyph(&egui::FontId::proportional(20.0), *c)).collect());
    assert!(missing.is_empty(), "no glyph for {missing:?}");
    h.render().unwrap().save(snap_dir().join("kit-glyphs.png")).unwrap();
}
