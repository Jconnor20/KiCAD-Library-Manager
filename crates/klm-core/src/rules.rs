//! User rules for naming parts and choosing their category. Stored in [`Settings`](crate::settings::Settings).

use crate::categories;
use crate::part::Part;
use serde::{Deserialize, Serialize};

/// Part text containing `keyword` (any case) goes to `category`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CategoryRule {
    pub keyword: String,
    pub category: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum MpnCase {
    #[default]
    Keep,
    Upper,
    Lower,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Rules {
    /// Checked in order; the first match wins, before the built-in guess.
    pub categories: Vec<CategoryRule>,
    /// e.g. `{Manufacturer}_{MPN}`; empty means just the MPN.
    pub name_template: String,
    pub mpn_case: MpnCase,
}

/// Placeholders a name template may use.
pub const TEMPLATE_FIELDS: &[&str] = &["MPN", "Manufacturer", "Package", "Category"];

impl Rules {
    /// The category a user rule gives for this text (case-insensitive substring), if any.
    pub fn rule_category(&self, text: &str) -> Option<&str> {
        let text = text.to_lowercase();
        self.categories
            .iter()
            .filter(|r| !r.keyword.trim().is_empty() && !r.category.trim().is_empty())
            .find(|r| text.contains(&r.keyword.trim().to_lowercase()))
            .map(|r| r.category.trim())
    }

    /// Category for a part: user rules over its hint, package, MPN and description, else the built-in guess.
    pub fn category_for(&self, part: &Part) -> String {
        let text = [&part.category_hint, &part.package, &part.mpn, &part.description].map(String::as_str).join(" ");
        self.rule_category(&text).map_or_else(|| categories::guess(&part.category_hint).to_string(), str::to_string)
    }

    pub fn normalise_mpn(&self, mpn: &str) -> String {
        let mpn = mpn.trim();
        match self.mpn_case {
            MpnCase::Keep => mpn.to_string(),
            MpnCase::Upper => mpn.to_uppercase(),
            MpnCase::Lower => mpn.to_lowercase(),
        }
    }

    /// Expands the name template from `(placeholder, value)` pairs (placeholder names match any case).
    /// Missing, empty and unknown placeholders vanish along with the separator they leave doubled; an
    /// empty result falls back to the normalised MPN. The MPN given is normalised before it is used.
    pub fn name(&self, fields: &[(&str, &str)]) -> String {
        let get = |k: &str| fields.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map_or("", |(_, v)| v.trim());
        let mpn = self.normalise_mpn(get("MPN"));
        if self.name_template.trim().is_empty() {
            return mpn;
        }
        let (mut out, mut rest) = (String::new(), self.name_template.as_str());
        while let Some(open) = rest.find('{') {
            let Some(len) = rest[open..].find('}') else { break };
            out.push_str(&rest[..open]);
            let key = &rest[open + 1..open + len];
            out.push_str(&if key.eq_ignore_ascii_case("MPN") { mpn.clone() } else { get(key).to_string() });
            rest = &rest[open + len + 1..];
        }
        out.push_str(rest);
        let is_sep = |c: char| matches!(c, '_' | '-' | ' ');
        let mut clean = String::new();
        for c in out.chars() {
            if !(is_sep(c) && clean.chars().last().is_none_or(is_sep)) {
                clean.push(c);
            }
        }
        let clean = clean.trim_end_matches(is_sep);
        if clean.is_empty() { mpn } else { clean.to_string() }
    }

    /// [`name`](Self::name) for a part imported into `category`.
    pub fn name_for(&self, part: &Part, category: &str) -> String {
        self.name(&[("MPN", &part.mpn), ("Manufacturer", &part.manufacturer), ("Package", &part.package), ("Category", category)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(template: &str) -> Rules {
        Rules { name_template: template.into(), ..Rules::default() }
    }

    #[test]
    fn category_rules_win_in_order_then_fall_back() {
        let r = Rules {
            categories: vec![
                CategoryRule { keyword: "SOT-23".into(), category: "Small".into() },
                CategoryRule { keyword: "resistor".into(), category: "Passives".into() },
                CategoryRule { keyword: " ".into(), category: "Ignored".into() },
            ],
            ..Rules::default()
        };
        let mut p = Part { category_hint: "Resistor Chip".into(), package: "sot-23-3".into(), ..Part::default() };
        assert_eq!(r.category_for(&p), "Small");
        p.package.clear();
        assert_eq!(r.category_for(&p), "Passives");
        p.category_hint = "Magnetometers".into();
        assert_eq!(r.category_for(&p), "Sensors");
        assert_eq!(Rules::default().category_for(&p), "Sensors");
    }

    #[test]
    fn template_expands_and_drops_missing_fields() {
        let f = [("mpn", "lm358"), ("Manufacturer", "TI")];
        assert_eq!(rules("{Manufacturer}_{MPN}").name(&f), "TI_lm358");
        assert_eq!(rules("{Manufacturer}_{Package}_{MPN}").name(&f), "TI_lm358");
        assert_eq!(rules("{Package}-{MPN}-{Nope}").name(&f), "lm358");
        assert_eq!(rules("{Package}").name(&f), "lm358");
        assert_eq!(rules("").name(&f), "lm358");
        assert_eq!(rules("x{MPN").name(&f), "x{MPN"); // unclosed brace is kept as typed
    }

    #[test]
    fn mpn_case_is_applied_in_names() {
        let r = Rules { mpn_case: MpnCase::Upper, name_template: "{Manufacturer}_{MPN}".into(), ..Rules::default() };
        assert_eq!(r.normalise_mpn(" lm358n "), "LM358N");
        assert_eq!(r.name(&[("MPN", "lm358n"), ("Manufacturer", "TI")]), "TI_LM358N");
        assert_eq!(Rules { mpn_case: MpnCase::Lower, ..Rules::default() }.normalise_mpn("AbC"), "abc");
        assert_eq!(Rules::default().normalise_mpn("AbC"), "AbC");
    }
}
