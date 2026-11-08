//! Part information per component class: which fields a part of that class carries, how their values
//! are written, and the Description, Value and Keywords built from them.
//!
//! The schema lives in the active standard profile (`Profile::classes`); a class the profile leaves out
//! uses [`default_class`]. Class keys: Resistor, Capacitor, Inductor, Diode, Led, Transistor (NPN and PNP
//! alike, the Polarity field tells them apart), Ic and Generic.
//! KiCad 8/9 name the description property `Description` and the keywords `ki_keywords`.

use crate::categories;
use crate::library::{property, set_property};
use crate::matching::{self, Kind};
use crate::part::Part;
use crate::sexpr::Sexpr;
use crate::standardise::profile::Profile;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// One field of a class.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldDef {
    pub name: String,
    pub required: bool,
    /// Shown on the schematic. Recorded for the editors; nothing places a new field on the sheet yet.
    pub visible: bool,
    /// "F", "Ω", "H", "V", "A", "W" or "%": the value is a number with an SI prefix and is written in one canonical form. None: free text.
    pub unit: Option<String>,
    /// When not empty, the only values allowed (compared ignoring case).
    pub allowed: Vec<String>,
    /// Names the sources use for this field's parameter ("Voltage - Rated"); the source mapping table.
    pub aliases: Vec<String>,
}

/// The fields of one class and the templates that build its text. A template is text with `{Field}`
/// placeholders; a placeholder whose field is missing disappears. An empty template leaves that property alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClassInfo {
    pub fields: Vec<FieldDef>,
    pub description: String,
    pub value: String,
    pub keywords: String,
}

fn f(name: &str, required: bool, unit: Option<&str>, aliases: &[&str]) -> FieldDef {
    FieldDef { name: name.into(), required, visible: false, unit: unit.map(Into::into), allowed: vec![], aliases: aliases.iter().map(|a| a.to_string()).collect() }
}

