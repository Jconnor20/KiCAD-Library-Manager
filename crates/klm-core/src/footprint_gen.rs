//! IPC-7351B land patterns from datasheet dimensions: chip, SOD, SOT-23, SOIC (dual gull-wing), QFN, QFP and BGA.
//!
//! [`generate`] builds the pads, fab outline, silkscreen, courtyard and Reference/Value text as a `.kicad_mod`,
//! then runs the profile's own footprint rules over it (`standardise::fix_footprint`: line widths, silk clipped off
//! pads, courtyard grown to the density's excess, text and pin-1 marker), so it passes the same checks as any
//! standardised footprint.
//!
//! Sources. The standard is paywalled and was not read; every number below comes from secondary sources and is
//! marked with where. Check against your copy of IPC-7351B before trusting a design to it.
//! - Formulas (IPC-7351B §3.1.3, as implemented in kicad-footprint-generator `ipc_pad_size_calculators.py`):
//!   Zmax = Lmin + 2·Jt + √(CL² + F² + P²); Gmin = Smax − 2·Jh − √(CS² + F² + P²); Xmax = Wmin + 2·Js + √(CW² + F² + P²);
//!   Smax = Lmax − 2·Tmin, Smin = Lmin − 2·Tmax, C = max − min of L, S, W.
//! - F (fabrication) and P (placement) tolerances: PCB Libraries' LP Wizard defaults for IPC-7351B, 0.05 and 0.025 mm.
//!   (kicad-footprint-generator falls back to 0.1 / 0.05, the older 7351A values.)
//! - Toe/heel/side goals Jt, Jh, Js per density (Most, Nominal, Least): the tables as reproduced by PCB Libraries'
//!   IPC-7351B summary and Altium's IPC wizard documentation, noted at each table.
//! - Courtyard excess comes from `fab_data` (0.50 / 0.25 / 0.10 mm, IPC-7351B) so the footprint check agrees.

use crate::convert::easyeda::geom::fmt;
use crate::fab_data::{self, FabChoice};
use crate::kicad_env::write_atomic;
use crate::library::{LibraryRoot, sanitize};
use crate::sexpr::{self, Sexpr};
use crate::standardise::{self, FootprintStyle};
use anyhow::{Result, bail};
use std::path::PathBuf;

/// IPC-7351B fabrication and placement tolerances, mm (see the module note).
const F: f64 = 0.05;
const P: f64 = 0.025;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Family {
    Chip,
    Sod,
    Sot,
    Soic,
    Qfn,
    Qfp,
    Bga,
}

impl Family {
    pub const ALL: [Family; 7] = [Family::Chip, Family::Sod, Family::Sot, Family::Soic, Family::Qfn, Family::Qfp, Family::Bga];

    pub fn label(self) -> &'static str {
        match self {
            Family::Chip => "chip",
            Family::Sod => "sod",
            Family::Sot => "sot",
            Family::Soic => "soic",
            Family::Qfn => "qfn",
            Family::Qfp => "qfp",
            Family::Bga => "bga",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.label() == s.to_lowercase())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}

impl Range {
    pub fn new(min: f64, max: f64) -> Self {
        Self { min, max }
    }

    pub fn nom(self) -> f64 {
        (self.min + self.max) / 2.0
    }

    /// "1.6" or "1.55-1.65".
    pub fn parse(s: &str) -> Result<Self> {
        let num = |t: &str| t.trim().parse::<f64>().map_err(|_| anyhow::anyhow!("\"{s}\" is not a number or a min-max range"));
        let r = match s.split_once('-').filter(|(a, _)| !a.is_empty()) {
            Some((a, b)) => Self::new(num(a)?, num(b)?),
            None => Self::new(num(s)?, num(s)?),
        };
        if r.min > r.max {
            bail!("\"{s}\": min is above max");
        }
        Ok(r)
    }
}

