//! Part sources. Each search source (LCSC, DigiKey, …) implements [`Source`] and is listed in
//! [`all`]; the Search tab queries every enabled one. Vendor ZIPs go through `zip_import` instead.

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

/// Every source the user has enabled. Each phase adds its sources here.
pub fn all(_settings: &Settings) -> Vec<Box<dyn Source>> {
    vec![]
}
