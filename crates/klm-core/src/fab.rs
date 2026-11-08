//! One footprint against a fab's limits: copper gaps, drills, annular rings, silkscreen and courtyard.
//!
//! Only reads. The safe fixes (silk width, silk clipped off pads, courtyard grown) live with the other
//! footprint rules in `standardise::footprint` and use the geometry here, so a fixed footprint passes.

use crate::checks::{Issue, Pad, Severity, list, numbered_pads, on_layer, points, xy};
use crate::convert::easyeda::geom::fmt;
use crate::fab_data::FabLimits;
use crate::sexpr::{self, Sexpr};
use std::collections::BTreeSet;

pub(crate) type Pt = (f64, f64);
/// (min x, min y, max x, max y).
pub(crate) type Rect = (f64, f64, f64, f64);

/// Slack for numbers that went through the file's 4-decimal rounding.
const EPS: f64 = 1e-4;
const GRAPHICS: &[&str] = &["fp_line", "fp_rect", "fp_circle", "fp_arc", "fp_poly", "fp_curve"];
/// Silk-to-copper keep-out when the fab gives no solder-mask web.
const SILK_CLEARANCE: f64 = 0.1;

/// A pad's copper as a convex outline (a point, a segment or a rectangle) grown by `r`: exact for rect,
/// roundrect, circle and oval pads, at any rotation.
pub(crate) struct Shape {
    pts: Vec<Pt>,
    r: f64,
}

pub(crate) fn shape(p: &Pad) -> Shape {
    let (w, h) = p.dims;
    let r = match p.shape.as_str() {
        "circle" => w / 2.0,
        "oval" => w.min(h) / 2.0,
        "roundrect" => p.rratio.clamp(0.0, 0.5) * w.min(h),
        // shortcut: trapezoid and custom pads are taken as their size rectangle; read their primitives if that misreports.
        _ => 0.0,
    };
    let (hw, hh) = if p.shape == "circle" { (0.0, 0.0) } else { ((w / 2.0 - r).max(0.0), (h / 2.0 - r).max(0.0)) };
    let local = match (hw > 1e-9, hh > 1e-9) {
        (false, false) => vec![(0.0, 0.0)],
        (true, false) => vec![(-hw, 0.0), (hw, 0.0)],
        (false, true) => vec![(0.0, -hh), (0.0, hh)],
        _ => vec![(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)],
    };
    // KiCad turns counter-clockwise on screen, where y points down.
    let (s, c) = p.angle.to_radians().sin_cos();
    Shape { pts: local.iter().map(|&(x, y)| (p.at.0 + x * c + y * s, p.at.1 - x * s + y * c)).collect(), r }
}

pub(crate) fn shape_bbox(s: &Shape) -> Rect {
    let b = bbox(&s.pts).unwrap_or_default();
    (b.0 - s.r, b.1 - s.r, b.2 + s.r, b.3 + s.r)
}

pub(crate) fn bbox(pts: &[Pt]) -> Option<Rect> {
    let f = *pts.first()?;
    Some(pts.iter().fold((f.0, f.1, f.0, f.1), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))))
}

fn edges(pts: &[Pt]) -> Vec<(Pt, Pt)> {
    if pts.len() <= 2 { vec![(pts[0], pts[pts.len() - 1])] } else { (0..pts.len()).map(|i| (pts[i], pts[(i + 1) % pts.len()])).collect() }
}

fn cross(o: Pt, a: Pt, b: Pt) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

