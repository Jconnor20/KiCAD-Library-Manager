//! The UI bugs found by hand after Wave 5 (Feature_Roadmap.md, "Review after Wave 5"), one test each.
//! A bug that is still open is `#[should_panic(expected = "KNOWN BUG")]`: the test passes while the bug exists
//! and fails with "did not panic" the day it is fixed, which is the cue to drop the attribute.

use super::harness::*;
use crate::Tab;
use eframe::egui::accesskit::Role;
use egui_kittest::kittest::Queryable;

macro_rules! bug {
    ($cond:expr, $($msg:tt)+) => {
        assert!($cond, "KNOWN BUG {}", format!($($msg)+))
    };
}

fn select(h: &mut Gui, mpn: &str) {
    h.get_by_label(mpn).click();
    h.run_ok();
}

#[test]
#[should_panic(expected = "KNOWN BUG")]
fn b1_settings_key_fields_are_at_least_300px_wide() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Library);
        click(&mut h, "Settings…");
        let widths: Vec<f32> = h.query_all_by_role(Role::PasswordInput).map(|n| n.rect().width()).collect();
        assert!(!widths.is_empty(), "no key fields found");
        bug!(widths.iter().all(|w| *w >= 300.0), "W7: key field widths at {size:?}: {widths:?}");
    }
}

#[test]
fn b2_alternatives_window_stays_on_its_tab() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    click(&mut h, "Alternatives…");
    assert!(h.query_by_label("Alternatives and duplicates").is_some(), "Alternatives did not open");
    super::shell::goto(&mut h, Tab::History);
    assert!(h.query_by_label("Alternatives and duplicates").is_none(), "Alternatives window is still shown on the History tab");
}

#[test]
fn b2_preview_window_stays_on_its_tab() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    select(&mut h, "RP2040");
    click(&mut h, "Preview");
    assert!(h.query_by_label("Preview: RP2040").is_some(), "Preview did not open");
    super::shell::goto(&mut h, Tab::History);
    assert!(h.query_by_label("Preview: RP2040").is_none(), "Preview window is still shown on the History tab");
}

/// Everything the Search tab draws that matters must sit inside the window, and the log may take 25% at most.
fn assert_nothing_clipped(h: &Gui, size: (f32, f32), tab: &str) {
    let inside = |label: &str| {
        let r = h.get_all_by_label(label).last().unwrap().rect();
        assert!(r.min.y >= 0.0 && r.max.y <= size.1 && r.max.x <= size.0, "{tab} at {size:?}: {label:?} is outside the window: {r:?}");
    };
    let log_top = h.get_all_by_label("Copy").last().unwrap().rect().min.y;
    let log_share = (size.1 - log_top) / size.1;
    assert!(log_share <= 0.25, "{tab} at {size:?}: the log panel takes {:.0}% of the height (limit 25%)", log_share * 100.0);
    match tab {
        "Search" => {
            inside("Browse by category");
            inside("Search");
        }
        _ => inside("Refresh"),
    }
}

#[test]
fn b3_search_header_is_not_clipped_at_both_sizes() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Search);
        with_probe(&mut h);
        let search = h.get_all_by_label("Search").last().unwrap().rect();
        let top = h.get_by_label("Choose project…").rect().max.y;
        assert!(search.min.y > top && search.max.y <= size.1, "Search box misplaced at {size:?}: {search:?}");
        assert_nothing_clipped(&h, size, "Search");
    }
}

#[test]
fn b3_library_header_is_not_clipped_at_both_sizes() {
    let _env = setup();
    for size in SIZES {
        let h = app(size, Tab::Library);
        assert_nothing_clipped(&h, size, "Library");
        let table = h.get_by_label("RP2040").rect();
        assert!(table.max.y <= size.1 * 0.75, "first table row hidden behind the log at {size:?}: {table:?}");
    }
}

#[test]
fn b4_search_sidebar_icons_have_glyphs() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Search);
        with_probe(&mut h);
        settle(&mut h);
        assert!(h.query_by_label("Recently imported").is_some(), "side list missing");
        let missing = missing_glyphs(&h);
        assert!(missing.is_empty(), " Search at {size:?} draws {missing:?} as empty boxes (star buttons, sort arrows)");
    }
}

#[test]
fn b4b_library_table_has_no_tofu() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    settle(&mut h);
    let missing = missing_glyphs(&h);
    assert!(missing.is_empty(), " the Library table draws {missing:?} as empty boxes (the ✔ cells, sort arrows)");
}

#[test]
fn b4c_settings_has_no_tofu() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    click(&mut h, "Settings…");
    let missing = missing_glyphs(&h);
    assert!(missing.is_empty(), " Settings draws {missing:?} as an empty box (the show/hide-key button)");
}

#[test]
#[should_panic(expected = "KNOWN BUG")]
fn b5_alternatives_opens_on_the_selected_part() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    select(&mut h, "RC0603FR-0710KL");
    click(&mut h, "Alternatives…");
    settle(&mut h);
    let shown = h.get_by_role_and_label(Role::ComboBox, "For part").value().unwrap_or_default();
    // Control: the box is readable, and today it holds the first part.
    assert!(shown.contains("LM358"), "cannot read the 'For part' box: {shown:?}");
    bug!(shown.contains("RC0603FR-0710KL"), "W7: Alternatives' 'For part' box shows {shown:?}, not the selected RC0603FR-0710KL");
}

#[test]
#[should_panic(expected = "KNOWN BUG")]
fn b6_missing_sim_model_is_not_flagged_like_a_real_gap() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    settle(&mut h);
    let col = h.get_by_label("Sim model").rect().min.x;
    let flagged = h.query_all_by_label("missing").filter(|n| (n.rect().min.x - col).abs() < 3.0).count();
    bug!(flagged == 0, "W7: {flagged} rows show a 'missing' sim model, though sim models are optional (drawn orange like a real gap)");
}

#[test]
#[should_panic(expected = "lack accessible name")]
fn b7_every_widget_has_an_accessible_name() {
    let _env = setup();
    let mut h = app_checked(SIZES[1], Tab::Library, true);
    settle(&mut h);
    h.run_ok();
}

#[test]
fn no_distributor_was_called() {
    // Runs last by name only if the others ran first; it also holds on its own: the counter only moves on a source call.
    assert_eq!(SOURCE_CALLS.load(std::sync::atomic::Ordering::SeqCst), 0, "a test reached a distributor");
}

