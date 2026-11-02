//! EasyEDA PCB footprint → KiCad `.kicad_mod`.

use super::geom::*;
use serde_json::Value;
use std::fmt::Write;

pub struct Footprint {
    pub text: String,
    /// EasyEDA's id of the footprint's 3D model, if it has one.
    pub model_uuid: Option<String>,
}

struct Ctx {
    ox: f64,
    oy: f64,
    /// min x, min y, max x, max y of everything drawn, in mm
    bbox: Option<[f64; 4]>,
    /// the same for pads alone, which size the courtyard
    pad_bbox: Option<[f64; 4]>,
}

fn grow(b: &mut Option<[f64; 4]>, x0: f64, y0: f64, x1: f64, y1: f64) {
    let v = b.get_or_insert([x0, y0, x1, y1]);
    *v = [v[0].min(x0), v[1].min(y0), v[2].max(x1), v[3].max(y1)];
}

impl Ctx {
    /// Footprints are y-down in both programs, so only the origin and unit change.
    fn pt(&mut self, p: Pt) -> Pt {
        let q = ((p.0 - self.ox) * MM_PER_UNIT, (p.1 - self.oy) * MM_PER_UNIT);
        grow(&mut self.bbox, q.0, q.1, q.0, q.1);
        q
    }
}

fn layer_name(id: &str) -> Option<&'static str> {
    Some(match id {
        "1" => "F.Cu",
        "2" => "B.Cu",
        "3" => "F.SilkS",
        "4" => "B.SilkS",
        "5" => "F.Paste",
        "6" => "B.Paste",
        "7" => "F.Mask",
        "8" => "B.Mask",
        "10" => "Edge.Cuts",
        "12" => "Dwgs.User",
        "13" | "99" | "100" | "101" => "F.Fab",
        "14" => "B.Fab",
        "15" => "Cmts.User",
        _ => return None,
    })
}

fn stroke(w_mm: f64) -> String {
    format!("(stroke (width {}) (type solid))", fmt(if w_mm > 0.0 { w_mm } else { 0.1 }))
}

fn pad(ctx: &mut Ctx, f: &[&str]) -> Option<String> {
    let g = |i: usize| f.get(i).copied().unwrap_or("");
    let (c, w, h) = ((num(g(2)), num(g(3))), num(g(4)) * MM_PER_UNIT, num(g(5)) * MM_PER_UNIT);
    let pos = ctx.pt(c);
    let rot = num(g(11));
    let (ew, eh) = if (rot.rem_euclid(180.0) - 90.0).abs() < 1.0 { (h, w) } else { (w, h) };
    let reach = if rot.rem_euclid(90.0) == 0.0 { (ew / 2.0, eh / 2.0) } else { (w.max(h) / 2.0, w.max(h) / 2.0) };
    grow(&mut ctx.pad_bbox, pos.0 - reach.0, pos.1 - reach.1, pos.0 + reach.0, pos.1 + reach.1);

    let drill_d = num(g(9)) * 2.0 * MM_PER_UNIT;
    let drilled = drill_d >= 0.05; // C9900 and friends carry a 0.001 "hole" on SMD pads
    let (kind, layers) = match (g(6), drilled) {
        (_, true) if g(15) == "N" => ("np_thru_hole", "\"*.Cu\" \"*.Mask\""),
        (_, true) => ("thru_hole", "\"*.Cu\" \"*.Mask\""),
        ("2", _) => ("smd", "\"B.Cu\" \"B.Paste\" \"B.Mask\""),
        _ => ("smd", "\"F.Cu\" \"F.Paste\" \"F.Mask\""),
    };
    let number = if kind == "np_thru_hole" { "" } else { g(8) };
    let shape = match g(1) {
        "RECT" => "rect",
        "POLYGON" => "custom",
        _ if (w - h).abs() < 0.001 => "circle",
        _ => "oval",
    };
    let angle = if shape == "custom" { 0.0 } else { kicad_angle(rot) };
    let at = if angle == 0.0 { format!("(at {} {})", fmt(pos.0), fmt(pos.1)) } else { format!("(at {} {} {})", fmt(pos.0), fmt(pos.1), fmt(angle)) };

    let mut s = format!("(pad {number:?} {kind} {shape} {at} ");
    if shape == "custom" {
        let outline: Vec<String> = points(g(10)).into_iter().map(|p| {
            let q = ctx.pt(p);
            format!("(xy {} {})", fmt(q.0 - pos.0), fmt(q.1 - pos.1))
        }).collect();
        if outline.len() < 3 {
            return None;
        }
        let _ = write!(s, "(size 0.1 0.1) (layers {layers}) (options (clearance outline) (anchor circle)) (primitives (gr_poly (pts {}) (width 0) (fill yes)))", outline.join(" "));
    } else {
        let _ = write!(s, "(size {} {}) (layers {layers})", fmt(w), fmt(h));
        if drilled {
            let slot = num(g(13)) * MM_PER_UNIT;
            if slot > drill_d + 0.001 {
                // The slot's axis is only given as two points in board space; undo the pad rotation to get it in pad space.
                let ends = points(g(14));
                let vertical_in_board = ends.len() == 2 && (ends[1].1 - ends[0].1).abs() > (ends[1].0 - ends[0].0).abs();
                let vertical = vertical_in_board != ((angle / 90.0).round() as i64 % 2 != 0);
                let (dx, dy) = if vertical { (drill_d, slot) } else { (slot, drill_d) };
                let _ = write!(s, " (drill oval {} {})", fmt(dx), fmt(dy));
            } else {
                let _ = write!(s, " (drill {})", fmt(drill_d));
            }
        }
    }
    s += ")\n";
    Some(s)
}

