//! Pure-Rust datasheet PDF text extraction, electrical parameter parsing,
//! and SPICE `.model` / `.subckt` synthesis (optionally seeded from a similar existing component).

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;
use flate2::read::{DeflateDecoder, ZlibDecoder};

use super::matcher::Candidate;
use super::pins::SymPin;
use super::{SpiceKind, parse_models, sanitize_spice_name};
use crate::part::Part;
use crate::sexpr::Sexpr;

/// Coarse electrical device archetype used to select synthesis formulas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Archetype {
    Resistor,
    Capacitor,
    Inductor,
    Diode(DiodeKind),
    Bjt { npn: bool },
    Mosfet { n_ch: bool },
    Jfet { n_ch: bool },
    OpAmp,
    Comparator,
    LinearRegulator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiodeKind {
    Standard,
    Schottky,
    Zener,
    Led,
    Tvs,
}

impl Archetype {
    pub fn spice_device(&self) -> &'static str {
        match self {
            Self::Resistor => "R",
            Self::Capacitor => "C",
            Self::Inductor => "L",
            Self::Diode(_) => "D",
            Self::Bjt { npn: true } => "NPN",
            Self::Bjt { npn: false } => "PNP",
            Self::Mosfet { n_ch: true } => "NMOS",
            Self::Mosfet { n_ch: false } => "PMOS",
            Self::Jfet { n_ch: true } => "NJFET",
            Self::Jfet { n_ch: false } => "PJFET",
            Self::OpAmp | Self::Comparator | Self::LinearRegulator => "SUBCKT",
        }
    }
}

/// Electrical parameters extracted from a datasheet PDF, distributor parameter table, description, and symbol fields.
#[derive(Clone, Debug, Default)]
pub struct ExtractedSpecs {
    pub archetype: Option<Archetype>,
    // Passives
    pub resistance_ohm: Option<f64>,
    pub capacitance_f: Option<f64>,
    pub inductance_h: Option<f64>,
    pub esr_ohm: Option<f64>,
    pub dcr_ohm: Option<f64>,
    pub voltage_rating_v: Option<f64>,
    pub power_w: Option<f64>,
    // Discrete semiconductors
    pub vf_v: Option<f64>,
    pub if_a: Option<f64>,
    pub vr_v: Option<f64>,
    pub vz_v: Option<f64>,
    pub ir_a: Option<f64>,
    pub cjo_f: Option<f64>,
    pub trr_s: Option<f64>,
    pub hfe: Option<f64>,
    pub vceo_v: Option<f64>,
    pub ic_max_a: Option<f64>,
    pub vce_sat_v: Option<f64>,
    pub ft_hz: Option<f64>,
    pub vds_max_v: Option<f64>,
    pub id_max_a: Option<f64>,
    pub rds_on_ohm: Option<f64>,
    pub vgs_th_v: Option<f64>,
    pub ciss_f: Option<f64>,
    pub coss_f: Option<f64>,
    pub crss_f: Option<f64>,
    // Linear ICs (Op-amp / Comparator / LDO)
    pub gbw_hz: Option<f64>,
    pub slew_v_us: Option<f64>,
    pub aol_db: Option<f64>,
    pub vos_v: Option<f64>,
    pub iout_a: Option<f64>,
    pub iq_a: Option<f64>,
    pub vout_v: Option<f64>,
    pub vdo_v: Option<f64>,
    pub provenance: Vec<String>,
}

/// Extracts readable text from a local datasheet file (PDF with uncompressed or FlateDecode streams, or plain text).
pub fn extract_pdf_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() {
        return Ok(String::new());
    }
    if !bytes.starts_with(b"%PDF") {
        // Plain text / HTML / CSV fallback for fixtures or text datasheets.
        return Ok(String::from_utf8_lossy(&bytes).chars().take(512 * 1024).collect());
    }
    Ok(extract_pdf_bytes(&bytes))
}

