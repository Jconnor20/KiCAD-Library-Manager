# Known limits and risks

A running list of what each merged work package left undone, cut short or made risky. Add to it when a WP merges, and strike an item through (or delete it) once it's fixed. Things that only need trying by hand go in [TESTING.md](TESTING.md) instead.

---

## Wave 4

### W4.1 BOM import and enrichment
- **No country of origin.** The DigiKey, Mouser and LCSC search replies don't seem to carry it, so the column is missing. Not checked against their API docs.
- **Lead-time fields unverified.** DigiKey `ManufacturerLeadWeeks` and Mouser `LeadTime` were coded from memory and never called live.
- **Shipping region is DigiKey only.** Mouser and LCSC ignore it (the BOM tab says so).
- **Fixed BOM field names:** `MPN`, `Manufacturer`, `LCSC`, `DigiKey`, `Mouser`. A project using other field names won't match.
- **Only DigiKey's first packaging variation is read**, so cut tape vs reel pricing may be wrong.
- **No rate limiting**, and Mouser's order multiples are ignored. A large BOM may hit API quotas.
- Currency: quotes that can't be converted are left out of totals, so coverage can drop when rates fail to load.
- Nexar is left out on purpose.

### W4.2 Project health report
- **Schematics aren't read.** A symbol placed in a schematic but missing from every library isn't found.
- **Orphans are matched by file name only**, so two same-named files in different folders hide each other. This is marked `shortcut:` in the code.
- A part with no footprint is reported twice (`footprint` and `part-check`).
- `klm-cli health` has no end-to-end test; only its argument parsing is tested.

### W4.3 Import from other tools
- **Altium and EasyEDA are untested on real files.** They go through `kicad-cli sym/fp upgrade`, but only Eagle `.lbr` and KiCad libraries are tested end to end. A fake `.SchLib` checks only the "can't convert" message.
- **Only the first 3D model** of a footprint with several is carried over.
- **Leftover temp folder** after a cancelled import review, until the next import finishes.
- **No choice of destination.** Parts go to the global library, or the project library when there is no global one.
- **Existing schematics keep the old library names**, so they don't point at the imported KLM copies.
- The last four review fixes weren't re-reviewed by Opus.

### W4.4 Portable export
- **Footprint name collisions:** two categories with a same-named footprint overwrite each other in the merged `.pretty`.
- **Relative 3D model paths** (`../../3dmodels/x.step`) aren't officially defined by KiCad, so Altium/Fusion/KiCad may not find the models.
- No "Export…" button in the GUI yet; CLI only.

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