/// Shapes that are drawn lines/arcs/polygons in the footprint's graphic layers.
fn graphics(ctx: &mut Ctx, f: &[&str]) -> String {
    let g = |i: usize| f.get(i).copied().unwrap_or("");
    let mut out = String::new();
    match f[0] {
        "TRACK" => {
            if let Some(layer) = layer_name(g(2)) {
                let st = stroke(num(g(1)) * MM_PER_UNIT);
                let pts = points(g(4));
                for pair in pts.windows(2) {
                    let (a, b) = (ctx.pt(pair[0]), ctx.pt(pair[1]));
                    let _ = writeln!(out, "(fp_line (start {} {}) (end {} {}) {st} (layer {layer:?}))", fmt(a.0), fmt(a.1), fmt(b.0), fmt(b.1));
                }
            }
        }
        "CIRCLE" => {
            if let Some(layer) = layer_name(g(5)) {
                let (c, r) = ((num(g(1)), num(g(2))), num(g(3)));
                let (m, e) = (ctx.pt(c), ctx.pt((c.0 + r, c.1)));
                let _ = writeln!(out, "(fp_circle (center {} {}) (end {} {}) {} (fill none) (layer {layer:?}))", fmt(m.0), fmt(m.1), fmt(e.0), fmt(e.1), stroke(num(g(4)) * MM_PER_UNIT));
            }
        }
        "ARC" => {
            if let Some(layer) = layer_name(g(2)) {
                for sub in parse_path(g(4)) {
                    if let Some(Seg::Arc(a)) = sub.segs.first() {
                        let (_, mid) = arc_centre_mid(sub.start, a);
                        let (p1, pm, p2) = (ctx.pt(sub.start), ctx.pt(mid), ctx.pt(a.to));
                        let _ = writeln!(out, "(fp_arc (start {} {}) (mid {} {}) (end {} {}) {} (layer {layer:?}))", fmt(p1.0), fmt(p1.1), fmt(pm.0), fmt(pm.1), fmt(p2.0), fmt(p2.1), stroke(num(g(1)) * MM_PER_UNIT));
                    }
                }
            }
        }
        "RECT" => {
            if let Some(layer) = layer_name(g(5)) {
                let (x, y) = (num(g(1)), num(g(2)));
                let (a, b) = (ctx.pt((x, y)), ctx.pt((x + num(g(3)), y + num(g(4)))));
                let _ = writeln!(out, "(fp_rect (start {} {}) (end {} {}) {} (fill none) (layer {layer:?}))", fmt(a.0), fmt(a.1), fmt(b.0), fmt(b.1), stroke(num(g(8)) * MM_PER_UNIT));
            }
        }
        "SOLIDREGION" => {
            // Cut-outs inside copper pours have no footprint equivalent.
            if let (Some(layer), "solid" | "npth") = (layer_name(g(1)), g(4)) {
                let (layer, filled) = if g(4) == "npth" { ("Edge.Cuts", false) } else { (layer, matches!(layer, "F.SilkS" | "B.SilkS" | "F.Cu" | "B.Cu" | "F.Mask" | "B.Mask" | "F.Paste" | "B.Paste")) };
                for sub in parse_path(g(3)) {
                    let pts: Vec<String> = flatten(&sub).into_iter().map(|p| {
                        let q = ctx.pt(p);
                        format!("(xy {} {})", fmt(q.0), fmt(q.1))
                    }).collect();
                    if pts.len() >= 3 {
                        let _ = writeln!(out, "(fp_poly (pts {}) {} (fill {}) (layer {layer:?}))", pts.join(" "), stroke(0.0), if filled { "yes" } else { "no" });
                    }
                }
            }
        }
        "HOLE" | "VIA" => {
            // HOLE~x~y~radius ; VIA~x~y~diameter~net~holeRadius
            let (p, d) = (ctx.pt((num(g(1)), num(g(2)))), if f[0] == "HOLE" { num(g(3)) * 2.0 } else { num(g(5)) * 2.0 } * MM_PER_UNIT);
            let (kind, pad_d, number) = if f[0] == "HOLE" { ("np_thru_hole", d, "") } else { ("thru_hole", num(g(3)) * MM_PER_UNIT, "") };
            let _ = writeln!(out, "(pad {number:?} {kind} circle (at {} {}) (size {} {}) (drill {}) (layers \"*.Cu\" \"*.Mask\"))", fmt(p.0), fmt(p.1), fmt(pad_d), fmt(pad_d), fmt(d));
        }
        "TEXT" => {
            // Types "P" and "N" are the reference and name; KiCad has its own.
            if let (Some(layer), true) = (layer_name(g(7)), g(1) == "L" && !g(10).is_empty()) {
                let p = ctx.pt((num(g(2)), num(g(3))));
                let size = (num(g(9)) * MM_PER_UNIT).clamp(0.5, 2.0);
                let _ = writeln!(out, "(fp_text user {:?} (at {} {} {}) (layer {layer:?}) (effects (font (size {} {}) (thickness 0.15))))", g(10), fmt(p.0), fmt(p.1), fmt(kicad_angle(num(g(5)))), fmt(size), fmt(size));
            }
        }
        _ => {}
    }
    out
}

