//! Vendor CAD ZIPs (SnapMagic, Ultra Librarian, SamacSys, Octopart). They differ in folder
//! layout but not in file types, so files are picked by type rather than per vendor.

use crate::library::property;
use crate::part::Part;
use crate::sexpr;
use anyhow::{Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_ENTRY: u64 = 200 * 1024 * 1024;
const SIM_EXTS: &[&str] = &["cir", "sub", "sp", "spi", "ckt", "mod", "ibs"];

/// Reads a ZIP and extracts the part's files into `work`. `Ok(None)` means it holds no KiCad files at all;
/// a ZIP with only footprints or 3D models gives a part without `symbol_file`, to be attached to an existing part.
pub fn read_zip(zip_path: &Path, work: &Path) -> Result<Option<Part>> {
    let file = std::fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).with_context(|| format!("reading {}", zip_path.display()))?;

    let (mut sym_new, mut sym_legacy, mut fps, mut steps, mut wrls, mut pdfs, mut sims) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    let mut part_info = String::new();

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() || entry.size() > MAX_ENTRY {
            continue;
        }
        let name = entry.name()?.into_owned();
        let path_in_zip = name.to_lowercase();
        // Only the file name is used for the destination, so a hostile "../" path can't escape `work`.
        let Some(file_name) = Path::new(&name).file_name().map(|n| n.to_owned()) else { continue };
        let ext = Path::new(&path_in_zip).extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
        let wanted = matches!(ext.as_str(), "kicad_sym" | "lib" | "kicad_mod" | "step" | "stp" | "wrl" | "stl" | "pdf")
            || SIM_EXTS.contains(&ext.as_str())
            || path_in_zip.ends_with("part_info.txt");
        if !wanted {
            continue;
        }
        let mut bytes = vec![];
        entry.read_to_end(&mut bytes)?;
        if path_in_zip.ends_with("part_info.txt") {
            part_info = String::from_utf8_lossy(&bytes).into_owned();
            continue;
        }
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_lowercase();
        let bucket = match ext.as_str() {
            "kicad_sym" => &mut sym_new,
            "lib" if head.starts_with("eeschema-library") => &mut sym_legacy,
            // .lib is also used by SPICE and other ECAD tools; only SPICE contains .subckt/.model.
            "lib" if is_spice(&bytes) => &mut sims,
            "lib" => continue,
            "kicad_mod" => &mut fps,
            "step" | "stp" => &mut steps,
            "wrl" | "stl" => &mut wrls,
            "pdf" => &mut pdfs,
            _ => &mut sims,
        };
        let dest = work.join(i.to_string());
        std::fs::create_dir_all(&dest)?;
        let dest = dest.join(file_name);
        std::fs::write(&dest, &bytes)?;
        bucket.push((path_in_zip, dest));
    }

    // Prefer the copy in a "kicad" folder when a ZIP carries several versions.
    let pick = |v: &mut Vec<(String, PathBuf)>| {
        v.sort_by_key(|(p, _)| !p.contains("kicad"));
        v.first().map(|(_, f)| f.clone())
    };
    let symbol_file = pick(&mut sym_new).or_else(|| pick(&mut sym_legacy));
    if symbol_file.is_none() && fps.is_empty() && steps.is_empty() && wrls.is_empty() && sims.is_empty() {
        return Ok(None);
    }

    let info = |key: &str| {
        part_info
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('=').map(|v| v.trim().to_string()))
            .unwrap_or_default()
    };
    let mut part = Part {
        mpn: info("PartNumber"),
        manufacturer: info("Manufacturer"),
        category_hint: info("PartCategory"),
        source: format!("ZIP: {}", zip_path.file_name().unwrap_or_default().to_string_lossy()),
        footprint_files: fps.into_iter().map(|(_, f)| f).collect(),
        model_files: steps.into_iter().chain(wrls).map(|(_, f)| f).collect(),
        sim_files: sims.into_iter().map(|(_, f)| f).collect(),
        ..Default::default()
    };

    // Fill gaps from the symbol's own fields (vendors use different field names).
    if let Some(sym) = symbol_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| sexpr::parse(&t).ok()) {
        if let Some(s) = sym.child("symbol") {
            let field = |keys: &[&str]| keys.iter().find_map(|k| property(s, k)).filter(|v| !v.trim().is_empty()).map(String::from);
            if part.mpn.is_empty() {
                part.mpn = field(&["MPN", "MP", "Manufacturer_Part_Number", "PARTNO"]).unwrap_or_else(|| s.arg(0).unwrap_or_default().into());
            }
            if part.manufacturer.is_empty() {
                part.manufacturer = field(&["Manufacturer", "MANUFACTURER", "MF", "Manufacturer_Name"]).unwrap_or_default();
            }
            part.description = field(&["Description", "ki_description", "DESCRIPTION"])
                .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            part.datasheet_url = field(&["Datasheet"]).filter(|d| d.starts_with("http")).unwrap_or_default();
        }
    }
    if part.mpn.is_empty() {
        let any = symbol_file.as_ref().or(part.footprint_files.first()).or(part.model_files.first()).or(part.sim_files.first());
        part.mpn = any.and_then(|f| f.file_stem()).unwrap_or_default().to_string_lossy().into();
    }
    if part.model_files.iter().any(|m| m.extension().is_some_and(|e| e.eq_ignore_ascii_case("stl"))) {
        part.warnings.push("STL 3D models are not shown by KiCad's 3D viewer (it reads STEP and WRL); the file is kept with the part".into());
    }
    if symbol_file.is_none() {
        part.warnings.push("no symbol in this ZIP: attach its footprint/3D/SPICE model to an existing part".into());
    }
    if part.category_hint.is_empty() {
        part.category_hint = part.description.clone();
    }
    let mpn_lower = part.mpn.to_lowercase();
    part.datasheet_file = pdfs
        .into_iter()
        .map(|(_, f)| f)
        .find(|f| {
            // File name only: vendors put the MPN in folder names too.
            let name = f.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            name.contains("datasheet") || (!mpn_lower.is_empty() && name.contains(&mpn_lower))
        });
    part.symbol_file = symbol_file;
    Ok(Some(part))
}

