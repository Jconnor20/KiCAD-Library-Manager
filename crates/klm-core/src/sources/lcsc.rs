//! LCSC / JLCPCB parts via two unofficial endpoints: JLCPCB's parts search (metadata) and
//! EasyEDA's component API (CAD data). All URLs live here so a dead or disallowed endpoint is a
//! one-file change.

use super::Source;
use crate::convert::easyeda;
use crate::download::download_pdf;
use crate::part::Part;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

const SEARCH_URL: &str = "https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList";
const COMPONENT_URL: &str = "https://easyeda.com/api/products/{id}/components?version=6.4.19.5";
const STEP_URL: &str = "https://modules.easyeda.com/qAxj6KHrDKw4blvCG8QJPs7Y/{uuid}";
// The CDNs in front of these hosts reject generic browser-looking clients, so say who we are.
const USER_AGENT: &str = concat!("KiCad-Library-Manager/", env!("CARGO_PKG_VERSION"));
const SEARCH_LIMIT: usize = 25;

pub struct Lcsc {
    http: reqwest::blocking::Client,
}

impl Lcsc {
    pub fn new() -> Self {
        let http = reqwest::blocking::Client::builder().user_agent(USER_AGENT).timeout(Duration::from_secs(60)).build().expect("HTTP client");
        Lcsc { http }
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.http.get(url).send().with_context(|| format!("requesting {url}"))?;
        if !resp.status().is_success() {
            let hint = if matches!(resp.status().as_u16(), 403 | 429) { " (the server is refusing requests, likely rate limiting; try again in a few minutes)" } else { "" };
            bail!("{url}: HTTP {}{hint}", resp.status());
        }
        Ok(resp.bytes()?.to_vec())
    }

}

/// Saves the first candidate that really is a PDF (some "datasheet" links are HTML viewer pages);
/// the error lists why each one failed.
fn download_first_pdf(candidates: &[&str], dest: &Path) -> Result<()> {
    let mut why = vec![];
    for url in candidates.iter().filter(|u| u.starts_with("http")) {
        match download_pdf(url, dest) {
            Ok(()) => return Ok(()),
            Err(e) => why.push(format!("{e:#}")),
        }
    }
    bail!("{}", if why.is_empty() { "no datasheet link".into() } else { why.join("; ") })
}

impl Default for Lcsc {
    fn default() -> Self {
        Self::new()
    }
}

