//! Footprint rules. Footprint coordinates have y pointing down.

use super::{Rules, bbox, child_mut, differs, fmt, move_to, num, set_num};
use crate::checks::{has_pin1_marker, numbered_pads, on_layer, points};
use crate::sexpr::Sexpr;

const GRAPHICS: &[&str] = &["fp_line", "fp_rect", "fp_circle", "fp_arc", "fp_poly", "fp_curve"];

/// Applies the footprint rules and says what changed.
pub(super) fn fix(fp: &mut Sexpr, r: &Rules) -> Vec<String> {
    let mut out = vec![];
    for (width, layer, what) in [(r.silk_width, ".SilkS", "silkscreen"), (r.fab_width, ".Fab", "fab"), (r.courtyard_width, ".CrtYd", "courtyard")] {
        out.extend(width.and_then(|w| line_width(fp, layer, w, what)));
    }
    // Origin first: placement and the pin-1 marker are worked out from the moved footprint.
    if r.footprint_origin {
        out.extend(origin(fp));
    }
    if r.footprint_ref_value {
        out.extend(ref_value(fp));
    }
    if r.pin1_marker {
        out.extend(pin1_marker(fp, r.silk_width.unwrap_or(0.12)));
    }
    out
}

fn line_width(fp: &mut Sexpr, layer: &str, width: f64, what: &str) -> Option<String> {
    let mut n = 0;
    for g in fp.items_mut().iter_mut().filter(|c| c.head().is_some_and(|h| GRAPHICS.contains(&h))) {
        if !g.child("layer").and_then(|l| l.arg(0)).is_some_and(|l| l.ends_with(layer)) {
            continue;
        }
        // KiCad 7+ writes (stroke (width w)), older files a bare (width w).
        let w = if g.child("stroke").is_some() { child_mut(g, "stroke").and_then(|s| child_mut(s, "width")) } else { child_mut(g, "width") };
        if let Some(w) = w.filter(|w| num(w, 0).is_some_and(|v| differs(v, width))) {
            set_num(w, 0, width);
            n += 1;
        }
    }
    (n > 0).then(|| format!("{n} {what} line width(s) set to {} mm", fmt(width)))
}

/// Moves everything in the footprint by `d`.
fn shift(x: &mut Sexpr, d: (f64, f64)) {
    match x.head() {
        Some("at" | "start" | "end" | "center" | "mid" | "xy") => {
            if let (Some(a), Some(b)) = (num(x, 0), num(x, 1)) {
                move_to(x, (a + d.0, b + d.1));
            }
        }
        // A pad's custom-shape primitives are relative to the pad, so only its position moves.
        Some("pad") => {
            if let Some(at) = child_mut(x, "at") {
                shift(at, d);
            }
        }
        // The 3D model's y axis points up, against the board's.
        Some("model") => {
            if let Some(xyz) = child_mut(x, "offset").and_then(|o| child_mut(o, "xyz")) {
                if let (Some(a), Some(b)) = (num(xyz, 0), num(xyz, 1)) {
                    set_num(xyz, 0, a + d.0);
                    set_num(xyz, 1, b - d.1);
                }
            }
        }
        _ => {
            if let Sexpr::List(v) = x {
                v.iter_mut().for_each(|c| shift(c, d));
            }
        }
    }
}

fn origin(fp: &mut Sexpr) -> Option<String> {
    let pads = numbered_pads(fp);
    let (target, what) = match fp.child("attr").and_then(|a| a.arg(0)) {
        Some("smd") => {
            let pts: Vec<_> = fp.children("pad").filter_map(|p| p.child("at")).filter_map(|a| Some((num(a, 0)?, num(a, 1)?))).collect();
            let (x0, y0, x1, y1) = bbox(&pts)?;
            (((x0 + x1) / 2.0, (y0 + y1) / 2.0), "the centre of the pads")
        }
        Some("through_hole") => (pads.iter().find(|p| p.number == "1")?.at, "pin 1"),
        _ => return None,
    };
    if !differs(target.0, 0.0) && !differs(target.1, 0.0) {
        return None;
    }
    shift(fp, (-target.0, -target.1));
    Some(format!("origin moved to {what} (by {}, {} mm)", fmt(-target.0), fmt(-target.1)))
}

