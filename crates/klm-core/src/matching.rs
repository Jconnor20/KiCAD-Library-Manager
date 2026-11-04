//! Links a vendor ZIP to the search result the user clicked "Get…" on.

use crate::part::Part;

/// Case, `/`, spaces and `-` differ between distributors and vendor ZIPs for the same MPN.
pub fn key(mpn: &str) -> String {
    mpn.chars().filter(|c| !matches!(c, '/' | ' ' | '-')).flat_map(char::to_lowercase).collect()
}

pub fn same_mpn(a: &str, b: &str) -> bool {
    let k = key(a);
    !k.is_empty() && k == key(b)
}

/// Component kind a value or package belongs to, read from the footprint name (R_0603, CAPC1608X55N…).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Res,
    Cap,
    Ind,
}

impl Kind {
    /// From a `Library:Name` footprint or bare name. None when it isn't a passive's footprint.
    pub fn of_footprint(footprint: &str) -> Option<Kind> {
        let name = footprint.rsplit_once(':').map_or(footprint, |(_, n)| n).to_ascii_lowercase();
        let starts = |ps: &[&str]| ps.iter().any(|p| name.starts_with(p));
        if starts(&["r_", "resc", "resm"]) {
            Some(Kind::Res)
        } else if starts(&["c_", "capc", "capm", "cp_"]) {
            Some(Kind::Cap)
        } else if starts(&["l_", "indc", "indm"]) {
            Some(Kind::Ind)
        } else {
            None
        }
    }
}

/// Decimal multiplier letters; `M` is mega and `m` milli. `K` is accepted for `k`.
fn multiplier(c: char) -> Option<i32> {
    Some(match c {
        'k' | 'K' => 3,
        'M' => 6,
        'G' => 9,
        'R' | 'r' => 0,
        'm' => -3,
        'u' => -6,
        'n' => -9,
        'p' => -12,
        _ => return None,
    })
}

/// Tokens that qualify a value without being part of it: tolerance, voltage ("16V", "6V3"), power
/// ("1/10W"), temperature coefficient and dielectric.
fn is_qualifier(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '/');
    l.contains('%')
        || l.starts_with('±')
        || l.contains("ppm")
        || ["x7r", "x5r", "x7s", "x6s", "np0", "c0g", "y5v", "z5u"].contains(&l.as_str())
        || l.strip_suffix('w').is_some_and(|b| numeric(b.trim_end_matches('m')))
        || l.split_once('v').is_some_and(|(a, b)| numeric(a) && b.chars().all(|c| c.is_ascii_digit()))
}

