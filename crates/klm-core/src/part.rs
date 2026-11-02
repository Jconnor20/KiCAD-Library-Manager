use std::path::PathBuf;

/// Everything known about one real part, plus the local files that came with it.
#[derive(Clone, Debug, Default)]
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
    pub symbol_file: Option<PathBuf>,
    pub footprint_files: Vec<PathBuf>,
    pub model_files: Vec<PathBuf>,
    pub datasheet_file: Option<PathBuf>,
    pub sim_files: Vec<PathBuf>,
}
