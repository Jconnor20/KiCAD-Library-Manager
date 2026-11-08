//! Does the footprint's 3D model sit on its outline? The model's bounding box (read from the STEP or WRL
//! point data) is put through the footprint's offset, scale and rotate and compared with the fab outline
//! and pads. [`check`] reports; [`fix`] centres the model and turns it by 90° when that is what fits.
//!
//! The box is approximate. STEP: every `CARTESIAN_POINT` except the placements of axes, plus the extent of
//! each `CIRCLE`; points are taken as they are, so a model built from transformed sub-assemblies, or any
//! `Transform` node in a WRL, is not followed. Files that can't be read are skipped (`check_model` says why).
// shortcut: .stpz/.wrz (gzip) are skipped, since klm-core has no flate2 dependency; add it if they matter.

use crate::checks::{Issue, Severity, numbered_pads, on_layer, points};
use crate::convert::easyeda::geom::fmt;
use crate::fab::{self, Rect};
use crate::library::LibraryRoot;
use crate::sexpr::Sexpr;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Centre further than this from the outline's centre is off-centre. 0.25 mm is a visible shift on a
/// 0402 and about what the 4-decimal offsets and a drawn-by-eye outline differ by.
const CENTRE_TOL: f64 = 0.25;
/// Slack on the size window. Models include leads that fab outlines leave out, so a model is allowed to be
/// as large as the outline plus the pads, and as small as the outline, each within 25%.
const SIZE_TOL: f64 = 0.25;
/// Larger files are skipped.
const MAX_BYTES: u64 = 64 << 20;

type Pt = (f64, f64);
type Box3 = ([f64; 3], [f64; 3]);

/// A warning when the footprint's first model is off-centre, rotated 90° or the wrong size for it.
pub(crate) fn check(root: &LibraryRoot, fp: &Sexpr) -> Option<Issue> {
    let a = analyse(root, fp)?;
    let mut parts = vec![];
    if a.rotated {
        parts.push(format!("looks turned by 90° (it is {} × {} mm, the outline {} × {} mm)", fmt(a.size.0), fmt(a.size.1), fmt(a.outline.0), fmt(a.outline.1)));
    } else if !a.size_ok {
        parts.push(format!("is {} × {} mm, which doesn't match the outline ({} × {} mm)", fmt(a.size.0), fmt(a.size.1), fmt(a.outline.0), fmt(a.outline.1)));
    }
    if a.off > CENTRE_TOL {
        parts.push(format!("is {} mm off the outline's centre", fmt(a.off)));
    }
    (!parts.is_empty()).then(|| Issue { severity: Severity::Warning, message: format!("3D model {}", parts.join(" and ")) })
}

/// Centres the footprint's first model on its outline and turns it by 90° when that fits better. Does
/// nothing when the size is wrong, since that is a different model and not a placement slip.
pub(crate) fn fix(root: &LibraryRoot, fp: &mut Sexpr) -> Option<String> {
    let a = analyse(root, fp)?;
    if !a.size_ok_after || (!a.rotated && a.off <= CENTRE_TOL) {
        return None;
    }
    let model = fp.items_mut().iter_mut().find(|c| c.head() == Some("model"))?;
    set_xyz(model, "offset", [a.new_offset.0, a.new_offset.1, a.offset_z]);
    set_xyz(model, "rotate", [a.rot[0], a.rot[1], a.new_rz]);
    let turn = if a.rotated { " and turned 90°" } else { "" };
    Some(format!("3D model centred on the outline{turn} (offset {}, {} mm)", fmt(a.new_offset.0), fmt(a.new_offset.1)))
}

struct Analysis {
    size: (f64, f64),
    outline: (f64, f64),
    /// The size fits the outline window as it stands.
    size_ok: bool,
    /// ...and after the turn or centring [`fix`] would make.
    size_ok_after: bool,
    rotated: bool,
    off: f64,
    new_offset: (f64, f64),
    offset_z: f64,
    rot: [f64; 3],
    new_rz: f64,
}