/// A value in base units as an exact decimal: `v:` + kind + mantissa + `e` + exponent, so 0.1uF, 100nF
/// and 100n are all `v:C1e-7`, and 4k7, 4.7kΩ and 4700 ohm are `v:R47e2`. `kind` (from the footprint) wins
/// over the unit letters; a unit or letter that contradicts it gives None. A bare number is only
/// understood for resistors ("100" could be pF or µF on a capacitor) unless `allow_bare`. Anything that is
/// not a lone number with an optional multiplier and unit (an MPN, "DNP", "10k 4k7") gives None.
/// Voltage, tolerance and dielectric words are ignored, so "100nF 16V" and "100nF 50V" are the same value.
pub fn value_key(value: &str, kind: Option<Kind>, allow_bare: bool) -> Option<String> {
    let mut toks: Vec<String> = vec![];
    for t in value.split(|c: char| c.is_whitespace() || c == ',') {
        if t.is_empty() || is_qualifier(t) {
            continue;
        }
        for part in t.split('/').filter(|p| !p.is_empty() && !is_qualifier(p)) {
            toks.push(part.replace(['µ', 'μ'], "u"));
        }
    }
    let mut t = match toks.as_slice() {
        [one] => one.clone(),
        [num, unit] if unit.chars().count() <= 4 && unit.starts_with(|c: char| c.is_alphabetic()) => format!("{num}{unit}"),
        _ => return None,
    };
    let lower = t.to_lowercase();
    let mut unit = None;
    // `lower` of Ω (U+03A9 or the ohm sign U+2126) is ω (U+03C9).
    for (suffix, k) in [("ohms", Kind::Res), ("ohm", Kind::Res), ("\u{3c9}", Kind::Res), ("f", Kind::Cap), ("h", Kind::Ind)] {
        if lower.ends_with(suffix) && lower.chars().count() > suffix.chars().count() {
            t = t.chars().take(t.chars().count() - suffix.chars().count()).collect();
            unit = Some(k);
            break;
        }
    }
    if unit.is_none() && lower.ends_with("meg") {
        t.truncate(t.len() - 3);
        t.push('M');
    }
    // After a unit the prefix letters may be upper case ("0.1UF").
    if unit.is_some() {
        t = t.chars().map(|c| if matches!(c, 'U' | 'N' | 'P') { c.to_ascii_lowercase() } else { c }).collect();
    }
    // Number [letter [digits]]: the letter is the multiplier, or the decimal point when digits follow ("1k5").
    let (mut int, mut frac, mut exp, mut letter, mut dot, mut lead_r) = (String::new(), String::new(), 0i32, None, false, false);
    for c in t.chars() {
        match c {
            '0'..='9' if letter.is_some() && dot => return None,
            '0'..='9' if letter.is_none() && !dot => int.push(c),
            '0'..='9' => frac.push(c),
            '.' if letter.is_none() && !dot => dot = true,
            // "R47" is 0.47 Ω.
            'R' | 'r' if letter.is_none() && int.is_empty() && !dot => {
                int.push('0');
                letter = Some(c);
                lead_r = true;
            }
            _ if letter.is_none() && !int.is_empty() => {
                exp = multiplier(c)?;
                letter = Some(c);
            }
            _ => return None,
        }
    }
    if int.is_empty() || (lead_r && frac.is_empty()) {
        return None;
    }
    let mut exp = exp - frac.len() as i32;
    if kind.or(unit) == Some(Kind::Ind) && matches!(letter, Some('R' | 'r')) {
        exp -= 6; // EIA inductor code: 4R7 is 4.7 µH
    }
    let implied = match letter {
        Some('k' | 'K' | 'M' | 'G' | 'R' | 'r') => Some(Kind::Res),
        Some('u' | 'n' | 'p') => Some(Kind::Cap),
        _ => None,
    };
    if letter.is_none() && unit.is_none() && kind != Some(Kind::Res) && !allow_bare {
        return None;
    }
    if kind.is_some() && unit.is_some() && kind != unit {
        return None;
    }
    if matches!(kind, Some(Kind::Res | Kind::Cap)) && implied.is_some() && implied != kind {
        return None;
    }
    // One form per value: no leading zeros, trailing zeros folded into the exponent.
    let mut digits = format!("{int}{frac}").trim_start_matches('0').to_string();

    if digits.is_empty() {
        digits = "0".into();
        exp = 0;
    } else {
        let kept = digits.trim_end_matches('0').len();
        exp += (digits.len() - kept) as i32;
        digits.truncate(kept);
    }
    let k = match kind.or(unit).or(implied) {
        Some(Kind::Res) => 'R',
        Some(Kind::Cap) => 'C',
        Some(Kind::Ind) => 'L',
        None => '?',
    };
    Some(format!("v:{k}{digits}e{exp}"))
}

fn alnum(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).flat_map(char::to_lowercase).collect()
}

/// Chip sizes as (imperial, metric) codes: the same package under both naming schemes.
const CHIP_SIZES: &[(&str, &str)] =
    &[("01005", "0402"), ("0201", "0603"), ("0402", "1005"), ("0603", "1608"), ("0805", "2012"), ("1206", "3216"), ("1210", "3225"), ("1812", "4532"), ("2010", "5025"), ("2512", "6332")];

/// Name prefixes after which a bare number is an imperial chip size.
const CHIP_PREFIXES: &[&str] = &["r", "c", "l", "led", "d", "fuse", "fb", "cp"];

/// IPC-7351 chip names: `RESC1608X55N` is metric 1608 (four digits then `x`; CAPCAE530X540N is not a chip).
const IPC_CHIP: &[&str] = &["resc", "capc", "indc", "ledc", "diodec"];

