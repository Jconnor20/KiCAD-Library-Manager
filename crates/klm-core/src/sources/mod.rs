//! Part sources. Each search source (LCSC, DigiKey, …) implements [`Source`] and is listed in
//! [`all`]; the Search tab queries every enabled one. Vendor ZIPs go through `zip_import` instead.

pub mod digikey;
mod http;
pub mod mouser;
pub mod nexar;
pub mod watcher;
pub mod zip_import;

use crate::part::Part;
use crate::settings::Settings;
use anyhow::Result;
use std::path::Path;

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
        "DigiKey" => Box::new(digikey::DigiKey::from_credentials()?),
        "Mouser" => Box::new(mouser::Mouser::from_credentials()?),
        "Nexar" => Box::new(nexar::Nexar::from_credentials()?),
        _ => return None,
    })
}

/// Every source the user has enabled. Each phase adds its sources here.
pub fn all(settings: &Settings) -> Vec<Box<dyn Source>> {
    ["DigiKey", "Mouser", "Nexar"].into_iter().filter(|n| settings.is_enabled(n)).filter_map(build).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cad_url_encodes_the_mpn() {
        let part = Part { mpn: "SN74LVC1G17DBVR/A B".into(), ..Part::default() };
        let url = snapmagic_url(&part).unwrap();
        assert!(url.starts_with("https://www.snapeda.com/search/?q=SN74LVC1G17DBVR%2FA+B&search-type=parts"), "{url}");
    }
}
