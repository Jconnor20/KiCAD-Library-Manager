//! EasyEDA schematic symbol → KiCad `.kicad_sym` (one symbol, one sub-symbol per unit).

use super::geom::*;
use serde_json::Value;
use std::fmt::Write;

const GRID: f64 = 1.27; // KiCad's schematic grid; pins must stay on it

struct Unit {
    gfx: String,
    pins: String,
    /// min x, min y, max x, max y in mm
    bbox: Option<[f64; 4]>,
}

struct Ctx {
    ox: f64,
    oy: f64,
    bbox: Option<[f64; 4]>,
}

impl Ctx {
    /// EasyEDA (y down) → KiCad symbol coordinates in mm (y up).
    fn pt(&mut self, p: Pt) -> (f64, f64) {
        let (x, y) = ((p.0 - self.ox) * MM_PER_UNIT, -(p.1 - self.oy) * MM_PER_UNIT);
        let b = self.bbox.get_or_insert([x, y, x, y]);
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        (x, y)
    }

    fn xy(&mut self, p: Pt) -> String {
        let (x, y) = self.pt(p);
        format!("(xy {} {})", fmt(x), fmt(y))
    }
}

fn stroke(width: &str) -> String {
    format!("(stroke (width {}) (type default))", fmt(num(width) * MM_PER_UNIT))
}

fn fill(color: &str, closed: bool) -> &'static str {
    if closed && !color.is_empty() && color != "none" { "(fill (type background))" } else { "(fill (type none))" }
}

fn polyline(ctx: &mut Ctx, pts: &[Pt], stroke_w: &str, fill_color: &str, closed: bool) -> String {
    let mut pts = pts.to_vec();
    if closed && pts.first() != pts.last() {
        pts.push(pts[0]);
    }
    let xy: Vec<String> = pts.iter().map(|p| ctx.xy(*p)).collect();
    let closed = closed || pts.first() == pts.last();
    format!("(polyline (pts {}) {} {})\n", xy.join(" "), stroke(stroke_w), fill(fill_color, closed))
}

fn convert_unit(shapes: &[&str], ox: f64, oy: f64, n_total_pins: &mut usize, passive: bool) -> Unit {
    let mut ctx = Ctx { ox, oy, bbox: None };
    let (mut gfx, mut pins, mut pin_count) = (String::new(), String::new(), 0);
    for s in shapes {
        let f: Vec<&str> = s.split('~').collect();
        let g = |i: usize| f.get(i).copied().unwrap_or("");
        match f[0] {
            "R" => {
                let (x, y) = (num(g(1)), num(g(2)));
                let (a, b) = (ctx.pt((x, y)), ctx.pt((x + num(g(5)), y + num(g(6)))));
                let _ = writeln!(gfx, "(rectangle (start {} {}) (end {} {}) {} {})", fmt(a.0), fmt(a.1), fmt(b.0), fmt(b.1), stroke(g(8)), fill(g(11), true));
            }
            "E" => {
                let (c, rx, ry) = ((num(g(1)), num(g(2))), num(g(3)), num(g(4)));
                if (rx - ry).abs() < 0.01 {
                    let m = ctx.pt(c);
                    ctx.pt((c.0 - rx, c.1 - rx));
                    ctx.pt((c.0 + rx, c.1 + rx));
                    let _ =
                        writeln!(gfx, "(circle (center {} {}) (radius {}) {} {})", fmt(m.0), fmt(m.1), fmt(rx * MM_PER_UNIT), stroke(g(6)), fill(g(8), true));
                } else {
                    let ring: Vec<Pt> = (0..24)
                        .map(|i| {
                            let t = i as f64 * std::f64::consts::TAU / 24.0;
                            (c.0 + rx * t.cos(), c.1 + ry * t.sin())
                        })
                        .collect();
                    gfx += &polyline(&mut ctx, &ring, g(6), g(8), true);
                }
            }
            "PL" | "PG" => {
                let pts = points(g(1));
                if pts.len() >= 2 {
                    gfx += &polyline(&mut ctx, &pts, g(3), g(5), f[0] == "PG");
                }
            }
            "PT" => {
                for sub in parse_path(g(1)) {
                    let pts = flatten(&sub);
                    if pts.len() >= 2 {
                        gfx += &polyline(&mut ctx, &pts, g(3), g(5), sub.closed);
                    }
                }
            }
            "A" => {
                for sub in parse_path(g(1)) {
                    let mut from = sub.start;
                    for seg in &sub.segs {
                        if let Seg::Arc(a) = seg {
                            let (_, mid) = arc_centre_mid(from, a);
                            let (p1, pm, p2) = (ctx.pt(from), ctx.pt(mid), ctx.pt(a.to));
                            let _ = writeln!(
                                gfx,
                                "(arc (start {} {}) (mid {} {}) (end {} {}) {} {})",
                                fmt(p1.0),
                                fmt(p1.1),
                                fmt(pm.0),
                                fmt(pm.1),
                                fmt(p2.0),
                                fmt(p2.1),
                                stroke(g(4)),
                                fill(g(6), false)
                            );
                        }
                        from = match seg {
                            Seg::Line(p) | Seg::Cubic(_, _, p) => *p,
                            Seg::Arc(a) => a.to,
                        };
                    }
                }
            }
            // Mark "L" is a free label; "P" and "N" are the reference and name, which KiCad has as fields.
            "T" if g(1) == "L" && !g(12).is_empty() => {
                let p = ctx.pt((num(g(2)), num(g(3))));
                let justify = match g(14) {
                    "start" => " (justify left)",
                    "end" => " (justify right)",
                    _ => "",
                };
                let _ = writeln!(
                    gfx,
                    "(text {:?} (at {} {} {}) (effects (font (size 1.27 1.27)){justify}))",
                    g(12),
                    fmt(p.0),
                    fmt(p.1),
                    fmt(kicad_angle(num(g(4))))
                );
            }
            "P" => {
                pins += &pin(&mut ctx, s, passive);
                pin_count += 1;
            }
            _ => {}
        }
    }
    *n_total_pins += pin_count;
    Unit { gfx, pins, bbox: ctx.bbox }
}