/// The built-in schema of a class (see the module docs for the keys); Generic for any other key.
pub fn default_class(key: &str) -> ClassInfo {
    let package = f("Package", false, None, &["Package / Case", "Supplier Device Package", "Case/Package", "Package"]);
    let tol = |a: &[&str]| f("Tolerance", false, Some("%"), a);
    let (mut fields, description, value, keywords) = match key {
        "Capacitor" => (
            vec![
                f("Capacitance", true, Some("F"), &["Capacitance"]),
                f("Voltage", true, Some("V"), &["Voltage - Rated", "Voltage Rating", "Rated Voltage", "Voltage Rating - DC"]),
                tol(&["Tolerance", "Capacitance Tolerance"]),
                f("Dielectric", false, None, &["Temperature Coefficient", "Dielectric", "Dielectric Material", "Characteristic"]),
                package,
            ],
            "Capacitor {Capacitance} {Voltage} {Tolerance} {Dielectric} {Package}",
            "{Capacitance}",
            "capacitor cap {Capacitance} {Voltage} {Dielectric} {Package}",
        ),
        "Resistor" => (
            vec![
                f("Resistance", true, Some("Ω"), &["Resistance"]),
                tol(&["Tolerance", "Resistance Tolerance"]),
                f("Power", false, Some("W"), &["Power (Watts)", "Power Rating", "Power"]),
                f("Temp coefficient", false, None, &["Temperature Coefficient", "TCR"]),
                package,
            ],
            "Resistor {Resistance} {Tolerance} {Power} {Temp coefficient} {Package}",
            "{Resistance}",
            "resistor res {Resistance} {Package}",
        ),
        "Inductor" => (
            vec![
                f("Inductance", true, Some("H"), &["Inductance"]),
                f("Current", false, Some("A"), &["Current Rating (Amps)", "Current Rating", "Current - Rated"]),
                f("Saturation current", false, Some("A"), &["Current - Saturation (Isat)", "Current - Saturation"]),
                f("DCR", false, Some("Ω"), &["DC Resistance (DCR)", "DCR", "DC Resistance"]),
                tol(&["Tolerance", "Inductance Tolerance"]),
            ],
            "Inductor {Inductance} {Tolerance} {Current} {Saturation current} {DCR}",
            "{Inductance}",
            "inductor coil {Inductance} {Current}",
        ),
        "Diode" => (
            vec![
                f("Vr", false, Some("V"), &["Voltage - DC Reverse (Vr) (Max)", "Reverse Voltage", "Vr"]),
                f("If", false, Some("A"), &["Current - Average Rectified (Io)", "Current - DC Forward (If) (Max)", "Forward Current", "If"]),
                f("Vf", false, Some("V"), &["Voltage - Forward (Vf) (Max)", "Voltage - Forward (Vf) (Typ)", "Forward Voltage", "Vf"]),
            ],
            "Diode {MPN} {Vr} {If} {Vf}",
            "",
            "diode {Vr} {If}",
        ),
        "Led" => (
            vec![
                f("Vf", false, Some("V"), &["Voltage - Forward (Vf) (Typ)", "Voltage - Forward (Vf) (Max)", "Forward Voltage", "Vf"]),
                f("If", false, Some("A"), &["Current - DC Forward (If) (Max)", "Forward Current", "If"]),
                f("Colour", false, None, &["Color", "Colour"]),
                f("Wavelength", false, None, &["Wavelength - Dominant", "Wavelength"]),
            ],
            "LED {Colour} {Wavelength} {Vf} {If}",
            "",
            "led {Colour} {Wavelength}",
        ),
        "Transistor" => (
            vec![
                f("Vce/Vds", false, Some("V"), &["Voltage - Collector Emitter Breakdown (Max)", "Drain to Source Voltage (Vdss)", "Vce", "Vds"]),
                f("Ic/Id", false, Some("A"), &["Current - Collector (Ic) (Max)", "Current - Continuous Drain (Id) @ 25°C", "Ic", "Id"]),
                f("Rds(on)", false, Some("Ω"), &["Rds On (Max) @ Id, Vgs", "Rds(on)"]),
                f("Polarity", false, None, &["Transistor Type", "Polarity"]),
            ],
            "{Polarity} transistor {MPN} {Vce/Vds} {Ic/Id} {Rds(on)}",
            "",
            "transistor {Polarity}",
        ),
        "Ic" => (
            vec![
                f("Supply range", false, None, &["Voltage - Supply", "Supply Voltage", "supplyvoltage"]),
                f("Interface", false, None, &["Interface"]),
                f("Temp range", false, None, &["Operating Temperature"]),
            ],
            "{MPN} {Interface} {Supply range}",
            "",
            "ic {Interface}",
        ),
        _ => (vec![], "", "", ""),
    };
    fields.extend([
        f("Manufacturer", true, None, &["Manufacturer"]),
        f("MPN", true, None, &["Manufacturer Part Number", "MPN"]),
        f("Datasheet", false, None, &[]),
        f("Supplier", false, None, &[]),
        f("Supplier PN", false, None, &[]),
    ]);
    ClassInfo { fields, description: description.into(), value: value.into(), keywords: keywords.into() }
}

impl Profile {
    /// The schema of class `key`: this profile's, or the built-in default when the profile has none.
    pub fn class(&self, key: &str) -> ClassInfo {
        self.classes.get(key).cloned().unwrap_or_else(|| default_class(key))
    }
}

/// The class a symbol in the `category` library belongs to. Looser than `symbols::class::detect`, which
/// also needs pins and a template: here a wrong guess only costs a missing-field warning.
pub fn class_key(category: &str, sym: &Sexpr) -> &'static str {
    let text = ["Description", "ki_keywords", "Value"].iter().filter_map(|k| property(sym, k)).chain(sym.arg(0)).collect::<Vec<_>>().join(" ");
    let coarse = [category, text.as_str()].into_iter().map(categories::guess).find(|c| *c != "Misc").unwrap_or("Misc");
    let led = text.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| w == "led" || w == "leds");
    match coarse {
        "Resistors" => "Resistor",
        "Capacitors" => "Capacitor",
        "Inductors" => "Inductor",
        "Diodes" | "Optoelectronics" if led => "Led",
        "Diodes" => "Diode",
        "Transistors" => "Transistor",
        "ICs" | "Sensors" | "Power" => "Ic",
        _ => "Generic",
    }
}

