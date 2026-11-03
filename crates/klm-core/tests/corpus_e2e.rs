//! Imports real third-party files end to end: the same calls the Import tab makes (`read_zip`, `extract_foreign`,
//! `foreign::scan`, `batch::plan`/`apply`, `attach_zip`) into temp libraries, then `kicad-cli` renders the result.
//! SVGs are left in `target/corpus-renders/<id>/` for looking at. Files come from `testdata/` (git-ignored, see
//! docs/testdata-manifest.md); whatever is missing, or `kicad-cli` itself, is skipped with a message.
//! Each case asserts its outcome by name: imports and renders, or fails with the stated message.

use klm_core::batch::{self, Op, Target};
use klm_core::convert::easyeda;
use klm_core::library::LibraryRoot;
use klm_core::part::Part;
use klm_core::rules::Rules;
use klm_core::sources::{foreign, zip_import};
use klm_core::kicad_env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus(file: &str) -> Option<PathBuf> {
    let p = root().join("testdata").join(file);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("{} not found; skipping (see docs/testdata-manifest.md)", p.display());
        None
    }
}

fn cli() -> Option<PathBuf> {
    let c = kicad_env::kicad_cli().ok().filter(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success()));
    if c.is_none() {
        eprintln!("kicad-cli not found; skipping");
    }
    c
}

fn fresh(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("klm-corpus").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Where the renders of case `id` go.
fn renders(id: &str) -> PathBuf {
    let d = root().join("target/corpus-renders").join(id);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Runs `kicad-cli <sym|fp> export svg` and returns the non-empty SVGs it wrote.
fn export_svg(cli: &Path, kind: &str, input: &Path, out: &Path) -> Vec<PathBuf> {
    std::fs::create_dir_all(out).unwrap();
    let r = Command::new(cli).args([kind, "export", "svg", "-o"]).arg(out).arg(input).output().unwrap();
    assert!(r.status.success(), "{kind} export {}: {}{}", input.display(), String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr));
    let svgs: Vec<PathBuf> = std::fs::read_dir(out).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "svg")).collect();
    assert!(svgs.iter().all(|s| std::fs::metadata(s).unwrap().len() > 500), "empty svg in {}", out.display());
    svgs
}

/// What the Import tab does with a ZIP: the KiCad files if there are any, else the foreign converter.
fn app_zip(zip: &Path, work: &Path) -> Result<Vec<Part>, String> {
    let rules = Rules::default();
    match zip_import::read_zip(zip, work).map_err(|e| format!("{e:#}"))? {
        Some(part) if part.symbol_file.is_none() && part.footprint_files.is_empty() => {
            let dir = work.join("foreign");
            if zip_import::extract_foreign(zip, &dir).is_ok_and(|(n, _)| n > 0) {
                let mut scan = foreign::scan(&dir, &rules, &work.join("conv")).map_err(|e| format!("{e:#}"))?;
                for f in scan.parts.iter_mut().filter(|f| f.part.model_files.is_empty()) {
                    f.part.model_files = part.model_files.clone();
                }
                return Ok(scan.parts.into_iter().map(|f| f.part).collect());
            }
            Ok(vec![part])
        }
        Some(part) => Ok(vec![part]),
        None => match zip_import::extract_foreign(zip, &work.join("foreign")) {
            Ok((n, _)) if n > 0 => foreign::scan(&work.join("foreign"), &rules, &work.join("conv")).map(|s| s.parts.into_iter().map(|f| f.part).collect()).map_err(|e| format!("{e:#}")),
            Ok((_, true)) => Err("an Altium project (.LibPkg) with no library files; pick the .SchLib and .PcbLib files with Import library…".into()),
            _ => Err("no KiCad, Altium or Eagle library files inside, ignored".into()),
        },
    }
}

/// Imports `parts` into a fresh library and renders what was written: (symbol svgs, footprint svgs).
fn import_and_render(id: &str, cli: &Path, parts: Vec<Part>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let lib = LibraryRoot::global(fresh(&format!("{id}-lib")), fresh(&format!("{id}-config")));
    let ops = parts.into_iter().map(|part| Op::ImportPart { part, category: Some("Corpus".into()), target: Target::Global, overwrite: false }).collect();
    let plan = batch::plan(ops, Some(&lib), None);
    let results = batch::apply(&plan, |_, _| {});
    assert!(results.iter().all(|r| r.error.is_none()), "{id}: {:?}", results.iter().map(|r| &r.error).collect::<Vec<_>>());
    let out = renders(id);
    let sym = export_svg(cli, "sym", &lib.dir.join("symbols/Corpus.kicad_sym"), &out.join("sym"));
    let fp_dir = lib.dir.join("footprints/Corpus.pretty");
    let fp = if fp_dir.is_dir() && std::fs::read_dir(&fp_dir).unwrap().next().is_some() { export_svg(cli, "fp", &fp_dir, &out.join("fp")) } else { vec![] };
    (sym, fp)
}

