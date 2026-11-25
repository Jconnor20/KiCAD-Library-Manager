//! SPICE simulation model parsing, pin mapping, close-variant discovery, and datasheet synthesis.

pub mod matcher;
pub mod pins;
pub mod synth;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::library::{LibraryRoot, property};
use crate::matching;
use crate::part::Part;
use crate::sexpr::Sexpr;

/// File extensions recognized as SPICE / simulation model libraries.
pub const SIM_EXTS: &[&str] = &["lib", "cir", "sub", "sp", "spi", "ckt", "mod", "ibs"];

/// True when `ext` (without leading dot) is a recognized simulation extension.
pub fn is_spice_ext(ext: &str) -> bool {
    SIM_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

/// True when `text` looks like SPICE netlist/model text (`.subckt`, `.model`, or `[IBIS Ver]`).
pub fn is_spice_text(text: &str) -> bool {
    text.lines().any(|l| {
        let lo = l.trim_start().to_ascii_lowercase();
        lo.starts_with(".subckt ") || lo.starts_with(".model ") || lo.starts_with("[ibis ver]")
    })
}

/// Safe SPICE identifier derived from an MPN or symbol name (starts with a letter, alphanumeric + `_`).
pub fn sanitize_spice_name(name: &str) -> String {
    let mut out: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if out.is_empty() || !out.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') {
        out.insert_str(0, "M_");
    }
    out
}

/// Kind of SPICE definition inside a model file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpiceKind {
    Subckt,
    Model(String),
}

/// A parsed `.subckt` or `.model` definition.
#[derive(Clone, Debug)]
pub struct SpiceModel {
    pub kind: SpiceKind,
    pub name: String,
    pub device: String,
    pub sub_type: Option<String>,
    pub ports: Vec<String>,
    pub params: Vec<(String, String)>,
    pub raw_block: String,
}

/// Where a resolved simulation model came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimOrigin {
    Provided,
    ExactMatch { source: String, model: String },
    CloseMatch { source: String, matched_part: String, reason: String },
    GeneratedFromDatasheet { based_on: Option<String>, notes: Vec<String> },
}

impl SimOrigin {
    pub fn summary(&self) -> String {
        match self {
            Self::Provided => "provided with component".into(),
            Self::ExactMatch { source, model } => format!("exact SPICE match {model} ({source})"),
            Self::CloseMatch { source, matched_part, reason } => {
                format!("close SPICE match {matched_part} ({reason}, {source})")
            }
            Self::GeneratedFromDatasheet { based_on: Some(base), .. } => {
                format!("generated from datasheet + similar model {base}")
            }
            Self::GeneratedFromDatasheet { based_on: None, .. } => "generated from datasheet / part specs".into(),
        }
    }
}

/// Output of resolving or synthesizing a simulation model for a component.
#[derive(Clone, Debug)]
pub struct SimResolution {
    pub file: PathBuf,
    pub model_name: String,
    pub device: String,
    pub sub_type: Option<String>,
    pub pins: Option<String>,
    pub origin: SimOrigin,
}

