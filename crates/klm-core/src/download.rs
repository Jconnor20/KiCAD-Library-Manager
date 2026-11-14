//! Downloading datasheet PDFs, for fresh LCSC imports and for parts that only have a datasheet link.

use crate::library::{AttachKind, Check, LibraryRoot};
use crate::sources::Source;
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Set to cut short every rate-limit wait (a stop in a bulk import); waits then end and the last answer is passed on.
static ABORT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_abort(on: bool) {
    ABORT.store(on, std::sync::atomic::Ordering::SeqCst);
}

pub fn aborted() -> bool {
    ABORT.load(std::sync::atomic::Ordering::SeqCst)
}

/// Sleeps `dur` in short steps, ending early on [`set_abort`]. True when it was cut short.
pub fn sleep_unless_aborted(dur: Duration) -> bool {
    let end = Instant::now() + dur;
    while let Some(left) = end.checked_duration_since(Instant::now()).filter(|l| !l.is_zero()) {
        if aborted() {
            return true;
        }
        std::thread::sleep(left.min(Duration::from_millis(200)));
    }
    aborted()
}

/// Every download request in the process (whichever thread, whichever source) starts at least `gap` after the previous one, and
/// a refusal pushes the next start back for all of them, so a burst of workers cannot keep hitting a host that has just said stop.
/// The gap doubles (up to 4 s) with each refusal and shrinks back as requests succeed.
const BASE_GAP: Duration = Duration::from_millis(350);
static GATE: Mutex<(Option<Instant>, Duration)> = Mutex::new((None, BASE_GAP));

fn take_turn() {
    let wait = {
        let mut g = GATE.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let start = g.0.map_or(now, |n| n.max(now));
        g.0 = Some(start + g.1);
        start - now
    };
    note_wait("the server", wait, false);
    sleep_unless_aborted(wait);
}

fn backed_off(wait: Duration) {
    let mut g = GATE.lock().unwrap_or_else(|e| e.into_inner());
    g.0 = Some(g.0.unwrap_or_else(Instant::now).max(Instant::now() + wait));
    g.1 = (g.1 * 2).min(Duration::from_secs(4));
}

fn went_fine() {
    let mut g = GATE.lock().unwrap_or_else(|e| e.into_inner());
    g.1 = g.1.mul_f32(0.9).max(BASE_GAP);
}