/// `value` written once, the house way: `100nF`, `4.7kΩ`, `10%`. None when it isn't a number in `unit`.
/// `0.1uF`, `100n` and `100 nF` all come out as `100nF`; the parsing is `matching::value_key`'s.
pub fn normalise(unit: &str, value: &str) -> Option<String> {
    let t = value.trim().trim_start_matches(['±', '+']).trim();
    let kind = match unit {
        "Ω" => Some(Kind::Res),
        "F" => Some(Kind::Cap),
        "H" => Some(Kind::Ind),
        _ => None,
    };
    let key = if kind.is_some() {
        // value_key takes "10 kΩ" but not "10 kOhms".
        let mut words = t.split_whitespace();
        let joined = match (words.next(), words.next(), words.next()) {
            (Some(a), Some(b), None) => format!("{a}{b}"),
            _ => t.to_string(),
        };
        matching::value_key(&joined, kind, true)?
    } else {
        let lower_unit = unit.to_lowercase();
        let n = t.chars().count().saturating_sub(unit.chars().count());
        let bare: String = if t.to_lowercase().ends_with(&lower_unit) { t.chars().take(n).collect() } else { t.to_string() };
        // Power ratings come as fractions of a watt ("1/10W").
        let bare = match bare.trim().split_once('/').map(|(a, b)| (a.trim().parse::<f64>(), b.trim().parse::<f64>())) {
            Some((Ok(a), Ok(b))) if b != 0.0 => (a / b).to_string(),
            _ => bare,
        };
        matching::value_key(bare.trim(), None, true)?
    };
    let (digits, exp) = key.strip_prefix("v:")?.get(1..)?.split_once('e')?;
    let exp: i32 = exp.parse().ok()?;
    if digits == "0" {
        return Some(format!("0{unit}"));
    }
    // Decimal exponent of the leading digit, then the SI prefix at or below it.
    let lead = exp + digits.len() as i32 - 1;
    let p = if unit == "%" { 0 } else { (lead.div_euclid(3) * 3).clamp(-12, 9) };
    let before = (lead - p + 1) as isize;
    let mantissa = if before <= 0 {
        format!("0.{}{digits}", "0".repeat(-before as usize))
    } else if before as usize >= digits.len() {
        format!("{digits}{}", "0".repeat(before as usize - digits.len()))
    } else {
        format!("{}.{}", &digits[..before as usize], &digits[before as usize..])
    };
    let prefix = match p {
        -12 => "p",
        -9 => "n",
        -6 => "u",
        -3 => "m",
        3 => "k",
        6 => "M",
        9 => "G",
        _ => "",
    };
    Some(format!("{mantissa}{prefix}{unit}"))
}

/// Fills the `{Field}` placeholders of `template` from `get`; a missing field leaves no gap behind.
pub fn render(template: &str, get: &dyn Fn(&str) -> Option<String>) -> String {
    let (mut out, mut rest) = (String::new(), template);
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        };
        out.push_str(&get(&rest[i + 1..i + j]).unwrap_or_default());
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The names inside the `{}` of a template.
fn find_placeholders(template: &str) -> impl Iterator<Item = &str> {
    template.split('{').skip(1).filter_map(|s| s.split_once('}').map(|(n, _)| n))
}

/// The symbol's field `name`, trimmed; None when missing or empty.
pub fn field(sym: &Sexpr, name: &str) -> Option<String> {
    property(sym, name).map(str::trim).filter(|v| !v.is_empty()).map(String::from)
}

/// What `info`'s templates give for this symbol: (property, rendered text). A template none of whose
/// fields the symbol has is skipped, so a bare part isn't given a Description of just "Resistor".
pub fn rebuilt(sym: &Sexpr, info: &ClassInfo) -> Vec<(&'static str, String)> {
    [("Description", &info.description), ("Value", &info.value), ("ki_keywords", &info.keywords)]
        .into_iter()
        .filter(|(_, t)| !t.is_empty())
        .filter(|(_, t)| find_placeholders(t).any(|n| field(sym, n).is_some()))
        .map(|(k, t)| (k, render(t, &|n| field(sym, n))))
        .collect()
}

