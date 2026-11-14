//! Close-variant and similar-component SPICE model discovery across the library,
//! user-configured SPICE folders, and KiCad's built-in simulation models.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{SpiceKind, SpiceModel, is_spice_ext, parse_models, sanitize_spice_name};
use crate::kicad_env;
use crate::library::{LibraryRoot, property};
use crate::matching;
use crate::rules::Rules;
use crate::sexpr;
use crate::symbols::detect as detect_class;

/// One candidate SPICE model discovered on disk.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub file: PathBuf,
    pub model: SpiceModel,
    pub mpn_hint: String,
    pub manufacturer: String,
    pub category: String,
    pub description: String,
    pub class_name: Option<String>,
    pub pin_count: usize,
    pub source_label: String,
}

/// How closely a candidate matched the target component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchTier {
    Exact,
    CloseVariant { matched_part: String, reason: String },
    SimilarComponent { matched_part: String, reason: String },
}

/// Collects SPICE candidates from the library, user `spice_dirs`, and KiCad's built-in `Simulation_SPICE.sp`.
pub fn collect_candidates(library: Option<&LibraryRoot>, spice_dirs: &[PathBuf]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut seen_model_files = HashSet::new();

    if let Some(lib) = library {
        collect_from_library(lib, &mut out, &mut seen_model_files);
    }
    for dir in spice_dirs {
        collect_from_dir(dir, "SPICE library", &mut out, &mut seen_model_files, 0);
    }
    if let Some(kicad_sp) = kicad_builtin_spice_file() {
        collect_from_file(&kicad_sp, "KiCad built-in", &mut out, &mut seen_model_files);
    }
    out
}

/// Locates KiCad's bundled `Simulation_SPICE.sp`, if installed.
pub fn kicad_builtin_spice_file() -> Option<PathBuf> {
    for var in ["KICAD10_SYMBOL_DIR", "KICAD9_SYMBOL_DIR", "KICAD8_SYMBOL_DIR"] {
        if let Some(dir) = kicad_env::default_var_dir(var) {
            let sp = dir.join("Simulation_SPICE.sp");
            if sp.is_file() {
                return Some(sp);
            }
        }
    }
    None
}