fn is_lcsc_id(q: &str) -> bool {
    q.len() > 1 && q.starts_with(['C', 'c']) && q[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Results of JLCPCB's search endpoint, exact LCSC-number matches first.
pub(crate) fn parse_search(body: &Value, query: &str) -> Vec<Part> {
    let list = body["data"]["componentPageInfo"]["list"].as_array().map(Vec::as_slice).unwrap_or_default();
    let s = |c: &Value, k: &str| c[k].as_str().unwrap_or("").trim().to_string();
    let mut parts: Vec<Part> = list
        .iter()
        .filter(|c| c["componentCode"].is_string())
        .map(|c| {
            let price = c["componentPrices"].as_array().and_then(|p| p.first()).and_then(|p| Some(format!("${} @{}", p["productPrice"].as_f64()?, p["startNumber"].as_u64()?)));
            let datasheet = [s(c, "dataManualUrl"), s(c, "dataManualOfficialLink")].into_iter().find(|u| !u.is_empty());
            let breaks = c["componentPrices"].as_array().into_iter().flatten().filter_map(|p| Some((p["startNumber"].as_u64()?, p["productPrice"].as_f64()?))).collect();
            Part {
                price_breaks: breaks,
                price_currency: "USD".into(),
                mpn: s(c, "componentModelEn"),
                manufacturer: s(c, "componentBrandEn"),
                description: s(c, "describe"),
                category_hint: format!("{} {}", s(c, "firstSortName"), s(c, "secondSortName")).trim().to_string(),
                source: "LCSC".into(),
                supplier_ids: vec![("LCSC".into(), s(c, "componentCode"))],
                datasheet_url: datasheet.unwrap_or_default(),
                package: s(c, "componentSpecificationEn"),
                stock: c["stockCount"].as_u64(),
                moq: c["minPurchaseNum"].as_u64().filter(|&n| n > 0),
                pack_qty: c["encapsulationNumber"].as_u64().filter(|&n| n > 0),
                unit_price: price.unwrap_or_default(),
                ..Default::default()
            }
        })
        .collect();
    if is_lcsc_id(query) {
        parts.sort_by_key(|p| !p.supplier_ids[0].1.eq_ignore_ascii_case(query));
    }
    parts
}

impl Source for Lcsc {
    fn name(&self) -> &'static str {
        "LCSC"
    }

    fn search(&self, query: &str) -> Result<Vec<Part>> {
        let body = json!({"keyword": query, "currentPage": 1, "pageSize": SEARCH_LIMIT});
        let resp = self.http.post(SEARCH_URL).json(&body).send().context("LCSC search")?;
        if !resp.status().is_success() {
            bail!("LCSC search: HTTP {}", resp.status());
        }
        Ok(parse_search(&resp.json()?, query))
    }

    fn fetch(&self, part: &Part, work: &Path) -> Result<Option<Part>> {
        let id = part.supplier_ids.iter().find(|(k, _)| k == "LCSC").map(|(_, v)| v.as_str()).context("not an LCSC part")?;
        let url = COMPONENT_URL.replace("{id}", id);
        let data: Value = serde_json::from_slice(&self.get_bytes(&url)?).context("EasyEDA sent something that is not JSON")?;
        let c = easyeda::convert(&data, &part.description, &part.datasheet_url).with_context(|| format!("converting {id}"))?;

        // The GUI passes a fresh path, not an existing folder.
        std::fs::create_dir_all(work).with_context(|| format!("creating {}", work.display()))?;
        let mut out = part.clone();
        let sym = work.join(format!("{id}.kicad_sym"));
        std::fs::write(&sym, &c.symbol)?;
        out.symbol_file = Some(sym);

        let mut footprint = c.footprint;
        let step = match c.model_uuid.as_deref() {
            None => Err(anyhow!("EasyEDA has no 3D model for this part")),
            Some(u) => self.get_bytes(&STEP_URL.replace("{uuid}", u)).and_then(|b| if b.starts_with(b"ISO-10303") { Ok(b) } else { bail!("response is not a STEP file") }),
        };
        match step {
            Ok(step) => {
                let path = work.join(format!("{}.step", c.footprint_name));
                std::fs::write(&path, step)?;
                out.model_files = vec![path];
            }
            Err(e) => {
                out.warnings.push(format!("3D model not downloaded: {e:#}"));
                // add_part only repoints a model path it is given a file for.
                footprint = footprint.lines().filter(|l| !l.starts_with("(model ")).collect::<Vec<_>>().join("\n");
            }
        }
        let fp = work.join(format!("{}.kicad_mod", c.footprint_name));
        std::fs::write(&fp, footprint)?;
        out.footprint_files = vec![fp];

        let pdf = work.join(format!("{id}.pdf"));
        let links = [part.datasheet_url.as_str(), c.datasheet_url.as_str()];
        match download_first_pdf(&links, &pdf) {
            Ok(()) => out.datasheet_file = Some(pdf),
            Err(e) => {
                if out.datasheet_url.is_empty() {
                    out.datasheet_url = c.datasheet_url;
                }
                out.warnings.push(format!("datasheet not downloaded (link kept): {e:#}"));
            }
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_response_becomes_parts_with_exact_id_first() {
        let body: Value = serde_json::from_str(include_str!("../../tests/fixtures/jlcpcb_search_C2040.json")).unwrap();
        let parts = parse_search(&body, "C2040");
        assert!(parts.len() > 1);
        let p = &parts[0];
        assert_eq!((p.mpn.as_str(), p.manufacturer.as_str(), p.package.as_str()), ("RP2040", "Raspberry Pi", "LQFN-56(7x7)"));
        assert_eq!(p.supplier_ids, [("LCSC".to_string(), "C2040".to_string())]);
        assert_eq!((p.moq, p.pack_qty), (Some(1), Some(3400)));
        assert!(p.stock.is_some() && p.unit_price.starts_with('$') && p.unit_price.contains(" @1"));
        assert!(p.datasheet_url.starts_with("https://") && p.category_hint.contains("Microcontrollers"));
    }

    #[test]
    fn lcsc_ids() {
        assert!(is_lcsc_id("C2040") && is_lcsc_id("c25804"));
        assert!(!is_lcsc_id("C") && !is_lcsc_id("CC0603") && !is_lcsc_id("2040"));
    }
}
