//! Runs the importers over `tests/fixtures/third_party` and the `testdata/` folder at the repo root (vendor
//! downloads that can't be redistributed, see docs/testdata-manifest.md). Skips what isn't there.

use klm_core::rules::Rules;
use klm_core::sources::{foreign, zip_import};
use klm_core::kicad_env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cli_works() -> bool {
    kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success()))
}

fn fresh(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("klm-corpus").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn altium_and_eagle_fixtures_convert() {
    if !cli_works() {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for (name, src) in [("sch", "third_party/altium_pipe.SchLib"), ("pcb", "third_party/altium_pipe.PcbLib"), ("lbr", "eagle_r0603.lbr")] {
        let (input, work) = (fresh(&format!("{name}-in")), fresh(&format!("{name}-work")));
        std::fs::copy(here.join(src), input.join(Path::new(src).file_name().unwrap())).unwrap();
        let scan = foreign::scan(&input, &Rules::default(), &work).unwrap();
        eprintln!("{src}: {} parts, unsupported {:?}, skipped {:?}", scan.parts.len(), scan.unsupported, scan.skipped);
        // PcbLib has footprints only, so it yields no parts but must not be reported as unreadable.
        assert!(!scan.parts.is_empty() || name == "pcb", "{src} gave no parts: {:?}", scan.unsupported);
        assert!(scan.unsupported.is_empty(), "{src}: {:?}", scan.unsupported);
    }
}

#[test]
fn downloaded_vendor_zips_are_readable() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("testdata/ not found; skipping (see docs/testdata-manifest.md)");
        return;
    };
    for e in rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("zip"))) {
        let work = fresh(&e.file_name().to_string_lossy());
        let kicad = zip_import::read_zip(&e.path(), &work.join("kicad")).unwrap();
        let (foreign_n, libpkg) = zip_import::extract_foreign(&e.path(), &work.join("foreign")).unwrap();
        eprintln!("{}: kicad part {:?}, {foreign_n} foreign files, libpkg {libpkg}", e.path().display(), kicad.as_ref().map(|p| &p.mpn));
        assert!(kicad.is_some() || foreign_n > 0 || libpkg, "{}: nothing importable inside", e.path().display());
    }
}
