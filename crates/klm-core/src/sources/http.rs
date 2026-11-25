//! Shared plumbing for the distributor APIs.

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, RequestBuilder};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub fn client() -> Client {
    client_with("KiCAD-Library-Manager", 20)
}

/// The one place HTTP clients are built; callers differ only in who they say they are and how long they wait.
pub(crate) fn client_with(user_agent: &str, timeout_secs: u64) -> Client {
    // An https link must not be redirected to plain http (or anything else); local test servers may stay on http.
    let policy = reqwest::redirect::Policy::custom(|a| {
        let local = matches!(a.url().host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if a.previous().len() >= 10 {
            a.error("too many redirects")
        } else if a.url().scheme() == "https" || local {
            a.follow()
        } else {
            a.error("redirect away from https refused")
        }
    });
    Client::builder().timeout(Duration::from_secs(timeout_secs)).user_agent(user_agent).redirect(policy).build().expect("TLS backend")
}

/// Sends `req` and parses the JSON reply. The URL is stripped from errors because Mouser's
/// API key is a query parameter.
pub fn send<T: for<'de> Deserialize<'de>>(req: RequestBuilder) -> Result<T> {
    let resp = req.send().map_err(reqwest::Error::without_url).context("request failed")?;
    let status = resp.status();
    if !status.is_success() {
        let body: String = resp.text().unwrap_or_default().chars().take(200).collect();
        bail!("HTTP {status}: {body}");
    }
    resp.json().map_err(reqwest::Error::without_url).context("unreadable reply")
}

/// String at a JSON pointer; empty when missing or null.
pub fn text(v: &Value, pointer: &str) -> String {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or_default().trim().to_string()
}

/// Distributors sometimes return protocol-relative datasheet links.
pub fn https(url: String) -> String {
    if url.starts_with("//") { format!("https:{url}") } else { url }
}

/// OAuth2 client-credentials token, reused until shortly before it expires.
pub struct TokenCache {
    url: &'static str,
    id: String,
    secret: String,
    cached: Mutex<Option<(String, Instant)>>,
}

impl TokenCache {
    pub fn new(url: &'static str, id: String, secret: String) -> Self {
        Self { url, id, secret, cached: Mutex::new(None) }
    }

    pub fn token(&self, http: &Client) -> Result<String> {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, expires)) = &*cached
            && Instant::now() < *expires
        {
            return Ok(t.clone());
        }
        #[derive(Deserialize)]
        struct Reply {
            access_token: String,
            expires_in: u64,
        }
        let form = [("grant_type", "client_credentials"), ("client_id", &self.id), ("client_secret", &self.secret)];
        let r: Reply = send(http.post(self.url).form(&form)).context("could not get an access token (check the credentials)")?;
        let expires = Instant::now() + Duration::from_secs(r.expires_in.saturating_sub(60));
        *cached = Some((r.access_token.clone(), expires));
        Ok(r.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unexpired_token_is_reused_without_a_request() {
        let c = TokenCache::new("http://invalid.invalid/token", "id".into(), "secret".into());
        *c.cached.lock().unwrap() = Some(("tok".into(), Instant::now() + Duration::from_secs(300)));
        assert_eq!(c.token(&client()).unwrap(), "tok");
    }

    #[test]
    fn expired_token_is_refreshed_and_failure_hides_credentials() {
        let c = TokenCache::new("http://127.0.0.1:1/token", "my-id".into(), "my-secret".into());
        *c.cached.lock().unwrap() = Some(("old".into(), Instant::now()));
        let e = format!("{:#}", c.token(&client()).unwrap_err());
        assert!(!e.contains("my-secret") && !e.contains("my-id"));
    }

    #[test]
    fn text_and_https() {
        let v: Value = serde_json::from_str(r#"{"a":{"b":" x "},"n":null}"#).unwrap();
        assert_eq!(text(&v, "/a/b"), "x");
        assert_eq!(text(&v, "/n"), "");
        assert_eq!(text(&v, "/zz"), "");
        assert_eq!(https("//m.digikey.com/a.pdf".into()), "https://m.digikey.com/a.pdf");
    }
}
