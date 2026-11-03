//! Importing a whole library made elsewhere. Existing KiCad libraries (`.kicad_sym` or legacy `.lib`,
//! `.pretty` folders, 3D models) are split into one [`Part`] per symbol, each with its category from the
//! user's rules, its footprint and its 3D model found and ready for `add_part`, which rewrites the paths.
//!
//! Other tools' libraries (Eagle/Fusion `.lbr`, Altium `.SchLib`/`.PcbLib`/`.IntLib`, EasyEDA `.json`/`.epro`)
//! go through `kicad-cli sym upgrade` and `fp upgrade`, which pick KiCad's own importer from the file. That
//! is verified for Eagle; the others are tried the same way and, if `kicad-cli` refuses, reported in
//! [`Scan::unsupported`] rather than guessed. Nothing here writes to a library: feed the parts to
//! `batch::plan` (dry run) and `batch::apply` (one snapshot for the lot).

use crate::{categories, kicad_env};
use crate::library::{property, sanitize};
use crate::part::Part;
use crate::rules::Rules;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A part to import and the category chosen for it.
pub struct Found {
    pub part: Part,
    pub category: String,
}

#[derive(Default)]
pub struct Scan {
    pub parts: Vec<Found>,
    /// Things left out and why (derived symbols, duplicates, unreadable files).
    pub skipped: Vec<String>,
    /// Foreign-format files found, one line per format.
    pub unsupported: Vec<String>,
}

/// Files of another tool that `kicad-cli` may be able to convert.
fn is_foreign(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["lbr", "schlib", "pcblib", "intlib", "epro", "elib"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

const SKIP_DIRS: &[&str] = &[crate::snapshot::DIR, ".git", "node_modules"];

#[derive(Default)]
struct Files {
    syms: Vec<PathBuf>,
    legacy_syms: Vec<PathBuf>,
    /// Footprint name (lowercase) → (`.pretty` stem lowercase, file).
    fps: HashMap<String, Vec<(String, PathBuf)>>,
    /// File name (lowercase) → paths.
    models: HashMap<String, Vec<PathBuf>>,
    foreign: Vec<PathBuf>,
    /// Altium projects, which hold no library data themselves.
    libpkgs: Vec<PathBuf>,
}

fn walk(dir: &Path, f: &mut Files) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_symlink() {
            continue; // links and junctions can loop back on themselves
        }
        if kind.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, f);
            }
            continue;
        }
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
        let pretty = p.parent().and_then(|d| d.extension()).is_some_and(|x| x.eq_ignore_ascii_case("pretty"));
        match ext.as_str() {
            "kicad_sym" => f.syms.push(p),
            "lib" if std::fs::read(&p).is_ok_and(|b| String::from_utf8_lossy(&b[..b.len().min(64)]).to_lowercase().starts_with("eeschema-library")) => f.legacy_syms.push(p),
            "kicad_mod" if pretty => {
                let lib = p.parent().and_then(|d| d.file_stem()).unwrap_or_default().to_string_lossy().to_lowercase();
                let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
                f.fps.entry(stem).or_default().push((lib, p));
            }
            "step" | "stp" | "wrl" => f.models.entry(name.to_lowercase()).or_default().push(p),
            "libpkg" => f.libpkgs.push(p),
            _ if is_foreign(&p) => f.foreign.push(p),
            _ => {}
        }
    }
}

