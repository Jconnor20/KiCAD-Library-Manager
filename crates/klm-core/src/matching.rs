//! Links a vendor ZIP to the search result the user clicked "Get…" on.

use crate::part::Part;

/// Case, `/`, spaces and `-` differ between distributors and vendor ZIPs for the same MPN.
fn key(mpn: &str) -> String {
    mpn.chars().filter(|c| !matches!(c, '/' | ' ' | '-')).flat_map(char::to_lowercase).collect()
}

pub fn same_mpn(a: &str, b: &str) -> bool {
    let k = key(a);
    !k.is_empty() && k == key(b)
}

/// Distributor data is cleaner than what ZIP authors type, so it wins where it has a value.
pub fn merge(zip: &mut Part, found: &Part) {
    for (to, from) in [
        (&mut zip.manufacturer, &found.manufacturer),
        (&mut zip.description, &found.description),
        (&mut zip.datasheet_url, &found.datasheet_url),
    ] {
        if !from.is_empty() {
            to.clone_from(from);
        }
    }
    if zip.category_hint.is_empty() {
        zip.category_hint.clone_from(&found.category_hint);
    }
    for id in &found.supplier_ids {
        if !zip.supplier_ids.iter().any(|(s, _)| *s == id.0) {
            zip.supplier_ids.push(id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpn_matching_ignores_case_slash_space_dash() {
        assert!(same_mpn("rc0603fr-0710kl", "RC0603FR0710KL"));
        assert!(same_mpn("STM32F103C8T6 ", "stm32 f103/c8t6"));
        assert!(!same_mpn("LM358", "LM3580"));
        assert!(!same_mpn("", "-"));
    }

    #[test]
    fn merge_prefers_distributor_values_and_keeps_zip_files() {
        let mut zip = Part {
            mpn: "LM358".into(),
            manufacturer: "?".into(),
            description: "zip text".into(),
            supplier_ids: vec![("Mouser".into(), "old".into())],
            ..Part::default()
        };
        let found = Part {
            manufacturer: "Texas Instruments".into(),
            datasheet_url: "https://x/ds.pdf".into(),
            category_hint: "ICs".into(),
            supplier_ids: vec![("DigiKey".into(), "296-1395-5-ND".into()), ("Mouser".into(), "new".into())],
            ..Part::default()
        };
        merge(&mut zip, &found);
        assert_eq!(zip.manufacturer, "Texas Instruments");
        assert_eq!(zip.description, "zip text");
        assert_eq!(zip.datasheet_url, "https://x/ds.pdf");
        assert_eq!(zip.category_hint, "ICs");
        assert_eq!(zip.supplier_ids, vec![("Mouser".into(), "old".into()), ("DigiKey".into(), "296-1395-5-ND".into())]);
    }
}
