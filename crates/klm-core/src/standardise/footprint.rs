//! Footprint rules. Footprint coordinates have y pointing down.

use super::{Rules, bbox, child_mut, differs, fmt, move_to, num, set_num};
use crate::checks::{has_pin1_marker, numbered_pads, on_layer, points};
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const GRAPHICS: &[&str] = &["fp_line", "fp_rect", "fp_circle", "fp_arc", "fp_poly", "fp_curve"];

/// How a reference footprint looks, without its geometry: line width per layer, Reference/Value text and
/// where it sits relative to the courtyard, and the shape of the pin-1 marker.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FootprintStyle {
    /// Line width per layer, e.g. "F.SilkS": 0.12.
    pub widths: BTreeMap<String, f64>,
    /// By field: "Reference", "Value".
    pub texts: BTreeMap<String, TextStyle>,
    /// The pin-1 marker as drawn, centred on the origin; drawn where a footprint has no marker.
    pub pin1: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub layer: String,
    /// The `(font ...)` s-expression: size, thickness, face.
    pub font: Option<String>,
    /// Offset from the courtyard's centre (x) and from its top edge, or its bottom edge when `below` (y).
    pub dx: f64,
    pub dy: f64,
    pub below: bool,
}

fn is_graphic(c: &Sexpr) -> bool {
    c.head().is_some_and(|h| GRAPHICS.contains(&h))
}

/// Reference or Value: (property "Reference" ...) in KiCad 8+, (fp_text reference ...) before.
fn is_text(c: &Sexpr, key: &str) -> bool {
    (c.head() == Some("property") && c.arg(0) == Some(key)) || (c.head() == Some("fp_text") && c.arg(0) == Some(key.to_lowercase().as_str()))
}

fn width(g: &Sexpr) -> Option<f64> {
    num(g.child("stroke").and_then(|s| s.child("width")).or(g.child("width"))?, 0)
}

impl FootprintStyle {
    /// The style of `fp`, a footprint the user chose as the house reference for a class.
    pub fn from_footprint(fp: &Sexpr) -> Result<Self> {
        let mut style = Self::default();
        for g in fp.items().iter().filter(|c| is_graphic(c)) {
            if let (Some(layer), Some(w)) = (g.child("layer").and_then(|l| l.arg(0)), width(g)) {
                style.widths.entry(layer.to_string()).or_insert(w);
            }
        }
        let (x0, y0, x1, y1) = outline(fp).context("the footprint has no courtyard or pads")?;
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        for key in ["Reference", "Value"] {
            let Some(t) = fp.items().iter().find(|c| is_text(c, key)) else { continue };
            let Some((x, y)) = t.child("at").and_then(|a| Some((num(a, 0)?, num(a, 1)?))) else { continue };
            let below = y > cy;
            style.texts.insert(
                key.into(),
                TextStyle {
                    layer: t.child("layer").and_then(|l| l.arg(0)).unwrap_or("F.SilkS").into(),
                    font: t.child("effects").and_then(|e| e.child("font")).map(sexpr::to_string),
                    dx: x - cx,
                    dy: y - if below { y1 } else { y0 },
                    below,
                },
            );
        }
        style.pin1 = pin1_graphic(fp).map(|mut g| {
            let mut pts = vec![];
            points(&g, &mut pts);
            if let Some((a, b, c, d)) = bbox(&pts) {
                shift(&mut g, (-(a + c) / 2.0, -(b + d) / 2.0));
            }
            sexpr::to_string(&g)
        });
        Ok(style)
    }
}

/// The graphic that marks pin 1 in `fp`, found with the same test as the checks: the one graphic that
/// alone makes an otherwise bare footprint pass. `None` when the pad's shape is the marker or there is none.
fn pin1_graphic(fp: &Sexpr) -> Option<Sexpr> {
    let pads = numbered_pads(fp);
    let bare = |extra: Option<&Sexpr>| {
        let mut f = fp.clone();
        f.items_mut().retain(|c| !is_graphic(c) && !matches!(c.head(), Some("fp_text" | "text")));
        f.items_mut().extend(extra.cloned());
        f
    };
    if has_pin1_marker(&bare(None), &pads) {
        return None;
    }
    fp.items().iter().filter(|g| is_graphic(g)).find(|g| has_pin1_marker(&bare(Some(g)), &pads)).cloned()
}

