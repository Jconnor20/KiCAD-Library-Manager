//! DigiKey Product Information API v4 (keyword search), OAuth2 client credentials.

use super::http::{self, TokenCache};
use super::{Source, categories};
use crate::credentials;
use crate::part::Part;
use anyhow::Result;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::path::Path;

pub struct DigiKey {
    http: Client,
    /// `X-DIGIKEY-Locale-Site` and `-Currency`.
    locale: (&'static str, &'static str),
    client_id: String,
    token: TokenCache,
}

impl DigiKey {
    pub fn from_credentials() -> Option<Self> {
        let id = credentials::get("DigiKey", "client_id")?;
        let secret = credentials::get("DigiKey", "client_secret")?;
        Some(Self {
            http: http::client(),
            locale: ("US", "USD"),
            token: TokenCache::new("https://api.digikey.com/v1/oauth2/token", id.clone(), secret),
            client_id: id,
        })
    }

    /// Prices and stock for a shipping region (ISO country); regions DigiKey has no site for stay US/USD.
    pub fn with_region(mut self, region: &str) -> Self {
        self.locale = locale(region);
        self
    }
}

/// DigiKey site and currency for a country.
// shortcut: a few common sites only; the rest fall back to US/USD, add rows as users ask.
pub fn locale(region: &str) -> (&'static str, &'static str) {
    match region.trim().to_ascii_uppercase().as_str() {
        "CA" => ("CA", "CAD"),
        "GB" | "UK" => ("UK", "GBP"),
        "DE" => ("DE", "EUR"),
        "FR" => ("FR", "EUR"),
        "IT" => ("IT", "EUR"),
        "NL" => ("NL", "EUR"),
        "AU" => ("AU", "AUD"),
        "JP" => ("JP", "JPY"),
        _ => ("US", "USD"),
    }
}

impl Source for DigiKey {
    fn name(&self) -> &'static str {
        "DigiKey"
    }

    fn search(&self, query: &str) -> Result<Vec<Part>> {
        self.search_with(query, None)
    }

    fn categories(&self) -> Result<Vec<categories::Category>> {
        let req = self
            .http
            .get("https://api.digikey.com/products/v4/search/categories")
            .bearer_auth(self.token.token(&self.http)?)
            .header("X-DIGIKEY-Client-Id", &self.client_id);
        Ok(categories::parse_digikey(&http::send(req)?))
    }

    fn search_in_category(&self, id: &str, query: &str) -> Result<Vec<Part>> {
        self.search_with(query, Some(id))
    }

    fn fetch(&self, _part: &Part, _work: &Path) -> Result<Option<Part>> {
        Ok(None)
    }

    fn cad_page_url(&self, part: &Part) -> Option<String> {
        super::snapmagic_url(part)
    }
}

impl DigiKey {
    fn search_with(&self, query: &str, category: Option<&str>) -> Result<Vec<Part>> {
        let mut body = json!({ "Keywords": query, "Limit": 10, "Offset": 0 });
        if let Some(id) = category {
            body["FilterOptionsRequest"] = json!({ "CategoryFilter": [{ "Id": id }] });
        }
        let req = self
            .http
            .post("https://api.digikey.com/products/v4/search/keyword")
            .bearer_auth(self.token.token(&self.http)?)
            .header("X-DIGIKEY-Client-Id", &self.client_id)
            .header("X-DIGIKEY-Locale-Site", self.locale.0)
            .header("X-DIGIKEY-Locale-Language", "en")
            .header("X-DIGIKEY-Locale-Currency", self.locale.1)
            .json(&body);
        let mut parts = parse(&http::send(req)?);
        for p in &mut parts {
            p.price_currency = self.locale.1.into();
            p.unit_price = coded_price(&p.unit_price, self.locale.1);
            for v in &mut p.variations {
                v.price_currency = self.locale.1.into();
                v.unit_price = coded_price(&v.unit_price, self.locale.1);
            }
        }
        Ok(parts)
    }
}

