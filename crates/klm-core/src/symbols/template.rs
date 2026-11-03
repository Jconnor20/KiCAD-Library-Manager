//! The house templates: body graphics as KiCad s-expressions (geometry follows KiCad's own Device and
//! Transistor_BJT libraries, KLC S3/S4) plus where each pin slot sits. Slot order is the role order of
//! `Template::roles`.

use super::class::Class;

/// A pin position: connection point, rotation (direction towards the body) and length, mm.
pub struct Slot(pub f64, pub f64, pub f64, pub f64);

pub struct Template {
    /// Graphics of the `_0_1` unit.
    pub body: &'static str,
    pub slots: &'static [Slot],
    /// What each slot is, matched against pin names. Empty: the pins are interchangeable.
    pub roles: &'static [&'static [&'static str]],
    /// Names and numbers are hidden (two-terminal parts), or only names (transistors).
    pub hide_numbers: bool,
}

const R: Template = Template {
    body: "(rectangle (start -1.016 -2.54) (end 1.016 2.54) (stroke (width 0.254) (type default)) (fill (type none)))",
    slots: &[Slot(0.0, 3.81, 270.0, 1.27), Slot(0.0, -3.81, 90.0, 1.27)],
    roles: &[],
    hide_numbers: true,
};

const C: Template = Template {
    body: "(polyline (pts (xy -2.032 0.762) (xy 2.032 0.762)) (stroke (width 0.508) (type default)) (fill (type none)))
           (polyline (pts (xy -2.032 -0.762) (xy 2.032 -0.762)) (stroke (width 0.508) (type default)) (fill (type none)))",
    slots: &[Slot(0.0, 3.81, 270.0, 2.794), Slot(0.0, -3.81, 90.0, 2.794)],
    roles: &[],
    hide_numbers: true,
};

const L: Template = Template {
    body: "(arc (start 0 2.54) (mid 0.6323 1.905) (end 0 1.27) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 1.27) (mid 0.6323 0.635) (end 0 0) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 0) (mid 0.6323 -0.635) (end 0 -1.27) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 -1.27) (mid 0.6323 -1.905) (end 0 -2.54) (stroke (width 0.254) (type default)) (fill (type none)))",
    slots: &[Slot(0.0, 3.81, 270.0, 1.27), Slot(0.0, -3.81, 90.0, 1.27)],
    roles: &[],
    hide_numbers: true,
};

const DIODE_BODY: &str = "(polyline (pts (xy -1.27 1.27) (xy -1.27 -1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 1.27 1.27) (xy 1.27 -1.27) (xy -1.27 0) (xy 1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 1.27 0) (xy -1.27 0)) (stroke (width 0.254) (type default)) (fill (type none)))";

/// Slot 0 is the cathode (the bar), slot 1 the anode.
const DIODE_SLOTS: &[Slot] = &[Slot(-3.81, 0.0, 0.0, 2.54), Slot(3.81, 0.0, 180.0, 2.54)];
const DIODE_ROLES: &[&[&str]] = &[&["K", "C", "CATHODE", "-"], &["A", "ANODE", "+"]];

const D: Template = Template { body: DIODE_BODY, slots: DIODE_SLOTS, roles: DIODE_ROLES, hide_numbers: true };

const LED: Template = Template {
    body: "(polyline (pts (xy -3.048 -0.762) (xy -4.572 -2.286) (xy -3.81 -2.286) (xy -4.572 -2.286) (xy -4.572 -1.524)) (stroke (width 0.254) (type default)) (fill (type none)))
           (polyline (pts (xy -1.778 -0.762) (xy -3.302 -2.286) (xy -2.54 -2.286) (xy -3.302 -2.286) (xy -3.302 -1.524)) (stroke (width 0.254) (type default)) (fill (type none)))
           (polyline (pts (xy -1.27 1.27) (xy -1.27 -1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
           (polyline (pts (xy 1.27 1.27) (xy 1.27 -1.27) (xy -1.27 0) (xy 1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
           (polyline (pts (xy 1.27 0) (xy -1.27 0)) (stroke (width 0.254) (type default)) (fill (type none)))",
    slots: DIODE_SLOTS,
    roles: DIODE_ROLES,
    hide_numbers: true,
};

/// Base, collector, emitter.
const BJT_SLOTS: &[Slot] = &[Slot(-5.08, 0.0, 0.0, 2.54), Slot(2.54, 5.08, 270.0, 2.54), Slot(2.54, -5.08, 90.0, 2.54)];
const BJT_ROLES: &[&[&str]] = &[&["B", "BASE"], &["C", "COL", "COLLECTOR"], &["E", "EM", "EMITTER"]];
const NPN: Template = Template {
    body: concat!(
        "(polyline (pts (xy -2.54 0) (xy 0.635 0)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 1.905) (xy 0.635 -1.905)) (stroke (width 0.508) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 0.635) (xy 2.54 2.54)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 -0.635) (xy 2.54 -2.54)) (stroke (width 0.254) (type default)) (fill (type none)))
    (circle (center 1.27 0) (radius 2.8194) (stroke (width 0.254) (type default)) (fill (type none)))",
        "(polyline (pts (xy 1.27 -1.778) (xy 1.778 -1.27) (xy 2.286 -2.286) (xy 1.27 -1.778)) (stroke (width 0.254) (type default)) (fill (type outline)))"
    ),
    slots: BJT_SLOTS,
    roles: BJT_ROLES,
    hide_numbers: false,
};

const PNP: Template = Template {
    body: concat!(
        "(polyline (pts (xy -2.54 0) (xy 0.635 0)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 1.905) (xy 0.635 -1.905)) (stroke (width 0.508) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 0.635) (xy 2.54 2.54)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 0.635 -0.635) (xy 2.54 -2.54)) (stroke (width 0.254) (type default)) (fill (type none)))
    (circle (center 1.27 0) (radius 2.8194) (stroke (width 0.254) (type default)) (fill (type none)))",
        "(polyline (pts (xy 2.286 -1.778) (xy 1.778 -2.286) (xy 1.27 -1.27) (xy 2.286 -1.778)) (stroke (width 0.254) (type default)) (fill (type outline)))"
    ),
    slots: BJT_SLOTS,
    roles: BJT_ROLES,
    hide_numbers: false,
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
