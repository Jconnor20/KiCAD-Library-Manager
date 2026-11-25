//! Import, History, BOM and Clean up.

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
    bom_with(size, [quote(1000), quote(1), vec![]])
}

fn bom_with(size: (f32, f32), quotes: [Vec<Quote>; 3]) -> Gui<'static> {
    let mut h = app(size, Tab::Bom);
    let app = h.state_mut();
    let root = app.cx.global_root().unwrap();
    let lib = klm_core::alternatives::scan(&root);
    let store = klm_core::store::Store::open(&klm_core::store::Store::default_path().unwrap()).unwrap();
    for (mpn, maker, role) in [("LM358", "Texas Instruments", "preferred"), ("LM358B", "Texas Instruments", "substitute")] {
        store
            .set_alternative("LM358", &klm_core::store::Alternative { mpn: mpn.into(), manufacturer: maker.into(), supplier_ids: vec![], role: role.into() })
            .unwrap();
    }
    let lines = vec![
        line(&["U1"], "LM358", "Texas Instruments"),
        line(&["U2", "U3"], "RP2040", "Raspberry Pi"),
        line(&["C1", "C2", "C3", "C4", "C5"], "GRM155R71C104KA88D", "Murata"),
    ];
    let project = app.cx.project.clone().unwrap();
    app.bom_tab.seed(&app.cx, project, lines, quotes.to_vec(), lib);
    h.run_ok();
    h
}

fn y(h: &Gui, label: &str) -> f32 {
    h.get_by_label(label).rect().top()
}

fn settings_open(h: &Gui) -> bool {
    h.state().cx.tab == Tab::Settings
}

fn on_page(h: &Gui, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
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
    assert!(settings_open(&h) && on_page(&h, "Folders watched for vendor ZIPs"));
}

fn history(size: (f32, f32)) -> Gui<'static> {
    let mut h = app(size, Tab::History);
    settle(&mut h); // the snapshot list is read on a worker
    h
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
        let (roll, mark, del) =
            (h.get_by_label("Roll back to before this").rect(), h.get_by_label("Mark as version…").rect(), h.get_by_label("Delete…").rect());
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
    settle(&mut h);
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
        settle(&mut h);
        assert!(h.query_all_by_label_contains("CHANGED SINCE THIS SNAPSHOT").next().is_some(), "{}", dump(&h));
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
    assert!(settings_open(&h) && on_page(&h, "Keep the last"));
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
    h.run_ok();
    let store = klm_core::store::Store::open(&klm_core::store::Store::default_path().unwrap()).unwrap();
    assert_eq!(store.recent_searches(1).unwrap(), ["GRM155R71C104KA88D"], "Search didn't run for the MPN");
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
    // The old part kept its in-stock quotes and LM358B has none, so JLC mode marks the old one as better.
    assert!(h.query_by_label("better: LM358 ▾").is_some(), "the old part is now the alternate");
    assert!(h.state().cx.log.lines.iter().any(|(_, s)| s.contains("LM358B in place of LM358")));
    snap(&mut h, "bom-swapped");
}

#[test]
fn b5_region_link() {
    let _env = setup();
    let mut h = bom(SIZES[0]);
    assert!(!settings_open(&h));
    click(&mut h, "Set in Settings…");
    assert!(settings_open(&h) && on_page(&h, "Shipping region"));
}

#[test]
fn c1_cleanup_issues() {
    let env = setup();
    let cfg = klm_core::kicad_env::config_dir().unwrap();
    std::fs::create_dir_all(&cfg).unwrap();
    let gone = env.dir.join("gone");
    let entry = |n: &str, t: &str| {
        format!("(lib (name \"{n}\")(type \"KiCad\")(uri \"{}/{n}\")(options \"\")(descr \"\"))", gone.display().to_string().replace('\\', "/") + t)
    };
    std::fs::write(
        cfg.join("sym-lib-table"),
        format!(
            "(sym_lib_table\n  (version 7)\n  {}\n  {}\n)\n",
            entry("Old_A", "").replace("Old_A\"", "Old_A.kicad_sym\""),
            entry("Old_B", "").replace("Old_B\"", "Old_B.kicad_sym\"")
        ),
    )
    .unwrap();
    for size in SIZES {
        let mut h = app(size, Tab::Cleanup);
        click(&mut h, "Scan KiCad's library tables");
        settle(&mut h);
        assert!(h.query_all_by_label("Broken · 3").next().is_some(), "{}", dump(&h));
        assert!(snap(&mut h, "cleanup-scan").is_empty());
        click(&mut h, "Remove from table");
        assert!(h.query_by_label("Changes to apply").is_some());
        assert!(snap(&mut h, "cleanup-queued").is_empty());
    }
}