/// Each field of `info` with the value `part` gives it, in the house form: the part's own data
/// (manufacturer, MPN, datasheet, supplier) or the source parameter an alias names. A parameter that
/// should be a number in a unit and isn't ("Jumper") is skipped.
pub fn autofill(info: &ClassInfo, part: &Part) -> Vec<(String, String)> {
    let param = |fd: &FieldDef| {
        part.params.iter().find(|(n, _)| n.eq_ignore_ascii_case(&fd.name) || fd.aliases.iter().any(|a| a.eq_ignore_ascii_case(n))).map(|(_, v)| v.trim().to_string())
    };
    info.fields
        .iter()
        .filter_map(|fd| {
            let own = match fd.name.as_str() {
                "Manufacturer" => Some(part.manufacturer.clone()),
                "MPN" => Some(part.mpn.clone()),
                "Datasheet" => Some(part.datasheet_url.clone()),
                "Supplier" => part.supplier_ids.first().map(|s| s.0.clone()),
                "Supplier PN" => part.supplier_ids.first().map(|s| s.1.clone()),
                "Package" => Some(part.package.clone()),
                _ => None,
            };
            let raw = param(fd).filter(|v| v != "-" && !v.is_empty()).or(own)?;
            // "0.1W, 1/10W" and "0603 (1608 Metric)": the first form is the value.
            let cut = if fd.unit.is_some() { raw.split([',', '(', '@']).next() } else if fd.name == "Package" { raw.split('(').next() } else { Some(raw.as_str()) };
            let v = cut?.trim();
            let v = match &fd.unit {
                Some(u) => normalise(u, v)?,
                None => v.to_string(),
            };
            (!v.is_empty()).then(|| (fd.name.clone(), v))
        })
        .collect()
}

/// Brings `sym` to the profile's standard for its class: fills fields it lacks from `part` (when known),
/// writes unit values in the canonical form and rebuilds Description, Value and Keywords. Returns what changed.
pub fn fix(sym: &mut Sexpr, category: &str, profile: &Profile, part: Option<&Part>) -> Vec<String> {
    let info = profile.class(class_key(category, sym));
    let mut changes = vec![];
    for (name, v) in part.map(|p| autofill(&info, p)).unwrap_or_default() {
        if field(sym, &name).is_none() {
            set_property(sym, &name, &v);
            changes.push(format!("{name}: added {v}"));
        }
    }
    for fd in &info.fields {
        let (Some(unit), Some(cur)) = (&fd.unit, field(sym, &fd.name)) else { continue };
        if let Some(n) = normalise(unit, &cur).filter(|n| *n != cur) {
            set_property(sym, &fd.name, &n);
            changes.push(format!("{}: {cur} -> {n}", fd.name));
        }
    }
    for (key, new) in rebuilt(sym, &info) {
        if property(sym, key) != Some(new.as_str()) {
            changes.push(format!("{key}: \"{}\" -> \"{new}\"", property(sym, key).unwrap_or_default()));
            set_property(sym, key, &new);
        }
    }
    changes
}

/// The profile the library features use, and whether imports apply it. Set by `symbols::configure`.
static ACTIVE: Mutex<Option<(Profile, bool)>> = Mutex::new(None);

pub fn configure(profile: &Profile, on_import: bool) {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some((profile.clone(), on_import));
}

/// The configured profile, or the built-in one before anything is configured.
pub fn active() -> Profile {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|a| a.0.clone()).unwrap_or_default()
}

