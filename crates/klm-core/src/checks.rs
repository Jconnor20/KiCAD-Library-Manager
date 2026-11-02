//! Does a part hold together? Pins against pads, required fields, courtyard, pin-1 marker and 3D model.
//!
//! `Library::list_parts` says what a part has; this says whether what it has is right. Every check
//! only reads files, and skips what it can't see (a footprint or model in another library).

use crate::library::{LibraryRoot, property, sanitize};
use crate::sexpr::{self, Sexpr};
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
}

const REQUIRED_FIELDS: &[&str] = &["Reference", "Value", "Footprint", "MPN", "Manufacturer"];
const MARKER_LAYERS: &[&str] = &["F.SilkS", "B.SilkS", "F.Fab", "B.Fab"];

fn error(out: &mut Vec<Issue>, message: String) {
    out.push(Issue { severity: Severity::Error, message });
}

fn warning(out: &mut Vec<Issue>, message: String) {
    out.push(Issue { severity: Severity::Warning, message });
}

/// Issues for every part in the library, keyed by (category, symbol name). Each symbol library is
/// read once, so this is the cheap way to check everything.
pub fn check_library(root: &LibraryRoot) -> HashMap<(String, String), Vec<Issue>> {
    let mut all = HashMap::new();
    for cat in root.categories() {
        let Some(lib) = read_symbol_lib(root, &cat) else { continue };
        for sym in lib.children("symbol") {
            all.insert((cat.clone(), sym.arg(0).unwrap_or_default().to_string()), check_symbol(root, &cat, &lib, sym));
        }
    }
    all
}

pub fn check_part(root: &LibraryRoot, category: &str, symbol_name: &str) -> Vec<Issue> {
    let Some(lib) = read_symbol_lib(root, category) else {
        return vec![Issue { severity: Severity::Error, message: format!("The {category} symbol library can't be read") }];
    };
    match lib.children("symbol").find(|s| s.arg(0) == Some(symbol_name)) {
        Some(sym) => check_symbol(root, category, &lib, sym),
        None => vec![Issue { severity: Severity::Error, message: format!("{symbol_name} is not in the {category} symbol library") }],
    }
}

fn read_symbol_lib(root: &LibraryRoot, category: &str) -> Option<Sexpr> {
    let path = root.dir.join("symbols").join(format!("{}.kicad_sym", sanitize(category)));
    sexpr::parse(&std::fs::read_to_string(path).ok()?).ok()
}

fn check_symbol(root: &LibraryRoot, cat: &str, lib: &Sexpr, sym: &Sexpr) -> Vec<Issue> {
    let mut out = vec![];
    for key in REQUIRED_FIELDS {
        if property(sym, key).is_none_or(|v| v.trim().is_empty()) {
            error(&mut out, format!("Required field {key} is missing or empty"));
        }
    }
    let pins = symbol_pins(lib, sym);
    check_duplicate_pins(&pins, &mut out);

    // Only a footprint in this library's own .pretty folder can be read.
    let Some(fp_name) = property(sym, "Footprint").and_then(|f| f.strip_prefix(&format!("{}:", root.nickname(cat)))) else { return out };
    let fp_path = root.dir.join("footprints").join(format!("{cat}.pretty")).join(format!("{fp_name}.kicad_mod"));
    let Some(fp) = std::fs::read_to_string(fp_path).ok().and_then(|t| sexpr::parse(&t).ok()) else {
        error(&mut out, format!("Footprint {fp_name} can't be read from the library"));
        return out;
    };

    let pads = numbered_pads(&fp);
    let pin_numbers: BTreeSet<&str> = pins.iter().map(|p| p.2.as_str()).collect();
    let pad_numbers: BTreeSet<&str> = pads.iter().map(|p| p.number.as_str()).collect();
    let no_pad: Vec<_> = pin_numbers.difference(&pad_numbers).copied().collect();
    if !no_pad.is_empty() {
        error(&mut out, format!("Symbol pin(s) {} have no pad in the footprint", list(no_pad)));
    }
    let no_pin: Vec<_> = pad_numbers.difference(&pin_numbers).copied().collect();
    if !no_pin.is_empty() {
        warning(&mut out, format!("Footprint pad(s) {} have no pin in the symbol", list(no_pin)));
    }

    if !on_layer(&fp, &["F.CrtYd", "B.CrtYd"]) {
        warning(&mut out, "The footprint has no courtyard (F.CrtYd or B.CrtYd)".into());
    }
    if !has_pin1_marker(&fp, &pads) {
        warning(&mut out, "The footprint doesn't mark pin 1 (no silkscreen or fab marker near it, and its pad looks like the others)".into());
    }
    if let Some(model) = fp.child("model").and_then(|m| m.arg(0)) {
        check_model(root, model, &mut out);
    }
    out
}