fn collect_from_library(lib: &LibraryRoot, out: &mut Vec<Candidate>, seen_files: &mut HashSet<PathBuf>) {
    let rules = Rules::default();
    // 1. Index symbols that already have a valid Sim.Library so we inherit rich metadata (Class, pins, MPN, description).
    let mut file_meta: HashMap<PathBuf, (String, String, String, String, Option<String>, usize)> = HashMap::new();
    for cat in lib.categories() {
        let sym_path = lib.dir.join("symbols").join(format!("{cat}.kicad_sym"));
        let Ok(text) = std::fs::read_to_string(&sym_path) else { continue };
        let Ok(parsed) = sexpr::parse(&text) else { continue };
        for sym in parsed.children("symbol") {
            let Some(sim_lib) = property(sym, "Sim.Library").filter(|s| !s.trim().is_empty()) else { continue };
            let resolved = resolve_sim_library_path(sim_lib, lib);
            let Some(resolved) = resolved.filter(|p| p.is_file()) else { continue };
            let mpn = property(sym, "MPN").or_else(|| sym.arg(0)).unwrap_or("").trim().to_string();
            let mfg = property(sym, "Manufacturer").unwrap_or("").trim().to_string();
            let desc = property(sym, "Description").unwrap_or("").trim().to_string();
            let pins = super::pins::extract_pins(sym, Some(&parsed)).len();
            let cls = detect_class(&cat, sym, pins, &rules).ok().map(|c| format!("{c:?}"));
            file_meta.insert(resolved, (mpn, mfg, cat.clone(), desc, cls, pins));
        }
    }

    for (path, (mpn, mfg, cat, desc, cls, pins)) in file_meta {
        if !seen_files.insert(path.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let models = parse_models(&text);
        let chosen = super::pick_model(&models, &mpn, None).cloned();
        if let Some(model) = chosen {
            out.push(Candidate {
                file: path,
                model,
                mpn_hint: mpn,
                manufacturer: mfg,
                category: cat,
                description: desc,
                class_name: cls,
                pin_count: pins,
                source_label: "library".into(),
            });
        } else {
            for model in models {
                out.push(Candidate {
                    file: path.clone(),
                    mpn_hint: model.name.clone(),
                    manufacturer: mfg.clone(),
                    category: cat.clone(),
                    description: desc.clone(),
                    class_name: cls.clone(),
                    pin_count: pins,
                    model,
                    source_label: "library".into(),
                });
            }
        }
    }

    // 2. Also pick up any unreferenced SPICE files placed directly in `<lib>/simulation/`.
    let sim_dir = lib.dir.join("simulation");
    if sim_dir.is_dir() {
        collect_from_dir(&sim_dir, "library simulation/", out, seen_files, 0);
    }
}

/// Resolves a KiCad `Sim.Library` string (expanding `${KIPRJMOD}` or KiCad vars) when possible.
pub fn resolve_sim_library_path(raw: &str, lib: &LibraryRoot) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let replaced = if trimmed.contains("${KIPRJMOD}") {
        if let Some(parent) = lib.dir.parent() {
            trimmed.replace("${KIPRJMOD}", &parent.to_string_lossy().replace('\\', "/"))
        } else {
            trimmed.to_string()
        }
    } else if let Some(rest) = trimmed.strip_prefix("${").and_then(|s| s.split_once('}')) {
        if let Some(dir) = kicad_env::default_var_dir(rest.0) {
            format!("{}{}", dir.to_string_lossy().replace('\\', "/"), rest.1)
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    };
    let p = PathBuf::from(&replaced);
    if p.is_absolute() { Some(p) } else { Some(lib.dir.join("simulation").join(p)) }
}

fn collect_from_dir(dir: &Path, label: &str, out: &mut Vec<Candidate>, seen_files: &mut HashSet<PathBuf>, depth: u8) {
    if depth > 4 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            collect_from_dir(&p, label, out, seen_files, depth + 1);
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(is_spice_ext) {
            collect_from_file(&p, label, out, seen_files);
        }
    }
}

