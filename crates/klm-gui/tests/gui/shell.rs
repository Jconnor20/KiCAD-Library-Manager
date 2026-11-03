//! W7.2 app shell: start tab, first-run checklist, error badge, windows per tab, window defaults.

use super::harness::*;
use crate::tabs::ui_kit::Level;
use crate::{App, Tab};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// The app as `main` starts it: no tab forced.
fn fresh<'a>(size: (f32, f32)) -> Gui<'a> {
    let mut h = Harness::builder().with_size(egui::vec2(size.0, size.1)).allow_missing_glyphs().with_accessibility_check(false).wgpu().build_eframe(|cc| App::new(&cc.egui_ctx, None));
    h.run_ok();
    h
}

/// Switches tab directly: a floating window can sit over the tab row and take the click.
pub fn goto(h: &mut Gui, tab: Tab) {
    h.state_mut().cx.tab = tab;
    h.run_ok();
}

/// Settings of a first run: nothing chosen yet.
fn first_run(env: &Env) {
    std::fs::write(env.dir.join("settings.json"), "{}").unwrap();
}

#[test]
fn s1_window_returns_with_its_tab() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    click(&mut h, "Alternatives…");
    goto(&mut h, Tab::History);
    assert!(h.query_by_label("Alternatives and duplicates").is_none());
    goto(&mut h, Tab::Library);
    assert!(h.query_by_label("Alternatives and duplicates").is_some(), "Alternatives did not come back with the Library tab");
}

#[test]
fn s2_window_defaults() {
    let _env = setup();
    let o = crate::native_options();
    assert_eq!(o.viewport.inner_size, Some(egui::vec2(1200.0, 800.0)));
    let path = o.persistence_path.expect("window size is not persisted");
    assert!(path.starts_with(sandbox()), "persistence leaves the sandbox: {}", path.display());
}

#[test]
fn s3_start_tab() {
    let env = setup();
    let h = fresh(SIZES[1]);
    assert!(h.state().cx.tab == Tab::Library, "set-up app should open on Library");
    drop(h);
    first_run(&env);
    let mut h = fresh(SIZES[1]);
    assert!(h.state().cx.tab == Tab::Import, "first run should open on Import");
    assert!(h.query_by_label("Set up:").is_some());
    snap(&mut h, "first-run");
}

#[test]
fn s6_error_badge_opens_log() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    assert!(h.query_by_label("1 error").is_none());
    h.state_mut().cx.log(Level::Info, "fine");
    h.state_mut().cx.log(Level::Error, "boom");
    h.state_mut().log_open = false;
    h.run_ok();
    assert!(h.query_by_label("Copy").is_none(), "log should be hidden");
    click(&mut h, "1 error");
    assert!(h.state().log_open && h.query_by_label("Copy").is_some(), "the badge did not open the log");
    h.state_mut().cx.log(Level::Error, "again");
    h.run_ok();
    assert!(h.query_by_label("2 errors").is_some());
    snap(&mut h, "log-badge");
}

#[test]
fn s7_log_levels_snapshot() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    for (l, t) in [(Level::Info, "info line"), (Level::Ok, "ok line"), (Level::Warn, "warn line"), (Level::Error, "error line")] {
        h.state_mut().cx.log(l, t);
    }
    snap(&mut h, "log-levels");
}

#[test]
fn s8_buttons() {
    let _env = setup();
    for tab in [Tab::Library, Tab::Search, Tab::History] {
        let h = app(SIZES[1], tab);
        assert!(h.query_by_label("Relink libraries").is_none() || h.query_all_by_label("Relink libraries").count() == 1);
        assert!(h.query_by_label("Close project").is_some());
        assert_eq!(h.query_all_by_label("Clear").count(), 1, "only the log has Clear");
    }
}

#[test]
fn s9_first_run_checklist() {
    let env = setup();
    first_run(&env);
    let mut h = fresh(SIZES[1]);
    assert_eq!(h.query_all_by_label("Open").count(), 2, "KiCad and the (default-on) sources are fine; the library and watch folder are open");
    assert!(h.query_by_label("✔ KiCad found").is_some());
    click(&mut h, "Open");
    assert!(h.query_by_label("Settings").is_some(), "Open did not show Settings");
    assert!(h.query_all_by_label_contains("General › Libraries").next().is_some(), "the first Open goes to Libraries");
}

#[test]
fn s10_nothing_clipped_on_any_tab() {
    let env = setup();
    first_run(&env); // the checklist is the tallest the top area gets
    for size in SIZES {
        for tab in [Tab::Search, Tab::Import, Tab::Library, Tab::Bom, Tab::History, Tab::Cleanup] {
            let h = app(size, tab);
            for label in ["Global library", "Project", "Settings…", "Copy"] {
                let r = h.get_all_by_label(label).last().unwrap().rect();
                assert!(r.min.y >= 0.0 && r.max.y <= size.1 && r.max.x <= size.0, "{size:?}: {label:?} is outside the window: {r:?}");
            }
            let tabs = h.get_by_label("Library").rect();
            let log = h.get_all_by_label("Copy").last().unwrap().rect();
            assert!(log.min.y > tabs.max.y + 100.0, "{size:?}: log leaves no room for the tab: {tabs:?} vs {log:?}");
        }
    }
}