/// 3D model placement: the SVGNODE entry whose `c_etype` is `outline3D`. Only its rotation is
/// used (parts whose STEP origin is not the footprint centre, e.g. connectors modelled from pin 1, end up displaced): EasyEDA's STEP files come already positioned relative to the footprint origin (applying
/// `c_origin` and `z` as offsets visibly displaces the body, checked against KiCad's 3D render).
fn model(ds: &Value) -> Option<(String, String)> {
    let shapes = ds["shape"].as_array()?;
    let attrs = shapes.iter().filter_map(|s| s.as_str()?.strip_prefix("SVGNODE~")).filter_map(|j| serde_json::from_str::<Value>(j).ok()).map(|v| v["attrs"].clone()).find(|a| a["c_etype"] == "outline3D")?;
    let uuid = attrs["uuid"].as_str()?.to_string();
    let rot: Vec<f64> = attrs["c_rotation"].as_str().unwrap_or("").split(',').map(num).collect();
    let r = |i: usize| rot.get(i).copied().unwrap_or(0.0);
    // Z turns the opposite way in KiCad (checked on SO-8, SOT-23 and USB-C); X is taken as given (checked on one JST XH).
    let text = format!("(model \"model.step\" (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz {} {} {})))
", fmt(r(0)), fmt(r(1)), fmt(-r(2)));
    Some((uuid, text))
}

/// What KiCad's footprint chooser shows next to the drawing.
pub struct Docs<'a> {
    pub description: &'a str,
    pub tags: &'a str,
    pub datasheet: &'a str,
}