fn point_seg(p: Pt, (a, b): (Pt, Pt)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

fn seg_seg(s: (Pt, Pt), t: (Pt, Pt)) -> f64 {
    let crosses = cross(s.0, s.1, t.0) * cross(s.0, s.1, t.1) < 0.0 && cross(t.0, t.1, s.0) * cross(t.0, t.1, s.1) < 0.0;
    if crosses { 0.0 } else { point_seg(s.0, t).min(point_seg(s.1, t)).min(point_seg(t.0, s)).min(point_seg(t.1, s)) }
}

fn contains(s: &Shape, p: Pt) -> bool {
    let c: Vec<f64> = edges(&s.pts).iter().map(|e| cross(e.0, e.1, p)).collect();
    s.pts.len() >= 3 && (c.iter().all(|&v| v >= 0.0) || c.iter().all(|&v| v <= 0.0))
}

/// Copper-to-copper gap; negative when the shapes overlap.
pub(crate) fn gap(a: &Shape, b: &Shape) -> f64 {
    let d = if contains(a, b.pts[0]) || contains(b, a.pts[0]) {
        0.0
    } else {
        edges(&a.pts).iter().flat_map(|&e| edges(&b.pts).into_iter().map(move |f| seg_seg(e, f))).fold(f64::MAX, f64::min)
    };
    d - a.r - b.r
}

/// Whether the pad has copper on side "F" or "B".
// shortcut: inner layers are ignored, since a footprint's pads almost never sit only on them.
pub(crate) fn on_side(p: &Pad, side: &str) -> bool {
    p.layers.iter().any(|l| l == "*.Cu" || l == "F&B.Cu" || *l == format!("{side}.Cu"))
}

fn shares_copper(a: &Pad, b: &Pad) -> bool {
    ["F", "B"].iter().any(|s| on_side(a, s) && on_side(b, s))
}

/// "F" or "B" for a graphic on a silkscreen layer.
pub(crate) fn silk_side(g: &Sexpr) -> Option<&'static str> {
    match g.child("layer").and_then(|l| l.arg(0)) {
        Some("F.SilkS") => Some("F"),
        Some("B.SilkS") => Some("B"),
        _ => None,
    }
}

pub(crate) fn is_graphic(g: &Sexpr) -> bool {
    g.head().is_some_and(|h| GRAPHICS.contains(&h))
}

pub(crate) fn stroke_width(g: &Sexpr) -> Option<f64> {
    g.child("stroke").and_then(|s| s.child("width")).or(g.child("width"))?.arg(0)?.parse().ok()
}

/// A filled shape drawn with a zero-width stroke (a pin-1 triangle) has no line to be too thin.
pub(crate) fn thin_line(g: &Sexpr, min: f64) -> bool {
    let filled = g.child("fill").is_some_and(|f| sexpr::to_string(f).contains("solid") || sexpr::to_string(f).contains("yes"));
    stroke_width(g).is_some_and(|w| w + EPS < min && !(filled && w <= 0.0))
}

/// The straight segments of a silkscreen graphic.
// shortcut: circles, arcs and curves give none, so they are never reported as crossing a pad; split them into chords if that matters.
fn segments(g: &Sexpr) -> Vec<(Pt, Pt)> {
    let pt = |h: &str| g.child(h).and_then(xy);
    match (g.head(), pt("start"), pt("end")) {
        (Some("fp_line"), Some(a), Some(b)) => vec![(a, b)],
        (Some("fp_rect"), Some(a), Some(b)) => edges(&[a, (b.0, a.1), b, (a.0, b.1)]),
        (Some("fp_poly"), ..) => {
            let pts: Vec<Pt> = g.child("pts").map(|p| p.children("xy").filter_map(xy).collect()).unwrap_or_default();
            if pts.len() < 2 { vec![] } else { edges(&pts) }
        }
        _ => vec![],
    }
}

/// Where silkscreen may not go: each pad's box grown by the mask web (or 0.1 mm), as (pad, side, box).
pub(crate) fn keepouts(fp: &Sexpr, limits: &FabLimits) -> Vec<(String, &'static str, Rect)> {
    let c = limits.min_mask_web.unwrap_or(SILK_CLEARANCE);
    let mut out = vec![];
    for p in numbered_pads(fp) {
        let b = shape_bbox(&shape(&p));
        for side in ["F", "B"].into_iter().filter(|s| on_side(&p, s)) {
            out.push((p.number.clone(), side, (b.0 - c, b.1 - c, b.2 + c, b.3 + c)));
        }
    }
    out
}

pub(crate) fn grow(r: Rect, d: f64) -> Rect {
    (r.0 - d, r.1 - d, r.2 + d, r.3 + d)
}

