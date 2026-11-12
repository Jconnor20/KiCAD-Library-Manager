use super::*;
use crate::sexpr::{parse, to_string};

fn rules() -> rules::Rules {
    rules::Rules::default()
}

fn std_rules() -> Profile {
    Profile::default()
}

/// A vendor-style symbol: odd geometry, hidden fields, a footprint link and a few custom fields.
fn vendor(name: &str, reference: &str, desc: &str, pins: &[(&str, &str, &str, bool)]) -> String {
    let pin = |(typ, num, pname, hide): &(&str, &str, &str, bool), i: usize| {
        format!(
            r#"(pin {typ} line (at {} {} 0) (length 1) {}(name "{pname}" (effects (font (size 1 1)))) (number "{num}" (effects (font (size 1 1)))) (alternate "ALT{num}" input line))"#,
            -7 - i as i32, 3 * i, if *hide { "(hide yes) " } else { "" }
        )
    };
    let pins: String = pins.iter().enumerate().map(|(i, p)| pin(p, i)).collect();
    format!(
        r#"(symbol "{name}" (pin_names (offset 0.5)) (exclude_from_sim no) (in_bom yes) (on_board yes)
        (property "Reference" "{reference}" (at 9 9 0) (effects (font (size 2 2))))
        (property "Value" "{name}" (at 9 9 0) (effects (font (size 1.27 1.27))))
        (property "Footprint" "Vendor:FP_{name}" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes)))
        (property "Datasheet" "https://example.com/{name}.pdf" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes)))
        (property "Description" "{desc}" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes)))
        (property "MPN" "{name}" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes)))
        (property "Alternates" "ALT-1;ALT-2" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes)))
        (symbol "{name}_0_1" (rectangle (start -3 3) (end 3 -3) (stroke (width 0.1) (type default)) (fill (type none))))
        (symbol "{name}_1_1" {pins})
        (embedded_fonts no))"#
    )
}

fn pin_summary(s: &Sexpr) -> Vec<(String, String, String)> {
    let mut v: Vec<_> = source_pins(s).unwrap().iter().map(|p| (pin_number(p).to_string(), pin_name(p).to_string(), pin_type(p).to_string())).collect();
    v.sort();
    v
}

fn restyled(name: &str, reference: &str, desc: &str, category: &str, pins: &[(&str, &str, &str, bool)]) -> (Sexpr, Sexpr, Class) {
    let sym = parse(&vendor(name, reference, desc, pins)).unwrap();
    let class = detect(category, &sym, pins.len(), &rules()).unwrap();
    let new = restyle(&sym, class, &std_rules()).unwrap();
    (sym, new, class)
}

fn pin_at(s: &Sexpr, number: &str) -> String {
    let p = source_pins(s).unwrap().into_iter().find(|p| pin_number(p) == number).unwrap();
    to_string(p.child("at").unwrap()).trim().to_string()
}

/// What must not change: every property and flag, and the pins as pins.
fn assert_kept(old: &Sexpr, new: &Sexpr) {
    for k in ["Footprint", "Datasheet", "Description", "MPN", "Alternates", "Value"] {
        assert_eq!(property(old, k), property(new, k), "{k}");
    }
    for flag in ["exclude_from_sim", "in_bom", "on_board", "embedded_fonts"] {
        assert_eq!(old.child(flag), new.child(flag), "{flag}");
    }
    assert_eq!(pin_summary(old), pin_summary(new));
    // Hidden fields stay hidden, and alternates survive.
    assert!(to_string(new).contains("(alternate \"ALT"));
    assert!(to_string(new.children("property").find(|p| p.arg(0) == Some("MPN")).unwrap()).contains("hide yes"));
}