/// `parse` writes "$" whatever the site's currency; swap it for the real ISO code so `currency::detect` is right.
fn coded_price(text: &str, code: &str) -> String {
    text.strip_prefix('$').map_or_else(|| text.to_string(), |rest| format!("{code} {rest}"))
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

pub(crate) fn parse(v: &Value) -> Vec<Part> {
    let Some(products) = v["Products"].as_array() else { return vec![] };
    products
        .iter()
        .map(|p| {
            let param = |name: &str| p["Parameters"].as_array().into_iter().flatten().find(|x| x["ParameterText"] == name).map(|x| http::text(x, "/ValueText"));
            let params = p["Parameters"].as_array().into_iter().flatten().map(|x| (http::text(x, "/ParameterText"), http::text(x, "/ValueText"))).collect();
            let base = Part {
                params,
                mpn: http::text(p, "/ManufacturerProductNumber"),
                manufacturer: http::text(p, "/Manufacturer/Name"),
                description: http::text(p, "/Description/ProductDescription"),
                category_hint: category_chain(&p["Category"]),
                datasheet_url: http::https(http::text(p, "/DatasheetUrl")),
                image_url: http::https(http::text(p, "/PhotoUrl")),
                package: param("Package / Case").or_else(|| param("Supplier Device Package")).unwrap_or_default(),
                lifecycle: http::text(p, "/ProductStatus/Status"),
                lead_time_days: p["ManufacturerLeadWeeks"].as_str().and_then(|w| w.trim().parse::<u32>().ok()).map(|w| w * 7),
                ..Part::default()
            };
            // One entry per packaging (cut tape, reel, Digi-Reel...), each with its own number, stock and prices.
            let mut all: Vec<Part> = p["ProductVariations"].as_array().into_iter().flatten().map(|v| one_variation(p, v, base.clone())).collect();
            if all.is_empty() {
                all.push(one_variation(p, &Value::Null, base));
            }
            let mut part = all.remove(0);
            part.variations = all;
            part
        })
        .collect()
}

fn one_variation(p: &Value, variation: &Value, mut part: Part) -> Part {
    part.stock = variation["QuantityAvailableforPackageType"].as_u64().or_else(|| p["QuantityAvailable"].as_u64());
    let id = http::text(variation, "/DigiKeyProductNumber");
    if !id.is_empty() {
        part.supplier_ids.push(("DigiKey".into(), id));
    }
    part.price_breaks =
        variation["StandardPricing"].as_array().into_iter().flatten().filter_map(|b| Some((b["BreakQuantity"].as_u64()?, b["UnitPrice"].as_f64()?))).collect();
    part.moq = variation["MinimumOrderQuantity"].as_u64();
    part.packaging = http::text(variation, "/PackageType/Name");
    part.pack_qty = variation["StandardPackage"].as_u64();
    if let Some(b) = variation["StandardPricing"].get(0) {
        part.unit_price = format!("${} @{}", b["UnitPrice"], b["BreakQuantity"]);
    } else if let Some(price) = p["UnitPrice"].as_f64() {
        part.unit_price = format!("${price} @1");
    }
    part
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
        "DatasheetUrl": "//www.yageo.com/upload/media/product/RC_L.pdf", "PhotoUrl": "https://mm.digikey.com/Volume0/opasdata/d220001/medias/images/1/rc0603.jpg",
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
        assert_eq!((p.price_breaks.clone(), p.moq), (vec![(1, 0.1), (10, 0.012)], Some(1)));
        assert_eq!(p.datasheet_url, "https://www.yageo.com/upload/media/product/RC_L.pdf");
        assert!(p.image_url.ends_with("rc0603.jpg"));
        assert_eq!(p.category_hint, "Resistors Chip Resistor - Surface Mount");
        assert_eq!(p.supplier_ids, vec![("DigiKey".to_string(), "311-10.0KHRCT-ND".to_string())]);
    }

    #[test]
    fn live_reply_keeps_every_packaging() {
        let parts = parse(&serde_json::from_str(include_str!("../../tests/fixtures/live/digikey_search_LM358.json")).unwrap());
        let p = parts.iter().find(|p| p.mpn == "LM358DT").unwrap();
        let all: Vec<_> = std::iter::once(p).chain(&p.variations).map(|v| (v.packaging.as_str(), v.moq, v.stock, v.supplier_ids[0].1.as_str())).collect();
        assert_eq!(all.len(), 3);
        assert_eq!((all[1].0, all[1].1, all[1].2), ("Cut Tape (CT)", Some(1), Some(76865)));
        assert_eq!(all[0].2, Some(75000), "stock is per packaging");
        assert!(all.iter().all(|a| a.3.starts_with("497-")) && all[0].3 != all[1].3);
        assert_eq!(p.variations[0].price_breaks[0].0, 1);
    }

    // Recorded from the live API (W6.1).
    #[test]
    fn live_keyword_reply_maps_every_field() {
        let parts = parse(&serde_json::from_str(include_str!("../../tests/fixtures/live/digikey_search_LM358.json")).unwrap());
        let p = parts.iter().find(|p| p.mpn == "LM358DT").unwrap();
        assert_eq!(
            (p.manufacturer.as_str(), p.package.as_str(), p.packaging.as_str()),
            ("STMicroelectronics", "8-SOIC (0.154\", 3.90mm Width)", "Tape & Reel (TR)")
        );
        assert_eq!((p.stock, p.moq, p.pack_qty, p.lead_time_days), (Some(75000), Some(2500), Some(2500), Some(30 * 7)));
        assert_eq!(p.price_breaks[..2], [(2500, 0.08508), (5000, 0.08225)]);
        assert_eq!(p.supplier_ids, [("DigiKey".to_string(), "497-1591-2-ND".to_string())]);
        assert!(p.datasheet_url.starts_with("https://www.st.com/") && p.image_url.starts_with("https://mm.digikey.com/"));
        assert!(p.category_hint.starts_with("Integrated Circuits (ICs)"), "{}", p.category_hint);
        // Tubes have no StandardPackage, so the pack size stays unknown rather than wrong.
        let tube = parts.iter().find(|p| p.mpn == "LM358P").unwrap();
        assert_eq!((tube.moq, tube.pack_qty, tube.packaging.as_str()), (Some(1), None, "Tube"));
    }

    #[test]
    fn live_category_filtered_reply_parses() {
        let parts = parse(&serde_json::from_str(include_str!("../../tests/fixtures/live/digikey_search_category.json")).unwrap());
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.category_hint.starts_with("Resistors")), "{:?}", parts.iter().map(|p| &p.category_hint).collect::<Vec<_>>());
    }

    #[test]
    fn parameters_autofill_a_resistor() {
        let parts = parse(&serde_json::from_str(include_str!("../../tests/fixtures/live/digikey_search_category.json")).unwrap());
        let fills = crate::fields::autofill(&crate::fields::default_class("Resistor"), &parts[1]);
        let get = |n: &str| fills.iter().find(|(f, _)| f == n).map(|(_, v)| v.as_str());
        assert_eq!((get("Resistance"), get("Tolerance"), get("Power"), get("Package")), (Some("10kΩ"), Some("1%"), Some("100mW"), Some("0402")));
        assert_eq!(get("Temp coefficient"), Some("±100ppm/°C"), "free text is kept as the source writes it");
        // A jumper's "Jumper" tolerance and "-" power are not values.
        let jumper = crate::fields::autofill(&crate::fields::default_class("Resistor"), &parts[0]);
        assert!(!jumper.iter().any(|(f, _)| f == "Tolerance" || f == "Power"), "{jumper:?}");
    }

    #[test]
    fn price_text_carries_the_real_currency() {
        let t = coded_price("$0.1 @1", "CAD");
        assert_eq!(t, "CAD 0.1 @1");
        assert_eq!(crate::currency::detect(&t).as_deref(), Some("CAD"));
        assert_eq!(crate::currency::price_in(&t, None, None), Some(0.1));
    }

    #[test]
    fn tolerates_nulls_and_empty_reply() {
        let parts = parse(&serde_json::from_str(REPLY).unwrap());
        assert_eq!(parts[1].mpn, "NODATA");
        assert!(parts[1].supplier_ids.is_empty() && parts[1].category_hint.is_empty());
        assert!(parse(&serde_json::json!({})).is_empty());
    }
}
