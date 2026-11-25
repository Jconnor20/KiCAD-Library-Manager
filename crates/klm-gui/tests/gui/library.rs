//! W7.3 Library tab: selection, loading off the UI thread, status-cell fixes and the toolbar's dry-run dialogs.
//! Test IDs (L1…) are the rows of the W7.3 test plan.

use super::harness::*;
use crate::Tab;
use crate::tabs::library::TEST_PICK;
use eframe::egui::Modifiers;
use eframe::egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

fn library<'a>(size: (f32, f32)) -> Gui<'a> {
    let mut h = app(size, Tab::Library);
    settle(&mut h);
    h
}

fn tick(h: &mut Gui, mpn: &str, mods: Modifiers) {
    h.get_by_role_and_label(Role::CheckBox, &format!("Select {mpn}")).click_modifiers(mods);
    h.run_ok();
}

/// "N selected" from the selection bar; 0 when the bar isn't there.
fn selected(h: &Gui) -> usize {
    // The Standardise button also says "N selected", but starts with a word.
    h.query_all_by_label_contains(" selected")
        .find_map(|n| {
            let a = n.accesskit_node();
            let text = a.label().or_else(|| a.value()).unwrap_or_default().to_string();
            text.split(' ').next().and_then(|n| n.parse().ok())
        })
        .unwrap_or(0)
}

/// Every file under `dir` with its bytes, to prove a dry run wrote nothing.
fn files(dir: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                out.insert(p.clone(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// Opens the header's More menu and clicks `item`.
fn more(h: &mut Gui, item: &str) {
    click(h, "More");
    click(h, item);
}

fn log_has(h: &Gui, needle: &str) -> bool {
    h.state().cx.log.lines.iter().any(|(_, s)| s.contains(needle))
}

#[test]
fn l1_select_all_and_none() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    assert_eq!(selected(&h), 0);
    click(&mut h, "Select all");
    assert_eq!(selected(&h), 3);
    click(&mut h, "Select none");
    assert_eq!(selected(&h), 0);
}

#[test]
fn l2_shift_click_selects_a_range() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    // On screen: LM358, RP2040 (ICs), RC0603FR-0710KL (Resistors).
    tick(&mut h, "LM358", Modifiers::NONE);
    assert_eq!(selected(&h), 1);
    tick(&mut h, "RC0603FR-0710KL", Modifiers::SHIFT);
    assert_eq!(selected(&h), 3, "shift-click should tick the whole range");
    tick(&mut h, "RP2040", Modifiers::SHIFT);
    assert_eq!(selected(&h), 2, "the range runs from the same anchor to the new row");
    // Ctrl-click on a row toggles that row only.
    // (The detail panel repeats the current part's name; the table row is the last match.)
    h.get_all_by_label("RC0603FR-0710KL").last().unwrap().click_modifiers(Modifiers::COMMAND);
    h.run_ok();
    assert_eq!(selected(&h), 3);
    h.get_all_by_label("RC0603FR-0710KL").last().unwrap().click_modifiers(Modifiers::COMMAND);
    h.run_ok();
    assert_eq!(selected(&h), 2);
}

#[test]
fn l3_range_follows_the_order_shown() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    click(&mut h, "MPN");
    // By MPN: LM358, RC0603FR-0710KL, RP2040. In category order the same two rows would take RP2040 with them.
    tick(&mut h, "LM358", Modifiers::NONE);
    tick(&mut h, "RC0603FR-0710KL", Modifiers::SHIFT);
    assert_eq!(selected(&h), 2);
    // Filtering drops the ticks on rows that are no longer shown.
    let filter = h.query_all_by_role(Role::TextInput).nth(1).expect("no filter box"); // [0] is the Ctrl+K box
    filter.focus();
    filter.type_text("RC06");
    h.run_ok();
    assert_eq!(selected(&h), 1, "a hidden row must not stay selected");
}

