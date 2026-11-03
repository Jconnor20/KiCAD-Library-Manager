//! W7.5: Import, History, BOM and Clean up (see docs/ui-review/W7.5.md).

use super::harness::*;
use crate::Tab;
use eframe::egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use klm_core::bom::{Line, Quote};

fn line(refs: &[&str], mpn: &str, maker: &str) -> Line {
    Line { refs: refs.iter().map(|r| r.to_string()).collect(), value: mpn.into(), mpn: mpn.into(), manufacturer: maker.into(), ..Default::default() }
}

fn quote(stock: u64) -> Vec<Quote> {
    vec![Quote { source: "LCSC".into(), id: "C1".into(), stock: Some(stock), breaks: vec![(1, 0.5)], currency: "USD".into(), moq: 1, ..Default::default() }]
}

/// A BOM tab with three lines: LM358 (in the library, plenty of stock, one alternate), RP2040 (in the library, stock 1
/// for a need of 2) and a capacitor the library lacks, with no price.
fn bom(size: (f32, f32)) -> Gui<'static> {
    let mut h = app(size, Tab::Bom);
    let app = h.state_mut();
    let root = app.cx.global_root().unwrap();
    let lib = klm_core::alternatives::scan(&root);
    let store = klm_core::store::Store::open(&klm_core::store::Store::default_path().unwrap()).unwrap();
    for (mpn, maker, role) in [("LM358", "Texas Instruments", "preferred"), ("LM358B", "Texas Instruments", "substitute")] {
        store.set_alternative("LM358", &klm_core::store::Alternative { mpn: mpn.into(), manufacturer: maker.into(), supplier_ids: vec![], role: role.into() }).unwrap();
    }
    let lines = vec![line(&["U1"], "LM358", "Texas Instruments"), line(&["U2", "U3"], "RP2040", "Raspberry Pi"), line(&["C1", "C2", "C3", "C4", "C5"], "GRM155R71C104KA88D", "Murata")];
    let project = app.cx.project.clone().unwrap();
    app.bom_tab.seed(&app.cx, project, lines, vec![quote(1000), quote(1), vec![]], lib);
    h.run_ok();
    h
}

fn y(h: &Gui, label: &str) -> f32 {
    h.get_by_label(label).rect().top()
}

fn settings_open(h: &Gui) -> bool {
    h.query_by_label("Settings").is_some()
}

#[test]
fn i1_import_buttons() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Import);
        assert!(h.query_by_label("Import folder…").is_some() && h.query_by_label("Import files…").is_some());
        assert!(h.query_all_by_role(Role::CheckBox).next().is_none(), "the 'files' checkbox is gone");
        assert!(snap(&mut h, "import").is_empty());
    }
}

#[test]
fn i2_folder_and_files_both_reach_the_review() {
    let env = setup();
    let src = env.dir.join("src");
    for picked in [vec![src.clone()], vec![src.join("LM358.kicad_sym"), src.join("RP2040.kicad_sym")]] {
        let mut h = app(SIZES[1], Tab::Import);
        let app = h.state_mut();
        app.import_tab.import_library(picked, &mut app.cx);
        settle(&mut h);
        assert!(h.query_by_label("This will change:").is_some(), "no review dialog:\n{}", dump(&h));
    }
}

#[test]
fn i3_watch_folder_link_opens_settings() {
    let _env = setup();
    let mut h = app(SIZES[0], Tab::Import);
    assert!(!settings_open(&h));
    click(&mut h, "Watch folders…");
    assert!(settings_open(&h));
}

fn history(size: (f32, f32)) -> Gui<'static> {
    app(size, Tab::History)
}

fn menu_buttons<'a>(h: &'a Gui) -> Vec<egui_kittest::Node<'a>> {
    h.query_all_by_label("⋯").collect()
}

fn snapshots(h: &Gui) -> usize {
    klm_core::snapshot::list(&h.state().cx.global_root().unwrap()).len()
}

#[test]
fn h1_row_menu() {
    let _env = setup();
    for size in SIZES {
        let mut h = history(size);
        assert!(h.query_by_label("Delete").is_none() && h.query_by_label("Roll back to before this").is_none(), "no action buttons in the rows");
        assert_eq!(menu_buttons(&h).len(), 3);
        menu_buttons(&h)[0].click();
        h.run_ok();
        let (roll, mark, del) = (h.get_by_label("Roll back to before this").rect(), h.get_by_label("Mark as version…").rect(), h.get_by_label("Delete…").rect());
        assert!(roll.top() < mark.top() && mark.top() < del.top(), "Delete is last");
        assert!(del.top() - mark.bottom() > 4.0 && del.top() - mark.bottom() > mark.top() - roll.bottom(), "a separator sits above Delete");
        assert!(snap(&mut h, "history-row-menu").is_empty());
    }
}