/// (unit, body style, number) of every symbol pin. A derived symbol (`extends`) uses its parent's pins.
fn symbol_pins(lib: &Sexpr, sym: &Sexpr) -> Vec<(u32, u32, String)> {
    let mut sym = sym;
    // The depth limit stops a cycle of symbols extending each other.
    for _ in 0..8 {
        let Some(parent) = sym.child("extends").and_then(|e| e.arg(0)) else { break };
        match lib.children("symbol").find(|s| s.arg(0) == Some(parent)) {
            Some(p) => sym = p,
            None => break,
        }
    }
    let mut pins = vec![];
    for unit in sym.children("symbol") {
        // Unit sub-symbols are named NAME_<unit>_<style>; unit 0 / style 0 apply to all.
        let mut parts = unit.arg(0).unwrap_or_default().rsplitn(3, '_');
        let (Some(style), Some(u)) = (parts.next().and_then(|s| s.parse().ok()), parts.next().and_then(|s| s.parse().ok())) else { continue };
        for pin in unit.children("pin") {
            if let Some(n) = pin.child("number").and_then(|n| n.arg(0)).filter(|n| !n.is_empty()) {
                pins.push((u, style, n.to_string()));
            }
        }
    }
    pins
}

/// A number may repeat across units (shared power pins) but not within what's drawn together:
/// the pins of one unit plus the common unit 0, in one body style.
fn check_duplicate_pins(pins: &[(u32, u32, String)], out: &mut Vec<Issue>) {
    let units: BTreeSet<u32> = pins.iter().map(|p| p.0).filter(|&u| u > 0).collect();
    let units = if units.is_empty() { BTreeSet::from([0]) } else { units };
    let styles = if pins.iter().any(|p| p.1 == 2) { vec![1, 2] } else { vec![1] };
    for &u in &units {
        for &s in &styles {
            let mut seen = BTreeSet::new();
            let dups: Vec<&str> = pins.iter().filter(|p| (p.0 == 0 || p.0 == u) && (p.1 == 0 || p.1 == s)).filter(|p| !seen.insert(&p.2)).map(|p| p.2.as_str()).collect();
            let msg = format!("Pin number(s) {} used more than once{}", list(dups.iter().copied().collect::<BTreeSet<_>>()), if units.len() > 1 { format!(" in unit {u}") } else { String::new() });
            if !dups.is_empty() && !out.iter().any(|i| i.message == msg) {
                error(out, msg);
            }
        }
    }
}

/// Up to 8 numbers in natural order (2 before 10), then a count of the rest.
fn list<'a>(numbers: impl IntoIterator<Item = &'a str>) -> String {
    let mut v: Vec<_> = numbers.into_iter().collect();
    v.sort_by_key(|n| (n.len(), *n));
    let more = v.len().saturating_sub(8);
    let shown = v.iter().take(8).copied().collect::<Vec<_>>().join(", ");
    if more > 0 { format!("{shown} and {more} more") } else { shown }
}

struct Pad {
    number: String,
    shape: String,
    at: (f64, f64),
    /// Larger side of the pad.
    size: f64,
}

fn xy(n: &Sexpr) -> Option<(f64, f64)> {
    Some((n.arg(0)?.parse().ok()?, n.arg(1)?.parse().ok()?))
}

/// Pads with a number. Unnumbered pads and non-plated holes are mechanical and have no pin.
fn numbered_pads(fp: &Sexpr) -> Vec<Pad> {
    fp.children("pad")
        .filter(|p| p.arg(1) != Some("np_thru_hole"))
        .filter_map(|p| {
            let number = p.arg(0).filter(|n| !n.is_empty())?.to_string();
            let (w, h) = p.child("size").and_then(xy).unwrap_or_default();
            Some(Pad { number, shape: p.arg(2).unwrap_or_default().to_string(), at: p.child("at").and_then(xy).unwrap_or_default(), size: w.max(h) })
        })
        .collect()
}

/// Whether any node under `x` sits on one of `layers`.
fn on_layer(x: &Sexpr, layers: &[&str]) -> bool {
    x.child("layer").and_then(|l| l.arg(0)).is_some_and(|l| layers.contains(&l)) || x.items().iter().any(|c| on_layer(c, layers))
}

fn points(x: &Sexpr, out: &mut Vec<(f64, f64)>) {
    if matches!(x.head(), Some("xy" | "start" | "end" | "center" | "mid" | "at")) {
        out.extend(xy(x));
    }
    x.items().iter().for_each(|c| points(c, out));
}

