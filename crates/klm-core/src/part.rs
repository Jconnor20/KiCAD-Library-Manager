use std::path::PathBuf;

/// Everything known about one real part, plus the local files that came with it.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Part {
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    /// Free text from the source used to guess a category ("Resistor Chip", ...).
    pub category_hint: String,
    /// Where it came from, for display ("SamacSys ZIP: LIB_x.zip").
    pub source: String,
    /// Supplier part numbers written as symbol fields, e.g. ("LCSC", "C2040").
    pub supplier_ids: Vec<(String, String)>,
    pub datasheet_url: String,
    /// Product photo, for the Search tab thumbnails.
    pub image_url: String,
    /// Search-result details for display; not written to the library.
    pub package: String,
    pub stock: Option<u64>,
    /// Display text, e.g. "$0.0123 @100".
    pub unit_price: String,
    /// Full price list as (from quantity, unit price) in `price_currency`; empty when the source sent none.
    pub price_breaks: Vec<(u64, f64)>,
    pub price_currency: String,
    /// Smallest order quantity the source sells, when it says.
    pub moq: Option<u64>,
    /// Order multiple / standard pack size, when the source says.
    pub pack_qty: Option<u64>,
    /// Packaging ("Cut Tape", "Reel"), when the source says.
    pub packaging: String,
    /// Days until a part that is out of stock ships from the factory, when the source says.
    pub lead_time_days: Option<u32>,
    /// The source's other packagings of this part (DigiKey lists cut tape, reel...), each priced on its own.
    pub variations: Vec<Part>,
    pub symbol_file: Option<PathBuf>,
    pub footprint_files: Vec<PathBuf>,
    pub model_files: Vec<PathBuf>,
    pub datasheet_file: Option<PathBuf>,
    pub sim_files: Vec<PathBuf>,
    /// Non-fatal problems while fetching (a download that failed); shown to the user.
    pub warnings: Vec<String>,
}