#[test]
fn h2_delete_is_confirmed() {
    let _env = setup();
    let mut h = history(SIZES[1]);
    let before = snapshots(&h);
    menu_buttons(&h)[0].click();
    h.run_ok();
    click(&mut h, "Delete…");
    assert!(h.query_by_label("Delete this snapshot?").is_some());
    assert_eq!(snapshots(&h), before, "nothing is deleted before the confirmation");
    snap(&mut h, "history-delete-confirm");
    click(&mut h, "Cancel");
    assert_eq!(snapshots(&h), before);
    menu_buttons(&h)[0].click();
    h.run_ok();
    click(&mut h, "Delete…");
    click(&mut h, "Delete");
    assert_eq!(snapshots(&h), before - 1);
}

#[test]
fn h3_rollback_asks_first() {
    let _env = setup();
    let mut h = history(SIZES[1]);
    menu_buttons(&h)[1].click();
    h.run_ok();
    click(&mut h, "Roll back to before this");
    assert!(h.query_by_label("Roll back?").is_some());
    let before = snapshots(&h);
    click(&mut h, "Cancel");
    assert!(h.query_by_label("Roll back?").is_none());
    assert_eq!(snapshots(&h), before);
}

#[test]
fn h4_what_changed() {
    let _env = setup();
    for size in SIZES {
        let mut h = history(size);
        assert!(h.query_by_label("Click a row to see which files that change touched.").is_some());
        click(&mut h, "import RP2040");
        assert!(h.query_all_by_label_contains("What \"import RP2040\" changed").next().is_some(), "{}", dump(&h));
        assert!(h.query_all_by_label("added").next().is_some(), "the files the import added are listed");
        assert!(snap(&mut h, "history-what-changed").is_empty());
    }
}

#[test]
fn h5_retention_link() {
    let _env = setup();
    let mut h = history(SIZES[0]);
    assert!(h.query_by_label("Retention…").is_none());
    click(&mut h, "Retention settings…");
    assert!(settings_open(&h));
}

#[test]
fn b1_sort() {
    let _env = setup();
    let mut h = bom(SIZES[1]);
    assert!(y(&h, "LM358") < y(&h, "GRM155R71C104KA88D"), "schematic order before sorting");
    click(&mut h, "Qty");
    assert!(h.query_by_label("Qty ▲").is_some());
    assert!(y(&h, "LM358") < y(&h, "RP2040") && y(&h, "RP2040") < y(&h, "GRM155R71C104KA88D"));
    click(&mut h, "Qty ▲");
    assert!(h.query_by_label("Qty ▼").is_some());
    assert!(y(&h, "GRM155R71C104KA88D") < y(&h, "RP2040") && y(&h, "RP2040") < y(&h, "LM358"));
    snap(&mut h, "bom-sorted");
}

#[test]
fn b2_filter_and_shortfalls() {
    let _env = setup();
    let mut h = bom(SIZES[1]);
    snap(&mut h, "bom");
    assert_eq!(h.query_all_by_label("Shortfalls only").count(), 1);
    click(&mut h, "Shortfalls only");
    assert!(h.query_by_label("LM358").is_none(), "LM358 is in the library with enough stock");
    assert!(h.query_by_label("RP2040").is_some() && h.query_by_label("GRM155R71C104KA88D").is_some(), "low stock and missing parts stay");
    snap(&mut h, "bom-shortfalls");
    click(&mut h, "Shortfalls only");
    assert!(h.query_by_label("LM358").is_some());
    h.query_all_by_role(Role::TextInput).last().unwrap().click();
    h.run_ok();
    h.query_all_by_role(Role::TextInput).last().unwrap().type_text("grm");
    h.run_ok();
    assert!(h.query_by_label("GRM155R71C104KA88D").is_some() && h.query_by_label("RP2040").is_none() && h.query_by_label("LM358").is_none(), "{}", dump(&h));
    h.query_all_by_role(Role::TextInput).last().unwrap().type_text("zzz");
    h.run_ok();
    assert!(h.query_by_label("No line matches.").is_some());
}