pub fn convert(name: &str, docs: &Docs, ds: &Value) -> Footprint {
    let num_of = |v: &Value| v.as_f64().or_else(|| v.as_str().map(num)).unwrap_or(0.0);
    let mut ctx = Ctx { ox: num_of(&ds["head"]["x"]), oy: num_of(&ds["head"]["y"]), bbox: None, pad_bbox: None };
    let (mut pads, mut gfx) = (String::new(), String::new());
    let mut through_hole = false;
    for s in ds["shape"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let f: Vec<&str> = s.split('~').collect();
        match f[0] {
            "PAD" => {
                if let Some(p) = pad(&mut ctx, &f) {
                    through_hole |= p.contains(" thru_hole ");
                    pads += &p;
                }
            }
            "HOLE" | "VIA" => {
                through_hole |= f[0] == "VIA";
                pads += &graphics(&mut ctx, &f);
            }
            _ => gfx += &graphics(&mut ctx, &f),
        }
    }
    let m = model(ds);
    let [_, y0, _, y1] = ctx.bbox.unwrap_or([-1.0, -1.0, 1.0, 1.0]);
    let mut out = String::new();
    let _ = writeln!(out, "(footprint {name:?} (version 20240108) (generator \"klm\") (generator_version \"1.0\") (layer \"F.Cu\")");
    let _ = writeln!(out, "(descr {:?})
(tags {:?})", docs.description, docs.tags);
    let text = |k: &str, v: &str, y: f64, layer: &str| format!("(property {k:?} {v:?} (at 0 {}) (layer {layer:?}) (effects (font (size 1 1) (thickness 0.15))))\n", fmt(y));
    out += &text("Reference", "REF**", y0 - 1.0, "F.SilkS");
    out += &text("Value", name, y1 + 1.0, "F.Fab");
    let _ = writeln!(out, "(property \"Datasheet\" {:?} (at 0 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1) (thickness 0.15))))", docs.datasheet);
    let _ = writeln!(out, "(attr {})", if through_hole { "through_hole" } else { "smd" });
    out += &gfx;
    if let Some([px0, py0, px1, py1]) = ctx.pad_bbox {
        const MARGIN: f64 = 0.25;
        let _ = writeln!(out, "(fp_rect (start {} {}) (end {} {}) (stroke (width 0.05) (type solid)) (fill none) (layer \"F.CrtYd\"))", fmt(px0 - MARGIN), fmt(py0 - MARGIN), fmt(px1 + MARGIN), fmt(py1 + MARGIN));
    }
    out += &pads;
    let model_uuid = m.map(|(uuid, text)| {
        out += &text;
        uuid
    });
    out += ")\n";
    Footprint { text: out, model_uuid }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr;
    use serde_json::json;

    fn fp(shapes: &[&str]) -> sexpr::Sexpr {
        let ds = json!({"head": {"x": "4000", "y": "3000"}, "shape": shapes});
        sexpr::parse(&convert("T", &Docs { description: "", tags: "", datasheet: "" }, &ds).text).unwrap()
    }

    fn atts(x: &sexpr::Sexpr, head: &str) -> Vec<String> {
        x.child(head).unwrap().items()[1..].iter().map(|v| v.text().unwrap_or("?").to_string()).collect()
    }

    #[test]
    fn smd_pad_is_centred_on_the_origin_unit_converted_and_rotation_negated() {
        let x = fp(&["PAD~RECT~4010~3020~10~4~1~~7~0~~90~g~0~~Y~0~0~0.2~4010,3020"]);
        let pad = x.child("pad").unwrap();
        assert_eq!((pad.arg(0), pad.arg(1), pad.arg(2)), (Some("7"), Some("smd"), Some("rect")));
        assert_eq!(atts(pad, "at"), ["2.54", "5.08", "270"], "EasyEDA turns clockwise, KiCad counter-clockwise");
        assert_eq!(atts(pad, "size"), ["2.54", "1.016"]);
        assert_eq!(atts(pad, "layers"), ["F.Cu", "F.Paste", "F.Mask"]);
    }

    #[test]
    fn through_hole_slot_follows_the_hole_points() {
        // Round pad with a drill, then a vertical slot (hole points differ in y) on an unrotated oval pad.
        let x = fp(&[
            "PAD~ELLIPSE~4000~3000~6~6~11~~1~2~~0~g~0~~Y~0~0~0.2~4000,3000",
            "PAD~OVAL~4010~3000~4.7244~7.0866~11~~2~1.5748~~0~g~4.7244~4017 3007.9 4017 3006.4~Y~0~0~0.2~x",
            "PAD~OVAL~4020~3000~4~7~11~~3~1~~0~g~0~~N~0~0~0.2~x",
        ]);
        let pads: Vec<_> = x.children("pad").collect();
        assert_eq!((pads[0].arg(1), pads[0].arg(2)), (Some("thru_hole"), Some("circle")));
        assert_eq!(atts(pads[0], "drill"), ["1.016"]);
        assert_eq!(atts(pads[1], "drill"), ["oval", "0.8", "1.2"]);
        assert_eq!(pads[2].arg(1), Some("np_thru_hole"));
        assert_eq!(pads[2].arg(0), Some(""), "unplated holes carry no pad number");
    }

    #[test]
    fn chooser_documentation_is_written() {
        let ds = json!({"head": {"x": 0, "y": 0}, "shape": []});
        let x = sexpr::parse(&convert("T", &Docs { description: "A part", tags: "a b", datasheet: "https://x/y.pdf" }, &ds).text).unwrap();
        assert_eq!(x.child("descr").unwrap().arg(0), Some("A part"));
        assert_eq!(x.child("tags").unwrap().arg(0), Some("a b"));
        let ds = x.children("property").find(|p| p.arg(0) == Some("Datasheet")).unwrap();
        assert_eq!(ds.arg(1), Some("https://x/y.pdf"));
    }

    #[test]
    fn model_z_rotation_is_negated_and_offsets_ignored() {
        let node = r#"SVGNODE~{"attrs":{"c_etype":"outline3D","uuid":"abc123","c_rotation":"270,0,270","c_origin":"4006.5,3011.2","z":"-11.8"}}"#;
        let ds = json!({"head": {"x": 4000, "y": 3000}, "shape": [node]});
        let f = convert("T", &Docs { description: "", tags: "", datasheet: "" }, &ds);
        assert_eq!(f.model_uuid.as_deref(), Some("abc123"));
        let x = sexpr::parse(&f.text).unwrap();
        let m = x.child("model").unwrap();
        assert_eq!(atts(m.child("offset").unwrap(), "xyz"), ["0", "0", "0"]);
        assert_eq!(atts(m.child("rotate").unwrap(), "xyz"), ["270", "0", "-270"]);
    }

    #[test]
    fn graphics_map_layers_arcs_and_regions() {
        let x = fp(&[
            "TRACK~0.6~3~~4000 3000 4010 3000 4010 3010~g~0",
            "ARC~1~3~~M4010,3000 A10,10 0 0 1 4000,3010~~g~0",
            "SOLIDREGION~99~~M4000 3000 L4010 3000 L4010 3010 Z~solid~g~~~~0",
            "SOLIDREGION~3~~M4000 3000 L4010 3000 L4010 3010 Z~cutout~g~~~~0",
            "TEXT~N~4000~3000~0.6~0~0~3~~6~name~M0 0~1~g~0",
            "RECT~3913.187~2949.6043~55.118~19.094~3~gge270~0~1~none~~~",
        ]);
        let rect = x.child("fp_rect").unwrap();
        assert_eq!(atts(rect, "layer"), ["F.SilkS"]);
        assert_eq!(atts(rect.child("stroke").unwrap(), "width"), ["0.254"]);
        assert_eq!(x.children("fp_line").count(), 2);
        let arc = x.child("fp_arc").unwrap();
        assert_eq!(atts(arc, "start"), ["2.54", "0"]);
        assert_eq!(atts(arc, "end"), ["0", "2.54"]);
        let mid = atts(arc, "mid");
        assert!(mid[0].parse::<f64>().unwrap() > 0.7 && mid[1].parse::<f64>().unwrap() > 0.7, "bulges away from the chord's centre: {mid:?}");
        assert_eq!(x.children("fp_poly").count(), 1, "cut-out regions and N/P text are dropped");
        assert_eq!(atts(x.child("fp_line").unwrap(), "layer"), ["F.SilkS"]);
        assert!(x.child("fp_text").is_none());
    }
}
