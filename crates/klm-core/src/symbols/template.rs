//! The house templates: body graphics as KiCad s-expressions (geometry follows KiCad's own Device and
//! Transistor_BJT libraries, KLC S3/S4) plus where each pin slot sits. Slot order is the role order of
//! `Template::roles`.

use super::class::Class;
use super::{assign, is_hidden, pin_number, source_pins};
use crate::sexpr::{self, Sexpr};
use anyhow::{Result, anyhow, bail};
use std::borrow::Cow;

/// A pin position: connection point, rotation (direction towards the body) and length, mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot(pub f64, pub f64, pub f64, pub f64);

/// Built-in templates are static; a house template drawn by the user (`from_symbol`) owns its parts.
#[derive(Clone, Debug)]
pub struct Template {
    /// Graphics of the `_0_1` unit.
    pub body: Cow<'static, str>,
    pub slots: Cow<'static, [Slot]>,
    /// What each slot is, matched against pin names. Empty: the pins are interchangeable.
    pub roles: &'static [&'static [&'static str]],
    /// Names and numbers are hidden (two-terminal parts), or only names (transistors).
    pub hide_numbers: bool,
    /// Where Reference and Value sit (x, y, rotation); `None` puts them above and below the body.
    pub fields: Option<[(f64, f64, f64); 2]>,
}

const R: Template = Template {
    body: Cow::Borrowed("(rectangle (start -1.016 -2.54) (end 1.016 2.54) (stroke (width 0.254) (type default)) (fill (type none)))"),
    slots: Cow::Borrowed(&[Slot(0.0, 3.81, 270.0, 1.27), Slot(0.0, -3.81, 90.0, 1.27)]),
    roles: &[],
    hide_numbers: true,
    fields: None,
};

const C: Template = Template {
    body: Cow::Borrowed("(polyline (pts (xy -2.032 0.762) (xy 2.032 0.762)) (stroke (width 0.508) (type default)) (fill (type none)))
           (polyline (pts (xy -2.032 -0.762) (xy 2.032 -0.762)) (stroke (width 0.508) (type default)) (fill (type none)))"),
    slots: Cow::Borrowed(&[Slot(0.0, 3.81, 270.0, 2.794), Slot(0.0, -3.81, 90.0, 2.794)]),
    roles: &[],
    hide_numbers: true,
    fields: None,
};

const L: Template = Template {
    body: Cow::Borrowed("(arc (start 0 2.54) (mid 0.6323 1.905) (end 0 1.27) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 1.27) (mid 0.6323 0.635) (end 0 0) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 0) (mid 0.6323 -0.635) (end 0 -1.27) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 -1.27) (mid 0.6323 -1.905) (end 0 -2.54) (stroke (width 0.254) (type default)) (fill (type none)))"),
    slots: Cow::Borrowed(&[Slot(0.0, 3.81, 270.0, 1.27), Slot(0.0, -3.81, 90.0, 1.27)]),
    roles: &[],
    hide_numbers: true,
    fields: None,
};

const DIODE_BODY: &str = "(polyline (pts (xy -1.27 1.27) (xy -1.27 -1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 1.27 1.27) (xy 1.27 -1.27) (xy -1.27 0) (xy 1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 1.27 0) (xy -1.27 0)) (stroke (width 0.254) (type default)) (fill (type none)))";

/// Slot 0 is the cathode (the bar), slot 1 the anode.
const DIODE_SLOTS: &[Slot] = &[Slot(-3.81, 0.0, 0.0, 2.54), Slot(3.81, 0.0, 180.0, 2.54)];
const DIODE_ROLES: &[&[&str]] = &[&["K", "C", "CATHODE", "-"], &["A", "ANODE", "+"]];

const D: Template = Template { body: Cow::Borrowed(DIODE_BODY), slots: Cow::Borrowed(DIODE_SLOTS), roles: DIODE_ROLES, hide_numbers: true, fields: None };

const LED: Template = Template {
    body: Cow::Borrowed("(polyline (pts (xy -3.048 -0.762) (xy -4.572 -2.286) (xy -3.81 -2.286) (xy -4.572 -2.286) (xy -4.572 -1.524)) (stroke (width 0.254) (type default)) (fill (type none)))
           (polyline (pts (xy -1.778 -0.762) (xy -3.302 -2.286) (xy -2.54 -2.286) (xy -3.302 -2.286) (xy -3.302 -1.524)) (stroke (width 0.254) (type default)) (fill (type none)))
           (polyline (pts (xy -1.27 1.27) (xy -1.27 -1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
           (polyline (pts (xy 1.27 1.27) (xy 1.27 -1.27) (xy -1.27 0) (xy 1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
           (polyline (pts (xy 1.27 0) (xy -1.27 0)) (stroke (width 0.254) (type default)) (fill (type none)))"),
    slots: Cow::Borrowed(DIODE_SLOTS),
    roles: DIODE_ROLES,
    hide_numbers: true,
    fields: None,
};

/// Base, collector, emitter.
const BJT_SLOTS: &[Slot] = &[Slot(-5.08, 0.0, 0.0, 2.54), Slot(2.54, 5.08, 270.0, 2.54), Slot(2.54, -5.08, 90.0, 2.54)];
const BJT_ROLES: &[&[&str]] = &[&["B", "BASE"], &["C", "COL", "COLLECTOR"], &["E", "EM", "EMITTER"]];
const NPN: Template = Template {
    body: Cow::Borrowed(concat!(
        "(polyline (pts (xy -2.54 0) (xy 0.635 0)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 1.905) (xy 0.635 -1.905)) (stroke (width 0.508) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 0.635) (xy 2.54 2.54)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 -0.635) (xy 2.54 -2.54)) (stroke (width 0.254) (type default)) (fill (type none)))
    (circle (center 1.27 0) (radius 2.8194) (stroke (width 0.254) (type default)) (fill (type none)))",
        "(polyline (pts (xy 1.27 -1.778) (xy 1.778 -1.27) (xy 2.286 -2.286) (xy 1.27 -1.778)) (stroke (width 0.254) (type default)) (fill (type outline)))"
    )),
    slots: Cow::Borrowed(BJT_SLOTS),
    roles: BJT_ROLES,
    hide_numbers: false,
    fields: None,
};

const PNP: Template = Template {
    body: Cow::Borrowed(concat!(
        "(polyline (pts (xy -2.54 0) (xy 0.635 0)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 1.905) (xy 0.635 -1.905)) (stroke (width 0.508) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 0.635) (xy 2.54 2.54)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 -0.635) (xy 2.54 -2.54)) (stroke (width 0.254) (type default)) (fill (type none)))
    (circle (center 1.27 0) (radius 2.8194) (stroke (width 0.254) (type default)) (fill (type none)))",
        "(polyline (pts (xy 2.286 -1.778) (xy 1.778 -2.286) (xy 1.27 -1.27) (xy 2.286 -1.778)) (stroke (width 0.254) (type default)) (fill (type outline)))"
    )),
    slots: Cow::Borrowed(BJT_SLOTS),
    roles: BJT_ROLES,
    hide_numbers: false,
    fields: None,
};

/// The fixed template of a class; the IC box is drawn to fit its pins instead.
pub fn of(class: Class) -> Option<&'static Template> {
    Some(match class {
        Class::Resistor => &R,
        Class::Capacitor => &C,
        Class::Inductor => &L,
        Class::Diode => &D,
        Class::Led => &LED,
        Class::Npn => &NPN,
        Class::Pnp => &PNP,
        Class::Ic => return None,
    })
}

impl Template {
    /// A house template from a symbol the user drew in KiCad: its graphics, pin positions, pin-number
    /// display and Reference/Value positions. Roles are matched by pin name, as when restyling.
    pub fn from_symbol(sym: &Sexpr, class: Class) -> Result<Template> {
        let Some(built_in) = of(class) else { bail!("IC symbols are drawn to fit their pins and take no template") };
        let pins = source_pins(sym)?;
        let order = assign(built_in, &pins).map_err(|e| anyhow!("doesn't fit a {class:?}: {e:#}"))?;
        let num = |x: &Sexpr, i: usize| x.arg(i).and_then(|v| v.parse::<f64>().ok());
        let mut slots = vec![];
        for &i in &order {
            // `source_pins` made sure every pin has both.
            let (at, len) = (pins[i].child("at").unwrap(), pins[i].child("length").unwrap());
            let (Some(x), Some(y), Some(l)) = (num(at, 0), num(at, 1), num(len, 0)) else { bail!("pin {} has a bad position", pin_number(&pins[i])) };
            slots.push(Slot(x, y, num(at, 2).unwrap_or(0.0), l));
        }
        let body: Vec<String> = sym.children("symbol").flat_map(|u| u.items().iter().skip(2)).filter(|c| c.head() != Some("pin")).map(sexpr::to_string).collect();
        if body.is_empty() {
            bail!("has no body graphics");
        }
        let field = |k: &str| {
            let at = sym.children("property").find(|p| p.arg(0) == Some(k))?.child("at")?;
            Some((num(at, 0)?, num(at, 1)?, num(at, 2).unwrap_or(0.0)))
        };
        Ok(Template {
            body: Cow::Owned(body.join("\n")),
            slots: Cow::Owned(slots),
            roles: built_in.roles,
            hide_numbers: sym.child("pin_numbers").is_some_and(is_hidden),
            fields: field("Reference").zip(field("Value")).map(|(r, v)| [r, v]),
        })
    }
}