/// The part of segment a–b inside `r`, as a range of t in 0..1 (Liang–Barsky); `None` when it only touches.
pub(crate) fn inside(a: Pt, b: Pt, r: Rect) -> Option<(f64, f64)> {
    let d = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-d.0, a.0 - r.0), (d.0, r.2 - a.0), (-d.1, a.1 - r.1), (d.1, r.3 - a.1)] {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    // Less than a micron inside is the file's rounding, not an overlap.
    ((t1 - t0) * d.0.hypot(d.1) > 1e-3).then_some((t0, t1))
}

/// Per side with a courtyard: (side, courtyard box, box of the pads on that side and the fab outline).
// shortcut: circles and arcs count by their centre and end points, so a round courtyard's box is approximate.
pub(crate) fn courtyards(fp: &Sexpr) -> Vec<(&'static str, Rect, Rect)> {
    let pads = numbered_pads(fp);
    let mut out = vec![];
    for side in ["F", "B"] {
        let (mut crt, mut body) = (vec![], vec![]);
        for g in fp.items().iter().filter(|g| is_graphic(g)) {
            if on_layer(g, &[&format!("{side}.CrtYd")]) {
                points(g, &mut crt);
            } else if on_layer(g, &[&format!("{side}.Fab")]) {
                points(g, &mut body);
            }
        }
        for p in pads.iter().filter(|p| on_side(p, side)) {
            let b = shape_bbox(&shape(p));
            body.extend([(b.0, b.1), (b.2, b.3)]);
        }
        if let (Some(c), Some(b)) = (bbox(&crt), bbox(&body)) {
            out.push((side, c, b));
        }
    }
    out
}

fn mm(v: f64) -> String {
    format!("{} mm", fmt(v))
}