fn collect_from_file(path: &Path, label: &str, out: &mut Vec<Candidate>, seen_files: &mut HashSet<PathBuf>) {
    if !seen_files.insert(path.to_path_buf()) {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else { return };
    if bytes.len() > 8 * 1024 * 1024 {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let models = parse_models(&text);
    for model in models {
        let pin_count = model.ports.len();
        let mpn_hint = if !model.name.is_empty() && !model.name.starts_with("kicad_builtin_") {
            model.name.clone()
        } else {
            stem.clone()
        };
        out.push(Candidate {
            file: path.to_path_buf(),
            model,
            mpn_hint,
            manufacturer: String::new(),
            category: String::new(),
            description: String::new(),
            class_name: None,
            pin_count,
            source_label: label.to_string(),
        });
    }
}

/// Strips packaging, tape-and-reel, tolerance, and automotive grade suffixes to reveal the electrical base family.
pub fn normalize_family_mpn(mpn: &str) -> String {
    let mut s = mpn.trim().to_ascii_uppercase();
    // Strip common separator tails like "-7-F", "-13-F", "/NOPB", "-TP", "#PBF".
    for sep in ["/NOPB", "#PBF", "-PBF", "-LF", "-TR", "-CT", "-ND"] {
        if let Some(stripped) = s.strip_suffix(sep) {
            s = stripped.to_string();
        }
    }
    if let Some((head, tail)) = s.rsplit_once('-')
        && head.len() >= 4
        && matches!(tail, "7" | "13" | "F" | "7F" | "13F" | "G" | "T" | "R" | "E3" | "Q1")
    {
        s = head.to_string();
    }
    for suffix in ["TRPBF", "TR1G", "T1GE3", "DGKR", "DBVR", "DBVT", "DCKR", "DRVR", "PWR", "PWP", "RGTR", "RGET", "TPBF", "NOPB", "E3", "TR", "WS", "W", "DR", "DT", "DGK", "DBV", "DCK", "N", "P"] {
        if s.len() > suffix.len() + 3 && s.ends_with(suffix) {
            let prefix = &s[..s.len() - suffix.len()];
            // Do not strip single-letter suffix when it separates two digits (keep e.g. 2N2222A vs 2N2222 separately if exact exists, or strip below).
            if prefix.ends_with(|c: char| c.is_ascii_digit() || c.is_ascii_alphabetic()) {
                s = prefix.to_string();
                break;
            }
        }
    }
    s.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// Returns the best matching exact or close-variant SPICE candidate for the target part, if any.
pub fn find_close_sim(
    mpn: &str,
    alternatives: &[String],
    category: &str,
    description: &str,
    pin_count: usize,
    candidates: &[Candidate],
) -> Option<(Candidate, MatchTier)> {
    let target_key = matching::key(mpn);
    let target_family = normalize_family_mpn(mpn);
    let alt_keys: Vec<(String, String)> = alternatives
        .iter()
        .flat_map(|a| {
            [matching::key(a), normalize_family_mpn(a)]
                .into_iter()
                .filter(|k| k.len() >= 3)
                .map(move |k| (k, a.clone()))
        })
        .collect();

    let mut best: Option<(i32, Candidate, MatchTier)> = None;
    for c in candidates {
        let model_key = matching::key(&c.model.name);
        let hint_key = matching::key(&c.mpn_hint);
        let stem_key = c.file.file_stem().and_then(|s| s.to_str()).map(matching::key).unwrap_or_default();
        let model_fam = normalize_family_mpn(&c.model.name);
        let hint_fam = normalize_family_mpn(&c.mpn_hint);

        // Penalize helper/sub-block models when a full-device match exists.
        if c.pin_count > 0 && pin_count > 0 && matches!(c.model.kind, SpiceKind::Subckt) && c.pin_count != pin_count && c.pin_count + 1 != pin_count {
            // Op-amp subckts can be 5-pin models for 8-pin dual opamps; keep if target looks multi-unit/8-pin, else downrank heavily.
            if !(c.pin_count == 5 && pin_count == 8) {
                continue;
            }
        }

        let (score, tier) = if !target_key.is_empty() && (model_key == target_key || hint_key == target_key || stem_key == target_key) {
            (1000, MatchTier::Exact)
        } else if target_family.len() >= 4
            && (model_fam == target_family || hint_fam == target_family || (!model_key.is_empty() && model_key == target_family.to_ascii_lowercase()))
        {
            (
                850,
                MatchTier::CloseVariant {
                    matched_part: c.mpn_hint.clone(),
                    reason: format!("same base family as {mpn}"),
                },
            )
        } else if let Some((_, alt_orig)) = alt_keys.iter().find(|(ak, _)| *ak == model_key || *ak == hint_key || *ak == model_fam) {
            (
                750,
                MatchTier::CloseVariant {
                    matched_part: c.mpn_hint.clone(),
                    reason: format!("listed alternative {alt_orig}"),
                },
            )
        } else if target_family.len() >= 5
            && !c.model.name.starts_with("kicad_builtin_")
            && ((model_fam.len() >= 5 && (model_fam.starts_with(&target_family) || target_family.starts_with(&model_fam)))
                || (hint_fam.len() >= 5 && (hint_fam.starts_with(&target_family) || target_family.starts_with(&hint_fam))))
        {
            // Guard against accidental numeric prefix collisions like LM35 vs LM358: suffix must be alphabetic grade/package only.
            let a = &target_family;
            let b = if model_fam.len() >= 5 { &model_fam } else { &hint_fam };
            let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
            let remainder = &longer[shorter.len()..];
            if remainder.chars().all(|ch| ch.is_ascii_alphabetic()) && remainder.len() <= 3 {
                (
                    700,
                    MatchTier::CloseVariant {
                        matched_part: c.mpn_hint.clone(),
                        reason: format!("close part family variant ({})", c.model.name),
                    },
                )
            } else {
                continue;
            }
        } else {
            continue;
        };

        let bonus = if !category.is_empty() && c.category.eq_ignore_ascii_case(category) { 20 } else { 0 }
            + if !description.is_empty() && !c.description.is_empty() { 10 } else { 0 };
        let total = score + bonus;
        if total > best.as_ref().map_or(0, |b| b.0) {
            best = Some((total, c.clone(), tier));
        }
    }

    best.map(|(_, c, tier)| (c, tier))
}

/// Finds the best similar-class component candidate to act as a template/reference when generating a SPICE file.
pub fn find_similar_for_adaptation(
    target_device: &str,
    pin_count: usize,
    class_hint: Option<&str>,
    candidates: &[Candidate],
) -> Option<Candidate> {
    let mut best: Option<(i32, &Candidate)> = None;
    for c in candidates {
        let mut score = 0i32;
        if !target_device.is_empty() && c.model.device.eq_ignore_ascii_case(target_device) {
            score += 200;
        }
        if let Some(cls) = class_hint
            && c.class_name.as_deref() == Some(cls)
        {
            score += 120;
        }
        if pin_count > 0 && c.pin_count == pin_count {
            score += 60;
        }
        if c.model.name.starts_with("kicad_builtin_") {
            // Real library / user models beat generic built-ins when equivalent.
            score += 10;
        } else {
            score += 40;
        }
        // Must share either device type or component class.
        if score < 120 {
            continue;
        }
        if score > best.map_or(0, |b| b.0) {
            best = Some((score, c));
        }
    }
    best.map(|(_, c)| c.clone())
}

/// Writes a self-contained SPICE `.lib` file in `work_dir` for a matched candidate, extracting only the
/// matched `.subckt`/`.model` and any local helper definitions it references, with provenance comments.
pub fn materialize_matched_model(
    candidate: &Candidate,
    target_mpn: &str,
    tier: &MatchTier,
    work_dir: &Path,
) -> Result<PathBuf> {
    std::fs::create_dir_all(work_dir)?;
    let full_text = std::fs::read_to_string(&candidate.file)?;
    let all_models = parse_models(&full_text);

    let mut included = Vec::new();
    let mut seen_names = HashSet::new();
    seen_names.insert(candidate.model.name.to_ascii_uppercase());
    included.push(candidate.model.raw_block.clone());

    // Pull any `.model` / `.subckt` dependencies referenced inside a matched subcircuit.
    if matches!(candidate.model.kind, SpiceKind::Subckt) {
        let upper_body = candidate.model.raw_block.to_ascii_uppercase();
        for m in &all_models {
            let uname = m.name.to_ascii_uppercase();
            if seen_names.contains(&uname) {
                continue;
            }
            if upper_body.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-').any(|tok| tok == uname) {
                seen_names.insert(uname);
                included.push(m.raw_block.clone());
            }
        }
    }

    let safe_name = sanitize_spice_name(target_mpn);
    let out_path = work_dir.join(format!("{safe_name}.lib"));
    let note = match tier {
        MatchTier::Exact => format!("* SPICE model matched to {target_mpn} (source: {})", candidate.source_label),
        MatchTier::CloseVariant { matched_part, reason } => {
            format!("* Close SPICE model for {target_mpn}: using {matched_part} ({reason}, source: {})", candidate.source_label)
        }
        MatchTier::SimilarComponent { matched_part, reason } => {
            format!("* Similar SPICE model for {target_mpn}: based on {matched_part} ({reason})")
        }
    };
    let mut content = format!("{note}\n* Original model: {} (device: {})\n\n", candidate.model.name, candidate.model.device);
    for block in included {
        content.push_str(block.trim());
        content.push_str("\n\n");
    }
    std::fs::write(&out_path, content)?;
    Ok(out_path)
}