/// Import step: fills and rebuilds the part information of the symbol about to be written, if house style on import is on.
pub(crate) fn fix_imported(category: &str, symbol: &mut Sexpr, part: &Part) {
    let on = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|a| (a.0.clone(), a.1));
    if let Some((profile, true)) = on {
        fix(symbol, category, &profile, Some(part));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr::parse;

    #[test]
    fn units_have_one_form() {
        for v in ["100nF", "0.1uF", "100n", "100 nF", "0.1 µF", "100NF"] {
            assert_eq!(normalise("F", v).as_deref(), Some("100nF"), "{v}");
        }
        assert_eq!(normalise("Ω", "4k7").as_deref(), Some("4.7kΩ"));
        assert_eq!(normalise("Ω", "10 kOhms").as_deref(), Some("10kΩ"));
        assert_eq!(normalise("Ω", "0 Ohms").as_deref(), Some("0Ω"));
        assert_eq!(normalise("Ω", "R47").as_deref(), Some("470mΩ"));
        assert_eq!(normalise("H", "4.7uH").as_deref(), Some("4.7uH"));
        assert_eq!(normalise("V", "16 V").as_deref(), Some("16V"));
        assert_eq!(normalise("V", "0.0033").as_deref(), Some("3.3mV"));
        assert_eq!(normalise("A", "1500mA").as_deref(), Some("1.5A"));
        assert_eq!(normalise("W", "1/10W").as_deref(), Some("100mW"));
        assert_eq!(normalise("%", "±10%").as_deref(), Some("10%"));
        assert_eq!(normalise("%", "0.5 %").as_deref(), Some("0.5%"));
        assert_eq!(normalise("F", "Jumper"), None);
        assert_eq!(normalise("V", "3V ~ 32V"), None);
    }

    #[test]
    fn templates_drop_missing_fields_cleanly() {
        let get = |n: &str| match n {
            "A" => Some("1k".to_string()),
            "C" => Some("0603".to_string()),
            _ => None,
        };
        assert_eq!(render("Res {A} {B} {C}", &get), "Res 1k 0603");
        assert_eq!(render("{B}", &get), "");
        assert_eq!(render("open {A", &get), "open {A");
    }

    #[test]
    fn defaults_cover_every_class_and_unknown_profiles_fall_back() {
        for key in ["Resistor", "Capacitor", "Inductor", "Diode", "Led", "Transistor", "Ic", "Generic"] {
            let c = default_class(key);
            for common in ["Manufacturer", "MPN", "Datasheet", "Supplier", "Supplier PN"] {
                assert!(c.fields.iter().any(|f| f.name == common), "{key} lacks {common}");
            }
            assert!(!c.description.is_empty() || key == "Generic");
        }
        let cap: Vec<_> = default_class("Capacitor").fields.into_iter().map(|f| f.name).take(5).collect();
        assert_eq!(cap, ["Capacitance", "Voltage", "Tolerance", "Dielectric", "Package"]);
        let mut p = Profile::default();
        assert_eq!(p.class("Resistor"), default_class("Resistor"));
        p.classes.insert("Resistor".into(), ClassInfo { description: "R".into(), ..Default::default() });
        assert_eq!(p.class("Resistor").description, "R");
        assert_eq!(p.class("Ic"), default_class("Ic"));
    }

    #[test]
    fn class_follows_category_then_text() {
        let s = |v: &str| parse(&format!(r#"(symbol "X" (property "Value" "{v}"))"#)).unwrap();
        assert_eq!(class_key("Capacitors", &s("x")), "Capacitor");
        assert_eq!(class_key("Misc", &s("red LED")), "Led");
        assert_eq!(class_key("Connectors", &s("x")), "Generic");
    }

    #[test]
    fn fix_normalises_and_rebuilds() {
        let mut sym = parse(
            r#"(symbol "C1" (property "Value" "C1") (property "Description" "cap") (property "Capacitance" "0.1uF") (property "Voltage" "16 V") (property "Package" "0603"))"#,
        )
        .unwrap();
        let changes = fix(&mut sym, "Capacitors", &Profile::default(), None);
        assert_eq!(property(&sym, "Capacitance"), Some("100nF"));
        assert_eq!(property(&sym, "Value"), Some("100nF"));
        assert_eq!(property(&sym, "Description"), Some("Capacitor 100nF 16V 0603"));
        assert_eq!(property(&sym, "ki_keywords"), Some("capacitor cap 100nF 16V 0603"));
        assert!(changes.contains(&"Capacitance: 0.1uF -> 100nF".to_string()), "{changes:?}");
        assert!(fix(&mut sym, "Capacitors", &Profile::default(), None).is_empty(), "second run is a no-op");
    }
}