/// Parses all top-level `.subckt ... .ends` blocks and `.model ...` cards in SPICE text,
/// unfolding `+` continuation lines and stripping inline `;` / `*` comments.
pub fn parse_models(text: &str) -> Vec<SpiceModel> {
    let raw_lines: Vec<&str> = text.lines().collect();
    // Group logical lines (folding `+` continuations) while remembering source line ranges for `raw_block`.
    let mut logical: Vec<(String, usize, usize)> = Vec::new();
    for (idx, &raw) in raw_lines.iter().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('*') || trimmed.starts_with(';') {
            continue;
        }
        let code = strip_inline_comment(trimmed);
        if let Some(cont) = code.strip_prefix('+')
            && let Some(last) = logical.last_mut()
        {
            last.0.push(' ');
            last.0.push_str(cont.trim());
            last.2 = idx;
        } else if !code.is_empty() {
            logical.push((code.to_string(), idx, idx));
        }
    }

    let mut out = Vec::new();
    let mut subckt_depth = 0usize;
    let mut active_subckt: Option<(String, Vec<String>, Vec<(String, String)>, usize)> = None;

    for (line, start_idx, end_idx) in &logical {
        let mut tokens = line.split_whitespace();
        let Some(first) = tokens.next() else { continue };
        let kw = first.to_ascii_lowercase();

        if kw == ".subckt" {
            if subckt_depth == 0 {
                let Some(name) = tokens.next() else { continue };
                let rest: Vec<&str> = tokens.collect();
                let (ports, params) = split_ports_and_params(&rest);
                active_subckt = Some((name.to_string(), ports, params, *start_idx));
            }
            subckt_depth += 1;
            continue;
        }

        if kw == ".ends" {
            if subckt_depth > 0 {
                subckt_depth -= 1;
                if subckt_depth == 0
                    && let Some((name, ports, params, block_start)) = active_subckt.take()
                {
                    let raw_block = raw_lines[block_start..=*end_idx].join("\n");
                    out.push(SpiceModel { kind: SpiceKind::Subckt, name, device: "SUBCKT".into(), sub_type: None, ports, params, raw_block });
                }
            }
            continue;
        }

        if kw == ".model" && subckt_depth == 0 {
            if let Some(m) = parse_model_card(line, &raw_lines[*start_idx..=*end_idx].join("\n")) {
                out.push(m);
            }
        }
    }

    // Also include unclosed subckt gracefully if `.ends` was omitted at EOF.
    if let Some((name, ports, params, block_start)) = active_subckt.take() {
        let raw_block = raw_lines[block_start..].join("\n");
        out.push(SpiceModel { kind: SpiceKind::Subckt, name, device: "SUBCKT".into(), sub_type: None, ports, params, raw_block });
    }

    out
}

fn strip_inline_comment(line: &str) -> &str {
    // `;` and ` $` start SPICE inline comments (avoid stripping `;` inside quotes if any).
    let mut out_end = line.len();
    for (i, c) in line.char_indices() {
        if c == ';' || (c == '$' && i > 0 && line.as_bytes()[i - 1] == b' ') {
            out_end = i;
            break;
        }
    }
    line[..out_end].trim()
}

fn split_ports_and_params(rest: &[&str]) -> (Vec<String>, Vec<(String, String)>) {
    let mut ports = Vec::new();
    let mut params = Vec::new();
    let mut in_params = false;
    for &tok in rest {
        let lo = tok.to_ascii_lowercase();
        if lo == "params:" || lo == "param:" {
            in_params = true;
            continue;
        }
        if let Some(stripped) = lo.strip_prefix("params:") {
            in_params = true;
            if let Some((k, v)) = tok[tok.len() - stripped.len()..].split_once('=') {
                params.push((k.trim().to_string(), v.trim().to_string()));
            }
            continue;
        }
        if in_params || tok.contains('=') {
            in_params = true;
            if let Some((k, v)) = tok.split_once('=') {
                params.push((k.trim().to_string(), v.trim().to_string()));
            }
        } else {
            ports.push(tok.to_string());
        }
    }
    (ports, params)
}