#[test]
fn every_class_is_redrawn_and_keeps_its_fields() {
    let two = |a: &'static str, b: &'static str| [("passive", "2", b, false), ("passive", "1", a, false)];
    let cases: Vec<(&str, &str, &str, &str, Vec<(&str, &str, &str, bool)>, Class, &str)> = vec![
        ("RC0603", "U", "Chip resistor", "Resistors", two("~", "~").to_vec(), Class::Resistor, "R"),
        ("CC0603", "U", "MLCC capacitor", "Capacitors", two("1", "2").to_vec(), Class::Capacitor, "C"),
        ("LQ0603", "U", "Power inductor", "Inductors", two("", "").to_vec(), Class::Inductor, "L"),
        ("1N4148", "U", "Switching diode", "Diodes", two("A", "K").to_vec(), Class::Diode, "D"),
        ("LED0603", "U", "Red LED", "Optoelectronics", two("K", "A").to_vec(), Class::Led, "D"),
        ("BC847", "U", "NPN transistor", "Transistors", vec![("input", "1", "B", false), ("passive", "3", "C", false), ("passive", "2", "E", false)], Class::Npn, "Q"),
        ("BC857", "U", "PNP transistor", "Transistors", vec![("input", "1", "BASE", false), ("passive", "2", "Emitter", false), ("passive", "3", "Collector", false)], Class::Pnp, "Q"),
    ];
    for (name, reference, desc, category, pins, class, prefix) in cases {
        let (old, new, got) = restyled(name, reference, desc, category, &pins);
        assert_eq!(got, class, "{name}");
        assert_eq!(property(&new, "Reference"), Some(prefix), "{name}");
        assert_kept(&old, &new);
        // The result is a valid file body that parses back to itself.
        assert_eq!(parse(&to_string(&new)).unwrap(), new);
        assert_eq!(new.children("symbol").count(), 2);
        // Restyling again changes nothing.
        assert_eq!(restyle(&new, class, &std_rules()).unwrap(), new, "{name} must be stable");
    }
}

#[test]
fn pins_go_where_their_role_says() {
    // Diode: the pin named K is the cathode, whatever its number.
    let (_, d, _) = restyled("D1", "D", "Diode", "Diodes", &[("passive", "1", "A", false), ("passive", "2", "K", false)]);
    assert_eq!(pin_at(&d, "2"), "(at -3.81 0 0)");
    assert_eq!(pin_at(&d, "1"), "(at 3.81 0 180)");
    // Resistor: lowest number on top.
    let (_, r, _) = restyled("R1", "R", "Resistor", "Resistors", &[("passive", "2", "2", false), ("passive", "1", "1", false)]);
    assert_eq!(pin_at(&r, "1"), "(at 0 3.81 270)");
    // BJT by name, not number.
    let (_, q, _) = restyled("Q1", "Q", "NPN", "Transistors", &[("passive", "1", "C", false), ("passive", "2", "B", false), ("passive", "3", "E", false)]);
    assert_eq!(pin_at(&q, "2"), "(at -5.08 0 0)");
    assert_eq!(pin_at(&q, "1"), "(at 2.54 5.08 270)");
    assert_eq!(pin_at(&q, "3"), "(at 2.54 -5.08 90)");
}