/// Families spelt differently by KiCad, IPC and distributors: canonical name and patterns over the name
/// reduced to lower-case letters and digits (`*` matches anything; `{n}` is each pin count).
const FAMILIES: &[(&str, &[&str])] = &[
    ("sot23", &["sot23", "sot233", "to236", "to236ab", "to236aa", "sc59"]),
    ("sot23-5", &["sot235", "sc74a", "sot753"]),
    ("sot23-6", &["sot236", "sc74"]),
    ("sot223", &["sot223", "sot2233", "sot2233*", "sot223tab*"]),
    ("sot89", &["sot89", "sot893", "sot893*"]),
    ("soic{n}", &["soic{n}", "so{n}", "soic{n}39x*", "soic127p600x*-{n}n"]),
    // 208 mil (5.3 mm body) sits between the 3.9 mm and 7.5 mm bodies and must not fall into either.
    ("soic{n}-208", &["soic{n}w53x*", "soic{n}53x*", "soic{n}n208", "soic127p800x*-{n}n"]),
    ("soic{n}w", &["soic{n}w", "so{n}w", "soic{n}w*", "soic127p1030x*-{n}n"]),
];
// Largest first, so the IPC pattern for 8 pins does not swallow "-18N".
const PIN_COUNTS: &[u32] = &[28, 24, 20, 18, 16, 14, 8];

fn glob(pattern: &str, s: &str) -> bool {
    match pattern.split_once('*') {
        Some((a, b)) => s.len() >= a.len() + b.len() && s.starts_with(a) && s.ends_with(b),
        None => pattern == s,
    }
}

/// Canonical names for a package (`chip:0603`, `sot23`), one per comma-separated alternative in `name`
/// ("SC-74A, SOT-753"); `raw:…` (the name reduced to letters and digits) when it isn't in the tables.
/// Exact family names are tried before globs, so SOT-23-5 never matches SOT-23.
pub fn package_ids(name: &str) -> Vec<String> {
    let name = name.rsplit_once(':').map_or(name, |(_, n)| n);
    name.split(',').filter(|p| !p.trim().is_empty()).map(|p| one_package(p.trim())).collect()
}

pub fn package_id(name: &str) -> String {
    package_ids(name).into_iter().next().unwrap_or_default()
}