fn parse_model_card(logical_line: &str, raw_block: &str) -> Option<SpiceModel> {
    // Forms: `.model NAME TYPE(...)` or `.model NAME TYPE (...)` or `.model NAME TYPE k=v ...` or `.model NAME VDMOS pchan (...)`
    let after_kw = logical_line.trim().get(6..)?.trim();
    let mut parts = after_kw.splitn(3, char::is_whitespace);
    let name = parts.next()?.trim().to_string();
    let rest = parts.next().unwrap_or("").to_string() + " " + parts.next().unwrap_or("");
    let rest = rest.trim();
    if name.is_empty() || rest.is_empty() {
        return None;
    }

    let (type_raw, param_body) = if let Some(paren_idx) = rest.find('(') {
        let head = rest[..paren_idx].trim();
        let tail = rest[paren_idx + 1..].trim();
        let tail = tail.strip_suffix(')').unwrap_or(tail);
        (head, tail)
    } else {
        match rest.split_once(char::is_whitespace) {
            Some((head, tail)) => (head.trim(), tail.trim()),
            None => (rest, ""),
        }
    };

    let mut head_words = type_raw.split_whitespace();
    let model_type = head_words.next()?.to_ascii_uppercase();
    let extra_flags: Vec<&str> = head_words.collect();
    let mut is_pchan = extra_flags.iter().any(|f| f.eq_ignore_ascii_case("pchan"));

    let mut params = Vec::new();
    for tok in param_body.split_whitespace() {
        let clean = tok.trim_matches(|c| c == '(' || c == ')' || c == ',');
        if clean.eq_ignore_ascii_case("pchan") {
            is_pchan = true;
        } else if let Some((k, v)) = clean.split_once('=') {
            params.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    let (device, sub_type) = match model_type.as_str() {
        "D" => ("D".into(), None),
        "NPN" | "LPNP" => ("NPN".into(), None),
        "PNP" => ("PNP".into(), None),
        "VDMOS" => (if is_pchan { "PMOS".into() } else { "NMOS".into() }, Some("VDMOS".into())),
        "NMOS" => ("NMOS".into(), None),
        "PMOS" => ("PMOS".into(), None),
        "NJF" | "NJFET" => ("NJFET".into(), None),
        "PJF" | "PJFET" => ("PJFET".into(), None),
        "R" | "RES" => ("R".into(), None),
        "C" | "CAP" => ("C".into(), None),
        "L" | "IND" => ("L".into(), None),
        "SW" | "VSWITCH" => ("SW".into(), None),
        _ => ("SPICE".into(), None),
    };

    Some(SpiceModel { kind: SpiceKind::Model(model_type), name, device, sub_type, ports: Vec::new(), params, raw_block: raw_block.to_string() })
}

/// Chooses the best model inside a SPICE file for `mpn` (preferring exact `hint`, then `same_mpn`, then first).
pub fn pick_model<'a>(models: &'a [SpiceModel], mpn: &str, hint: Option<&str>) -> Option<&'a SpiceModel> {
    if models.is_empty() {
        return None;
    }
    if let Some(h) = hint.map(str::trim).filter(|s| !s.is_empty())
        && let Some(m) = models.iter().find(|m| m.name.eq_ignore_ascii_case(h))
    {
        return Some(m);
    }
    let target = matching::key(mpn);
    let fam = matcher::normalize_family_mpn(mpn);
    models.iter().max_by_key(|m| {
        let mk = matching::key(&m.name);
        let mf = matcher::normalize_family_mpn(&m.name);
        let exact = if !target.is_empty() && mk == target {
            100
        } else if fam.len() >= 4 && mf == fam {
            80
        } else if matches!(m.kind, SpiceKind::Subckt) {
            20
        } else {
            10
        };
        exact
    })
}