/// The courtyard, or the pads when there is none.
fn outline(fp: &Sexpr) -> Option<(f64, f64, f64, f64)> {
    let mut pts = vec![];
    for g in fp.items().iter().filter(|g| g.head().is_some_and(|h| GRAPHICS.contains(&h)) && on_layer(g, &["F.CrtYd", "B.CrtYd"])) {
        points(g, &mut pts);
    }
    if pts.is_empty() {
        for p in numbered_pads(fp) {
            let h = p.size / 2.0;
            pts.extend([(p.at.0 - h, p.at.1 - h), (p.at.0 + h, p.at.1 + h)]);
        }
    }
    bbox(&pts)
}

fn ref_value(fp: &mut Sexpr) -> Option<String> {
    let (x0, y0, x1, y1) = outline(fp)?;
    let cx = (x0 + x1) / 2.0;
    let mut moved = vec![];
    for (key, old, y) in [("Reference", "reference", y0 - 1.0), ("Value", "value", y1 + 1.0)] {
        // KiCad 8+ has (property "Reference" ...), older files (fp_text reference ...).
        let node = fp.items_mut().iter_mut().find(|c| (c.head() == Some("property") && c.arg(0) == Some(key)) || (c.head() == Some("fp_text") && c.arg(0) == Some(old)));
        if node.and_then(|n| child_mut(n, "at")).is_some_and(|at| move_to(at, (cx, y))) {
            moved.push(key);
        }
    }
    (!moved.is_empty()).then(|| format!("{} moved {}", moved.join(" and "), if moved.len() == 2 { "above and below the courtyard" } else { "next to the courtyard" }))
}

