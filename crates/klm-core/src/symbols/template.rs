//! The house templates: body graphics as KiCad s-expressions (geometry follows KiCad's own Device and
//! Transistor_BJT libraries, KLC S3/S4) plus where each pin slot sits. Slot order is the role order of
//! `Template::roles`. Every built-in connection point is on the 2.54 mm (100 mil) grid, so wires meet the pins.

use super::class::Class;
use super::{assign, atom, is_hidden, pin_number, source_pins};
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
    /// Fields to the right of the body are left-justified.
    pub fields: Option<[(f64, f64, f64); 2]>,
    /// Text height of Reference and Value, mm; `None` leaves it to the standardise rules.
    pub field_sizes: Option<[f64; 2]>,
}

const R: Template = Template {
    body: Cow::Borrowed("(rectangle (start -1.016 -2.54) (end 1.016 2.54) (stroke (width 0.254) (type default)) (fill (type none)))"),
    slots: Cow::Borrowed(&[Slot(0.0, 5.08, 270.0, 2.54), Slot(0.0, -5.08, 90.0, 2.54)]),
    roles: &[],
    hide_numbers: true,
    fields: Some([(2.54, 1.27, 0.0), (2.54, -1.27, 0.0)]),
    field_sizes: None,
};

const C: Template = Template {
    body: Cow::Borrowed(
        "(polyline (pts (xy -2.032 0.762) (xy 2.032 0.762)) (stroke (width 0.508) (type default)) (fill (type none)))
           (polyline (pts (xy -2.032 -0.762) (xy 2.032 -0.762)) (stroke (width 0.508) (type default)) (fill (type none)))",
    ),
    slots: Cow::Borrowed(&[Slot(0.0, 5.08, 270.0, 4.064), Slot(0.0, -5.08, 90.0, 4.064)]),
    roles: &[],
    hide_numbers: true,
    fields: Some([(3.048, 1.27, 0.0), (3.048, -1.27, 0.0)]),
    field_sizes: None,
};

const L: Template = Template {
    body: Cow::Borrowed(
        "(arc (start 0 2.54) (mid 0.6323 1.905) (end 0 1.27) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 1.27) (mid 0.6323 0.635) (end 0 0) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 0) (mid 0.6323 -0.635) (end 0 -1.27) (stroke (width 0.254) (type default)) (fill (type none)))
           (arc (start 0 -1.27) (mid 0.6323 -1.905) (end 0 -2.54) (stroke (width 0.254) (type default)) (fill (type none)))",
    ),
    slots: Cow::Borrowed(&[Slot(0.0, 5.08, 270.0, 2.54), Slot(0.0, -5.08, 90.0, 2.54)]),
    roles: &[],
    hide_numbers: true,
    fields: Some([(2.54, 1.27, 0.0), (2.54, -1.27, 0.0)]),
    field_sizes: None,
};

const DIODE_BODY: &str = "(polyline (pts (xy -1.27 1.27) (xy -1.27 -1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 1.27 1.27) (xy 1.27 -1.27) (xy -1.27 0) (xy 1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
    (polyline (pts (xy 1.27 0) (xy -1.27 0)) (stroke (width 0.254) (type default)) (fill (type none)))";

/// Slot 0 is the cathode (the bar), slot 1 the anode.
const DIODE_SLOTS: &[Slot] = &[Slot(-2.54, 0.0, 0.0, 1.27), Slot(2.54, 0.0, 180.0, 1.27)];
const DIODE_ROLES: &[&[&str]] = &[&["K", "C", "CATHODE", "-"], &["A", "ANODE", "+"]];

