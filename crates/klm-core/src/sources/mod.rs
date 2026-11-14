//! Part sources. Each search source (LCSC, DigiKey, …) implements [`Source`] and is listed in
//! [`all`]; the Search tab queries every enabled one. Vendor ZIPs go through `zip_import` instead.

pub mod categories;
pub mod combine;
pub mod digikey;
pub mod foreign;
mod http;
pub mod lcsc;
pub mod mouser;
pub mod nexar;
pub mod thumbs;
pub mod watcher;
pub mod zip_import;

use crate::part::Part;
use crate::settings::Settings;
use anyhow::Result;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub trait Source: Send + Sync {
    /// Shown in the results table, e.g. "LCSC".
    fn name(&self) -> &'static str;

    /// Parts matching `query` (MPN, supplier number or keywords). Results carry metadata
    /// only (mpn, manufacturer, description, package, stock, unit_price, datasheet_url,
    /// supplier_ids); no files.
    fn search(&self, query: &str) -> Result<Vec<Part>>;

    /// Downloads CAD files for a search result into `work` and returns the part with its
    /// file fields filled in, ready for `LibraryRoot::add_part`. `Ok(None)` means this source
    /// has no downloadable CAD; the UI then opens [`Source::cad_page_url`] in the browser.
    fn fetch(&self, part: &Part, work: &Path) -> Result<Option<Part>>;

    /// This source's category list, flattened, for browsing by part type.
    fn categories(&self) -> Result<Vec<categories::Category>> {
        anyhow::bail!("{} has no category list in its API; search by keyword instead", self.name())
    }

    /// Search within the category `id` from [`Source::categories`]; `query` may be empty.
    fn search_in_category(&self, _id: &str, _query: &str) -> Result<Vec<Part>> {
        anyhow::bail!("{} cannot search by category", self.name())
    }

    /// Web page where the user can download CAD for this part themselves (SnapMagic, UL, …).
    fn cad_page_url(&self, _part: &Part) -> Option<String> {
        None
    }
}

/// SnapMagic's part search; its results link to the Ultra Librarian and SamacSys downloads too.
fn snapmagic_url(part: &Part) -> Option<String> {
    reqwest::Url::parse_with_params("https://www.snapeda.com/search/", [("q", part.mpn.as_str()), ("search-type", "parts")])
        .ok()
        .map(String::from)
}

/// A source built from its stored credentials, enabled or not (the Settings "Test" button
/// uses this). `None` when the credentials are missing or the name is not a search source.
pub fn build(name: &str) -> Option<Box<dyn Source>> {
    Some(match name {
        "LCSC" => Box::new(lcsc::Lcsc::new()),
        "DigiKey" => Box::new(digikey::DigiKey::from_credentials()?),
        "Mouser" => Box::new(mouser::Mouser::from_credentials()?),
        "Nexar" => Box::new(nexar::Nexar::from_credentials()?),
        _ => return None,
    })
}

/// Every source the user has enabled, each kept under its host's rate limit. Each phase adds its sources here.
pub fn all(settings: &Settings) -> Vec<Box<dyn Source>> {
    ["LCSC", "DigiKey", "Mouser", "Nexar"]
        .into_iter()
        .filter(|n| settings.is_enabled(n))
        .filter_map(|n| {
            // Mouser allows about 30 requests a minute; the others are unofficial or quota-limited, so go gently.
            let gap = Duration::from_millis(match n {
                "Mouser" => 2100,
                "LCSC" => 1500,
                _ => 500,
            });
            build(n).map(|s| Box::new(Throttled::new(s, gap, Duration::from_secs(5))) as Box<dyn Source>)
        })
        .collect()
}

/// A source that never gets two requests closer than `gap` (however many threads ask), and that waits and tries again
/// (`pause`, then three times that) when the host answers 429, instead of passing the refusal on to a BOM of hundreds of lines.
/// ponytail: one slot clock per source and no Retry-After parsing; add that if a host asks for longer waits than this.
pub struct Throttled {
    inner: Box<dyn Source>,
    gap: Duration,
    pause: Duration,
    next: Mutex<Instant>,
}