/// Reads every KiCad library under `root` into parts. Single-symbol library files are written under `work`
/// (which must outlive the parts' use). `rules` pick each part's category.
pub fn scan(root: &Path, rules: &Rules, work: &Path) -> Result<Scan> {
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    let mut files = Files::default();
    walk(root, &mut files);
    let mut out = Scan::default();
    out.unsupported.extend(files.libpkgs.iter().map(|p| format!("{}: an Altium project, not a library; pick the .SchLib and .PcbLib files next to it instead", p.display())));
    // Other tools' libraries: convert into `work`, then treat the result like any KiCad library.
    for (i, p) in std::mem::take(&mut files.foreign).iter().enumerate() {
        let (dir, stem) = (work.join(format!("conv{i}")), lib_stem(p));
        let _ = std::fs::create_dir_all(&dir);
        let sym = kicad_env::upgrade_symbols(p, &dir.join(format!("{stem}.kicad_sym")));
        let fp = kicad_env::upgrade_footprints(p, &dir.join(format!("{stem}.pretty")));
        if sym.is_err() && fp.is_err() {
            out.unsupported.push(format!("{}: kicad-cli could not convert it. kicad-cli said: {}; {}", p.display(), first_line(&sym.unwrap_err()), first_line(&fp.unwrap_err())));
        } else if sym.is_err() && !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pcblib")) {
            out.unsupported.push(format!("{}: footprints converted but it has no symbols kicad-cli can read", p.display()));
        }
        walk(&dir, &mut files);
    }

    let mut libs: Vec<(PathBuf, String)> = files.syms.iter().map(|p| (p.clone(), lib_stem(p))).collect();
    for (i, p) in files.legacy_syms.iter().enumerate() {
        let up = work.join(format!("legacy{i}.kicad_sym"));
        match kicad_env::upgrade_symbols(p, &up) {
            Ok(()) => libs.push((up, lib_stem(p))),
            Err(e) => out.skipped.push(format!("{}: {e:#}", p.display())),
        }
    }
    if libs.is_empty() && out.unsupported.is_empty() {
        bail!("no KiCad symbol libraries (.kicad_sym) found in {}", root.display());
    }

    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (n, (path, stem)) in libs.iter().enumerate() {
        let lib = match std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|t| sexpr::parse(&t)) {
            Ok(l) => l,
            Err(e) => {
                out.skipped.push(format!("{}: {e:#}", path.display()));
                continue;
            }
        };
        for (k, sym) in lib.children("symbol").enumerate() {
            let name = sym.arg(0).unwrap_or_default().to_string();
            if sym.child("power").is_some() {
                out.skipped.push(format!("{stem}:{name}: power symbol, not a part"));
                continue;
            }
            let Some(sym) = &flatten(&lib, sym) else {
                out.skipped.push(format!("{stem}:{name}: its parent symbol is missing"));
                continue;
            };
            let mut part = part_from(sym, stem, &files, root);
            // A file named for a category ("Resistors", "Connector_Generic") sorts all of its parts.
            let by_file = rules.rule_category(stem).map(String::from).or_else(|| Some(categories::guess(stem)).filter(|c| *c != "Misc").map(String::from));
            let category = by_file.unwrap_or_else(|| rules.category_for(&part));
            // One flat single-symbol library per part: add_part takes exactly one symbol.
            let single = Sexpr::List(lib.items().iter().filter(|c| c.head() != Some("symbol")).cloned().chain([sym.clone()]).collect());
            let dir = work.join(format!("sym{n}_{k}"));
            std::fs::create_dir_all(&dir)?;
            let file = dir.join(format!("{}.kicad_sym", sanitize(&name)));
            std::fs::write(&file, sexpr::to_string(&single)).with_context(|| format!("writing {}", file.display()))?;
            part.symbol_file = Some(file);
            if !seen.insert((sanitize(&category), sanitize(&part.mpn).to_lowercase())) {
                out.skipped.push(format!("{stem}:{name}: {} appears twice in {category}, kept the first", part.mpn));
                continue;
            }
            out.parts.push(Found { part, category });
        }
    }
    Ok(out)
}

fn first_line(e: &anyhow::Error) -> String {
    format!("{e:#}").lines().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string()
}

fn lib_stem(p: &Path) -> String {
    p.file_stem().unwrap_or_default().to_string_lossy().into()
}

fn part_from(sym: &Sexpr, lib: &str, files: &Files, root: &Path) -> Part {
    let name = sym.arg(0).unwrap_or_default();
    let field = |keys: &[&str]| keys.iter().find_map(|k| property(sym, k)).map(str::trim).filter(|v| !v.is_empty()).map(String::from);
    let mpn = field(&["MPN", "Manufacturer_Part_Number", "PARTNO", "MP"]).unwrap_or_else(|| name.to_string());
    let description = field(&["Description", "ki_description"]).unwrap_or_default();
    let keywords = field(&["ki_keywords"]).unwrap_or_default();
    let fp_ref = property(sym, "Footprint").unwrap_or("").trim();
    let (fp_nick, fp_name) = fp_ref.split_once(':').unwrap_or(("", fp_ref));
    let mut part = Part {
        mpn,
        manufacturer: field(&["Manufacturer", "MANUFACTURER", "MF", "Manufacturer_Name"]).unwrap_or_default(),
        category_hint: format!("{lib} {keywords} {description} {name}"),
        package: fp_name.to_string(),
        datasheet_url: field(&["Datasheet"]).filter(|d| d.starts_with("http")).unwrap_or_default(),
        source: format!("KiCad library: {lib}"),
        description,
        supplier_ids: ["LCSC", "DigiKey", "Mouser", "Farnell"].iter().filter_map(|k| Some((k.to_string(), field(&[k])?))).collect(),
        ..Part::default()
    };
    if fp_name.is_empty() {
        return part;
    }
    let Some(cands) = files.fps.get(&fp_name.to_lowercase()) else {
        part.warnings.push(format!("footprint {fp_ref} not found in the folder"));
        return part;
    };
    let exact: Vec<_> = cands.iter().filter(|(l, _)| l.eq_ignore_ascii_case(fp_nick)).collect();
    if exact.len() != 1 {
        part.warnings.push(format!("footprint {fp_ref}: {} folders have a footprint of that name, check the one used", exact.len().max(cands.len())));
    }
    let fp = exact.first().copied().unwrap_or(&cands[0]).1.clone();
    match find_model(&fp, files, root) {
        Some(models) => part.model_files = models,
        None if std::fs::read_to_string(&fp).is_ok_and(|t| t.contains("(model")) => part.warnings.push(format!("3D model of {fp_name} not found; imported without it")),
        None => {}
    }
    part.footprint_files = vec![fp];
    part
}

