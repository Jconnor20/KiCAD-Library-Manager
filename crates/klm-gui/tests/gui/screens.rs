use super::harness::*;
use crate::Tab;

/// The Search tab with a source enabled (a stub that fails every call), which is its real layout.
#[test]
fn search_tab_with_a_source() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::Search);
        with_probe(&mut h);
        snap(&mut h, "search-with-source");
    }
}

#[test]
fn every_tab_opens_and_is_saved() {
    let _env = setup();
    for size in SIZES {
        for (tab, name) in [
            (Tab::Search, "search"),
            (Tab::Import, "import"),
            (Tab::Library, "library"),
            (Tab::Bom, "bom"),
            (Tab::History, "history"),
            (Tab::Cleanup, "cleanup"),
            (Tab::ProjectCheck, "project-check"),
        ] {
            let mut h = app(size, tab);
            let m = snap(&mut h, &format!("tab-{name}"));
            eprintln!("{name} {size:?} missing glyphs: {m:?}");
        }
    }
}

use eframe::egui::accesskit::Role;
use egui_kittest::kittest::Queryable;

/// Opens Settings and every Library dialog at both sizes and saves a PNG of each.
#[test]
fn settings_and_every_library_dialog_open() {
    let _env = setup();
    for size in SIZES {
        let lib = || {
            let mut h = app(size, Tab::Library);
            settle(&mut h);
            h
        };
        let mut h = lib();
        snap(&mut h, "library-list");

        click(&mut h, "Settings…");
        snap(&mut h, "settings");

        let mut h = lib();
        click(&mut h, "RP2040");
        snap(&mut h, "library-detail-panel");

        h.get_all_by_label("RP2040").next().unwrap().click_secondary();
        h.run_ok();
        snap(&mut h, "library-context-menu");

        let mut h = lib();
        click(&mut h, "RP2040");
        click(&mut h, "Edit…");
        snap(&mut h, "library-edit-dialog");

        let mut h = lib();
        click(&mut h, "RP2040");
        click(&mut h, "Actions");
        click(&mut h, "Preview");
        snap(&mut h, "library-preview");

        let mut h = lib();
        click(&mut h, "LM358");
        click(&mut h, "Restyle symbol…");
        snap(&mut h, "library-restyle-logged");

        let mut h = lib();
        click(&mut h, "Alternatives…");
        snap(&mut h, "library-alternatives");

        // "Attach…" opens a native file picker, so the dialog is opened directly with a file.
        let mut h = lib();
        click(&mut h, "RP2040");
        let ds = h.state().cx.global_root().unwrap().dir.join("datasheets/x.pdf");
        h.state_mut().library_tab.attach_dialog =
            Some(crate::tabs::library::AttachDialog::new(klm_core::library::Scope::Global, "ICs".into(), "RP2040".into(), ds));
        h.run_ok();
        snap(&mut h, "library-attach-dialog");

        let mut h = lib();
        click(&mut h, "More");
        click(&mut h, "Relink libraries");
        snap(&mut h, "library-batch-relink");

        let mut h = lib();
        h.query_all_by_role(Role::CheckBox).next().unwrap().click();
        h.run_ok();
        click(&mut h, "Copy to project");
        snap(&mut h, "library-batch-copy-to-project");

        let mut h = lib();
        click(&mut h, "More");
        click(&mut h, "Find missing datasheets");
        snap(&mut h, "library-find-datasheets");
    }
}
