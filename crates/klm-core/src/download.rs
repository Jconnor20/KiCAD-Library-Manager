//! Downloading datasheet PDFs, for fresh LCSC imports and for parts that only have a datasheet link.

use crate::library::{AttachKind, Check, LibraryRoot};
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::time::Duration;

// Some datasheet hosts turn away anything that doesn't look like a browser.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// Saves the PDF at `url` to `dest`. Redirects are followed. Many "datasheet" links lead to an HTML
/// viewer page, so anything that doesn't start with `%PDF` is refused; `dest` is only written for a PDF.
pub fn download_pdf(url: &str, dest: &Path) -> Result<()> {
    let http = reqwest::blocking::Client::builder().user_agent(BROWSER_USER_AGENT).timeout(Duration::from_secs(60)).build().expect("HTTP client");
    let resp = http.get(url).send().with_context(|| format!("requesting {url}"))?;
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
        for p in self.list_parts() {
            let Some(url) = p.datasheet_target.as_ref().filter(|u| p.datasheet == Check::Link && u.starts_with("http")) else { continue };
            let result = download_pdf(url, &tmp).and_then(|()| self.attach_file(&p.category, &p.name, AttachKind::Datasheet, &tmp));
            out.push((p.mpn, result));
        }
        let _ = std::fs::remove_file(&tmp);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

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
}
