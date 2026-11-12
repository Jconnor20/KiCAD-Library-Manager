//! W7.1: the Settings page (it was a popup until the wireframe redesign). docs/ui-review/W7.1.md has the plan (S1-S17).

use super::harness::*;
use crate::tabs::settings::Msg as SettingsMsg;
use crate::tabs::ui_kit::Section;
use crate::{Msg, Tab};
use eframe::egui::{self, accesskit::Role};
use egui_kittest::kittest::{NodeT, Queryable};

/// (tree label, text only that page has, page name for the snapshot)
const PAGES: [(&str, &str, &str); 15] = [
    ("Libraries", "Global library", "libraries"),
    ("Watch folders", "Add folder…", "watch-folders"),
    ("KiCad", "Browse…", "kicad"),
    ("LCSC", "LCSC needs no keys.", "lcsc"),
    ("DigiKey", "Client secret", "digikey"),
    ("Mouser", "API key", "mouser"),
    ("Nexar", "Client ID", "nexar"),
    ("SnapMagic", "Kept for later: there is no search source for it yet.", "snapmagic"),
    ("Search", "Reuse search results for", "search"),
    ("Import", "Add category rule", "import"),
    ("Symbols", "Text size", "symbols"),
    ("Footprints", "Silkscreen line width", "footprints"),
    ("Part information", "Add field", "part-info"),
    ("BOM", "Shipping region", "bom"),
    ("History", "Keep the last", "history"),
];

fn open(h: &mut Gui) {
    click(h, "Settings…");
}

fn tree(h: &mut Gui, label: &str) {
    let b = h.get_all_by_role_and_label(Role::Button, label).last().unwrap();
    b.scroll_to_me();
    h.run_ok();
    h.get_all_by_role_and_label(Role::Button, label).last().unwrap().click();
    h.run_ok();
}

fn has(h: &Gui, text: &str) -> bool {
    h.query_all_by_label(text).next().is_some() || h.query_all_by_label_contains(text).next().is_some()
}

/// Leaves the page (the shell's other tabs), which throws away edits Save didn't take.
fn close(h: &mut Gui) {
    h.state_mut().cx.tab = Tab::Library;
    h.run_ok();
}

fn on_settings(h: &Gui) -> bool {
    h.state().cx.tab == Tab::Settings
}

/// The text boxes of the page, top to bottom: the tree's search box first (the top bar's omnibox is left out).
fn boxes<'a>(h: &'a Gui) -> Vec<egui_kittest::Node<'a>> {
    let mut v: Vec<_> = h.get_all_by_role(Role::TextInput).filter(|n| n.rect().min.y > 50.0).collect();
    v.sort_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y));
    v
}

/// Types into the first page text box (right of the tree), after the search box.
fn type_in_page(h: &mut Gui, text: &str) {
    let b = boxes(h).into_iter().find(|n| n.rect().min.x > 300.0).expect("a page text box");
    b.click();
    b.type_text(text);
    h.run_ok();
}

fn search(h: &mut Gui, text: &str) {
    let b = boxes(h).into_iter().next().expect("search box");
    b.click();
    b.type_text(text);
    h.run_ok();
}

fn settings_file(env: &Env) -> Vec<u8> {
    std::fs::read(env.dir.join("settings.json")).unwrap()
}

fn saved(env: &Env) -> serde_json::Value {
    serde_json::from_slice(&settings_file(env)).unwrap()
}

/// Edits a text box, a checkbox and a list on three pages without applying anything.
fn edit_everything(h: &mut Gui) {
    tree(h, "BOM");
    type_in_page(h, "ZZ");
    tree(h, "Import");
    click(h, "Add category rule");
    tree(h, "DigiKey");
    click(h, "Enabled");
}

#[test]
fn s1_page_and_first_page() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Library);
        assert!(!on_settings(&h));
        open(&mut h);
        assert!(on_settings(&h), "the Settings item did not open the page");
        assert!(has(&h, "General › Libraries"), "does not open on Libraries");
        assert!(has(&h, "Appearance"), "no theme choice on General");
        snap(&mut h, "settings-open");
    }
}

#[test]
fn s2_every_page_opens_from_the_tree() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Library);
        open(&mut h);
        for (label, marker, name) in PAGES {
            tree(&mut h, label);
            assert!(has(&h, marker), "{label} at {size:?}: no {marker:?}");
            snap(&mut h, &format!("page-{name}"));
        }
    }
}

#[test]
fn s3_every_page_opens_from_search() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    for (label, marker, _) in PAGES {
        open(&mut h);
        search(&mut h, label);
        tree(&mut h, label);
        assert!(has(&h, marker), "{label}: no {marker:?} after searching for it");
        close(&mut h);
    }
}

