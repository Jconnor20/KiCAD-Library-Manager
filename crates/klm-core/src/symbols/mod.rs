//! House-style symbols: redraw a part's symbol from a template for its class (resistor, capacitor,
//! inductor, diode, LED, bipolar transistor, generic IC box), whatever the source drew.
//!
//! The pins of the old symbol are moved, never rebuilt, so numbers, names, electrical types, hiding and
//! alternates survive; every property stays too. If any pin can't be placed with certainty the symbol is
//! refused and left exactly as it was. Templates follow KLC and KiCad's own libraries; the user's
//! standardise rules (text size, line width) are applied on top.
//!
//! Restyling moves pin positions: a schematic that already uses the symbol may need its wires fixed.

mod class;
pub mod edit;
pub(crate) mod pintype;
pub mod shapes;
mod template;

pub use class::{Class, detect};
pub use template::{Slot, Template, of as template_of};

use crate::convert::easyeda::geom::fmt;
use crate::kicad_env::write_atomic;
use crate::library::{LibraryRoot, property, sanitize, set_property};
use crate::sexpr::{self, Sexpr};
use crate::standardise::profile::Profile;
use crate::{rules, settings::Settings, standardise};
use anyhow::{Context, Result, anyhow, bail};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::sync::Mutex;

const PITCH: f64 = 2.54;

/// What [`restyle_part`] did.
#[derive(Debug)]
pub struct Restyled {
    pub class: Class,
    pub changes: Vec<String>,
    /// The library file was (or, in a dry run, would be) rewritten.
    pub changed: bool,
}

fn atom(v: f64) -> Sexpr {
    Sexpr::Atom(fmt(v))
}

fn pin_number(p: &Sexpr) -> &str {
    p.child("number").and_then(|n| n.arg(0)).unwrap_or("")
}

fn pin_name(p: &Sexpr) -> &str {
    p.child("name").and_then(|n| n.arg(0)).unwrap_or("")
}

fn pin_type(p: &Sexpr) -> &str {
    p.arg(0).unwrap_or("")
}

fn is_hidden(p: &Sexpr) -> bool {
    p.child("hide").is_some_and(|h| h.arg(0) != Some("no")) || p.items().contains(&Sexpr::Atom("hide".into()))
}

