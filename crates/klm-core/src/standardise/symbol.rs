//! Symbol rules. Symbol coordinates have y pointing up.

use super::{Rules, bbox, child_mut, differs, fmt, move_to, num, set_num};
use crate::sexpr::Sexpr;

const GRAPHICS: &[&str] = &["rectangle", "polyline", "circle", "arc", "bezier"];

/// Applies the symbol rules to one top-level symbol and says what changed.
pub(crate) fn fix(sym: &mut Sexpr, r: &Rules) -> Vec<String> {
    let mut out = vec![];
    out.extend(r.symbol_text_size.and_then(|s| text_size(sym, s)));
    out.extend(r.pin_length_min.and_then(|l| pin_length(sym, l)));
    out.extend(r.symbol_line_width.and_then(|w| line_width(sym, w)));
    // Last, so the body includes any lengthened pins.
    if r.symbol_field_positions {
        out.extend(field_positions(sym));
    }
    out
}

fn units(sym: &mut Sexpr) -> impl Iterator<Item = &mut Sexpr> {
    sym.items_mut().iter_mut().filter(|c| c.head() == Some("symbol"))
}

fn walk_mut(x: &mut Sexpr, f: &mut impl FnMut(&mut Sexpr)) {
    f(x);
    if let Sexpr::List(v) = x {
        v.iter_mut().for_each(|c| walk_mut(c, f));
    }
}

fn text_size(sym: &mut Sexpr, size: f64) -> Option<String> {
    let mut n = 0;
    walk_mut(sym, &mut |x| {
        let Some(sz) = (x.head() == Some("font")).then(|| child_mut(x, "size")).flatten() else { return };
        if (0..2).any(|i| num(sz, i).is_some_and(|v| differs(v, size))) {
            set_num(sz, 0, size);
            set_num(sz, 1, size);
            n += 1;
        }
    });
    (n > 0).then(|| format!("{n} text(s) resized to {} mm", fmt(size)))
}

/// Unit vector a pin points in, from its connection point towards the body.
fn direction(pin: &Sexpr) -> Option<(f64, f64)> {
    match num(pin.child("at")?, 2).unwrap_or(0.0).round() as i64 % 360 {
        0 => Some((1.0, 0.0)),
        90 => Some((0.0, 1.0)),
        180 => Some((-1.0, 0.0)),
        270 => Some((0.0, -1.0)),
        _ => None,
    }
}

fn pin_length(sym: &mut Sexpr, min: f64) -> Option<String> {
    let pins = sym.children("symbol").flat_map(|u| u.children("pin")).count();
    if sym.child("power").is_some() || pins <= 2 {
        return None;
    }
    let mut n = 0;
    for pin in units(sym).flat_map(|u| u.items_mut().iter_mut().filter(|c| c.head() == Some("pin"))) {
        let hidden = pin.child("hide").is_some_and(|h| h.arg(0) != Some("no")) || pin.items().contains(&Sexpr::Atom("hide".into()));
        let (Some(len), Some(dir), Some((x, y))) =
            (pin.child("length").and_then(|l| num(l, 0)), direction(pin), pin.child("at").and_then(|a| Some((num(a, 0)?, num(a, 1)?))))
        else {
            continue;
        };
        if hidden || len >= min - 1e-4 {
            continue;
        }
        // Keep the body end where it is and move the connection point outwards.
        let grow = min - len;
        move_to(child_mut(pin, "at").unwrap(), (x - dir.0 * grow, y - dir.1 * grow));
        set_num(child_mut(pin, "length").unwrap(), 0, min);
        n += 1;
    }
    (n > 0).then(|| format!("{n} pin(s) lengthened to {} mm", fmt(min)))
}

fn line_width(sym: &mut Sexpr, width: f64) -> Option<String> {
    let mut n = 0;
    for g in units(sym).flat_map(|u| u.items_mut().iter_mut().filter(|c| c.head().is_some_and(|h| GRAPHICS.contains(&h)))) {
        if let Some(w) = child_mut(g, "stroke").and_then(|s| child_mut(s, "width")).filter(|w| num(w, 0).is_some_and(|v| differs(v, width))) {
            set_num(w, 0, width);
            n += 1;
        }
    }
    (n > 0).then(|| format!("{n} line width(s) set to {} mm", fmt(width)))
}