/// Waits in progress because of a rate limit: who, until when, and whether the host refused (else it is our own pacing).
static WAITS: Mutex<Vec<(&'static str, Instant, bool)>> = Mutex::new(Vec::new());

/// Records that `who` is waiting `dur` on a rate limit, for [`held`]. Short pacing gaps are not worth showing.
pub fn note_wait(who: &'static str, dur: Duration, refused: bool) {
    if refused || dur >= Duration::from_millis(400) {
        let mut w = WAITS.lock().unwrap_or_else(|e| e.into_inner());
        w.retain(|(_, until, _)| *until > Instant::now());
        w.push((who, Instant::now() + dur, refused));
    }
}

/// The rate-limit wait with the most time left, if any: who, how long, and whether the host refused (429/403) rather than us pacing ourselves.
pub fn held() -> Option<(&'static str, Duration, bool)> {
    let now = Instant::now();
    let mut w = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    w.retain(|(_, until, _)| *until > now);
    w.iter().max_by_key(|(_, until, _)| *until).map(|(who, until, refused)| (*who, *until - now, *refused))
}

// Some datasheet hosts turn away anything that doesn't look like a browser.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// Sends the request `build` makes, paced with every other download (see [`take_turn`]), and again after a wait (the server's
/// `Retry-After`, else 3 s, 6 s, 12 s, 24 s) while the host answers 429, so a burst of imports slows down instead of failing.
/// With `blocks_with_403` (EasyEDA's CDN answers a burst with 403, not 429) a 403 is retried too, after 10 s, 20 s, 40 s, 60 s.
/// A stop ([`set_abort`]) ends the waiting and hands back the refusal.
pub(crate) fn send_retrying(blocks_with_403: bool, build: impl Fn() -> reqwest::blocking::RequestBuilder) -> reqwest::Result<reqwest::blocking::Response> {
    for attempt in 0..5u32 {
        take_turn();
        let resp = build().send()?;
        let code = resp.status().as_u16();
        if code != 429 && !(blocks_with_403 && code == 403) {
            went_fine();
            return Ok(resp);
        }
        let wait = Duration::from_secs(resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or((if code == 403 { 10 } else { 3 }) << attempt).min(60));
        backed_off(wait);
        note_wait("the server", wait, true);
        if sleep_unless_aborted(wait) {
            return Ok(resp);
        }
    }
    take_turn();
    build().send()
}

/// Saves the PDF at `url` to `dest`. Redirects are followed. Many "datasheet" links lead to an HTML
/// viewer page, so anything that doesn't start with `%PDF` is refused; `dest` is only written for a PDF.
pub fn download_pdf(url: &str, dest: &Path) -> Result<()> {
    let http = reqwest::blocking::Client::builder().user_agent(BROWSER_USER_AGENT).timeout(Duration::from_secs(60)).build().expect("HTTP client");
    let resp = send_retrying(false, || http.get(url)).with_context(|| format!("requesting {url}"))?;
    if !resp.status().is_success() {
        let hint = if matches!(resp.status().as_u16(), 403 | 429) { " (the server is refusing requests, likely rate limiting; try again in a few minutes)" } else { "" };
        bail!("{url}: HTTP {}{hint}", resp.status());
    }
    let body = resp.bytes().with_context(|| format!("reading {url}"))?;
    if !body.starts_with(b"%PDF") {
        bail!("{url} is not a PDF");
    }
    let tmp = dest.with_extension("klm-tmp");
    std::fs::write(&tmp, &body).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, dest).with_context(|| format!("replacing {}", dest.display()))
}

impl LibraryRoot {
    /// Downloads a local PDF for every part whose datasheet is only a web link, as a manual Attach
    /// would. Returns (MPN, outcome) per part tried; a part that fails keeps its link.
    pub fn download_missing_datasheets(&self) -> Vec<(String, Result<()>)> {
        let tmp = std::env::temp_dir().join(format!("klm-datasheet-{}.pdf", std::process::id()));
        let mut out = vec![];
        let todo: Vec<_> = self.list_parts().into_iter().filter(|p| p.datasheet == Check::Link && p.datasheet_target.as_ref().is_some_and(|u| u.starts_with("http"))).collect();
        if todo.is_empty() {
            return out;
        }
        let _snapshot = match crate::snapshot::guard(self, "download missing datasheets") {
            Ok(g) => g,
            Err(e) => return vec![(String::new(), Err(e))],
        };
        for p in todo {
            let url = p.datasheet_target.clone().unwrap_or_default();
            let result = download_pdf(&url, &tmp).and_then(|()| self.attach_file(&p.category, &p.name, AttachKind::Datasheet, &tmp));
            out.push((p.mpn, result));
        }
        let _ = std::fs::remove_file(&tmp);
        out
    }
}

/// Finds a real PDF for `mpn`: the part's existing link first, then every `datasheet_url` the
/// `sources` return for the MPN. `fetch` saves a URL to `dest` (it is `download_pdf` outside tests).
/// Returns the URL that worked, or why each attempt failed.
pub fn find_datasheet(mpn: &str, existing: Option<&str>, sources: &[Box<dyn Source>], dest: &Path, fetch: &dyn Fn(&str, &Path) -> Result<()>) -> Result<String> {
    let mut why = vec![];
    let mut tried = vec![];
    let mut try_url = |url: &str, from: &str, why: &mut Vec<String>| {
        if url.is_empty() || !url.starts_with("http") || tried.iter().any(|t: &String| t == url) {
            return false;
        }
        tried.push(url.to_string());
        match fetch(url, dest) {
            Ok(()) => true,
            Err(e) => {
                why.push(format!("{from}: {e:#}"));
                false
            }
        }
    };
    if let Some(url) = existing {
        if try_url(url, "existing link", &mut why) {
            return Ok(url.to_string());
        }
    }
    if mpn.is_empty() {
        bail!("no MPN to search for{}", if why.is_empty() { String::new() } else { format!("; {}", why.join("; ")) });
    }
    if sources.is_empty() {
        why.push("no distributor source is enabled to search".into());
    }
    for source in sources {
        match source.search(mpn) {
            Ok(found) => {
                for url in found.iter().filter(|f| crate::matching::same_mpn(&f.mpn, mpn)).map(|f| f.datasheet_url.as_str()) {
                    if try_url(url, source.name(), &mut why) {
                        return Ok(url.to_string());
                    }
                }
            }
            Err(e) => why.push(format!("{}: {e:#}", source.name())),
        }
    }
    bail!("{}", if why.is_empty() { "no source knows a datasheet for this part".to_string() } else { why.join("; ") })
}

