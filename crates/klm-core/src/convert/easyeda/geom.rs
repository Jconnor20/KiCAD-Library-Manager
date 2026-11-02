//! Units and SVG-style path parsing shared by the symbol and footprint converters.

use std::f64::consts::PI;

/// EasyEDA draws in units of 10 mil.
pub const MM_PER_UNIT: f64 = 0.254;

pub type Pt = (f64, f64);

pub fn num(s: &str) -> f64 {
    s.trim().parse().unwrap_or(0.0)
}

/// KiCad number: at most 4 decimals, no trailing zeros, never "-0".
pub fn fmt(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.into() }
}

/// KiCad angle (counter-clockwise on screen) from an EasyEDA one (clockwise), in [0, 360).
pub fn kicad_angle(eda_rotation: f64) -> f64 {
    (360.0 - eda_rotation).rem_euclid(360.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcSeg {
    pub rx: f64,
    pub large: bool,
    pub sweep: bool,
    pub to: Pt,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line(Pt),
    Arc(ArcSeg),
    Cubic(Pt, Pt, Pt),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sub {
    pub start: Pt,
    pub segs: Vec<Seg>,
    pub closed: bool,
}

fn tokens(d: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    let mut prev = ' ';
    for c in d.chars() {
        match c {
            'a'..='z' | 'A'..='Z' if c != 'e' && c != 'E' => {
                flush(&mut cur, &mut out);
                out.push(c.to_string());
            }
            ' ' | ',' | '\t' | '\n' => flush(&mut cur, &mut out),
            '-' | '+' if !cur.is_empty() && prev != 'e' && prev != 'E' => {
                flush(&mut cur, &mut out);
                cur.push(c);
            }
            _ => cur.push(c),
        }
        prev = c;
    }
    flush(&mut cur, &mut out);
    out
}

/// Parses `M L H V C A Z` (absolute and relative). Other commands end the path.
pub fn parse_path(d: &str) -> Vec<Sub> {
    let toks = tokens(d);
    let mut subs: Vec<Sub> = vec![];
    let (mut i, mut cmd) = (0, ' ');
    let (mut cur, mut start) = ((0.0, 0.0), (0.0, 0.0));
    let arity = |c: char| match c.to_ascii_uppercase() {
        'M' | 'L' => 2,
        'H' | 'V' => 1,
        'C' => 6,
        'A' => 7,
        _ => 0,
    };
    while i < toks.len() {
        if toks[i].len() == 1 && toks[i].chars().all(|c| c.is_ascii_alphabetic()) {
            cmd = toks[i].chars().next().unwrap();
            i += 1;
            if cmd.eq_ignore_ascii_case(&'z') {
                if let Some(s) = subs.last_mut() {
                    s.closed = true;
                }
                cur = start;
                continue;
            }
        }
        let n = arity(cmd);
        if n == 0 || i + n > toks.len() {
            break;
        }
        let a: Vec<f64> = toks[i..i + n].iter().map(|t| num(t)).collect();
        i += n;
        let rel = cmd.is_ascii_lowercase();
        let abs = |p: Pt| if rel { (cur.0 + p.0, cur.1 + p.1) } else { p };
        match cmd.to_ascii_uppercase() {
            'M' => {
                cur = abs((a[0], a[1]));
                start = cur;
                subs.push(Sub { start, segs: vec![], closed: false });
                cmd = if rel { 'l' } else { 'L' }; // extra pairs after M are line-tos
            }
            'L' | 'H' | 'V' => {
                let p = match cmd.to_ascii_uppercase() {
                    'H' => (if rel { cur.0 + a[0] } else { a[0] }, cur.1),
                    'V' => (cur.0, if rel { cur.1 + a[0] } else { a[0] }),
                    _ => abs((a[0], a[1])),
                };
                cur = p;
                if let Some(s) = subs.last_mut() {
                    s.segs.push(Seg::Line(p));
                }
            }
            'C' => {
                let (c1, c2, p) = (abs((a[0], a[1])), abs((a[2], a[3])), abs((a[4], a[5])));
                cur = p;
                if let Some(s) = subs.last_mut() {
                    s.segs.push(Seg::Cubic(c1, c2, p));
                }
            }
            _ => {
                let p = abs((a[5], a[6]));
                cur = p;
                if let Some(s) = subs.last_mut() {
                    s.segs.push(Seg::Arc(ArcSeg { rx: a[0], large: a[3] != 0.0, sweep: a[4] != 0.0, to: p }));
                }
            }
        }
    }
    subs
}

/// Centre of the circular arc from `from` through `a`, and the point halfway along it.
pub fn arc_centre_mid(from: Pt, a: &ArcSeg) -> (Pt, Pt) {
    let (d, mid) = (((a.to.0 - from.0) / 2.0, (a.to.1 - from.1) / 2.0), ((from.0 + a.to.0) / 2.0, (from.1 + a.to.1) / 2.0));
    let half = d.0.hypot(d.1);
    if half == 0.0 {
        return (mid, mid);
    }
    let r = a.rx.abs().max(half);
    let h = (r * r - half * half).sqrt();
    // The centre lies on the chord's perpendicular bisector, on the side the flags pick.
    let sign = if a.large != a.sweep { 1.0 } else { -1.0 };
    let c = (mid.0 + sign * h * -d.1 / half, mid.1 + sign * h * d.0 / half);
    let t1 = (from.1 - c.1).atan2(from.0 - c.0);
    let mut delta = (a.to.1 - c.1).atan2(a.to.0 - c.0) - t1;
    if a.sweep && delta < 0.0 {
        delta += 2.0 * PI;
    } else if !a.sweep && delta > 0.0 {
        delta -= 2.0 * PI;
    }
    let tm = t1 + delta / 2.0;
    (c, (c.0 + r * tm.cos(), c.1 + r * tm.sin()))
}

/// Subpath as a polyline: arcs and curves are cut into short chords.
pub fn flatten(sub: &Sub) -> Vec<Pt> {
    let mut pts = vec![sub.start];
    for seg in &sub.segs {
        let from = *pts.last().unwrap();
        match *seg {
            Seg::Line(p) => pts.push(p),
            Seg::Cubic(c1, c2, p) => {
                for i in 1..=12 {
                    let t = i as f64 / 12.0;
                    let u = 1.0 - t;
                    let f = |a: f64, b: f64, c: f64, d: f64| u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d;
                    pts.push((f(from.0, c1.0, c2.0, p.0), f(from.1, c1.1, c2.1, p.1)));
                }
            }
            Seg::Arc(a) => {
                let (c, _) = arc_centre_mid(from, &a);
                let r = (from.0 - c.0).hypot(from.1 - c.1);
                let t1 = (from.1 - c.1).atan2(from.0 - c.0);
                let mut delta = (a.to.1 - c.1).atan2(a.to.0 - c.0) - t1;
                if a.sweep && delta < 0.0 {
                    delta += 2.0 * PI;
                } else if !a.sweep && delta > 0.0 {
                    delta -= 2.0 * PI;
                }
                let steps = ((delta.abs() / (PI / 12.0)).ceil() as usize).max(1);
                for i in 1..steps {
                    let t = t1 + delta * i as f64 / steps as f64;
                    pts.push((c.0 + r * t.cos(), c.1 + r * t.sin()));
                }
                pts.push(a.to);
            }
        }
    }
    pts
}

/// `"x y x y …"` (as in EasyEDA polylines and tracks) → points.
pub fn points(s: &str) -> Vec<Pt> {
    let n: Vec<f64> = s.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).map(num).collect();
    n.chunks_exact(2).map(|c| (c[0], c[1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_format() {
        assert_eq!(fmt(2.5400000001), "2.54");
        assert_eq!(fmt(-0.00001), "0");
        assert_eq!(fmt(10.0), "10");
    }

    #[test]
    fn path_commands() {
        let p = parse_path("M 40 -0 h-10");
        assert_eq!(p[0].start, (40.0, 0.0));
        assert_eq!(p[0].segs, [Seg::Line((30.0, 0.0))]);
        let p = parse_path("M10,10 L20 10 v5 H10 Z");
        assert_eq!(p[0].segs.len(), 3);
        assert!(p[0].closed);
        let p = parse_path("M3990.1575,3002.8605 A2.8648,2.8648 0 0 0 3990.1768,2997.1406");
        assert!(matches!(p[0].segs[0], Seg::Arc(a) if !a.large && !a.sweep && a.rx == 2.8648));
    }

    #[test]
    fn arc_midpoint_follows_sweep_flag() {
        // Quarter circle of radius 10 from (10,0) to (0,10) about the origin.
        let mk = |sweep| ArcSeg { rx: 10.0, large: false, sweep, to: (0.0, 10.0) };
        let (c, m) = arc_centre_mid((10.0, 0.0), &mk(true));
        assert!(c.0.abs() < 1e-9 && c.1.abs() < 1e-9);
        assert!((m.0 - 7.0711).abs() < 1e-3 && (m.1 - 7.0711).abs() < 1e-3);
        // The other direction round the same chord is centred at (10,10) and bulges the other way.
        let (c, m) = arc_centre_mid((10.0, 0.0), &mk(false));
        assert!((c.0 - 10.0).abs() < 1e-9 && (c.1 - 10.0).abs() < 1e-9);
        assert!((m.0 - 2.9289).abs() < 1e-3 && (m.1 - 2.9289).abs() < 1e-3);
    }

    #[test]
    fn rotation_is_mirrored() {
        assert_eq!(kicad_angle(90.0), 270.0);
        assert_eq!(kicad_angle(0.0), 0.0);
    }
}
