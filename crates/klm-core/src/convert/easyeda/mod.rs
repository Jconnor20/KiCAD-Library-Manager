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

/// `response` is the body of EasyEDA's `products/<LCSC id>/components` call.
pub fn convert(response: &Value) -> Result<Converted> {
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
    let fp = footprint::convert(fp_name, r["description"].as_str().unwrap_or(""), &pkg["dataStr"]);
    Ok(Converted {
        symbol: symbol::convert(name, prefix, fp_name, &units),
        footprint: fp.text,
        footprint_name: fp_name.to_string(),
        model_uuid: fp.model_uuid,
        datasheet_url: pkg["dataStr"]["head"]["c_para"]["link"].as_str().unwrap_or("").to_string(),
    })
}