fn analyse(root: &LibraryRoot, fp: &Sexpr) -> Option<Analysis> {
    let model = fp.child("model")?;
    let path = resolve(root, model.arg(0)?)?;
    let raw = read_box(&path)?;
    let offset = xyz(model, "offset", 0.0);
    let scale = xyz(model, "scale", 1.0);
    let rot = xyz(model, "rotate", 0.0);
    let (lo, hi, centre) = target(fp)?;

    // The box in the footprint's frame before the offset; y is flipped last.
    let at = |rz: f64| {
        let b = transform(raw, scale, [rot[0], rot[1], rz]);
        ((b.1[0] - b.0[0], b.1[1] - b.0[1]), ((b.0[0] + b.1[0]) / 2.0, (b.0[1] + b.1[1]) / 2.0))
    };
    let ok = |s: (f64, f64)| fits(s.0, lo.0, hi.0) && fits(s.1, lo.1, hi.1);
    let (size, mid) = at(rot[2]);
    let (turned, turned_mid) = at(rot[2] + 90.0);
    let rotated = !ok(size) && ok(turned);
    let (new_rz, size_after, mid_after) = if rotated { (norm(rot[2] + 90.0), turned, turned_mid) } else { (rot[2], size, mid) };
    // The outline's centre in the model's frame, whose y points up.
    let want = (centre.0, -centre.1);
    let now = (mid.0 + offset[0], mid.1 + offset[1]);
    Some(Analysis {
        size,
        outline: lo,
        size_ok: ok(size),
        size_ok_after: ok(size_after),
        rotated,
        off: (now.0 - want.0).hypot(now.1 - want.1),
        new_offset: (want.0 - mid_after.0, want.1 - mid_after.1),
        offset_z: offset[2],
        rot,
        new_rz,
    })
}

fn fits(v: f64, lo: f64, hi: f64) -> bool {
    v >= lo * (1.0 - SIZE_TOL) && v <= hi * (1.0 + SIZE_TOL)
}

