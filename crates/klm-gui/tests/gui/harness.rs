use crate::{App, Tab};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use klm_core::library::LibraryRoot;
use klm_core::part::Part;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

pub const SIZES: [(f32, f32); 2] = [(820.0, 560.0), (1280.0, 800.0)];

static LOCK: Mutex<()> = Mutex::new(());
static SANDBOX: OnceLock<tempfile::TempDir> = OnceLock::new();

pub fn sandbox() -> &'static Path {
    SANDBOX
        .get_or_init(|| {
            let d = tempfile::Builder::new().prefix("klm-gui-test").tempdir().unwrap();
            klm_core::settings::set_sandbox(d.path());
            d
        })
        .path()
}

const SYMBOL: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "NAME" (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "NAME" (id 1) (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (symbol "NAME_0_1" (rectangle (start -2.54 1.27) (end 2.54 -1.27) (stroke (width 0.254)) (fill (type background))))
    (symbol "NAME_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
)"#;
const FOOTPRINT: &str = r#"(module FP (layer F.Cu) (tedit 5A02FF57)
  (fp_text reference REF** (at 0 -2.5) (layer F.SilkS) (effects (font (size 1 1) (thickness 0.15))))
  (fp_text value FP (at 0 2.5) (layer F.Fab) (effects (font (size 1 1) (thickness 0.15))))
  (pad 1 smd rect (at -1 0.95) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask))
)"#;

/// The three seeded parts: (MPN, maker, category, has footprint+3D+datasheet, has footprint only).
pub const PARTS: [(&str, &str, &str); 3] =
    [("LM358", "Texas Instruments", "ICs"), ("RP2040", "Raspberry Pi", "ICs"), ("RC0603FR-0710KL", "Yageo", "Resistors")];

/// Holds the lock that serialises the tests (they share one sandbox) and the sandbox's paths.
#[allow(dead_code)] // paths for tests that need them; most only hold the lock
pub struct Env {
    _guard: MutexGuard<'static, ()>,
    pub dir: PathBuf,
    pub lib: PathBuf,
    pub project: PathBuf,
}

/// Wipes the sandbox, seeds a library and a project folder, and writes settings with every source off.
pub fn setup() -> Env {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = sandbox().to_path_buf();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = entry.path();
        let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    }
    let (lib, project, src, dl) = (dir.join("lib"), dir.join("proj"), dir.join("src"), dir.join("downloads"));
    for d in [&project, &src, &dl] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(project.join("test.kicad_pro"), "{}").unwrap();
    std::fs::write(
        dir.join("settings.json"),
        serde_json::json!({
            "global_root": lib, "last_project": project, "watch_dirs": [dl],
            "source_enabled": {"LCSC": false, "DigiKey": false, "Mouser": false, "Nexar": false},
        })
        .to_string(),
    )
    .unwrap();

    let cfg = klm_core::kicad_env::config_dir().unwrap();
    assert!(cfg.starts_with(&dir), "KiCad tables must be in the sandbox: {}", cfg.display());
    let root = LibraryRoot::global(lib.clone(), cfg);
    for (i, (mpn, maker, cat)) in PARTS.iter().enumerate() {
        let sym = src.join(format!("{mpn}.kicad_sym"));
        std::fs::write(&sym, SYMBOL.replace("NAME", mpn)).unwrap();
        let mut part = Part {
            mpn: mpn.to_string(),
            manufacturer: maker.to_string(),
            description: format!("Test part {mpn}"),
            symbol_file: Some(sym),
            ..Default::default()
        };
        if i < 2 {
            let fp = src.join(format!("{mpn}.kicad_mod"));
            std::fs::write(&fp, FOOTPRINT).unwrap();
            part.footprint_files = vec![fp];
        }
        if i == 1 {
            let (step, pdf) = (src.join(format!("{mpn}.step")), src.join(format!("{mpn}.pdf")));
            std::fs::write(&step, "ISO-10303-21;").unwrap();
            std::fs::write(&pdf, "%PDF-1.4").unwrap();
            (part.model_files, part.datasheet_file) = (vec![step], Some(pdf));
        }
        root.add_part(&part, cat, false).unwrap();
    }
    // What the side lists on the Search tab show.
    let store = klm_core::store::Store::open(&klm_core::store::Store::default_path().unwrap()).unwrap();
    for (mpn, _, _) in PARTS {
        store.log("import", mpn).unwrap();
    }
    store.set_favourite("RP2040", true).unwrap();
    Env { _guard: guard, dir, lib, project }
}