/// Applies the footprint rules, then the house style of the part's class when there is one, and says what
/// changed. The style's widths and text placement replace the generic rules; its pin-1 marker replaces the
/// default dot (still only when the pin-1 rule is on).
pub(super) fn fix(fp: &mut Sexpr, r: &Rules, style: Option<&FootprintStyle>) -> Vec<String> {
    let mut out = vec![];
    let none = FootprintStyle::default();
    let style = style.unwrap_or(&none);
    for (width, layer, what) in [(r.silk_width, ".SilkS", "silkscreen"), (r.fab_width, ".Fab", "fab"), (r.courtyard_width, ".CrtYd", "courtyard")] {
        out.extend(width.and_then(|w| line_width(fp, layer, w, what, &style.widths)));
    }
    for (layer, w) in &style.widths {
        out.extend(line_width(fp, layer, *w, layer, &BTreeMap::new()));
    }
    // Origin first: placement and the pin-1 marker are worked out from the moved footprint.
    if r.footprint_origin {
        out.extend(origin(fp));
    }
    if r.footprint_ref_value && style.texts.is_empty() {
        out.extend(ref_value(fp));
    }
    out.extend(house_texts(fp, &style.texts));
    if r.pin1_marker {
        out.extend(pin1_marker(fp, r.silk_width.unwrap_or(0.12), style.pin1.as_deref()));
    }
    out
}