/// Datasheet dimensions in mm. Which fields a family reads is in [`Dims::check`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dims {
    /// Pins around the part; QFN without its exposed pad; BGA ignores it (rows × cols).
    pub pins: u32,
    pub pitch: f64,
    /// Overall lead span, tip to tip (chip: overall length), along x. QFN/QFP: also along y unless `span_y`.
    pub span: Range,
    pub span_y: Option<Range>,
    /// Lead (terminal) length T and width W.
    pub lead_len: Range,
    pub lead_w: Range,
    /// Body size for the fab outline; derived from the span and width when absent.
    pub body: Option<(f64, f64)>,
    pub height: f64,
    pub rows: u32,
    pub cols: u32,
    /// BGA ball diameter.
    pub ball: f64,
    /// QFN exposed pad.
    pub thermal: Option<(f64, f64)>,
}

impl Dims {
    fn check(&self, f: Family) -> Result<()> {
        let pos = |ok: bool, what: &str| if ok { Ok(()) } else { bail!("{} needs {what}", f.label()) };
        pos(self.height > 0.0, "a height")?;
        if f == Family::Bga {
            pos(self.rows > 0 && self.cols > 0 && self.pitch > 0.0 && self.ball > 0.0, "rows, cols, pitch and ball diameter")?;
            return pos(self.rows <= 400 && self.cols <= 999, "fewer rows or columns");
        }
        pos(self.span.max > 0.0 && self.lead_len.max > 0.0 && self.lead_w.max > 0.0, "span, lead length and lead width")?;
        match f {
            Family::Chip | Family::Sod => {}
            Family::Sot => {
                pos(matches!(self.pins, 3 | 5 | 6) && self.pitch > 0.0, "3, 5 or 6 pins and a pitch")?;
            }
            Family::Soic => pos(self.pins >= 4 && self.pins % 2 == 0 && self.pitch > 0.0, "an even pin count and a pitch")?,
            _ => pos(self.pins >= 4 && self.pins % 4 == 0 && self.pitch > 0.0, "a pin count divisible by 4 and a pitch")?,
        }
        // The lead span minus the foot length is not the body of a gull-wing part, so the outline needs the real one.
        pos(self.body.is_some() || matches!(f, Family::Chip | Family::Qfn), "the body size")?;
        Ok(())
    }
}

/// Toe, heel and side goals for Most, Nominal, Least density.
type Goals = [(f64, f64, f64); 3];

// Chip resistors and capacitors, IPC-7351B Table 3-? as in PCB Libraries' summary: toe 0.55/0.35/0.15, heel 0,
// side 0.05/0/-0.05. shortcut: the 0402-and-smaller rows (smaller toe) are not modelled; check tiny chips by hand.
const CHIP: Goals = [(0.55, 0.0, 0.05), (0.35, 0.0, 0.0), (0.15, 0.0, -0.05)];
// Gull-wing leads, pitch above 0.625 (SOIC, SOT, SOD): toe 0.55/0.35/0.15, heel 0.45/0.35/0.25, side 0.05/0.03/0.01.
const GULL: Goals = [(0.55, 0.45, 0.05), (0.35, 0.35, 0.03), (0.15, 0.25, 0.01)];
// Gull-wing at pitch 0.625 or less (fine-pitch QFP, SSOP): same toe and heel, side 0.01/-0.02/-0.04 to keep the gaps open
// (kicad-footprint-generator ipc_definitions.yaml, "updated to IPC-7351 B").
const GULL_FINE: Goals = [(0.55, 0.45, 0.01), (0.35, 0.35, -0.02), (0.15, 0.25, -0.04)];
// QFN/DFN with toe fillet: toe 0.40/0.30/0.20, heel 0, side -0.04 (same yaml). shortcut: the pull-back no-lead table
// is not modelled, nor the 0.2 mm minimum gap between exposed pad and signal pads.
const NOLEAD: Goals = [(0.40, 0.0, -0.04), (0.30, 0.0, -0.04), (0.20, 0.0, -0.04)];
// BGA: one land size for all densities, only the courtyard varies. Collapsing balls: about 75% of the ball from 0.6 mm
// up, 80% below (recalled from IPC-7351B summaries, not verified).
fn bga_pad(ball: f64) -> f64 {
    ball * if ball >= 0.6 { 0.75 } else { 0.8 }
}

/// Gull-wing goals for the pitch.
fn gull(pitch: f64) -> &'static Goals {
    if pitch > 0.0 && pitch <= 0.625 { &GULL_FINE } else { &GULL }
}