fn one_package(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let toks: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).collect();
    let bare_ok = toks.first().is_some_and(|f| CHIP_PREFIXES.contains(f) || f.chars().all(|c| c.is_ascii_digit()));
    let size = |code: &str, metric: bool| CHIP_SIZES.iter().find(|(i, m)| code == if metric { *m } else { *i });
    for (n, t) in toks.iter().enumerate() {
        let metric_tok = t.strip_suffix("metric").filter(|d| !d.is_empty()).or_else(|| (toks.get(n + 1) == Some(&"metric")).then_some(*t));
        let ipc = IPC_CHIP.iter().find_map(|p| t.strip_prefix(p)).and_then(|r| r.split_once('x')).filter(|(d, _)| d.len() == 4 && d.bytes().all(|b| b.is_ascii_digit()));
        let hit = match (metric_tok, ipc) {
            (Some(d), _) | (None, Some((d, _))) => size(d, true),
            _ if bare_ok => size(t, false),
            _ => None,
        };
        if let Some((imp, _)) = hit {
            return format!("chip:{imp}");
        }
    }
    let k = alnum(name);
    // IPC names end "…X175-8N": keep the dash so the height's last digit doesn't merge into the pin count.
    let kd: String = lower.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    for exact in [true, false] {
        for (canon, pats) in FAMILIES {
            for &n in if canon.contains("{n}") { PIN_COUNTS } else { &[0] } {
                for pat in *pats {
                    let pat = pat.replace("{n}", &n.to_string());
                    let k = if pat.contains('-') { &kd } else { &k };
                    if if exact { pat == *k } else { pat.contains('*') && glob(&pat, k) } {
                        return canon.replace("{n}", &n.to_string());
                    }
                }
            }
        }
    }
    format!("raw:{k}")
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

    fn v(s: &str, k: Option<Kind>) -> Option<String> {
        value_key(s, k, false)
    }

    #[test]
    fn values_with_units() {
        let (r, c, l) = (Some(Kind::Res), Some(Kind::Cap), Some(Kind::Ind));
        for (a, b, k) in [
            ("0.1uF", "100nF", c), ("100n", "0.1µF", c), ("100000p", "0.10 μF", c), ("0.1UF", "100nf", c), ("4n7", "4.7nF", c), ("1u5", "1.5uF", c),
            ("4k7", "4.7kΩ", r), ("4700 ohm", "4K7", r), ("4k7", "4.7 kOhm", r), ("1R0", "1Ω", r), ("1R0", "1", r), ("100", "100R", r), ("0R", "0", r),
            ("1Meg", "1M", r), ("1k5", "1.5k", r), ("1k5", "1500", r), ("10k 1%", "10K", r), ("100nF 50V X7R ±10%", "100n/16V", c), ("10k 1% 1/10W", "10kΩ", r),
            ("10 kΩ", "10000", r), ("6V3 10uF", "10u", c), ("4R7", "4.7uH", l), ("1R", "1", r),
        ] {
            assert!(v(a, k).is_some(), "{a} should parse");
            assert_eq!(v(a, k), v(b, k), "{a} vs {b}");
        }
        assert_eq!(v("4k7", None), v("4.7kΩ", None), "kind from the letters when the footprint says nothing");
        for (a, b, k) in [("1m", "1M", r), ("2m2", "2.2", r), ("10k", "100k", r)] {
            assert_ne!(v(a, k), v(b, k), "{a} vs {b}");
        }
        assert_ne!(v("4R7", l), v("4R7", r), "an inductor's 4R7 is µH, not ohms");
        assert_ne!(v("4u7", c), v("4u7", l));
        assert_eq!(v("1M", c), None, "mega on a capacitor");
        assert_eq!(v("1mF", r), None, "unit contradicts the footprint");
        for bad in ["1N4148", "2N2222", "BC547", "1.5k5", "1k5k", "10kΩx", "DNP", "NC", "-", "", "10k 4k7", "LM358", "X7R", "RC0603FR-0710KL"] {
            assert_eq!(v(bad, None), None, "{bad}");
        }
        assert_eq!(v("100", c), None, "bare number on a capacitor is ambiguous");
        assert!(value_key("100", c, true).is_some());
        assert_eq!(v("100nF 16V", c), v("100nF 50V", c));
        assert_eq!(v("10 4k7", r), None, "two numbers are not a number and a unit");
        assert_eq!(v("R47", r), v("0R47", r));
        assert_eq!(v("R", r), None);
        assert_eq!(v("1m", r), Some("v:R1e-3".into()));
        assert_eq!(v("0.10uF", c), Some("v:C1e-7".into()));
    }

    #[test]
    fn package_aliases() {
        let same = |a: &str, b: &str| assert_eq!(package_id(a), package_id(b), "{a} vs {b}");
        let diff = |a: &str, b: &str| assert_ne!(package_id(a), package_id(b), "{a} vs {b}");
        same("R_0603", "RESC1608X55N");
        same("KLM:R_0603", "C_0603_1608Metric");
        same("R_0603", "0603 (1608 Metric)");
        same("R_0201_0603Metric", "RESC0603X30N");
        same("RESC0603X30N", "0201");
        same("R_0805", "CAPC2012X90N");
        diff("R_0201_0603Metric", "R_0603");
        diff("R_01005_0402Metric", "R_0402");
        diff("R_0402", "R_0603");
        diff("CAPCAE530X540N", "C_0805");
        diff("Crystal_SMD_3225-4Pin", "R_1206");
        same("SOT-23", "SOT-23-3");
        same("SOT-23", "TO-236AB");
        diff("SOT-23-5", "SOT-23");
        same("SOT-23-5", "SC-74A, SOT-753");
        same("SOIC-8", "SOIC127P600X175-8N");
        same("SOIC-8", "SOIC-8_3.9x4.9mm_P1.27mm");
        same("SO-16", "SOIC-16");
        same("SOIC-8", "SOIC127P600X171-8N");
        diff("SOIC-18", "SOIC127P600X171-8N");
        diff("SOIC-16W_7.5x10.3mm", "SOIC-16_3.9x9.9mm");
        diff("SOIC-8", "SOIC-14");
        same("SOIC-8W_7.5x5.0mm_P1.27mm", "SOIC127P1030X265-8N");
        same("SOIC-8W_5.3x5.3mm_P1.27mm", "SOIC127P800X215-8N");
        same("SOIC-8_5.3x5.3mm_P1.27mm", "SOIC-8W_5.3x5.3mm_P1.27mm");
        diff("SOIC-8W_5.3x5.3mm_P1.27mm", "SOIC-8W_7.5x5.0mm_P1.27mm");
        diff("SOIC-8W_5.3x5.3mm_P1.27mm", "SOIC-8_3.9x4.9mm_P1.27mm");
        diff("SOIC127P800X215-8N", "SOIC127P600X175-8N");
        diff("SOIC127P1030X265-8N", "SOIC127P800X215-8N");
        assert!(package_id("Weird-Pkg").starts_with("raw:"));
    }

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