/// Two-pin discretes (a TVS, a polarised capacitor) show neither pin names nor numbers, as in KiCad's
/// Device library. Hidden pins don't count: two visible terminals make a two-pin part. Not for
/// connectors, whose pin numbers matter: the caller picks by category.
pub(crate) fn two_pin_labels(sym: &mut Sexpr) -> Option<String> {
    let pins = sym
        .children("symbol")
        .flat_map(|u| u.children("pin"))
        .filter(|p| !p.child("hide").is_some_and(|h| h.arg(0) != Some("no")) && !p.items().contains(&Sexpr::Atom("hide".into())))
        .count();
    if sym.child("power").is_some() || pins != 2 {
        return None;
    }
    let yes = || Sexpr::list("hide", vec![Sexpr::Atom("yes".into())]);
    let mut n = 0;
    for head in ["pin_numbers", "pin_names"] {
        match child_mut(sym, head) {
            Some(Sexpr::List(v)) => {
                // `(hide no)` or nothing shows them; a bare `hide` (old files) or `(hide yes)` hides them.
                if v.iter().any(|c| *c == Sexpr::Atom("hide".into()) || (c.head() == Some("hide") && c.arg(0) != Some("no"))) {
                    continue;
                }
                v.retain(|c| c.head() != Some("hide"));
                v.push(yes());
            }
            _ => {
                let at = 2.min(sym.items().len());
                sym.items_mut().insert(at, Sexpr::list(head, vec![yes()]));
            }
        }
        n += 1;
    }
    (n > 0).then(|| "pin names and numbers hidden (two-pin part)".to_string())
}

fn xy(x: &Sexpr) -> Option<(f64, f64)> {
    Some((num(x, 0)?, num(x, 1)?))
}

/// Outline of the graphics and pins of every unit.
fn body(sym: &Sexpr) -> Option<(f64, f64, f64, f64)> {
    fn points(x: &Sexpr, out: &mut Vec<(f64, f64)>) {
        match x.head() {
            Some("start" | "end" | "mid" | "xy") => out.extend(xy(x)),
            Some("circle") => {
                if let (Some(c), Some(r)) = (x.child("center").and_then(xy), x.child("radius").and_then(|r| num(r, 0))) {
                    out.extend([(c.0 - r, c.1 - r), (c.0 + r, c.1 + r)]);
                }
            }
            _ => x.items().iter().for_each(|c| points(c, out)),
        }
    }
    let mut pts = vec![];
    for unit in sym.children("symbol") {
        for g in unit.items().iter().filter(|c| c.head().is_some_and(|h| GRAPHICS.contains(&h))) {
            points(g, &mut pts);
        }
        for pin in unit.children("pin") {
            if let (Some(at), Some(d), Some(len)) = (pin.child("at").and_then(xy), direction(pin), pin.child("length").and_then(|l| num(l, 0))) {
                pts.extend([at, (at.0 + d.0 * len, at.1 + d.1 * len)]);
            }
        }
    }
    bbox(&pts)
}

pub(crate) fn field_positions(sym: &mut Sexpr) -> Option<String> {
    let (x0, y0, x1, y1) = body(sym)?;
    let cx = (x0 + x1) / 2.0;
    let mut moved = vec![];
    for (key, y) in [("Reference", y1 + 1.27), ("Value", y0 - 1.27)] {
        let Some(prop) = sym.items_mut().iter_mut().find(|p| p.head() == Some("property") && p.arg(0) == Some(key)) else { continue };
        let centred = justify(prop, false);
        let Some(at) = child_mut(prop, "at") else { continue };
        let turned = num(at, 2).is_some_and(|r| differs(r, 0.0));
        if move_to(at, (cx, y)) | turned | centred {
            if turned {
                set_num(at, 2, 0.0);
            }
            moved.push(key);
        }
    }
    (!moved.is_empty()).then(|| format!("{} moved {}", moved.join(" and "), if moved.len() == 2 { "above and below the body" } else { "next to the body" }))
}

/// Sets the text height of a property, adding the font if it has none.
pub(crate) fn set_font_size(prop: &mut Sexpr, size: f64) {
    let want = Sexpr::list("size", vec![Sexpr::Atom(fmt(size)), Sexpr::Atom(fmt(size))]);
    let Some(Sexpr::List(fx)) = child_mut(prop, "effects") else {
        prop.items_mut().push(Sexpr::list("effects", vec![Sexpr::list("font", vec![want])]));
        return;
    };
    match fx.iter_mut().find(|c| c.head() == Some("font")) {
        Some(Sexpr::List(font)) => match font.iter().position(|c| c.head() == Some("size")) {
            Some(i) => font[i] = want,
            None => font.push(want),
        },
        _ => fx.insert(0, Sexpr::list("font", vec![want])),
    }
}