#[test]
fn ic_box_groups_pins_by_function() {
    let pins = [
        ("power_in", "8", "VCC", false),
        ("power_in", "4", "GND", false),
        ("input", "3", "IN+", false),
        ("input", "2", "IN-", false),
        ("output", "1", "OUT", false),
        ("power_in", "9", "VDDIO", true),
        ("bidirectional", "5", "PA1", false),
        ("bidirectional", "6", "PA0", false),
        ("bidirectional", "7", "PB0", false),
    ];
    let (old, new, class) = restyled("OPA", "U", "Op amp", "ICs", &pins);
    assert_eq!(class, Class::Ic);
    assert_kept(&old, &new);
    let rot = |n: &str| pin_at(&new, n).trim_end_matches(')').rsplit(' ').next().unwrap().parse::<i32>().unwrap();
    assert_eq!((rot("8"), rot("4")), (270, 90), "supply on top, ground below");
    assert_eq!((rot("3"), rot("1")), (0, 180), "inputs left, outputs right");
    let y = |n: &str| pin_at(&new, n).split(' ').nth(2).unwrap().parse::<f64>().unwrap();
    assert!(y("6") > y("5"), "PA0 above PA1 in the same group");
    // The hidden pin sits on its own point, clear of every visible pin end.
    let hidden = pin_at(&new, "9");
    assert!(source_pins(&new).unwrap().iter().filter(|p| pin_number(p) != "9").all(|p| to_string(p.child("at").unwrap()).trim() != hidden));
    for p in source_pins(&new).unwrap() {
        let (x, y) = (p.child("at").and_then(|a| a.arg(0)).unwrap().parse::<f64>().unwrap(), p.child("at").and_then(|a| a.arg(1)).unwrap().parse::<f64>().unwrap());
        assert!(((x / 2.54).round() * 2.54 - x).abs() < 1e-3 && ((y / 2.54).round() * 2.54 - y).abs() < 1e-3, "pin {} off the 100 mil grid", pin_number(&p));
    }
}

#[test]
fn refuses_rather_than_guess() {
    let sym = |pins: &[(&str, &str, &str, bool)]| parse(&vendor("X1", "U", "Part", pins)).unwrap();
    let diode = |a, b| sym(&[("passive", "1", a, false), ("passive", "2", b, false)]);
    for (label, s, class) in [
        ("diode pins named by number only", diode("1", "2"), Class::Diode),
        ("both pins named anode", diode("A", "ANODE"), Class::Diode),
        ("resistor with meaningful names", diode("IN", "OUT"), Class::Resistor),
        ("BJT without an emitter", sym(&[("input", "1", "B", false), ("passive", "2", "C", false), ("passive", "3", "X", false)]), Class::Npn),
        ("BJT with two collectors", sym(&[("input", "1", "B", false), ("passive", "2", "C", false), ("passive", "3", "C", false)]), Class::Npn),
        ("duplicate pin numbers", sym(&[("passive", "1", "~", false), ("passive", "1", "~", false)]), Class::Resistor),
        ("wrong pin count for the class", sym(&[("passive", "1", "~", false)]), Class::Resistor),
    ] {
        assert!(restyle(&s, class, &std_rules()).is_err(), "{label} must be refused");
    }
    // Multi-unit, derived and power symbols.
    let multi = vendor("X1", "U", "Part", &[("passive", "1", "~", false), ("passive", "2", "~", false)]).replace("X1_1_1", "X1_2_1");
    assert!(restyle(&parse(&multi).unwrap(), Class::Resistor, &std_rules()).is_err());
    let derived = vendor("X1", "U", "Part", &[("passive", "1", "~", false), ("passive", "2", "~", false)]).replace("(in_bom yes)", "(extends \"Y\")");
    assert!(restyle(&parse(&derived).unwrap(), Class::Resistor, &std_rules()).is_err());
    let power = vendor("X1", "U", "Part", &[("passive", "1", "~", false), ("passive", "2", "~", false)]).replace("(in_bom yes)", "(power)");
    assert!(restyle(&parse(&power).unwrap(), Class::Resistor, &std_rules()).is_err());
}

fn library(name: &str, category: &str, symbols: &[String]) -> (std::path::PathBuf, LibraryRoot) {
    let base = crate::test_dir(name);
    let root = LibraryRoot::global(base.join("lib"), base.join("config"));
    root.ensure_layout().unwrap();
    std::fs::write(lib_path(&root, category), format!("(kicad_symbol_lib (version 20251024) (generator \"t\") {})", symbols.join("\n"))).unwrap();
    (base, root)
}

