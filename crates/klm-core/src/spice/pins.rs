//! KiCad symbol pin extraction and automatic `Sim.Pins` / `Sim.Device` mapping.

use super::{SpiceKind, SpiceModel};
use crate::sexpr::Sexpr;

/// One pin extracted from a KiCad symbol S-expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymPin {
    pub number: String,
    pub name: String,
    pub etype: String,
}

/// Extracts all unique electrical pins from `symbol`, resolving `(extends ...)` against `lib` when given.
pub fn extract_pins(symbol: &Sexpr, lib: Option<&Sexpr>) -> Vec<SymPin> {
    let mut pins = Vec::new();
    collect_pins_recursive(symbol, lib, &mut pins, 0);
    // Natural sort by numeric pin number first, then lexicographically.
    pins.sort_by(|a, b| match (a.number.parse::<u32>(), b.number.parse::<u32>()) {
        (Ok(na), Ok(nb)) => na.cmp(&nb),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => a.number.cmp(&b.number),
    });
    let mut seen = std::collections::HashSet::new();
    pins.retain(|p| seen.insert(p.number.clone()));
    pins
}

fn collect_pins_recursive(symbol: &Sexpr, lib: Option<&Sexpr>, out: &mut Vec<SymPin>, depth: u8) {
    if depth > 4 {
        return;
    }
    if let Some(parent_name) = symbol.child("extends").and_then(|c| c.arg(0))
        && let Some(parent) = lib.and_then(|l| l.children("symbol").find(|s| s.arg(0) == Some(parent_name)))
    {
        collect_pins_recursive(parent, lib, out, depth + 1);
    }
    for pin in symbol.children("pin") {
        if let Some(p) = parse_pin(pin) {
            out.push(p);
        }
    }
    for sub in symbol.children("symbol") {
        for pin in sub.children("pin") {
            if let Some(p) = parse_pin(pin) {
                out.push(p);
            }
        }
    }
}

fn parse_pin(pin: &Sexpr) -> Option<SymPin> {
    let etype = pin.arg(0).unwrap_or("passive").to_string();
    let number = pin.child("number")?.arg(0)?.trim().to_string();
    if number.is_empty() {
        return None;
    }
    let name = pin.child("name").and_then(|n| n.arg(0)).unwrap_or("~").trim().to_string();
    Some(SymPin { number, name, etype })
}

/// Maps `symbol` pins to SPICE nodes/ports for `model`, returning `(Sim.Pins, Sim.Device)`.
pub fn map_sim_pins(pins: &[SymPin], model: &SpiceModel, sym_ref: &str, category: &str) -> (Option<String>, String) {
    let device = infer_sim_device(pins, model, sym_ref, category);
    let mapped = match &model.kind {
        SpiceKind::Subckt => map_subckt_pins(pins, &model.ports, &device),
        SpiceKind::Model(_) => map_primitive_pins(pins, &device),
    };
    (mapped, device)
}

fn infer_sim_device(pins: &[SymPin], model: &SpiceModel, sym_ref: &str, category: &str) -> String {
    if !model.device.is_empty() && model.device != "SPICE" {
        return model.device.clone();
    }
    let ref_up = sym_ref.trim().to_ascii_uppercase();
    let cat_lo = category.to_ascii_lowercase();
    if matches!(model.kind, SpiceKind::Subckt) {
        return "SUBCKT".into();
    }
    if ref_up.starts_with('R') || cat_lo.contains("resistor") {
        "R".into()
    } else if ref_up.starts_with('C') || cat_lo.contains("capacitor") {
        "C".into()
    } else if ref_up.starts_with('L') || cat_lo.contains("inductor") {
        "L".into()
    } else if ref_up.starts_with('D') || cat_lo.contains("diode") || cat_lo.contains("led") {
        "D".into()
    } else if pins.len() == 3 && (ref_up.starts_with('Q') || cat_lo.contains("transistor") || cat_lo.contains("fet")) {
        let names: Vec<String> = pins.iter().map(|p| p.name.to_ascii_uppercase()).collect();
        if names.iter().any(|n| n == "G" || n == "GATE" || n == "D" || n == "DRAIN") { "NMOS".into() } else { "NPN".into() }
    } else {
        model.device.clone()
    }
}