impl LibraryRoot {
    /// Gives every part whose datasheet is missing, broken, a web page or a missing file a local PDF
    /// (see [`find_datasheet`]) and points its `Datasheet` property at it. Returns (MPN, outcome) per
    /// part tried; the outcome is the URL used. A part that fails is left as it was.
    pub fn find_missing_datasheets(&self, sources: &[Box<dyn Source>]) -> Vec<(String, Result<String>)> {
        let todo: Vec<_> = self.list_parts().into_iter().filter(|p| p.datasheet != Check::Ok).collect();
        if todo.is_empty() {
            return vec![];
        }
        let _snapshot = match crate::snapshot::guard(self, "find missing datasheets") {
            Ok(g) => g,
            Err(e) => return vec![(String::new(), Err(e))],
        };
        let tmp = std::env::temp_dir().join(format!("klm-datasheet-{}.pdf", std::process::id()));
        let out = todo
            .into_iter()
            .map(|p| {
                let result = find_datasheet(&p.mpn, p.datasheet_target.as_deref(), sources, &tmp, &download_pdf)
                    .and_then(|url| self.attach_file(&p.category, &p.name, AttachKind::Datasheet, &tmp).map(|()| url));
                (if p.mpn.is_empty() { p.name } else { p.mpn }, result)
            })
            .collect();
        let _ = std::fs::remove_file(&tmp);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;


    struct Stub(&'static str, Vec<&'static str>);
    impl Source for Stub {
        fn name(&self) -> &'static str {
            self.0
        }
        fn search(&self, _: &str) -> Result<Vec<crate::part::Part>> {
            Ok(self.1.iter().map(|u| crate::part::Part { mpn: "ABC123".into(), datasheet_url: u.to_string(), ..Default::default() }).collect())
        }
        fn fetch(&self, _: &crate::part::Part, _: &Path) -> Result<Option<crate::part::Part>> {
            Ok(None)
        }
    }

    #[test]
    fn finder_tries_the_link_then_each_source_url_and_explains_failures() {
        let fetch = |url: &str, _: &Path| if url.ends_with("good.pdf") { Ok(()) } else { bail!("bad {url}") };
        let dest = Path::new("unused");
        let sources: Vec<Box<dyn Source>> = vec![Box::new(Stub("A", vec!["http://x/page", ""])), Box::new(Stub("B", vec!["http://x/good.pdf"]))];
        assert_eq!(find_datasheet("ABC123", Some("http://x/old"), &sources, dest, &fetch).unwrap(), "http://x/good.pdf");
        assert_eq!(find_datasheet("ABC123", Some("http://x/good.pdf"), &[], dest, &fetch).unwrap(), "http://x/good.pdf", "existing link wins");
        let e = format!("{:#}", find_datasheet("ABC123", None, &sources[..1], dest, &fetch).unwrap_err());
        assert!(e.contains("A: bad http://x/page"), "{e}");
        assert!(format!("{:#}", find_datasheet("ABC123", None, &[], dest, &fetch).unwrap_err()).contains("no distributor source"));
    }

    const PDF: &[u8] = b"%PDF-1.4 fake datasheet";