#[test]
fn s4_search_filters_the_tree() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    search(&mut h, "mouser");
    assert!(h.query_by_role_and_label(Role::Button, "Mouser").is_some());
    for gone in ["DigiKey", "Libraries", "Symbols", "Nexar"] {
        assert!(h.query_by_role_and_label(Role::Button, gone).is_none(), "{gone} is still in the tree");
    }
    assert!(has(&h, "SOURCES"), "the group header of the match is kept");
    snap(&mut h, "search-filter");
    // A group's name finds all its pages.
    close(&mut h);
    open(&mut h);
    search(&mut h, "standards");
    assert!(h.query_by_role_and_label(Role::Button, "Symbols").is_some() && h.query_by_role_and_label(Role::Button, "Footprints").is_some());
    close(&mut h);
    open(&mut h);
    search(&mut h, "xyzzy");
    assert!(has(&h, "No page matches"));
    snap(&mut h, "search-no-match");
}

#[test]
fn s5_discard_leaves_settings_json_identical() {
    let env = setup();
    let before = settings_file(&env);
    let mut h = app(SIZES[1], Tab::Library);
    let live = format!("{:?}", h.state().cx.settings);
    open(&mut h);
    edit_everything(&mut h);
    assert!(has(&h, "unsaved changes"));
    click(&mut h, "Discard");
    assert!(on_settings(&h), "Discard leaves the page");
    assert!(!has(&h, "unsaved changes"));
    assert_eq!(settings_file(&env), before, "settings.json changed");
    assert_eq!(format!("{:?}", h.state().cx.settings), live, "the live settings changed");
    // The page shows the saved values, not the abandoned edits.
    tree(&mut h, "BOM");
    assert!(boxes(&h).iter().all(|b| b.value().unwrap_or_default() != "ZZ"));
}

#[test]
fn s6_leaving_the_page_discards() {
    let env = setup();
    let before = settings_file(&env);
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    edit_everything(&mut h);
    // The rail's own Settings item while on the page keeps the edits.
    click(&mut h, "Settings…");
    assert!(has(&h, "unsaved changes"), "the rail item dropped the edits");
    h.state_mut().cx.tab = Tab::History;
    h.run_ok();
    assert!(!on_settings(&h));
    assert_eq!(settings_file(&env), before);
    open(&mut h);
    assert!(!has(&h, "unsaved changes"), "the abandoned edits came back");
}

#[test]
fn s7_save_keeps_the_page_open() {
    let env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    tree(&mut h, "BOM");
    type_in_page(&mut h, "DE");
    click(&mut h, "Save");
    assert!(on_settings(&h), "Save left the page");
    assert!(!has(&h, "unsaved changes"));
    assert_eq!(saved(&env)["shipping_region"], "DE");
    assert_eq!(h.state().cx.settings.shipping_region, "DE");
    tree(&mut h, "Import");
    click(&mut h, "Add category rule");
    click(&mut h, "Save");
    assert_eq!(saved(&env)["rules"]["categories"].as_array().unwrap().len(), 1);
    assert_eq!(saved(&env)["shipping_region"], "DE");
}

#[test]
fn b1_settings_key_fields_are_at_least_300px_wide() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Library);
        open(&mut h);
        for source in ["DigiKey", "Mouser", "Nexar", "SnapMagic"] {
            tree(&mut h, source);
            let widths: Vec<f32> = h.query_all_by_role(Role::PasswordInput).map(|n| n.rect().width()).collect();
            assert!(!widths.is_empty(), "no key fields on {source}");
            assert!(widths.iter().all(|w| *w >= 300.0), "key field widths for {source} at {size:?}: {widths:?}");
        }
    }
}

#[test]
fn s9_keys_are_saved_at_once() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    tree(&mut h, "Mouser");
    let b = h.get_by_role(Role::PasswordInput);
    b.click();
    b.type_text("test-key-123");
    h.run_ok();
    click(&mut h, "Save keys");
    assert_eq!(klm_core::credentials::get("Mouser", "api_key").as_deref(), Some("test-key-123"));
    click(&mut h, "Discard");
    assert!(klm_core::credentials::get("Mouser", "api_key").is_some(), "Discard took the key back");
    close(&mut h);
    open(&mut h);
    tree(&mut h, "Mouser");
    assert!(h.get_by_role(Role::PasswordInput).value().unwrap_or_default().is_empty(), "a stored key was read back into the box");
    click(&mut h, "Clear keys");
    assert!(klm_core::credentials::get("Mouser", "api_key").is_none());
}

#[test]
fn s10_test_result_is_shown_on_the_page() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    tree(&mut h, "Mouser");
    h.state().cx.send(Msg::Settings(SettingsMsg::Tested("Mouser", Ok("25 results".into()))));
    h.run_ok();
    h.run_ok();
    assert!(has(&h, "✔ 25 results"), "no ok badge");
    snap(&mut h, "test-ok");
    h.state().cx.send(Msg::Settings(SettingsMsg::Tested("Mouser", Err("HTTP 401 unauthorized".into()))));
    h.run_ok();
    h.run_ok();
    assert!(has(&h, "✘ HTTP 401 unauthorized") && !has(&h, "✔ 25 results"), "no failure badge");
    snap(&mut h, "test-fail");
}

