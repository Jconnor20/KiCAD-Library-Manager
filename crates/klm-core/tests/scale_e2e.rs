//! Scale and KiCad-open checks. The library is written straight to disk in KiCad's format, because
//! `add_part` runs `kicad-cli` once per part and 2,000 of those would take minutes. Everything is then
//! read back through the real `LibraryRoot`, `checks` and `alternatives` functions. The whole binary runs
//! in one sandbox (`KLM_CONFIG_DIR`), so KiCad's tables and KLM's queues never reach the real `%APPDATA%`.

use klm_core::library::{Check, LibraryRoot};
use klm_core::{alternatives, checks, kicad_env, settings};
use klm_core::kicad_env::{PendingLib, PendingTable, TableOp};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::Instant;

const CATS: [&str; 8] = ["Resistors", "Capacitors", "Inductors", "ICs", "Diodes", "Transistors", "Connectors", "Crystals"];
const PKGS: [&str; 4] = ["R_0603", "C_0805", "SOT-23", "SOIC-8"];
const VALUES: [&str; 6] = ["10k", "100nF", "1uH", "LM358", "BAT54", "2N7002"];
const PARTS: usize = 2000;

/// One sandbox for every test in this binary, set once before any test reads it.
fn sandbox() -> PathBuf {
    static ONCE: Once = Once::new();
    let d = std::env::temp_dir().join(format!("klm-scale-sandbox-{}", std::process::id()));
    ONCE.call_once(|| {
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        settings::set_sandbox(&d);
    });
    d
}

fn fresh(name: &str) -> PathBuf {
    let d = sandbox().join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn symbol(mpn: &str, value: &str, footprint: &str, datasheet: &str) -> String {
    format!(
        r#"  (symbol "{mpn}" (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "Value" "{value}" (id 1) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Footprint" "{footprint}" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "{datasheet}" (id 3) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "MPN" "{mpn}" (id 4) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Manufacturer" "ACME" (id 5) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Description" "Synthetic part {mpn}" (id 6) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "{mpn}_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "1" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 5.08 0 180) (length 2.54) (name "2" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27))))))
  )
"#
    )
}

fn footprint(pkg: &str, model: &str) -> String {
    format!(
        r#"(footprint "{pkg}" (layer "F.Cu")
  (fp_rect (start -1.6 -0.9) (end 1.6 0.9) (stroke (width 0.05) (type default)) (fill none) (layer "F.CrtYd"))
  (fp_line (start -2.2 -0.9) (end -2.2 0.9) (stroke (width 0.12) (type solid)) (layer "F.SilkS"))
  (pad "1" smd rect (at -0.8 0) (size 0.9 0.95) (layers "F.Cu" "F.Mask" "F.Paste"))
  (pad "2" smd rect (at 0.8 0) (size 0.9 0.95) (layers "F.Cu" "F.Mask" "F.Paste"))
  (model "{model}" (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 0)))
)
"#
    )
}

/// A global library of `parts` parts in `dir/lib`. One part in ten has no footprint, one in four no
/// datasheet. Footprints and models are shared by package, as they are in a real library.
fn seed(dir: &Path, parts: usize) -> LibraryRoot {
    let lib = dir.join("lib");
    let root = LibraryRoot::global(lib.clone(), kicad_env::config_dir().unwrap());
    root.ensure_layout().unwrap();
    let var = lib.to_string_lossy().replace('\\', "/");
    std::fs::write(lib.join("3dmodels/model.step"), "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n").unwrap();
    for cat in CATS {
        std::fs::write(lib.join("datasheets").join(format!("{cat}.pdf")), "%PDF-1.4\n").unwrap();
        std::fs::create_dir_all(lib.join("footprints").join(format!("{cat}.pretty"))).unwrap();
        for pkg in PKGS {
            let model = format!("{var}/3dmodels/model.step");
            std::fs::write(lib.join("footprints").join(format!("{cat}.pretty")).join(format!("{pkg}.kicad_mod")), footprint(pkg, &model)).unwrap();
        }
    }
    let mut libs: Vec<String> = CATS.iter().map(|_| SYMBOL_HEAD.to_string()).collect();
    for i in 0..parts {
        let cat = CATS[i % CATS.len()];
        let pkg = PKGS[(i / CATS.len()) % PKGS.len()];
        let value = VALUES[(i / 32) % VALUES.len()];
        let mpn = format!("PN{i:05}");
        let fp = if i % 10 == 9 { String::new() } else { format!("KLM_{cat}:{pkg}") };
        let ds = if i % 4 == 3 { String::new() } else { format!("{var}/datasheets/{cat}.pdf") };
        libs[i % CATS.len()] += &symbol(&mpn, value, &fp, &ds);
    }
    for (cat, text) in CATS.iter().zip(libs) {
        std::fs::write(lib.join("symbols").join(format!("{cat}.kicad_sym")), text + ")\n").unwrap();
    }
    root
}

