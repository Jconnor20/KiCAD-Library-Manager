//! Sorting and filtering of search results, as plain functions so any front end can use them.

use crate::part::Part;
use std::cmp::Ordering;

/// Leading number of a source's price text ("$0.0123 @100", "0,10 €", "1.234,56 €"), ignoring the currency.
pub fn parse_price(text: &str) -> Option<f64> {
    // The part after '@' is the quantity break, not a price (Mouser can send " @1" with no price).
    let text = text.split('@').next()?;
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let run: String = text[start..].chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ',')).collect();
    let run = run.trim_end_matches(['.', ',']);
    let (dots, commas) = (run.matches('.').count(), run.matches(',').count());
    let decimal = match (dots, commas) {
        (0, 0) => None,
        (_, 0) => (dots == 1).then_some('.'),
        // shortcut: "1,234" reads as a thousands separator and "0,125" as a decimal; a 3-decimal price like "1,234" is misread.
        (0, _) => (commas == 1 && !is_thousands(run, ',')).then_some(','),
        // Both present: whichever comes last is the decimal point.
        _ => Some(if run.rfind('.') > run.rfind(',') { '.' } else { ',' }),
    };
    let clean: String = run.chars().filter_map(|c| if c.is_ascii_digit() { Some(c) } else { (Some(c) == decimal).then_some('.') }).collect();
    clean.parse().ok()
}

