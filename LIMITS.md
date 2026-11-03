# Known limits and risks

A running list of what each merged work package left undone, cut short or made risky. Add to it when a WP merges, and strike an item through (or delete it) once it's fixed. Things that only need trying by hand go in [TESTING.md](TESTING.md) instead.

---

## Wave 5

### W5.1 Search by part type
- **Category APIs never called live.** The Nexar `supCategories`/`category_id` query and DigiKey's `CategoryFilter` search with an empty keyword follow the docs but are untested.
- **Mouser has no category browsing.** It's assumed to have no category list; not checked against its docs.
- **CAD icons** are known ahead of import only for LCSC/EasyEDA parts. Nothing ever shows a SPICE model.
- **No LCSC thumbnails.** The image field in LCSC's search reply is unknown.

### W5.2 House-style symbols
- **Wires can come loose.** Restyling moves pins, so schematics already using the symbol may need wires fixed.
- **Refused kinds:** MOSFETs, Zener/TVS/Schottky diodes, polarised caps, batteries, thermistors, multi-unit symbols, `extends` and power symbols.
- **No KiCad-open check on the Library-tab button** (the CLI has one). This matches the Edit action.
- **Not handled:** two visible pins stacked on one point. Templates aren't editable.
- **Tests:** one test briefly turns on the import setting and could race another importing test. The tests use KiCad's installed Device/Transistor_BJT libraries, and skip without KiCad.

### W5.3 Alternatives beyond the library
- **Equivalents are matched as text,** so "0.1uF" won't match "100nF". This is marked `shortcut:` in the code.
- The search fixture was written by hand, not captured live.

### W5.4 Clean up existing KiCad libraries
- **Only the newest KiCad version's config is scanned.** Older ones are listed as "not scanned".
- **Footprint-only libraries can't be moved into KLM.**
- **Path rules from memory.** How it resolves env vars, text variables and relative project paths comes from Opus's memory of how KiCad does it, not from KiCad's source.

### W5.5 Rest of W4.5
- **KiCad open:** rollback, move-global and relink apply at once and queue their table edits in `pending-tables.json`. The queue is replayed after KiCad closes, but only when the GUI next re-checks. A table edit made outside KLM in between is overwritten. Never tried with real KiCad.
- **`klm-cli import`** that adds libraries still refuses while KiCad is open.

## Wave 4.5

- ~~**Symbol-less ZIPs.** Attaching a footprint-only or 3D-only ZIP to an existing part isn't built yet.~~ Fixed in W5.5: Attach… takes them. Importing one on its own still fails with "part has no symbol file" unless an Import dialog for the same MPN is open.
- **STL models.** These are attached with a warning. Whether KiCad's 3D viewer shows them isn't confirmed.
- **Formats still untested on real files:** EasyEDA, and Altium files outside Ultra Librarian ZIPs.
- **KiCad open.** New libraries are written while KiCad runs and re-checked when it closes. ~~Rollback, move-global and relink still ask you to close KiCad~~ (queued since W5.5). Old `${KLM_LIB}` path rewrites still ask you to close KiCad. It isn't known whether KiCad picks up a table edited while it runs.
- **Watcher.** It only sees ZIPs saved after the app starts, or after the folder list changes.
- **Search.**
  - Availability icons show what is known before import, not what the import will find.
  - ~~LCSC and Nexar MOQ and pack fields aren't filled.~~ Fixed in W5.5.
  - The DigiKey `StandardPackage` field name is untested.
- **Datasheet finder.** With no distributor source enabled, it can only retry existing links. Search results are used only when their MPN matches the part's.
- **History.**
  - Retention settings apply to every library.
  - Auto-version dates are in UTC. Weekly means 7 days and Monthly means 30.
  - Manifest edits made outside the app need **Refresh**.
- **Standards Apply and import can overlap.** Apply doesn't mark the app busy, so an import started during Apply could write a file mid-apply. Standards results are only collected while Settings is open.
- **Plugin.** It is untested in KiCad 10. If KiCad's `kicad-python` lacks a schematic call, the button opens KLM with no project.

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
- ~~**UI freeze.**~~ Fixed in W4.5.8. Dry run and Apply run on the UI thread, so a very large library may freeze the window briefly. This is marked `shortcut:` in the code.
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