/// `sym` with its `extends` parent's drawing merged in (properties stay the child's), following chains.
fn flatten(lib: &Sexpr, sym: &Sexpr) -> Option<Sexpr> {
    let Some(parent) = sym.child("extends").and_then(|e| e.arg(0)).map(String::from) else { return Some(sym.clone()) };
    let base = flatten(lib, lib.children("symbol").find(|s| s.arg(0) == Some(&parent))?)?;
    let (name, mut out) = (sym.arg(0)?, base);
    let old = out.arg(0)?.to_string();
    out.set_arg(0, Sexpr::str(name));
    let own: Vec<&Sexpr> = sym.children("property").collect();
    let items = out.items_mut();
    items.retain(|c| c.head() != Some("property") || !own.iter().any(|p| p.arg(0) == c.arg(0)));
    for (i, p) in own.into_iter().enumerate() {
        items.insert(2 + i, p.clone());
    }
    for c in items.iter_mut().filter(|c| c.head() == Some("symbol")) {
        if let Some(unit) = c.arg(0).and_then(|u| u.strip_prefix(&old)).map(|u| format!("{name}{u}")) {
            c.set_arg(0, Sexpr::str(&unit));
        }
    }
    Some(out)
}

/// The 3D model a footprint points at, plus a same-named STEP/WRL sibling (add_part prefers STEP).
fn find_model(fp: &Path, files: &Files, root: &Path) -> Option<Vec<PathBuf>> {
    let uri = sexpr::parse(&std::fs::read_to_string(fp).ok()?).ok()?.child("model")?.arg(0)?.replace('\\', "/");
    let tail = match uri.strip_prefix("${") {
        // `${KIPRJMOD}`/`${KICAD9_3DMODEL_DIR}`/...: the variable is unknown here, so match on what follows it.
        Some(rest) => rest.split_once('}').map_or("", |(_, t)| t).trim_start_matches('/').to_string(),
        None => uri.clone(),
    };
    let abs = PathBuf::from(&uri);
    let direct = [root.join(&tail), fp.parent()?.join(&tail), abs];
    let main = direct.into_iter().find(|p| p.is_file() && (p.is_absolute() || p.starts_with(root) || p.starts_with(fp.parent().unwrap_or(root)))).or_else(|| {
        // Fall back to the file name, choosing the candidate whose path shares most of the URI's tail.
        let cands = files.models.get(&Path::new(&tail).file_name()?.to_string_lossy().to_lowercase())?;
        let want = tail.to_lowercase();
        cands.iter().max_by_key(|c| {
            let c = c.to_string_lossy().replace('\\', "/").to_lowercase();
            want.rsplit('/').zip(c.rsplit('/')).take_while(|(a, b)| a == b).count()
        }).cloned()
    })?;
    let mut all = vec![main.clone()];
    for ext in ["step", "stp", "wrl"] {
        let sib = main.with_extension(ext);
        if sib != main && sib.is_file() {
            all.push(sib);
        }
    }
    Some(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_symbols_are_flattened() {
        let lib = sexpr::parse(r#"(kicad_symbol_lib (symbol "A" (property "Reference" "U") (property "Value" "A") (symbol "A_1_1" (pin x))) (symbol "B" (extends "A") (property "Value" "B")))"#).unwrap();
        let b = flatten(&lib, lib.children("symbol").nth(1).unwrap()).unwrap();
        assert_eq!(property(&b, "Value"), Some("B"));
        assert_eq!(property(&b, "Reference"), Some("U"));
        assert_eq!(b.children("symbol").next().unwrap().arg(0), Some("B_1_1"));
        assert!(b.child("extends").is_none());
    }
}
