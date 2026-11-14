//! Symbol, footprint and SPICE helpers shared by the library features.

use super::sanitize;
use crate::kicad_env::write_atomic;
use crate::sexpr::{self, Sexpr};
use anyhow::Result;
use std::path::Path;

/// Adds `symbol` to the library file at `path`, replacing any symbol of that name. A new file takes
/// its header (version, generator) from `header_from` so formats match.
pub(super) fn merge_symbol(path: &Path, header_from: &Sexpr, symbol: Sexpr) -> Result<()> {
    let mut lib = if path.exists() {
        sexpr::parse(&std::fs::read_to_string(path)?)?
    } else {
        Sexpr::List(header_from.items().iter().filter(|c| c.head() != Some("symbol")).cloned().collect())
    };
    let name = symbol.arg(0).unwrap_or_default().to_string();
    lib.items_mut().retain(|c| !(c.head() == Some("symbol") && c.arg(0) == Some(name.as_str())));
    lib.items_mut().push(symbol);
    write_atomic(path, &sexpr::to_string(&lib))
}

pub(super) fn datasheet_name(manufacturer: &str, mpn: &str) -> String {
    format!("{}.pdf", sanitize(&format!("{manufacturer}_{mpn}")).trim_start_matches('_'))
}

pub fn property<'a>(symbol: &'a Sexpr, key: &str) -> Option<&'a str> {
    symbol.children("property").find(|p| p.arg(0) == Some(key)).and_then(|p| p.arg(1))
}

/// Sets a symbol field, adding it as a hidden field if missing.
pub fn set_property(symbol: &mut Sexpr, key: &str, value: &str) {
    let items = symbol.items_mut();
    if let Some(p) = items.iter_mut().find(|p| p.head() == Some("property") && p.arg(0) == Some(key)) {
        p.set_arg(1, Sexpr::str(value));
        return;
    }
    let new = Sexpr::list(
        "property",
        vec![
            Sexpr::str(key),
            Sexpr::str(value),
            Sexpr::list("at", vec![Sexpr::Atom("0".into()), Sexpr::Atom("0".into()), Sexpr::Atom("0".into())]),
            Sexpr::list("hide", vec![Sexpr::Atom("yes".into())]),
            Sexpr::list(
                "effects",
                vec![Sexpr::list("font", vec![Sexpr::list("size", vec![Sexpr::Atom("1.27".into()), Sexpr::Atom("1.27".into())])])],
            ),
        ],
    );
    let after_last_prop = items.iter().rposition(|p| p.head() == Some("property")).map_or(items.len(), |i| i + 1);
    items.insert(after_last_prop, new);
}

/// Renames a symbol and its unit sub-symbols (`OLD_1_1` → `NEW_1_1`).
pub(crate) fn rename_symbol(symbol: &mut Sexpr, new_name: &str) {
    let old = symbol.arg(0).unwrap_or_default().to_string();
    symbol.set_arg(0, Sexpr::str(new_name));
    for unit in symbol.items_mut().iter_mut().filter(|c| c.head() == Some("symbol")) {
        if let Some(suffix) = unit.arg(0).and_then(|n| n.strip_prefix(old.as_str())).map(String::from) {
            unit.set_arg(0, Sexpr::str(&format!("{new_name}{suffix}")));
        }
    }
}

/// Renames a footprint and points its 3D model at `model_uri`, keeping the vendor's offset/rotation.
pub(super) fn set_footprint(fp: &mut Sexpr, name: &str, model_uri: Option<&str>) {
    fp.set_arg(0, Sexpr::str(name));
    let Some(uri) = model_uri else { return };
    let items = fp.items_mut();
    match items.iter().position(|c| c.head() == Some("model")) {
        Some(first) => {
            items[first].set_arg(0, Sexpr::str(uri));
            let mut i = 0;
            items.retain(|c| {
                i += 1;
                i - 1 == first || c.head() != Some("model")
            });
        }
        None => {
            let xyz = |v: &str| Sexpr::list("xyz", vec![Sexpr::Atom(v.into()), Sexpr::Atom(v.into()), Sexpr::Atom(v.into())]);
            items.push(Sexpr::list(
                "model",
                vec![
                    Sexpr::str(uri),
                    Sexpr::list("offset", vec![xyz("0")]),
                    Sexpr::list("scale", vec![xyz("1")]),
                    Sexpr::list("rotate", vec![xyz("0")]),
                ],
            ));
        }
    }
}

/// Equal apart from UUIDs, which `kicad-cli fp upgrade` regenerates on every run.
pub(super) fn same_geometry(a: &str, b: &str) -> bool {
    fn strip(x: &mut Sexpr) {
        if let Sexpr::List(v) = x {
            v.retain(|c| !matches!(c.head(), Some("uuid" | "tstamp")));
            v.iter_mut().for_each(strip);
        }
    }
    match (sexpr::parse(a), sexpr::parse(b)) {
        (Ok(mut a), Ok(mut b)) => {
            strip(&mut a);
            strip(&mut b);
            a == b
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_and_fields() {
        let mut s = sexpr::parse(r#"(symbol "old" (property "Value" "old") (symbol "old_0_1") (symbol "old_1_1"))"#).unwrap();
        rename_symbol(&mut s, "NEW");
        set_property(&mut s, "Value", "NEW");
        set_property(&mut s, "MPN", "NEW");
        assert_eq!(s.arg(0), Some("NEW"));
        let units: Vec<_> = s.children("symbol").filter_map(|u| u.arg(0)).collect();
        assert_eq!(units, ["NEW_0_1", "NEW_1_1"]);
        assert_eq!(property(&s, "Value"), Some("NEW"));
        assert_eq!(property(&s, "MPN"), Some("NEW"));
        // New field sits after the existing properties, before the units.
        assert_eq!(s.items()[3].head(), Some("property"));
    }

    #[test]
    fn footprint_model_path_keeps_offsets() {
        let mut fp = sexpr::parse(r#"(footprint "X" (model "a.wrl" (offset (xyz 0 0 1))) (model "b.step"))"#).unwrap();
        set_footprint(&mut fp, "Y", Some("${KLM_LIB}/3dmodels/ICs.3dshapes/Y.step"));
        let models: Vec<_> = fp.children("model").collect();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].arg(0), Some("${KLM_LIB}/3dmodels/ICs.3dshapes/Y.step"));
        assert!(sexpr::to_string(models[0]).contains("xyz 0 0 1"));
        assert_eq!(fp.arg(0), Some("Y"));
    }

}