/// The Standards pages (Classes, Create footprint and symbol, Library health) open at both sizes and show real data.
#[test]
fn c2_standards_pages() {
    let env = setup();
    // The fixture parts have one pin each, which no house class accepts; a two-pin capacitor gives Classes something to count.
    let prop = |k: &str, v: &str| format!("(property \"{k}\" \"{v}\" (at 0 0 0) (effects (font (size 1.27 1.27))))");
    let pin = |n: &str, x: i32, r: i32| {
        format!("(pin passive line (at {x} 0 {r}) (length 1) (name \"{n}\" (effects (font (size 1 1)))) (number \"{n}\" (effects (font (size 1 1)))))")
    };
    let sym = format!(
        "(kicad_symbol_lib (version 20251024) (generator \"t\") (symbol \"CL05\" {} {} {} {} {} (symbol \"CL05_1_1\" {} {})))",
        prop("Reference", "U"),
        prop("Value", "CL05"),
        prop("Footprint", ""),
        prop("Description", "Capacitor"),
        prop("MPN", "CL05"),
        pin("1", -5, 0),
        pin("2", 5, 180)
    );
    std::fs::write(env.lib.join("symbols/Capacitors.kicad_sym"), sym).unwrap();
    for size in SIZES {
        let mut h = app(size, Tab::Classes);
        settle(&mut h);
        assert!(h.query_by_label("Capacitor").is_some() && h.query_by_label("When new parts come in").is_some(), "{}", dump(&h));
        assert!(h.query_by_label("1 differ").is_some(), "{}", dump(&h));
        assert!(snap(&mut h, "standards-classes").is_empty());

        let mut h = app(size, Tab::Health);
        settle(&mut h);
        assert!(h.query_by_label("Parts checked").is_some() || h.query_by_label("PARTS CHECKED").is_some(), "{}", dump(&h));
        assert!(snap(&mut h, "standards-health").is_empty());

        let mut h = app(size, Tab::Footprint);
        click(&mut h, "Generate");
        assert!(snap(&mut h, "standards-create-footprint").is_empty());
        click(&mut h, "Symbol");
        assert!(snap(&mut h, "standards-create-symbol").is_empty());
    }
}

/// The graphical template editor opens from Classes, draws on a grid, and Save puts the template in the profile.
#[test]
fn c3_template_editor_saves_to_the_profile() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Classes);
    settle(&mut h);
    click(&mut h, "Edit symbol…");
    settle(&mut h);
    assert!(h.query_by_label("Save to standard").is_some() && h.query_by_label("Show pin numbers").is_some(), "{}", dump(&h));
    assert!(snap(&mut h, "standards-template-editor").is_empty());
    let shown = |h: &Gui| h.get_by_label("Show pin numbers").accesskit_node().toggled();
    assert_eq!(shown(&h), Some(eframe::egui::accesskit::Toggled::False));
    click(&mut h, "Show pin numbers");
    click(&mut h, "Save to standard");
    settle(&mut h);
    assert!(h.query_by_label("Save to standard").is_none(), "the editor stayed open");
    assert!(
        h.state().cx.settings.standard.templates.contains_key("Resistor"),
        "nothing stored: {:?}",
        h.state().cx.settings.standard.templates.keys().collect::<Vec<_>>()
    );
    // Editing again starts from what was saved, not from the built-in.
    click(&mut h, "Edit symbol…");
    settle(&mut h);
    assert_eq!(shown(&h), Some(eframe::egui::accesskit::Toggled::True), "the saved template was not reopened");
    click(&mut h, "Reset to default");
    assert_eq!(shown(&h), Some(eframe::egui::accesskit::Toggled::False));
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
                assert!(
                    r.x0 >= -1.0 && r.y0 >= -1.0 && r.x1 <= w as f64 + 1.0 && r.y1 <= hgt as f64 + 1.0,
                    "{what} at {size:?}: {:?} {r:?} is outside the window",
                    a.label()
                );
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

#[test]
fn b6_swap_opens_alternatives_filtered_to_jlcpcb() {
    let _env = setup();
    let q = |stock, jlc: &str, lifecycle: &str| vec![Quote { jlc_type: jlc.into(), lifecycle: lifecycle.into(), ..quote(stock).remove(0) }];
    // LM358 is basic and active; RP2040 is extended and out of stock at JLC; the capacitor has no quote at all.
    let mut h = bom_with(SIZES[1], [q(1000, "basic", "Active"), q(0, "extended", "Obsolete"), vec![]]);
    assert!(on_page(&h, "JLCPCB: 1 basic, 1 extended, 1 out of stock at JLC, 1 obsolete or last-time-buy, 1 not known at JLC."), "{}", dump(&h));
    assert!(h.query_by_label("no stock").is_some() && h.query_by_label("obsolete").is_some() && h.query_by_label("basic").is_some());
    assert_eq!(h.query_all_by_label("Swap").count(), 1, "only the flagged line offers Swap");
    h.get_by_label("Swap").click();
    h.run_ok();
    h.run_ok();
    assert!(h.state().cx.tab == Tab::Library, "{}", dump(&h));
    assert!(h.query_by_label("Alternatives and duplicates").is_some(), "{}", dump(&h));
    h.run_ok();
    let on = |h: &Gui, label: &str| h.get_by_label(label).accesskit_node().toggled() == Some(eframe::egui::accesskit::Toggled::True);
    assert!(on(&h, "Available at JLCPCB") && on(&h, "Same package"), "{}", dump(&h));
}