fn pin(ctx: &mut Ctx, s: &str, passive: bool) -> String {
    let seg: Vec<Vec<&str>> = s.split("^^").map(|p| p.split('~').collect()).collect();
    let at = |i: usize, j: usize| seg.get(i).and_then(|v| v.get(j)).copied().unwrap_or("");
    let conn = (num(at(0, 4)), num(at(0, 5)));
    // Direction from the connection point into the body, read off the pin's own line.
    let subs = parse_path(at(2, 0));
    let (end, len) = match subs.first().and_then(|s| s.segs.first()) {
        Some(Seg::Line(e)) => (*e, (e.0 - conn.0).hypot(e.1 - conn.1)),
        _ => {
            let rot = num(at(0, 6)).to_radians();
            ((conn.0 - rot.cos() * 10.0, conn.1 + rot.sin() * 10.0), 10.0)
        }
    };
    let (dx, dy) = (end.0 - conn.0, -(end.1 - conn.1));
    let angle = ((dy.atan2(dx).to_degrees() / 90.0).round() * 90.0).rem_euclid(360.0);
    let number = if at(4, 4).is_empty() { at(0, 3) } else { at(4, 4) };
    let name = at(3, 4);
    let kind = match (at(0, 2), passive) {
        (_, true) => "passive",
        ("1", _) => "input",
        ("2", _) => "output",
        ("3", _) => "bidirectional",
        ("4", _) => "power_in",
        _ => "unspecified",
    };
    let shape = match (at(5, 0) == "1", at(6, 0) == "1") {
        (true, true) => "inverted_clock",
        (true, false) => "inverted",
        (false, true) => "clock",
        _ => "line",
    };
    let p = ctx.pt(conn);
    ctx.pt(end);
    // Hide a name that only repeats the number (EasyEDA names resistor pins "1", "2").
    let hide_name = at(3, 0) != "1" || name == number;
    let font = "(font (size 1.27 1.27))";
    format!(
        "(pin {kind} {shape} (at {} {} {}) (length {}) (name {:?} (effects {font}{})) (number {:?} (effects {font})))\n",
        fmt(p.0),
        fmt(p.1),
        fmt(angle),
        fmt(len * MM_PER_UNIT),
        if name.is_empty() { "~" } else { name },
        if hide_name { " hide" } else { "" },
        number,
    )
}

