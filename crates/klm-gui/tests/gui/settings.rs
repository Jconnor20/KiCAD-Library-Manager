//! W7.1: the Settings popup. docs/ui-review/W7.1.md has the plan (S1-S17).

use super::harness::*;
use crate::tabs::settings::Msg as SettingsMsg;
use crate::tabs::ui_kit::Section;
use crate::{Msg, Tab};
use eframe::egui::{self, accesskit::Role};
use egui_kittest::kittest::{NodeT, Queryable};

/// (tree label, text only that page has, page name for the snapshot)
const PAGES: [(&str, &str, &str); 14] = [
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
    ("BOM", "Shipping region", "bom"),
    ("History", "Keep the last", "history"),
];

fn open(h: &mut Gui) {
    click(h, "Settings…");
}

fn tree(h: &mut Gui, label: &str) {
    h.get_all_by_role_and_label(Role::Button, label).last().unwrap().click();
    h.run_ok();
}

fn has(h: &Gui, text: &str) -> bool {
    h.query_all_by_label(text).next().is_some() || h.query_all_by_label_contains(text).next().is_some()
}

fn window(h: &Gui) -> egui::Rect {
    h.get_by_role_and_label(Role::Window, "Settings").rect()
}

/// The text boxes of the popup, top to bottom: the search box first.
fn boxes<'a>(h: &'a Gui) -> Vec<egui_kittest::Node<'a>> {
    let w = window(h);
    let mut v: Vec<_> = h.get_all_by_role(Role::TextInput).filter(|n| w.contains_rect(n.rect())).collect();
    v.sort_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y));
    v
}

/// Types into the first page text box (right of the tree), after the search box.
fn type_in_page(h: &mut Gui, text: &str) {
    let b = boxes(h).into_iter().find(|n| n.rect().min.x > 270.0).expect("a page text box");
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
fn s1_popup_size_and_first_page() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Library);
        assert!(h.query_by_role_and_label(Role::Window, "Settings").is_none());
        open(&mut h);
        let w = window(&h);
        assert!((w.width() / size.0 - 0.8).abs() < 0.03 && (w.height() / size.1 - 0.8).abs() < 0.03, "popup is {:?} in {size:?}", w.size());
        assert!(has(&h, "General › Libraries"), "does not open on Libraries");
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
        click(&mut h, "Cancel");
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
    assert!(has(&h, "Sources"), "the group header of the match is kept");
    snap(&mut h, "search-filter");
    // A group's name finds all its pages.
    click(&mut h, "Cancel");
    open(&mut h);
    search(&mut h, "standards");
    assert!(h.query_by_role_and_label(Role::Button, "Symbols").is_some() && h.query_by_role_and_label(Role::Button, "Footprints").is_some());
    click(&mut h, "Cancel");
    open(&mut h);
    search(&mut h, "xyzzy");
    assert!(has(&h, "No page matches"));
    snap(&mut h, "search-no-match");
}

#[test]
fn s5_cancel_leaves_settings_json_identical() {
    let env = setup();
    let before = settings_file(&env);
    let mut h = app(SIZES[1], Tab::Library);
    let live = format!("{:?}", h.state().cx.settings);
    open(&mut h);
    edit_everything(&mut h);
    click(&mut h, "Cancel");
    assert!(h.query_by_role_and_label(Role::Window, "Settings").is_none());
    assert_eq!(settings_file(&env), before, "settings.json changed");
    assert_eq!(format!("{:?}", h.state().cx.settings), live, "the live settings changed");
    // Reopening shows the saved values, not the abandoned edits.
    open(&mut h);
    tree(&mut h, "BOM");
    assert!(boxes(&h).iter().all(|b| b.value().unwrap_or_default() != "ZZ"));
}

#[test]
fn s6_close_button_and_escape_discard() {
    let env = setup();
    let before = settings_file(&env);
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    edit_everything(&mut h);
    h.get_by_role_and_label(Role::Button, "Close window").click();
    h.run_ok();
    assert!(h.query_by_role_and_label(Role::Window, "Settings").is_none(), "✕ did not close");
    assert_eq!(settings_file(&env), before);
    open(&mut h);
    edit_everything(&mut h);
    h.key_press(egui::Key::Escape);
    h.run_ok();
    assert!(h.query_by_role_and_label(Role::Window, "Settings").is_none(), "Esc did not close");
    assert_eq!(settings_file(&env), before);
}

#[test]
fn s7_apply_and_ok_save() {
    let env = setup();
    let mut h = app(SIZES[1], Tab::Library);
    open(&mut h);
    tree(&mut h, "BOM");
    type_in_page(&mut h, "DE");
    click(&mut h, "Apply");
    assert!(h.query_by_role_and_label(Role::Window, "Settings").is_some(), "Apply closed the popup");
    assert_eq!(saved(&env)["shipping_region"], "DE");
    assert_eq!(h.state().cx.settings.shipping_region, "DE");
    tree(&mut h, "Import");
    click(&mut h, "Add category rule");
    click(&mut h, "OK");
    assert!(h.query_by_role_and_label(Role::Window, "Settings").is_none(), "OK did not close");
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
    click(&mut h, "Cancel");
    assert!(klm_core::credentials::get("Mouser", "api_key").is_some(), "Cancel took the key back");
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
    click(&mut h, "Apply");
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
        click(&mut h, "Cancel");
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
            let w = window(&h);
            assert!(w.min.x >= 0.0 && w.min.y >= 0.0 && w.max.x <= size.0 && w.max.y <= size.1, "{label}: popup {w:?} outside {size:?}");
            for b in ["Restore defaults", "OK", "Cancel", "Apply"] {
                let r = h.get_by_role_and_label(Role::Button, b).rect();
                assert!(w.contains_rect(r), "{label}: {b} at {r:?} is outside the popup {w:?}");
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
    click(&mut h, "Apply");
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
    click(&mut h, "OK");
    assert_eq!(h.state().cx.settings.global_root.as_ref(), Some(&moved));
    assert_eq!(saved(&env)["global_root"], serde_json::json!(moved));
}