/// Pin 1 counts as marked when its pad has a shape of its own (say a square pad among round ones), or
/// when a silkscreen/fab circle, polygon, arc, diagonal line or text lies within 2 mm of its edge. A
/// plain outline of straight lines doesn't count, since every footprint has one. Footprints with fewer
/// than three pads (resistors, capacitors) have no pin 1 to find, and are not checked. It is a
/// heuristic: a marker drawn further out, or only as an elongated outline line, is reported as missing.
fn has_pin1_marker(fp: &Sexpr, pads: &[Pad]) -> bool {
    let numbers: BTreeSet<&str> = pads.iter().map(|p| p.number.as_str()).collect();
    let Some(one) = pads.iter().find(|p| p.number == "1" || p.number == "A1") else { return true };
    if numbers.len() < 3 {
        return true;
    }
    let mut shapes: HashMap<&str, usize> = HashMap::new();
    for p in pads.iter().filter(|p| p.number != one.number) {
        *shapes.entry(&p.shape).or_default() += 1;
    }
    if shapes.iter().max_by_key(|s| s.1).is_some_and(|s| *s.0 != one.shape) {
        return true;
    }
    let reach = one.size / 2.0 + 2.0;
    fp.items().iter().any(|g| {
        let is_marker = match g.head() {
            Some("fp_circle" | "fp_poly" | "fp_arc" | "fp_curve") => true,
            // Reference/value text and ${REFERENCE} placeholders are not markers.
            Some("fp_text" | "text") => !matches!(g.arg(0), Some("reference" | "value")) && !g.arg(1).is_some_and(|t| t.starts_with("${")),
            Some("fp_line") => match (g.child("start").and_then(xy), g.child("end").and_then(xy)) {
                (Some(a), Some(b)) => (a.0 - b.0).abs() > 1e-6 && (a.1 - b.1).abs() > 1e-6,
                _ => false,
            },
            _ => false,
        };
        if !is_marker || !on_layer(g, MARKER_LAYERS) {
            return false;
        }
        let mut pts = vec![];
        points(g, &mut pts);
        pts.iter().any(|p| (p.0 - one.at.0).hypot(p.1 - one.at.1) <= reach)
    })
}