/// Everything in `fp` the limits rule out, as warnings. A limit that is `None` checks nothing.
pub fn check_footprint(fp: &Sexpr, limits: &FabLimits) -> Vec<Issue> {
    let mut out = vec![];
    let mut warn = |message: String| out.push(Issue { severity: Severity::Warning, message });
    let pads = numbered_pads(fp);

    if let Some(min) = limits.min_clearance {
        let shapes: Vec<Shape> = pads.iter().map(shape).collect();
        let mut close = vec![];
        for i in 0..pads.len() {
            for j in i + 1..pads.len() {
                // Pads with one number are one net, so their copper may touch.
                if pads[i].number != pads[j].number && shares_copper(&pads[i], &pads[j]) {
                    let g = gap(&shapes[i], &shapes[j]);
                    if g + EPS < min {
                        close.push((g, i, j));
                    }
                }
            }
        }
        if let Some(&(g, i, j)) = close.iter().min_by(|a, b| a.0.total_cmp(&b.0)) {
            let more = if close.len() > 1 { format!(" ({} more pairs too close)", close.len() - 1) } else { String::new() };
            let what = if g < 0.0 { "overlap".to_string() } else { format!("gap {}", mm(g)) };
            warn(format!("Pads {}–{} copper {what} < {} clearance{more}", pads[i].number, pads[j].number, mm(min)));
        }
    }

    let drilled: Vec<(&Pad, (f64, f64))> = pads.iter().filter_map(|p| Some((p, p.drill?))).collect();
    let worst = |v: Vec<(&str, f64)>| -> Option<(String, f64)> {
        let smallest = v.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
        (!v.is_empty()).then(|| (list(v.iter().map(|x| x.0).collect::<BTreeSet<_>>()), smallest))
    };
    if let Some(min) = limits.min_drill {
        let small = drilled.iter().map(|(p, d)| (p.number.as_str(), d.0.min(d.1))).filter(|x| x.1 + EPS < min).collect();
        if let Some((names, d)) = worst(small) {
            warn(format!("Pad(s) {names} drill {} < {} minimum", mm(d), mm(min)));
        }
    }
    if let Some(min) = limits.min_annular_ring {
        let thin = drilled.iter().map(|(p, d)| (p.number.as_str(), ((p.dims.0 - d.0) / 2.0).min((p.dims.1 - d.1) / 2.0))).filter(|x| x.1 + EPS < min).collect();
        if let Some((names, r)) = worst(thin) {
            warn(format!("Pad(s) {names} annular ring {} < {} minimum", mm(r), mm(min)));
        }
    }

    let silk: Vec<&Sexpr> = fp.items().iter().filter(|g| is_graphic(g) && silk_side(g).is_some()).collect();
    if let Some(min) = limits.min_silk_width {
        let thin: Vec<f64> = silk.iter().filter(|g| thin_line(g, min)).filter_map(|g| stroke_width(g)).collect();
        if !thin.is_empty() {
            warn(format!("{} silkscreen line(s) as thin as {} < {} minimum", thin.len(), mm(thin.iter().copied().fold(f64::INFINITY, f64::min)), mm(min)));
        }
        let keep = keepouts(fp, limits);
        let mut hit = BTreeSet::new();
        for g in &silk {
            let half = stroke_width(g).unwrap_or(0.0) / 2.0;
            for (a, b) in segments(g) {
                hit.extend(keep.iter().filter(|k| Some(k.1) == silk_side(g) && inside(a, b, grow(k.2, half)).is_some()).map(|k| k.0.as_str()));
            }
        }
        if !hit.is_empty() {
            warn(format!("Silkscreen runs over pad(s) {} (keep it {} clear of copper)", list(hit), mm(limits.min_mask_web.unwrap_or(SILK_CLEARANCE))));
        }
    }
    if let Some(min) = limits.min_silk_height {
        let small: Vec<f64> = fp
            .items()
            .iter()
            .filter(|t| matches!(t.head(), Some("fp_text" | "property")) && matches!(t.child("layer").and_then(|l| l.arg(0)), Some("F.SilkS" | "B.SilkS")) && !hidden(t))
            .filter_map(|t| t.child("effects")?.child("font")?.child("size")?.arg(0)?.parse::<f64>().ok())
            .filter(|h| h + EPS < min)
            .collect();
        if !small.is_empty() {
            warn(format!("{} silkscreen text(s) as small as {} high < {} minimum", small.len(), mm(small.iter().copied().fold(f64::INFINITY, f64::min)), mm(min)));
        }
    }
    if let Some(e) = limits.courtyard_excess {
        for (side, c, b) in courtyards(fp) {
            let sides = [("left", b.0 - c.0), ("top", b.1 - c.1), ("right", c.2 - b.2), ("bottom", c.3 - b.3)];
            let tight: Vec<_> = sides.iter().filter(|s| s.1 + EPS < e).collect();
            if !tight.is_empty() {
                let least = tight.iter().map(|s| s.1).fold(f64::INFINITY, f64::min);
                let names: Vec<&str> = tight.iter().map(|s| s.0).collect();
                warn(format!("{side}.CrtYd courtyard is {} from the pads or body ({}) < {} excess", mm(least), names.join(", "), mm(e)));
            }
        }
    }
    out
}