pub type Gui<'a> = Harness<'a, App>;

/// The app, rendered offscreen at `size`, started on `tab`.
pub fn app<'a>(size: (f32, f32), tab: Tab) -> Gui<'a> {
    app_checked(size, tab, false)
}

/// `a11y`: also fail on widgets without an accessible name (egui_kittest's own check).
pub fn app_checked<'a>(size: (f32, f32), tab: Tab, a11y: bool) -> Gui<'a> {
    let mut h = Harness::builder()
        .with_size(eframe::egui::vec2(size.0, size.1))
        .allow_missing_glyphs()
        .with_accessibility_check(a11y)
        .wgpu()
        .build_eframe(|cc| App::new(&cc.egui_ctx, None));
    h.state_mut().cx.tab = tab;
    h.state_mut().log_open = true; // the tests read the log; the real app starts with it collapsed into the status bar
    h.run_ok();
    h
}

pub fn snap_dir() -> PathBuf {
    // target/ui-snapshots next to the workspace's target dir
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-snapshots");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Characters in any label or value on screen that no installed font can draw (they show as tofu).
pub fn missing_glyphs(h: &Gui) -> std::collections::BTreeSet<char> {
    use egui_kittest::kittest::NodeT;
    let texts: Vec<String> = h
        .root()
        .children_recursive()
        .flat_map(|n| {
            let a = n.accesskit_node();
            [a.label(), a.value()].into_iter().flatten().collect::<Vec<_>>()
        })
        .collect();
    h.ctx.fonts_mut(|f| {
        let id = eframe::egui::FontId::proportional(14.0);
        texts.iter().flat_map(|t| t.chars()).filter(|c| !c.is_control() && !f.has_glyph(&id, *c)).collect()
    })
}

/// Settles the UI (worker threads included), saves `<name>-<w>x<h>.png` and returns the glyphs missing on screen.
pub fn snap(h: &mut Gui, name: &str) -> std::collections::BTreeSet<char> {
    settle(h);
    let size = h.ctx.content_rect().size();
    let img = h.render().expect("offscreen render");
    img.save(snap_dir().join(format!("{name}-{}x{}.png", size.x as u32, size.y as u32))).unwrap();
    missing_glyphs(h)
}

/// Lets background threads (part checks, scans) deliver, then runs until the UI is quiet.
pub fn settle(h: &mut Gui) {
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        h.step();
    }
    h.run_ok();
}

pub fn click(h: &mut Gui, label: &str) {
    h.get_all_by_label(label).next().unwrap().click();
    h.run_ok();
}

/// A distributor that fails every call and counts them: the GUI must not reach a source during these tests.
pub static SOURCE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct Probe;

impl klm_core::sources::Source for Probe {
    fn name(&self) -> &'static str {
        "Probe"
    }
    fn search(&self, _: &str) -> anyhow::Result<Vec<Part>> {
        SOURCE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        anyhow::bail!("no network in tests")
    }
    fn fetch(&self, _: &Part, _: &Path) -> anyhow::Result<Option<Part>> {
        SOURCE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        anyhow::bail!("no network in tests")
    }
}

/// Puts the `Probe` source in the app (so Search shows its full header) and redraws.
pub fn with_probe(h: &mut Gui) {
    h.state_mut().cx.sources = std::sync::Arc::new(vec![Box::new(Probe)]);
    h.run_ok();
}

#[allow(dead_code)] // debugging aid: print it to see what a test can click
/// Every named node on screen as `role "label" rect`, for working out what a test can click.
pub fn dump(h: &Gui) -> String {
    use egui_kittest::kittest::NodeT;
    h.root()
        .children_recursive()
        .filter_map(|n| {
            let a = n.accesskit_node();
            let label = a.label().or_else(|| a.value())?;
            Some(format!("{:?} {:?} {:?}", a.role(), label, n.rect()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}
