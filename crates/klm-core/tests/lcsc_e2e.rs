//! Converts recorded EasyEDA data (offline) and, when the network and KiCad are available, fetches
//! C2040 live, imports it into temp libraries and has KiCad render it.

use klm_core::convert::easyeda;
use klm_core::library::LibraryRoot;
use klm_core::sources::{Source, lcsc::Lcsc};
use klm_core::{kicad_env, sexpr};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fresh(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("klm-e2e").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fixture(id: &str) -> easyeda::Converted {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/easyeda_{id}.json"))).unwrap();
    easyeda::convert(&serde_json::from_str(&text).unwrap()).unwrap()
}

fn kicad_cli() -> Option<PathBuf> {
    kicad_env::kicad_cli().ok().filter(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success()))
}

fn export_svg(cli: &Path, kind: &str, input: &Path, out: &Path) {
    let r = Command::new(cli).args([kind, "export", "svg", "-o"]).arg(out).arg(input).output().unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
}

/// Counts of the drawn things in a footprint/symbol, to catch a converter that silently drops shapes.
fn count(x: &sexpr::Sexpr, head: &str) -> usize {
    x.children(head).count()
}

#[test]
fn recorded_parts_convert_to_the_expected_pins_and_pads() {
    for (id, pins, pads, name) in [("C25804", 2, 2, "R0603"), ("C7950", 8, 8, "SOIC-8_L5.0-W4.0-P1.27-LS6.0-BL"), ("C2040", 57, 57, "LQFN-56_L7.0-W7.0-P0.4-EP")] {
        let c = fixture(id);
        assert_eq!(c.footprint_name, name);
        let sym = sexpr::parse(&c.symbol).unwrap();
        let unit = sym.child("symbol").unwrap().child("symbol").unwrap();
        assert_eq!(count(unit, "pin"), pins, "{id} pins");
        let fp = sexpr::parse(&c.footprint).unwrap();
        assert_eq!(count(&fp, "pad"), pads, "{id} pads");
        assert!(c.model_uuid.is_some() && fp.child("model").is_some(), "{id} model");
    }
}

#[test]
fn c2040_live_import_renders_in_kicad() {
    let Some(cli) = kicad_cli() else {
        eprintln!("kicad-cli not found; skipping");
        return;
    };
    let lcsc = Lcsc::new();
    let found = match lcsc.search("C2040") {
        Ok(f) => f,
        Err(e) => {
            eprintln!("offline? skipping: {e:#}");
            return;
        }
    };
    let part = found.into_iter().find(|p| p.supplier_ids[0].1 == "C2040").expect("C2040 in the results");
    assert_eq!(part.mpn, "RP2040");
    let work = fresh("lcsc-work");
    let part = lcsc.fetch(&part, &work).unwrap().expect("LCSC has CAD");
    assert!(part.symbol_file.is_some() && part.footprint_files.len() == 1);
    assert!(part.model_files.len() == 1, "STEP model");
    assert!(part.datasheet_file.is_some(), "datasheet PDF");

    let root = LibraryRoot::global(fresh("lcsc-lib"), fresh("lcsc-config"));
    root.add_part(&part, "ICs", false).unwrap();
    let status = &root.list_parts()[0];
    use klm_core::library::Check;
    assert_eq!([status.footprint, status.model, status.datasheet], [Check::Ok; 3]);

    let out = fresh("lcsc-svg");
    export_svg(&cli, "sym", &root.dir.join("symbols/ICs.kicad_sym"), &out);
    export_svg(&cli, "fp", &root.dir.join("footprints/ICs.pretty"), &out);
    // Left in place for looking at: <temp>/klm-e2e/lcsc-svg
    assert!(std::fs::read_dir(&out).unwrap().count() >= 2);
}
