//! Live contract check for the distributor APIs. Everything here is `#[ignore]`d: run with
//! `cargo test -p klm-core --test live_api -- --ignored live`. Keys come from Credential Manager;
//! `live_record_*` saves each raw reply (secrets replaced by REDACTED) under `tests/fixtures/live/`,
//! which the unit tests in `src/sources/*` replay. Nothing touches the app's settings or database.

use klm_core::credentials;
use klm_core::sources;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live")
}

fn http() -> Client {
    Client::builder().user_agent("KiCAD-Library-Manager").timeout(std::time::Duration::from_secs(30)).build().unwrap()
}

/// Writes `v` pretty-printed with every stored secret replaced, and refuses to write if one survives.
fn save(name: &str, v: &Value, secrets: &[String]) {
    let mut text = serde_json::to_string_pretty(v).unwrap();
    // JLCPCB's Aliyun links carry a temporary STS token and signature in their query string.
    let mut from = 0;
    while let Some(i) = text[from..].find("aliyuncs.com").map(|i| i + from) {
        let end = text[i..].find('"').map_or(text.len(), |n| i + n);
        if let Some(q) = text[i..end].find('?') {
            text.replace_range(i + q..end, "?REDACTED");
        }
        from = i + 1;
    }
    for s in secrets.iter().filter(|s| s.len() > 3) {
        text = text.replace(s.as_str(), "REDACTED");
    }
    assert!(!secrets.iter().filter(|s| s.len() > 3).any(|s| text.contains(s.as_str())));
    std::fs::create_dir_all(dir()).unwrap();
    std::fs::write(dir().join(name), text).unwrap();
}

fn cred(source: &str, field: &str) -> String {
    credentials::get(source, field).unwrap_or_else(|| panic!("BLOCKED: no {source}.{field} in Credential Manager"))
}

fn token(c: &Client, url: &str, id: &str, secret: &str) -> String {
    let r: Value = c
        .post(url)
        .form(&[("grant_type", "client_credentials"), ("client_id", id), ("client_secret", secret)])
        .send()
        .unwrap()
        .error_for_status()
        .expect("token request")
        .json()
        .unwrap();
    r["access_token"].as_str().unwrap().to_string()
}

fn ok(r: reqwest::blocking::Response) -> Value {
    let s = r.status();
    let t = r.text().unwrap();
    assert!(s.is_success(), "HTTP {s}: {}", t.chars().take(300).collect::<String>());
    serde_json::from_str(&t).unwrap()
}

#[test]
#[ignore]
fn live_record_digikey() {
    let (id, secret) = (cred("DigiKey", "client_id"), cred("DigiKey", "client_secret"));
    let c = http();
    let tok = token(&c, "https://api.digikey.com/v1/oauth2/token", &id, &secret);
    let secrets = [id.clone(), secret, tok.clone()];
    let post = |body: Value| {
        ok(c.post("https://api.digikey.com/products/v4/search/keyword")
            .bearer_auth(&tok)
            .header("X-DIGIKEY-Client-Id", &id)
            .header("X-DIGIKEY-Locale-Site", "US")
            .header("X-DIGIKEY-Locale-Language", "en")
            .header("X-DIGIKEY-Locale-Currency", "USD")
            .json(&body)
            .send()
            .unwrap())
    };
    let search = post(json!({ "Keywords": "LM358", "Limit": 5, "Offset": 0 }));
    save("digikey_search_LM358.json", &search, &secrets);
    let cats = ok(c.get("https://api.digikey.com/products/v4/search/categories").bearer_auth(&tok).header("X-DIGIKEY-Client-Id", &id).send().unwrap());
    save("digikey_categories.json", &cats, &secrets);
    // Resistors category id, from the tree just fetched.
    let rid = cats["Categories"].as_array().unwrap().iter().find(|c| c["Name"] == "Resistors").map(|c| c["CategoryId"].clone()).unwrap_or(json!(2));
    let by_cat =
        post(json!({ "Keywords": "", "Limit": 3, "Offset": 0, "FilterOptionsRequest": { "CategoryFilter": [{ "Id": rid.to_string().trim_matches('"') }] } }));
    save("digikey_search_category.json", &by_cat, &secrets);
}

#[test]
#[ignore]
fn live_record_mouser() {
    let key = cred("Mouser", "api_key");
    let r = http()
        .post("https://api.mouser.com/api/v1/search/keyword")
        .query(&[("apiKey", &key)])
        .json(&json!({ "SearchByKeywordRequest": { "keyword": "LM358", "records": 5, "startingRecord": 0 } }))
        .send()
        .unwrap();
    save("mouser_search_LM358.json", &ok(r), &[key]);
}

#[test]
#[ignore]
fn live_record_nexar() {
    let (id, secret) = (cred("Nexar", "client_id"), cred("Nexar", "client_secret"));
    let c = http();
    let tok = token(&c, "https://identity.nexar.com/connect/token", &id, &secret);
    let secrets = [id, secret, tok.clone()];
    let gql = |q: &str| ok(c.post("https://api.nexar.com/graphql").bearer_auth(&tok).json(&json!({ "query": q })).send().unwrap());
    // The same query text the app sends, with the term inlined.
    let search = gql(
        "query { supSearch(q: \"LM358\", limit: 3) { results { part { mpn shortDescription totalAvail manufacturer { name } category { name } bestDatasheet { url } bestImage { url } specs { attribute { name shortname } displayValue } sellers { company { name } offers { sku moq packaging factoryLeadDays factoryPackQuantity orderMultiple multipackQuantity prices { quantity price currency } } } } } } }",
    );
    save("nexar_quota_exceeded.json", &search, &secrets);
    let schema = gql(
        "query { a: __type(name: \"SupOffer\") { fields { name } } b: __type(name: \"SupPart\") { fields { name } } c: __type(name: \"SupCategory\") { fields { name } } }",
    );
    save("nexar_schema_fields.json", &schema, &secrets);
    let cats = gql("query { supCategories { id name parentId numParts path } }");
    save("nexar_categories.json", &cats, &secrets);
}

#[test]
#[ignore]
fn live_record_lcsc() {
    let r = http()
        .post("https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList")
        .json(&json!({"keyword": "LM358", "currentPage": 1, "pageSize": 5}))
        .send()
        .unwrap();
    save("jlcpcb_search_LM358.json", &ok(r), &[]);
}

/// The mapped `Part` for each source must carry the fields the API really has.
fn live_search(name: &str) -> klm_core::part::Part {
    let src = sources::build(name).unwrap_or_else(|| panic!("BLOCKED: no {name} credentials"));
    let parts = src.search("LM358").unwrap();
    assert!(!parts.is_empty());
    parts.into_iter().find(|p| !p.price_breaks.is_empty() || !p.unit_price.is_empty()).expect("a priced part")
}

#[test]
#[ignore]
fn live_search_digikey() {
    let p = live_search("DigiKey");
    assert!(p.stock.is_some() && !p.price_breaks.is_empty() && p.moq.is_some() && !p.package.is_empty() && !p.datasheet_url.is_empty());
}

#[test]
#[ignore]
fn live_search_mouser() {
    let p = live_search("Mouser");
    assert!(p.stock.is_some() && !p.price_breaks.is_empty() && p.moq.is_some());
}

#[test]
#[ignore]
fn live_search_nexar() {
    let p = live_search("Nexar");
    assert!(p.stock.is_some() && !p.unit_price.is_empty());
}

#[test]
#[ignore]
fn live_search_lcsc() {
    let p = live_search("LCSC");
    assert!(p.stock.is_some() && !p.price_breaks.is_empty() && !p.package.is_empty());
}