fn pin1_marker(fp: &mut Sexpr, width: f64) -> Option<String> {
    let pads = numbered_pads(fp);
    if has_pin1_marker(fp, &pads) {
        return None;
    }
    let one = pads.iter().find(|p| p.number == "1" || p.number == "A1")?;
    let n = pads.len() as f64;
    let centre = pads.iter().fold((0.0, 0.0), |c, p| (c.0 + p.at.0 / n, c.1 + p.at.1 / n));
    let (dx, dy) = (one.at.0 - centre.0, one.at.1 - centre.1);
    let len = dx.hypot(dy);
    let dir = if len > 1e-6 { (dx / len, dy / len) } else { (-1.0, 0.0) };
    // Away from the centre and clear of the pad's corners (0.71 × its size).
    let reach = one.size * 0.75 + 0.5;
    let (x, y) = (one.at.0 + dir.0 * reach, one.at.1 + dir.1 * reach);
    let a = |v: f64| Sexpr::Atom(fmt(v));
    fp.items_mut().push(Sexpr::list(
        "fp_circle",
        vec![
            Sexpr::list("center", vec![a(x), a(y)]),
            Sexpr::list("end", vec![a(x + 0.2), a(y)]),
            Sexpr::list("stroke", vec![Sexpr::list("width", vec![a(width)]), Sexpr::list("type", vec![Sexpr::Atom("solid".into())])]),
            Sexpr::list("layer", vec![Sexpr::str("F.SilkS")]),
        ],
    ));
    Some(format!("pin-1 marker added on F.SilkS at ({}, {})", fmt(x), fmt(y)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr::{parse, to_string};

    fn only(r: Rules) -> Rules {
        Rules { silk_width: None, fab_width: None, courtyard_width: None, footprint_ref_value: false, footprint_origin: false, pin1_marker: false, ..r }
    }

    /// Four square pads around (1, 1): off-centre, wrong widths, text on top of the pads, no pin-1 marker.
    const FP: &str = r#"(footprint "FP" (layer "F.Cu")
        (property "Reference" "REF**" (at 1 1 0) (layer "F.SilkS"))
        (fp_text value "FP" (at 1 1) (layer "F.Fab"))
        (attr smd)
        (fp_line (start 0 -1) (end 2 -1) (stroke (width 0.2) (type solid)) (layer "F.SilkS"))
        (fp_line (start -1 -1) (end -1 3) (width 0.3) (layer "F.Fab"))
        (fp_rect (start -2 -2) (end 4 4) (stroke (width 0.1) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd rect (at 0 0) (size 0.5 0.5) (layers "F.Cu"))
        (pad "2" smd rect (at 0 2) (size 0.5 0.5) (layers "F.Cu"))
        (pad "3" smd rect (at 2 2) (size 0.5 0.5) (layers "F.Cu"))
        (pad "4" smd rect (at 2 0) (size 0.5 0.5) (layers "F.Cu"))
        (model "x.step" (offset (xyz 0 0 0))))"#;

    fn run(r: Rules) -> (Sexpr, Vec<String>) {
        let mut fp = parse(FP).unwrap();
        let changes = fix(&mut fp, &r);
        (fp, changes)
    }

    fn at(fp: &Sexpr, head: &str, arg: &str) -> String {
        to_string(fp.items().iter().find(|c| c.head() == Some(head) && c.arg(0) == Some(arg)).unwrap().child("at").unwrap()).trim().to_string()
    }

    #[test]
    fn line_widths_per_layer() {
        let (fp, c) = run(Rules { silk_width: Some(0.12), fab_width: Some(0.1), courtyard_width: Some(0.05), ..only(Rules::default()) });
        assert_eq!(c, ["1 silkscreen line width(s) set to 0.12 mm", "1 fab line width(s) set to 0.1 mm", "1 courtyard line width(s) set to 0.05 mm"]);
        let text = to_string(&fp);
        assert!(text.contains("(width 0.12)") && text.contains("(width 0.1)") && text.contains("(width 0.05)"));
    }

    #[test]
    fn smd_origin_moves_to_the_pad_centre() {
        let (fp, c) = run(Rules { footprint_origin: true, ..only(Rules::default()) });
        assert_eq!(c, ["origin moved to the centre of the pads (by -1, -1 mm)"]);
        assert_eq!(at(&fp, "pad", "1"), "(at -1 -1)");
        assert_eq!(at(&fp, "property", "Reference"), "(at 0 0 0)");
        assert!(to_string(&fp).contains("(start -3 -3)"), "courtyard moved too");
        assert!(to_string(&fp).contains("(xyz -1 1 0)"), "3D model follows, with its y axis flipped");
        // Through-hole: pin 1 becomes the origin.
        let mut tht = parse(&FP.replace("(attr smd)", "(attr through_hole)").replace("(at 0 0) (size", "(at 3 4) (size")).unwrap();
        fix(&mut tht, &Rules { footprint_origin: true, ..only(Rules::default()) });
        assert_eq!(at(&tht, "pad", "1"), "(at 0 0)");
    }

    #[test]
    fn reference_above_value_below_the_courtyard() {
        let (fp, c) = run(Rules { footprint_ref_value: true, ..only(Rules::default()) });
        assert_eq!(c, ["Reference and Value moved above and below the courtyard"]);
        assert_eq!(at(&fp, "property", "Reference"), "(at 1 -3 0)");
        assert_eq!(at(&fp, "fp_text", "value"), "(at 1 5)");
    }

    #[test]
    fn missing_pin1_marker_is_added() {
        let r = Rules { pin1_marker: true, ..only(Rules::default()) };
        assert!(!has_pin1_marker(&parse(FP).unwrap(), &numbered_pads(&parse(FP).unwrap())));
        let (fp, c) = run(r.clone());
        assert_eq!(c.len(), 1);
        assert!(c[0].starts_with("pin-1 marker added"), "{c:?}");
        assert!(has_pin1_marker(&fp, &numbered_pads(&fp)));
        // A footprint that already marks pin 1 is left alone.
        let mut again = fp.clone();
        assert!(fix(&mut again, &r).is_empty());
    }
}
