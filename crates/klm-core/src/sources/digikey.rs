//! DigiKey Product Information API v4 (keyword search), OAuth2 client credentials.

use super::Source;
use super::http::{self, TokenCache};
use crate::credentials;
use crate::part::Part;
use anyhow::Result;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::path::Path;

pub struct DigiKey {
    http: Client,
    client_id: String,
    token: TokenCache,
}

impl DigiKey {
    pub fn from_credentials() -> Option<Self> {
        let id = credentials::get("DigiKey", "client_id")?;
        let secret = credentials::get("DigiKey", "client_secret")?;
        Some(Self {
            http: http::client(),
            token: TokenCache::new("https://api.digikey.com/v1/oauth2/token", id.clone(), secret),
            client_id: id,
        })
    }
}

impl Source for DigiKey {
    fn name(&self) -> &'static str {
        "DigiKey"
    }

    fn search(&self, query: &str) -> Result<Vec<Part>> {
        let req = self
            .http
            .post("https://api.digikey.com/products/v4/search/keyword")
            .bearer_auth(self.token.token(&self.http)?)
            .header("X-DIGIKEY-Client-Id", &self.client_id)
            .header("X-DIGIKEY-Locale-Site", "US")
            .header("X-DIGIKEY-Locale-Language", "en")
            .header("X-DIGIKEY-Locale-Currency", "USD")
            .json(&json!({ "Keywords": query, "Limit": 10, "Offset": 0 }));
        Ok(parse(&http::send(req)?))
    }

    fn fetch(&self, _part: &Part, _work: &Path) -> Result<Option<Part>> {
        Ok(None)
    }

    fn cad_page_url(&self, part: &Part) -> Option<String> {
        super::snapmagic_url(part)
    }
}

/// "Resistors Chip Resistor - Surface Mount": the category chain, for `categories::guess`.
fn category_chain(cat: &Value) -> String {
    let mut names = vec![];
    let mut cur = cat;
    while let Some(n) = cur.get("Name").and_then(Value::as_str) {
        names.push(n);
        cur = &cur["ChildCategories"][0];
    }
    names.join(" ")
}

fn parse(v: &Value) -> Vec<Part> {
    let Some(products) = v["Products"].as_array() else { return vec![] };
    products
        .iter()
        .map(|p| {
            let param = |name: &str| {
                p["Parameters"].as_array().into_iter().flatten().find(|x| x["ParameterText"] == name).map(|x| http::text(x, "/ValueText"))
            };
            let variation = &p["ProductVariations"][0];
            let mut part = Part {
                mpn: http::text(p, "/ManufacturerProductNumber"),
                manufacturer: http::text(p, "/Manufacturer/Name"),
                description: http::text(p, "/Description/ProductDescription"),
                category_hint: category_chain(&p["Category"]),
                datasheet_url: http::https(http::text(p, "/DatasheetUrl")),
                package: param("Package / Case").or_else(|| param("Supplier Device Package")).unwrap_or_default(),
                stock: p["QuantityAvailable"].as_u64(),
                ..Part::default()
            };
            let id = http::text(variation, "/DigiKeyProductNumber");
            if !id.is_empty() {
                part.supplier_ids.push(("DigiKey".into(), id));
            }
            if let Some(b) = variation["StandardPricing"].get(0) {
                part.unit_price = format!("${} @{}", b["UnitPrice"], b["BreakQuantity"]);
            } else if let Some(price) = p["UnitPrice"].as_f64() {
                part.unit_price = format!("${price} @1");
            }
            part
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape follows DigiKey's documented v4 KeywordSearch response.
    const REPLY: &str = r#"{
      "Products": [{
        "Description": {"ProductDescription": "RES 10K OHM 1% 1/10W 0603", "DetailedDescription": "10 kOhms 1% 0.1W"},
        "Manufacturer": {"Id": 603, "Name": "YAGEO"},
        "ManufacturerProductNumber": "RC0603FR-0710KL",
        "UnitPrice": 0.1,
        "ProductUrl": "https://www.digikey.com/en/products/detail/yageo/RC0603FR-0710KL/726880",
        "DatasheetUrl": "//www.yageo.com/upload/media/product/RC_L.pdf",
        "QuantityAvailable": 4510000,
        "Category": {"CategoryId": 52, "ParentId": 0, "Name": "Resistors",
                     "ChildCategories": [{"CategoryId": 53, "ParentId": 52, "Name": "Chip Resistor - Surface Mount", "ChildCategories": []}]},
        "Parameters": [
          {"ParameterId": 2085, "ParameterText": "Supplier Device Package", "ValueId": "1", "ValueText": "0603"},
          {"ParameterId": 16, "ParameterText": "Package / Case", "ValueId": "2", "ValueText": "0603 (1608 Metric)"}],
        "ProductVariations": [{
          "DigiKeyProductNumber": "311-10.0KHRCT-ND",
          "PackageType": {"Id": 2, "Name": "Cut Tape (CT)"},
          "StandardPricing": [{"BreakQuantity": 1, "UnitPrice": 0.1, "TotalPrice": 0.1}, {"BreakQuantity": 10, "UnitPrice": 0.012, "TotalPrice": 0.12}],
          "MinimumOrderQuantity": 1}]
      },
      {"ManufacturerProductNumber": "NODATA", "DatasheetUrl": null, "Category": null, "ProductVariations": []}],
      "ProductsCount": 2
    }"#;

    #[test]
    fn parses_keyword_search_reply() {
        let parts = parse(&serde_json::from_str(REPLY).unwrap());
        assert_eq!(parts.len(), 2);
        let p = &parts[0];
        assert_eq!((p.mpn.as_str(), p.manufacturer.as_str()), ("RC0603FR-0710KL", "YAGEO"));
        assert_eq!(p.description, "RES 10K OHM 1% 1/10W 0603");
        assert_eq!(p.package, "0603 (1608 Metric)");
        assert_eq!(p.stock, Some(4510000));
        assert_eq!(p.unit_price, "$0.1 @1");
        assert_eq!(p.datasheet_url, "https://www.yageo.com/upload/media/product/RC_L.pdf");
        assert_eq!(p.category_hint, "Resistors Chip Resistor - Surface Mount");
        assert_eq!(p.supplier_ids, vec![("DigiKey".to_string(), "311-10.0KHRCT-ND".to_string())]);
    }

    #[test]
    fn tolerates_nulls_and_empty_reply() {
        let parts = parse(&serde_json::from_str(REPLY).unwrap());
        assert_eq!(parts[1].mpn, "NODATA");
        assert!(parts[1].supplier_ids.is_empty() && parts[1].category_hint.is_empty());
        assert!(parse(&serde_json::json!({})).is_empty());
    }
}