/// Left-justifies a property's text, or centres it. Returns whether that changed anything.
pub(crate) fn justify(prop: &mut Sexpr, left: bool) -> bool {
    let want = left.then(|| Sexpr::list("justify", vec![Sexpr::Atom("left".into())]));
    let Some(Sexpr::List(fx)) = child_mut(prop, "effects") else {
        if let (Sexpr::List(v), Some(w)) = (prop, want) {
            v.push(Sexpr::list("effects", vec![w]));
            return true;
        }
        return false;
    };
    match (fx.iter().position(|c| c.head() == Some("justify")), want) {
        (Some(i), Some(w)) if fx[i] != w => fx[i] = w,
        (Some(i), None) => _ = fx.remove(i),
        (None, Some(w)) => fx.push(w),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr::{parse, to_string};

    fn only(r: Rules) -> Rules {
        Rules { symbol_text_size: None, pin_length_min: None, symbol_line_width: None, symbol_field_positions: false, house_symbols: false, ..r }
    }

    #[test]
    fn two_pin_parts_hide_pin_labels() {
        let mut d = parse(r#"(symbol "D" (pin_names (offset 1.016) (hide no)) (symbol "D_1_1" (pin passive line (at -3.81 0 0) (length 2.54) (name "A") (number "1")) (pin passive line (at 3.81 0 180) (length 2.54) (name "K") (number "2"))))"#).unwrap();
        assert_eq!(two_pin_labels(&mut d).as_deref(), Some("pin names and numbers hidden (two-pin part)"));
        let s: String = to_string(&d).split_whitespace().collect();
        assert!(s.contains("(pin_numbers(hideyes))") && s.contains("(pin_names(offset1.016)(hideyes))"), "{s}");
        assert!(two_pin_labels(&mut d).is_none(), "second run changes nothing");
        // Three visible pins (an IC) keep their labels; a hidden third pin does not make one.
        assert!(two_pin_labels(&mut parse(&SYM.replace("(hide yes) ", "")).unwrap()).is_none());
        assert!(two_pin_labels(&mut parse(SYM).unwrap()).is_some());
    }

    const SYM: &str = r#"(symbol "U1"
        (property "Reference" "U" (at 9 9 90) (effects (font (size 2 2))))
        (property "Value" "U1" (at 9 9 0) (effects (font (size 1.27 1.27))))
        (symbol "U1_0_1" (rectangle (start -5 5) (end 5 -5) (stroke (width 0.1) (type default)) (fill (type none))))
        (symbol "U1_1_1"
            (pin input line (at -6.27 2.54 0) (length 1.27) (name "A" (effects (font (size 1 1)))) (number "1" (effects (font (size 1.27 1.27)))))
            (pin input line (at -7.54 0 0) (length 2.54) (name "B" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))
            (pin output line (at 7.54 0 180) (length 2.54) (hide yes) (name "Y" (effects (font (size 1.27 1.27)))) (number "3" (effects (font (size 1.27 1.27)))))))"#;

    fn run(r: Rules) -> (Sexpr, Vec<String>) {
        let mut s = parse(SYM).unwrap();
        let changes = fix(&mut s, &r);
        (s, changes)
    }

    #[test]
    fn text_sizes() {
        let (s, c) = run(Rules { symbol_text_size: Some(1.27), ..only(Rules::default()) });
        assert_eq!(c, ["2 text(s) resized to 1.27 mm"]);
        assert!(!to_string(&s).contains("(size 2 2)") && !to_string(&s).contains("(size 1 1)"));
    }

    #[test]
    fn short_pins_grow_outwards() {
        let (s, c) = run(Rules { pin_length_min: Some(2.54), ..only(Rules::default()) });
        assert_eq!(c, ["1 pin(s) lengthened to 2.54 mm"]);
        let pin = s.children("symbol").nth(1).unwrap().child("pin").unwrap();
        assert_eq!(to_string(pin.child("at").unwrap()).trim(), "(at -7.54 2.54 0)", "body end stays at x = -5");
        assert_eq!(pin.child("length").unwrap().arg(0), Some("2.54"));
        // Two-pin symbols (resistors) keep their short pins.
        let mut r = parse(r#"(symbol "R" (symbol "R_1_1" (pin passive line (at 0 3.81 270) (length 1.27)) (pin passive line (at 0 -3.81 90) (length 1.27))))"#)
            .unwrap();
        assert!(fix(&mut r, &Rules { pin_length_min: Some(2.54), ..only(Rules::default()) }).is_empty());
    }

    #[test]
    fn line_widths() {
        let (s, c) = run(Rules { symbol_line_width: Some(0.254), ..only(Rules::default()) });
        assert_eq!(c, ["1 line width(s) set to 0.254 mm"]);
        assert!(to_string(&s).contains("(width 0.254)"));
    }

    #[test]
    fn reference_above_value_below() {
        let (s, c) = run(Rules { symbol_field_positions: true, ..only(Rules::default()) });
        assert_eq!(c, ["Reference and Value moved above and below the body"]);
        let at = |k: &str| to_string(s.children("property").find(|p| p.arg(0) == Some(k)).unwrap().child("at").unwrap()).trim().to_string();
        // Body spans x -7.54..7.54 (pins included), y -5..5.
        assert_eq!(at("Reference"), "(at 0 6.27 0)");
        assert_eq!(at("Value"), "(at 0 -6.27 0)");
    }
}