/// The model file exists and starts like a STEP or WRL file (and a STEP ends like one, which catches truncated downloads).
fn check_model(root: &LibraryRoot, uri: &str, out: &mut Vec<Issue>) {
    let path = uri.replace("${KIPRJMOD}", &root.table_dir.to_string_lossy());
    if path.starts_with("http") || path.contains("${") {
        return; // can't be resolved here
    }
    let path = Path::new(&path);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let Ok(mut file) = std::fs::File::open(path) else {
        return error(out, format!("3D model {name} is missing"));
    };
    let mut head = vec![];
    let _ = (&mut file).take(64).read_to_end(&mut head);
    let text = String::from_utf8_lossy(&head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
    match ext.as_str() {
        "step" | "stp" => {
            let mut tail = vec![];
            let _ = file.seek(SeekFrom::End(-512)).or_else(|_| file.seek(SeekFrom::Start(0))); // a file under 512 bytes can't seek back that far
            let _ = file.read_to_end(&mut tail);
            if !text.starts_with("ISO-10303-21") {
                error(out, format!("3D model {name} isn't a valid STEP file (header missing)"));
            } else if !String::from_utf8_lossy(&tail).contains("END-ISO-10303-21") {
                error(out, format!("3D model {name} looks cut off (STEP end marker missing)"));
            }
        }
        "wrl" if !text.starts_with("#VRML") => error(out, format!("3D model {name} isn't a valid WRL file (header missing)")),
        "stpz" | "wrz" if !head.starts_with(&[0x1f, 0x8b]) => error(out, format!("3D model {name} isn't a valid compressed file")),
        "wrl" | "stpz" | "wrz" => {}
        _ => warning(out, format!("3D model {name} is not a STEP or WRL file")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYM: &str = r#"(kicad_symbol_lib (version 20251024) (generator "t")
        (symbol "P1"
            (property "Reference" "U" (at 0 0 0))
            (property "Value" "P1" (at 0 0 0))
            (property "Footprint" "PRJ_Test:FP" (at 0 0 0))
            (property "MPN" "P1" (at 0 0 0))
            (property "Manufacturer" "Acme" (at 0 0 0))
            (symbol "P1_0_1" (rectangle (start 0 0) (end 1 1)))
            (symbol "P1_1_1"
                (pin passive line (at 0 0 0) (length 2) (name "A") (number "1"))
                (pin passive line (at 0 2 0) (length 2) (name "B") (number "2"))
                (pin passive line (at 0 4 0) (length 2) (name "C") (number "3"))))
        (symbol "P2" (extends "P1")
            (property "Reference" "U" (at 0 0 0))
            (property "Value" "P2" (at 0 0 0))
            (property "Footprint" "PRJ_Test:FP" (at 0 0 0))
            (property "MPN" "P2" (at 0 0 0))
            (property "Manufacturer" "Acme" (at 0 0 0))))"#;

    const FP: &str = r#"(footprint "FP" (layer "F.Cu")
        (model "${KIPRJMOD}/lib/3dmodels/Test.3dshapes/p.step")
        (fp_line (start -1 -1) (end 1 -1) (layer "F.CrtYd"))
        (fp_line (start -1 -1) (end 1 -1) (layer "F.SilkS"))
        (pad "1" smd rect (at -1 0) (size 1 0.5) (layers "F.Cu"))
        (pad "2" smd roundrect (at 0 0) (size 1 0.5) (layers "F.Cu"))
        (pad "3" smd roundrect (at 1 0) (size 1 0.5) (layers "F.Cu"))
        (pad "" np_thru_hole circle (at 0 2) (size 1 1) (layers "*.Cu")))"#;

    const STEP: &[u8] = b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";

    /// A project library with the given symbol/footprint text and model file; returns the root.
    fn lib(name: &str, sym: &str, fp: &str, model: &[u8]) -> LibraryRoot {
        let dir = crate::test_dir(name);
        let root = LibraryRoot::project(&dir);
        for (sub, file, text) in [("symbols", "Test.kicad_sym", sym.as_bytes()), ("footprints/Test.pretty", "FP.kicad_mod", fp.as_bytes()), ("3dmodels/Test.3dshapes", "p.step", model)] {
            std::fs::create_dir_all(root.dir.join(sub)).unwrap();
            std::fs::write(root.dir.join(sub).join(file), text).unwrap();
        }
        root
    }

    fn messages(root: &LibraryRoot) -> Vec<String> {
        check_part(root, "Test", "P1").into_iter().map(|i| format!("{:?}: {}", i.severity, i.message)).collect()
    }

    /// Exactly one issue, and it mentions `needle`.
    fn assert_only(root: &LibraryRoot, needle: &str) {
        let m = messages(root);
        assert!(m.len() == 1 && m[0].contains(needle), "{m:?} should be only {needle:?}");
    }

    #[test]
    fn healthy_part_has_no_issues() {
        let root = lib("checks_healthy", SYM, FP, STEP);
        assert_eq!(messages(&root), Vec::<String>::new());
        assert!(check_part(&root, "Test", "P2").is_empty(), "a derived symbol uses its parent's pins");
        let all = check_library(&root);
        assert_eq!(all.len(), 2);
        assert!(all.values().all(Vec::is_empty));
    }

    #[test]
    fn pin_without_pad_is_an_error_and_pad_without_pin_a_warning() {
        let more_pins = SYM.replace(r#"(number "3"))))"#, r#"(number "3")) (pin passive line (at 0 6 0) (length 2) (name "D") (number "10"))))"#);
        let m = messages(&lib("checks_pin_no_pad", &more_pins, FP, STEP));
        assert!(m.len() == 1 && m[0].starts_with("Error") && m[0].contains("pin(s) 10"), "{m:?}");

        let fewer_pins = SYM.replace(r#"(pin passive line (at 0 4 0) (length 2) (name "C") (number "3"))"#, "");
        let m = messages(&lib("checks_pad_no_pin", &fewer_pins, FP, STEP));
        assert!(m.len() == 1 && m[0].starts_with("Warning") && m[0].contains("pad(s) 3"), "{m:?}");
    }

    #[test]
    fn unnumbered_pads_are_ignored() {
        // The healthy footprint already has an unnumbered mechanical pad.
        assert!(FP.contains(r#"(pad "" np_thru_hole"#));
        assert!(messages(&lib("checks_mech_pad", SYM, FP, STEP)).is_empty());
    }

    #[test]
    fn duplicate_pin_numbers_in_a_unit() {
        let dup = SYM.replace(r#"(number "3")"#, r#"(number "2")"#);
        let m = messages(&lib("checks_dup_pins", &dup, &FP.replace(r#"(pad "3""#, r#"(pad "2""#), STEP));
        assert!(m.iter().any(|i| i.starts_with("Error") && i.contains("2 used more than once")), "{m:?}");
    }

    #[test]
    fn a_number_shared_by_several_units_is_fine() {
        let units = SYM.replace(
            r#"(symbol "P1_1_1""#,
            r#"(symbol "P1_2_1" (pin passive line (at 0 0 0) (length 2) (name "A") (number "1"))) (symbol "P1_1_1""#,
        );
        assert!(messages(&lib("checks_units", &units, FP, STEP)).is_empty());
    }

    #[test]
    fn required_fields() {
        let m = messages(&lib("checks_fields", &SYM.replace(r#"(property "MPN" "P1""#, r#"(property "MPN" "  ""#).replace(r#"(property "Manufacturer" "Acme" (at 0 0 0))"#, ""), FP, STEP));
        assert!(m.len() == 2 && m[0].contains("MPN") && m[1].contains("Manufacturer"), "{m:?}");
    }

    #[test]
    fn courtyard() {
        assert_only(&lib("checks_courtyard", SYM, &FP.replace("F.CrtYd", "F.Fab"), STEP), "courtyard");
        assert!(messages(&lib("checks_courtyard_back", SYM, &FP.replace("F.CrtYd", "B.CrtYd"), STEP)).is_empty());
    }

    #[test]
    fn pin_1_marker() {
        let same_shape = FP.replace(r#"(pad "1" smd rect"#, r#"(pad "1" smd roundrect"#);
        assert_only(&lib("checks_pin1", SYM, &same_shape, STEP), "pin 1");
        let with_triangle = same_shape.replace(
            r#"(pad "1""#,
            r#"(fp_poly (pts (xy -1.5 -1) (xy -1.8 -1.2) (xy -1.8 -0.8)) (layer "F.SilkS")) (pad "1""#,
        );
        assert!(messages(&lib("checks_pin1_marked", SYM, &with_triangle, STEP)).is_empty());
        let far_away = with_triangle.replace("-1.5 -1", "-9 -9").replace("-1.8 -1.2", "-9 -9.2").replace("-1.8 -0.8", "-9.2 -9");
        assert_only(&lib("checks_pin1_far", SYM, &far_away, STEP), "pin 1");
    }

    #[test]
    fn model_file() {
        assert_only(&lib("checks_model_header", SYM, FP, b"solid not a step file"), "STEP file (header missing)");
        assert_only(&lib("checks_model_cut", SYM, FP, b"ISO-10303-21;\nHEADER;\nDATA;\n"), "cut off");
        assert_only(&lib("checks_model_empty", SYM, FP, b""), "header missing");
        let root = lib("checks_model_gone", SYM, FP, STEP);
        std::fs::remove_file(root.dir.join("3dmodels/Test.3dshapes/p.step")).unwrap();
        assert_only(&root, "p.step is missing");
        let wrl = FP.replace("p.step", "p.wrl");
        let root = lib("checks_model_wrl", SYM, &wrl, STEP);
        std::fs::write(root.dir.join("3dmodels/Test.3dshapes/p.wrl"), "#VRML V2.0 utf8\n").unwrap();
        assert!(messages(&root).is_empty());
        std::fs::write(root.dir.join("3dmodels/Test.3dshapes/p.wrl"), "<html>").unwrap();
        assert_only(&root, "WRL");
    }

    #[test]
    fn missing_part_or_footprint() {
        let root = lib("checks_missing", SYM, FP, STEP);
        assert!(check_part(&root, "Test", "Nope")[0].message.contains("not in"));
        std::fs::remove_file(root.dir.join("footprints/Test.pretty/FP.kicad_mod")).unwrap();
        assert_only(&root, "Footprint FP can't be read");
    }

    /// KiCad's own footprints are well-formed, so none of them may trip the courtyard or pin-1 heuristics.
    #[test]
    fn installed_kicad_footprints_pass_the_heuristics() {
        let base = Path::new("C:/Program Files/KiCad/10.0/share/kicad/footprints");
        for f in ["Package_SO.pretty/SOIC-8_3.9x4.9mm_P1.27mm", "Connector_PinHeader_2.54mm.pretty/PinHeader_1x03_P2.54mm_Vertical", "Package_QFP.pretty/LQFP-48_7x7mm_P0.5mm"] {
            let Ok(text) = std::fs::read_to_string(base.join(format!("{f}.kicad_mod"))) else { return };
            let fp = sexpr::parse(&text).unwrap();
            assert!(on_layer(&fp, &["F.CrtYd"]), "{f}: courtyard");
            assert!(has_pin1_marker(&fp, &numbered_pads(&fp)), "{f}: pin 1");
        }
    }
}
