//! Mouser Search API v1 (keyword search), API key.

use super::Source;
use super::http;
use crate::credentials;
use crate::part::Part;
use crate::search_filter::parse_price;
use anyhow::{Result, bail};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::path::Path;

pub struct Mouser {
    http: Client,
    api_key: String,
}

impl Mouser {
    pub fn from_credentials() -> Option<Self> {
        Some(Self { http: http::client(), api_key: credentials::get("Mouser", "api_key")? })
    }
}

impl Source for Mouser {
    fn name(&self) -> &'static str {
        "Mouser"
    }

    fn search(&self, query: &str) -> Result<Vec<Part>> {
        let req = self
            .http
            .post("https://api.mouser.com/api/v1/search/keyword")
            .query(&[("apiKey", &self.api_key)])
            .json(&json!({ "SearchByKeywordRequest": { "keyword": query, "records": 10, "startingRecord": 0 } }));
        parse(&http::send(req)?)
    }

    fn fetch(&self, _part: &Part, _work: &Path) -> Result<Option<Part>> {
        Ok(None)
    }

    fn cad_page_url(&self, part: &Part) -> Option<String> {
        super::snapmagic_url(part)
    }
}

/// "1,234 In Stock" gives 1234; "None" or "On Order" give no number.
fn stock(availability: &str) -> Option<u64> {
    let digits: String = availability.chars().take_while(|c| c.is_ascii_digit() || *c == ',').filter(|c| *c != ',').collect();
    digits.parse().ok()
}

/// "84 Days" or "12 Weeks" as days; anything else is unknown.
fn lead_days(text: &str) -> Option<u32> {
    let (n, unit) = text.trim().split_once(' ')?;
    let n: u32 = n.parse().ok()?;
    match unit.trim().to_ascii_lowercase().trim_end_matches('s') {
        "day" => Some(n),
        "week" => Some(n * 7),
        _ => None,
    }
}

fn parse(v: &Value) -> Result<Vec<Part>> {
    if let Some(errors) = v["Errors"].as_array().filter(|e| !e.is_empty()) {
        let msgs: Vec<String> = errors.iter().map(|e| http::text(e, "/Message")).collect();
        bail!("{}", msgs.join("; "));
    }
    Ok(v["SearchResults"]["Parts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let id = http::text(p, "/MouserPartNumber");
            let b = &p["PriceBreaks"][0];
            Part {
                mpn: http::text(p, "/ManufacturerPartNumber"),
                manufacturer: http::text(p, "/Manufacturer"),
                description: http::text(p, "/Description"),
                category_hint: http::text(p, "/Category"),
                datasheet_url: http::https(http::text(p, "/DataSheetUrl")),
                stock: stock(&http::text(p, "/Availability")),
                price_breaks: p["PriceBreaks"].as_array().into_iter().flatten().filter_map(|b| Some((b["Quantity"].as_u64()?, parse_price(&http::text(b, "/Price"))?))).collect(),
                price_currency: http::text(b, "/Currency"),
                moq: http::text(p, "/Min").parse().ok(),
                pack_qty: http::text(p, "/Mult").parse().ok(),
                lead_time_days: lead_days(&http::text(p, "/LeadTime")),
                unit_price: if b.is_null() { String::new() } else { format!("{} @{}", http::text(b, "/Price"), b["Quantity"]) },
                // The search reply has no package field; ProductAttributes only lists packaging options.
                supplier_ids: if id.is_empty() { vec![] } else { vec![("Mouser".into(), id)] },
                ..Part::default()
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape follows Mouser's documented SearchByKeyword response.
    const REPLY: &str = r#"{
      "Errors": [],
      "SearchResults": {"NumberOfResult": 2, "Parts": [
        {"Availability": "12,345 In Stock", "DataSheetUrl": "https://www.vishay.com/doc?20035", "Description": "Thick Film Resistors - SMD 10Kohms 1% 0603",
         "Category": "Thick Film Resistors - SMD", "Manufacturer": "Vishay / Dale", "ManufacturerPartNumber": "CRCW060310K0FKEA",
         "Min": "1", "Mult": "1", "LeadTime": "84 Days", "MouserPartNumber": "71-CRCW0603-10K-E3", "ProductAttributes": [{"AttributeName": "Standard Pack Qty", "AttributeValue": "5000"}],
         "PriceBreaks": [{"Quantity": 1, "Price": "$0.10", "Currency": "USD"}, {"Quantity": 10, "Price": "$0.019", "Currency": "USD"}]},
        {"Availability": "None", "DataSheetUrl": "", "Description": "x", "Manufacturer": "Y", "ManufacturerPartNumber": "Z1", "MouserPartNumber": "", "PriceBreaks": []}]}
    }"#;

    #[test]
    fn parses_keyword_search_reply() {
        let parts = parse(&serde_json::from_str(REPLY).unwrap()).unwrap();
        assert_eq!(parts.len(), 2);
        let p = &parts[0];
        assert_eq!((p.mpn.as_str(), p.manufacturer.as_str()), ("CRCW060310K0FKEA", "Vishay / Dale"));
        assert_eq!(p.stock, Some(12345));
        assert_eq!((p.price_breaks.clone(), p.price_currency.as_str(), p.moq, p.lead_time_days), (vec![(1, 0.10), (10, 0.019)], "USD", Some(1), Some(84)));
        assert_eq!(lead_days("2 Weeks"), Some(14));
        assert_eq!(lead_days("N/A"), None);
        assert_eq!(p.unit_price, "$0.10 @1");
        assert_eq!(p.category_hint, "Thick Film Resistors - SMD");
        assert_eq!(p.supplier_ids, vec![("Mouser".to_string(), "71-CRCW0603-10K-E3".to_string())]);
        assert_eq!((parts[1].stock, parts[1].unit_price.as_str()), (None, ""));
        assert!(parts[1].supplier_ids.is_empty());
    }

    #[test]
    fn reports_api_errors_and_null_results() {
        let e = r#"{"Errors":[{"Id":0,"Code":"Invalid","Message":"Invalid unique identifier."}],"SearchResults":null}"#;
        assert!(parse(&serde_json::from_str(e).unwrap()).unwrap_err().to_string().contains("Invalid unique identifier"));
        assert!(parse(&serde_json::json!({"Errors": [], "SearchResults": null})).unwrap().is_empty());
    }
}