/// Sets the widths of lines on layers ending in `layer`, except on the layers in `except` (the house style's).
fn line_width(fp: &mut Sexpr, layer: &str, width: f64, what: &str, except: &BTreeMap<String, f64>) -> Option<String> {
    let mut n = 0;
    for g in fp.items_mut().iter_mut().filter(|c| is_graphic(c)) {
        if !g.child("layer").and_then(|l| l.arg(0)).is_some_and(|l| l.ends_with(layer) && !except.contains_key(l)) {
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

/// Reference and Value on the house style's layer and font, at its place relative to the courtyard.
fn house_texts(fp: &mut Sexpr, texts: &BTreeMap<String, TextStyle>) -> Option<String> {
    let (x0, y0, x1, y1) = outline(fp)?;
    let mut styled = vec![];
    for (key, t) in texts {
        let Some(node) = fp.items_mut().iter_mut().find(|c| is_text(c, key)) else { continue };
        let target = ((x0 + x1) / 2.0 + t.dx, if t.below { y1 } else { y0 } + t.dy);
        let mut changed = child_mut(node, "at").is_some_and(|at| move_to(at, target));
        if let Some(l) = child_mut(node, "layer").filter(|l| l.arg(0) != Some(t.layer.as_str())) {
            l.set_arg(0, Sexpr::str(&t.layer));
            changed = true;
        }
        if let Some(font) = t.font.as_deref().and_then(|f| sexpr::parse(f).ok()) {
            if child_mut(node, "effects").is_none() {
                node.items_mut().push(Sexpr::list("effects", vec![]));
            }
            let effects = child_mut(node, "effects").unwrap();
            match child_mut(effects, "font") {
                Some(old) if *old == font => {}
                Some(old) => {
                    *old = font;
                    changed = true;
                }
                None => {
                    effects.items_mut().push(font);
                    changed = true;
                }
            }
        }
        if changed {
            styled.push(key.as_str());
        }
    }
    (!styled.is_empty()).then(|| format!("{} placed and styled like the house footprint", styled.join(" and ")))
}

fn ref_value(fp: &mut Sexpr) -> Option<String> {
    let (x0, y0, x1, y1) = outline(fp)?;
    let cx = (x0 + x1) / 2.0;
    let mut moved = vec![];
    for (key, y) in [("Reference", y0 - 1.0), ("Value", y1 + 1.0)] {
        let node = fp.items_mut().iter_mut().find(|c| is_text(c, key));
        if node.and_then(|n| child_mut(n, "at")).is_some_and(|at| move_to(at, (cx, y))) {
            moved.push(key);
        }
    }
    (!moved.is_empty()).then(|| format!("{} moved {}", moved.join(" and "), if moved.len() == 2 { "above and below the courtyard" } else { "next to the courtyard" }))
}

fn pin1_marker(fp: &mut Sexpr, width: f64, house: Option<&str>) -> Option<String> {
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
    let house = house.and_then(|h| sexpr::parse(h).ok());
    let mut pts = vec![];
    if let Some(h) = &house {
        points(h, &mut pts);
    }
    let extent = bbox(&pts).map_or(0.0, |(a, b, c, d)| (c - a).max(d - b) / 2.0);
    // Away from the centre and clear of the pad's corners (0.71 × its size).
    let reach = one.size * 0.75 + 0.5 + extent;
    let (x, y) = (one.at.0 + dir.0 * reach, one.at.1 + dir.1 * reach);
    if let Some(mut h) = house {
        shift(&mut h, (x, y));
        fp.items_mut().push(h);
        if has_pin1_marker(fp, &pads) {
            return Some(format!("house pin-1 marker added at ({}, {})", fmt(x), fmt(y)));
        }
        // Too large to read as a marker where it landed: the default dot instead.
        fp.items_mut().pop();
    }
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
        let changes = fix(&mut fp, &r, None);
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
        fix(&mut tht, &Rules { footprint_origin: true, ..only(Rules::default()) }, None);
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
        assert!(fix(&mut again, &r, None).is_empty());
    }

    #[test]
    fn house_style_copies_widths_text_and_pin1_marker_but_no_geometry() {
        // Reference: a triangle on silk marks pin 1, text on F.Fab half a mm above the courtyard.
        let reference = parse(r#"(footprint "REF" (layer "F.Cu")
            (property "Reference" "REF**" (at 0 -3.5 0) (layer "F.Fab") (effects (font (size 0.7 0.7) (thickness 0.1))))
            (attr smd)
            (fp_line (start -2 -2) (end 2 -2) (stroke (width 0.15) (type solid)) (layer "F.SilkS"))
            (fp_poly (pts (xy -2.5 -1.5) (xy -2.1 -1.5) (xy -2.5 -1.1)) (stroke (width 0.15) (type solid)) (fill solid) (layer "F.SilkS"))
            (fp_rect (start -3 -3) (end 3 3) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
            (pad "1" smd rect (at -1.5 -1) (size 0.6 0.6) (layers "F.Cu"))
            (pad "2" smd rect (at -1.5 1) (size 0.6 0.6) (layers "F.Cu"))
            (pad "3" smd rect (at 1.5 1) (size 0.6 0.6) (layers "F.Cu"))
            (pad "4" smd rect (at 1.5 -1) (size 0.6 0.6) (layers "F.Cu")))"#).unwrap();
        let style = FootprintStyle::from_footprint(&reference).unwrap();
        assert_eq!(style.widths.get("F.SilkS"), Some(&0.15));
        let t = &style.texts["Reference"];
        assert_eq!((t.layer.as_str(), t.dx, t.dy, t.below), ("F.Fab", 0.0, -0.5, false));
        assert!(style.pin1.as_deref().is_some_and(|p| p.contains("fp_poly")), "{:?}", style.pin1);

        let r = Rules { silk_width: Some(0.12), footprint_ref_value: true, pin1_marker: true, ..only(Rules::default()) };
        let mut fp = parse(FP).unwrap();
        let pads_before: Vec<_> = fp.children("pad").cloned().collect();
        let changes = fix(&mut fp, &r, Some(&style));
        assert!(changes.iter().any(|c| c == "1 F.SilkS line width(s) set to 0.15 mm"), "{changes:?}");
        assert!(!changes.iter().any(|c| c.starts_with("1 silkscreen")), "the house width replaces the generic rule: {changes:?}");
        assert!(changes.iter().any(|c| c.starts_with("house pin-1 marker added")), "{changes:?}");
        // Courtyard of FP is (-2, -2)..(4, 4): Reference half a mm above it, centred, on F.Fab.
        assert_eq!(at(&fp, "property", "Reference"), "(at 1 -2.5 0)");
        assert!(to_string(&fp).contains("(size 0.7 0.7)"));
        assert_eq!(fp.children("pad").cloned().collect::<Vec<_>>(), pads_before, "pads never move");
        assert_eq!(fp.children("fp_poly").count(), 1);
        let mut again = fp.clone();
        assert!(fix(&mut again, &r, Some(&style)).is_empty(), "a second run changes nothing");
    }
}