fn map_primitive_pins(pins: &[SymPin], device: &str) -> Option<String> {
    if pins.is_empty() {
        return None;
    }
    match device {
        "D" if pins.len() >= 2 => {
            let mut k_pins = Vec::new();
            let mut a_pins = Vec::new();
            for p in pins {
                let n = p.name.to_ascii_lowercase();
                if matches!(n.as_str(), "k" | "c" | "cathode" | "-") || n.contains("cath") {
                    k_pins.push(p.number.as_str());
                } else if matches!(n.as_str(), "a" | "anode" | "+") || n.contains("anod") {
                    a_pins.push(p.number.as_str());
                }
            }
            if !k_pins.is_empty() && !a_pins.is_empty() {
                let mut pairs = Vec::new();
                for p in pins {
                    if k_pins.contains(&p.number.as_str()) {
                        pairs.push(format!("{}=K", p.number));
                    } else if a_pins.contains(&p.number.as_str()) {
                        pairs.push(format!("{}=A", p.number));
                    }
                }
                return Some(pairs.join(" "));
            }
            // KiCad Device:D convention has pin 1 = K, pin 2 = A.
            if pins.len() == 2 {
                return Some(format!("{}=K {}=A", pins[0].number, pins[1].number));
            }
            None
        }
        "NPN" | "PNP" if pins.len() >= 3 => {
            let mut pairs = Vec::new();
            for p in pins {
                let n = norm(&p.name);
                let role = if matches!(n.as_str(), "c" | "coll" | "collector") {
                    Some("C")
                } else if matches!(n.as_str(), "b" | "base") {
                    Some("B")
                } else if matches!(n.as_str(), "e" | "emit" | "emitter") {
                    Some("E")
                } else {
                    None
                };
                if let Some(r) = role {
                    pairs.push(format!("{}={r}", p.number));
                }
            }
            if pairs.len() >= 3 {
                return Some(pairs.join(" "));
            }
            if pins.len() == 3 {
                return Some(format!("{}=C {}=B {}=E", pins[0].number, pins[1].number, pins[2].number));
            }
            None
        }
        "NMOS" | "PMOS" | "NJFET" | "PJFET" if pins.len() >= 3 => {
            let mut pairs = Vec::new();
            let mut have = [false; 4]; // D, G, S, B
            for p in pins {
                let n = norm(&p.name);
                let role = if matches!(n.as_str(), "d" | "drain") {
                    have[0] = true;
                    Some("D")
                } else if matches!(n.as_str(), "g" | "gate") {
                    have[1] = true;
                    Some("G")
                } else if matches!(n.as_str(), "s" | "src" | "source") {
                    have[2] = true;
                    Some("S")
                } else if matches!(n.as_str(), "b" | "bulk" | "sub" | "substrate") {
                    have[3] = true;
                    Some("B")
                } else {
                    None
                };
                if let Some(r) = role {
                    pairs.push(format!("{}={r}", p.number));
                }
            }
            if have[0] && have[1] && have[2] {
                return Some(pairs.join(" "));
            }
            if pins.len() == 3 {
                // KiCad Q_NMOS_GDS vs GSD fallback: standard KiCad 3-pin FET defaults to G/D/S only if generic names,
                // while Simulation_SPICE Q_NMOS is 1=D 2=G 3=S.
                return Some(format!("{}=D {}=G {}=S", pins[0].number, pins[1].number, pins[2].number));
            }
            None
        }
        "R" | "C" | "L" if pins.len() == 2 => Some(format!("{}=1 {}=2", pins[0].number, pins[1].number)),
        "V" | "I" if pins.len() == 2 => Some(format!("{}=+ {}=-", pins[0].number, pins[1].number)),
        _ if pins.len() == 2 => Some(format!("{}=1 {}=2", pins[0].number, pins[1].number)),
        _ => None,
    }
}

