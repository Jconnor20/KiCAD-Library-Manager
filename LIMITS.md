# Known limits and risks

A running list of what each merged work package left undone, cut short or made risky. Add to it when a WP merges, and strike an item through (or delete it) once it's fixed. Things that only need trying by hand go in [TESTING.md](TESTING.md) instead.

---

## Wave 3

### W3.1 Standardise to KLC + overrides
- **Wires can come loose.** Moving a footprint's origin or lengthening a pin moves its connection point, so schematics and boards that later update the part from the library may lose connections. The dry run lists every such change before anything is written.
- **Pin-1 marker on large pads.** On pads larger than about 6 mm, the added marker can land more than 2 mm from pin 1, and the W1.2 check may still report it as missing.
- **UI freeze.** Dry run and Apply run on the UI thread, so a very large library may freeze the window briefly. This is marked `shortcut:` in the code.
- **Naming rules not used yet.** W3.3's name template isn't applied during standardise. See the `TODO(W3.3)` in `standardise/mod.rs`.
- Left out on purpose: symbols with two pins or fewer, power symbols and hidden pins.

### W3.2 Alternatives and duplicates
- **No ungroup.** A group can only be changed by confirming a new one over it.
- **Rejected suggestions come back** on the next scan. Nothing remembers a rejection.
- **Packages are matched by footprint name**, so the same package under different vendor names (`R_0603` vs `RESC1608X55N`) won't group. This is marked `shortcut:` in the code.

### W3.3 Naming and category rules
- **The name template isn't applied anywhere.** The symbol name is the MPN, so renaming on import or edit would rewrite the MPN. The template is only exposed for standardise or a later rename feature to call.
- **Batch import and the CLI ignore the rules.** `batch::plan` and `klm-cli import` still fall back to the built-in `categories::guess`.

### W3.4 Search cache, history and favourites
- **No Settings control for the cache TTL.** It can only be changed through `cache_ttl_minutes` in `settings.json` (the default is a day, and 0 turns the cache off).
- **Stale stock and prices.** Within the TTL, cached results can show stock or prices that are out of date by up to a day.
- `Part` now derives serde for caching, so changing its fields can make old cache entries fail to load. When that happens it falls back to a fresh search.
