use super::harness::*;
use crate::Tab;
use egui_kittest::kittest::Queryable;

const PCB: &str = r#"(kicad_pcb (version 20240108) (generator "klm")
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (setup)
  (net 0 "")
  (gr_rect (start 0 0) (end 20 10) (stroke (width 0.1) (type solid)) (fill none) (layer "Edge.Cuts"))
  (segment (start 5 5) (end 15 5) (width 0.1) (layer "F.Cu") (net 0))
)
"#;
const SCH: &str = r#"(kicad_sch (version 20231120) (generator "klm") (uuid "0b1c2d3e-0000-4000-8000-000000000001") (paper "A4")
  (lib_symbols)
  (label "DANGLING" (at 50.8 50.8 0) (effects (font (size 1.27 1.27))) (uuid "0b1c2d3e-0000-4000-8000-000000000003"))
)
"#;

/// Polls until `label` (a substring) is on screen; kicad-cli takes a few seconds per run.
fn wait_for(h: &mut Gui, label: &str) -> bool {
    for _ in 0..120 {
        settle(h);
        if h.query_by_label_contains(label).is_some() {
            return true;
        }
    }
    false
}

#[test]
fn project_check_empty_state_and_run_button() {
    let _env = setup();
    for size in SIZES {
        let mut h = app(size, Tab::ProjectCheck);
        settle(&mut h);
        assert!(h.query_by_label("Run check").is_some() && h.query_by_label("All fabs").is_some(), "{}", dump(&h));
        assert!(snap(&mut h, "standards-project-check").is_empty());

        h.state_mut().cx.project = None;
        h.run_ok();
        assert!(h.query_by_label_contains("Choose a project").is_some(), "{}", dump(&h));
        assert!(h.query_by_label("Run check").is_none());
    }
}

/// The seeded project has no board or schematic: the check's error is shown instead of a table.
#[test]
fn project_check_shows_the_error() {
    let _env = setup();
    let mut h = app(SIZES[1], Tab::ProjectCheck);
    click(&mut h, "Run check");
    assert!(wait_for(&mut h, "kicad_pcb"), "{}", dump(&h));
    assert!(h.query_by_label_contains("Checked against").is_some());
}

/// Needs the real kicad-cli; skipped without it, like the other kicad-cli tests.
#[test]
fn project_check_with_kicad_cli() {
    let env = setup();
    if !klm_core::kicad_env::kicad_cli().is_ok_and(|c| std::process::Command::new(c).arg("version").output().is_ok_and(|o| o.status.success())) {
        return eprintln!("kicad-cli not found; skipping");
    }
    let dir = env.dir.join("mini");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in [("mini.kicad_pro", "{}"), ("mini.kicad_pcb", PCB), ("mini.kicad_sch", SCH)] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let mut h = app(SIZES[1], Tab::ProjectCheck);
    h.state_mut().cx.project = Some(dir);
    h.run_ok();
    click(&mut h, "Run check");
    assert!(wait_for(&mut h, "DANGLING"), "{}", dump(&h));
    assert!(h.query_by_label_contains("errors").is_some() || h.query_by_label_contains("warnings").is_some());
    assert!(snap(&mut h, "standards-project-check-results").is_empty());

    click(&mut h, "All fabs");
    click(&mut h, "Run check");
    assert!(wait_for(&mut h, "fabs pass"), "{}", dump(&h));
}