#[test]
fn restyle_part_snapshots_and_can_be_rolled_back() {
    let good = vendor("RC0603", "U", "Chip resistor", &[("passive", "1", "1", false), ("passive", "2", "2", false)]);
    let bad = vendor("RDIO", "U", "Chip resistor", &[("passive", "1", "IN", false), ("passive", "2", "OUT", false)]);
    let (_b, root) = library("symbols_part", "Resistors", &[good, bad]);
    let path = lib_path(&root, "Resistors");
    let before = std::fs::read_to_string(&path).unwrap();

    // A dry run writes nothing.
    let dry = restyle_part(&root, "Resistors", "RC0603", &rules(), &std_rules(), false).unwrap();
    assert!(dry.changed && std::fs::read_to_string(&path).unwrap() == before && crate::snapshot::list(&root).is_empty());

    // A refusal writes nothing and leaves no history entry.
    let err = restyle_part(&root, "Resistors", "RDIO", &rules(), &std_rules(), true).unwrap_err().to_string();
    assert!(err.contains("not restyled"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert!(crate::snapshot::list(&root).is_empty());

    let done = restyle_part(&root, "Resistors", "RC0603", &rules(), &std_rules(), true).unwrap();
    assert_eq!(done.class, Class::Resistor);
    let after = std::fs::read_to_string(&path).unwrap();
    assert_ne!(after, before);
    let lib = parse(&after).unwrap();
    let other = lib.children("symbol").find(|s| s.arg(0) == Some("RDIO")).unwrap();
    assert!(to_string(other).contains("\"IN\""), "the other symbol is untouched");
    let snaps = crate::snapshot::list(&root);
    assert_eq!(snaps.len(), 1);

    // Restyling a house symbol again is a no-op: no write, no new snapshot.
    assert!(!restyle_part(&root, "Resistors", "RC0603", &rules(), &std_rules(), true).unwrap().changed);
    assert_eq!(crate::snapshot::list(&root).len(), 1);

    crate::snapshot::rollback(&root, &snaps[0].id).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn a_symbol_other_symbols_derive_from_is_refused() {
    let parent = vendor("P1", "U", "Chip resistor", &[("passive", "1", "1", false), ("passive", "2", "2", false)]);
    let (_b, root) = library("symbols_extends", "Resistors", &[parent, r#"(symbol "C1" (extends "P1") (property "Value" "C1"))"#.into()]);
    let e = restyle_part(&root, "Resistors", "P1", &rules(), &std_rules(), true).unwrap_err().to_string();
    assert!(e.contains("derived from"), "{e}");
}

#[test]
fn import_step_restyles_or_keeps_the_vendor_symbol() {
    let mut settings = Settings::default();
    settings.house_symbols_on_import = true;
    configure(&settings);
    let mut ok = parse(&vendor("RC0603", "U", "Chip resistor", &[("passive", "1", "1", false), ("passive", "2", "2", false)])).unwrap();
    restyle_imported("Resistors", &mut ok);
    assert_eq!(property(&ok, "Reference"), Some("R"));
    assert!(take_note().unwrap().contains("Resistor"));
    let mut bad = parse(&vendor("RDIO", "U", "Chip resistor", &[("passive", "1", "IN", false), ("passive", "2", "OUT", false)])).unwrap();
    let before = bad.clone();
    restyle_imported("Resistors", &mut bad);
    assert_eq!(bad, before);
    assert!(take_note().unwrap().contains("Kept the vendor symbol"));
    settings.house_symbols_on_import = false;
    configure(&settings);
    restyle_imported("Resistors", &mut ok);
    assert!(take_note().is_none());
}

/// Real symbols from the installed KiCad: restyled, then opened by `kicad-cli` itself.
#[test]
fn kicad_opens_the_generated_symbols() {
    let dir = "C:/Program Files/KiCad/10.0/share/kicad/symbols";
    let Ok(version) = crate::kicad_env::kicad_cli_version() else { return };
    let (Ok(device), Ok(bjt)) = (std::fs::read_to_string(format!("{dir}/Device.kicad_sym")), std::fs::read_to_string(format!("{dir}/Transistor_BJT.kicad_sym"))) else { return };
    let (device, bjt) = (parse(&device).unwrap(), parse(&bjt).unwrap());
    let get = |lib: &Sexpr, n: &str| lib.children("symbol").find(|s| s.arg(0) == Some(n)).unwrap().clone();
    let mut out = vec![];
    for (lib, n, class) in [
        (&device, "R", Class::Resistor),
        (&device, "C", Class::Capacitor),
        (&device, "L", Class::Inductor),
        (&device, "D", Class::Diode),
        (&device, "LED", Class::Led),
        (&bjt, "Q_NPN_BCE", Class::Npn),
        (&bjt, "Q_PNP_BCE", Class::Pnp),
        (&device, "Q_NPN_BRT", Class::Ic), // a box, not a transistor: it has built-in resistors
    ] {
        let sym = get(lib, n);
        let new = restyle(&sym, class, &std_rules()).unwrap_or_else(|e| panic!("{n}: {e:#}"));
        assert_eq!(pin_summary(&sym), pin_summary(&new), "{n}");
        out.push(new);
    }
    let base = crate::test_dir("symbols_kicad_cli");
    let file = base.join("house.kicad_sym");
    let header = "(kicad_symbol_lib (version 20251024) (generator \"t\")";
    std::fs::write(&file, format!("{header} {})", out.iter().map(to_string).collect::<String>())).unwrap();
    let up = base.join("up.kicad_sym");
    crate::kicad_env::upgrade_symbols(&file, &up).unwrap_or_else(|e| panic!("kicad-cli {version} rejected the symbols: {e:#}"));
    let back = parse(&std::fs::read_to_string(&up).unwrap()).unwrap();
    assert_eq!(back.children("symbol").count(), out.len());
    for a in &out {
        let b = back.children("symbol").find(|s| s.arg(0) == a.arg(0)).expect("symbol survives the upgrade");
        assert_eq!(pin_summary(a), pin_summary(b));
    }
}

/// Real vendor library from the third-party corpus: batteries have no house style.
#[test]
fn corpus_battery_symbols_are_refused() {
    let lib = parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/third_party/sparkfun_Battery.kicad_sym")).unwrap()).unwrap();
    let sym = lib.children("symbol").next().unwrap();
    let n = source_pins(sym).unwrap().len();
    assert!(detect("Power", sym, n, &rules()).is_err());
    assert!(detect("Misc", sym, n, &rules()).is_err());
}

#[test]
fn hidden_pad_pin_stays_on_its_visible_twin_and_extra_hidden_pins_stay_clear() {
    let mut text = vendor("REG", "U", "LDO", &[("power_in", "1", "VIN", false), ("power_in", "2", "GND", false), ("output", "3", "VOUT", false), ("passive", "4", "EP", true), ("power_in", "5", "VDD", true)]);
    // Stack the exposed pad (4) on GND (2) as vendors do.
    text = text.replace("(at -10 9 0) (length 1) (hide yes)", "(at -8 3 0) (length 1) (hide yes)");
    let sym = parse(&text).unwrap();
    assert_eq!(pin_at(&sym, "4"), pin_at(&sym, "2"), "fixture stacks the pad on GND");
    let new = restyle(&sym, Class::Ic, &std_rules()).unwrap();
    assert_eq!(pin_at(&new, "4"), pin_at(&new, "2"), "pad stays on the GND net");
    assert_ne!(pin_at(&new, "5"), pin_at(&new, "2"), "an unrelated hidden pin does not join it");
    let (hx, hy) = {
        let p = pin_at(&new, "5");
        let v: Vec<f64> = p.trim_matches(|c| c == '(' || c == ')').split(' ').skip(1).take(2).map(|n| n.parse().unwrap()).collect();
        (v[0], v[1])
    };
    // No visible bottom pin shares its column with the hidden one.
    for n in ["1", "2", "3"] {
        let p = pin_at(&new, n);
        let x: f64 = p.trim_matches(|c| c == '(' || c == ')').split(' ').nth(1).unwrap().parse().unwrap();
        assert!((x - hx).abs() > 1.0 || n == "3", "pin {n} in the hidden pin's column at y {hy}");
    }
}

#[test]
fn exclusions_beat_led_and_match_plurals() {
    let r = rules();
    let s = |d: &str| parse(&format!(r#"(symbol "X" (property "Description" "{d}"))"#)).unwrap();
    assert!(detect("Diodes", &s("TVS diode for LED lighting"), 2, &r).is_err());
    assert!(detect("Capacitors", &s("Supercapacitors 5.5V 1F"), 2, &r).is_err());
    assert!(detect("Resistors", &s("Varistors 14V"), 2, &r).is_err());
    assert!(detect("Inductors", &s("EMI suppression beads"), 2, &r).is_err());
}

/// A diode the user drew: vertical, cathode up, filled triangle, fields to the right.
const HOUSE_D: &str = r#"(symbol "D_HOUSE" (pin_numbers (hide yes)) (pin_names (offset 0) (hide yes)) (exclude_from_sim no) (in_bom yes) (on_board yes)
    (property "Reference" "D" (at 2.54 1.27 0) (effects (font (size 1.27 1.27))))
    (property "Value" "D_HOUSE" (at 2.54 -1.27 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Vendor:FP_HOUSE" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes)))
    (symbol "D_HOUSE_0_1"
        (polyline (pts (xy -1.27 1.27) (xy 1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
        (polyline (pts (xy -1.27 -1.27) (xy 1.27 -1.27) (xy 0 1.27) (xy -1.27 -1.27)) (stroke (width 0.254) (type default)) (fill (type outline))))
    (symbol "D_HOUSE_1_1"
        (pin passive line (at 0 3.81 270) (length 2.54) (name "K" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
        (pin passive line (at 0 -3.81 90) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))))"#;

#[test]
fn a_house_template_drawn_by_the_user_restyles_vendor_parts() {
    let tpl = Template::from_symbol(&parse(HOUSE_D).unwrap(), Class::Diode).unwrap();
    assert_eq!(tpl.slots.to_vec(), [Slot(0.0, 3.81, 270.0, 2.54), Slot(0.0, -3.81, 90.0, 2.54)], "cathode slot first, as the roles say");
    assert_eq!(tpl.fields, Some([(2.54, 1.27, 0.0), (2.54, -1.27, 0.0)]));
    assert!(tpl.hide_numbers && tpl.body.contains("outline"));
    // Wrong pin count, roles that can't be told, and IC boxes are refused.
    assert!(Template::from_symbol(&parse(HOUSE_D).unwrap(), Class::Npn).unwrap_err().to_string().contains("doesn't fit"));
    assert!(Template::from_symbol(&parse(&HOUSE_D.replace("(name \"K\"", "(name \"X\"")).unwrap(), Class::Diode).is_err());
    assert!(Template::from_symbol(&parse(HOUSE_D).unwrap(), Class::Ic).is_err());

    let mut profile = Profile::default();
    profile.templates.insert("Diode".into(), HOUSE_D.into());
    // The vendor numbers the anode 1 and the cathode 2, the reverse of the house symbol.
    let sym = parse(&vendor("1N4148", "D", "Switching diode", &[("passive", "1", "A", false), ("passive", "2", "K", false)])).unwrap();
    let new = restyle(&sym, Class::Diode, &profile).unwrap();
    assert_kept(&sym, &new);
    assert_eq!(pin_summary(&sym), pin_summary(&new));
    assert_eq!(pin_at(&new, "2"), "(at 0 3.81 270)", "the cathode goes where the house cathode is");
    assert_eq!(pin_at(&new, "1"), "(at 0 -3.81 90)");
    let field_at = |k: &str| to_string(new.children("property").find(|p| p.arg(0) == Some(k)).unwrap().child("at").unwrap()).trim().to_string();
    assert_eq!((field_at("Reference"), field_at("Value")), ("(at 2.54 1.27 0)".into(), "(at 2.54 -1.27 0)".into()));
    assert!(to_string(&new).contains("(type outline)"), "the house body, not the built-in one");
    assert_eq!(restyle(&new, Class::Diode, &profile).unwrap(), new, "stable");
    // Other classes keep the built-in template; a broken stored template is refused, not ignored.
    assert!(restyle(&sym, Class::Led, &profile).is_ok());
    profile.templates.insert("Diode".into(), "(symbol".into());
    assert!(restyle(&sym, Class::Diode, &profile).unwrap_err().to_string().contains("house template"));
}

#[test]
fn set_house_template_takes_the_symbol_and_footprint_style_used_by_restyle_and_standardise() {
    let house = HOUSE_D.replace("D_HOUSE", "R_HOUSE").replace("\"K\"", "\"~\"").replace("\"A\"", "\"~\"").replace("\"Reference\" \"D\"", "\"Reference\" \"R\"");
    let vendor_r = vendor("RC0603", "U", "Chip resistor", &[("passive", "1", "1", false), ("passive", "2", "2", false)]);
    let (_b, root) = library("symbols_house", "Resistors", &[vendor_r, house]);
    let pretty = root.dir.join("footprints/Resistors.pretty");
    std::fs::create_dir_all(&pretty).unwrap();
    // House footprint: text on F.Fab, 1 mm outside the courtyard, 0.15 mm silkscreen.
    std::fs::write(pretty.join("FP_HOUSE.kicad_mod"), r#"(footprint "FP_HOUSE" (layer "F.Cu")
        (property "Reference" "REF**" (at 0 -2 0) (layer "F.Fab") (effects (font (size 0.8 0.8) (thickness 0.12))))
        (property "Value" "FP_HOUSE" (at 0 2 0) (layer "F.Fab") (effects (font (size 0.8 0.8) (thickness 0.12))))
        (attr smd)
        (fp_line (start -0.5 -0.8) (end 0.5 -0.8) (stroke (width 0.15) (type solid)) (layer "F.SilkS"))
        (fp_rect (start -1.5 -1) (end 1.5 1) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd rect (at -0.8 0) (size 0.6 0.8) (layers "F.Cu"))
        (pad "2" smd rect (at 0.8 0) (size 0.6 0.8) (layers "F.Cu")))"#).unwrap();
    std::fs::write(pretty.join("FP_RC0603.kicad_mod"), r#"(footprint "FP_RC0603" (layer "F.Cu")
        (property "Reference" "REF**" (at 0 0 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15))))
        (property "Value" "FP_RC0603" (at 0 0 0) (layer "F.Fab") (effects (font (size 1 1) (thickness 0.15))))
        (attr smd)
        (fp_line (start -1 -2) (end 1 -2) (stroke (width 0.2) (type solid)) (layer "F.SilkS"))
        (fp_rect (start -3 -3) (end 3 3) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu"))
        (pad "2" smd rect (at 1 0) (size 1 1) (layers "F.Cu")))"#).unwrap();

    let mut profile = Profile::default();
    assert_eq!(set_house_template(&mut profile, &root, "Resistors", "R_HOUSE", &rules()).unwrap(), (Class::Resistor, true));
    assert!(profile.templates.contains_key("Resistor") && profile.footprint_styles.contains_key("Resistor"));

    restyle_part(&root, "Resistors", "RC0603", &rules(), &profile, true).unwrap();
    let lib = parse(&std::fs::read_to_string(lib_path(&root, "Resistors")).unwrap()).unwrap();
    let r = lib.children("symbol").find(|s| s.arg(0) == Some("RC0603")).unwrap();
    assert_eq!((pin_at(r, "1"), pin_at(r, "2")), ("(at 0 3.81 270)".into(), "(at 0 -3.81 90)".into()));

    let plan = standardise::plan(&root, &profile, &rules());
    let text = plan.to_string();
    assert!(text.contains("1 F.SilkS line width(s) set to 0.15 mm") && text.contains("Reference and Value placed and styled like the house footprint"), "{text}");
    standardise::apply(&plan).unwrap();
    let fp = parse(&std::fs::read_to_string(pretty.join("FP_RC0603.kicad_mod")).unwrap()).unwrap();
    let flat = |x: &Sexpr| to_string(x).split_whitespace().collect::<Vec<_>>().join(" ").replace("( ", "(").replace(" )", ")");
    let field = |k: &str| flat(fp.children("property").find(|p| p.arg(0) == Some(k)).unwrap());
    for (k, at) in [("Reference", "(at 0 -4 0)"), ("Value", "(at 0 4 0)")] {
        let f = field(k);
        assert!(f.contains(at) && f.contains("(layer \"F.Fab\")") && f.contains("(size 0.8 0.8)"), "1 mm outside its own courtyard, house font and layer: {f}");
    }
    assert!(flat(&fp).contains("(start -3 -3) (end 3 3)") && flat(&fp).contains("(at -1 0)"), "geometry untouched");
    assert!(standardise::plan(&root, &profile, &rules()).files.is_empty(), "a second run has nothing to do");
}

fn pin_row(number: &str) -> edit::PinRow {
    edit::PinRow { number: number.into(), name: "X".into(), etype: "output".into(), x: 1.3, y: -2.5, rot: 180.0, length: 3.0 }
}

#[test]
fn symbol_editor_ops_round_trip_through_the_file_and_keep_everything_else() {
    use edit::*;
    let sym = vendor("U1", "U", "an IC", &[("input", "1", "IN", false), ("output", "2", "OUT", true)]);
    let (_base, root) = library("edit_ops", "ICs", &[sym]);
    let was = load(&root, "ICs", "U1").unwrap();
    let mut s = was.clone();
    add_pin(&mut s, pin_row("3")).unwrap();
    set_pin(&mut s, "1", PinRow { name: "VIN".into(), x: 0.7, ..pin_row("1") }).unwrap();
    remove_pin(&mut s, "2").unwrap();
    set_body(&mut s, [-4.0, 4.0, 4.0, -4.0]).unwrap();
    set_text_at(&mut s, "Reference", [3.0, 5.0, 0.0]).unwrap();
    assert!(add_pin(&mut s, pin_row("3")).is_err() && set_pin(&mut s, "1", pin_row("3")).is_err() && remove_pin(&mut s, "9").is_err());
    save(&root, "ICs", "U1", &was, s.clone()).unwrap();
    // A save from a stale copy is refused.
    assert!(save(&root, "ICs", "U1", &was, s).is_err());

    let back = load(&root, "ICs", "U1").unwrap();
    let p = pins(&back);
    assert_eq!(p.iter().map(|p| p.number.as_str()).collect::<Vec<_>>(), ["1", "3"]);
    // Snapped to 1.27 mm; type, name and length changed.
    assert_eq!(p[0], PinRow { number: "1".into(), name: "VIN".into(), etype: "output".into(), x: 1.27, y: -2.54, rot: 180.0, length: 2.54 });
    assert_eq!(body(&back), Some([-3.81, 3.81, 3.81, -3.81]));
    assert_eq!(text_at(&back, "Reference"), Some([2.54, 5.08, 0.0]));
    assert_eq!(property(&back, "Datasheet"), property(&was, "Datasheet"));
    assert_eq!(property(&back, "Alternates"), property(&was, "Alternates"));
    assert!(back.children("symbol").flat_map(|u| u.children("pin")).all(|p| p.child("alternate").is_some() || pin_number(p) == "3"));
}

#[test]
fn classes_in_lists_the_parts_that_have_a_class() {
    let r = vendor("RC0603", "U", "Chip resistor", &[("passive", "1", "1", false), ("passive", "2", "2", false)]);
    let (_b, root) = library("symbols_classes_in", "Resistors", &[r]);
    let found = classes_in(&root, &rules());
    assert_eq!(found.get(&("Resistors".to_string(), "RC0603".to_string())), Some(&Class::Resistor));
    assert_eq!(found.len(), 1);
}