fn is_thousands(run: &str, sep: char) -> bool {
    run.split_once(sep).is_some_and(|(a, b)| b.len() == 3 && (1..=3).contains(&a.len()) && !a.starts_with('0'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Source,
    Mpn,
    Manufacturer,
    Description,
    Package,
    Stock,
    Price,
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    /// Source indexes the user unticked; a source not listed here is shown.
    pub hidden_sources: Vec<usize>,
    pub in_stock: bool,
    /// Exact manufacturer, empty for any.
    pub manufacturer: String,
    /// Case-insensitive substring of the package.
    pub package: String,
    /// Price text; empty or unparseable means no limit. Parts with no parseable price are hidden when set.
    pub max_price: String,
    /// Case-insensitive substring of MPN, manufacturer or description.
    pub text: String,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.hidden_sources.is_empty() && !self.in_stock && self.manufacturer.is_empty() && self.package.is_empty() && self.max_price.is_empty() && self.text.is_empty()
    }

    fn matches(&self, src: usize, p: &Part) -> bool {
        let has = |hay: &str, needle: &str| hay.to_lowercase().contains(&needle.trim().to_lowercase());
        let max = parse_price(&self.max_price);
        !self.hidden_sources.contains(&src)
            && (!self.in_stock || p.stock.is_some_and(|s| s > 0))
            && (self.manufacturer.is_empty() || p.manufacturer == self.manufacturer)
            && has(&p.package, &self.package)
            && max.is_none_or(|m| parse_price(&p.unit_price).is_some_and(|v| v <= m))
            && (self.text.trim().is_empty() || [&p.mpn, &p.manufacturer, &p.description].into_iter().any(|f| has(f, &self.text)))
    }
}

/// Indexes into `results` of the rows to show, filtered then sorted. Rows with no value for a
/// numeric key (no stock figure, unparseable price) go last in either direction.
pub fn view(results: &[(usize, Part)], filter: &Filter, sort: Option<(SortKey, bool)>) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..results.len()).filter(|&i| filter.matches(results[i].0, &results[i].1)).collect();
    let Some((key, ascending)) = sort else { return rows };
    let number = |i: usize| -> Option<f64> {
        let (_, p) = &results[i];
        match key {
            SortKey::Stock => p.stock.map(|s| s as f64),
            _ => parse_price(&p.unit_price),
        }
    };
    let text = |i: usize| {
        let p = &results[i].1;
        match key {
            SortKey::Mpn => &p.mpn,
            SortKey::Manufacturer => &p.manufacturer,
            SortKey::Description => &p.description,
            _ => &p.package,
        }
        .to_lowercase()
    };
    let flip = |o: Ordering| if ascending { o } else { o.reverse() };
    // Stable, so rows that tie keep their search order.
    rows.sort_by(|&a, &b| match key {
        SortKey::Stock | SortKey::Price => match (number(a), number(b)) {
            (Some(x), Some(y)) => flip(x.total_cmp(&y)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        },
        SortKey::Source => flip(results[a].0.cmp(&results[b].0)),
        _ => flip(text(a).cmp(&text(b))),
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(mpn: &str, mfr: &str, pkg: &str, stock: Option<u64>, price: &str) -> Part {
        Part { mpn: mpn.into(), manufacturer: mfr.into(), package: pkg.into(), stock, unit_price: price.into(), description: format!("{mpn} thing"), ..Part::default() }
    }

    fn sample() -> Vec<(usize, Part)> {
        vec![
            (0, part("B", "Yageo", "0603", Some(100), "$0.0123 @100")),
            (1, part("a", "TI", "SOT-23", None, "")),
            (1, part("C", "Yageo", "0402", Some(0), "$1.50 @1")),
            (0, part("D", "TI", "0603", Some(5000), "$0.10 @1")),
        ]
    }

    #[test]
    fn prices_from_every_source() {
        for (text, want) in [
            ("$0.0123 @100", 0.0123),
            ("$0.1 @1", 0.1),
            ("$0.52 @1", 0.52),
            ("0,10 € @1", 0.10),
            ("€0,125", 0.125),
            ("1.234,56 €", 1234.56),
            ("$1,234.50", 1234.5),
            ("1,234 €", 1234.0),
            ("US$ 5", 5.0),
        ] {
            assert_eq!(parse_price(text), Some(want), "{text}");
        }
        for text in ["", "n/a", "$ @1", " @1"] {
            assert_eq!(parse_price(text), None, "{text}");
        }
    }

    #[test]
    fn numeric_sort_puts_missing_last_both_ways() {
        let r = sample();
        let mpns = |v: Vec<usize>| v.into_iter().map(|i| r[i].1.mpn.clone()).collect::<Vec<_>>();
        assert_eq!(mpns(view(&r, &Filter::default(), Some((SortKey::Price, true)))), ["B", "D", "C", "a"]);
        assert_eq!(mpns(view(&r, &Filter::default(), Some((SortKey::Price, false)))), ["C", "D", "B", "a"]);
        assert_eq!(mpns(view(&r, &Filter::default(), Some((SortKey::Stock, false)))), ["D", "B", "C", "a"]);
        // 100 < 5000 numerically although "100" > "5000" as text sorts the other way round.
        assert_eq!(mpns(view(&r, &Filter::default(), Some((SortKey::Stock, true)))), ["C", "B", "D", "a"]);
    }

    #[test]
    fn text_sort_ignores_case() {
        let r = sample();
        let order: Vec<_> = view(&r, &Filter::default(), Some((SortKey::Mpn, true))).into_iter().map(|i| r[i].1.mpn.as_str()).collect();
        assert_eq!(order, ["a", "B", "C", "D"]);
        assert_eq!(view(&r, &Filter::default(), None), [0, 1, 2, 3]);
    }

    #[test]
    fn filters_combine_and_return_original_indexes() {
        let r = sample();
        let f = |f: Filter| view(&r, &f, None);
        assert_eq!(f(Filter { hidden_sources: vec![1], ..Filter::default() }), [0, 3]);
        assert_eq!(f(Filter { in_stock: true, ..Filter::default() }), [0, 3]);
        assert_eq!(f(Filter { manufacturer: "Yageo".into(), ..Filter::default() }), [0, 2]);
        assert_eq!(f(Filter { package: "06".into(), ..Filter::default() }), [0, 3]);
        assert_eq!(f(Filter { max_price: "0.5".into(), ..Filter::default() }), [0, 3]);
        assert_eq!(f(Filter { text: "ti".into(), ..Filter::default() }), [1, 3]);
        assert_eq!(f(Filter { text: "yag".into(), package: "0603".into(), ..Filter::default() }), [0]);
        assert!(Filter::default().is_empty());
        assert!(!Filter { in_stock: true, ..Filter::default() }.is_empty());
    }
}