impl Throttled {
    pub fn new(inner: Box<dyn Source>, gap: Duration, pause: Duration) -> Self {
        Throttled { inner, gap, pause, next: Mutex::new(Instant::now()) }
    }

    /// Runs `call` once its turn comes, again after a pause while the host says "too many requests".
    fn run<T>(&self, call: impl Fn() -> Result<T>) -> Result<T> {
        for attempt in 0..=3u32 {
            let start = {
                let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
                let start = (*next).max(Instant::now());
                *next = start + self.gap;
                start
            };
            let queued = start.saturating_duration_since(Instant::now());
            crate::download::note_wait(self.inner.name(), queued, false);
            crate::download::sleep_unless_aborted(queued);
            match call() {
                // The refusal moves everyone's next slot back, so all threads wait it out together instead of each trying again.
                Err(e) if attempt < 3 && !crate::download::aborted() && (format!("{e:#}").contains("429") || format!("{e:#}").contains("403 Forbidden")) => {
                    let wait = self.pause * 3u32.pow(attempt);
                    crate::download::note_wait(self.inner.name(), wait, true);
                    let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
                    *next = (*next).max(Instant::now() + wait);
                }
                r => return r,
            }
        }
        unreachable!("the last attempt returns")
    }
}

impl Source for Throttled {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn search(&self, query: &str) -> Result<Vec<Part>> {
        self.run(|| self.inner.search(query))
    }

    fn fetch(&self, part: &Part, work: &Path) -> Result<Option<Part>> {
        self.run(|| self.inner.fetch(part, work))
    }

    fn categories(&self) -> Result<Vec<categories::Category>> {
        self.run(|| self.inner.categories())
    }

    fn search_in_category(&self, id: &str, query: &str) -> Result<Vec<Part>> {
        self.run(|| self.inner.search_in_category(id, query))
    }

    fn cad_page_url(&self, part: &Part) -> Option<String> {
        self.inner.cad_page_url(part)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Refuses with a 429 the first `refuse` times, then answers.
    struct Flaky(AtomicUsize, usize);

    impl Source for Flaky {
        fn name(&self) -> &'static str {
            "Flaky"
        }
        fn search(&self, _: &str) -> Result<Vec<Part>> {
            if self.0.fetch_add(1, Ordering::SeqCst) < self.1 { anyhow::bail!("HTTP 429 Too Many Requests") } else { Ok(vec![]) }
        }
        fn fetch(&self, _: &Part, _: &Path) -> Result<Option<Part>> {
            Ok(None)
        }
    }

    #[test]
    fn throttled_spaces_requests_and_retries_a_429() {
        let ms = Duration::from_millis;
        let t = Throttled::new(Box::new(Flaky(AtomicUsize::new(0), 1)), ms(60), ms(20));
        let started = Instant::now();
        assert!(t.search("x").is_ok(), "one 429 is retried");
        assert!(t.search("y").is_ok());
        // 429 + retry (the 60 ms slot gap covers the 20 ms pause), then the next call waits its slot: three slots, two gaps.
        assert!(started.elapsed() >= ms(120), "{:?}", started.elapsed());
        let t = Throttled::new(Box::new(Flaky(AtomicUsize::new(0), 99)), ms(1), ms(1));
        assert!(t.search("x").is_err(), "gives up after three tries");
    }

    #[test]
    fn cad_url_encodes_the_mpn() {
        let part = Part { mpn: "SN74LVC1G17DBVR/A B".into(), ..Part::default() };
        let url = snapmagic_url(&part).unwrap();
        assert!(url.starts_with("https://www.snapeda.com/search/?q=SN74LVC1G17DBVR%2FA+B&search-type=parts"), "{url}");
    }
}