/// "A2" < "A10", "1" < "2" < "10".
fn natural_cmp(a: &str, b: &str) -> Ordering {
    fn chunks(s: &str) -> Vec<(bool, &str)> {
        let mut out = vec![];
        let mut start = 0;
        for (i, c) in s.char_indices() {
            if i > start && c.is_ascii_digit() != s[start..].starts_with(|d: char| d.is_ascii_digit()) {
                out.push((s[start..].starts_with(|d: char| d.is_ascii_digit()), &s[start..i]));
                start = i;
            }
        }
        if start < s.len() {
            out.push((s[start..].starts_with(|d: char| d.is_ascii_digit()), &s[start..]));
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(&cb) {
        let o = match (x.0, y.0) {
            (true, true) => x.1.parse::<u64>().unwrap_or(0).cmp(&y.1.parse::<u64>().unwrap_or(0)).then_with(|| x.1.cmp(y.1)),
            _ => x.1.cmp(y.1),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    ca.len().cmp(&cb.len())
}

/// The pins of a symbol that this module can handle, or why it can't: derived, power, multi-unit,
/// de Morgan and jumper symbols are refused, as are pins without a unique number.
fn source_pins(sym: &Sexpr) -> Result<Vec<Sexpr>> {
    let name = sym.arg(0).context("symbol has no name")?;
    for (head, why) in [
        ("extends", "derived from another symbol"),
        ("power", "a power symbol"),
        ("body_styles", "has alternate body styles"),
        ("unit_name", "has named units"),
        ("jumper_pin_groups", "has jumper pins"),
    ] {
        if sym.child(head).is_some() {
            bail!("{why}");
        }
    }
    if sym.child("duplicate_pin_numbers_are_jumpers").and_then(|j| j.arg(0)) == Some("yes") {
        bail!("has jumper pins");
    }
    let mut pins = vec![];
    for unit in sym.children("symbol") {
        let suffix = unit.arg(0).and_then(|n| n.strip_prefix(name));
        if !matches!(suffix, Some("_0_1" | "_1_1")) {
            bail!("has several units or body styles ({})", unit.arg(0).unwrap_or("?"));
        }
        if unit.child("unit_name").is_some() || unit.child("body_styles").is_some() {
            bail!("has named units or body styles");
        }
        pins.extend(unit.children("pin").cloned());
    }
    if pins.is_empty() {
        bail!("has no pins");
    }
    let mut seen = std::collections::HashSet::new();
    for p in &pins {
        if pin_number(p).is_empty() || !seen.insert(pin_number(p)) {
            bail!("pin number {:?} is missing or used twice", pin_number(p));
        }
        if p.child("at").is_none() || p.child("length").is_none() {
            bail!("pin {} has no position", pin_number(p));
        }
    }
    Ok(pins)
}

/// `pin` with a new position; everything else about it is kept.
fn place(pin: &Sexpr, Slot(x, y, rot, len): Slot) -> Sexpr {
    let mut p = pin.clone();
    for c in p.items_mut() {
        match c.head() {
            Some("at") => *c = Sexpr::list("at", vec![atom(x), atom(y), atom(rot)]),
            Some("length") => *c = Sexpr::list("length", vec![atom(len)]),
            _ => {}
        }
    }
    p
}

/// Names a passive part may have without meaning anything: empty, "~", "1", "P2".
fn trivial_name(n: &str) -> bool {
    let n = n.trim();
    let digits = n.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    n.is_empty() || n == "~" || (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}

/// Pins in slot order for a fixed template, or why the roles can't be told.
fn assign(tpl: &Template, pins: &[Sexpr]) -> Result<Vec<usize>> {
    if pins.len() != tpl.slots.len() {
        bail!("{} pins, the template has {}", pins.len(), tpl.slots.len());
    }
    if tpl.roles.is_empty() {
        if let Some(p) = pins.iter().find(|p| !trivial_name(pin_name(p))) {
            bail!("pin {} is named {:?}, so the two ends may not be interchangeable", pin_number(p), pin_name(p));
        }
        let mut order: Vec<usize> = (0..pins.len()).collect();
        order.sort_by(|&a, &b| natural_cmp(pin_number(&pins[a]), pin_number(&pins[b])));
        return Ok(order);
    }
    let mut order = vec![];
    for names in tpl.roles {
        let hits: Vec<usize> = (0..pins.len()).filter(|&i| names.contains(&pin_name(&pins[i]).trim().to_uppercase().as_str())).collect();
        match hits.as_slice() {
            [i] if !order.contains(i) => order.push(*i),
            _ => bail!("can't tell which pin is {} from the pin names", names[0]),
        }
    }
    Ok(order)
}

fn stroke_fill(fill: &str) -> [Sexpr; 2] {
    [
        Sexpr::list("stroke", vec![Sexpr::list("width", vec![atom(0.254)]), Sexpr::list("type", vec![Sexpr::Atom("default".into())])]),
        Sexpr::list("fill", vec![Sexpr::list("type", vec![Sexpr::Atom(fill.into())])]),
    ]
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

fn side(p: &Sexpr) -> Side {
    let name = pin_name(p).to_uppercase();
    let ground = ["GND", "VSS", "AGND", "DGND", "PGND", "SGND", "VEE", "0V"].iter().any(|g| name.starts_with(g));
    match pin_type(p) {
        "power_in" if ground => Side::Bottom,
        "power_in" => Side::Top,
        "output" | "bidirectional" | "tri_state" | "open_collector" | "open_emitter" | "power_out" => Side::Right,
        _ => Side::Left,
    }
}

/// Name with its index stripped: PA0..PA7 share a group.
fn stem(p: &Sexpr) -> String {
    pin_name(p).to_uppercase().trim_end_matches(|c: char| c.is_ascii_digit() || "_[]{}-./ ".contains(c)).to_string()
}

/// Pins of one side in order, grouped by function; `None` is the gap between two groups.
fn grouped(pins: &[Sexpr], mut idx: Vec<usize>, gaps: bool) -> Vec<Option<usize>> {
    idx.sort_by(|&a, &b| natural_cmp(pin_number(&pins[a]), pin_number(&pins[b])));
    let mut groups: Vec<(String, Vec<usize>)> = vec![];
    for i in idx {
        match groups.iter_mut().find(|g| g.0 == stem(&pins[i])) {
            Some(g) => g.1.push(i),
            None => groups.push((stem(&pins[i]), vec![i])),
        }
    }
    let mut out = vec![];
    for (n, (_, mut g)) in groups.into_iter().enumerate() {
        g.sort_by(|&a, &b| natural_cmp(pin_name(&pins[a]), pin_name(&pins[b])).then_with(|| natural_cmp(pin_number(&pins[a]), pin_number(&pins[b]))));
        if gaps && n > 0 {
            out.push(None);
        }
        out.extend(g.into_iter().map(Some));
    }
    out
}

/// A box that fits the pins: inputs left, outputs right, supply on top, ground below.
/// Hidden pins go on their own points below the box, so they never touch a visible pin.
fn ic_box(pins: &[Sexpr]) -> (Vec<Sexpr>, Vec<Sexpr>) {
    let pick = |s: Side| (0..pins.len()).filter(|&i| !is_hidden(&pins[i]) && side(&pins[i]) == s).collect::<Vec<_>>();
    let (left, right) = (grouped(pins, pick(Side::Left), true), grouped(pins, pick(Side::Right), true));
    let (top, bottom) = (grouped(pins, pick(Side::Top), false), grouped(pins, pick(Side::Bottom), false));
    let hidden: Vec<usize> = (0..pins.len()).filter(|&i| is_hidden(&pins[i])).collect();
    let longest = |v: &[Option<usize>]| v.iter().flatten().map(|&i| pin_name(&pins[i]).chars().count()).max().unwrap_or(0) as f64;
    let rows = left.len().max(right.len()).max(1);
    let cols = top.len().max(bottom.len()).max(1);
    // Half the width: room for both name columns and for the top and bottom pins, on the grid.
    let by_names = (longest(&left) + longest(&right)) * 1.016 + PITCH;
    let hw = ((by_names / 2.0 / PITCH).ceil() * PITCH).max((((cols + 1) as f64 * PITCH) / 2.0 / PITCH).ceil() * PITCH).max(2.0 * PITCH);
    let y_first = PITCH * ((rows - 1) / 2) as f64;
    let (y_top, y_bottom) = (y_first + PITCH, y_first - (rows - 1) as f64 * PITCH - PITCH);
    let x_first = -PITCH * ((cols - 1) / 2) as f64;
    let mut out = vec![];
    for (i, p) in left.iter().enumerate() {
        out.extend(p.map(|p| place(&pins[p], Slot(-hw - PITCH, y_first - i as f64 * PITCH, 0.0, PITCH))));
    }
    for (i, p) in right.iter().enumerate() {
        out.extend(p.map(|p| place(&pins[p], Slot(hw + PITCH, y_first - i as f64 * PITCH, 180.0, PITCH))));
    }
    for (i, p) in top.iter().enumerate() {
        out.extend(p.map(|p| place(&pins[p], Slot(x_first + i as f64 * PITCH, y_top + PITCH, 270.0, PITCH))));
    }
    for (i, p) in bottom.iter().enumerate() {
        out.extend(p.map(|p| place(&pins[p], Slot(x_first + i as f64 * PITCH, y_bottom - PITCH, 90.0, PITCH))));
    }
    // A hidden pin stacked on a visible one (an exposed pad on GND) must stay on that pin's net; the
    // rest get points of their own, clear of every wire a visible pin could have.
    let at = |p: &Sexpr| p.child("at").and_then(|a| Some((a.arg(0)?.to_string(), a.arg(1)?.to_string())));
    let (mut spare, row_x) = (0, hw.max(x_first + (cols - 1) as f64 * PITCH) + 2.0 * PITCH);
    for &p in &hidden {
        // Pin numbers are unique, so the twin's new place is found by number.
        let twin = (0..pins.len()).filter(|i| !hidden.contains(i)).find(|&v| at(&pins[v]).is_some() && at(&pins[v]) == at(&pins[p]));
        let twin_new = twin.and_then(|v| out.iter().find(|o| pin_number(o) == pin_number(&pins[v]))).cloned();
        let mut moved = place(&pins[p], Slot(row_x + spare as f64 * PITCH, y_bottom - 2.0 * PITCH, 90.0, PITCH));
        match twin_new {
            Some(t) => {
                for c in moved.items_mut() {
                    if let Some(h) = c.head().filter(|h| matches!(*h, "at" | "length"))
                        && let Some(n) = t.child(h)
                    {
                        *c = n.clone();
                    }
                }
            }
            None => spare += 1,
        }
        out.push(moved);
    }
    let [stroke, fill] = stroke_fill("background");
    let rect =
        Sexpr::list("rectangle", vec![Sexpr::list("start", vec![atom(-hw), atom(y_top)]), Sexpr::list("end", vec![atom(hw), atom(y_bottom)]), stroke, fill]);
    (vec![rect], out)
}

/// Pins and body graphics from `tpl`, or an IC box when there is none.
fn layout(tpl: Option<&Template>, pins: &[Sexpr]) -> Result<(Vec<Sexpr>, Vec<Sexpr>)> {
    let Some(tpl) = tpl else { return Ok(ic_box(pins)) };
    let order = assign(tpl, pins)?;
    let placed = order.iter().zip(tpl.slots.iter()).map(|(&i, s)| place(&pins[i], Slot(s.0, s.1, s.2, s.3))).collect();
    let body = sexpr::parse(&format!("(g {})", tpl.body)).context("template graphics are not valid")?.items()[1..].to_vec();
    Ok((body, placed))
}

/// The symbol rebuilt: same properties and flags, new unit sub-symbols and pin display options.
fn rebuild(sym: &Sexpr, tpl: Option<&Template>, graphics: Vec<Sexpr>, pins: Vec<Sexpr>) -> Sexpr {
    let name = sym.arg(0).unwrap_or_default();
    let unit = |suffix: &str, items: Vec<Sexpr>| {
        let mut v = vec![Sexpr::str(&format!("{name}{suffix}"))];
        v.extend(items);
        Sexpr::list("symbol", v)
    };
    let units = [unit("_0_1", graphics), unit("_1_1", pins)];
    let mut head = vec![];
    if tpl.is_some_and(|t| t.hide_numbers) {
        head.push(Sexpr::list("pin_numbers", vec![Sexpr::list("hide", vec![Sexpr::Atom("yes".into())])]));
    }
    head.push(match tpl {
        Some(_) => Sexpr::list("pin_names", vec![Sexpr::list("offset", vec![atom(0.0)]), Sexpr::list("hide", vec![Sexpr::Atom("yes".into())])]),
        None => Sexpr::list("pin_names", vec![Sexpr::list("offset", vec![atom(1.016)])]),
    });
    let mut items = vec![sym.items()[0].clone(), sym.items()[1].clone()];
    items.extend(head);
    let mut placed = false;
    let old: Vec<Sexpr> = sym.items()[2..].to_vec();
    for c in old.iter().filter(|c| !matches!(c.head(), Some("pin_names" | "pin_numbers"))) {
        match c.head() {
            Some("symbol") if !placed => {
                items.extend(units.clone());
                placed = true;
            }
            Some("symbol") => {}
            Some("embedded_fonts" | "embedded_files") if !placed => {
                items.extend(units.clone());
                placed = true;
                items.push(c.clone());
            }
            _ => items.push(c.clone()),
        }
    }
    if !placed {
        items.extend(units);
    }
    Sexpr::List(items)
}

/// Pins as the verification compares them: everything but where they sit and how long they are.
fn identity(pins: &[Sexpr]) -> Vec<String> {
    let mut v: Vec<String> = pins
        .iter()
        .map(|p| {
            let mut p = p.clone();
            p.items_mut().retain(|c| !matches!(c.head(), Some("at" | "length")));
            sexpr::to_string(&p)
        })
        .collect();
    v.sort();
    v
}

/// The profile's house template for `class` (drawn by the user), if it has one.
pub fn house_template(profile: &Profile, class: Class) -> Result<Option<Template>> {
    let Some(text) = profile.templates.get(&format!("{class:?}")) else { return Ok(None) };
    let t = sexpr::parse(text).and_then(|s| Template::from_symbol(&s, class));
    t.map(Some).with_context(|| format!("the {class:?} house template of profile {} can't be used", profile.name))
}

/// Redraws `sym` in the house style of `class`: the profile's template for it, else the built-in one.
/// Pure: the refusal says why, and nothing is half-done.
pub fn restyle(sym: &Sexpr, class: Class, profile: &Profile) -> Result<Sexpr> {
    let pins = source_pins(sym)?;
    let custom = house_template(profile, class)?;
    let tpl = custom.as_ref().or(template::of(class));
    let (graphics, placed) = layout(tpl, &pins)?;
    let mut new = rebuild(sym, tpl, graphics, placed);
    // Every pin must have come across exactly as it was.
    let after = source_pins(&new).context("the restyled symbol is not valid")?;
    if identity(&after) != identity(&pins) {
        bail!("not every pin could be mapped across");
    }
    if property(&new, "Reference").is_some_and(|r| r != class.prefix()) {
        set_property(&mut new, "Reference", class.prefix());
    }
    standardise::symbol::field_positions(&mut new);
    standardise::symbol::fix(&mut new, &profile.rules);
    // The house template's field positions win over the generic above/below rule.
    for (key, (x, y, r)) in ["Reference", "Value"].into_iter().zip(tpl.and_then(|t| t.fields).into_iter().flatten()) {
        let prop = new.items_mut().iter_mut().find(|p| p.head() == Some("property") && p.arg(0) == Some(key));
        let Some(prop) = prop else { continue };
        // Text to the right of the body reads from the body outwards, as in KiCad's Device library.
        standardise::symbol::justify(prop, x > 0.0 && r == 0.0);
        if let Some(at) = prop.items_mut().iter_mut().find(|c| c.head() == Some("at")) {
            *at = Sexpr::list("at", vec![atom(x), atom(y), atom(r)]);
        }
    }
    if let Some(sizes) = tpl.and_then(|t| t.field_sizes) {
        for (key, size) in ["Reference", "Value"].into_iter().zip(sizes) {
            if let Some(prop) = new.items_mut().iter_mut().find(|p| p.head() == Some("property") && p.arg(0) == Some(key)) {
                standardise::symbol::set_font_size(prop, size);
            }
        }
    }
    Ok(new)
}

/// The house-style class of `sym`, a symbol in the `category` library, or why it has none.
pub fn class_of(category: &str, sym: &Sexpr, naming: &rules::Rules) -> Result<Class> {
    let pins = source_pins(sym)?;
    detect(category, sym, pins.len(), naming).map_err(anyhow::Error::msg)
}

/// The house-style class of every part in `root` that has one, by (category, symbol name). Parts with no
/// class (a potentiometer, an unclear IC) are left out. Reads the symbol libraries only.
pub fn classes_in(root: &LibraryRoot, naming: &rules::Rules) -> std::collections::HashMap<(String, String), Class> {
    let mut out = std::collections::HashMap::new();
    for cat in root.categories() {
        let Some(lib) = std::fs::read_to_string(lib_path(root, &cat)).ok().and_then(|t| sexpr::parse(&t).ok()) else { continue };
        for sym in lib.children("symbol") {
            if let (Some(name), Ok(class)) = (sym.arg(0), class_of(&cat, sym, naming)) {
                out.insert((cat.clone(), name.to_string()), class);
            }
        }
    }
    out
}

fn lib_path(root: &LibraryRoot, category: &str) -> std::path::PathBuf {
    root.dir.join("symbols").join(format!("{}.kicad_sym", sanitize(category)))
}

/// Restyles one part's symbol in place, after a snapshot (so History can undo it). With `write` false it
/// only works out what would happen. A refusal is an error and leaves the library untouched.
pub fn restyle_part(root: &LibraryRoot, category: &str, name: &str, naming: &rules::Rules, profile: &Profile, write: bool) -> Result<Restyled> {
    let path = lib_path(root, category);
    let text = std::fs::read_to_string(&path).with_context(|| format!("no library {}", root.nickname(category)))?;
    let mut lib = sexpr::parse(&text)?;
    let idx = lib
        .items()
        .iter()
        .position(|c| c.head() == Some("symbol") && c.arg(0) == Some(name))
        .with_context(|| format!("{name} is not in {}", root.nickname(category)))?;
    if lib.children("symbol").any(|s| s.child("extends").and_then(|e| e.arg(0)) == Some(name)) {
        bail!("other symbols in {} are derived from {name}; restyling it would change them", root.nickname(category));
    }
    let sym = lib.items()[idx].clone();
    let count = source_pins(&sym).map_err(|e| anyhow!("{name} not restyled: {e:#}"))?.len();
    let class = detect(category, &sym, count, naming).map_err(|e| anyhow!("{name} not restyled: {e}"))?;
    let new = restyle(&sym, class, profile).map_err(|e| anyhow!("{name} not restyled: {e:#}"))?;
    if new == sym {
        return Ok(Restyled { class, changes: vec![format!("already the {class:?} house style")], changed: false });
    }
    let mut changes = vec![format!("redrawn as {class:?}")];
    if property(&sym, "Reference") != property(&new, "Reference") {
        changes.push(format!("Reference {} -> {}", property(&sym, "Reference").unwrap_or(""), class.prefix()));
    }
    if write {
        let _snapshot = crate::snapshot::guard(root, &format!("restyle {name}"))?;
        if std::fs::read_to_string(&path)? != text {
            bail!("{} changed meanwhile; try again", path.display());
        }
        lib.items_mut()[idx] = new;
        write_atomic(&path, &sexpr::to_string(&lib))?;
    }
    Ok(Restyled { class, changes, changed: true })
}

/// Makes a part the profile's house template for its class: its symbol, and the style of its footprint
/// when that is in the library. Returns the class and whether a footprint style was taken. The caller saves.
pub fn set_house_template(profile: &mut Profile, root: &LibraryRoot, category: &str, name: &str, naming: &rules::Rules) -> Result<(Class, bool)> {
    let lib = sexpr::parse(&std::fs::read_to_string(lib_path(root, category)).with_context(|| format!("no library {}", root.nickname(category)))?)?;
    let sym = lib.children("symbol").find(|s| s.arg(0) == Some(name)).with_context(|| format!("{name} is not in {}", root.nickname(category)))?;
    let class = class_of(category, sym, naming).map_err(|e| anyhow!("{name} can't be a house template: {e:#}"))?;
    let key = format!("{class:?}");
    // IC boxes are drawn to fit their pins, so an IC only gives its footprint style.
    if class != Class::Ic {
        Template::from_symbol(sym, class).map_err(|e| anyhow!("{name} can't be the {class:?} house template: {e:#}"))?;
        profile.templates.insert(key.clone(), sexpr::to_string(sym));
    }
    // The footprint is looked up by name in every footprint library of this root.
    let fp_name = property(sym, "Footprint").map(|f| f.rsplit(':').next().unwrap_or(f).to_string());
    let style = fp_name.and_then(|f| {
        let dirs = std::fs::read_dir(root.dir.join("footprints")).ok()?;
        let text = dirs.flatten().find_map(|d| std::fs::read_to_string(d.path().join(format!("{f}.kicad_mod"))).ok())?;
        standardise::FootprintStyle::from_footprint(&sexpr::parse(&text).ok()?).ok()
    });
    let took = style.is_some();
    match style {
        Some(style) => _ = profile.footprint_styles.insert(key, style),
        None if class == Class::Ic => bail!("{name} is an IC, whose symbols are drawn to fit their pins, and its footprint isn't in this library"),
        None => {}
    }
    Ok((class, took))
}

struct Config {
    naming: rules::Rules,
    profile: Profile,
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

thread_local! {
    static NOTE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Turns the import step on or off from the settings (`house_symbols_on_import`); call when they load or change.
pub fn configure(s: &Settings) {
    crate::fields::configure(&s.standard);
    *CONFIG.lock().unwrap_or_else(|e| e.into_inner()) = s.house_symbols_on_import.then(|| Config { naming: s.rules.clone(), profile: s.standard.clone() });
}

/// Import step: restyles the symbol about to be written to the `category` library, if the setting is on.
/// A refusal keeps the vendor's symbol. Either way [`take_note`] says what happened.
pub(crate) fn restyle_imported(category: &str, symbol: &mut Sexpr) {
    NOTE.with(|n| *n.borrow_mut() = None);
    let (naming, profile) = match CONFIG.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some(c) => (c.naming.clone(), c.profile.clone()),
        None => return,
    };
    // ICs keep the vendor's drawing: redrawing them to fit their pins mangles real parts.
    if class_of(category, symbol, &naming).is_ok_and(|c| c == Class::Ic) {
        NOTE.with(|n| *n.borrow_mut() = Some("Kept the vendor symbol (ICs are not restyled on import)".into()));
        return;
    }
    let note = match class_of(category, symbol, &naming).and_then(|class| restyle(symbol, class, &profile).map(|new| (class, new))) {
        Ok((class, new)) => {
            *symbol = new;
            format!("Redrew the symbol as {class:?} (house style)")
        }
        Err(e) => format!("Kept the vendor symbol (house style not applied: {e:#})"),
    };
    NOTE.with(|n| *n.borrow_mut() = Some(note));
}

/// The note the last import on this thread left, if the import step ran.
pub fn take_note() -> Option<String> {
    NOTE.with(|n| n.borrow_mut().take())
}

#[cfg(test)]
mod tests;