fn norm(deg: f64) -> f64 {
    let d = deg.rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// What the model should cover: (the outline's size, the largest it may be, the outline's centre).
/// The fab layer's box is the outline and the fab layer plus pads is the largest; without a fab layer
/// the pads are the largest and half of them the smallest.
fn target(fp: &Sexpr) -> Option<(Pt, Pt, Pt)> {
    let mut pts = vec![];
    for g in fp.items().iter().filter(|g| fab::is_graphic(g) && on_layer(g, &["F.Fab", "B.Fab"])) {
        points(g, &mut pts);
    }
    let pads: Vec<_> = numbered_pads(fp).iter().map(|p| fab::shape_bbox(&fab::shape(p))).collect();
    let pad_box = bbox_of(&pads.iter().flat_map(|r| [(r.0, r.1), (r.2, r.3)]).collect::<Vec<_>>());
    let dims = |r: Rect| (r.2 - r.0, r.3 - r.1);
    let centre = |r: Rect| ((r.0 + r.2) / 2.0, (r.1 + r.3) / 2.0);
    match (fab::bbox(&pts), pad_box) {
        (Some(f), p) => {
            let all = p.map_or(f, |p| (f.0.min(p.0), f.1.min(p.1), f.2.max(p.2), f.3.max(p.3)));
            Some((dims(f), dims(all), centre(f))).filter(|t| t.0.0 > 0.0 && t.0.1 > 0.0)
        }
        (None, Some(p)) => Some(((dims(p).0 / 2.0, dims(p).1 / 2.0), dims(p), centre(p))),
        _ => None,
    }
}

fn bbox_of(pts: &[(f64, f64)]) -> Option<Rect> {
    fab::bbox(pts)
}

/// The box after scale, then rotation (x, then y, then z), as KiCad's `CalcModelMatrix` does it: each angle
/// is negated, so a positive angle turns clockwise seen from the positive axis.
fn transform(b: Box3, scale: [f64; 3], rot: [f64; 3]) -> Box3 {
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for i in 0..8 {
        let mut p = [0, 1, 2].map(|k| if i >> k & 1 == 1 { b.1[k] } else { b.0[k] } * scale[k]);
        for (axis, deg) in rot.iter().enumerate() {
            let (s, c) = (-deg).to_radians().sin_cos();
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            (p[u], p[v]) = (p[u] * c - p[v] * s, p[u] * s + p[v] * c);
        }
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn xyz(model: &Sexpr, key: &str, default: f64) -> [f64; 3] {
    let v = model.child(key).and_then(|k| k.child("xyz"));
    [0, 1, 2].map(|i| v.and_then(|v| v.arg(i)?.parse().ok()).unwrap_or(default))
}

fn set_xyz(model: &mut Sexpr, key: &str, v: [f64; 3]) {
    let atoms = v.iter().map(|n| Sexpr::Atom(fmt(*n))).collect();
    let new = Sexpr::list(key, vec![Sexpr::list("xyz", atoms)]);
    match model.items_mut().iter_mut().find(|c| c.head() == Some(key)) {
        Some(old) => *old = new,
        None => model.items_mut().push(new),
    }
}

fn resolve(root: &LibraryRoot, uri: &str) -> Option<PathBuf> {
    let path = uri.replace("${KIPRJMOD}", &root.table_dir.to_string_lossy());
    (!path.starts_with("http") && !path.contains("${")).then(|| PathBuf::from(path))
}

/// The model's box in mm, from the file's point data.
fn read_box(path: &Path) -> Option<Box3> {
    if std::fs::metadata(path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(&std::fs::read(path).ok()?).into_owned();
    match path.extension()?.to_str()?.to_lowercase().as_str() {
        "step" | "stp" => step_box(&text),
        "wrl" => wrl_box(&text),
        _ => None,
    }
}

fn grow(b: &mut Option<Box3>, p: [f64; 3]) {
    let (lo, hi) = b.get_or_insert((p, p));
    for k in 0..3 {
        lo[k] = lo[k].min(p[k]);
        hi[k] = hi[k].max(p[k]);
    }
}

fn numbers(s: &str) -> Vec<f64> {
    s.split(|c: char| c.is_whitespace() || c == ',').filter_map(|t| t.parse().ok()).collect()
}

/// VRML units are 0.1 inch.
fn wrl_box(text: &str) -> Option<Box3> {
    let mut b = None;
    let mut rest = text;
    while let Some(i) = rest.find("point") {
        rest = &rest[i + 5..];
        let Some(open) = rest.trim_start().strip_prefix('[') else { continue };
        let Some(end) = open.find(']') else { break };
        numbers(&open[..end]).as_chunks::<3>().0.iter().for_each(|c| grow(&mut b, c.map(|v| v * 2.54)));
        rest = &open[end..];
    }
    b
}

fn step_box(text: &str) -> Option<Box3> {
    let unit = if text.contains("'INCH'") || text.contains("'inch'") {
        25.4
    } else if text.contains("SI_UNIT($,.METRE.)") {
        1000.0
    } else {
        1.0 // millimetres, which is what the header nearly always says
    };
    let data = &text[text.find("DATA;")?..];
    // id -> (entity name, text between its outer brackets)
    let mut ents: HashMap<&str, (&str, &str)> = HashMap::new();
    for st in data.split(';') {
        let Some((id, rest)) = st.trim().split_once('=') else { continue };
        let Some((name, args)) = rest.trim().split_once('(') else { continue };
        ents.insert(id.trim(), (name.trim(), args.trim_end().trim_end_matches(')')));
    }
    let vec3 = |id: &str| -> Option<[f64; 3]> {
        let v = numbers(ents.get(id)?.1.rsplit('(').next()?.trim_end_matches(')'));
        Some([*v.first()?, *v.get(1)?, *v.get(2)?])
    };
    let refs = |args: &str| -> Vec<String> { args.split(|c: char| !(c.is_ascii_digit() || c == '#')).filter(|t| t.len() > 1 && t.starts_with('#')).map(String::from).collect() };
    // Axis placements: where each circle is centred, and which way it faces.
    let mut placement: HashMap<String, ([f64; 3], [f64; 3])> = HashMap::new();
    let mut placed = std::collections::HashSet::new();
    for (id, (name, args)) in &ents {
        if *name == "AXIS2_PLACEMENT_3D" {
            let r = refs(args);
            if let (Some(loc), Some(p)) = (r.first(), r.first().and_then(|l| vec3(l))) {
                placed.insert(loc.clone());
                let dir = r.get(1).and_then(|d| vec3(d)).unwrap_or([0.0, 0.0, 1.0]);
                placement.insert(id.to_string(), (p, dir));
            }
        }
    }
    let mut b = None;
    for (id, (name, args)) in &ents {
        match *name {
            "CARTESIAN_POINT" if !placed.contains(*id) => {
                if let Some(p) = vec3(id) {
                    grow(&mut b, p.map(|v| v * unit));
                }
            }
            "CIRCLE" => {
                let radius = numbers(args.rsplit(',').next().unwrap_or_default()).first().copied();
                if let (Some(r), Some((c, d))) = (radius, refs(args).first().and_then(|a| placement.get(a))) {
                    let len = d.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-9);
                    // A circle reaches r * sqrt(1 - d²) along each axis, d being its unit normal.
                    let reach = d.map(|v| r * (1.0 - (v / len).powi(2)).max(0.0).sqrt());
                    grow(&mut b, [0, 1, 2].map(|k| (c[k] - reach[k]) * unit));
                    grow(&mut b, [0, 1, 2].map(|k| (c[k] + reach[k]) * unit));
                }
            }
            _ => {}
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr;

    fn step(lo: [f64; 3], hi: [f64; 3]) -> String {
        // An axis placement at the origin (not geometry) and the 8 corners of the box.
        let mut s = String::from("ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=AXIS2_PLACEMENT_3D('',#1,#90,#91);\n#90=DIRECTION('',(0.,0.,1.));\n#91=DIRECTION('',(1.,0.,0.));\n");
        for i in 0..8 {
            let p = [0, 1, 2].map(|k| if i >> k & 1 == 1 { hi[k] } else { lo[k] });
            s += &format!("#{}=CARTESIAN_POINT('',({:.3},{:.3},{:.3}));\n", 10 + i, p[0], p[1], p[2]);
        }
        s + "ENDSEC;\nEND-ISO-10303-21;\n"
    }

    fn wrl(lo: [f64; 3], hi: [f64; 3]) -> String {
        let (l, h) = (lo.map(|v| v / 2.54), hi.map(|v| v / 2.54));
        format!("#VRML V2.0 utf8\nShape {{ geometry IndexedFaceSet {{ coord Coordinate {{ point [\n{} {} {},\n{} {} {}\n] }} }} }}\n", l[0], l[1], l[2], h[0], h[1], h[2])
    }

    /// A 2 x 1 mm outline on F.Fab centred on the origin, with two pads at +-1.
    fn footprint(model: &str) -> Sexpr {
        sexpr::parse(&format!(
            r#"(footprint "R" (attr smd)
            (fp_rect (start -1 -0.5) (end 1 0.5) (stroke (width 0.1) (type solid)) (layer "F.Fab"))
            (pad "1" smd rect (at -1 0) (size 0.6 1) (layers "F.Cu"))
            (pad "2" smd rect (at 1 0) (size 0.6 1) (layers "F.Cu"))
            (model "${{KIPRJMOD}}/m.step" {model}))"#
        ))
        .unwrap()
    }

    fn run(file: &str, text: &str, model: &str) -> (Option<Issue>, Option<String>, Sexpr) {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("klm_model3d_{}_{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), text).unwrap();
        let root = LibraryRoot { dir: dir.clone(), scope: crate::library::Scope::Project, table_dir: dir.clone() };
        let mut fp = footprint(model);
        let uri = format!("${{KIPRJMOD}}/{file}");
        if let Some(m) = fp.items_mut().iter_mut().find(|c| c.head() == Some("model")) {
            m.set_arg(0, Sexpr::str(&uri));
        }
        let (issue, fixed) = (check(&root, &fp), fix(&root, &mut fp));
        std::fs::remove_dir_all(dir).ok();
        (issue, fixed, fp)
    }

    fn offset(fp: &Sexpr) -> [f64; 3] {
        xyz(fp.child("model").unwrap(), "offset", 0.0)
    }

    #[test]
    fn centred_model_is_fine() {
        let (issue, fixed, _) = run("m.step", &step([-1.0, -0.5, 0.0], [1.0, 0.5, 0.6]), "");
        assert!(issue.is_none() && fixed.is_none(), "{issue:?}");
        let (issue, ..) = run("m.wrl", &wrl([-1.0, -0.5, 0.0], [1.0, 0.5, 0.6]), "");
        assert!(issue.is_none());
    }

    #[test]
    fn offset_model_is_warned_and_centred() {
        // The body sits 1 mm to the right and 0.3 mm below on screen (y up in the model: -0.3).
        for (file, text) in [("m.step", step([0.0, -0.8, 0.0], [2.0, 0.2, 0.6])), ("m.wrl", wrl([0.0, -0.8, 0.0], [2.0, 0.2, 0.6]))] {
            let (issue, fixed, fp) = run(file, &text, "");
            assert!(issue.unwrap().message.contains("off the outline's centre"));
            assert!(fixed.is_some());
            let o = offset(&fp);
            assert!((o[0] + 1.0).abs() < 1e-3 && (o[1] - 0.3).abs() < 1e-3, "{o:?}");
        }
    }

    #[test]
    fn existing_offset_counts_and_the_y_axis_is_flipped() {
        // Centred in the model, offset 0.5 up in the model = 0.5 mm towards the top of the screen's -y.
        let (issue, _, fp) = run("m.step", &step([-1.0, -0.5, 0.0], [1.0, 0.5, 0.6]), "(offset (xyz 0 0.5 0.2))");
        assert!(issue.is_some());
        assert_eq!(offset(&fp), [0.0, 0.0, 0.2]);
    }

    #[test]
    fn model_turned_90_degrees_is_turned_back() {
        let (issue, fixed, fp) = run("m.step", &step([-0.5, -1.0, 0.0], [0.5, 1.0, 0.6]), "");
        assert!(issue.unwrap().message.contains("turned by 90"));
        assert!(fixed.unwrap().contains("turned 90"));
        assert_eq!(xyz(fp.child("model").unwrap(), "rotate", 0.0)[2], 90.0);
        assert_eq!(offset(&fp)[..2], [0.0, 0.0]);
    }

    #[test]
    fn turn_direction_matches_kicad() {
        // 1 x 2 mm with its corner on the origin. KiCad's +90 is clockwise from above: (x, y) -> (y, -x),
        // so the box becomes x 0..2, y -1..0 and the offset that centres it is (-1, 0.5).
        let (_, fixed, fp) = run("m.step", &step([0.0, 0.0, 0.0], [1.0, 2.0, 0.6]), "");
        assert!(fixed.unwrap().contains("turned 90"));
        let o = offset(&fp);
        assert!((o[0] + 1.0).abs() < 1e-3 && (o[1] - 0.5).abs() < 1e-3, "{o:?}");
    }

    #[test]
    fn scale_is_applied() {
        // Half the size in the file, scale 2 in the footprint.
        let (issue, ..) = run("m.step", &step([-0.5, -0.25, 0.0], [0.5, 0.25, 0.3]), "(scale (xyz 2 2 2))");
        assert!(issue.is_none(), "{issue:?}");
        let (issue, ..) = run("m.step", &step([-0.5, -0.25, 0.0], [0.5, 0.25, 0.3]), "");
        assert!(issue.unwrap().message.contains("doesn't match"));
    }

    #[test]
    fn wrong_size_is_warned_but_not_moved() {
        let (issue, fixed, fp) = run("m.step", &step([2.0, 2.0, 0.0], [10.0, 6.0, 1.0]), "");
        assert!(issue.unwrap().message.contains("doesn't match"));
        assert!(fixed.is_none());
        assert_eq!(offset(&fp), [0.0; 3]);
    }

    #[test]
    fn units_and_circles() {
        let inch = "ISO-10303-21;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=AXIS2_PLACEMENT_3D('',#1,#3,#4);\n#3=DIRECTION('',(0.,0.,1.));\n#4=DIRECTION('',(1.,0.,0.));\n#5=CIRCLE('',#2,0.5);\n#6=( NAMED_UNIT(*) SI_UNIT($,.METRE.) LENGTH_UNIT() 'INCH' );\nENDSEC;\n";
        let b = step_box(inch).unwrap();
        assert!((b.1[0] - 12.7).abs() < 1e-9 && b.1[2].abs() < 1e-9, "{b:?}");
    }
}