#[test]
fn s11_restore_defaults_is_per_page() {
    let env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    h.state_mut().cx.settings.history.keep_minors = 33;
    h.state_mut().cx.settings.history.auto_version = klm_core::settings::AutoVersion::Daily;
    open(&mut h);
    tree(&mut h, "BOM");
    type_in_page(&mut h, "FR");
    tree(&mut h, "History");
    click(&mut h, "Restore defaults");
    click(&mut h, "Save");
    let s = saved(&env);
    assert_eq!(s["history"]["keep_minors"], 10);
    assert_eq!(s["history"]["auto_version"], "Weekly");
    assert_eq!(s["shipping_region"], "FR", "restoring History touched BOM");
    tree(&mut h, "Libraries");
    assert!(h.get_by_role_and_label(Role::Button, "Restore defaults").accesskit_node().is_disabled(), "Libraries has nothing to restore");
}

#[test]
fn s12_open_settings_at_a_section() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    for (section, marker) in [(Section::Sources, "LCSC needs no keys."), (Section::History, "Keep the last"), (Section::Bom, "Shipping region"), (Section::Standards, "Text size"), (Section::Mouser, "API key"), (Section::General, "Global library")] {
        h.state().cx.open_settings(section);
        h.run_ok();
        assert!(has(&h, marker), "{section:?}: no {marker:?}");
        close(&mut h);
    }
}

#[test]
fn s13_pages_fit_and_have_no_tofu() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Library);
        open(&mut h);
        for (label, _, _) in PAGES {
            tree(&mut h, label);
            let w = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1));
            for b in ["Restore defaults", "Save", "Discard"] {
                let r = h.get_by_role_and_label(Role::Button, b).rect();
                assert!(w.contains_rect(r), "{label}: {b} at {r:?} is outside the window {w:?}");
            }
            let missing = missing_glyphs(&h);
            assert!(missing.is_empty(), "{label} at {size:?} draws {missing:?} as boxes");
        }
    }
}

#[test]
fn s14_history_page() {
    let env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    h.state_mut().cx.settings.history.keep_minors = 33;
    open(&mut h);
    tree(&mut h, "History");
    click(&mut h, "Save");
    assert_eq!(saved(&env)["history"]["keep_minors"], 33);
}

#[test]
fn s15_two_columns_only_when_wide() {
    let _env = setup();
    let mut side_by_side = vec![];
    for size in SIZES {
        let mut h = app(size, Tab::Library);
        open(&mut h);
        tree(&mut h, "Import");
        let (a, b) = (h.get_by_label("Category rules").rect(), h.get_by_label("Part names").rect());
        side_by_side.push((a.min.y - b.min.y).abs() < 2.0);
    }
    assert_eq!(side_by_side, [false, true], "side by side at 820 / at 1280");
}

#[test]
fn s16_apply_keeps_a_moved_library() {
    let env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    let moved = env.dir.join("moved");
    h.state_mut().cx.settings.global_root = Some(moved.clone());
    tree(&mut h, "BOM");
    type_in_page(&mut h, "DE");
    click(&mut h, "Save");
    assert_eq!(h.state().cx.settings.global_root.as_ref(), Some(&moved));
    assert_eq!(saved(&env)["global_root"], serde_json::json!(moved));
}

#[test]
fn s17_part_information_edits_save_with_the_profile() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    tree(&mut h, "Part information");
    assert!(has(&h, "Built-in default") && has(&h, "Preview: Resistor 10kΩ 5% 100mW Temp coefficient 0603"));
    // The button is below the fold of the field list.
    h.get_by_role_and_label(Role::Button, "Add field").scroll_to_me();
    h.run_ok();
    h.get_by_role_and_label(Role::Button, "Add field").click();
    h.run_ok();
    h.run_ok();
    assert!(has(&h, "Customised in this profile"));
    // The Description template box is the first one after the field rows' boxes.
    let desc = boxes(&h).into_iter().find(|n| n.value().is_some_and(|v| v.starts_with("Resistor {Resistance}"))).expect("description box");
    desc.scroll_to_me();
    h.run_ok();
    let desc = boxes(&h).into_iter().find(|n| n.value().is_some_and(|v| v.starts_with("Resistor {Resistance}"))).unwrap();
    desc.click();
    h.run_ok();
    h.key_press(egui::Key::End);
    h.run_ok();
    boxes(&h).into_iter().find(|n| n.value().is_some_and(|v| v.starts_with("Resistor {Resistance}"))).unwrap().type_text(" {New field}");
    h.run_ok();
    assert!(has(&h, "Preview: Resistor 10kΩ 5% 100mW Temp coefficient 0603 New field"));
    click(&mut h, "Save");
    let p = klm_core::standardise::profile::load(klm_core::standardise::profile::BUILT_IN).unwrap();
    let r = &p.classes["Resistor"];
    assert!(r.fields.iter().any(|f| f.name == "New field") && r.description.ends_with("{New field}"));
    assert!(!p.classes.contains_key("Capacitor"), "only the edited class is customised");
    tree(&mut h, "Part information");
    click(&mut h, "Restore defaults");
    assert!(has(&h, "Built-in default"));
}
