//! Electrical pin types for symbols whose source left them `unspecified`. Only that type is ever
//! replaced: a vendor's own choice wins, and a name we can't read stays `unspecified`, since a wrong
//! type makes ERC lie while a missing one only makes it warn.

use crate::sexpr::Sexpr;

/// The type a pin called `name` almost certainly has. `category` is the library the symbol sits in.
pub(crate) fn guess(name: &str, category: &str) -> Option<&'static str> {
    // "~{RESET}" (KiCad overbar), "/CS", "nRST" and "RESET#" are all active-low spellings of the same pin.
    let n = name.trim().to_uppercase().replace(['~', '{', '}', '#', '/', ' '], "");
    let n = n.strip_prefix('N').filter(|r| ACTIVE_LOW.contains(r)).unwrap_or(n.as_str());
    let cat = category.to_lowercase();
    let digits_after = |p: &str| n.strip_prefix(p).is_some_and(|r| r.chars().all(|c| c.is_ascii_digit() || c == '_'));
    Some(match n {
        // Relays and switches use NC for "normally closed", a real contact.
        "NC" | "N.C." | "DNC" | "NIC" if !["relay", "switch"].iter().any(|w| cat.contains(w)) => "no_connect",
        "EP" | "EPAD" | "PAD" | "TAB" | "SHIELD" | "MH" => "passive",
        _ if ["GND", "AGND", "DGND", "PGND", "SGND", "VSS", "VSSA", "AVSS", "DVSS", "VEE", "V-"].iter().any(|p| digits_after(p)) => "power_in",
        _ if ["VCC", "VDD", "AVDD", "DVDD", "VDDA", "VDDIO", "VCCIO", "AVCC", "VIN", "PVIN", "VBAT", "VBUS", "VS", "VS+", "V+", "3V3", "5V"]
            .iter()
            .any(|p| digits_after(p)) =>
        {
            "power_in"
        }
        "VOUT" | "VO" if cat.contains("power") || cat.contains("regulator") => "power_out",
        "OUT" | "VOUT" | "VO" | "DOUT" | "TX" | "TXD" | "SDO" | "INT" | "IRQ" | "ALERT" | "PG" | "PGOOD" => "output",
        "IN" | "DIN" | "RX" | "RXD" | "SDI" | "EN" | "CE" | "CS" | "SS" | "RST" | "RESET" | "MR" | "SHDN" | "WP" | "HOLD" | "SCK" | "SCLK" | "CLK" => "input",
        "SDA" | "SDIO" | "SWDIO" => "bidirectional",
        // GPIO-style names: IO3, GPIO12, PA0, PB15, P0.1, P1_7.
        _ if digits_after("IO") || digits_after("GPIO") || port_pin(n) => "bidirectional",
        _ => return None,
    })
}

const ACTIVE_LOW: &[&str] = &["RST", "RESET", "CS", "SS", "EN", "CE", "SHDN", "WP", "HOLD", "MR", "INT", "IRQ"];

/// PA0 … PK15 or P0.1 / P1_7.
fn port_pin(n: &str) -> bool {
    let Some(r) = n.strip_prefix('P') else { return false };
    let mut c = r.chars();
    match c.next() {
        Some(port) if port.is_ascii_uppercase() => r.len() > 1 && c.all(|d| d.is_ascii_digit()),
        Some(d) if d.is_ascii_digit() => r.split(['.', '_']).count() == 2 && r.split(['.', '_']).all(|s| !s.is_empty() && s.chars().all(|d| d.is_ascii_digit())),
        _ => false,
    }
}

fn pins_mut(sym: &mut Sexpr) -> Vec<&mut Sexpr> {
    let mut out = vec![];
    for unit in sym.items_mut().iter_mut().filter(|u| u.head() == Some("symbol")) {
        out.extend(unit.items_mut().iter_mut().filter(|p| p.head() == Some("pin")));
    }
    out
}

/// Fills in `unspecified` pins of one top-level symbol and says what changed. A two-pin part with no
/// types at all is passive (resistors, capacitors, crystals, fuses).
pub(crate) fn fill(sym: &mut Sexpr, category: &str) -> Option<String> {
    let mut pins = pins_mut(sym);
    let unknown = |p: &Sexpr| matches!(p.arg(0), Some("unspecified") | None);
    let two_terminal = pins.len() == 2 && pins.iter().all(|p| unknown(p));
    let (mut set, mut left) = (0, 0);
    for p in pins.iter_mut().filter(|p| unknown(p)) {
        let name = p.child("name").and_then(|n| n.arg(0)).unwrap_or("").to_string();
        match if two_terminal { Some("passive") } else { guess(&name, category) } {
            Some(t) => {
                p.set_arg(0, Sexpr::Atom(t.into()));
                set += 1;
            }
            None => left += 1,
        }
    }
    (set > 0).then(|| match left {
        0 => format!("set the electrical type of {set} pin(s)"),
        _ => format!("set the electrical type of {set} pin(s); {left} left unspecified (name gives no clue)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr::parse;

    #[test]
    fn names_map_to_types() {
        for (name, want) in [
            ("GND", Some("power_in")),
            ("AGND2", Some("power_in")),
            ("VDD_1", Some("power_in")),
            ("~{RESET}", Some("input")),
            ("nCS", Some("input")),
            ("RST#", Some("input")),
            ("PA10", Some("bidirectional")),
            ("P0.1", Some("bidirectional")),
            ("GPIO12", Some("bidirectional")),
            ("TXD", Some("output")),
            ("NC", Some("no_connect")),
            ("EP", Some("passive")),
            ("FB", None),
            ("PAD", Some("passive")),
            ("PWR", None),
            ("NODE", None),
        ] {
            assert_eq!(guess(name, "ICs"), want, "{name}");
        }
        assert_eq!(guess("NC", "Relays"), None, "relay NC is a contact");
        assert_eq!(guess("VOUT", "Power"), Some("power_out"));
        assert_eq!(guess("VOUT", "ICs"), Some("output"));
    }

    #[test]
    fn fill_keeps_vendor_types_and_makes_two_pin_parts_passive() {
        let mut ic = parse(r#"(symbol "U" (symbol "U_1_1" (pin unspecified line (name "GND") (number "1")) (pin output line (name "VCC") (number "2")) (pin unspecified line (name "FB") (number "3"))))"#).unwrap();
        assert_eq!(fill(&mut ic, "ICs").as_deref(), Some("set the electrical type of 1 pin(s); 1 left unspecified (name gives no clue)"));
        let types: Vec<_> = ic.child("symbol").unwrap().children("pin").map(|p| p.arg(0).unwrap()).collect();
        assert_eq!(types, ["power_in", "output", "unspecified"]);

        let mut r = parse(r#"(symbol "R" (symbol "R_1_1" (pin unspecified line (name "1") (number "1")) (pin unspecified line (name "2") (number "2"))))"#).unwrap();
        fill(&mut r, "Resistors");
        assert!(r.child("symbol").unwrap().children("pin").all(|p| p.arg(0) == Some("passive")));
        assert_eq!(fill(&mut r, "Resistors"), None, "nothing left to do");
    }
}
