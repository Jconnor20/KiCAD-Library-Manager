//! Search tab. Test IDs (S1…) are the ones used in this file.
//! Results are handed to the tab as if a source had answered (`Msg::Done`), so nothing reaches the network.

use super::harness::*;
use crate::Tab;
use crate::tabs::search::Msg;
use egui_kittest::kittest::Queryable;
use klm_core::part::Part;
use klm_core::store::Store;

fn part(mpn: &str, lcsc: bool) -> Part {
    Part {
        mpn: mpn.into(),
        manufacturer: "Texas Instruments".into(),
        description: "Dual operational amplifier, 1 MHz, 3 V to 32 V supply, SOIC-8".into(),
        package: "SOIC-8".into(),
        stock: Some(12000),
        unit_price: "$0.0123 @100".into(),
        price_currency: "USD".into(),
        supplier_ids: if lcsc { vec![("LCSC".into(), "C7950".into())] } else { vec![] },
        ..Default::default()
    }
}

/// The app on Search with the Probe source, showing these results.
fn with_results<'a>(size: (f32, f32), mpns: &[(&str, bool)]) -> Gui<'a> {
    let mut h = app(size, Tab::Search);
    with_probe(&mut h);
    let s = h.state_mut();
    s.search_tab.handle(Msg::Done(mpns.iter().map(|(m, l)| (0, part(m, *l))).collect()), &mut s.cx);
    settle(&mut h);
    h
}

const THREE: [(&str, bool); 3] = [("LM358", true), ("NE555", true), ("TL072", false)];

#[test]
fn s1_header_is_not_cut_off() {
    let _env = setup();
    for size in SIZES {
        let h = with_results(size, &THREE);
        for title in ["Source", "MPN", "Manufacturer", "Description", "Package", "Stock", "Price (as listed)", "CAD"] {
            // The detail pane repeats a few of these words (Package, ...) so check every match.
            for n in h.get_all_by_label(title) {
                let r = n.rect();
                assert!(r.max.x <= size.0, "{title} runs off the right edge at {size:?}: {r:?}");
            }
        }
        let mut h = h;
        let m = snap(&mut h, "search-results");
        assert!(m.is_empty(), "missing glyphs {m:?}");
    }
}

#[test]
fn s2_empty_state_before_first_search() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Search);
        with_probe(&mut h);
        assert!(h.query_by_label("Search for a part").is_some());
        snap(&mut h, "search-empty");
        let s = h.state_mut();
        s.search_tab.handle(Msg::Done(vec![]), &mut s.cx);
        h.run_ok();
        assert!(h.query_by_label("Search for a part").is_none());
        assert!(h.query_by_label_contains("No results").is_some());
    }
}

#[test]
fn s3_in_library_badge_and_open_in_library() {
    let _env = setup();
    let mut h = with_results(SIZES[1], &THREE);
    assert_eq!(h.query_all_by_label("In library").count(), 1, "only LM358 is in the seeded library");
    snap(&mut h, "search-in-library");
    h.get_by_label("In library").click();
    h.run_ok();
    assert!(h.state().cx.tab == Tab::Library);
    settle(&mut h);
    assert!(
        h.query_by_label("LM358").is_some() && h.query_by_label("RP2040").is_none(),
        "Library isn't filtered to LM358:
{}",
        dump(&h)
    );
    snap(&mut h, "library-opened-from-search");
}

#[test]
fn s4_save_and_unsave_a_recent_search() {
    let _env = setup();
    let store = Store::open(&Store::default_path().unwrap()).unwrap();
    store.log("search", "lm358").unwrap();
    let mut h = app(SIZES[1], Tab::Search);
    with_probe(&mut h);
    click(&mut h, "Saved & recent ▾");
    assert!(h.query_by_label_contains("Click ☆").is_some(), "empty Saved list explains itself");
    snap(&mut h, "search-side-before-save");
    h.get_all_by_label("☆").next().unwrap().click();
    h.run_ok();
    assert_eq!(store.saved_searches().unwrap(), ["lm358"]);
    assert!(h.query_by_label_contains("Click ☆").is_none());
    snap(&mut h, "search-side-saved");
    h.get_all_by_label("★").next().unwrap().click();
    h.run_ok();
    assert!(store.saved_searches().unwrap().is_empty());
}

#[test]
fn s5_cad_availability_is_text() {
    let _env = setup();
    let mut h = with_results(SIZES[1], &THREE);
    assert_eq!(h.query_all_by_label("SYM").count(), 2);
    assert_eq!(h.query_all_by_label("FP").count(), 2);
    assert_eq!(h.query_all_by_label("3D").count(), 2);
    assert!(h.query_by_label_contains("not known").is_some());
    assert!(h.query_by_label_contains("🔣").is_none());
    snap(&mut h, "search-cad");
}

#[test]
fn s6_no_sources_links_to_settings() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Search);
        snap(&mut h, "search-no-sources");
        assert!(h.state().cx.tab == Tab::Search);
        h.get_by_label("Open Settings · Sources").click();
        h.run_ok();
        assert!(h.state().cx.tab == Tab::Settings, "Settings did not open");
        assert!(h.query_all_by_label_contains("LCSC needs no keys.").next().is_some(), "Settings isn't on Sources");
    }
}

#[test]
fn s7_sortable_columns() {
    let _env = setup();
    let mut h = with_results(SIZES[1], &THREE);
    h.get_by_label("MPN").click();
    h.run_ok();
    assert!(h.query_by_label("MPN ▲").is_some());
    h.get_by_label("MPN ▲").click();
    h.run_ok();
    assert!(h.query_by_label("MPN ▼").is_some());
}

#[test]
fn s8_recently_imported_icons_work() {
    let _env = setup();
    let store = Store::open(&Store::default_path().unwrap()).unwrap();
    let mut h = app(SIZES[1], Tab::Search);
    with_probe(&mut h);
    click(&mut h, "Saved & recent ▾");
    assert!(!store.favourites().unwrap().contains(&"LM358".to_string()));
    // Newest first: RC0603FR-0710KL, RP2040 (already starred), LM358; the first ☆ is the newest import.
    h.get_all_by_label("☆").next().unwrap().click();
    h.run_ok();
    assert!(store.favourites().unwrap().contains(&"RC0603FR-0710KL".to_string()));
}