fn rss(c: f64) -> f64 {
    (c * c + F * F + P * P).sqrt()
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// One row of pads: outer extent `z`, inner gap `g`, width `x`, all from the formulas in the module note.
struct Land {
    z: f64,
    g: f64,
    x: f64,
}

impl Land {
    fn new(span: Range, lead_len: Range, w: Range, (jt, jh, js): (f64, f64, f64)) -> Result<Self> {
        // Z and X use the plain tolerance C = max − min. S depends on two dimensions, so IPC uses the root-sum-square
        // of their tolerances (CS = √(CL² + 2·CT²)) and pulls Smax in by the difference to the worst case, as
        // kicad-footprint-generator's `ipc_pad_size_calculators.py` does.
        let (cl, ct) = (span.max - span.min, lead_len.max - lead_len.min);
        let (cs_worst, cs) = (cl + 2.0 * ct, (cl * cl + 2.0 * ct * ct).sqrt());
        let smax = span.max - 2.0 * lead_len.min - (cs_worst - cs) / 2.0;
        let z = span.min + 2.0 * jt + rss(cl);
        let g = smax - 2.0 * jh - rss(cs);
        let x = w.min + 2.0 * js + rss(w.max - w.min);
        if z <= g || x <= 0.0 {
            bail!("these dimensions give no land (outer {} mm, inner gap {} mm): check the lead span and lead length", fmt(z), fmt(g));
        }
        Ok(Self { z: r2(z), g: r2(g.max(0.0)), x: r2(x) })
    }

    fn len(&self) -> f64 {
        r2((self.z - self.g) / 2.0)
    }

    /// Pad centre's distance from the middle.
    fn centre(&self) -> f64 {
        r2((self.z + self.g) / 4.0)
    }
}

fn atom(s: &str) -> Sexpr {
    Sexpr::Atom(s.into())
}

fn n(v: f64) -> Sexpr {
    atom(&fmt(v))
}

fn xy(head: &str, x: f64, y: f64) -> Sexpr {
    Sexpr::list(head, vec![n(x), n(y)])
}

fn stroke(w: f64) -> Sexpr {
    Sexpr::list("stroke", vec![Sexpr::list("width", vec![n(w)]), Sexpr::list("type", vec![atom("solid")])])
}

fn layer(l: &str) -> Sexpr {
    Sexpr::list("layer", vec![Sexpr::str(l)])
}

fn line(l: &str, a: (f64, f64), b: (f64, f64), w: f64) -> Sexpr {
    Sexpr::list("fp_line", vec![xy("start", a.0, a.1), xy("end", b.0, b.1), stroke(w), layer(l)])
}

fn rect(l: &str, a: (f64, f64), b: (f64, f64), w: f64) -> Sexpr {
    Sexpr::list("fp_rect", vec![xy("start", a.0, a.1), xy("end", b.0, b.1), stroke(w), Sexpr::list("fill", vec![atom("no")]), layer(l)])
}

fn pad(number: &str, at: (f64, f64), size: (f64, f64), round: bool) -> Sexpr {
    let mut v = vec![Sexpr::str(number), atom("smd"), atom(if round { "circle" } else { "roundrect" }), xy("at", at.0, at.1), xy("size", size.0, size.1)];
    v.push(Sexpr::list("layers", ["F.Cu", "F.Paste", "F.Mask"].map(Sexpr::str).to_vec()));
    if !round {
        v.push(Sexpr::list("roundrect_rratio", vec![n(0.25)]));
    }
    Sexpr::list("pad", v)
}

fn text(key: &str, value: &str, y: f64, l: &str) -> Sexpr {
    let font = Sexpr::list("font", vec![xy("size", 1.0, 1.0), Sexpr::list("thickness", vec![n(0.15)])]);
    Sexpr::list("property", vec![Sexpr::str(key), Sexpr::str(value), xy("at", 0.0, y), layer(l), Sexpr::list("effects", vec![font])])
}

/// BGA row letters: JEDEC skips I, O, Q, S, X and Z; past Y the letters double (AA, AB, ...).
fn row_letter(i: u32) -> String {
    const L: &[u8] = b"ABCDEFGHJKLMNPRTUVWY";
    let l = L.len() as u32;
    if i < l { (L[i as usize] as char).to_string() } else { format!("{}{}", L[(i / l - 1) as usize] as char, L[(i % l) as usize] as char) }
}

pub struct Generated {
    /// The IPC-7351 name, also the file name.
    pub name: String,
    pub text: String,
    /// What the profile's rules changed after generation.
    pub fixes: Vec<String>,
}

fn h(v: f64) -> String {
    format!("{}", (v * 100.0).round() as i64)
}

fn suffix(density: char) -> char {
    match density.to_ascii_uppercase() {
        'A' => 'M',
        'C' => 'L',
        _ => 'N',
    }
}

/// Builds the footprint for `family` from `d` at the density in `choice`, finished with `rules` (and the house
/// `style` when there is one). Everything is mm, y pointing down, origin at the centre of the pads.
pub fn generate(family: Family, d: &Dims, choice: &FabChoice, rules: &standardise::Rules, style: Option<&FootprintStyle>) -> Result<Generated> {
    d.check(family)?;
    if !matches!(choice.density.to_ascii_uppercase(), 'A' | 'B' | 'C') {
        bail!("density must be A (most), B (nominal) or C (least)");
    }
    let di = (choice.density.to_ascii_uppercase() as u8 - b'A') as usize;
    let limits = fab_data::limits(choice);
    let sfx = suffix(choice.density);
    // Pads may not be wider than the pitch leaves room for under the fab's copper clearance.
    let cap = |w: f64| match (d.pitch > 0.0, limits.min_clearance) {
        (true, Some(c)) => w.min(r2(d.pitch - c - 0.005)),
        _ => w,
    };
    let (mut pads, mut body) = (vec![], d.body);
    let (name, descr);
    let pitch_code = h(d.pitch);

    match family {
        Family::Chip | Family::Sod | Family::Sot | Family::Soic => {
            let goals = if family == Family::Chip { CHIP[di] } else { gull(d.pitch)[di] };
            let land = Land::new(d.span, d.lead_len, d.lead_w, goals)?;
            let (c, len) = (land.centre(), land.len());
            match family {
                Family::Chip | Family::Sod => {
                    pads = vec![pad("1", (-c, 0.0), (len, land.x), false), pad("2", (c, 0.0), (len, land.x), false)];
                    body = body.or(Some((d.span.nom(), d.lead_w.nom())));
                    // IPC names SOD by lead span and body width, chips by body length and width (= terminal width).
                    let w = if family == Family::Sod { body.map_or(0.0, |b| b.1) } else { d.lead_w.nom() };
                    let code = format!("{:02}{:02}", (d.span.nom() * 10.0).round(), (w * 10.0).round());
                    name = format!("{}{code}X{}{sfx}", if family == Family::Chip { "RESC" } else { "SOD" }, h(d.height));
                    descr = format!("{} land pattern {code}, IPC-7351B", family.label());
                }
                _ => {
                    let w = cap(land.x);
                    let (left, right): (Vec<f64>, Vec<f64>) = if family == Family::Soic {
                        let m = d.pins / 2;
                        let ys: Vec<f64> = (0..m).map(|i| r2((i as f64 - (m - 1) as f64 / 2.0) * d.pitch)).collect();
                        (ys.clone(), ys.into_iter().rev().collect())
                    } else {
                        let p = d.pitch;
                        match d.pins {
                            3 => (vec![-p, p], vec![0.0]),
                            5 => (vec![-p, 0.0, p], vec![p, -p]),
                            _ => (vec![-p, 0.0, p], vec![p, 0.0, -p]),
                        }
                    };
                    for (i, y) in left.iter().enumerate() {
                        pads.push(pad(&(i + 1).to_string(), (-c, *y), (len, w), false));
                    }
                    for (i, y) in right.iter().enumerate() {
                        pads.push(pad(&(left.len() + i + 1).to_string(), (c, *y), (len, w), false));
                    }
                    name = format!("{}{pitch_code}P{}X{}-{}{sfx}", if family == Family::Soic { "SOIC" } else { "SOT" }, h(d.span.nom()), h(d.height), d.pins);
                    descr = format!("{} pin {} land pattern, pitch {} mm, IPC-7351B", d.pins, family.label(), fmt(d.pitch));
                }
            }
        }
        Family::Qfn | Family::Qfp => {
            let goals = if family == Family::Qfn { NOLEAD[di] } else { gull(d.pitch)[di] };
            let sy = d.span_y.unwrap_or(d.span);
            let (lx, ly) = (Land::new(d.span, d.lead_len, d.lead_w, goals)?, Land::new(sy, d.lead_len, d.lead_w, goals)?);
            let (w, m, p) = (cap(lx.x.min(ly.x)), d.pins / 4, d.pitch);
            let at = |i: u32| r2((i as f64 - (m - 1) as f64 / 2.0) * p);
            // Counter-clockwise from the top of the left side.
            for i in 0..m {
                pads.push(pad(&(i + 1).to_string(), (-lx.centre(), at(i)), (lx.len(), w), false));
            }
            for i in 0..m {
                pads.push(pad(&(m + i + 1).to_string(), (at(i), ly.centre()), (w, ly.len()), false));
            }
            for i in 0..m {
                pads.push(pad(&(2 * m + i + 1).to_string(), (lx.centre(), -at(i)), (lx.len(), w), false));
            }
            for i in 0..m {
                pads.push(pad(&(3 * m + i + 1).to_string(), (-at(i), -ly.centre()), (w, ly.len()), false));
            }
            let mut total = d.pins;
            if let (Some(t), Family::Qfn) = (d.thermal, family) {
                total += 1;
                pads.push(pad(&total.to_string(), (0.0, 0.0), t, false));
            }
            body = body.or(Some((d.span.nom(), sy.nom())));
            let (bx, by) = body.unwrap();
            name = if family == Family::Qfn {
                format!("QFN{pitch_code}P{}X{}X{}-{total}{sfx}", h(bx), h(by), h(d.height))
            } else {
                format!("QFP{pitch_code}P{}X{}X{}-{total}{sfx}", h(d.span.nom()), h(sy.nom()), h(d.height))
            };
            descr = format!("{total} pin {} land pattern, pitch {} mm, IPC-7351B", family.label(), fmt(d.pitch));
        }
        Family::Bga => {
            let dia = r2(bga_pad(d.ball));
            for r in 0..d.rows {
                for c in 0..d.cols {
                    let at = (r2((c as f64 - (d.cols - 1) as f64 / 2.0) * d.pitch), r2((r as f64 - (d.rows - 1) as f64 / 2.0) * d.pitch));
                    pads.push(pad(&format!("{}{}", row_letter(r), c + 1), at, (dia, dia), true));
                }
            }
            let b = body.unwrap_or(((d.cols as f64) * d.pitch + 0.5, (d.rows as f64) * d.pitch + 0.5));
            body = Some(b);
            name = format!("BGA{}C{pitch_code}P{}X{}_{}X{}X{}", d.rows * d.cols, d.cols, d.rows, h(b.0), h(b.1), h(d.height));
            descr = format!("{} ball BGA land pattern, pitch {} mm, IPC-7351B", d.rows * d.cols, fmt(d.pitch));
        }
    }

    let (bx, by) = body.unwrap_or((1.0, 1.0));
    let (hx, hy) = (bx / 2.0, by / 2.0);
    let extent = pads.iter().fold((hx, hy), |a, p| {
        let at = p.child("at").unwrap();
        let sz = p.child("size").unwrap();
        let g = |s: &Sexpr, i: usize| s.arg(i).and_then(|t| t.parse::<f64>().ok()).unwrap_or(0.0);
        (a.0.max(g(at, 0).abs() + g(sz, 0) / 2.0), a.1.max(g(at, 1).abs() + g(sz, 1) / 2.0))
    });
    let excess = limits.courtyard_excess.unwrap_or(0.25);
    let up = |v: f64| (v * 100.0 - 1e-6).ceil() / 100.0;
    let (cx, cy) = (up(extent.0 + excess), up(extent.1 + excess));

    let mut items = vec![
        Sexpr::list("version", vec![atom("20240108")]),
        Sexpr::list("generator", vec![Sexpr::str("klm")]),
        Sexpr::list("generator_version", vec![Sexpr::str("1.0")]),
        layer("F.Cu"),
        Sexpr::list("descr", vec![Sexpr::str(&descr)]),
        text("Reference", "REF**", -cy - 1.0, "F.SilkS"),
        text("Value", &name, cy + 1.0, "F.Fab"),
        Sexpr::list("attr", vec![atom("smd")]),
        rect("F.Fab", (-hx, -hy), (hx, hy), 0.1),
        rect("F.CrtYd", (-cx, -cy), (cx, cy), 0.05),
        // Outside the body; the profile's silk fix clips it off the pads.
    ];
    let (sx, sy) = (hx + 0.11, hy + 0.11);
    items.extend([((-sx, -sy), (sx, -sy)), ((sx, -sy), (sx, sy)), ((sx, sy), (-sx, sy)), ((-sx, sy), (-sx, -sy))].map(|(a, b)| line("F.SilkS", a, b, 0.12)));
    items.extend(pads);
    let mut fp = Sexpr::list("footprint", [vec![Sexpr::str(&name)], items].concat());
    let fixes = standardise::fix_footprint(&mut fp, rules, style, Some(&limits));
    Ok(Generated { name, text: sexpr::to_string(&fp), fixes })
}

/// Writes the footprint into the category's `.pretty` folder. Refuses to replace one unless `overwrite`.
pub fn save(root: &LibraryRoot, category: &str, g: &Generated, overwrite: bool) -> Result<PathBuf> {
    let dir = root.dir.join("footprints").join(format!("{}.pretty", sanitize(category)));
    let path = dir.join(format!("{}.kicad_mod", sanitize(&g.name)));
    if path.exists() && !overwrite {
        bail!("{} already exists", path.display());
    }
    std::fs::create_dir_all(&dir)?;
    write_atomic(&path, &g.text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::numbered_pads;

    fn choice(d: char) -> FabChoice {
        FabChoice { fabs: vec!["jlcpcb".into()], ipc_class: 2, density: d }
    }

    fn make(f: Family, d: &Dims, density: char) -> (Generated, Sexpr) {
        let g = generate(f, d, &choice(density), &standardise::Rules::default(), None).unwrap();
        let fp = sexpr::parse(&g.text).unwrap();
        (g, fp)
    }

    fn pad_of(fp: &Sexpr, number: &str) -> (f64, f64, f64, f64) {
        let p = fp.children("pad").find(|p| p.arg(0) == Some(number)).unwrap();
        let g = |s: &Sexpr, i| s.arg(i).unwrap().parse::<f64>().unwrap();
        let (at, sz) = (p.child("at").unwrap(), p.child("size").unwrap());
        (g(at, 0), g(at, 1), g(sz, 0), g(sz, 1))
    }

    fn near(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) {
        for (x, y) in [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)] {
            assert!((x - y).abs() <= 0.0101, "{a:?} vs {b:?}");
        }
    }

    /// Every pad passes the fab's limits and the footprint parses back to the same text.
    fn clean(g: &Generated, fp: &Sexpr, pads: usize) {
        assert_eq!(numbered_pads(fp).len(), pads, "{}", g.name);
        let issues = crate::fab::check_footprint(fp, &fab_data::limits(&choice('B')));
        let msgs: Vec<_> = issues.iter().map(|i| i.message.as_str()).collect();
        // Density B's courtyard is the only limit that depends on the choice; others are the same for A and C.
        assert!(msgs.is_empty() || msgs.iter().all(|m| m.contains("CrtYd")), "{}: {msgs:?}", g.name);
        assert_eq!(sexpr::parse(&sexpr::to_string(fp)).unwrap(), *fp);
    }

    // Golden values are the IPC-7351B formulas worked by hand for each input (F 0.05, P 0.025), not data from the
    // standard itself, which is paywalled.

    #[test]
    fn chip_1608_nominal() {
        // L 1.45–1.75, T 0.2–0.5, W 0.7–0.9. CS = √(0.3² + 2·0.3²) = 0.5196 (worst case 0.9), Smax = 1.35 − 0.1902 = 1.1598.
        // Z = 1.45 + 0.7 + 0.3052 = 2.4552; G = 1.1598 − 0.5223 = 0.6372 (Jh = 0); X = 0.7 + 0.2077 = 0.9077.
        let d = Dims { span: Range::new(1.45, 1.75), lead_len: Range::new(0.2, 0.5), lead_w: Range::new(0.7, 0.9), height: 0.55, ..Default::default() };
        let (g, fp) = make(Family::Chip, &d, 'B');
        assert_eq!(g.name, "RESC1608X55N");
        near(pad_of(&fp, "1"), (-0.77, 0.0, 0.91, 0.91));
        clean(&g, &fp, 2);
    }

    #[test]
    fn sod_123_nominal() {
        // L 3.55–3.85, T 0.25–0.75, W 0.8–1.0, body 2.7 × 1.6 (gull table). CS = √(0.3² + 2·0.5²) = 0.7616, Smax = 3.35 − 0.2192 = 3.1308 (before Jh).
        // Z = 3.55 + 0.7 + 0.3052 = 4.5552; G = 3.1308 − 0.7 − 0.7633 = 1.6139?; len 1.47, centre 1.54, X = 0.8 + 0.06 + 0.2077 = 1.0677.
        let d = Dims {
            span: Range::new(3.55, 3.85),
            lead_len: Range::new(0.25, 0.75),
            lead_w: Range::new(0.8, 1.0),
            body: Some((2.7, 1.6)),
            height: 1.35,
            ..Default::default()
        };
        let (g, fp) = make(Family::Sod, &d, 'B');
        assert_eq!(g.name, "SOD3716X135N");
        near(pad_of(&fp, "2"), (1.54, 0.0, 1.47, 1.07));
        clean(&g, &fp, 2);
    }

    #[test]
    fn sot23_nominal() {
        // L 2.6–3.0, T 0.3–0.6, W 0.3–0.5, pitch 0.95. Z = 3.7039, G = 0.9058, len 1.40, centre 1.15, X = 0.5677.
        let d = Dims {
            pins: 3,
            pitch: 0.95,
            span: Range::new(2.6, 3.0),
            lead_len: Range::new(0.3, 0.6),
            lead_w: Range::new(0.3, 0.5),
            body: Some((1.3, 2.9)),
            height: 1.25,
            ..Default::default()
        };
        let (g, fp) = make(Family::Sot, &d, 'B');
        assert_eq!(g.name, "SOT95P280X125-3N");
        near(pad_of(&fp, "1"), (-1.15, -0.95, 1.4, 0.57));
        near(pad_of(&fp, "3"), (1.15, 0.0, 1.4, 0.57));
        clean(&g, &fp, 3);
    }

    #[test]
    fn soic8_nominal() {
        // L 5.8–6.2, T 0.4–0.7, W 0.31–0.51: CS = √(0.4² + 2·0.3²) = 0.5831, Z = 6.9039, G = 3.9058,
        // len 1.50, centre 2.70, X = 0.5777 (PCB Libraries\' published SOIC-8 is 1.55 at 2.70).
        let d = Dims {
            pins: 8,
            pitch: 1.27,
            span: Range::new(5.8, 6.2),
            lead_len: Range::new(0.4, 0.7),
            lead_w: Range::new(0.31, 0.51),
            body: Some((3.9, 4.9)),
            height: 1.75,
            ..Default::default()
        };
        let (g, fp) = make(Family::Soic, &d, 'B');
        assert_eq!(g.name, "SOIC127P600X175-8N");
        near(pad_of(&fp, "1"), (-2.7, -1.91, 1.5, 0.58));
        near(pad_of(&fp, "5"), (2.7, 1.91, 1.5, 0.58));
        clean(&g, &fp, 8);
    }

    #[test]
    fn qfn_nominal_with_thermal_pad() {
        // 4 × 4 body, L 3.9–4.1, T 0.3–0.5, W 0.2–0.3, pitch 0.5, 16 pins (toe-fillet table): Z = 4.7077, G = 3.0223,
        // len 0.84, centre 1.93, X = 0.2346.
        let d = Dims {
            pins: 16,
            pitch: 0.5,
            span: Range::new(3.9, 4.1),
            lead_len: Range::new(0.3, 0.5),
            lead_w: Range::new(0.2, 0.3),
            body: Some((4.0, 4.0)),
            height: 0.8,
            thermal: Some((2.0, 2.0)),
            ..Default::default()
        };
        let (g, fp) = make(Family::Qfn, &d, 'B');
        assert_eq!(g.name, "QFN50P400X400X80-17N");
        near(pad_of(&fp, "1"), (-1.93, -0.75, 0.84, 0.23));
        near(pad_of(&fp, "5"), (-0.75, 1.93, 0.23, 0.84));
        clean(&g, &fp, 17);
    }

    #[test]
    fn qfp_fine_pitch_nominal() {
        // LQFP-32: 7 × 7 body, lead span 8.8–9.2, T 0.45–0.75, W 0.30–0.45, pitch 0.8 (gull table): Z = 9.9039, G = 6.8058,
        // len 1.55, centre 4.18, X = 0.5201.
        let d = Dims {
            pins: 32,
            pitch: 0.8,
            span: Range::new(8.8, 9.2),
            lead_len: Range::new(0.45, 0.75),
            lead_w: Range::new(0.3, 0.45),
            body: Some((7.0, 7.0)),
            height: 1.6,
            ..Default::default()
        };
        let (g, fp) = make(Family::Qfp, &d, 'B');
        assert_eq!(g.name, "QFP80P900X900X160-32N");
        near(pad_of(&fp, "1"), (-4.18, -2.8, 1.55, 0.52));
        clean(&g, &fp, 32);
    }

    #[test]
    fn bga_pitch_one_mm() {
        // 4 × 4, 1.0 mm pitch, 0.5 mm balls: 0.8 × ball = 0.40 mm pads, A1 at the top left.
        let d = Dims { pitch: 1.0, rows: 4, cols: 4, ball: 0.5, body: Some((5.0, 5.0)), height: 1.2, ..Default::default() };
        let (g, fp) = make(Family::Bga, &d, 'B');
        assert_eq!(g.name, "BGA16C100P4X4_500X500X120");
        near(pad_of(&fp, "A1"), (-1.5, -1.5, 0.4, 0.4));
        near(pad_of(&fp, "D4"), (1.5, 1.5, 0.4, 0.4));
        clean(&g, &fp, 16);
    }

    #[test]
    fn density_moves_the_land_and_the_courtyard() {
        let d = Dims {
            pins: 8,
            pitch: 1.27,
            span: Range::new(5.8, 6.2),
            lead_len: Range::new(0.4, 0.7),
            lead_w: Range::new(0.31, 0.51),
            body: Some((3.9, 4.9)),
            height: 1.75,
            ..Default::default()
        };
        let (most, least) = (make(Family::Soic, &d, 'A').1, make(Family::Soic, &d, 'C').1);
        assert!(pad_of(&most, "1").2 > pad_of(&least, "1").2);
        for (fp, e) in [(&most, 0.5), (&least, 0.1)] {
            let issues = crate::fab::check_footprint(fp, &fab_data::limits(&choice(if e > 0.2 { 'A' } else { 'C' })));
            assert!(issues.iter().all(|i| !i.message.contains("CrtYd")), "{issues:?}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        let choice = choice('B');
        let r = standardise::Rules::default();
        assert!(generate(Family::Soic, &Dims { pins: 7, pitch: 1.27, ..Default::default() }, &choice, &r, None).is_err());
        assert!(Range::parse("2-1").is_err() && Range::parse("x").is_err());
        assert_eq!(Range::parse("1.5-1.7").unwrap(), Range::new(1.5, 1.7));
        assert_eq!(row_letter(8), "J");
        assert_eq!(row_letter(20), "AA");
    }

    #[test]
    fn saves_into_the_category_and_refuses_to_overwrite() {
        let dir = crate::test_dir("footprint_gen_save");
        let root = LibraryRoot::project(&dir);
        let d = Dims { span: Range::new(1.45, 1.75), lead_len: Range::new(0.2, 0.5), lead_w: Range::new(0.7, 0.9), height: 0.55, ..Default::default() };
        let g = make(Family::Chip, &d, 'B').0;
        let p = save(&root, "Resistors", &g, false).unwrap();
        assert!(p.ends_with("lib/footprints/Resistors.pretty/RESC1608X55N.kicad_mod"));
        assert!(save(&root, "Resistors", &g, false).is_err());
        assert!(save(&root, "Resistors", &g, true).is_ok());
    }
}
