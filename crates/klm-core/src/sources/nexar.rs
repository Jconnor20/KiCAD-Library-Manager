//! Nexar (Octopart) GraphQL API, OAuth2 client credentials.

use super::Source;
use super::http::{self, TokenCache};
use crate::credentials;
use crate::part::Part;
use anyhow::{Result, bail};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::path::Path;

const QUERY: &str = "query Search($q: String!) { supSearch(q: $q, limit: 10) { results { part { \
    mpn shortDescription totalAvail manufacturer { name } category { name } bestDatasheet { url } bestImage { url } \
    specs { attribute { name shortname } displayValue } \
    sellers { company { name } offers { sku moq multipackQuantity prices { quantity price currency } } } } } } }";

pub struct Nexar {
    http: Client,
    token: TokenCache,
}

impl Nexar {
    pub fn from_credentials() -> Option<Self> {
        let id = credentials::get("Nexar", "client_id")?;
        let secret = credentials::get("Nexar", "client_secret")?;
        Some(Self { http: http::client(), token: TokenCache::new("https://identity.nexar.com/connect/token", id, secret) })
    }
}

impl Source for Nexar {
    fn name(&self) -> &'static str {
        "Nexar"
    }

    fn search(&self, query: &str) -> Result<Vec<Part>> {
        let req = self
            .http
            .post("https://api.nexar.com/graphql")
            .bearer_auth(self.token.token(&self.http)?)
            .json(&json!({ "query": QUERY, "variables": { "q": query } }));
        parse(&http::send(req)?)
    }

    fn fetch(&self, _part: &Part, _work: &Path) -> Result<Option<Part>> {
        Ok(None)
    }

    fn cad_page_url(&self, part: &Part) -> Option<String> {
        super::snapmagic_url(part)
    }
}