#[test]
fn b3_missing_part_actions() {
    let _env = setup();
    let below_tabs = |h: &Gui, label: &str| h.query_all_by_label(label).filter(|n| n.rect().top() > 150.0).count();
    let mut h = bom(SIZES[1]);
    assert_eq!(below_tabs(&h, "Search"), 1, "only the missing capacitor offers Search");
    assert_eq!(below_tabs(&h, "Import"), 1);
    h.query_all_by_label("Search").find(|n| n.rect().top() > 150.0).unwrap().click();
    h.run_ok();
    assert!(h.state().cx.tab == Tab::Search);
    assert!(h.state().cx.log.lines.iter().any(|(_, s)| s.contains("GRM155R71C104KA88D") && s.contains("clipboard")));
    let mut h = bom(SIZES[1]);
    h.query_all_by_label("Import").find(|n| n.rect().top() > 150.0).unwrap().click();
    h.run_ok();
    assert!(h.state().cx.tab == Tab::Import, "{}", dump(&h));
}

#[test]
fn b4_swap_alternate() {
    let _env = setup();
    let mut h = bom(SIZES[1]);
    click(&mut h, "LM358B ▾");
    click(&mut h, "LM358B (substitute)");
    assert!(h.query_by_label("LM358B").is_some() && h.query_by_label("LM358").is_none(), "{}", dump(&h));
    assert!(h.query_by_label("close to LM358").is_some(), "the swapped part is matched against the library again");
    assert!(h.query_by_label("LM358 ▾").is_some(), "the old part is now the alternate");
    assert!(h.state().cx.log.lines.iter().any(|(_, s)| s.contains("LM358B in place of LM358")));
    snap(&mut h, "bom-swapped");
}

#[test]
fn b5_region_link() {
    let _env = setup();
    let mut h = bom(SIZES[0]);
    assert!(!settings_open(&h));
    click(&mut h, "Set in Settings…");
    assert!(settings_open(&h));
}

#[test]
fn c1_cleanup_issues() {
    let env = setup();
    let cfg = klm_core::kicad_env::config_dir().unwrap();
    std::fs::create_dir_all(&cfg).unwrap();
    let gone = env.dir.join("gone");
    let entry = |n: &str, t: &str| format!("(lib (name \"{n}\")(type \"KiCad\")(uri \"{}/{n}\")(options \"\")(descr \"\"))", gone.display().to_string().replace('\\', "/") + t);
    std::fs::write(cfg.join("sym-lib-table"), format!("(sym_lib_table\n  (version 7)\n  {}\n  {}\n)\n", entry("Old_A", "").replace("Old_A\"", "Old_A.kicad_sym\""), entry("Old_B", "").replace("Old_B\"", "Old_B.kicad_sym\""))).unwrap();
    for size in SIZES {
        let mut h = app(size, Tab::Cleanup);
        click(&mut h, "Scan KiCad's library tables");
        assert!(h.query_all_by_label("Broken").next().is_some(), "{}", dump(&h));
        assert!(snap(&mut h, "cleanup-scan").is_empty());
        click(&mut h, "Remove");
        assert!(h.query_by_label("Changes to apply").is_some());
        assert!(snap(&mut h, "cleanup-queued").is_empty());
    }
}

/// Every labelled widget is inside the window, and no tab shows a glyph the fonts lack.
#[test]
fn x1_nothing_clipped_or_missing() {
    let _env = setup();
    for size in SIZES {
        let (w, hgt) = size;
        let inside = |h: &Gui, what: &str| {
            for n in h.root().children_recursive() {
                let a = n.accesskit_node();
                let Some(r) = a.bounding_box().filter(|r| r.width() > 0.0 && a.label().is_some()) else { continue };
                assert!(r.x0 >= -1.0 && r.y0 >= -1.0 && r.x1 <= w as f64 + 1.0 && r.y1 <= hgt as f64 + 1.0, "{what} at {size:?}: {:?} {r:?} is outside the window", a.label());
            }
        };
        let mut h = app(size, Tab::Import);
        settle(&mut h);
        inside(&h, "import");
        let mut h = history(size);
        click(&mut h, "import RP2040");
        menu_buttons(&h)[0].click();
        h.run_ok();
        inside(&h, "history");
        assert!(missing_glyphs(&h).is_empty());
        let mut h = bom(size);
        inside(&h, "bom");
        assert!(snap(&mut h, "bom").is_empty());
    }
}