#[test]
fn l5_two_thousand_parts_stay_responsive() {
    let env = setup();
    let mut sym = String::from("(kicad_symbol_lib (version 20211014) (generator test)\n");
    for i in 0..2000 {
        sym += &format!(
            "  (symbol \"Bulk-{i}\" (in_bom yes) (on_board yes)\n    (property \"Reference\" \"U\" (id 0) (at 0 0 0))\n    (property \"Value\" \"Bulk-{i}\" (id 1) (at 0 0 0))\n    (property \"MPN\" \"Bulk-{i}\" (id 2) (at 0 0 0))\n    (property \"Manufacturer\" \"Acme\" (id 3) (at 0 0 0)))\n"
        );
    }
    sym += ")\n";
    std::fs::write(env.lib.join("symbols/Bulk.kicad_sym"), sym).unwrap();

    let mut h = app(SIZES[1], Tab::Import);
    h.state_mut().cx.tab = Tab::Library;
    // The listing runs on a worker, so the frame never waits for it. It takes ~75 ms, less than the first frame of a new tab
    // in a debug build, so "Loading parts" may already be gone: bound the frame instead of racing the worker.
    let first = Instant::now();
    h.step();
    assert!(first.elapsed().as_secs() < 3, "the first frame waited for the 2,000-part listing: {:?}", first.elapsed());
    for _ in 0..30 {
        if h.query_by_label("Bulk-0").is_some() {
            break;
        }
        settle(&mut h);
    }
    assert!(h.query_by_label("Bulk-0").is_some(), "rows never arrived");
    let rows = h.query_all_by_label_contains("Bulk-").count();
    assert!(rows < 60, "{rows} of 2,000 rows were laid out");
    assert!(h.query_by_label("Bulk-1999").is_none());
    let t = Instant::now();
    for _ in 0..5 {
        h.step();
    }
    let per_frame = t.elapsed() / 5;
    assert!(per_frame.as_millis() < 3000, "a frame with 2,000 parts takes {per_frame:?}");
    snap(&mut h, "library-2000");
}

#[test]
fn l7_reload_keeps_the_selection_that_still_exists() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    tick(&mut h, "LM358", Modifiers::NONE);
    tick(&mut h, "RP2040", Modifiers::COMMAND);
    assert_eq!(selected(&h), 2);
    more(&mut h, "Refresh");
    settle(&mut h);
    assert_eq!(selected(&h), 2, "a reload must not forget the ticks");
}

#[test]
fn l9_status_cell_click_finds_datasheet() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    click(&mut h, "Datasheet: missing");
    assert!(log_has(&h, "Looking for datasheets"), "the finder did not start");
    settle(&mut h);
    assert!(h.query_all_by_label("Find missing datasheets").count() >= 1, "no result window");
    assert_eq!(SOURCE_CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn l10_status_cell_click_attaches_a_model() {
    let env = setup();
    let step = env.dir.join("part.step");
    std::fs::write(&step, "ISO-10303-21;").unwrap();
    *TEST_PICK.lock().unwrap() = Some(step);
    let mut h = library(SIZES[1]);
    click(&mut h, "3D model: missing");
    *TEST_PICK.lock().unwrap() = None;
    assert!(h.query_by_label("Add file").is_some(), "the attach dialog did not open");
    assert!(h.query_by_label_contains("part.step").is_some());
}

#[test]
fn l11_standardise_dry_run_then_apply() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    let root = h.state().cx.global_root().unwrap();
    let before = files(&root.dir);
    click(&mut h, "Standardise…");
    settle(&mut h);
    assert!(h.query_by_label("Standardise").is_some(), "no review dialog");
    assert!(missing_glyphs(&h).is_empty());
    snap(&mut h, "library-standardise");
    click(&mut h, "Cancel");
    assert!(h.query_by_label("Apply").is_none(), "Cancel left the dialog open");
    assert_eq!(files(&root.dir), before, "the dry run wrote something");

    click(&mut h, "Standardise…");
    settle(&mut h);
    click(&mut h, "Apply");
    settle(&mut h);
    assert!(log_has(&h, "Standardised"), "nothing logged: {:?}", h.state().cx.log.lines);
    assert_ne!(files(&root.dir), before, "Apply changed nothing");
    assert!(!klm_core::snapshot::list(&root).is_empty(), "no snapshot to roll back to");
}