fn scan_file(id: &str, file: &str) -> Option<(PathBuf, anyhow::Result<foreign::Scan>)> {
    let src = corpus(file)?;
    let (input, work) = (fresh(&format!("{id}-in")), fresh(&format!("{id}-work")));
    std::fs::copy(&src, input.join(src.file_name().unwrap())).unwrap();
    Some((work.clone(), foreign::scan(&input, &Rules::default(), &work)))
}

/// A converted single-file symbol library imports and renders; returns the scan for further checks.
fn assert_imports(id: &str, file: &str, take: usize) -> Option<(usize, usize)> {
    let cli = cli()?;
    let (_work, scan) = scan_file(id, file)?;
    let scan = scan.unwrap_or_else(|e| panic!("{id}: {e:#}"));
    assert!(scan.unsupported.is_empty(), "{id}: {:?}", scan.unsupported);
    assert!(!scan.parts.is_empty(), "{id}: no parts");
    let total = scan.parts.len();
    let parts: Vec<Part> = scan.parts.into_iter().take(take).map(|f| f.part).collect();
    let (sym, fp) = import_and_render(id, &cli, parts);
    assert!(!sym.is_empty(), "{id}: no symbol rendered");
    Some((total, fp.len()))
}

#[test]
fn c01_altium_schlib_battery() {
    if let Some((n, _)) = assert_imports("C01", "corpus/nils_battery.SchLib", 3) {
        assert!(n >= 2, "battery.SchLib holds several symbols, got {n}");
    }
}

#[test]
fn c02_altium_schlib_mounting_holes() {
    assert_imports("C02", "corpus/kicad_mounting_holes.SchLib", 3);
}

#[test]
fn c03_altium_pcblib_alone_has_no_part_but_says_why() {
    let Some(cli) = cli() else { return };
    let Some((work, scan)) = scan_file("C03", "corpus/kicad_esp32_wroom.PcbLib") else { return };
    let msg = format!("{:#}", scan.err().expect("a PcbLib has no symbols, so no part"));
    assert!(msg.contains("footprint") && msg.contains("no symbol"), "{msg}");
    // The footprints it did convert are real and render.
    let pretty = work.join("conv0");
    let fps = std::fs::read_dir(&pretty).unwrap().flatten().find(|e| e.path().extension().is_some_and(|x| x == "pretty")).expect("converted .pretty").path();
    assert!(!export_svg(&cli, "fp", &fps, &renders("C03")).is_empty());
}

#[test]
fn c04_altium_intlib_blocked() {
    // No redistributable small .IntLib was found (only a 63 MB one); see docs/verification/W6.2-corpus.md.
    let Some(p) = corpus("corpus/any.IntLib") else { return };
    let (_w, scan) = scan_file("C04", p.file_name().unwrap().to_str().unwrap()).unwrap();
    eprintln!("{:?}", scan.map(|s| (s.parts.len(), s.unsupported)));
}

#[test]
fn c05_easyeda_pro_elibz2_is_reported_not_guessed() {
    let Some(_) = cli() else { return };
    let Some((_w, scan)) = scan_file("C05", "corpus/kicad_LS2K0300.elibz2") else { return };
    let msg = match scan {
        Ok(s) => s.unsupported.join("; "),
        Err(e) => format!("{e:#}"),
    };
    assert!(msg.contains("LS2K0300") && msg.contains("could not convert") && msg.contains("Unable to convert library"), "{msg}");
}

#[test]
fn c06_lcsc_easyeda_json_imports_and_renders() {
    let Some(cli) = cli() else { return };
    for (id, file, mpn, pins) in [("C06a", "corpus/lcsc_C8734.json", "STM32F103C8T6", 48), ("C06b", "corpus/lcsc_C14663.json", "CL05B104KO5NNNC", 2)] {
        let Some(path) = corpus(file) else { return };
        let data: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let c = easyeda::convert(&data, "corpus part", "").unwrap();
        // Same as `Lcsc::fetch` after the download; the STEP needs the network, so the model line is dropped as it is there.
        let work = fresh(&format!("{id}-work"));
        let sym = work.join("part.kicad_sym");
        std::fs::write(&sym, &c.symbol).unwrap();
        let fp = work.join(format!("{}.kicad_mod", c.footprint_name));
        std::fs::write(&fp, c.footprint.lines().filter(|l| !l.starts_with("(model ")).collect::<Vec<_>>().join("\n")).unwrap();
        let part = Part { mpn: mpn.into(), symbol_file: Some(sym), footprint_files: vec![fp], ..Default::default() };
        let (s, f) = import_and_render(id, &cli, vec![part]);
        assert!(!s.is_empty() && !f.is_empty(), "{id}");
        let n = c.symbol.matches("(pin ").count();
        assert_eq!(n, pins, "{id}: pins in the converted symbol");
    }
}

#[test]
fn c07_legacy_lib_symbols() {
    // 160 symbols of KiCad's pre-v6 Regulator_Linear.lib; three go through the library to keep the run short.
    if let Some((n, _)) = assert_imports("C07", "corpus/kicad_Regulator_Linear.lib", 3) {
        assert!(n > 50, "got {n}");
    }
}