/// Hidden text: `hide` as a bare flag or `(hide yes)`, anywhere in the node.
fn hidden(t: &Sexpr) -> bool {
    t.items().iter().any(|c| c.text() == Some("hide") || (c.head() == Some("hide") && c.arg(0) != Some("no")) || hidden(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr::parse;

    fn limits() -> FabLimits {
        FabLimits {
            min_clearance: Some(0.127),
            min_drill: Some(0.3),
            min_annular_ring: Some(0.13),
            min_silk_width: Some(0.153),
            min_silk_height: Some(1.0),
            min_mask_web: Some(0.1),
            courtyard_excess: Some(0.25),
            ..Default::default()
        }
    }

    /// Two 1 × 1.2 mm pads 2 mm apart, silk clear of them, courtyard 0.5 mm out: passes every limit.
    const CLEAN: &str = r#"(footprint "C" (layer "F.Cu")
        (property "Reference" "REF**" (at 0 -2 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15))))
        (fp_line (start -0.3 -1) (end 0.3 -1) (stroke (width 0.2) (type solid)) (layer "F.SilkS"))
        (fp_rect (start -2 -1.1) (end 2 1.1) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd roundrect (at -1 0) (size 1 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25))
        (pad "2" smd roundrect (at 1 0) (size 1 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25)))"#;

    fn messages(fp: &str) -> Vec<String> {
        check_footprint(&parse(fp).unwrap(), &limits()).into_iter().map(|i| i.message).collect()
    }

    #[test]
    fn clean_footprint_and_no_limits_give_nothing() {
        assert!(messages(CLEAN).is_empty(), "{:?}", messages(CLEAN));
        let bad = CLEAN.replace("(at 1 0)", "(at -0.9 0)");
        assert!(check_footprint(&parse(&bad).unwrap(), &FabLimits::default()).is_empty());
    }

    #[test]
    fn pads_too_close() {
        // Pad 1's right edge is at -0.5; pad 2 at x 0.1 starts at -0.4.
        let m = messages(&CLEAN.replace("(at 1 0)", "(at 0.1 0)"));
        assert!(m.iter().any(|m| m == "Pads 1–2 copper gap 0.1 mm < 0.127 mm clearance"), "{m:?}");
        // Turned 90°, pad 2 is 1.2 wide along x, so at x -0.2 it reaches -0.8 and overlaps pad 1.
        let m = messages(&CLEAN.replace("(at 1 0)", "(at -0.2 0 90)"));
        assert!(m.iter().any(|m| m.starts_with("Pads 1–2 copper overlap")), "{m:?}");
    }

    #[test]
    fn gap_between_round_and_oval_pads() {
        let pad = |s: &str| numbered_pads(&parse(&format!(r#"(footprint "x" {s})"#)).unwrap()).remove(0);
        let a = shape(&pad(r#"(pad "1" thru_hole circle (at 0 0) (size 1 1) (drill 0.5) (layers "*.Cu"))"#));
        let b = shape(&pad(r#"(pad "2" thru_hole oval (at 2 0) (size 1 2) (drill 0.5) (layers "*.Cu"))"#));
        assert!((gap(&a, &b) - 1.0).abs() < 1e-9);
        // Turned 90°, the oval's end reaches to x = 1: gap 0.5.
        let b = shape(&pad(r#"(pad "2" thru_hole oval (at 2 0 90) (size 1 2) (drill 0.5) (layers "*.Cu"))"#));
        assert!((gap(&a, &b) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn small_drill_and_thin_ring() {
        let tht = r#"(footprint "T" (layer "F.Cu")
            (pad "1" thru_hole circle (at 0 0) (size 1.6 1.6) (drill 0.2) (layers "*.Cu" "*.Mask"))
            (pad "2" thru_hole oval (at 2.54 0) (size 1.2 1.7) (drill oval 1 1.2) (layers "*.Cu" "*.Mask")))"#;
        let m = messages(tht);
        assert!(m.contains(&"Pad(s) 1 drill 0.2 mm < 0.3 mm minimum".to_string()), "{m:?}");
        assert!(m.contains(&"Pad(s) 2 annular ring 0.1 mm < 0.13 mm minimum".to_string()), "{m:?}");
    }

    #[test]
    fn silk_problems() {
        let m = messages(&CLEAN.replace("(start -0.3 -1) (end 0.3 -1) (stroke (width 0.2)", "(start -1.5 -0.5) (end 1.5 -0.5) (stroke (width 0.1)").replace("(size 1 1) (thickness", "(size 0.8 0.8) (thickness"));
        assert!(m.contains(&"1 silkscreen line(s) as thin as 0.1 mm < 0.153 mm minimum".to_string()), "{m:?}");
        assert!(m.contains(&"Silkscreen runs over pad(s) 1, 2 (keep it 0.1 mm clear of copper)".to_string()), "{m:?}");
        assert!(m.contains(&"1 silkscreen text(s) as small as 0.8 mm high < 1 mm minimum".to_string()), "{m:?}");
        // Hidden text isn't printed, so its size doesn't matter.
        assert!(messages(&CLEAN.replace("(size 1 1) (thickness 0.15))", "(size 0.5 0.5) (thickness 0.15)) (hide yes)")).is_empty());
    }

    #[test]
    fn tight_courtyard() {
        let m = messages(&CLEAN.replace("(start -2 -1.1) (end 2 1.1)", "(start -1.6 -0.7) (end 2 0.7)"));
        assert_eq!(m, ["F.CrtYd courtyard is 0.1 mm from the pads or body (left, top, bottom) < 0.25 mm excess"]);
    }
}