#[test]
fn l11b_standardise_selected_only() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    tick(&mut h, "LM358", Modifiers::NONE);
    assert!(h.query_by_label("Standardise…").is_none(), "the button should name the selection");
    click(&mut h, "Standardise 1 selected…");
    settle(&mut h);
    assert!(h.query_by_label("Standardise").is_some(), "no review dialog");
    assert!(h.query_by_label_contains("ICs / RP2040").is_none(), "an unticked part is in the plan");
}

#[test]
fn l13_export_selected() {
    let env = setup();
    let mut h = library(SIZES[1]);
    assert!(h.query_by_label("Export…").is_none(), "export needs ticked parts");
    tick(&mut h, "LM358", Modifiers::NONE);
    tick(&mut h, "RP2040", Modifiers::COMMAND);
    click(&mut h, "Export…");
    assert!(h.query_by_label("Export 2 part(s) as a ZIP").is_some(), "no review dialog");
    assert!(h.query_by_label("ICs / LM358").is_some() && h.query_by_label("ICs / RP2040").is_some());
    assert!(missing_glyphs(&h).is_empty());
    snap(&mut h, "library-export");
    click(&mut h, "Apply");
    settle(&mut h);
    let zip = env.dir.join("klm_export.zip");
    assert!(zip.is_file(), "no ZIP written; log: {:?}", h.state().cx.log.lines);
    assert!(log_has(&h, "Exported 2 part(s)"));
}

#[test]
fn l14_health_report() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    click(&mut h, "Health report…");
    settle(&mut h);
    assert!(h.query_by_label("Health report").is_some(), "no findings dialog");
    assert!(missing_glyphs(&h).is_empty());
    snap(&mut h, "library-health");
    click(&mut h, "Copy as CSV");
    assert!(h.query_by_label("Health report").is_some());
    click(&mut h, "Close");
    assert!(h.query_by_label("Health report").is_none());
}

#[test]
fn l15_delete_goes_through_a_review() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    assert!(h.query_by_label("Delete…").is_none(), "nothing to delete yet");
    tick(&mut h, "RP2040", Modifiers::NONE);
    let root = h.state().cx.global_root().unwrap();
    let before = files(&root.dir);

    click(&mut h, "Delete…");
    assert!(h.query_by_label("Delete 1 part(s)?").is_some(), "no confirm dialog");
    assert!(h.query_by_label("ICs / RP2040").is_some() && h.query_by_label_contains("Remove symbol RP2040").is_some());
    assert!(missing_glyphs(&h).is_empty());
    snap(&mut h, "library-delete");
    click(&mut h, "Cancel");
    assert_eq!(files(&root.dir), before, "Cancel must change nothing");

    click(&mut h, "Delete…");
    click(&mut h, "Delete 1 part(s)");
    settle(&mut h);
    assert!(log_has(&h, "Deleted RP2040"), "nothing logged: {:?}", h.state().cx.log.lines);
    assert!(!root.has_part("ICs", "RP2040") && root.has_part("ICs", "LM358"));
    assert!(h.query_by_label("RP2040").is_none(), "row still listed");
    assert!(!klm_core::snapshot::list(&root).is_empty(), "no snapshot to roll back to");
}

