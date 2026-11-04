# Known limits and risks

A running list of what each merged work package left undone, cut short or made risky. Add to it when a WP merges, and strike an item through (or delete it) once it's fixed. Things that only need trying by hand go in [TESTING.md](TESTING.md) instead.

---

## Wave 6

### W6.3 GUI test harness

Open UI bugs, each pinned by a `#[should_panic(expected = "KNOWN BUG")]` test in `crates/klm-gui/tests/gui/bugs.rs` (fix in W7, then delete the attribute). Details and snapshots: `docs/verification/W6.3-gui.md`.

- Settings API key boxes are 40 px wide (B1).
- Alternatives and Preview windows stay on screen after switching tab (B2).
- Missing glyphs (shown as empty boxes): `✔`, `▲`, `⇅`, `★`, `☆`, `👁` in the default egui fonts, so every status cell, sort arrow, star and the show-key button is a box (B4). The review's "purple ✔" was this box.
- Alternatives opens on the first part, not the selected one (B5).
- A part without a sim model shows orange "missing" (B6).
- Library table and Search results columns have unnamed combo/check boxes: screen readers see "ComboBox" and "CheckBox" with no name (B7).
- Not reproduced: Search header cut off, log over 25% of the height. At 820x560 and 1280x800 (1x scale) the header is inside the window and the log takes 21% / 15%. Not tried with results listed, nor at Windows display scaling above 100%.
- The harness does not cover: dialogs on the Import, History, Cleanup and BOM tabs, the "Close KiCad first" prompt, anything behind a native file picker (opened directly instead, e.g. the attach dialog), or dark/light themes.
- Tests share one sandbox and run one at a time (a lock); the full GUI suite takes about 80 s because of the offscreen wgpu rendering and `kicad-cli` previews.

### W6.0 Test sandbox

- Only `--config` / `KLM_CONFIG_DIR` runs are isolated. The older tests (`cli_e2e`, `cli_cleanup`, `snapshot_e2e`, `credentials::tests`) still use the real Credential Manager and let `kicad-cli` rewrite the real `kicad_common.json`.
- Sandbox API keys are plain text in `<dir>/credentials.json`; use it for test keys only.
- Temporary scratch folders (`klm-zip-*`, `klm-batch-*`, ...) still go to the OS temp dir, not the sandbox; they are unique per run and not user data.
- The GUI's native file dialogs open in the user's real folders and can browse anywhere; the sandbox does not restrict what the user picks.

### W6.4 Scale and KiCad-open checks

Timings: `cargo test --release -p klm-core --test scale_e2e -- --ignored scale --nocapture`, 2,000 parts in 8 categories, median of 3 runs, Windows 11. Full output in [docs/verification/W6.4-scale.md](docs/verification/W6.4-scale.md).

| Operation | Median ms | On the UI thread? | Note |
|---|---|---|---|
| `LibraryRoot::list_parts` | 345 | **yes, over 200 ms** | `klm-gui/src/tabs/library.rs:553`, `get_or_insert_with` in the Library tab's draw. Redone after every reset (`self.parts = None`, about ten sites), so an edit or refresh stalls the UI for this long. |
| `checks::check_library` | 382 | no | Runs on a worker (`library.rs:542`). Over 200 ms, but off the UI thread. |
| `alternatives::scan` + `suggest` | 53 | no | Runs on a worker (`alternatives.rs:36`). Store lookups per group are not timed. |

- **`list_parts` blocks the UI.** Move it to a worker like `check_library`, or cache the result across resets. Not fixed here (no feature work in Wave 6).
- **The scale fixture is written straight to disk.** `add_part` runs `kicad-cli` for each part, so 2,000 of them would take minutes. The reads all go through the real APIs. One footprint per package and one model per package is kinder than a real library.
- **No time budget is asserted.** The timing test prints numbers; it does not fail on them.
- **Pending queue, outside edit to a queued library:** passes. The queued library is added to the table as it is at apply time, so an edit made in between survives.
- ~~**Pending queue, outside edit after a queued table restore: data loss.**~~ Fixed in W6.4: `kicad_env::apply_table_op` splices entries that are neither in the restore text nor KLM-managed (descr "Managed by KiCad Library Manager") back into the restored table, and a queued removal keeps the table if such entries exist. Limit: an outside *change or removal* of an entry that the restore text also lists is still overwritten, and the immediate snapshot restore (`copy_replace`) still replaces the table as the user asked.
- **The KiCad-open branches are simulated.** Under a sandbox `kicad_running()` is always false, so `defer_table_op` and `register_or_queue` never queue. The replay tests write the queue files directly, in the format those functions write.
- **`klm-cli health` runs on projects only in the tests.** Global table checks are not covered.

## Wave 5