/// `units` are the `dataStr` objects of each unit (one for an ordinary symbol).
pub fn convert(name: &str, prefix: &str, footprint: &str, units: &[&Value]) -> String {
    let mut out = String::new();
    let mut total = 0;
    let mut all: Vec<Unit> = vec![];
    // Few pins → a two-terminal part, which ERC should treat as passive.
    let pin_total: usize = units.iter().map(|u| shapes_of(u).iter().filter(|s| s.starts_with("P~")).count()).sum();
    for u in units {
        let (ox, oy) = (num_of(&u["head"]["x"]), num_of(&u["head"]["y"]));
        let shapes = shapes_of(u);
        // First pass measures the drawing so the second can centre it on the origin, on-grid.
        let probe = convert_unit(&shapes, ox, oy, &mut 0, false);
        let [x0, y0, x1, y1] = probe.bbox.unwrap_or([0.0; 4]);
        let snap = |v: f64| (v / GRID).round() * GRID / MM_PER_UNIT;
        let (ox, oy) = (ox + snap((x0 + x1) / 2.0), oy - snap((y0 + y1) / 2.0));
        all.push(convert_unit(&shapes, ox, oy, &mut total, pin_total <= 2));
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for b in all.iter().filter_map(|u| u.bbox) {
        (x0, y0, x1, y1) = (x0.min(b[0]), y0.min(b[1]), x1.max(b[2]), y1.max(b[3]));
    }
    if x0 > x1 {
        (x0, y0, x1, y1) = (-2.54, -2.54, 2.54, 2.54);
    }
    let prop = |k: &str, v: &str, y: f64, hide: bool| {
        format!("(property {k:?} {v:?} (at {} {} 0) (effects (font (size 1.27 1.27)){}))\n", fmt((x0 + x1) / 2.0), fmt(y), if hide { " hide" } else { "" })
    };
    let _ = writeln!(out, "(kicad_symbol_lib (version 20231120) (generator \"klm\")");
    let _ = writeln!(out, "(symbol {name:?} (in_bom yes) (on_board yes)");
    out += &prop("Reference", prefix, y1 + GRID, false);
    out += &prop("Value", name, y0 - GRID, false);
    out += &prop("Footprint", footprint, 0.0, true);
    out += &prop("Datasheet", "", 0.0, true);
    for (i, u) in all.iter().enumerate() {
        let _ = writeln!(out, "(symbol \"{name}_{}_1\"", i + 1);
        out += &u.gfx;
        out += &u.pins;
        out += ")\n";
    }
    out += ")\n)\n";
    out
}

fn shapes_of(u: &Value) -> Vec<&str> {
    u["shape"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

fn num_of(v: &Value) -> f64 {
    v.as_f64().or_else(|| v.as_str().map(num)).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr;
    use serde_json::json;

    #[test]
    fn pin_geometry_and_orientation() {
        // Pin on the right of a body at x=30 (connects at x=40, line goes left), and one on top.
        let ds = json!({"head": {"x": 20, "y": 0}, "shape": [
            "P~show~1~2~40~0~0~r2~0^^40~0^^M 40 -0 h-10~#000^^0~25~3~0~OUT~end~~~#000^^1~35~-1~0~2~start~~~#000^^0~33~0^^0~M 30 -3",
            "P~show~3~1~0~-30~90~r3~0^^0~-30^^M 0 -30 v 10~#000^^1~0~0~0~CLK~end~~~#000^^1~0~0~0~1~start~~~#000^^1~0~0^^1~M 0 0",
            "P~show~0~3~0~30~270~r5~0^^0~30^^M 0 30 v-10~#000^^0~0~0~0~X~end~~~#000^^1~0~0~0~3~start~~~#000^^0~0~0^^0~M 0 0",
            "R~-10~-20~~~40~40~#880000~1~0~none~r4~0",
        ]});
        let sym = convert("TEST", "U", "FP", &[&ds]);
        let lib = sexpr::parse(&sym).unwrap();
        let unit = lib.child("symbol").unwrap().child("symbol").unwrap();
        assert_eq!(unit.arg(0), Some("TEST_1_1"));
        let pins: Vec<_> = unit.children("pin").collect();
        assert_eq!(pins.len(), 3);
        let at = |p: &sexpr::Sexpr| {
            let a = p.child("at").unwrap();
            (a.arg(0).unwrap().parse::<f64>().unwrap(), a.arg(1).unwrap().parse::<f64>().unwrap(), a.arg(2).unwrap().to_string())
        };
        let (right, top) = (at(pins[0]), at(pins[1]));
        assert_eq!(right.2, "180", "pin on the right points left into the body");
        assert_eq!(top.2, "270", "pin on top points down into the body");
        assert_eq!(pins[0].child("length").unwrap().arg(0), Some("2.54"));
        assert_eq!(pins[0].arg(0), Some("input"));
        assert_eq!(pins[1].arg(0), Some("bidirectional"));
        assert_eq!(pins[1].arg(1), Some("inverted_clock"));
        assert!(top.1 > right.1, "EasyEDA y is down, KiCad y is up");
        assert!((right.0 - top.0 - 10.16).abs() < 1e-6, "40 units = 10.16 mm: {right:?} {top:?}");
    }

    #[test]
    fn two_pin_part_is_passive_and_multi_unit_splits() {
        let pin =
            |n: &str, x: i32| format!("P~show~1~{n}~{x}~0~0~r~0^^{x}~0^^M {x} 0 h-10~#000^^0~0~0~0~{n}~end~~~#000^^1~0~0~0~{n}~end~~~#000^^0~0~0^^0~M 0 0");
        let one = json!({"head": {"x": 0, "y": 0}, "shape": [pin("1", 0), pin("2", 40)]});
        let sym = convert("R", "R", "", &[&one]);
        assert_eq!(sexpr::parse(&sym).unwrap().child("symbol").unwrap().child("symbol").unwrap().child("pin").unwrap().arg(0), Some("passive"));

        let a = json!({"head": {"x": 0, "y": 0}, "shape": [pin("1", 0), pin("2", 40), pin("3", 80)]});
        let b = json!({"head": {"x": 0, "y": 0}, "shape": [pin("4", 0), pin("5", 40), pin("6", 80)]});
        let sym = convert("OP", "U", "", &[&a, &b]);
        let units: Vec<_> = sexpr::parse(&sym).unwrap().child("symbol").unwrap().children("symbol").map(|u| u.arg(0).unwrap().to_string()).collect();
        assert_eq!(units, ["OP_1_1", "OP_2_1"]);
    }
}
