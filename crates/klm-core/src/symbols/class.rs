//! Which house-style template a symbol gets. Every doubt is a refusal: a wrong template is worse than the vendor's symbol.

use crate::categories;
use crate::library::property;
use crate::rules::Rules;
use crate::sexpr::Sexpr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Resistor,
    Capacitor,
    Inductor,
    Diode,
    Led,
    Npn,
    Pnp,
    Ic,
}

impl Class {
    pub const ALL: [Class; 8] = [Class::Resistor, Class::Capacitor, Class::Inductor, Class::Diode, Class::Led, Class::Npn, Class::Pnp, Class::Ic];

    /// Reference designator prefix.
    pub fn prefix(self) -> &'static str {
        match self {
            Class::Resistor => "R",
            Class::Capacitor => "C",
            Class::Inductor => "L",
            Class::Diode | Class::Led => "D",
            Class::Npn | Class::Pnp => "Q",
            Class::Ic => "U",
        }
    }
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// The class of `sym` (a symbol in the `category` library), or why it has none.
/// `pins` is its pin count; the template must fit exactly.
pub fn detect(category: &str, sym: &Sexpr, pins: usize, rules: &Rules) -> Result<Class, String> {
    let text = ["Description", "ki_keywords", "Value"].iter().filter_map(|k| property(sym, k)).chain(sym.arg(0)).collect::<Vec<_>>().join(" ");
    // The library's own category first, then the user's keyword rules, then the symbol's text.
    // A guess from free text never makes an IC box: "ic" would match too many words.
    let mut coarse = "Misc";
    for (i, t) in [category.to_string(), rules.rule_category(&format!("{category} {text}")).unwrap_or("").to_string(), text.clone()].iter().enumerate() {
        let c = categories::guess(t);
        if c != "Misc" {
            coarse = c;
            if i == 2 && matches!(c, "ICs" | "Sensors" | "Power") {
                return Err("category unclear; not guessing an IC from its description".into());
            }
            break;
        }
    }
    let w = words(&format!("{category} {text}"));
    // Prefixes, like `guess`: "supercapacitors" and "varistors" must hit the exclusions too.
    let has = |list: &[&str]| w.iter().any(|x| list.iter().any(|l| x.starts_with(l)));
    let exact = |list: &[&str]| w.iter().any(|x| list.contains(&x.as_str()));
    let (class, count) = match coarse {
        "Resistors" if has(&["potentiometer", "trimmer", "thermistor", "ntc", "ptc", "varistor", "array", "network", "fuse"]) => {
            return Err("potentiometers, thermistors and arrays have no house style".into());
        }
        "Resistors" => (Class::Resistor, 2),
        "Capacitors" if has(&["polarized", "polarised", "polar", "electrolytic", "tantalum", "polymer", "supercap", "supercapacitor", "array"]) => {
            return Err("polarised capacitors have no house style".into());
        }
        "Capacitors" => (Class::Capacitor, 2),
        "Inductors" if has(&["ferrite", "bead", "transformer", "choke", "array", "emi", "filter"]) => {
            return Err("ferrite beads, chokes and transformers have no house style".into());
        }
        "Inductors" => (Class::Inductor, 2),
        // Vendor descriptions often say only "Diode <MPN>", so well-known TVS and Schottky families are told by part number.
        "Diodes"
            if has(&[
                "zener",
                "tvs",
                "schottky",
                "varactor",
                "bridge",
                "array",
                "dual",
                "photo",
                "esd",
                "bidirectional",
                "smaj",
                "smbj",
                "smcj",
                "smf",
                "p6ke",
                "sm6t",
                "ss1",
                "ss2",
                "ss3",
                "ss5",
                "1n58",
                "b5819",
                "bat4",
                "bat5",
                "rclamp",
            ]) =>
        {
            return Err("only plain diodes have a house style".into());
        }
        "Diodes" | "Optoelectronics" if exact(&["led", "leds"]) => (Class::Led, 2),
        "Diodes" => (Class::Diode, 2),
        "Transistors"
            if has(&[
                "mosfet",
                "fet",
                "jfet",
                "igbt",
                "nmos",
                "pmos",
                "darlington",
                "array",
                "digital",
                "brt",
                "biased",
                "prebiased",
                "phototransistor",
                "photo",
                "resistor",
            ]) =>
        {
            return Err("only plain bipolar transistors have a house style".into());
        }
        "Transistors" if exact(&["npn"]) && !exact(&["pnp"]) => (Class::Npn, 3),
        "Transistors" if exact(&["pnp"]) && !exact(&["npn"]) => (Class::Pnp, 3),
        "Transistors" => return Err("can't tell NPN from PNP".into()),
        "ICs" | "Sensors" | "Power" if has(&["battery", "fuse"]) => return Err("not an IC".into()),
        "ICs" | "Sensors" | "Power" if pins >= 3 => return Ok(Class::Ic),
        "ICs" | "Sensors" | "Power" => return Err(format!("an IC box needs at least 3 pins, it has {pins}")),
        other => return Err(format!("no house style for {other} parts")),
    };
    if pins != count {
        return Err(format!("{class:?} needs exactly {count} pins, it has {pins}"));
    }
    Ok(class)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr::parse;

    fn sym(desc: &str) -> Sexpr {
        parse(&format!(r#"(symbol "X1" (property "Description" "{desc}") (property "Value" "X1"))"#)).unwrap()
    }

    #[test]
    fn detects_by_category_then_text_and_refuses_doubt() {
        let r = Rules::default();
        assert_eq!(detect("Resistors", &sym(""), 2, &r), Ok(Class::Resistor));
        assert_eq!(detect("Misc", &sym("Chip resistor 10k"), 2, &r), Ok(Class::Resistor));
        assert_eq!(detect("Optoelectronics", &sym("Red LED"), 2, &r), Ok(Class::Led));
        assert_eq!(detect("Diodes", &sym("Rectifier"), 2, &r), Ok(Class::Diode));
        assert_eq!(detect("Transistors", &sym("NPN BJT"), 3, &r), Ok(Class::Npn));
        assert_eq!(detect("ICs", &sym("Op amp"), 8, &r), Ok(Class::Ic));
        assert!(detect("Resistors", &sym(""), 3, &r).is_err(), "pin count must fit");
        assert!(detect("Transistors", &sym("N-channel MOSFET"), 3, &r).is_err());
        assert!(detect("Transistors", &sym("transistor"), 3, &r).is_err(), "polarity unknown");
        assert!(detect("Capacitors", &sym("Tantalum"), 2, &r).is_err());
        assert!(detect("Diodes", &sym("Zener diode"), 2, &r).is_err());
        assert!(detect("Diodes", &sym("Diode SMCJ18A-13-F"), 2, &r).is_err(), "TVS by part number");
        assert!(detect("Diodes", &sym("Diode SS14"), 2, &r).is_err(), "Schottky by part number");
        assert!(detect("Connectors", &sym(""), 4, &r).is_err());
        assert!(detect("Power", &sym("Coin cell battery"), 3, &r).is_err());
    }
}
