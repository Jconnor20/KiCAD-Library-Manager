//! A template's body graphics as shapes a graphical editor can move: lines, rectangles, circles and arcs.
//! Anything else in a drawn symbol (text, curves) is kept as it is and cannot be edited.

use super::atom;
use crate::sexpr::{self, Sexpr};

pub type Pt = (f64, f64);

#[derive(Clone, Debug, PartialEq)]
pub enum Geo {
    Line(Vec<Pt>),
    Rect(Pt, Pt),
    Circle(Pt, f64),
    /// Start, a point on the arc, end.
    Arc([Pt; 3]),
    /// Left alone.
    Other(Sexpr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub geo: Geo,
    /// Stroke width, mm.
    pub width: f64,
    /// `none`, `outline` or `background`.
    pub fill: String,
}

impl Shape {
    pub fn new(geo: Geo) -> Shape {
        Shape { geo, width: 0.254, fill: "none".into() }
    }

    /// Every editable point: line vertices, rectangle corners, a circle's centre, an arc's three points.
    pub fn points(&self) -> Vec<Pt> {
        match &self.geo {
            Geo::Line(v) => v.clone(),
            Geo::Rect(a, b) => vec![*a, *b],
            Geo::Circle(c, _) => vec![*c],
            Geo::Arc(p) => p.to_vec(),
            Geo::Other(_) => vec![],
        }
    }

    pub fn set_point(&mut self, i: usize, p: Pt) {
        match &mut self.geo {
            Geo::Line(v) => v[i] = p,
            Geo::Rect(a, b) => *[a, b][i] = p,
            Geo::Circle(c, _) => *c = p,
            Geo::Arc(a) => a[i] = p,
            Geo::Other(_) => {}
        }
    }

    pub fn translate(&mut self, d: Pt) {
        for i in 0..self.points().len() {
            let p = self.points()[i];
            self.set_point(i, (p.0 + d.0, p.1 + d.1));
        }
    }