const SYMBOL_HEAD: &str = "(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)\n";

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Runs `f` three times, checks every run gives the same answer, and returns the median time in ms.
fn measure(name: &str, f: impl Fn() -> usize) -> (String, f64, usize) {
    let mut ms = vec![];
    let mut answer = None;
    for _ in 0..3 {
        let t = Instant::now();
        let n = std::hint::black_box(f());
        ms.push(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(*answer.get_or_insert(n), n, "{name} gave different answers");
    }
    (name.to_string(), median(ms), answer.unwrap())
}

#[test]
fn two_thousand_parts_are_listed_checked_and_grouped() {
    let root = seed(&fresh("fast"), PARTS);

    let parts = root.list_parts();
    assert_eq!(parts.len(), PARTS);
    assert_eq!(parts.iter().filter(|p| p.footprint == Check::Missing).count(), PARTS / 10);
    assert_eq!(parts.iter().filter(|p| p.datasheet == Check::Missing).count(), PARTS / 4);
    assert_eq!(parts.iter().filter(|p| p.footprint == Check::Ok && p.model == Check::Ok).count(), PARTS - PARTS / 10);

    let issues = checks::check_library(&root);
    assert_eq!(issues.len(), PARTS);
    let no_fp = parts.iter().find(|p| p.footprint == Check::Missing).unwrap();
    let flagged = &issues[&(no_fp.category.clone(), no_fp.name.clone())];
    assert!(flagged.iter().any(|i| i.message.contains("Footprint")), "missing footprint not flagged: {flagged:?}");

    let alt = alternatives::scan(&root);
    assert_eq!(alt.len(), PARTS);
    assert!(!alternatives::suggest(&alt).is_empty(), "shared packages and values should give groups");
}

/// Rejections loaded (a thousand neighbour pairs plus the suggested groups' first pairs) must not make the filtered scan slow.
#[test]
fn two_thousand_parts_with_rejections_still_suggest_fast() {
    let root = seed(&fresh("rejections"), PARTS);
    let parts = alternatives::scan(&root);
    let store = klm_core::store::Store::open(&fresh("rejections-db").join("parts.db")).unwrap();
    let groups = alternatives::suggest(&parts);
    let mut n = 0;
    for g in groups.iter().take(1000) {
        alternatives::reject(&store, &[&parts[g[0]], &parts[g[1]]]).unwrap();
        n += 1;
    }
    assert!(n > 0);
    for w in parts.windows(2).take(1000) {
        alternatives::reject(&store, &[&w[0], &w[1]]).unwrap();
        n += 1;
    }
    let t = Instant::now();
    let left = alternatives::suggest_unrejected(&store, &parts).unwrap();
    let ms = t.elapsed().as_secs_f64() * 1e3;
    println!("suggest_unrejected, {PARTS} parts, {n} rejections: {ms:.1} ms");
    assert!(left.len() <= groups.len());
    assert!(ms < 5000.0, "took {ms} ms");
}

/// Timings for the Library tab and the Alternatives scan, on the release build. Not run by default:
/// `cargo test --release -- --ignored scale --nocapture`.
#[test]
#[ignore = "timing: cargo test --release -- --ignored scale --nocapture"]
fn scale_timings_for_2000_parts() {
    let root = seed(&fresh("timings"), PARTS);
    let rows = [
        measure("list_parts", || root.list_parts().len()),
        measure("check_library", || checks::check_library(&root).len()),
        measure("alternatives scan + suggest", || alternatives::suggest(&alternatives::scan(&root)).len()),
    ];
    println!("| operation | median ms (3 runs) | answer |");
    println!("|---|---|---|");
    for (name, ms, answer) in rows {
        println!("| {name} | {ms:.1} | {answer} |");
    }
}

// ---- KiCad-open: the pending queue against a KiCad table that changed in between ----

const TABLE: &str = "(sym_lib_table\n  (version 7)\n  (lib (name \"Device\")(type \"KiCad\")(uri \"${KICAD10_SYMBOL_DIR}/Device.kicad_sym\")(options \"\")(descr \"\"))\n)\n";

/// What a user or KiCad adds to the table while KLM's entry waits in the queue.
fn outside_edit(table: &Path, name: &str) {
    let text = std::fs::read_to_string(table).unwrap();
    let close = text.rfind(')').unwrap();
    let entry = format!("  (lib (name \"{name}\")(type \"KiCad\")(uri \"/elsewhere/{name}.kicad_sym\")(options \"\")(descr \"\"))\n");
    std::fs::write(table, format!("{}{entry}{}", &text[..close], &text[close..])).unwrap();
}

/// The two replay tests share one table and queue, so they take turns.
static KICAD_LOCK: Mutex<()> = Mutex::new(());

/// The sandbox's own KiCad 10 table and the queue beside it, reset to the starting state.
fn kicad_tables() -> (MutexGuard<'static, ()>, PathBuf, PathBuf) {
    let lock = KICAD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    sandbox();
    let table = kicad_env::config_dir().unwrap().join("sym-lib-table");
    std::fs::write(&table, TABLE).unwrap();
    let queue = kicad_env::default_queue_path().unwrap();
    assert!(queue.starts_with(sandbox()), "queue must be in the sandbox: {}", queue.display());
    let _ = std::fs::remove_file(&queue);
    let _ = std::fs::remove_file(queue.with_file_name("pending-tables.json"));
    (lock, table, queue)
}

#[test]
fn queued_library_survives_an_outside_table_edit() {
    let (_lock, table, queue) = kicad_tables();
    kicad_env::enqueue_in(&queue, PendingLib { table: table.clone(), nickname: "KLM_ICs".into(), uri: "x/ICs.kicad_sym".into() }).unwrap();
    outside_edit(&table, "UserLib");

    assert_eq!(kicad_env::apply_in(&queue).unwrap(), 1);
    let text = std::fs::read_to_string(&table).unwrap();
    assert!(text.contains("\"UserLib\""), "outside edit lost:\n{text}");
    assert!(text.contains("\"KLM_ICs\""), "queued library not added:\n{text}");
    assert!(kicad_env::pending_in(&queue).is_empty());
}

/// Found while writing W6.4: a queued restore holds the whole old table, so applying it used to throw away
/// an outside edit. Fixed in `kicad_env::apply_table_op`, which now splices outside entries back in.
#[test]
fn queued_restore_keeps_an_outside_edit() {
    let (_lock, table, queue) = kicad_tables();
    // A snapshot restore queued while KiCad was open: the table as it was then.
    let restore = vec![PendingTable { table: table.clone(), op: TableOp::Restore(Some(TABLE.into())) }];
    std::fs::write(queue.with_file_name("pending-tables.json"), serde_json::to_string(&restore).unwrap()).unwrap();
    outside_edit(&table, "UserLib");

    kicad_env::apply_in(&queue).unwrap();
    let text = std::fs::read_to_string(&table).unwrap();
    assert!(text.contains("\"UserLib\""), "outside edit silently lost:\n{text}");
}

#[test]
fn queued_removal_keeps_an_outside_edit_and_drops_ours() {
    let (_lock, table, queue) = kicad_tables();
    kicad_env::register_lib(&table, "KLM_ICs", "x/ICs.kicad_sym").unwrap();
    let removal = vec![PendingTable { table: table.clone(), op: TableOp::Restore(None) }];
    std::fs::write(queue.with_file_name("pending-tables.json"), serde_json::to_string(&removal).unwrap()).unwrap();
    outside_edit(&table, "UserLib");

    kicad_env::apply_in(&queue).unwrap();
    let text = std::fs::read_to_string(&table).unwrap();
    assert!(text.contains("\"UserLib\"") && text.contains("\"Device\""), "outside entries lost:
{text}");
    assert!(!text.contains("KLM_ICs"), "our entry should be gone:
{text}");
}
