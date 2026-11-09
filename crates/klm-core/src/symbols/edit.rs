//! Small edits to one symbol, for the in-app symbol editor: the pin table, the body rectangle and where
//! the text sits. Everything else in the symbol (properties, alternates, hidden flags, unknown syntax)
//! is left as it was. Full graphical editing stays in KiCad.

use super::{atom, lib_path, pin_name, pin_number, pin_type};
use crate::kicad_env::write_atomic;
use crate::library::LibraryRoot;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};

/// KiCad's 50 mil schematic grid, in mm.
pub const GRID: f64 = 1.27;

/// Nearest grid point.
pub fn snap(v: f64) -> f64 {
    (v / GRID).round() * GRID
}

/// Electrical types KiCad knows, for the editor's drop-down.
pub const PIN_TYPES: [&str; 12] = [
    "input", "output", "bidirectional", "tri_state", "passive", "free", "unspecified", "power_in", "power_out", "open_collector", "open_emitter", "no_connect",
];

/// One pin as the table shows it. `rot` is 0 (left side), 180 (right), 270 (top) or 90 (bottom), as KiCad stores it.
#[derive(Clone, Debug, PartialEq)]
pub struct PinRow {
    pub number: String,
    pub name: String,
    pub etype: String,
    pub x: f64,
    pub y: f64,
    pub rot: f64,
    pub length: f64,
}