    fn to_sexpr(&self) -> Sexpr {
        let xy = |p: &Pt| Sexpr::list("xy", vec![atom(p.0), atom(p.1)]);
        let at = |k: &str, p: &Pt| Sexpr::list(k, vec![atom(p.0), atom(p.1)]);
        let stroke = Sexpr::list("stroke", vec![Sexpr::list("width", vec![atom(self.width)]), Sexpr::list("type", vec![Sexpr::Atom("default".into())])]);
        let fill = Sexpr::list("fill", vec![Sexpr::list("type", vec![Sexpr::Atom(self.fill.clone())])]);
        match &self.geo {
            Geo::Line(v) => Sexpr::list("polyline", vec![Sexpr::list("pts", v.iter().map(xy).collect()), stroke, fill]),
            Geo::Rect(a, b) => Sexpr::list("rectangle", vec![at("start", a), at("end", b), stroke, fill]),
            Geo::Circle(c, r) => Sexpr::list("circle", vec![at("center", c), Sexpr::list("radius", vec![atom(*r)]), stroke, fill]),
            Geo::Arc([s, m, e]) => Sexpr::list("arc", vec![at("start", s), at("mid", m), at("end", e), stroke, fill]),
            Geo::Other(x) => x.clone(),
        }
    }
}

/// Points along the arc that starts at `a`, passes through `m` and ends at `e` (a straight run if they are in line).
pub fn arc_points(a: Pt, m: Pt, e: Pt) -> Vec<Pt> {
    let d = 2.0 * (a.0 * (m.1 - e.1) + m.0 * (e.1 - a.1) + e.0 * (a.1 - m.1));
    if d.abs() < 1e-9 {
        return vec![a, m, e];
    }
    let sq = |p: Pt| p.0 * p.0 + p.1 * p.1;
    let c = ((sq(a) * (m.1 - e.1) + sq(m) * (e.1 - a.1) + sq(e) * (a.1 - m.1)) / d, (sq(a) * (e.0 - m.0) + sq(m) * (a.0 - e.0) + sq(e) * (m.0 - a.0)) / d);
    let r = ((a.0 - c.0).powi(2) + (a.1 - c.1).powi(2)).sqrt();
    let ang = |p: Pt| (p.1 - c.1).atan2(p.0 - c.0);
    let tau = std::f64::consts::TAU;
    let ccw = |x: f64, y: f64| (y - x).rem_euclid(tau);
    let (ta, tm, te) = (ang(a), ang(m), ang(e));
    let sweep = if ccw(ta, tm) <= ccw(ta, te) { ccw(ta, te) } else { -ccw(te, ta) };
    (0..=24).map(|i| ta + sweep * i as f64 / 24.0).map(|t| (c.0 + r * t.cos(), c.1 + r * t.sin())).collect()
}

/// The shapes of a body (the text of a `_0_1` unit's graphics).
pub fn parse(body: &str) -> Vec<Shape> {
    let Ok(doc) = sexpr::parse(&format!("(g {body})")) else { return vec![] };
    let num = |x: &Sexpr, i| x.arg(i).and_then(|v| v.parse::<f64>().ok());
    let xy = |x: &Sexpr| Some((num(x, 0)?, num(x, 1)?));
    doc.items()
        .iter()
        .skip(1)
        .map(|g| {
            let geo = (|| {
                Some(match g.head()? {
                    "polyline" => Geo::Line(g.child("pts")?.children("xy").filter_map(xy).collect()),
                    "rectangle" => Geo::Rect(xy(g.child("start")?)?, xy(g.child("end")?)?),
                    "circle" => Geo::Circle(xy(g.child("center")?)?, num(g.child("radius")?, 0)?),
                    "arc" => Geo::Arc([xy(g.child("start")?)?, xy(g.child("mid")?)?, xy(g.child("end")?)?]),
                    _ => return None,
                })
            })()
            .unwrap_or_else(|| Geo::Other(g.clone()));
            let width = g.child("stroke").and_then(|s| s.child("width")).and_then(|w| num(w, 0)).unwrap_or(0.254);
            let fill = g.child("fill").and_then(|f| f.child("type")).and_then(|t| t.arg(0)).unwrap_or("none").to_string();
            Shape { geo, width, fill }
        })
        .collect()
}

pub fn to_body(shapes: &[Shape]) -> String {
    shapes.iter().map(|s| sexpr::to_string(&s.to_sexpr())).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_round_trip_and_keep_what_they_cannot_edit() {
        let body = r#"(rectangle (start -1 -2) (end 1 2) (stroke (width 0.3) (type default)) (fill (type background)))
            (circle (center 0 0) (radius 1.5) (stroke (width 0.254) (type default)) (fill (type none)))
            (arc (start 0 2) (mid 0.6 1) (end 0 0) (stroke (width 0.254) (type default)) (fill (type none)))
            (text "x" (at 0 0 0) (effects (font (size 1 1))))"#;
        let s = parse(body);
        assert_eq!(s.len(), 4);
        assert_eq!((s[0].width, s[0].fill.as_str()), (0.3, "background"));
        assert!(matches!(s[3].geo, Geo::Other(_)));
        assert_eq!(parse(&to_body(&s)), s);
        let mut moved = s.clone();
        moved[0].translate((1.0, 0.0));
        assert_eq!(moved[0].geo, Geo::Rect((0.0, -2.0), (2.0, 2.0)));
    }

    #[test]
    fn an_arc_runs_through_its_middle_point() {
        // A half circle of radius 1 from (1, 0) over the top to (-1, 0), and the same underneath.
        for (m, y) in [((0.0, 1.0), 1.0), ((0.0, -1.0), -1.0)] {
            let pts = arc_points((1.0, 0.0), m, (-1.0, 0.0));
            assert!((pts[0].0 - 1.0).abs() < 1e-9 && (pts[24].0 + 1.0).abs() < 1e-9);
            assert!(pts.iter().all(|p| (p.0.hypot(p.1) - 1.0).abs() < 1e-9 && p.1 * y >= -1e-9));
        }
    }
}