### W5.1 Search by part type
- **Category APIs.** DigiKey's category list and `CategoryFilter` search with an empty keyword were called live in W6.1 and work (the list nests under `Children`, which the parser had wrong; fixed). Nexar category browsing isn't built: the old unused parser was removed. The Nexar free plan's part quota was already spent, so `supCategories` itself is unverified; its fields (`id name parentId numParts path`) were checked against the schema.
- **Mouser has no category browsing.** Assumed from its docs; not checked live because no Mouser key was stored in W6.1.
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
- **KiCad open:** rollback, move-global and relink apply at once and queue their table edits in `pending-tables.json`. The queue is replayed after KiCad closes, but only when the GUI next re-checks. Outside additions in between are kept since W6.4 (see below); changes to existing entries are not. Never tried with real KiCad.
- **`klm-cli import`** that adds libraries still refuses while KiCad is open.

## Wave 4.5

- ~~**Symbol-less ZIPs.** Attaching a footprint-only or 3D-only ZIP to an existing part isn't built yet.~~ Fixed in W5.5: Attach… takes them. Importing one on its own still fails with "part has no symbol file" unless an Import dialog for the same MPN is open.
- **STL models.** These are attached with a warning. Whether KiCad's 3D viewer shows them isn't confirmed.
- **Formats still untested on real files:** EasyEDA Pro `.elibz2` (KiCad can't convert it; the app says so). Real `.SchLib`/`.PcbLib`/`.IntLib`, LCSC/EasyEDA JSON, legacy `.lib` and Ultra Librarian ZIPs are proven in `corpus_e2e.rs` (W6.2).
- **KiCad open.** New libraries are written while KiCad runs and re-checked when it closes. ~~Rollback, move-global and relink still ask you to close KiCad~~ (queued since W5.5). Old `${KLM_LIB}` path rewrites still ask you to close KiCad. It isn't known whether KiCad picks up a table edited while it runs.
- **Watcher.** It only sees ZIPs saved after the app starts, or after the folder list changes.
- **Search.**
  - Availability icons show what is known before import, not what the import will find.
  - ~~LCSC and Nexar MOQ and pack fields aren't filled.~~ Fixed in W5.5.
  - DigiKey `StandardPackage` is confirmed live (on each product variation; absent for tubes, so pack size stays blank).
- **Datasheet finder.** With no distributor source enabled, it can only retry existing links. Search results are used only when their MPN matches the part's.
- **History.**
  - Retention settings apply to every library.
  - Auto-version dates are in UTC. Weekly means 7 days and Monthly means 30.
  - Manifest edits made outside the app need **Refresh**.
- **Standards Apply and import can overlap.** Apply doesn't mark the app busy, so an import started during Apply could write a file mid-apply. Standards results are only collected while Settings is open.
- **Plugin.** It is untested in KiCad 10. If KiCad's `kicad-python` lacks a schematic call, the button opens KLM with no project.

## Wave 4

### W4.1 BOM import and enrichment
- **No country of origin.** The recorded DigiKey, LCSC/JLCPCB and Nexar-schema replies have no such field (W6.1), so the column stays missing. Mouser's documented reply (third-party docs; no key was available to record one) has `ProductCompliance` name/value pairs for HTS/ECCN-style codes and no country-of-origin field; if a live reply shows one, add it to `mouser.rs` with a fixture. `Mult` is documented as a string (like `Min`), which the parser reads; a live reply is still unrecorded.
- **Health resolves stock library variables from the installed KiCad only.** `${KICADn_SYMBOL_DIR}`, `_FOOTPRINT_DIR` and `_3DMODEL_DIR` map to the installed version's `share/kicad/...` whatever `n` is (Windows/Linux layout, not macOS). A variable that can't be resolved (no install, unknown name) is still trusted, not reported.
- **Lead time.** DigiKey `ManufacturerLeadWeeks` is confirmed live (a string of weeks). Mouser `LeadTime` is unverified (no Mouser key stored). LCSC has none. Nexar has `factoryLeadDays` per offer but isn't read.
- **Nexar free plan** allows 10 parts; once spent every search fails with that message, so Nexar values (stock, prices, MOQ, pack size) are unverified. Its field names were checked by schema introspection: `multipack` didn't exist and is now `multipackQuantity`.
- **DigiKey token** requests timed out at the app's 20 s limit in 3 of 5 live runs (other runs took under 2 s).
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
- **EasyEDA Pro libraries aren't proven.** Real Altium `.SchLib`/`.PcbLib`/`.IntLib` convert and render (W6.2). KiCad 9.0 and 10.0 both refuse an EasyEDA Pro `.elibz2` ("Unable to convert library"); the app reports that. A `.PcbLib` alone has no symbol, so its footprints convert but can't become a part.
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