fn num(n: Option<&Sexpr>, i: usize) -> f64 {
    n.and_then(|n| n.arg(i)).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

fn pins_mut(sym: &mut Sexpr) -> impl Iterator<Item = &mut Sexpr> {
    sym.items_mut().iter_mut().filter(|u| u.head() == Some("symbol")).flat_map(|u| u.items_mut().iter_mut()).filter(|p| p.head() == Some("pin"))
}

/// The symbol's pins, in file order.
pub fn pins(sym: &Sexpr) -> Vec<PinRow> {
    sym.children("symbol")
        .flat_map(|u| u.children("pin"))
        .map(|p| PinRow {
            number: pin_number(p).into(),
            name: pin_name(p).into(),
            etype: pin_type(p).into(),
            x: num(p.child("at"), 0),
            y: num(p.child("at"), 1),
            rot: num(p.child("at"), 2),
            length: num(p.child("length"), 0),
        })
        .collect()
}

fn set_text(p: &mut Sexpr, head: &str, text: &str) {
    if let Some(c) = p.items_mut().iter_mut().find(|c| c.head() == Some(head)) {
        c.set_arg(0, Sexpr::str(text));
    }
}

fn apply(p: &mut Sexpr, row: &PinRow) {
    p.set_arg(0, Sexpr::Atom(row.etype.clone()));
    set_text(p, "number", &row.number);
    set_text(p, "name", &row.name);
    for c in p.items_mut() {
        match c.head() {
            Some("at") => *c = Sexpr::list("at", vec![atom(row.x), atom(row.y), atom(row.rot)]),
            Some("length") => *c = Sexpr::list("length", vec![atom(row.length)]),
            _ => {}
        }
    }
}

/// Changes pin `number` to `row` (position and length snap to the grid; a pin may be renumbered).
pub fn set_pin(sym: &mut Sexpr, number: &str, mut row: PinRow) -> Result<()> {
    if row.number.trim().is_empty() {
        bail!("a pin needs a number");
    }
    if row.number != number && pins(sym).iter().any(|p| p.number == row.number) {
        bail!("pin number {} is used twice", row.number);
    }
    (row.x, row.y, row.length) = (snap(row.x), snap(row.y), snap(row.length).max(GRID));
    pins_mut(sym).find(|p| pin_number(p) == number).map(|p| apply(p, &row)).with_context(|| format!("no pin {number}"))
}

/// Adds a pin to the unit that holds the others (or a new `_1_1` unit when there are none).
pub fn add_pin(sym: &mut Sexpr, mut row: PinRow) -> Result<()> {
    if row.number.trim().is_empty() || pins(sym).iter().any(|p| p.number == row.number) {
        bail!("pin number {:?} is empty or used twice", row.number);
    }
    (row.x, row.y, row.length) = (snap(row.x), snap(row.y), snap(row.length).max(GRID));
    let effects = || Sexpr::list("effects", vec![Sexpr::list("font", vec![Sexpr::list("size", vec![atom(GRID), atom(GRID)])])]);
    let mut pin = Sexpr::list("pin", vec![Sexpr::Atom("passive".into()), Sexpr::Atom("line".into())]);
    pin.items_mut().extend([
        Sexpr::list("at", vec![]),
        Sexpr::list("length", vec![]),
        Sexpr::list("name", vec![Sexpr::str(""), effects()]),
        Sexpr::list("number", vec![Sexpr::str(""), effects()]),
    ]);
    apply(&mut pin, &row);
    let name = sym.arg(0).context("symbol has no name")?.to_string();
    let mut at = sym.items().iter().position(|u| u.head() == Some("symbol") && u.child("pin").is_some());
    if at.is_none() {
        let end = sym.items().iter().position(|c| matches!(c.head(), Some("embedded_fonts" | "embedded_files"))).unwrap_or(sym.items().len());
        sym.items_mut().insert(end, Sexpr::list("symbol", vec![Sexpr::str(&format!("{name}_1_1"))]));
        at = Some(end);
    }
    sym.items_mut()[at.unwrap()].items_mut().push(pin);
    Ok(())
}

pub fn remove_pin(sym: &mut Sexpr, number: &str) -> Result<()> {
    for u in sym.items_mut().iter_mut().filter(|u| u.head() == Some("symbol")) {
        if let Some(i) = u.items().iter().position(|p| p.head() == Some("pin") && pin_number(p) == number) {
            u.items_mut().remove(i);
            return Ok(());
        }
    }
    bail!("no pin {number}")
}

fn body_mut(sym: &mut Sexpr) -> Option<&mut Sexpr> {
    sym.items_mut().iter_mut().filter(|u| u.head() == Some("symbol")).flat_map(|u| u.items_mut().iter_mut()).find(|c| c.head() == Some("rectangle"))
}

/// The first rectangle of the symbol as `(x1, y1, x2, y2)`.
pub fn body(sym: &Sexpr) -> Option<[f64; 4]> {
    let r = sym.children("symbol").find_map(|u| u.child("rectangle"))?;
    Some([num(r.child("start"), 0), num(r.child("start"), 1), num(r.child("end"), 0), num(r.child("end"), 1)])
}

/// Moves or resizes the body rectangle, snapped to the grid; its stroke and fill are kept.
pub fn set_body(sym: &mut Sexpr, [x1, y1, x2, y2]: [f64; 4]) -> Result<()> {
    let r = body_mut(sym).context("the symbol has no body rectangle")?;
    for c in r.items_mut() {
        match c.head() {
            Some("start") => *c = Sexpr::list("start", vec![atom(snap(x1)), atom(snap(y1))]),
            Some("end") => *c = Sexpr::list("end", vec![atom(snap(x2)), atom(snap(y2))]),
            _ => {}
        }
    }
    Ok(())
}

/// Where a property's text sits, as `(x, y, rotation)`.
pub fn text_at(sym: &Sexpr, key: &str) -> Option<[f64; 3]> {
    let at = sym.children("property").find(|p| p.arg(0) == Some(key))?.child("at");
    Some([num(at, 0), num(at, 1), num(at, 2)])
}

/// Moves a property's text (snapped to the grid); the rotation is kept as given.
pub fn set_text_at(sym: &mut Sexpr, key: &str, [x, y, rot]: [f64; 3]) -> Result<()> {
    let at = sym.items_mut().iter_mut().find(|p| p.head() == Some("property") && p.arg(0) == Some(key)).and_then(|p| p.items_mut().iter_mut().find(|c| c.head() == Some("at")));
    *at.with_context(|| format!("no property {key}"))? = Sexpr::list("at", vec![atom(snap(x)), atom(snap(y)), atom(rot)]);
    Ok(())
}

/// The symbol `name` of the `category` library, to edit and hand to [`save`].
pub fn load(root: &LibraryRoot, category: &str, name: &str) -> Result<Sexpr> {
    let lib = sexpr::parse(&std::fs::read_to_string(lib_path(root, category)).with_context(|| format!("no library {}", root.nickname(category)))?)?;
    lib.children("symbol").find(|s| s.arg(0) == Some(name)).cloned().with_context(|| format!("{name} is not in {}", root.nickname(category)))
}

/// Replaces the symbol `name` in its library after a snapshot (History can undo it). `was` is the symbol as
/// [`load`] returned it: if the file changed meanwhile nothing is written. The caller re-runs the checks.
pub fn save(root: &LibraryRoot, category: &str, name: &str, was: &Sexpr, new: Sexpr) -> Result<()> {
    let path = lib_path(root, category);
    let mut lib = sexpr::parse(&std::fs::read_to_string(&path)?)?;
    let idx = lib.items().iter().position(|c| c.head() == Some("symbol") && c.arg(0) == Some(name)).with_context(|| format!("{name} is not in {}", root.nickname(category)))?;
    if lib.items()[idx] != *was {
        bail!("{name} changed on disk meanwhile (in KiCad?); reopen the editor");
    }
    let _snapshot = crate::snapshot::guard(root, &format!("edit symbol {name}"))?;
    lib.items_mut()[idx] = new;
    write_atomic(&path, &sexpr::to_string(&lib))
}