fn parse(v: &Value) -> Result<Vec<Part>> {
    if v["data"].is_null() {
        let msgs: Vec<String> = v["errors"].as_array().into_iter().flatten().map(|e| http::text(e, "/message")).collect();
        bail!("{}", if msgs.is_empty() { "empty reply".into() } else { msgs.join("; ") });
    }
    Ok(v["data"]["supSearch"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let p = &r["part"];
            let package = p["specs"].as_array().into_iter().flatten().find(|s| {
                let a = format!("{} {}", http::text(s, "/attribute/shortname"), http::text(s, "/attribute/name")).to_lowercase();
                a.contains("package") || a.contains("case")
            });
            let mut part = Part {
                mpn: http::text(p, "/mpn"),
                manufacturer: http::text(p, "/manufacturer/name"),
                description: http::text(p, "/shortDescription"),
                category_hint: http::text(p, "/category/name"),
                datasheet_url: http::https(http::text(p, "/bestDatasheet/url")),
                package: package.map(|s| http::text(s, "/displayValue")).unwrap_or_default(),
                stock: p["totalAvail"].as_u64(),
                params: p["specs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| {
                        let name = http::text(s, "/attribute/name");
                        (if name.is_empty() { http::text(s, "/attribute/shortname") } else { name }, http::text(s, "/displayValue"))
                    })
                    .collect(),
                ..Part::default()
            };
            // Lowest-quantity USD price over all sellers, so the column is comparable across rows.
            let mut best: Option<(u64, f64)> = None;
            let mut best_offer: Option<&Value> = None;
            for seller in p["sellers"].as_array().into_iter().flatten() {
                let company = http::text(seller, "/company/name").to_lowercase();
                let name = if company.contains("digi-key") || company.contains("digikey") {
                    Some("DigiKey")
                } else if company.contains("mouser") {
                    Some("Mouser")
                } else {
                    None
                };
                for offer in seller["offers"].as_array().into_iter().flatten() {
                    let sku = http::text(offer, "/sku");
                    if let Some(n) = name.filter(|n| !sku.is_empty() && !part.supplier_ids.iter().any(|(s, _)| s == n)) {
                        part.supplier_ids.push((n.into(), sku));
                    }
                    for b in offer["prices"].as_array().into_iter().flatten().filter(|b| b["currency"] == "USD") {
                        if let (Some(q), Some(price)) = (b["quantity"].as_u64(), b["price"].as_f64())
                            && best.is_none_or(|(bq, bp)| q < bq || (q == bq && price < bp))
                        {
                            best = Some((q, price));
                            best_offer = Some(offer);
                        }
                    }
                }
            }
            if let Some((q, price)) = best {
                part.unit_price = format!("${price} @{q}");
                // MOQ and pack size come from the offer that gave the shown price.
                part.moq = best_offer.and_then(|o| o["moq"].as_u64());
                part.pack_qty = best_offer.and_then(|o| o["multipackQuantity"].as_u64());
            }
            part
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape follows the Nexar supSearch schema (field names checked by live introspection; values hand-written because the quota was spent).
    const REPLY: &str = r#"{"data": {"supSearch": {"results": [{"part": {
        "mpn": "LM358DR", "shortDescription": "Dual Operational Amplifier, 8-Pin SOIC", "totalAvail": 98000,
        "manufacturer": {"name": "Texas Instruments"}, "category": {"name": "Operational Amplifiers"},
        "bestDatasheet": {"url": "https://www.ti.com/lit/ds/symlink/lm358.pdf"},
        "specs": [{"attribute": {"shortname": "supplyvoltage"}, "displayValue": "3V ~ 32V"},
                  {"attribute": {"name": "Case/Package", "shortname": "case_package"}, "displayValue": "SOIC-8"}],
        "sellers": [
          {"company": {"name": "Mouser Electronics"}, "offers": [{"sku": "595-LM358DR", "prices": [{"quantity": 1, "price": 0.55, "currency": "USD"}]}]},
          {"company": {"name": "Digi-Key"}, "offers": [{"sku": "296-1395-1-ND", "moq": 1, "multipackQuantity": 2500, "prices": [{"quantity": 1, "price": 0.52, "currency": "USD"}, {"quantity": 100, "price": 0.3, "currency": "EUR"}]}]},
          {"company": {"name": "Somewhere Else"}, "offers": [{"sku": "ZZ", "prices": []}]}]}}]}}}"#;

    #[test]
    fn parses_search_reply() {
        let parts = parse(&serde_json::from_str(REPLY).unwrap()).unwrap();
        assert_eq!(parts.len(), 1);
        let p = &parts[0];
        assert_eq!((p.mpn.as_str(), p.manufacturer.as_str(), p.package.as_str()), ("LM358DR", "Texas Instruments", "SOIC-8"));
        assert_eq!(p.stock, Some(98000));
        assert_eq!(p.category_hint, "Operational Amplifiers");
        assert_eq!(p.datasheet_url, "https://www.ti.com/lit/ds/symlink/lm358.pdf");
        assert_eq!(p.unit_price, "$0.52 @1");
        assert_eq!((p.moq, p.pack_qty), (Some(1), Some(2500)));
        assert_eq!(p.supplier_ids, vec![("Mouser".to_string(), "595-LM358DR".to_string()), ("DigiKey".to_string(), "296-1395-1-ND".to_string())]);
    }

    #[test]
    fn plan_limit_reply_is_reported_not_swallowed() {
        // Recorded live (W6.1): the free plan's part quota ran out, so `data` is null.
        let e = parse(&serde_json::from_str(include_str!("../../tests/fixtures/live/nexar_quota_exceeded.json")).unwrap()).unwrap_err();
        assert!(e.to_string().contains("part limit"), "{e}");
    }

    #[test]
    fn reports_graphql_errors() {
        let e = r#"{"errors":[{"message":"Unauthorized"}],"data":null}"#;
        assert!(parse(&serde_json::from_str(e).unwrap()).unwrap_err().to_string().contains("Unauthorized"));
        assert!(parse(&serde_json::json!({"data": {"supSearch": {"results": []}}})).unwrap().is_empty());
    }
}
