//! Currency detection and conversion for search prices, so results from different sources compare.

use crate::search_filter::parse_price;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RATES_URL: &str = "https://api.frankfurter.dev/v1/latest?base=USD";
const MAX_AGE_SECS: u64 = 24 * 3600;

/// Currency of a source's price text, from an ISO code ("CHF 0.5") or a symbol. `None` when unknown.
pub fn detect(text: &str) -> Option<String> {
    let text = text.split('@').next()?;
    let iso = text.split(|c: char| !c.is_ascii_alphabetic()).find(|w| w.len() == 3 && w.bytes().all(|b| b.is_ascii_uppercase()));
    if let Some(code) = iso {
        return Some(code.to_string());
    }
    // shortcut: "$" is read as USD (every source sends USD today) and "¥" is left unknown (JPY or CNY); add symbols when a source needs them.
    [("$", "USD"), ("€", "EUR"), ("£", "GBP")].into_iter().find(|(s, _)| text.contains(s)).map(|(_, c)| c.to_string())
}

/// Daily reference rates, as units of each currency per 1 USD.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Rates {
    /// Day the rates apply to, as the provider reports it.
    pub date: String,
    fetched: u64,
    per_usd: BTreeMap<String, f64>,
}

impl Rates {
    pub fn new(date: &str, per_usd: &[(&str, f64)]) -> Self {
        Self { date: date.into(), fetched: 0, per_usd: per_usd.iter().map(|(c, r)| (c.to_string(), *r)).collect() }
    }

    pub fn codes(&self) -> Vec<&str> {
        self.per_usd.keys().map(String::as_str).collect()
    }

    pub fn convert(&self, amount: f64, from: &str, to: &str) -> Option<f64> {
        Some(amount / self.per_usd.get(from)? * self.per_usd.get(to)?)
    }

    /// Cached rates if under a day old, else fresh ones (cached for next time); the stale cache if offline.
    pub fn load() -> Result<Self> {
        let path = cache_path();
        let cached: Option<Rates> = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok());
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if let Some(r) = cached.as_ref().filter(|r| now.saturating_sub(r.fetched) < MAX_AGE_SECS) {
            return Ok(r.clone());
        }
        match fetch(now) {
            Ok(fresh) => {
                if let Some(p) = path {
                    let _ = std::fs::create_dir_all(p.parent().unwrap());
                    let _ = std::fs::write(p, serde_json::to_string(&fresh)?);
                }
                Ok(fresh)
            }
            Err(e) => cached.ok_or(e),
        }
    }
}

fn cache_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("KiCAD-Library-Manager").join("rates.json"))
}

fn fetch(now: u64) -> Result<Rates> {
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(15)).user_agent("KiCAD-Library-Manager").build()?;
    let body: serde_json::Value = client.get(RATES_URL).send().and_then(|r| r.error_for_status()).and_then(|r| r.json()).map_err(reqwest::Error::without_url).context("fetching exchange rates")?;
    let mut per_usd: BTreeMap<String, f64> = body["rates"].as_object().context("unreadable exchange rates")?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect();
    per_usd.insert("USD".into(), 1.0);
    Ok(Rates { date: body["date"].as_str().unwrap_or_default().into(), fetched: now, per_usd })
}

/// Unit price as a number in `target` (`None` = the source's own currency). `None` when it can't
/// be parsed or converted, which callers treat as "no price".
pub fn price_in(text: &str, target: Option<&str>, rates: Option<&Rates>) -> Option<f64> {
    let amount = parse_price(text)?;
    let Some(to) = target else { return Some(amount) };
    let from = detect(text)?;
    if from == to { Some(amount) } else { rates?.convert(amount, &from, to) }
}

/// Price text to show: converted and labelled with `target`, or the source's own text if it can't be converted.
pub fn display_price(text: &str, target: Option<&str>, rates: Option<&Rates>) -> String {
    let (Some(to), Some(v)) = (target, price_in(text, target, rates)) else { return text.to_string() };
    let qty = text.split_once('@').map_or(String::new(), |(_, q)| format!(" @{}", q.trim()));
    let num = if v >= 100.0 { format!("{v:.2}") } else { format!("{v:.4}") };
    // Trim trailing zeros but keep two decimals ("0.10", not "0.1").
    let (int, frac) = num.split_once('.').unwrap_or((&num, ""));
    format!("{int}.{:0<2} {to}{qty}", frac.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates() -> Rates {
        Rates::new("2026-10-30", &[("USD", 1.0), ("EUR", 0.9), ("GBP", 0.75)])
    }

    #[test]
    fn detects_currency() {
        for (text, want) in [("$0.1 @1", Some("USD")), ("US$ 5", Some("USD")), ("0,10 € @1", Some("EUR")), ("£2", Some("GBP")), ("CHF 0.5 @10", Some("CHF")), ("¥100", None), ("", None)] {
            assert_eq!(detect(text).as_deref(), want, "{text}");
        }
        // The quantity break after '@' is not part of the price.
        assert_eq!(detect("5 @EUR"), None);
    }

    #[test]
    fn converts_between_currencies() {
        let r = rates();
        assert!((r.convert(1.0, "EUR", "GBP").unwrap() - 0.75 / 0.9).abs() < 1e-9);
        assert_eq!(r.convert(1.0, "EUR", "XXX"), None);
    }

    #[test]
    fn price_in_and_display() {
        let r = rates();
        assert_eq!(price_in("$1.00 @1", None, None), Some(1.0));
        assert_eq!(price_in("$1.00 @1", Some("USD"), None), Some(1.0), "same currency needs no rates");
        assert_eq!(price_in("$1.00 @1", Some("EUR"), None), None, "no rates, no conversion");
        assert_eq!(price_in("¥100", Some("EUR"), Some(&r)), None, "unknown source currency");
        assert_eq!(display_price("$1.00 @100", Some("EUR"), Some(&r)), "0.90 EUR @100");
        assert_eq!(display_price("0,10 € @1", Some("USD"), Some(&r)), "0.1111 USD @1");
        assert_eq!(display_price("$1.00 @100", None, Some(&r)), "$1.00 @100");
        assert_eq!(display_price("¥100", Some("EUR"), Some(&r)), "¥100");
    }
}