    /// Serves /ds.pdf, /page (HTML), /redir (to /ds.pdf) and 404 for anything else; returns the base URL.
    fn serve() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut req = [0u8; 2048];
                let n = s.read(&mut req).unwrap_or(0);
                let path = String::from_utf8_lossy(&req[..n]).split_whitespace().nth(1).unwrap_or("").to_string();
                let (head, body): (&str, &[u8]) = match path.as_str() {
                    "/ds.pdf" => ("200 OK\r\nContent-Type: application/pdf", PDF),
                    "/page" => ("200 OK\r\nContent-Type: text/html", b"<html>viewer</html>"),
                    "/redir" => ("302 Found\r\nLocation: /ds.pdf", b""),
                    _ => ("404 Not Found", b""),
                };
                let _ = write!(s, "HTTP/1.1 {head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(body);
            }
        });
        base
    }

    #[test]
    fn saves_pdfs_follows_redirects_and_refuses_the_rest() {
        let base = serve();
        let dir = crate::test_dir("download_pdf");
        for path in ["/ds.pdf", "/redir"] {
            let dest = dir.join("out.pdf");
            download_pdf(&format!("{base}{path}"), &dest).unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), PDF);
            std::fs::remove_file(&dest).unwrap();
        }
        for (path, why) in [("/page", "not a PDF"), ("/nope", "404")] {
            let e = download_pdf(&format!("{base}{path}"), &dir.join("bad.pdf")).unwrap_err();
            assert!(format!("{e:#}").contains(why), "{e:#}");
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "failed downloads leave nothing behind");
    }

    #[test]
    fn link_only_parts_get_a_local_pdf_and_failures_keep_their_link() {
        let base = serve();
        let sym = |mpn: &str, ds: &str| {
            format!(r#"(symbol "{mpn}" (property "Reference" "U" (at 0 0 0)) (property "MPN" "{mpn}" (at 0 0 0)) (property "Manufacturer" "ACME" (at 0 0 0)) (property "Footprint" "" (at 0 0 0)) (property "Datasheet" "{ds}" (at 0 0 0)))"#)
        };
        let lib = format!(
            "(kicad_symbol_lib (version 20241209)\n{}\n{}\n{}\n{}\n)",
            sym("GOOD", &format!("{base}/redir")),
            sym("HTML", &format!("{base}/page")),
            sym("GONE", &format!("{base}/nope")),
            sym("NONE", "~"),
        );
        let dir = crate::test_dir("download_missing");
        std::fs::create_dir_all(dir.join("lib/symbols")).unwrap();
        std::fs::write(dir.join("lib/symbols/ICs.kicad_sym"), lib).unwrap();
        let root = LibraryRoot::global(dir.join("lib"), dir.join("kicad"));

        let results = root.download_missing_datasheets();
        let outcome: Vec<_> = results.iter().map(|(mpn, r)| (mpn.as_str(), r.is_ok())).collect();
        assert_eq!(outcome, [("GOOD", true), ("HTML", false), ("GONE", false)]);

        let status = |mpn: &str| root.list_parts().into_iter().find(|p| p.mpn == mpn).unwrap();
        assert_eq!(status("GOOD").datasheet, Check::Ok);
        assert_eq!(std::fs::read(root.dir.join("datasheets/ACME_GOOD.pdf")).unwrap(), PDF);
        for mpn in ["HTML", "GONE"] {
            let p = status(mpn);
            assert_eq!(p.datasheet, Check::Link);
            assert!(p.datasheet_target.unwrap().starts_with(&base));
        }
        assert!(root.download_missing_datasheets().iter().all(|(mpn, _)| mpn != "GOOD"), "already local");
    }

    #[test]
    fn waits_are_reported_while_they_last_and_short_pacing_is_not() {
        note_wait("Quiet", Duration::from_millis(50), false);
        assert!(held().is_none_or(|(who, ..)| who != "Quiet"), "a short pacing gap is not worth showing");
        note_wait("Busy", Duration::from_secs(30), true);
        let (who, left, refused) = held().expect("a 30 s refusal is held");
        assert!(who == "Busy" && refused && left > Duration::from_secs(25));
    }
}