fn map_subckt_pins(pins: &[SymPin], ports: &[String], _device: &str) -> Option<String> {
    if pins.is_empty() || ports.is_empty() {
        return None;
    }
    let mut assignments: Vec<Option<usize>> = vec![None; pins.len()];
    let mut used_ports = vec![false; ports.len()];

    // Pass 1: exact normalized name match or semantic alias match.
    for (pi, pin) in pins.iter().enumerate() {
        let p_norm = norm(&pin.name);
        if p_norm.is_empty() {
            continue;
        }
        let mut best: Option<(usize, i32)> = None;
        for (si, port) in ports.iter().enumerate() {
            let s_norm = norm(port);
            let score = if p_norm == s_norm {
                100
            } else if semantic_role(&pin.name).is_some() && semantic_role(&pin.name) == semantic_role(port) {
                if !used_ports[si] { 80 } else { 70 }
            } else if pin.number == port.trim() && pins.len() == ports.len() {
                50
            } else {
                0
            };
            if score > best.map_or(0, |b| b.1) {
                best = Some((si, score));
            }
        }
        if let Some((si, _)) = best {
            assignments[pi] = Some(si);
            used_ports[si] = true;
        }
    }

    // Pass 2: assign remaining pins positionally to unused ports (if counts match or remaining fit).
    for slot in &mut assignments {
        if slot.is_none()
            && let Some(si) = used_ports.iter().position(|&u| !u)
        {
            *slot = Some(si);
            used_ports[si] = true;
        }
    }

    // Only emit Sim.Pins if at least one meaningful semantic/exact match succeeded OR pin count == port count.
    let all_mapped = assignments.iter().all(Option::is_some);
    if !all_mapped && assignments.iter().flatten().count() < ports.len() {
        return None;
    }
    let pairs: Vec<String> = pins.iter().zip(assignments.iter()).filter_map(|(pin, slot)| slot.map(|si| format!("{}={}", pin.number, ports[si]))).collect();
    if pairs.is_empty() { None } else { Some(pairs.join(" ")) }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '-').flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Anode,
    Cathode,
    Collector,
    Base,
    Emitter,
    Drain,
    Gate,
    Source,
    InPlus,
    InMinus,
    Vcc,
    Vee,
    Out,
    Vin,
    Vout,
    Gnd,
    Enable,
    Feedback,
}

fn semantic_role(name: &str) -> Option<Role> {
    let n = norm(name);
    match n.as_str() {
        "a" | "anode" | "an" => Some(Role::Anode),
        "k" | "cathode" | "cath" | "ka" => Some(Role::Cathode),
        "c" | "coll" | "collector" => Some(Role::Collector),
        "b" | "base" => Some(Role::Base),
        "e" | "emit" | "emitter" => Some(Role::Emitter),
        "d" | "drain" => Some(Role::Drain),
        "g" | "gate" => Some(Role::Gate),
        "s" | "src" | "source" => Some(Role::Source),
        "in+" | "+in" | "inp" | "vinp" | "noninv" | "+" => Some(Role::InPlus),
        "in-" | "-in" | "inm" | "inn" | "vinm" | "vinn" | "inv" | "-" => Some(Role::InMinus),
        "vcc" | "vdd" | "v+" | "vs+" | "vp" | "vpos" | "avdd" => Some(Role::Vcc),
        "vee" | "vss" | "v-" | "vs-" | "vn" | "vneg" | "avss" => Some(Role::Vee),
        "out" | "vo" | "output" => Some(Role::Out),
        "vout" => Some(Role::Vout),
        "in" | "vin" => Some(Role::Vin),
        "gnd" | "com" | "common" | "vssgnd" | "0" => Some(Role::Gnd),
        "en" | "enable" | "shdn" | "ce" => Some(Role::Enable),
        "fb" | "adj" | "vsense" | "sense" => Some(Role::Feedback),
        _ => None,
    }
}