const D: Template =
    Template { body: Cow::Borrowed(DIODE_BODY), slots: Cow::Borrowed(DIODE_SLOTS), roles: DIODE_ROLES, hide_numbers: true, fields: None, field_sizes: None };

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
    field_sizes: None,
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
    field_sizes: None,
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
    field_sizes: None,
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
            let (Some(at), Some(len)) = (pins[i].child("at"), pins[i].child("length")) else { bail!("pin {} has no position or length", pin_number(&pins[i])) };
            let (Some(x), Some(y), Some(l)) = (num(at, 0), num(at, 1), num(len, 0)) else { bail!("pin {} has a bad position", pin_number(&pins[i])) };
            slots.push(Slot(x, y, num(at, 2).unwrap_or(0.0), l));
        }
        let body: Vec<String> =
            sym.children("symbol").flat_map(|u| u.items().iter().skip(2)).filter(|c| c.head() != Some("pin")).map(sexpr::to_string).collect();
        if body.is_empty() {
            bail!("has no body graphics");
        }
        let field = |k: &str| {
            let at = sym.children("property").find(|p| p.arg(0) == Some(k))?.child("at")?;
            Some((num(at, 0)?, num(at, 1)?, num(at, 2).unwrap_or(0.0)))
        };
        let size = |k: &str| num(sym.children("property").find(|p| p.arg(0) == Some(k))?.child("effects")?.child("font")?.child("size")?, 0);
        Ok(Template {
            body: Cow::Owned(body.join("\n")),
            slots: Cow::Owned(slots),
            roles: built_in.roles,
            hide_numbers: sym.child("pin_numbers").is_some_and(is_hidden),
            fields: field("Reference").zip(field("Value")).map(|(r, v)| [r, v]),
            field_sizes: size("Reference").zip(size("Value")).map(|(r, v)| [r, v]),
        })
    }

    /// This template as a house-template symbol, the form `from_symbol` reads back and a profile stores.
    /// Pins are named after their roles (K, A, B, C, E) so the roles can be told again.
    pub fn to_symbol(&self, class: Class) -> Result<Sexpr> {
        let name = format!("House_{class:?}");
        let hide = || Sexpr::list("hide", vec![Sexpr::Atom("yes".into())]);
        let mut items = vec![Sexpr::str(&name)];
        if self.hide_numbers {
            items.push(Sexpr::list("pin_numbers", vec![hide()]));
        }
        items.push(Sexpr::list("pin_names", vec![Sexpr::list("offset", vec![atom(0.0)]), hide()]));
        for (key, f) in ["Reference", "Value"].into_iter().zip(self.fields.iter().flatten()) {
            let text = if key == "Reference" { class.prefix() } else { name.as_str() };
            let mut prop = vec![Sexpr::str(key), Sexpr::str(text), Sexpr::list("at", vec![atom(f.0), atom(f.1), atom(f.2)])];
            if let Some(size) = self.field_sizes.map(|s| s[(key == "Value") as usize]) {
                prop.push(Sexpr::list("effects", vec![Sexpr::list("font", vec![Sexpr::list("size", vec![atom(size), atom(size)])])]));
            }
            items.push(Sexpr::list("property", prop));
        }
        let body = sexpr::parse(&format!("(g {})", self.body)).map_err(|e| anyhow!("template graphics are not valid: {e:#}"))?;
        items.push(Sexpr::list("symbol", [vec![Sexpr::str(&format!("{name}_0_1"))], body.items()[1..].to_vec()].concat()));
        let pins = self.slots.iter().enumerate().map(|(i, s)| {
            let pin_name = self.roles.get(i).and_then(|r| r.first()).copied().unwrap_or("~");
            Sexpr::list(
                "pin",
                vec![
                    Sexpr::Atom("passive".into()),
                    Sexpr::Atom("line".into()),
                    Sexpr::list("at", vec![atom(s.0), atom(s.1), atom(s.2)]),
                    Sexpr::list("length", vec![atom(s.3)]),
                    Sexpr::list("name", vec![Sexpr::str(pin_name)]),
                    Sexpr::list("number", vec![Sexpr::str(&(i + 1).to_string())]),
                ],
            )
        });
        items.push(Sexpr::list("symbol", [vec![Sexpr::str(&format!("{name}_1_1"))], pins.collect()].concat()));
        Ok(Sexpr::List([vec![Sexpr::Atom("symbol".into())], items].concat()))
    }

    /// The body as editable shapes.
    pub fn shapes(&self) -> Vec<super::shapes::Shape> {
        super::shapes::parse(&self.body)
    }

    pub fn set_shapes(&mut self, shapes: &[super::shapes::Shape]) {
        self.body = Cow::Owned(super::shapes::to_body(shapes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_survives_being_written_as_a_house_symbol() {
        for class in [Class::Resistor, Class::Capacitor, Class::Diode, Class::Npn] {
            let mut t = of(class).unwrap().clone();
            t.fields = Some([(1.27, 2.54, 0.0), (1.27, -2.54, 90.0)]);
            t.slots.to_mut()[0].1 += 2.54;
            let back = Template::from_symbol(&t.to_symbol(class).unwrap(), class).unwrap();
            assert_eq!((back.slots.to_vec(), back.fields, back.hide_numbers), (t.slots.to_vec(), t.fields, t.hide_numbers), "{class:?}");
        }
    }

    #[test]
    fn edited_shapes_become_the_body() {
        let mut t = of(Class::Resistor).unwrap().clone();
        let mut shapes = t.shapes();
        shapes[0].translate((0.0, 1.0));
        t.set_shapes(&shapes);
        assert_eq!(t.shapes(), shapes);
    }
}