/// Other tools' library files in a ZIP (Eagle `.lbr`, Altium `.SchLib`/`.PcbLib`/`.IntLib`), extracted into
/// `dir` for `foreign::scan`. Also says whether an Altium `.LibPkg` project was seen (it holds no library data itself).
pub fn extract_foreign(zip_path: &Path, dir: &Path) -> Result<(usize, bool)> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?)?;
    let (mut n, mut libpkg) = (0, false);
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name()?.into_owned();
        let ext = Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        libpkg |= ext == "libpkg";
        let Some(file_name) = Path::new(&name).file_name().filter(|_| !entry.is_dir() && entry.size() <= MAX_ENTRY && ["lbr", "schlib", "pcblib", "intlib"].contains(&ext.as_str())) else { continue };
        let dest = dir.join(i.to_string());
        std::fs::create_dir_all(&dest)?;
        let mut bytes = vec![];
        entry.read_to_end(&mut bytes)?;
        std::fs::write(dest.join(file_name), bytes)?;
        n += 1;
    }
    Ok((n, libpkg))
}

fn is_spice(bytes: &[u8]) -> bool {
    crate::spice::is_spice_text(&String::from_utf8_lossy(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn samacsys_layout() {
        let dir = crate::test_dir("zip_samacsys");
        let zp = dir.join("LIB_X.zip");
        make_zip(
            &zp,
            &[
                ("X1/part_info.txt", "Manufacturer=ACME\nPartNumber=X1-100\nPartCategory=Resistor\n"),
                ("X1/KiCad/X1.lib", "EESCHEMA-LIBRARY Version 2.3\n"),
                ("X1/KiCad/X1.kicad_sym", r#"(kicad_symbol_lib (symbol "X1-100"))"#),
                ("X1/KiCad/RESC1005.kicad_mod", "(footprint RESC1005)"),
                ("X1/CADSTAR/X1.lib", "cadstar stuff"),
                ("X1/3D/X1.stp", "ISO-10303-21;"),
                ("X1-100/How_To_Use_Models.pdf", "%PDF"),
                ("../../evil.kicad_mod", "(footprint EVIL)"),
            ],
        );
        let work = dir.join("work");
        let p = read_zip(&zp, &work).unwrap().unwrap();
        assert_eq!((p.mpn.as_str(), p.manufacturer.as_str(), p.category_hint.as_str()), ("X1-100", "ACME", "Resistor"));
        assert!(p.symbol_file.unwrap().extension().unwrap() == "kicad_sym");
        assert_eq!(p.footprint_files.len(), 2);
        assert!(p.footprint_files.iter().all(|f| f.starts_with(&work)));
        assert_eq!(p.model_files.len(), 1);
        assert!(p.datasheet_file.is_none(), "instructions PDF is not a datasheet");
        assert!(p.sim_files.is_empty(), "CADSTAR .lib is not SPICE");
    }

    #[test]
    fn snapmagic_layout_reads_fields() {
        let dir = crate::test_dir("zip_snap");
        let zp = dir.join("MMC.zip");
        make_zip(
            &zp,
            &[
                ("MMC/MMC.kicad_sym", r#"(kicad_symbol_lib (symbol "MMC" (property "MF" "Memsic") (property "MP" "MMC5983MA")))"#),
                ("MMC/MMC5983MA_datasheet.pdf", "%PDF"),
                ("MMC/model.lib", ".SUBCKT MMC 1 2\n.ENDS"),
            ],
        );
        let p = read_zip(&zp, &dir.join("work")).unwrap().unwrap();
        assert_eq!((p.mpn.as_str(), p.manufacturer.as_str()), ("MMC5983MA", "Memsic"));
        assert!(p.datasheet_file.is_some());
        assert_eq!(p.sim_files.len(), 1);
    }

    #[test]
    fn footprint_only_and_stl_zips_are_accepted() {
        let dir = crate::test_dir("zip_fp_only");
        let zp = dir.join("fp.zip");
        make_zip(&zp, &[("A/R1.kicad_mod", "(footprint R1)"), ("A/R1.stl", "solid x"), ("A/x.LibPkg", "p")]);
        let p = read_zip(&zp, &dir.join("work")).unwrap().unwrap();
        assert!(p.symbol_file.is_none());
        assert_eq!((p.footprint_files.len(), p.model_files.len(), p.mpn.as_str()), (1, 1, "R1"));
        assert!(p.warnings.iter().any(|w| w.contains("STL")));
        let (n, libpkg) = extract_foreign(&zp, &dir.join("f")).unwrap();
        assert_eq!((n, libpkg), (0, true));
    }

    #[test]
    fn unrelated_zip_is_ignored() {
        let dir = crate::test_dir("zip_other");
        let zp = dir.join("photos.zip");
        make_zip(&zp, &[("a.jpg", "x"), ("readme.pdf", "%PDF")]);
        assert!(read_zip(&zp, &dir.join("work")).unwrap().is_none());
    }
}