#[test]
fn l16_restyle_goes_through_a_review() {
    let env = setup();
    // The seeded one-pin parts can't be restyled; give the resistor a second pin so it can.
    let font = "(effects (font (size 1.27 1.27)))";
    let pin = |n: &str, at: &str| format!("(pin passive line (at {at}) (length 2.54) (name \"{n}\" {font}) (number \"{n}\" {font}))");
    let name = "RC0603FR-0710KL";
    std::fs::write(
        env.lib.join("symbols/Resistors.kicad_sym"),
        format!(
            "(kicad_symbol_lib (version 20211014) (generator test)
 (symbol \"{name}\" (in_bom yes) (on_board yes)
  (property \"Reference\" \"U\" (id 0) (at 0 2.54 0) {font})
  (property \"Value\" \"{name}\" (id 1) (at 0 -2.54 0) {font})
  (property \"MPN\" \"{name}\" (id 2) (at 0 0 0) {font})
  (symbol \"{name}_0_1\" (rectangle (start -2.54 1.27) (end 2.54 -1.27) (stroke (width 0.254)) (fill (type background))))
  (symbol \"{name}_1_1\" {} {}))
)",
            pin("1", "-5.08 0 0"),
            pin("2", "5.08 0 180")
        ),
    )
    .unwrap();
    let mut h = library(SIZES[1]);
    let root = h.state().cx.global_root().unwrap();
    let before = files(&root.dir);

    // A refusal is explained and can only be closed.
    click(&mut h, "LM358");
    click(&mut h, "Restyle symbol…");
    assert!(h.query_by_label_contains("an IC box needs at least 3 pins").is_some(), "the refusal is not shown");
    click(&mut h, "Close");

    click(&mut h, "RC0603FR-0710KL");
    click(&mut h, "Restyle symbol…");
    assert!(h.query_by_label("Resistors / RC0603FR-0710KL").is_some(), "no review dialog");
    snap(&mut h, "library-restyle");
    assert_eq!(files(&root.dir), before, "the dry run wrote something");
    click(&mut h, "Cancel");
    assert_eq!(files(&root.dir), before);
    click(&mut h, "Restyle symbol…");
    click(&mut h, "Apply");
    settle(&mut h);
    assert!(log_has(&h, "RC0603FR-0710KL:"), "no result logged: {:?}", h.state().cx.log.lines);
    assert_ne!(files(&root.dir), before, "Apply restyled nothing");
}

#[test]
fn l18_for_part_label_comes_first() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    click(&mut h, "Alternatives…");
    settle(&mut h);
    let label = h
        .root()
        .children_recursive()
        .find(|n| n.accesskit_node().role() == Role::Label && n.accesskit_node().value().as_deref() == Some("For part"))
        .expect("no 'For part' label")
        .rect();
    let combo = h.get_by_role_and_label(Role::ComboBox, "For part").rect();
    assert!(label.max.x <= combo.min.x + 1.0, "label {label:?} is not before the box {combo:?}");
}

#[test]
fn l19_preview_thumbnails_are_at_least_240px() {
    let _env = setup();
    for size in SIZES {
        let mut h = library(size);
        click(&mut h, "RP2040");
        settle(&mut h);
        // The inspector shows the three pictures inside the window.
        let small: Vec<_> = h.query_all_by_role(Role::Image).map(|n| n.rect()).collect();
        assert!(small.len() >= 2, "{size:?}: no pictures in the inspector");
        assert!(small.iter().all(|r| r.min.x >= 0.0 && r.max.x <= size.0 && r.max.y <= size.1), "{size:?}: inspector pictures off the window: {small:?}");
        snap(&mut h, "library-inspector");
        // Actions → Preview opens the large pictures.
        click(&mut h, "Actions");
        click(&mut h, "Preview");
        settle(&mut h);
        // The picture keeps its aspect ratio inside a 240x180 frame, so the frames' spacing is what shows their width.
        let mut images: Vec<_> = h.query_all_by_role(Role::Image).map(|n| n.rect()).filter(|r| r.height() >= 170.0).collect();
        images.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        assert!(images.len() >= 2, "{size:?}: no thumbnails");
        assert!(images[1].min.x - images[0].min.x >= 240.0, "{size:?}: thumbnail frames narrower than 240 px: {images:?}");
        assert!(
            images.iter().all(|r| r.height() >= 179.0 && r.max.x <= size.0 && r.max.y <= size.1),
            "{size:?}: thumbnails too short or off the window: {images:?}"
        );
        snap(&mut h, "library-preview");
    }
}

#[test]
fn l20_every_column_fits_at_both_sizes() {
    let _env = setup();
    for size in SIZES {
        let h = library(size);
        let checks = h.get_by_label("Checks").rect();
        assert!(checks.max.x <= size.0, "{size:?}: the Checks column is cut off: {checks:?}");
        assert!(missing_glyphs(&h).is_empty());
    }
}

#[test]
fn l21_show_filters_to_one_part() {
    let _env = setup();
    let mut h = library(SIZES[1]);
    h.state().cx.send(crate::Msg::Library(crate::tabs::library::Msg::Show("RP2040".into())));
    h.run_ok();
    assert!(h.query_all_by_label("LM358").next().is_none() && h.query_all_by_label("RP2040").next().is_some());
}

/// The Alternatives window on a library where RC0603FR-0710KL is also in a second category, so the scan suggests that pair.
fn alternatives_with_duplicate<'a>(confirmed: bool) -> Gui<'a> {
    let mut h = library(SIZES[1]);
    let root = h.state().cx.global_root().unwrap();
    let part = klm_core::part::Part { mpn: "RC0603FR-0710KL".into(), manufacturer: "Yageo".into(), ..Default::default() };
    let sym = root.dir.join("dup.kicad_sym");
    std::fs::write(&sym, include_str!("dup.kicad_sym").replace("NAME", "RC0603FR-0710KL")).unwrap();
    root.add_part(&klm_core::part::Part { symbol_file: Some(sym), ..part }, "Spare", false).unwrap();
    if confirmed {
        // A stored group, as Confirm would leave it (Confirm itself needs two different MPNs).
        let store = klm_core::store::Store::open(&klm_core::store::Store::default_path().unwrap()).unwrap();
        for (mpn, role) in [("RC0603FR-0710KL", "preferred"), ("RC-ALT", "substitute")] {
            store
                .set_alternative(
                    "RC0603FR-0710KL",
                    &klm_core::store::Alternative { mpn: mpn.into(), manufacturer: "Yageo".into(), supplier_ids: vec![], role: role.into() },
                )
                .unwrap();
        }
    }
    click(&mut h, "Alternatives…");
    settle(&mut h);
    h
}

fn suggested(h: &Gui) -> usize {
    let root = h.state().cx.global_root().unwrap();
    let store = klm_core::store::Store::open(&klm_core::store::Store::default_path().unwrap()).unwrap();
    klm_core::alternatives::suggest_unrejected(&store, &klm_core::alternatives::scan(&root)).unwrap().len()
}

#[test]
fn l21_reject_hides_a_suggestion_for_good() {
    let _env = setup();
    let mut h = alternatives_with_duplicate(false);
    assert_eq!(suggested(&h), 1, "the duplicate is not suggested:\n{}", dump(&h));
    assert!(h.query_by_label("Ungroup").is_none(), "an unconfirmed group offers Ungroup");
    click(&mut h, "Reject");
    settle(&mut h);
    assert!(h.query_by_label("Reject").is_none() && h.query_by_label("Confirm group").is_none(), "the group is still shown");
    assert!(log_has(&h, "Rejected the group"), "{:?}", h.state().cx.log.lines);
    assert_eq!(suggested(&h), 0, "a rejected group came back on rescan");
}

#[test]
fn l22_ungroup_dissolves_a_confirmed_group() {
    let _env = setup();
    let mut h = alternatives_with_duplicate(true);
    assert!(
        h.query_by_label("Reject").is_none(),
        "a confirmed group offers Reject:
{}",
        dump(&h)
    );
    click(&mut h, "Ungroup");
    settle(&mut h);
    assert!(h.query_by_label("Ungroup").is_none() && h.query_by_label("Confirm group").is_none(), "the group is still shown");
    assert!(log_has(&h, "Ungrouped"), "{:?}", h.state().cx.log.lines);
    let store = klm_core::store::Store::open(&klm_core::store::Store::default_path().unwrap()).unwrap();
    assert!(klm_core::alternatives::alternatives_of(&store, "RC0603FR-0710KL").unwrap().is_empty());
    assert_eq!(suggested(&h), 0, "an ungrouped group came back on rescan");
}
