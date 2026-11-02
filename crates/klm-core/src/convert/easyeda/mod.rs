//! EasyEDA / JLCEDA component JSON → KiCad symbol and footprint. Written from the format's public
//! structure (shape strings of `~`-separated fields in 10 mil units), not from any other converter.

mod footprint;
mod geom;
mod symbol;

use anyhow::{Context, Result, bail};
use serde_json::Value;

pub struct Converted {
    pub symbol: String,
    pub footprint: String,
    pub footprint_name: String,
    /// Fetch the STEP file for this id from EasyEDA's model server.
    pub model_uuid: Option<String>,
    /// Datasheet link the footprint record carries, if any.
    pub datasheet_url: String,
}

/// `response` is the body of EasyEDA's `products/<LCSC id>/components` call. `description` and
/// `datasheet` (from the search result, which is richer) fill the footprint's chooser entry;
/// pass "" to fall back to what EasyEDA itself has.
pub fn convert(response: &Value, description: &str, datasheet: &str) -> Result<Converted> {
    if response["success"] == false {
        bail!("EasyEDA: {}", response["message"].as_str().unwrap_or("request failed"));
    }
    let r = response.get("result").filter(|r| r.is_object()).context("EasyEDA has no data for this part")?;
    let sym_ds = &r["dataStr"];
    let pkg = &r["packageDetail"];
    let para = &sym_ds["head"]["c_para"];
    let name = para["name"].as_str().or(r["title"].as_str()).context("component has no name")?;
    let prefix = para["pre"].as_str().unwrap_or("U").split(['?', '.']).next().filter(|p| !p.is_empty()).unwrap_or("U");
    let fp_name = pkg["title"].as_str().context("component has no footprint")?;

    // Multi-unit symbols keep their drawings in `subparts`; the top-level shape list is then empty.
    let units: Vec<&Value> = match r["subparts"].as_array().filter(|s| !s.is_empty()) {
        Some(parts) => parts.iter().map(|p| &p["dataStr"]).collect(),
        None => vec![sym_ds],
    };
    let pkg_link = pkg["dataStr"]["head"]["c_para"]["link"].as_str().unwrap_or("");
    let pick = |a: &'_ str, b: &'_ str| if a.is_empty() { b.to_string() } else { a.to_string() };
    let tags = format!("{name} {fp_name} {} {}", para["Supplier"].as_str().unwrap_or(""), para["Supplier Part"].as_str().unwrap_or(""));
    let docs = footprint::Docs {
        description: &pick(description, r["description"].as_str().filter(|d| !d.is_empty()).unwrap_or(name)),
        tags: tags.trim(),
        datasheet: &pick(datasheet, pkg_link),
    };
    let fp = footprint::convert(fp_name, &docs, &pkg["dataStr"]);
    Ok(Converted {
        symbol: symbol::convert(name, prefix, fp_name, &units),
        footprint: fp.text,
        footprint_name: fp_name.to_string(),
        model_uuid: fp.model_uuid,
        datasheet_url: pkg_link.to_string(),
    })
}
