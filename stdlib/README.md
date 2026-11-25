# KLM standard library manifests

`stdlib/` currently holds `Resistors.csv` only. The release zips include this folder: KLM looks for `stdlib/` next to the executable, then in the current folder, or pass `--manifest <dir>`.

Each `<Category>.csv` here becomes the KiCad library `KLM_STD_<Category>` when someone runs `klm-cli stdlib build`. Only part data is shipped. Symbols come from the user's own KiCad stock libraries, and footprints are referenced from them, so no CAD is redistributed.

## Columns

One row per orderable MPN. Rows with the same `part_id` are one generic part and its alternatives.

| Column | Fill on | Meaning |
|---|---|---|
| `part_id` | every row | Symbol name, e.g. `R_0603_10k_1pct`. Letters, digits, `_` and `.` only (no `%`). |
| `value` | preferred row | KiCad Value, e.g. `10k`, `100n`, `LM358`. |
| `symbol` | preferred row | Stock KiCad symbol, `Library:Name`, e.g. `Device:R`. |
| `footprint` | preferred row | Stock KiCad footprint, `Library:Name`, e.g. `Resistor_SMD:R_0603_1608Metric`. |
| `description` | preferred row | Short text: type, value, tolerance or rating, package. Quote it if it has commas. |
| `datasheet` | preferred row | The maker's datasheet URL (optional). |
| `mpn`, `manufacturer` | every row | The exact MPN and the maker's name as distributors give it. |
| `lcsc`, `digikey`, `mouser` | every row | Supplier part numbers. Leave one blank if that supplier doesn't sell the part. |
| `role` | every row | `preferred` (exactly one per part), `substitute` or `obsolete`. |

## What gets in

1. The part is active (no NRND, last-time-buy or obsolete notice).
2. It's stocked by at least two distributors.
3. A stock KiCad symbol and footprint fit it.
4. It has at least two makers where the part type allows (passives, discretes, regulators). Single-source ICs may stand alone.
5. The preferred row is a JLC basic part if there is one, then a JLC extended part, then the most widely stocked.

Alternates have to be true drop-ins: same value, the same or tighter tolerance, the same or higher voltage, current and power rating, the same dielectric for capacitors, the same package and the same pinout.

## How to add parts

**Never type an MPN or supplier number from memory.** Get each one from a lookup:

```
klm-cli lookup <mpn>...            # what LCSC / DigiKey / Mouser know about each MPN
klm-cli stdlib verify <Category>   # checks every row against the live sources
klm-cli stdlib verify --offline    # schema, roles, duplicates, stock symbol/footprint only
```

`verify` fails on errors: not found, an inactive lifecycle, a supplier number that disagrees, a missing symbol or footprint, or a bad group. Warnings name supplier numbers you can fill in ("LCSC can be filled: C25804"), possible maker or package mismatches, and parts with no alternates. A category is done when `verify` has 0 errors and you have worked through every warning.