#[test]
fn c08_ultra_librarian_zips() {
    let Some(cli) = cli() else { return };
    // (file, what it holds, expected parts, has footprint)
    for (id, file) in [("C08a", "ul_4515DO-DS3BK004DP (1).zip"), ("C08b", "ul_4515DO-DS3BK004DP (2).zip"), ("C08c", "ul_4515DO-DS3BK004DP (3).zip"), ("C08d", "ul_4515DO-DS3BK004DP.zip")] {
        let Some(zip) = corpus(file) else { return };
        let parts = app_zip(&zip, &fresh(&format!("{id}-zip"))).unwrap_or_else(|e| panic!("{id} {file}: {e}"));
        assert!(!parts.is_empty(), "{id}");
        assert!(parts.iter().all(|p| p.symbol_file.is_some() && !p.footprint_files.is_empty()), "{id}: every part needs a symbol and footprint");
        if id != "C08d" {
            // The fourth ZIP has no 3D model inside.
            assert!(parts.iter().all(|p| !p.model_files.is_empty()), "{id}: the loose .stl should ride along");
        }
        let (s, f) = import_and_render(id, &cli, parts);
        assert!(!s.is_empty() && !f.is_empty(), "{id}");
    }
}

#[test]
fn c09_stl_only_zip_attaches_to_an_existing_part() {
    let Some(cli) = cli() else { return };
    let Some(ul) = corpus("ul_4515DO-DS3BK004DP (1).zip") else { return };
    // The vendor's own STL, alone in a ZIP.
    let mut src = zip::ZipArchive::new(std::fs::File::open(ul).unwrap()).unwrap();
    let mut stl = vec![];
    for i in 0..src.len() {
        let mut e = src.by_index(i).unwrap();
        if e.name().unwrap().ends_with(".stl") {
            std::io::Read::read_to_end(&mut e, &mut stl).unwrap();
        }
    }
    assert!(!stl.is_empty());
    let zp = fresh("C09-zip").join("stl_only.zip");
    let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
    w.start_file("model/c-4515do.stl", zip::write::SimpleFileOptions::default()).unwrap();
    std::io::Write::write_all(&mut w, &stl).unwrap();
    w.finish().unwrap();

    let part = zip_import::read_zip(&zp, &fresh("C09-read")).unwrap().expect("STL counts as a model");
    assert!(part.symbol_file.is_none() && part.footprint_files.is_empty() && part.model_files.len() == 1);
    assert!(part.warnings.iter().any(|w| w.contains("STL")), "{:?}", part.warnings);

    // Attach it to a part that has a footprint (an LCSC one, as in C06).
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(corpus("corpus/lcsc_C14663.json").unwrap()).unwrap()).unwrap();
    let c = easyeda::convert(&data, "corpus part", "").unwrap();
    let work = fresh("C09-work");
    let (sym, fp) = (work.join("p.kicad_sym"), work.join(format!("{}.kicad_mod", c.footprint_name)));
    std::fs::write(&sym, &c.symbol).unwrap();
    std::fs::write(&fp, c.footprint.lines().filter(|l| !l.starts_with("(model ")).collect::<Vec<_>>().join("\n")).unwrap();
    let lib = LibraryRoot::global(fresh("C09-lib"), fresh("C09-config"));
    lib.add_part(&Part { mpn: "CL05B104KO5NNNC".into(), symbol_file: Some(sym), footprint_files: vec![fp], ..Default::default() }, "Corpus", false).unwrap();
    lib.attach_zip("Corpus", "CL05B104KO5NNNC", &zp).unwrap();
    let pretty = lib.dir.join("footprints/Corpus.pretty");
    let text = std::fs::read_dir(&pretty).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect::<String>();
    assert!(text.contains(".stl"), "footprint should point at the attached STL");
    assert!(!export_svg(&cli, "fp", &pretty, &renders("C09")).is_empty());
}

#[test]
fn altium_and_eagle_fixtures_convert() {
    if !cli().is_some() {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for (name, src) in [("sch", "third_party/altium_pipe.SchLib"), ("pcb", "third_party/altium_pipe.PcbLib"), ("lbr", "eagle_r0603.lbr")] {
        let (input, work) = (fresh(&format!("{name}-in")), fresh(&format!("{name}-work")));
        std::fs::copy(here.join(src), input.join(Path::new(src).file_name().unwrap())).unwrap();
        let scan = foreign::scan(&input, &Rules::default(), &work);
        if name == "pcb" {
            // Footprints only: converted, but there is no symbol to make a part from.
            assert!(scan.is_err(), "{src}");
            continue;
        }
        let scan = scan.unwrap();
        eprintln!("{src}: {} parts, unsupported {:?}, skipped {:?}", scan.parts.len(), scan.unsupported, scan.skipped);
        // PcbLib has footprints only, so it yields no parts but must not be reported as unreadable.
        assert!(!scan.parts.is_empty(), "{src} gave no parts: {:?}", scan.unsupported);
        assert!(scan.unsupported.is_empty(), "{src}: {:?}", scan.unsupported);
    }
}