/// A waiting part shows in the table with its includes and status, the Import button counts it, and the rail badge says one is waiting.
#[test]
fn i4_waiting_part_table_detail_and_badge() {
    let env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Import);
        let part = klm_core::part::Part {
            mpn: "TPS54331DR".into(),
            manufacturer: "Texas Instruments".into(),
            source: "Ultra Librarian ZIP".into(),
            symbol_file: Some(env.dir.join("src/LM358.kicad_sym")),
            ..Default::default()
        };
        let s = h.state_mut();
        s.import_tab.handle(crate::tabs::import::Msg::Found(part, env.dir.join("work")), &mut s.cx);
        settle(&mut h);
        assert!(h.query_all_by_label("TPS54331DR").next().is_some(), "{}", dump(&h));
        assert!(h.query_by_label("Import 1 part").is_some() && h.query_by_label("New").is_some(), "{}", dump(&h));
        assert_eq!(h.state().import_tab.waiting(), 1);
        let m = snap(&mut h, "import-waiting");
        assert!(m.is_empty(), "missing glyphs {m:?}");
    }
}

/// "From a BOM" opens without a file and says what it does.
#[test]
fn i5_bom_import_page() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::BomImport);
        assert!(h.query_by_label("Build a library from a BOM").is_some(), "{}", dump(&h));
        let m = snap(&mut h, "bom-import");
        assert!(m.is_empty(), "missing glyphs {m:?}");
    }
}

/// Looks at the three redesigned pages in the light theme, and the BOM with a line selected (snapshots only).
#[test]
fn w1_light_theme_and_bom_inspector() {
    let _env = setup();
    for size in SIZES {
        let mut h = bom(size);
        h.ctx.set_theme(eframe::egui::ThemePreference::Light);
        h.run_ok();
        click(&mut h, "LM358");
        assert!(snap(&mut h, "bom-selected-light").is_empty());
        let mut h = history(size);
        h.ctx.set_theme(eframe::egui::ThemePreference::Light);
        click(&mut h, "import RP2040");
        assert!(snap(&mut h, "history-light").is_empty());
        let mut h = app(size, Tab::Library);
        h.ctx.set_theme(eframe::egui::ThemePreference::Light);
        h.state().cx.open_settings(crate::tabs::ui_kit::Section::DigiKey);
        h.run_ok();
        assert!(snap(&mut h, "settings-light").is_empty());
    }
}

/// Open a BOM: the resistor already in the library is marked and unticked straight away, with no lookup. Look up the selected lines:
/// only they are looked up (the test point and the resistor are deselected). Generate is available without the lookup.
#[test]
fn i6_bom_lookup_only_selected_lines() {
    let env = setup();
    let bom = env.dir.join("b.csv");
    std::fs::write(
        &bom,
        "Reference,Value,Footprint,MPN,LCSC
R1,10k,R_0603,RC0603FR-0710KL,C25804
C1,100nF,C_0603,,C14663
TP1,TestPoint,TP,,
",
    )
    .unwrap();
    let mut h = app((1280.0, 800.0), Tab::BomImport);
    let s = h.state_mut();
    s.bom_import_tab.open(bom, &mut s.cx);
    h.run_ok();
    assert!(!h.get_by_label("Generate").accesskit_node().is_disabled(), "Generate does its own lookup, so it needs no Look up first");
    assert!(h.query_by_label("In library").is_some(), "the resistor is in the library before any lookup: {}", dump(&h));
    assert!(h.query_by_label("1 line(s) already in your library, so they start unticked. Tick one to import it again.").is_some(), "{}", dump(&h));
    click(&mut h, "Look up selected (1)");
    settle(&mut h);
    settle(&mut h);
    assert!(
        h.query_by_label("0 standard library, 0 from sources, 1 not found, 0 ambiguous. Lines not found or ambiguous are left out.").is_some(),
        "{}",
        dump(&h)
    );
    assert!(h.query_by_label("Generate: none of the ticked lines was found").is_some(), "nothing found, so nothing to generate: {}", dump(&h));
}