/// Pure-Rust PDF stream/text extractor handling uncompressed and FlateDecode content streams.
pub fn extract_pdf_bytes(bytes: &[u8]) -> String {
    let mut chunks = Vec::new();
    let mut total_len = 0usize;
    const MAX_OUT: usize = 512 * 1024;

    let mut pos = 0usize;
    while pos < bytes.len() && total_len < MAX_OUT {
        let Some(rel_start) = find_subslice(&bytes[pos..], b"stream") else { break };
        let kw_pos = pos + rel_start;
        // Look backward for dictionary attributes (`<< ... >>`) up to 1024 bytes.
        let dict_start = kw_pos.saturating_sub(1024);
        let dict = &bytes[dict_start..kw_pos];
        let dict_str = String::from_utf8_lossy(dict);

        // Skip stream keyword followed by EOL (`\r\n`, `\n`, `\r`).
        let mut data_start = kw_pos + 6;
        if data_start < bytes.len() && bytes[data_start] == b'\r' {
            data_start += 1;
        }
        if data_start < bytes.len() && bytes[data_start] == b'\n' {
            data_start += 1;
        } else {
            pos = kw_pos + 6;
            continue;
        }

        let Some(rel_end) = find_subslice(&bytes[data_start..], b"endstream") else { break };
        let mut data_end = data_start + rel_end;
        while data_end > data_start && matches!(bytes[data_end - 1], b'\r' | b'\n') {
            data_end -= 1;
        }
        pos = data_start + rel_end + 9;

        // Skip raster XObject image streams and metadata/font programs.
        if dict_str.contains("/Subtype /Image") || dict_str.contains("/Subtype/Image") {
            continue;
        }

        let raw_stream = &bytes[data_start..data_end];
        let decoded = if dict_str.contains("/FlateDecode") || dict_str.contains("/Fl ") { decompress_flate(raw_stream) } else { Some(raw_stream.to_vec()) };

        if let Some(stream_bytes) = decoded {
            let text = extract_text_operators(&stream_bytes);
            if !text.is_empty() {
                total_len += text.len();
                chunks.push(text);
            }
        }
    }

    // Some tiny test PDFs place `(Text) Tj` directly without wrapping in `stream ... endstream`.
    if chunks.is_empty() {
        let direct = extract_text_operators(bytes);
        if !direct.is_empty() {
            chunks.push(direct);
        }
    }

    chunks.join("\n")
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn decompress_flate(data: &[u8]) -> Option<Vec<u8>> {
    const MAX_DECOMP: usize = 2 * 1024 * 1024;
    let mut out = Vec::new();
    let mut zr = ZlibDecoder::new(data).take(MAX_DECOMP as u64);
    if zr.read_to_end(&mut out).is_ok() && !out.is_empty() {
        return Some(out);
    }
    out.clear();
    let mut dr = DeflateDecoder::new(data).take(MAX_DECOMP as u64);
    if dr.read_to_end(&mut out).is_ok() && !out.is_empty() {
        return Some(out);
    }
    None
}

/// Extracts literal `( ... )` and `<hex>` strings associated with PDF text operators (`Tj`, `TJ`, `'`, `"`).
fn extract_text_operators(stream: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    let n = stream.len();
    let mut pending = Vec::<String>::new();

    while i < n {
        match stream[i] {
            b'(' => {
                let (s, next) = read_pdf_literal_string(stream, i + 1);
                i = next;
                if !s.is_empty() {
                    pending.push(s);
                }
            }
            b'<' if i + 1 < n && stream[i + 1] != b'<' => {
                if let Some((s, next)) = read_pdf_hex_string(stream, i + 1) {
                    i = next;
                    if !s.is_empty() {
                        pending.push(s);
                    }
                } else {
                    i += 1;
                }
            }
            b'T' if i + 1 < n && matches!(stream[i + 1], b'j' | b'J') => {
                flush_pending(&mut pending, &mut out, false);
                i += 2;
            }
            b'T' if i + 1 < n && matches!(stream[i + 1], b'D' | b'd' | b'*') => {
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
                pending.clear();
                i += 2;
            }
            b'\'' | b'"' if !pending.is_empty() => {
                flush_pending(&mut pending, &mut out, true);
                i += 1;
            }
            b'E' if i + 1 < n && stream[i + 1] == b'T' => {
                flush_pending(&mut pending, &mut out, true);
                i += 2;
            }
            _ => i += 1,
        }
    }
    flush_pending(&mut pending, &mut out, false);
    out
}

fn flush_pending(pending: &mut Vec<String>, out: &mut String, newline: bool) {
    if pending.is_empty() {
        return;
    }
    for seg in pending.drain(..) {
        if !out.is_empty() && !out.ends_with([' ', '\n']) && !seg.starts_with([' ', '.', ',', ')']) {
            out.push(' ');
        }
        out.push_str(&seg);
    }
    if newline && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn read_pdf_literal_string(b: &[u8], mut i: usize) -> (String, usize) {
    let mut s = String::new();
    let mut depth = 1usize;
    while i < b.len() && depth > 0 {
        match b[i] {
            b'\\' if i + 1 < b.len() => {
                i += 1;
                match b[i] {
                    b'n' => s.push('\n'),
                    b'r' | b't' => s.push(' '),
                    b'b' | b'f' => {}
                    b'(' | b')' | b'\\' => s.push(b[i] as char),
                    c @ b'0'..=b'7' => {
                        let mut oct = (c - b'0') as u32;
                        for _ in 0..2 {
                            if i + 1 < b.len() && matches!(b[i + 1], b'0'..=b'7') {
                                i += 1;
                                oct = oct * 8 + (b[i] - b'0') as u32;
                            } else {
                                break;
                            }
                        }
                        if let Some(ch) = char::from_u32(oct) {
                            s.push(ch);
                        }
                    }
                    b'\r' | b'\n' => {}
                    other => s.push(other as char),
                }
            }
            b'(' => {
                depth += 1;
                s.push('(');
            }
            b')' => {
                depth -= 1;
                if depth > 0 {
                    s.push(')');
                }
            }
            byte => {
                if byte.is_ascii_graphic() || byte == b' ' {
                    s.push(byte as char);
                }
            }
        }
        i += 1;
    }
    (s, i)
}

fn read_pdf_hex_string(b: &[u8], mut i: usize) -> Option<(String, usize)> {
    let start = i;
    while i < b.len() && b[i] != b'>' {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let hex_slice = &b[start..i];
    let next = i + 1;
    let hex_digits: Vec<u8> = hex_slice.iter().copied().filter(u8::is_ascii_hexdigit).collect();
    if hex_digits.len() < 2 {
        return Some((String::new(), next));
    }
    let mut raw = Vec::with_capacity(hex_digits.len() / 2);
    for pair in hex_digits.chunks(2) {
        if pair.len() == 2 {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            raw.push(((hi << 4) | lo) as u8);
        }
    }
    let decoded = if raw.starts_with(&[0xFE, 0xFF]) {
        let u16s: Vec<u16> = raw[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u16s)
    } else {
        raw.into_iter().filter(|c| c.is_ascii_graphic() || *c == b' ').map(|c| c as char).collect()
    };
    Some((decoded, next))
}

/// Extracts electrical specifications and archetype from a part, symbol, and optional datasheet text.
pub fn extract_specs(part: &Part, symbol: Option<&Sexpr>, pins: &[SymPin], pdf_text: &str) -> ExtractedSpecs {
    let mut specs = ExtractedSpecs::default();
    let sym_prop = |k: &str| symbol.and_then(|s| crate::library::property(s, k)).unwrap_or("");
    let sym_ref = sym_prop("Reference");
    let sym_val = sym_prop("Value");
    let sym_desc = sym_prop("Description");

    let combined_desc = format!("{} {} {} {}", part.mpn, part.category_hint, part.description, sym_desc);

    specs.archetype = detect_archetype(&combined_desc, sym_ref, pins, &part.params, pdf_text);

    // 1. Parse structured Part.params (highest accuracy) and symbol custom fields.
    for (k, v) in &part.params {
        apply_kv_param(&mut specs, k, v, "part param");
    }
    if let Some(sym) = symbol {
        for prop in sym.children("property") {
            if let (Some(k), Some(v)) = (prop.arg(0), prop.arg(1))
                && !matches!(k, "Reference" | "Footprint" | "Datasheet" | "Sim.Library" | "Sim.Name" | "Sim.Device" | "Sim.Pins")
            {
                apply_kv_param(&mut specs, k, v, "symbol field");
            }
        }
    }

    // 2. Parse Value/Description for primary passive values or regulator output voltage.
    let val_or_desc =
        if !sym_val.is_empty() && !sym_val.eq_ignore_ascii_case(&part.mpn) { format!("{sym_val} {combined_desc}") } else { combined_desc.clone() };

    match specs.archetype {
        Some(Archetype::Resistor) if specs.resistance_ohm.is_none() => {
            if let Some(r) = find_unit_value(&val_or_desc, &["ohm", "ohms", "\u{3a9}", "\u{2126}", "r"]) {
                specs.resistance_ohm = Some(r);
                specs.provenance.push(format!("R={r} ohm from description/value"));
            }
        }
        Some(Archetype::Capacitor) if specs.capacitance_f.is_none() => {
            if let Some(c) = find_unit_value(&val_or_desc, &["f", "pf", "nf", "uf", "mf"]) {
                specs.capacitance_f = Some(c);
                specs.provenance.push(format!("C={c} F from description/value"));
            }
        }
        Some(Archetype::Inductor) if specs.inductance_h.is_none() => {
            if let Some(l) = find_unit_value(&val_or_desc, &["h", "nh", "uh", "mh"]) {
                specs.inductance_h = Some(l);
                specs.provenance.push(format!("L={l} H from description/value"));
            }
        }
        Some(Archetype::LinearRegulator) if specs.vout_v.is_none() => {
            if let Some(v) = infer_ldo_vout(&part.mpn, &val_or_desc) {
                specs.vout_v = Some(v);
                specs.provenance.push(format!("Vout={v} V from MPN/description"));
            }
        }
        _ => {}
    }

    // 3. Parse extracted PDF datasheet text to fill any missing parameters.
    if !pdf_text.trim().is_empty() {
        extract_from_datasheet_text(&mut specs, pdf_text);
    }

    specs
}

fn detect_archetype(desc: &str, sym_ref: &str, pins: &[SymPin], params: &[(String, String)], pdf: &str) -> Option<Archetype> {
    let d =
        format!("{desc} {} {}", params.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(" "), &pdf[..pdf.len().min(4096)]).to_ascii_lowercase();
    let ref_up = sym_ref.trim().to_ascii_uppercase();
    let pin_names: Vec<String> = pins.iter().map(|p| p.name.to_ascii_lowercase()).collect();

    if d.contains("mosfet") || d.contains("vds") || d.contains("rds(on)") || d.contains("rdson") || d.contains("vdmos") {
        let p_ch = d.contains("p-channel") || d.contains("p channel") || d.contains("pmos") || d.contains("p-ch");
        return Some(Archetype::Mosfet { n_ch: !p_ch });
    }
    if d.contains("jfet") {
        let p_ch = d.contains("p-channel") || d.contains("pjfet");
        return Some(Archetype::Jfet { n_ch: !p_ch });
    }
    if d.contains("bjt")
        || d.contains("bipolar transistor")
        || d.contains("hfe")
        || d.contains("vceo")
        || (pins.len() == 3 && pin_names.iter().any(|p| p == "b" || p == "base"))
    {
        let pnp = d.contains("pnp");
        return Some(Archetype::Bjt { npn: !pnp });
    }
    if d.contains("op amp") || d.contains("opamp") || d.contains("operational amplifier") {
        return Some(Archetype::OpAmp);
    }
    if d.contains("comparator") && d.contains("input") {
        return Some(Archetype::Comparator);
    }
    if d.contains("ldo") || d.contains("low-dropout") || d.contains("low dropout") || d.contains("linear regulator") || d.contains("voltage regulator") {
        return Some(Archetype::LinearRegulator);
    }
    if d.contains("schottky") {
        return Some(Archetype::Diode(DiodeKind::Schottky));
    }
    if d.contains("zener") {
        return Some(Archetype::Diode(DiodeKind::Zener));
    }
    if d.contains("tvs") || d.contains("esd") {
        return Some(Archetype::Diode(DiodeKind::Tvs));
    }
    if d.contains("led") || d.contains("light emitting") {
        return Some(Archetype::Diode(DiodeKind::Led));
    }
    if d.contains("diode") || d.contains("rectifier") || ref_up == "D" {
        return Some(Archetype::Diode(DiodeKind::Standard));
    }
    if d.contains("resistor") || ref_up == "R" {
        return Some(Archetype::Resistor);
    }
    if d.contains("capacitor") || d.contains("mlcc") || ref_up == "C" {
        return Some(Archetype::Capacitor);
    }
    if d.contains("inductor") || d.contains("ferrite") || d.contains("choke") || ref_up == "L" {
        return Some(Archetype::Inductor);
    }
    if pins.len() == 3 && pin_names.iter().any(|p| p == "g" || p == "gate") {
        return Some(Archetype::Mosfet { n_ch: !d.contains("pnp") && !d.contains("p-ch") });
    }
    if (pins.len() == 5 || pins.len() == 8) && pin_names.iter().any(|p| p.contains("in+")) && pin_names.iter().any(|p| p.contains("in-")) {
        return Some(Archetype::OpAmp);
    }
    None
}

fn apply_kv_param(specs: &mut ExtractedSpecs, key: &str, val: &str, src: &str) {
    let k = key.trim().to_ascii_lowercase();
    let v = val.trim();
    if v.is_empty() {
        return;
    }
    let Some(num) = parse_leading_si(v) else { return };
    if (k == "resistance" || k == "r") && specs.resistance_ohm.is_none() {
        specs.resistance_ohm = Some(num);
        specs.provenance.push(format!("R={val} ({src})"));
    } else if (k == "capacitance" || k == "c") && specs.capacitance_f.is_none() {
        specs.capacitance_f = Some(num);
        specs.provenance.push(format!("C={val} ({src})"));
    } else if (k == "inductance" || k == "l") && specs.inductance_h.is_none() {
        specs.inductance_h = Some(num);
        specs.provenance.push(format!("L={val} ({src})"));
    } else if k.contains("esr") && specs.esr_ohm.is_none() {
        specs.esr_ohm = Some(num);
        specs.provenance.push(format!("ESR={val} ({src})"));
    } else if (k.contains("dcr") || k.contains("dc resistance")) && specs.dcr_ohm.is_none() {
        specs.dcr_ohm = Some(num);
        specs.provenance.push(format!("DCR={val} ({src})"));
    } else if (k.contains("rds") || k.contains("on resistance")) && specs.rds_on_ohm.is_none() {
        specs.rds_on_ohm = Some(num);
        specs.provenance.push(format!("Rds(on)={val} ({src})"));
    } else if k.contains("vds") && specs.vds_max_v.is_none() {
        specs.vds_max_v = Some(num.abs());
        specs.provenance.push(format!("Vds={val} ({src})"));
    } else if (k.contains("vgs(th)") || k.contains("vth") || k.contains("threshold voltage")) && specs.vgs_th_v.is_none() {
        specs.vgs_th_v = Some(num);
        specs.provenance.push(format!("Vgs(th)={val} ({src})"));
    } else if (k == "id" || k.contains("drain current") || k.contains("continuous current")) && specs.id_max_a.is_none() {
        specs.id_max_a = Some(num.abs());
        specs.provenance.push(format!("Id={val} ({src})"));
    } else if k.contains("ciss") && specs.ciss_f.is_none() {
        specs.ciss_f = Some(num);
        specs.provenance.push(format!("Ciss={val} ({src})"));
    } else if k.contains("coss") && specs.coss_f.is_none() {
        specs.coss_f = Some(num);
        specs.provenance.push(format!("Coss={val} ({src})"));
    } else if k.contains("crss") && specs.crss_f.is_none() {
        specs.crss_f = Some(num);
        specs.provenance.push(format!("Crss={val} ({src})"));
    } else if (k == "vf" || k.contains("forward voltage")) && specs.vf_v.is_none() {
        specs.vf_v = Some(num);
        specs.provenance.push(format!("Vf={val} ({src})"));
    } else if (k == "if" || k.contains("forward current")) && specs.if_a.is_none() {
        specs.if_a = Some(num);
        specs.provenance.push(format!("If={val} ({src})"));
    } else if (k == "vr" || k.contains("vrrm") || k.contains("reverse voltage") || k.contains("breakdown")) && specs.vr_v.is_none() {
        specs.vr_v = Some(num);
        specs.provenance.push(format!("Vr={val} ({src})"));
    } else if (k == "vz" || k.contains("zener voltage")) && specs.vz_v.is_none() {
        specs.vz_v = Some(num);
        specs.provenance.push(format!("Vz={val} ({src})"));
    } else if (k == "hfe" || k.contains("dc current gain")) && specs.hfe.is_none() {
        specs.hfe = Some(num);
        specs.provenance.push(format!("hFE={val} ({src})"));
    } else if k.contains("vceo") && specs.vceo_v.is_none() {
        specs.vceo_v = Some(num.abs());
        specs.provenance.push(format!("Vceo={val} ({src})"));
    } else if (k == "gbw" || k.contains("gain bandwidth")) && specs.gbw_hz.is_none() {
        specs.gbw_hz = Some(num);
        specs.provenance.push(format!("GBW={val} ({src})"));
    } else if k.contains("slew") && specs.slew_v_us.is_none() {
        specs.slew_v_us = Some(num);
        specs.provenance.push(format!("Slew={val} ({src})"));
    } else if (k == "vos" || k.contains("offset voltage")) && specs.vos_v.is_none() {
        specs.vos_v = Some(num);
        specs.provenance.push(format!("Vos={val} ({src})"));
    } else if (k.contains("vout") || k.contains("output voltage")) && specs.vout_v.is_none() {
        specs.vout_v = Some(num);
        specs.provenance.push(format!("Vout={val} ({src})"));
    } else if (k.contains("dropout") || k == "vdo") && specs.vdo_v.is_none() {
        specs.vdo_v = Some(num);
        specs.provenance.push(format!("Vdo={val} ({src})"));
    } else if (k.contains("quiescent") || k == "iq") && specs.iq_a.is_none() {
        specs.iq_a = Some(num);
        specs.provenance.push(format!("Iq={val} ({src})"));
    } else if (k.contains("output current") || k == "iout") && specs.iout_a.is_none() {
        specs.iout_a = Some(num);
        specs.provenance.push(format!("Iout={val} ({src})"));
    } else if (k.contains("voltage") || k == "v") && specs.voltage_rating_v.is_none() {
        specs.voltage_rating_v = Some(num);
    } else if (k.contains("power") || k == "p") && specs.power_w.is_none() {
        specs.power_w = Some(num);
    }
}

fn fmt_num(v: f64) -> String {
    let a = v.abs();
    if a == 0.0 {
        "0".into()
    } else if a < 1e-3 || a >= 1e5 {
        format!("{v:.4e}")
    } else {
        let s = format!("{v:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn extract_from_datasheet_text(specs: &mut ExtractedSpecs, text: &str) {
    let mut try_set = |dst: &mut Option<f64>, keys: &[&str], label: &str| {
        if dst.is_none()
            && let Some(v) = search_pdf_metric(text, keys)
        {
            *dst = Some(v);
            specs.provenance.push(format!("{label}={} (datasheet PDF)", fmt_num(v)));
        }
    };

    try_set(&mut specs.resistance_ohm, &["resistance"], "R");
    try_set(&mut specs.capacitance_f, &["capacitance"], "C");
    try_set(&mut specs.inductance_h, &["inductance"], "L");
    try_set(&mut specs.esr_ohm, &["esr", "equivalent series resistance"], "ESR");
    try_set(&mut specs.dcr_ohm, &["dcr", "dc resistance"], "DCR");

    try_set(&mut specs.vf_v, &["forward voltage", "vf "], "Vf");
    try_set(&mut specs.vz_v, &["zener voltage", "vz "], "Vz");
    try_set(&mut specs.vr_v, &["reverse voltage", "vrrm", "breakdown voltage"], "Vr");
    try_set(&mut specs.trr_s, &["reverse recovery time", "trr"], "trr");
    try_set(&mut specs.cjo_f, &["junction capacitance", "total capacitance", "cj "], "Cj");

    try_set(&mut specs.vds_max_v, &["drain-source voltage", "vdss", "vds"], "Vds");
    try_set(&mut specs.rds_on_ohm, &["rds(on)", "rds(on) ", "static drain-source on-resistance", "on-state resistance"], "Rds(on)");
    try_set(&mut specs.vgs_th_v, &["gate threshold voltage", "vgs(th)"], "Vgs(th)");
    try_set(&mut specs.ciss_f, &["input capacitance", "ciss"], "Ciss");
    try_set(&mut specs.coss_f, &["output capacitance", "coss"], "Coss");
    try_set(&mut specs.crss_f, &["reverse transfer capacitance", "crss"], "Crss");

    try_set(&mut specs.hfe, &["dc current gain", "hfe"], "hFE");
    try_set(&mut specs.vceo_v, &["collector-emitter voltage", "vceo"], "Vceo");
    try_set(&mut specs.ic_max_a, &["collector current"], "Ic");
    try_set(&mut specs.ft_hz, &["transition frequency", "current gain bandwidth product", "ft "], "fT");

    try_set(&mut specs.gbw_hz, &["gain bandwidth product", "gain-bandwidth", "gbwp", "gbw"], "GBW");
    try_set(&mut specs.slew_v_us, &["slew rate"], "Slew");
    try_set(&mut specs.vos_v, &["input offset voltage", "vos"], "Vos");
    try_set(&mut specs.iq_a, &["quiescent current", "supply current"], "Iq");
    try_set(&mut specs.vdo_v, &["dropout voltage"], "Vdo");
    try_set(&mut specs.vout_v, &["output voltage"], "Vout");
    try_set(&mut specs.iout_a, &["output current", "short-circuit current"], "Iout");
}

fn search_pdf_metric(text: &str, keys: &[&str]) -> Option<f64> {
    let lower = text.to_ascii_lowercase();
    for key in keys {
        let mut idx = 0usize;
        while let Some(rel) = lower[idx..].find(key) {
            let start = idx + rel + key.len();
            let window = &text[start..text.len().min(start + 72)];
            // Scan whitespace/delimiter separated tokens right after the label for a number+unit.
            for tok in window.split([' ', '\t', '\n', '\r', '=', ':', ',', ';']).filter(|t| !t.is_empty()).take(10) {
                if tok.chars().next().is_some_and(|c| c.is_ascii_digit())
                    && let Some(val) = parse_leading_si(tok)
                    && val > 0.0
                {
                    return Some(val);
                }
            }
            idx = start;
        }
    }
    None
}

fn infer_ldo_vout(mpn: &str, desc: &str) -> Option<f64> {
    let up = mpn.to_ascii_uppercase();
    for suffix in ["3.3", "33", "5.0", "50", "1.8", "18", "2.5", "25", "1.2", "12"] {
        if up.ends_with(suffix) || up.contains(&format!("-{suffix}")) {
            return match suffix {
                "3.3" | "33" => Some(3.3),
                "5.0" | "50" => Some(5.0),
                "1.8" | "18" => Some(1.8),
                "2.5" | "25" => Some(2.5),
                "1.2" | "12" => Some(1.2),
                _ => None,
            };
        }
    }
    find_unit_value(desc, &["v"])
}

fn find_unit_value(text: &str, units: &[&str]) -> Option<f64> {
    for tok in text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')')) {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        let lo = t.to_ascii_lowercase();
        let matches_unit = units.iter().any(|u| lo.ends_with(u));
        if (matches_unit || t.chars().any(|c| "kMmunpΩ\u{3a9}".contains(c)))
            && let Some(v) = parse_leading_si(t)
        {
            return Some(v);
        }
    }
    None
}

/// Parses a numeric value with optional SPICE/SI multiplier and unit (e.g. `"4.7k"`, `"28mOhm"`, `"100nF"`, `"1.6V"`).
pub fn parse_leading_si(raw: &str) -> Option<f64> {
    let s = raw.trim().replace(['µ', 'μ'], "u");
    let s = s.trim_start_matches(['±', '~', '<', '>']);
    let mut num_end = 0usize;
    let mut seen_digit = false;
    for (idx, c) in s.char_indices() {
        if c.is_ascii_digit() {
            seen_digit = true;
            num_end = idx + c.len_utf8();
        } else if (c == '.' && seen_digit) || ((c == '+' || c == '-') && idx == 0) {
            num_end = idx + c.len_utf8();
        } else {
            break;
        }
    }
    if !seen_digit {
        return None;
    }
    let val: f64 = s[..num_end].parse().ok()?;
    let rest = s[num_end..].trim();
    let mult = if rest.starts_with("Meg") || rest.starts_with("meg") || rest.starts_with("MOhm") || rest.starts_with("MΩ") || rest == "M" {
        1e6
    } else if rest.starts_with('k') || rest.starts_with('K') {
        1e3
    } else if rest.starts_with('G') {
        1e9
    } else if rest.starts_with("mOhm")
        || rest.starts_with("mΩ")
        || rest.starts_with("mV")
        || rest.starts_with("mA")
        || rest.starts_with("mH")
        || rest.starts_with("mF")
        || rest.starts_with("ms")
        || rest.starts_with("mW")
        || rest == "m"
    {
        1e-3
    } else if rest.starts_with('u') {
        1e-6
    } else if rest.starts_with('n') {
        1e-9
    } else if rest.starts_with('p') {
        1e-12
    } else if rest.starts_with('f') {
        1e-15
    } else {
        1.0
    };
    Some(val * mult)
}

/// Synthesizes a valid SPICE `.lib` file from `specs` (and optionally `similar` model),
/// returning `Ok(None)` when the component lacks enough electrical identification to generate a meaningful model.
pub fn generate_sim(
    part: &Part,
    pins: &[SymPin],
    specs: &ExtractedSpecs,
    similar: Option<&Candidate>,
    work_dir: &Path,
) -> Result<Option<(PathBuf, Vec<String>)>> {
    let Some(arch) = &specs.archetype else {
        return Ok(None);
    };
    // Avoid synthesizing hollow models for zero-spec passives unless we actually know a value or have a similar model.
    match arch {
        Archetype::Resistor if specs.resistance_ohm.is_none() && similar.is_none() => return Ok(None),
        Archetype::Capacitor if specs.capacitance_f.is_none() && similar.is_none() => return Ok(None),
        Archetype::Inductor if specs.inductance_h.is_none() && similar.is_none() => return Ok(None),
        _ => {}
    }

    std::fs::create_dir_all(work_dir)?;
    let model_name = sanitize_spice_name(&part.mpn);
    let mut notes = specs.provenance.clone();

    // If a similar `.model` candidate exists for the same semiconductor family, adapt its parameters directly.
    let body = if let Some(sim) = similar
        && matches!(sim.model.kind, SpiceKind::Model(_))
        && sim.model.device.eq_ignore_ascii_case(arch.spice_device())
    {
        notes.push(format!("adapted from similar model {}", sim.model.name));
        adapt_similar_primitive(&model_name, arch, specs, &sim.model.raw_block)
    } else {
        if let Some(sim) = similar {
            notes.push(format!("informed by similar model {} topology", sim.model.name));
        }
        synthesize_archetype(&model_name, arch, specs, pins)
    };

    let Some(spice_body) = body else {
        return Ok(None);
    };

    let mut header = format!(
        "* Generated SPICE model for {} ({})\n* Strategy: {}\n",
        part.mpn,
        if part.manufacturer.is_empty() { "unknown maker" } else { &part.manufacturer },
        if similar.is_some() { "datasheet/parametric specs + similar component model" } else { "datasheet PDF / parametric specs" }
    );
    for n in &notes {
        header.push_str(&format!("*   - {n}\n"));
    }
    header.push('\n');
    let full = format!("{header}{}\n", spice_body.trim());

    let out_path = work_dir.join(format!("{model_name}.lib"));
    std::fs::write(&out_path, &full)?;
    // Verify our own output parses cleanly as a SPICE model.
    if parse_models(&full).is_empty() {
        return Ok(None);
    }
    Ok(Some((out_path, notes)))
}

fn adapt_similar_primitive(target_name: &str, arch: &Archetype, specs: &ExtractedSpecs, raw_block: &str) -> Option<String> {
    let parsed = parse_models(raw_block);
    let orig = parsed.first()?;
    let mut kvs: Vec<(String, String)> = orig.params.clone();

    let mut set_kv = |k: &str, val: f64| {
        let formatted = fmt_num(val);
        if let Some(existing) = kvs.iter_mut().find(|(ek, _)| ek.eq_ignore_ascii_case(k)) {
            existing.1 = formatted;
        } else {
            kvs.push((k.to_ascii_uppercase(), formatted));
        }
    };

    match arch {
        Archetype::Diode(kind) => {
            if let Some(vf) = specs.vf_v {
                let if_test = specs.if_a.unwrap_or(0.01).max(1e-4);
                let n = if *kind == DiodeKind::Schottky { 1.08 } else { 1.75 };
                let rs = 0.05_f64;
                let v_j = (vf - if_test * rs).clamp(0.15, 4.5);
                let is = (if_test / (v_j / (n * 0.02585)).exp()).clamp(1e-22, 1e-4);
                set_kv("IS", is);
                set_kv("N", n);
            }
            if let Some(bv) = specs.vz_v.or(specs.vr_v) {
                set_kv("BV", bv);
            }
            if let Some(cjo) = specs.cjo_f {
                set_kv("CJO", cjo);
            }
            if let Some(trr) = specs.trr_s {
                set_kv("TT", trr * 0.6);
            }
        }
        Archetype::Mosfet { n_ch } => {
            if let Some(vth) = specs.vgs_th_v {
                set_kv("VTO", if *n_ch { vth.abs() } else { -vth.abs() });
            }
            if let Some(rds) = specs.rds_on_ohm {
                set_kv("RD", (rds * 0.5).max(1e-4));
                set_kv("RS", (rds * 0.35).max(1e-4));
            }
            if let Some(ciss) = specs.ciss_f {
                set_kv("CGS", ciss * 0.8);
            }
            if let Some(crss) = specs.crss_f {
                set_kv("CGDMAX", crss * 2.0);
                set_kv("CGDMIN", crss * 0.2);
            }
            if let Some(coss) = specs.coss_f {
                set_kv("CJO", coss * 0.9);
            }
        }
        Archetype::Bjt { .. } => {
            if let Some(bf) = specs.hfe {
                set_kv("BF", bf.clamp(5.0, 2000.0));
            }
            if let Some(vceo) = specs.vceo_v {
                set_kv("VAF", (vceo * 1.5).max(10.0));
            }
            if let Some(ic) = specs.ic_max_a {
                set_kv("IKF", ic);
            }
            if let Some(ft) = specs.ft_hz {
                let tf = (1.0 / (2.0 * std::f64::consts::PI * ft)).clamp(1e-12, 1e-6);
                set_kv("TF", tf);
            }
        }
        _ => {}
    }

    let kind_kw = match &orig.kind {
        SpiceKind::Model(t) => t.clone(),
        SpiceKind::Subckt => return None,
    };
    let pchan = if matches!(arch, Archetype::Mosfet { n_ch: false }) && kind_kw.eq_ignore_ascii_case("VDMOS") { " pchan" } else { "" };
    let params_str = kvs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ");
    Some(format!(".model {target_name} {kind_kw}{pchan} ({params_str})"))
}

fn synthesize_archetype(name: &str, arch: &Archetype, specs: &ExtractedSpecs, pins: &[SymPin]) -> Option<String> {
    match arch {
        Archetype::Resistor => {
            let r = fmt_num(specs.resistance_ohm?);
            let p1 = pins.first().map_or("1", |p| p.number.as_str());
            let p2 = pins.get(1).map_or("2", |p| p.number.as_str());
            Some(format!(".subckt {name} {p1} {p2}\nR_main {p1} {p2} {r}\n.ends {name}"))
        }
        Archetype::Capacitor => {
            let c = fmt_num(specs.capacitance_f?);
            let esr = fmt_num(specs.esr_ohm.unwrap_or(0.02));
            let p1 = pins.first().map_or("1", |p| p.number.as_str());
            let p2 = pins.get(1).map_or("2", |p| p.number.as_str());
            Some(format!(".subckt {name} {p1} {p2}\nR_esr {p1} n_mid {esr}\nC_main n_mid {p2} {c}\nR_leak n_mid {p2} 1G\n.ends {name}"))
        }
        Archetype::Inductor => {
            let l = fmt_num(specs.inductance_h?);
            let dcr = fmt_num(specs.dcr_ohm.unwrap_or(0.05));
            let p1 = pins.first().map_or("1", |p| p.number.as_str());
            let p2 = pins.get(1).map_or("2", |p| p.number.as_str());
            Some(format!(".subckt {name} {p1} {p2}\nR_dcr {p1} n_mid {dcr}\nL_main n_mid {p2} {l}\n.ends {name}"))
        }
        Archetype::Diode(kind) => {
            let (def_vf, n, def_trr, def_cjo) = match kind {
                DiodeKind::Schottky => (0.35, 1.08, 5e-9, 40e-12),
                DiodeKind::Zener => (0.75, 1.5, 20e-9, 30e-12),
                DiodeKind::Led => (2.1, 2.0, 50e-9, 25e-12),
                DiodeKind::Tvs => (0.85, 1.6, 5e-9, 80e-12),
                DiodeKind::Standard => (0.72, 1.75, 4e-9, 4e-12),
            };
            let vf = specs.vf_v.unwrap_or(def_vf);
            let if_test = specs.if_a.unwrap_or(0.01).max(1e-4);
            let rs = 0.05_f64;
            let v_j = (vf - if_test * rs).clamp(0.15, 4.5);
            let is = (if_test / (v_j / (n * 0.02585)).exp()).clamp(1e-22, 1e-4);
            let bv = fmt_num(specs.vz_v.or(specs.vr_v).unwrap_or(if *kind == DiodeKind::Zener { 5.1 } else { 75.0 }));
            let cjo = specs.cjo_f.unwrap_or(def_cjo);
            let tt = specs.trr_s.unwrap_or(def_trr) * 0.6;
            Some(format!(".model {name} D(IS={is:.3e} N={n:.2} RS={} CJO={cjo:.3e} TT={tt:.3e} BV={bv} IBV=5m)", fmt_num(rs)))
        }
        Archetype::Bjt { npn } => {
            let kind = if *npn { "NPN" } else { "PNP" };
            let bf = specs.hfe.unwrap_or(150.0).clamp(5.0, 2000.0);
            let vaf = specs.vceo_v.map_or(80.0, |v| (v * 1.5).max(15.0));
            let ikf = fmt_num(specs.ic_max_a.unwrap_or(0.2).max(1e-3));
            let tf = specs.ft_hz.map_or(0.5e-9, |f| (1.0 / (2.0 * std::f64::consts::PI * f)).clamp(1e-12, 1e-6));
            Some(format!(".model {name} {kind}(IS=1e-14 BF={bf:.1} VAF={vaf:.1} IKF={ikf} RC=0.5 RE=0.1 RB=10 CJE=15p CJC=8p TF={tf:.3e} TR=10n)"))
        }
        Archetype::Mosfet { n_ch } => {
            let vth_mag = specs.vgs_th_v.unwrap_or(1.8).abs().clamp(0.3, 8.0);
            let vto = if *n_ch { vth_mag } else { -vth_mag };
            let rds = specs.rds_on_ohm.unwrap_or(0.05).clamp(1e-4, 100.0);
            let rd = fmt_num(rds * 0.5);
            let rs = fmt_num(rds * 0.35);
            let kp = fmt_num((1.0 / (rds * 0.3)).clamp(0.05, 200.0));
            let cgs = specs.ciss_f.unwrap_or(400e-12) * 0.8;
            let crss = specs.crss_f.unwrap_or(40e-12);
            let cgdmax = crss * 2.0;
            let cgdmin = crss * 0.2;
            let cjo = specs.coss_f.unwrap_or(90e-12) * 0.9;
            let pchan = if *n_ch { "" } else { " pchan" };
            Some(format!(
                ".model {name} VDMOS{pchan} (VTO={vto:.3} KP={kp} RD={rd} RS={rs} RG=3 CGS={cgs:.3e} CGDMAX={cgdmax:.3e} CGDMIN={cgdmin:.3e} CJO={cjo:.3e} IS=1p RB={rd})"
            ))
        }
        Archetype::Jfet { n_ch } => {
            let kind = if *n_ch { "NJF" } else { "PJF" };
            let vto = specs.vgs_th_v.map_or(-2.0, |v| -v.abs());
            Some(format!(".model {name} {kind}(VTO={vto:.2} BETA=1m RD=10 RS=10 CGS=2p CGD=2p IS=1e-14)"))
        }
        Archetype::OpAmp | Archetype::Comparator => {
            let aol_db = specs.aol_db.unwrap_or(100.0);
            let gain = 10f64.powf(aol_db / 20.0).clamp(100.0, 1e6);
            let gbw = specs.gbw_hz.unwrap_or(1e6).clamp(1e4, 1e9);
            let fp = (gbw / gain).max(0.1);
            let c_comp = 100e-12_f64;
            let r_pole = 1.0 / (2.0 * std::f64::consts::PI * fp * c_comp);
            let gm = gain / r_pole;
            let vos = specs.vos_v.unwrap_or(0.0);
            let iq = specs.iq_a.unwrap_or(500e-6).clamp(1e-6, 50e-3);
            let rout = 50.0_f64;
            Some(format!(
                ".subckt {name} in+ in- vcc vee out\n\
                 Vos in+ in_off {vos:.4e}\n\
                 Rin in_off in- 10Meg\n\
                 G1 0 n_gain in_off in- {gm:.4e}\n\
                 Rp n_gain 0 {r_pole:.4e}\n\
                 Cp n_gain 0 {c_comp:.4e}\n\
                 B_out n_raw 0 V=min(v(vcc)-0.1, max(v(vee)+0.1, v(n_gain)))\n\
                 R_out n_raw out {rout:.2}\n\
                 I_q vcc vee {iq:.4e}\n\
                 .ends {name}"
            ))
        }
        Archetype::LinearRegulator => {
            let vout = specs.vout_v.unwrap_or(3.3).clamp(0.5, 48.0);
            let vdo = specs.vdo_v.unwrap_or(0.3).clamp(0.02, 3.0);
            let iq = specs.iq_a.unwrap_or(50e-6).clamp(1e-6, 20e-3);
            Some(format!(
                ".subckt {name} IN GND OUT\n\
                 R_in IN GND 1Meg\n\
                 I_q IN GND {iq:.4e}\n\
                 B_reg n_target GND V=min({vout:.4}, max(0, v(IN)-{vdo:.4}))\n\
                 R_out n_target OUT 0.05\n\
                 C_stab OUT GND 1p\n\
                 .ends {name}"
            ))
        }
    }
}