/// Resolves an existing/close SPICE model or generates one from datasheet + similar models for a component.
pub fn resolve_or_generate(
    part: &Part,
    symbol: Option<&Sexpr>,
    all_symbols: Option<&Sexpr>,
    category: &str,
    library: Option<&LibraryRoot>,
    spice_dirs: &[PathBuf],
    allow_generate: bool,
    work_dir: &Path,
) -> Result<Option<SimResolution>> {
    let pins = symbol.map(|s| pins::extract_pins(s, all_symbols)).unwrap_or_default();
    let sym_ref = symbol.and_then(|s| property(s, "Reference")).unwrap_or("");
    let sim_name_hint = symbol.and_then(|s| property(s, "Sim.Name"));

    // 1. Existing SPICE file already attached to `part.sim_files` or referenced by `Sim.Library`.
    let existing = part.sim_files.first().cloned().or_else(|| {
        let raw = symbol.and_then(|s| property(s, "Sim.Library"))?;
        library.and_then(|lib| matcher::resolve_sim_library_path(raw, lib)).filter(|p| p.is_file())
    });
    if let Some(path) = existing
        && path.is_file()
    {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let models = parse_models(&text);
        if let Some(model) = pick_model(&models, &part.mpn, sim_name_hint) {
            let (sim_pins, device) = pins::map_sim_pins(&pins, model, sym_ref, category);
            return Ok(Some(SimResolution {
                file: path,
                model_name: model.name.clone(),
                device,
                sub_type: model.sub_type.clone(),
                pins: sim_pins,
                origin: SimOrigin::Provided,
            }));
        }
    }

    // Gather close / similar candidates from library, spice_dirs, and KiCad built-in models.
    let candidates = matcher::collect_candidates(library, spice_dirs);
    let alternatives: Vec<String> = symbol
        .and_then(|s| property(s, "Alternatives"))
        .map(|s| s.split([',', ';']).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect())
        .unwrap_or_default();

    // 2. Exact or close-variant SPICE match.
    if let Some((cand, tier)) = matcher::find_close_sim(&part.mpn, &alternatives, category, &part.description, pins.len(), &candidates) {
        let mat_path = matcher::materialize_matched_model(&cand, &part.mpn, &tier, work_dir)?;
        let (sim_pins, device) = pins::map_sim_pins(&pins, &cand.model, sym_ref, category);
        let origin = match tier {
            matcher::MatchTier::Exact => SimOrigin::ExactMatch { source: cand.source_label, model: cand.model.name.clone() },
            matcher::MatchTier::CloseVariant { matched_part, reason } | matcher::MatchTier::SimilarComponent { matched_part, reason } => {
                SimOrigin::CloseMatch { source: cand.source_label, matched_part, reason }
            }
        };
        return Ok(Some(SimResolution { file: mat_path, model_name: cand.model.name, device, sub_type: cand.model.sub_type, pins: sim_pins, origin }));
    }

    if !allow_generate {
        return Ok(None);
    }

    // 3. Datasheet PDF + parametric specs + similar-component model synthesis.
    let ds_path = part.datasheet_file.clone().or_else(|| {
        let raw = symbol.and_then(|s| property(s, "Datasheet"))?.trim();
        if raw.is_empty() || raw.starts_with("http://") || raw.starts_with("https://") {
            None
        } else {
            library.and_then(|lib| matcher::resolve_sim_library_path(raw, lib)).filter(|p| p.is_file())
        }
    });
    let pdf_text = ds_path.as_deref().filter(|p| p.is_file()).and_then(|p| synth::extract_pdf_text(p).ok()).unwrap_or_default();

    let specs = synth::extract_specs(part, symbol, &pins, &pdf_text);
    let target_dev = specs.archetype.as_ref().map(|a| a.spice_device()).unwrap_or("");
    let rules = crate::rules::Rules::default();
    let class_hint = symbol.and_then(|s| crate::symbols::detect(category, s, pins.len(), &rules).ok()).map(|c| format!("{c:?}"));
    let similar = if !target_dev.is_empty() { matcher::find_similar_for_adaptation(target_dev, pins.len(), class_hint.as_deref(), &candidates) } else { None };

    if let Some((gen_path, notes)) = synth::generate_sim(part, &pins, &specs, similar.as_ref(), work_dir)? {
        let text = std::fs::read_to_string(&gen_path).unwrap_or_default();
        let models = parse_models(&text);
        if let Some(model) = pick_model(&models, &part.mpn, None) {
            let (sim_pins, device) = pins::map_sim_pins(&pins, model, sym_ref, category);
            return Ok(Some(SimResolution {
                file: gen_path,
                model_name: model.name.clone(),
                device,
                sub_type: model.sub_type.clone(),
                pins: sim_pins,
                origin: SimOrigin::GeneratedFromDatasheet { based_on: similar.map(|s| s.model.name), notes },
            }));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join("klm-spice-unit").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_multi_model_file_and_vdmos_pchan() {
        let txt = r#"
* Vendor library
.model D_Helper D(IS=1n RS=0.1)
.model IRF9610 VDMOS pchan (VTO=-3.6 KP=0.81 RG=10)
.subckt LM358 in+ in- vcc vee out
+ params: GAIN=100k
R1 in+ in- 1Meg
.model inner_d D(IS=1p)
.ends LM358
"#;
        let models = parse_models(txt);
        assert_eq!(models.len(), 3);
        let fet = &models[1];
        assert_eq!(fet.name, "IRF9610");
        assert_eq!(fet.device, "PMOS");
        assert_eq!(fet.sub_type.as_deref(), Some("VDMOS"));

        let op = &models[2];
        assert_eq!(op.name, "LM358");
        assert_eq!(op.device, "SUBCKT");
        assert_eq!(op.ports, vec!["in+", "in-", "vcc", "vee", "out"]);

        let picked = pick_model(&models, "LM358DR", None).unwrap();
        assert_eq!(picked.name, "LM358");
    }

    #[test]
    fn maps_sim_pins_for_diode_mosfet_and_opamp() {
        let diode_sym = sexpr::parse(
            r#"(symbol "1N4148"
              (property "Reference" "D" (at 0 0 0))
              (symbol "1N4148_1_1"
                (pin passive line (at 0 0 0) (name "K") (number "1"))
                (pin passive line (at 0 0 0) (name "A") (number "2"))))"#,
        )
        .unwrap();
        let diode_model = parse_models(".model 1N4148 D(IS=2.5n)").pop().unwrap();
        let d_pins = pins::extract_pins(&diode_sym, None);
        let (sp, dev) = pins::map_sim_pins(&d_pins, &diode_model, "D", "Diodes");
        assert_eq!(dev, "D");
        assert_eq!(sp.as_deref(), Some("1=K 2=A"));

        // Reversed pin numbers (1=A 2=K) must still map cathode to K and anode to A!
        let rev_diode = sexpr::parse(
            r#"(symbol "MBR0520"
              (property "Reference" "D" (at 0 0 0))
              (symbol "MBR0520_1_1"
                (pin passive line (at 0 0 0) (name "A") (number "1"))
                (pin passive line (at 0 0 0) (name "K") (number "2"))))"#,
        )
        .unwrap();
        let (sp_rev, _) = pins::map_sim_pins(&pins::extract_pins(&rev_diode, None), &diode_model, "D", "Diodes");
        assert_eq!(sp_rev.as_deref(), Some("1=A 2=K"));

        // 8-pin power MOSFET (pins 1-3 S, 4 G, 5-8 D)
        let fet_sym = sexpr::parse(
            r#"(symbol "SI4410"
              (property "Reference" "Q" (at 0 0 0))
              (symbol "SI4410_1_1"
                (pin passive line (at 0 0 0) (name "S") (number "1"))
                (pin passive line (at 0 0 0) (name "S") (number "2"))
                (pin passive line (at 0 0 0) (name "S") (number "3"))
                (pin input line (at 0 0 0) (name "G") (number "4"))
                (pin passive line (at 0 0 0) (name "D") (number "5"))
                (pin passive line (at 0 0 0) (name "D") (number "6"))))"#,
        )
        .unwrap();
        let fet_model = parse_models(".model SI4410 VDMOS(VTO=2.0)").pop().unwrap();
        let (f_pins, f_dev) = pins::map_sim_pins(&pins::extract_pins(&fet_sym, None), &fet_model, "Q", "Transistors");
        assert_eq!(f_dev, "NMOS");
        assert_eq!(f_pins.as_deref(), Some("1=S 2=S 3=S 4=G 5=D 6=D"));
    }

    #[test]
    fn finds_close_variant_in_spice_dir() {
        let dir = temp_dir("close-match");
        let spice_dir = dir.join("models");
        std::fs::create_dir_all(&spice_dir).unwrap();
        std::fs::write(spice_dir.join("opamps.lib"), ".subckt LM358 IN+ IN- VCC VEE OUT\nRin IN+ IN- 1Meg\n.ends LM358\n").unwrap();
        let sym = sexpr::parse(
            r#"(symbol "LM358DR"
              (property "Reference" "U" (at 0 0 0))
              (symbol "LM358DR_1_1"
                (pin output line (at 0 0 0) (name "OUT") (number "1"))
                (pin input line (at 0 0 0) (name "IN-") (number "2"))
                (pin input line (at 0 0 0) (name "IN+") (number "3"))
                (pin power_in line (at 0 0 0) (name "VEE") (number "4"))
                (pin power_in line (at 0 0 0) (name "VCC") (number "8"))))"#,
        )
        .unwrap();
        let part = Part { mpn: "LM358DR".into(), description: "Dual operational amplifier".into(), ..Default::default() };
        let res = resolve_or_generate(&part, Some(&sym), None, "Amplifiers", None, &[spice_dir], false, &dir.join("work"))
            .unwrap()
            .expect("should match LM358 for LM358DR");
        assert_eq!(res.model_name, "LM358");
        assert_eq!(res.device, "SUBCKT");
        assert_eq!(res.pins.as_deref(), Some("1=OUT 2=IN- 3=IN+ 4=VEE 8=VCC"));
        assert!(matches!(res.origin, SimOrigin::CloseMatch { .. }));
    }

    #[test]
    fn extracts_flatedecode_pdf_and_generates_adapted_model() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let dir = temp_dir("pdf-synth");
        let spice_dir = dir.join("ref");
        std::fs::create_dir_all(&spice_dir).unwrap();
        std::fs::write(spice_dir.join("ref_fet.lib"), ".model REF_NMOS VDMOS(VTO=2.5 KP=40 RD=0.04 RS=0.02 CGS=1n CGDMAX=200p CGDMIN=20p CJO=300p)\n").unwrap();

        // Build a valid minimal PDF with a FlateDecode compressed page stream containing datasheet metrics.
        let content = b"BT /F1 12 Tf 72 720 Td (DMN3024L 30V N-Channel MOSFET) Tj T* (Drain-Source Voltage VDSS 30V) Tj T* (Gate Threshold Voltage Vgs\\(th\\) 1.4V) Tj T* (Static Drain-Source On-Resistance RDS\\(on\\) 24mOhm) Tj T* (Input Capacitance Ciss 620pF) Tj ET";
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(content).unwrap();
        let compressed = enc.finish().unwrap();

        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj << /Filter /FlateDecode /Length 999 >>\nstream\n");
        pdf.extend_from_slice(&compressed);
        pdf.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");
        let ds_path = dir.join("DMN3024L.pdf");
        std::fs::write(&ds_path, pdf).unwrap();

        let extracted = synth::extract_pdf_text(&ds_path).unwrap();
        assert!(extracted.contains("24mOhm"), "extracted={extracted}");
        assert!(extracted.contains("620pF"), "extracted={extracted}");

        let sym = sexpr::parse(
            r#"(symbol "DMN3024L"
              (property "Reference" "Q" (at 0 0 0))
              (symbol "DMN3024L_1_1"
                (pin passive line (at 0 0 0) (name "G") (number "1"))
                (pin passive line (at 0 0 0) (name "S") (number "2"))
                (pin passive line (at 0 0 0) (name "D") (number "3"))))"#,
        )
        .unwrap();
        let part = Part {
            mpn: "DMN3024L".into(),
            manufacturer: "Diodes Inc".into(),
            description: "30V N-Channel Enhancement MOSFET".into(),
            datasheet_file: Some(ds_path),
            ..Default::default()
        };

        let res = resolve_or_generate(&part, Some(&sym), None, "Transistors", None, &[spice_dir], true, &dir.join("work"))
            .unwrap()
            .expect("should synthesize model from datasheet PDF + similar MOSFET");
        assert_eq!(res.model_name, "DMN3024L");
        assert_eq!(res.device, "NMOS");
        assert_eq!(res.pins.as_deref(), Some("1=G 2=S 3=D"));
        let generated = std::fs::read_to_string(&res.file).unwrap();
        assert!(generated.contains("adapted from similar model REF_NMOS"), "{generated}");
        assert!(generated.contains("VTO=1.4"), "{generated}");
        assert!(generated.contains("RD=0.012"), "{generated}");
    }
}
