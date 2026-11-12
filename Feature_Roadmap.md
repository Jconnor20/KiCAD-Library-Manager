# KLM Feature Roadmap

Work is split into **waves**. Packages in the same wave don't share files, so each one can go to its own agent at the same time. Each wave branches from master once the wave before it is merged.

Each work package (WP) lists:
- **Owns**: the files it may change.
- **Needs**: the WPs that must be merged first.
- **Size**: S (under a day), M (1–3 days), L (several days).
- **Done when**: what has to be true before it merges.

Every WP also ends with `cargo test --workspace` passing and no build warnings. Your original roadmap items are tagged **(R1)–(R10)**. Ideas from the outside review are tagged **(Rev)**.

**How waves run:**
- Waves go in order. Each WP runs as its own session in its own worktree.
- Model: **Sonnet** by default, **Opus** only for WPs marked ⚑ (the hard ones).
- A WP whose **Needs** are already merged may start early, as long as it owns none of the same files as the wave in progress.

---

## Decisions made

- **Standardising (R4):** follow KiCad Library Conventions (KLC) by default, with overrides you can set in Settings.
- **Import (R5) and export (R6) targets:** existing KiCad libraries, Altium, Eagle/Fusion, EasyEDA.
- **Alternatives (R7):**
  - Use KLM's own **built-in part store** rather than a KiCad database library, so users don't need an ODBC driver installed.
  - The store is SQLite compiled into `klm.exe`, which adds no runtime dependency.
  - KiCad keeps reading normal `.kicad_sym` files. KLM writes the preferred MPN plus `MPN_Alt1…`, `Manufacturer_Alt1…` and their supplier-number fields into the symbol, so they show up in KiCad's BOM.
  - The store also holds what KiCad has no use for: preferred / substitute / obsolete flags, tags, search cache and history.
- **Symbol files stay the source of truth for KiCad.** The part store holds extra metadata and can always be rebuilt from the libraries. Losing it must never break a KiCad project.

- **Export (R6):** a portable bundle first. Native Altium/Eagle writers stay on the roadmap in Wave 5.
- **Shipping region (R8):** not fixed. It's where the user is, or where the board is made (e.g. JLCPCB assembly ships from China). So it's a Settings default with a per-project override, in W4.1.
- **CLI:** start small (`check`, `relink`, `import <zip>`) and grow it as features land.

- **Licence:** GPL-3.0-or-later, with `LICENSE` added in W8.5.

## Open questions

None at the moment.

---

## Wave 0: Groundwork (run here, one at a time, before any parallel work)

- [ ] **W0.1 Housekeeping** (S)
  - Push master to GitHub.
  - Delete the three finished phase branches and worktrees, and archive their sessions.
  - The `LICENSE` file waits until you pick a licence (open question 1).
- [x] **W0.2 Split the two hotspot files** (M). `main.rs` (772 lines) and `library.rs` (748 lines) are touched by almost every feature, so parallel agents would keep colliding there.
  - GUI: move each tab into its own module that owns its state and its background messages: `tabs/search.rs`, `tabs/import.rs`, `tabs/library.rs`, `tabs/settings.rs`. `main.rs` keeps only the app shell: top bar, log, KiCad-closed prompt and tab switching.
  - Core: split `library.rs` into `library/{mod, import, status, attach, copy, relocate}.rs`. No behaviour change.
  - **Done when:** all existing tests pass unchanged and every tab works as before.
  - Layout: GUI `main.rs` (shell: `AppCtx`, top bar, log, close-KiCad prompt) + `tabs/{search,import,library,settings}.rs`, each with its own state, UI and `Msg` enum wrapped by one top-level `Msg`; core `library/{mod,import,status,attach,copy,relocate,fields}.rs` with the public API unchanged.
- [x] **W0.3 Part store and CLI skeleton** (M)
  - `klm-core/src/store.rs`: a SQLite database (`rusqlite` with the `bundled` feature) in the user config folder. Tables for parts, alternatives, tags, search cache and history. Can be rebuilt from the libraries at any time.
  - New `crates/klm-cli` with a `klm-cli` binary and three basic commands:
    - `check`: prints the Library-tab status as text or JSON.
    - `relink`: re-adds missing KLM libraries to KiCad's tables, refusing while KiCad is open.
    - `import <zip> --category <c> --to global|project`.
    Later WPs add their own commands.
  - **Done when:** the store can be created, wiped and rebuilt from a library in a test, and `klm-cli check` runs.

## Wave 1: Small improvements and foundations (5 in parallel)

- [x] **W1.1 Search sort and filter (R1, R2)** (S)
  - **Owns:** `tabs/search.rs`
  - Click a column header to sort, ascending or descending.
  - Filters: source, in stock only, manufacturer, package, maximum unit price, plus a text box that filters within the results.
  - **Done when:** sorting and filtering work on mixed results from LCSC and the distributors. Price is sorted as a number, not as text.
- [x] **W1.2 Part checks (R10; Rev "validation")** (M)
  - **Owns:** `core/checks.rs` (new) and the status columns in `tabs/library.rs`.
  - Checks:
    - every symbol pin number has a footprint pad, and the other way round
    - no duplicate pin numbers
    - the required fields are present
    - the footprint has a courtyard and a pin-1 marker
    - the 3D model file loads
    - the model's size roughly matches the footprint's fab/courtyard outline
  - Results go into a "Checks" column with a list of issues on hover. Also add the `klm-cli check --deep` command.
  - **Done when:** each check has a test using a deliberately broken part.
- [x] **W1.3 Small fixes** (S)
  - **Owns:** `core/kicad_env.rs`, `core/library/relocate.rs`, `tabs/settings.rs`
  - A Settings field for the `kicad-cli` path, for when KiCad isn't installed in the usual place.
  - Moving the library to another drive from inside the app (copy, verify, then delete the original).
  - **Done when:** both are tested with temp folders.
- [x] **W1.4 Datasheet download for link-only parts** (S)
  - **Owns:** `core/download.rs` (new; moves LCSC's download helper here) and one button in `tabs/library.rs`, placed next to W1.2's column.
  - "Download missing datasheets": every part whose datasheet is only a web link gets a local PDF.
  - **Done when:** tested against a local HTTP server, and a failed download keeps the link.
- [x] **W1.5 Part metadata editor (Rev)** (M)
  - **Owns:** `gui/edit_dialog.rs` (new), `core/library/edit.rs` (new)
  - Edit MPN, manufacturer, description, category, supplier numbers, datasheet URL and tags after import.
  - Changing the category moves the part between category libraries, footprint and 3D model included.
  - Bulk editing for several selected rows.
  - **Done when:** a recategorised part still loads in KiCad (checked with `kicad-cli` render) and has no broken paths.

> W1.2 and W1.4 both add something to the Library tab. Give W1.2 the columns and W1.4 only a toolbar button so they don't clash.

## Wave 2: Better imports (4 in parallel)

- [x] **W2.1 Previews and model check (R9; Rev "preview before import")** (M)
  - **Owns:** `gui/preview.rs` (new) and the preview panel in `import_dialog.rs`
  - Render the symbol and footprint as SVG with `kicad-cli`, and the footprint with its 3D model with `kicad-cli pcb render`.
  - Show these in the import dialog and the Library tab, together with any W1.2 check warnings.
  - **Needs:** W1.2
- [x] **W2.2 Get from multiple sources (R3)** (L) ⚑
  - **Owns:** `core/sources/combine.rs` (new) and the per-file "from" picker in the import dialog
  - "Get…" searches every source by MPN and suggests the best file of each type, preferring a complete set from one source.
  - The dialog shows where each file comes from and lets you swap any of them.
  - It refuses a symbol/footprint mix that fails the pin/pad check.
  - **Needs:** W1.2, W2.1 (the import dialog is shared, so merge W2.1 first or agree the panel layout)
- [x] **W2.3 Batch operations and dry run (Rev)** (M)
  - **Owns:** `core/batch.rs` (new) and multi-select in `tabs/library.rs`
  - Import several ZIPs at once; bulk attach; bulk copy to project; bulk relink.
  - Every batch can run as a dry run first, showing exactly what would change.
  - **Needs:** W0.2
- [x] **W2.4 Snapshots and rollback (Rev)** (M)
  - **Owns:** `core/snapshot.rs` (new)
  - Automatic snapshot before any import, move, standardise or batch run.
  - A "History" list showing what changed, with rollback.
  - Optional: if the library folder is a git repository, commit each change.
  - **Needs:** W0.2

## Wave 3: Library quality (4 in parallel)

- [x] **W3.1 Standardise to KLC + overrides (R4)** (L) ⚑
  - **Owns:** `core/standardise/` (new), the "Standards" section of Settings
  - Automatic fixes for symbols: text sizes, pin length, line widths, field positions.
  - Automatic fixes for footprints: silkscreen/fab/courtyard line widths, reference and value placement, origin, pin-1 marking.
  - Each rule can be switched off or changed in Settings.
  - Always runs as a dry run first and takes a W2.4 snapshot.
  - **Needs:** W1.2, W2.4
- [x] **W3.2 Alternatives and duplicates (R7; Rev "dedup")** (L) ⚑
  - **Owns:** `core/alternatives.rs` (new), the "Alternatives" panel
  - Find likely-equivalent parts by normalised MPN, package and value.
  - You confirm each grouping. Nothing is ever merged automatically, because footprints may differ.
  - Mark parts as preferred, substitute or obsolete; stored in the part store.
  - Write the `MPN_AltN` and supplier fields into the symbol.
  - **Needs:** W0.3, W1.2
- [x] **W3.3 Naming and category rules (Rev)** (M)
  - **Owns:** `core/rules.rs` (new), the "Naming" section of Settings
  - Your own rules for choosing a category (keyword or package → category).
  - Name templates such as `{Manufacturer}_{MPN}`, and MPN case normalisation.
  - Used by import, the metadata editor and standardise.
  - **Needs:** W1.5
- [x] **W3.4 Search cache, history and favourites (Rev)** (M)
  - **Owns:** `core/cache.rs` (new) and the history/favourites side panel in `tabs/search.rs`
  - Cache distributor results for a set time to save API calls.
  - Recent searches, saved searches, favourite parts, recently imported.
  - **Needs:** W0.3, W1.1

## Wave 4: Projects, BOM and other tools (4 in parallel)

- [x] **W4.1 BOM import and enrichment (R8; Rev "BOM")** (L) ⚑
  - **Owns:** `core/bom.rs` (new), a new "BOM" tab
  - Read a project's BOM with `kicad-cli sch export bom`.
  - Match each line by MPN, manufacturer or supplier number.
  - Show missing CAD files, alternatives, stock, price at your quantity, lead time and country of origin.
  - Totals: unit cost, extended cost, distributor coverage, longest lead time.
  - Shipping region: a Settings default plus a per-project override (where you are, or where the board is made). Used for stock, lead time and shipping where the API supports it.
  - Export to CSV.
  - **Needs:** W3.2, working distributor keys
- [x] **W4.2 Project health report (Rev)** (M)
  - **Owns:** `core/health.rs` (new), `klm-cli health`
  - Scan a whole project for missing or broken symbols, footprints, 3D models, datasheets and sim models.
  - Also flag duplicate MPNs, inconsistent metadata, orphaned files and invalid library references.
  - Export as CSV or JSON. Usable in CI through the CLI.
  - **Needs:** W1.2
- [x] **W4.3 Import from other tools (R5)** (L) ⚑
  - **Owns:** `core/sources/foreign.rs` (new), "Import library…" on the Import tab
  - **Existing KiCad libraries** first, including your old `KiCAD_Library`: split into KLM categories, with fields filled in where possible.
  - Then Altium, Eagle/Fusion and EasyEDA libraries, converted through KiCad's own importers where `kicad-cli` supports them. Check each format first and say so where it doesn't.
  - **Needs:** W3.3 (categorising)
- [x] **W4.4 Portable export (R6)** (M)
  - **Owns:** `core/export.rs` (new)
  - A ZIP of the selected parts as standalone KiCad libraries with relative paths, importable by Altium and Fusion.
  - Add the `klm-cli export` command.

## Wave 4.5: Polish after Wave 4 (8 in parallel, ships 0.2.0)

- [x] **W4.5.0 Test-data corpus:** openly licensed fixtures in `tests/fixtures/third_party/`, plus a manifest of vendor downloads for a git-ignored `testdata/` (`docs/testdata-manifest.md`) and `corpus_e2e.rs`.
- [x] **W4.5.1 Version and plugin:** version 0.2.0 (every wave bumps the minor version), the plugin shows its errors, and there is a schematic button with a fallback.
- [x] **W4.5.2 Working with KiCad open:** new libraries are added straight away and re-checked once KiCad closes. The `kicad_running` check is cached, and the import queue tracks parts by id.
- [x] **W4.5.3 Settings:** Settings stays a popup. Wider API key fields with show/hide, and multiple watch folders.
- [x] **W4.5.4 Search:** sort hints, currency per row, MOQ and pack size, an Import button, availability icons, and favourites on right-click.
- [x] **W4.5.5 Import:** Ultra Librarian `.lbr` fix, Altium/Fusion vendor ZIPs, STL, picking files, and a `.LibPkg` hint.
- [x] **W4.5.6 Library:** detail panel and context menu, sorting and filters, and a "Find missing datasheets" that searches by MPN.
- [x] **W4.5.7 History:** automatic minors (count and size cap), named majors, and auto-versions at a regular interval.
- [x] **W4.5.8 Rough edges:** Alternatives and Standards run off the UI thread, the log has Clear and Copy, and labels are consistent.

## Wave 5: Features from the Wave 4 review (5 in parallel, ships 0.3.0)

- [x] **W5.1 Search by part type, with images:** browse and filter by category using the DigiKey/Mouser/Nexar category APIs, show thumbnails, and check what a part has (symbol, footprint, 3D, SPICE) before import.
  - Built: "Browse by category" for DigiKey and Nexar (Mouser has no category list), cached for 7 days. Thumbnails are cached in `thumbs/`, and CAD icons are known ahead of import only for LCSC/EasyEDA.
- [x] **W5.2 House-style symbols:** generate consistent symbols for each class (R, C, L, diodes, LEDs, transistors, IC boxes) from templates, whatever the source, mapping pins from the imported symbol.
  - Built: `symbols/` restyles R, C, L, diode, LED, NPN, PNP and IC box, refusing whenever a pin can't be placed with certainty. Available as "Restyle symbol", `klm-cli restyle`, and an opt-in import setting. Templates aren't editable in Settings.
- [x] **W5.3 Alternatives beyond the library:** search distributors for equivalents, define alternates by hand, and import them.
- [x] **W5.4 Clean up existing KiCad libraries:** scan every global and project library, report duplicates, broken entries and unused libraries, then remove or fix each one, or move it into KLM.
  - Built: `cleanup.rs`, a "Clean up" tab, and `klm-cli cleanup`, which only previews unless you pass `--apply`. Every table is backed up first and library files are never deleted.
- [x] **W5.5 Rest of W4.5:**
  - attach footprint-only or 3D-only ZIPs to an existing part
  - make rollback, move-global and relink safe while KiCad is open
  - LCSC/Nexar MOQ fields
  - show snapshot kind in `klm-cli history`

## Review after Wave 5 (2026-11-03)

- **Built against the original list:** R1, R2, R3, R9 and R10 are done. R4 and R7 mostly. R5, R6 and R8 are partial: Altium/EasyEDA never tried on real files, export is CLI-only, and there's no country of origin or shipping estimate.
- **Verified:** about 200 automated tests pass. **Nothing in [TESTING.md](TESTING.md) has been checked by hand**, and several distributor API fields were coded from memory and never called live (see LIMITS.md). Most of Waves 1–5 is "built, unverified".
- **The UI was opened and screenshotted (0.3.0, 1280×800 and the default 836×599).** What it showed:
  - **Settings is one long scrolling popup.** It's fixed at 700×520 whatever the window size, has no Close button (Esc or click outside only), and **its API key boxes are about 40 px wide**: the W4.5.3 "wider key fields" fix doesn't take, because `available_width()` inside the Grid cell is tiny. The sectioned Settings menu was never built; W4.5.3 kept the single popup.
  - **Floating windows leak across tabs.** Alternatives and Preview, opened on the Library tab, stay on top of History and other tabs.
  - **At the default size Search's header is cut off** and the log takes about a third of the height.
  - **Missing glyph:** "Recently imported" on Search shows an empty box (□) for its icon. "Recent searches" uses a red 📌 to mean "save", which reads as "already pinned".
  - **Sim model shows orange "missing" on every row,** although sim models are optional.
  - **Mixed signals:** ✔ draws purple, not the green the code asks for. "Clear" appears twice (project and log). "Relink libraries" appears twice and behaves differently (top bar: at once; Library: dry run first).
  - **Alternatives** opens on the first part (RP2040), not the selected one. Its label sits after its dropdown ("RP2040 (ICs) ▼ For part").
  - **Preview thumbnails** are about 140×105 px, so symbol text can't be read without enlarging.
  - **History** has three buttons on every row, and Delete sits next to Roll back. It doesn't show which files a snapshot changed.
  - **Not in the GUI at all:** export, health report, deleting a part. Standardise only runs from inside Settings.
- **Testing used the real config.** The GUI has no way to point at a test config, so opening it for a test uses your real library (this review only opened views and pressed Cancel).

## Wave 6: Verification and hardening (5 in parallel, ships 0.4.0)

No new features. Prove what's built works, fix what doesn't, and leave tests behind so it stays fixed. Findings that aren't fixed go into LIMITS.md.

- [x] **W6.0 Test sandbox (run first, here)** (S)
  - **Owns:** `core/settings.rs` (config dir), `gui/main.rs` (one argument)
  - A `KLM_CONFIG_DIR` env var / `--config <dir>` that moves `settings.json`, `parts.db`, caches, the pending-table queue and KiCad's config dir lookup into a temp folder, so tests and agents never touch the real library.
  - **Done when:** the GUI and `klm-cli` run end to end against a temp folder, and the real `%APPDATA%` folder is unchanged afterwards.
- [ ] **W6.1 Live API contract check** (M) ⚑
  - Status: DigiKey and LCSC confirmed against recorded replies; Nexar `multipack`, DigiKey category tree and LCSC image fixed. **Mouser blocked** (no key stored) and **Nexar blocked** (free plan quota); rerun `cargo test -p klm-core --test live_api -- --ignored live` once both are available.
  - **Owns:** `core/sources/*`, `core/bom.rs` field mapping, `core/tests/fixtures/live/`
  - Call DigiKey, Mouser, Nexar and LCSC once each with your keys. Record the replies (keys redacted) as fixtures, and fix every field that was guessed: DigiKey `ManufacturerLeadWeeks`, `StandardPackage`, `CategoryFilter`; Mouser `LeadTime` and category list; Nexar `supCategories`; LCSC image field; country of origin wherever an API has it.
  - Live calls go in `#[ignore]` tests (`cargo test -- --ignored live`); everyday tests replay the fixtures.
  - **Needs:** your API keys in Credential Manager.
  - **Done when:** every field listed in LIMITS.md as "untested" or "from memory" is either confirmed by a recorded reply or removed.
- [x] **W6.2 Real-file import corpus** (M)
  - **Owns:** `core/tests/corpus_e2e.rs`, `docs/testdata-manifest.md`, fixes in `core/sources/foreign.rs` and `zip_import.rs`
  - Real Altium `.SchLib`/`.PcbLib`/`.IntLib`, EasyEDA exports, an Ultra Librarian ZIP, a legacy `.lib` symbol and an STL-only model, each imported end to end and rendered by `kicad-cli`.
  - **Done when:** each format either imports and renders, or fails with the clear message, and the test says which.
- [x] **W6.3 GUI test harness** (M) ⚑
  - **Owns:** `crates/klm-gui/tests/` (new), `egui_kittest` as a dev-dependency, plus the smallest change needed to build `App` with a test config (after W6.0).
  - Headless tests that open every tab and every dialog through AccessKit names (as this review did by hand), and assert the bugs above stay fixed: key fields at least 300 px wide, no tab's window shown on another tab, nothing clipped at the default size. Save a PNG of each screen to `target/ui-snapshots/` for review.
  - **Needs:** W6.0
  - **Done when:** `cargo test -p klm-gui` opens all 6 tabs, Settings and every Library dialog without panicking, and saves their snapshots.
- [x] **W6.4 Scale and KiCad-open checks** (S)
  - **Owns:** `core/tests/scale_e2e.rs` (new), `crates/klm-cli/tests/cli_health.rs` (new)
  - Generate a 2,000-part library and time `list_parts`, `check_library` and an Alternatives scan. Record the numbers in LIMITS.md, and flag anything over 200 ms that runs on the UI thread.
  - An end-to-end test for `klm-cli health`, which has only argument-parsing tests today.
  - Replay `pending-tables.json` against a KiCad config that changed in between, and confirm the outside edit isn't lost silently.
- [ ] **W6.5 Manual pass (you)** (M)
  - Work through [TESTING.md](TESTING.md) with the latest build and real KiCad, ticking items or noting failures. Agent-findable bugs from W6.1–W6.4 should already be fixed, so this pass is about KiCad itself: the plugin, the PCM install, placed parts, KiCad 10.
  - **Done when:** every TESTING.md item is ticked or has a LIMITS.md entry.

## Wave 7: Clean UI (5 in parallel after W7.0, ships 0.5.0)

Driven by the screenshots above. Each WP checks its own tab with the W6.3 harness and adds the snapshots to its merge.

- Status (2026-11-04): all six merged and checked together. Test plans, results and before/after snapshots are in `docs/ui-review/` (W7.0–W7.5 and W7-integration). Still open: accessible names (b7), Restyle applying the reviewed result, a per-part datasheet finder, saving a BOM alternate swap. Not yet checked by hand.

- [x] **W7.0 Shared look (run first, here)** (S)
  - **Owns:** `gui/tabs/ui_kit.rs` (new), `gui/main.rs` (fonts and visuals only)
  - One place for status cells (✔ / missing / broken / optional "—"), badges, a sortable header, and a font fallback that has every glyph we use (fixes the □). The colours for ok/warn/error/info.
  - A `TableBuilder` wrapper (`egui_extras`, already a dependency) that only draws visible rows, with resizable columns and a fixed header.
  - The shared hooks other WPs call, so they don't all edit `main.rs`: `cx.open_settings(Section)` (an empty `Section` enum that W7.1 fills in) and `cx.log(Level, …)`.
- [x] **W7.1 Settings as a sectioned popup, like Altium's Preferences or Fusion's** (L)
  - **Owns:** `gui/tabs/settings.rs`, `gui/tabs/standards.rs`, `gui/tabs/settings/` (new, one file per page if it grows)
  - **Stays a popup** (modal over the app), sized to about 80% of the window, resizable, with an **✕ close button in its title bar** as well as Esc.
  - **Left:** a search box over a tree of sections. Typing filters the tree to pages whose labels match. **Right:** the selected page, with a title and its options in titled groups, in two columns when wide enough.
  - **Tree:**
    - **General:** Libraries (global location, Change location…), Watch folders, KiCad (`kicad-cli` path and Test)
    - **Sources:** one page each for LCSC, DigiKey, Mouser, Nexar and SnapMagic (enabled, keys, Test with the result shown on the page: ✔ 25 results / ✘ reason)
    - **Search:** cache time (today only in `settings.json`), default display currency
    - **Import:** category rules, name template, MPN case, house-style symbols
    - **Standards:** Symbols, Footprints
    - **BOM:** shipping region (and field names after W8.4)
    - **History:** retention and auto versions (moved from the History tab)
  - **Footer:** "Restore defaults" for the page shown, then OK / Cancel / Apply. The popup edits a copy of `Settings`: Apply or OK saves it, Cancel or ✕ throws it away. API keys are the one exception: "Save keys" writes to Credential Manager at once, as now.
  - API key boxes fill the row (no Grid around them), with show/hide.
  - **Open at a section:** fills in W7.0's `cx.open_settings(Section)` for any tab to call (Search's "no sources", BOM's region, Import's watch folder). This replaces the old Wave 6 nice-to-have.
  - History's retention controls move here from the History tab. Standards keeps its rules here; its Dry run/Apply moves to the Library tab (W7.3).
  - **Done when:** the harness opens every page by clicking the tree and through the search box, Cancel leaves `settings.json` unchanged, and a key box is at least 300 px at the default window size.
- [x] **W7.2 App shell** (M)
  - **Owns:** `gui/main.rs`
  - Default size 1200×800, and size and position remembered (eframe `persistence`).
  - Log: coloured by severity, a smaller default height, and an "N errors" badge in the top bar that opens it, using W7.0's `(Level, String)` messages.
  - **Floating windows belong to their tab:** hidden when you switch away, and back when you return. Or make them modals.
  - Opens on Library once set up (Import only for first run). One Relink (the dry-run one). Rename the project's "Clear" to "Close project".
  - First-run checklist: global library → KiCad found → a source enabled → a watch folder, each linking to its place.
- [x] **W7.3 Library tab** (L) ⚑
  - **Owns:** `gui/tabs/library.rs`, `edit_dialog.rs`, `preview.rs`, `alternatives.rs`, `batch_dialog.rs`
  - Moves to the W7.0 table: select all/none, shift-click ranges, and parts loaded off the UI thread.
  - Sim model shows "—" when absent (it's optional). Status cells offer the fix on click: missing datasheet → find it, missing 3D → attach.
  - Toolbar: **Standardise…** (dry run, then apply), **Export selected…** (W4.4 core), **Health report…** (W4.2 core), **Delete** (after W8.1, or hidden until then).
  - Restyle gets the same dry-run review as the rest.
  - Alternatives opens on the selected part, with its label before the dropdown. Preview thumbnails at least 240 px, with a larger default window.
- [x] **W7.4 Search tab** (M)
  - **Owns:** `gui/tabs/search.rs`
  - W7.0 table, so nothing is cut off at the default size. An empty-state message before the first search.
  - An **"In library"** badge on results whose MPN you already have, with "Open in Library".
  - Side panel: ☆ to save a recent search, a clear "Saved" list, and working icons for "Recently imported".
  - CAD-availability as text badges (Sym / FP / 3D / SPICE) instead of 🔣⏚🧊. "(native)" in the price header becomes the currency, or "as listed".
  - "No sources enabled" links to Settings → Sources.
- [x] **W7.5 Import, History, BOM and Clean up** (M)
  - **Owns:** `gui/tabs/import.rs`, `history.rs`, `bom.rs`, `cleanup.rs`
  - Import: "Import folder…" and "Import files…" buttons instead of the "files" checkbox.
  - History: actions in a row menu (⋯ or right-click), Delete kept away from Roll back, and a "what changed" list of files per snapshot.
  - BOM: sortable and filterable (shortfalls only). Missing parts get "Search" and "Import". The Alternatives cell can swap in an alternate. Region links to Settings when empty.

## Wave 8: Closing the gaps (5 in parallel, ships 0.6.0)

What the original plan or earlier WPs promised that isn't there yet.

- Status (2026-11-04): all five merged and the version is 0.6.0. A fix round then closed the WPs' follow-ups: Reject and Ungroup in the Alternatives tab, the Delete dialog (label, shared files, planning on a worker), stale and alias-proof rejections, SOIC body widths, `{Package}` on edit, a real-import test, health resolving KiCad path variables, and parallel test runs no longer sharing temp folders (also an export bug). The full suite passes in parallel.
- Still open: country of origin (no supplier reply carries it; see LIMITS.md), Mouser replies never checked live (no Mouser key), `KICADn_*` variables ignore the version, `{Package}` on edit for distributor-imported parts, rarer SOIC widths, the Delete preview's handling of 3D models shared with a deleted footprint, process-id-only temp paths in `bom.rs` and `download.rs`, and the 240 px thumbnail test under load. Not yet checked by hand in the app or KiCad.

- [x] **W8.1 Delete a part** (S)
  - **Owns:** `core/library/remove.rs` (new), `klm-cli remove`
  - Remove a part's symbol, and its footprint, 3D model, datasheet and sim model when no other part uses them, after a snapshot. Removing it from the store and alternatives too.
- [x] **W8.2 Rules everywhere (W3.3 leftovers)** (M)
  - **Owns:** `core/batch.rs`, `core/rules.rs`, `klm-cli import`
  - Batch import and `klm-cli import` use your category rules. The name template is applied on import and edit as a separate symbol name, so the MPN field is never rewritten. Standardise's `TODO(W3.3)` closed.
- [x] **W8.3 Better alternatives (W3.2 / W5.3 leftovers)** (M) ⚑
  - **Owns:** `core/alternatives.rs`, `core/matching.rs`
  - Unit-aware values (0.1uF = 100nF, 4k7 = 4.7kΩ). Package aliases (`R_0603` = `RESC1608X55N`). Ungroup, and rejected suggestions remembered in the store.
- [x] **W8.4 BOM and health (W4.1 / W4.2 leftovers)** (M)
  - **Owns:** `core/bom.rs`, `core/health.rs`
  - Configurable BOM field names per project. Rate limiting and Mouser order multiples. All DigiKey packaging variations (cut tape vs reel). Country of origin wherever W6.1 found it.
  - Health reads the schematics (placed symbols missing from every library). Orphans matched by full path, not file name. A part with no footprint reported once.
- [x] **W8.5 Export and docs (R6 leftovers)** (S)
  - **Owns:** `core/export.rs`, `README.md`, `LICENSE`
  - No footprint name collisions in the merged `.pretty`, and 3D paths checked in KiCad and Fusion.
  - README rewritten for 0.6.0: every tab, every CLI command, and no "developed on other branches". Licence chosen (open question 1) and `LICENSE` added.

## Wave 9: House standard profiles (W9.1 first, then 3 in parallel, ships 0.7.0)

The house standard for each component type, so that whatever the source, every part of a type looks and reads the same. Everything lives in one **Standard profile** file (`config_dir()/standards/<name>.json`): the KLC `Rules`, per-class templates, field schema and Description/Value/Keywords templates, naming, and fab/IPC choice. There are built-in profiles plus company profiles, and you share one by copying the file.

- [x] **W9.1 Profile file (run first, here)** (M)
  - **Owns:** `core/standardise/profile.rs` (new), `core/settings.rs`, `gui/tabs/standards.rs`
  - Profile load/save, built-in profiles, a one-time migration from `Settings.standards`, an active-profile picker, and Import/Export.
- [x] **W9.2 Reference-part templates** (L) ⚑
  - **Owns:** `core/symbols/template.rs`, `core/symbols/mod.rs`, `core/standardise/footprint.rs`
  - `Template::from_symbol` takes the body, pin slots and field positions from a symbol you drew in KiCad, with the built-in templates as fallbacks. "Use as house template for <class>" in the Library tab. Footprints copy the style only (text, layers, line widths, fab outline, pin-1 mark).
  - Done: `Profile.templates` (class -> symbol s-expression text) and `Profile.footprint_styles` (class -> `FootprintStyle`: line width per layer, Reference/Value layer, font and place relative to the courtyard, pin-1 marker shape). Restyle and Standardise use them; Settings > Symbols/Footprints "House templates" puts a class back to the built-in. Not done: IC symbol templates (still auto-boxed), fab outline style, the `${REFERENCE}` fab text.
- [x] **W9.3 Part information per class** (L) ⚑
  - **Owns:** `core/fields.rs` (new), `core/checks.rs`, `core/standardise/mod.rs`
  - Each class has a field schema (e.g. capacitor: Capacitance, Voltage, Tolerance, Dielectric, Package). Every field records whether it is required and visible, its unit and its format.
  - Description (`ki_description`), Value and Keywords (`ki_keywords`) are built from templates.
  - Autofill from source parameters (DigiKey, Mouser, Nexar, LCSC) through a name mapping and unit normalisation.
  - A consistency check flags missing fields, a bad unit, text that doesn't match its template, or a field that disagrees with the source. A Standardise fix fills and rebuilds, with a preview. Applied on import when house style is on.
- [x] **W9.4 Profile editors** (M)
  - **Owns:** `gui/tabs/settings/pages.rs`, a new class-editor page
  - Add, remove and reorder fields. Required and visible toggles, template editing with a live preview on a real part, the source mapping, and reset to defaults.
- Status (2026-11-08): all four merged and the version is 0.7.0. A fix round then made Standardise leave Reference and Value where a house template puts them, made the Visible switch write KiCad's hide flag (a field shown for the first time goes below Value), and made renaming a field update its placeholders. Known limits: ICs still get the automatic box, Value is only rebuilt for R/C/L, the Mouser and Nexar parameter mappings are thin or unverified, the editor's preview uses sample values, and `check_against_source` is still unused (see below). Not yet tried by hand in KiCad.

## Wave 10: Manufacturer and IPC checks (W10.0 first, then 3 in parallel, ships 0.8.0)

- [x] **W10.0 Fab data (research first)** (S)
  - **Done:** `fab_profiles.json` (standard 2-layer process, checked 2026-11-08), `fab_data::{fabs, limits}`, Manufacturer group on Settings › Footprints. Unpublished, left null: PCBWay via drill/pad, mask web, edge; OSH Park via pad, silk text height; Eurocircuits track, clearance, drill, ring, via (class table is only an image); Aisler via pad. IPC-6012 class 3 ring 0.05 mm is from secondary sources (the standard is paywalled).
  - **Owns:** `core/fab_profiles.json` (new), `core/fab_data.rs` (contract laid first: `FabChoice` in the profile, `FabLimits`, `fabs()`, `limits()`), a Manufacturer group in Settings › Standards
  - JLCPCB, PCBWay, OSH Park, Eurocircuits and Aisler minimums: trace and space, drill, annular ring, via, silk, mask web and edge clearance. Each fab has its source URL and the date checked. Adds IPC-6012 class 1/2/3 and IPC-7351 density A/B/C tables. Choosing several fabs means the strictest value wins.
- [x] **W10.1 Library footprint fab check** (M)
  - **Owns:** `core/fab.rs` (new)
  - Checks pad gap, drill, annular ring, silk width and height, silk on pads, and courtyard excess. Safe fixes (silk width, clip silk, grow courtyard) join the Standardise plan; everything else is flagged.
  - **Done:** `fab::check_footprint` runs in `check_part` (warnings) when the active profile names a fab. Pad gaps are exact for rect, roundrect, circle and oval at any rotation; trapezoid/custom pads count as their size rectangle, inner layers are ignored. Silk-on-pad uses the pad box plus mask web (0.1 mm default) and only straight silk; only `fp_line` is clipped. The courtyard grows by moving its outermost points to the 0.01 mm grid; round courtyards are not grown. Fixes run only with a fab chosen, so default profiles don't rewrite every footprint.
- [x] **W10.2 Whole-project check** (M) ⚑
  - **Owns:** `core/project_check.rs` (new), `klm-cli check-project`
  - Writes a `.kicad_dru` and board minimums from the profile, then runs `kicad-cli pcb drc` and `sch erc` (JSON) and maps the results into health findings. In "All fabs" mode it lists which fabs pass.
  - **Done:** `project_check::{dru, check, check_all}` run KiCad's DRC/ERC on a temp copy (the project's own `.kicad_dru` is kept and the fab rules appended; `${KIPRJMOD}` in its lib tables points back at the real folder). Rule syntax checked with kicad-cli 9.0 and 10.0.7, which silently drop a rules file they can't parse. No rule for the mask web (board setup only) or courtyard excess; sub-sheets in subfolders aren't copied.
- [x] **W10.3 Align parts to the fab** (M)
  - **Owns:** `core/bom.rs`, `gui/tabs/alternatives`
  - Per BOM line: JLC basic/extended/preferred, stock, lifecycle and obsolescence. "Swap" opens Alternatives filtered to "available at <assembler>" with the same footprint.
  - Done: `Part`/`Quote` carry `jlc_type` (LCSC `componentLibraryType` + `preferredComponentFlag`) and `lifecycle` (DigiKey `ProductStatus.Status`, Mouser `LifecycleStatus`); `bom::assembly` maps them, JLC stock is the LCSC quote's `stockCount`. Unknown stays unknown. BOM tab has a JLCPCB column, a summary line and Swap (Library tab's Alternatives panel with "Available at JLCPCB" and "Same package" ticked and the search started). No extended-part fee is shown (no source for it). Known limits: Swap needs the part in the library; "not at JLC" is only "out of stock at JLC"; Mouser's lifecycle wording is unverified; cache keys bumped to v3/v4 so old entries refetch.
- Status (2026-11-08): all four merged and the version is 0.8.0. An Opus review of the two Sonnet packages checked the JLCPCB and Aisler numbers against their pages and fixed three things: the fab table is parsed once, Swap opens the library that has the part (and never searches for a different one), and "Inactive" isn't read as active. Not yet tried by hand or on a real project.

## Not added yet (left out of Waves 9 and 10)

What the plan or the work packages called for and the product doesn't do yet, so nothing is lost when the sessions are archived. Ticking one means it was built later.

**Not built at all**
- [ ] **Built-in fab and IPC profiles.** Only KLC is built in. The plan also listed "JLC prototype" and "IPC Class 3". A fab choice on any profile now covers most of this, so it may not be needed.
- [ ] **Naming rules in the profile.** Naming stays in `Settings.rules`, so a shared profile doesn't carry it.
- [ ] **Project check in the app.** `check-project` and `--all-fabs` are CLI only; the GUI has no button or results view for them.
- [ ] **Source cross-check.** `checks::check_against_source` (a field disagreeing with the source data for its MPN) is written and tested but nothing calls it, so the Library tab never shows it.
- [ ] **Symbol template preview** on the Part information page. The page edits fields and text templates only.
- [ ] **Extended-part setup fee** in the BOM summary. No source gives it.

**Built, with gaps**
- [ ] **ICs have no house template.** They still get the automatic box.
- [ ] **Value is rebuilt only for resistors, capacitors and inductors.**
- [ ] **Parameter autofill:** Mouser fills almost nothing and the Nexar mapping is unverified against a real reply.
- [ ] **Template preview uses sample values,** not a part from the library.
- [ ] **Fab data gaps:** values not published by the fab are empty (PCBWay via drill and pad, mask web, edge; OSH Park via pad and silk text height; Eurocircuits track, clearance, drill, ring and via, since its table is only an image; Aisler via pad). The IPC-6012 class 3 ring (0.05 mm) and IPC-7351 courtyard excess come from secondary sources, because the standards are paywalled.
- [ ] **Footprint fab check approximations:** trapezoid and custom pads count as their size rectangle; inner copper layers, drill offsets and silk text stroke width aren't checked; silk circles and arcs are never flagged; only `fp_line` silk is clipped off pads (rectangles and polygons stay flagged); round courtyards aren't grown.
- [ ] **Project check gaps:** no rule for mask web (KiCad reads it only from board setup) or courtyard excess; the silk width rule covers silk text only (KiCad has no silk line constraint); sub-sheets kept in subfolders aren't copied, so ERC misses them; KiCad 9 labels our violations "board setup constraints".
- [ ] **BOM assembly status:** "not stocked by JLC at all" can't be told apart from "LCSC not asked or offline", so only "out of stock at JLC" is flagged; Mouser lifecycle wording is matched loosely (no saved reply); Swap needs the part in the library and finds it with a scan on the UI thread.

**Not checked**
- [ ] **By hand:** none of Waves 9 and 10 has been tried in the app, in KiCad, or on a real project. In particular, KiCad reading the `(hide yes)` flag Standardise writes, and Standardise on an existing library (it will now list "hidden" for every class field that is shown, and fab fixes once a fab is chosen).
- [ ] **Flaky timing test:** `requests_are_spaced_by_the_interval_but_cache_hits_are_not` failed once at 308 ms against a 290 ms limit while several builds ran at once. It passed in the full runs since.

## Wave 11: 3D fit and compliance (2 in parallel, ships 0.9.0)

- [x] **W11.1 3D fit check** (M)
  - **Owns:** `core/model3d.rs` (new)
  - An approximate STEP/WRL bounding box from the point data (no new dependency), compared with the fab outline and pads after the model's offset, rotate and scale. The fix centres the model and picks the best 90° rotation. This makes W1.2's "model size roughly matches the outline" true; there is no code for it today.
  - **Done:** `model3d::check` runs in `check_part` (warning) and `model3d::fix` joins the Standardise plan, for the footprint's first model. The STEP box is every `CARTESIAN_POINT` except axis-placement locations, plus the extent of each `CIRCLE`; units from the header (mm, inch, m). The WRL box is the `point [...]` lists at 0.1 inch per unit. The box goes through scale, then rotate (x, y, z), then the offset, with the model's y flipped against the footprint's. The model must be at least the F/B.Fab outline and at most outline plus pads (leads count), each within 25%; without a fab layer, between half the pads and the pads. Centre more than 0.25 mm off, or a size that only fits after a quarter turn, is flagged; the fix sets the offset x/y (and Z rotation +90°) and only runs when the size fits, so a wrong model is warned about but never moved. Known limits: nested STEP transforms and WRL `Transform` nodes aren't followed; `.stpz`/`.wrz` are skipped (no flate2 dependency); only multiples of 90° are exact for X/Y rotation; rotations follow KiCad's `CalcModelMatrix` (angles negated, so +90° is clockwise from above), not yet confirmed on a real model in the 3D viewer; there is no Settings switch for the fix.
- [x] **W11.2 Compliance badge** (M)
  - **Owns:** `core/compliance.rs` (new), `gui/tabs/library.rs`
  - A part is compliant when its Standardise plan is empty and its checks and fab check pass for the active profile. Cached by mtime. Badge column, a "Non-compliant" filter, and a JSON/CSV report through `health`.
  - **Done:** `compliance::{compute, Cache, findings}`: compliant = no plan change for the part (its symbol lines, or its footprint's) and no `check_part` issue of any severity. The Library tab has a Standard column (OK / No, reasons in the tooltip, "…" while computing, on a worker thread) and a "Non-compliant" filter; `health` adds one `compliance` warning per non-compliant part, so the existing `--json`/`--csv` carry it. Known limits: the cache is per library, not per part (the plan and checks run on the whole library, so any changed mtime of a symbol library, footprint, model or the profile redoes all of it); a profile edit in memory counts as a new profile; model mtime is found from the footprint's first `(model "…")` with `${KIPRJMOD}` only; `health` scans the project library only.
- Status (2026-11-08): both merged and the version is 0.9.0. An Opus review checked the model transform against KiCad's `CalcModelMatrix` and fixed the turn direction (KiCad negates the angles, so the fix had been placing turned models on the wrong side), and tidied the Library tab cell. Compliance now includes the model-fit check. Not yet tried by hand, in KiCad's 3D viewer, or on a real library.

## Wave 12: Editors and BOM import (ships 0.10.0)

- [x] **W12.1 Edit in KiCad round trip** (S): open the part in KiCad's symbol or footprint editor, watch the file, and re-check and re-badge when it is saved.
  - **Done:** `kicad_edit::{target, command, launch, FileWatch}`; the Library tab's part actions gain "Edit symbol in KiCad" (runs `eeschema` with the category's `.kicad_sym`, since KiCad can't open one symbol from the command line) and "Edit footprint in KiCad" (`pcbnew` with the part's `.kicad_mod`), both found next to `kicad-cli`. The tab polls the opened files' mtimes once a second and, on a save, reloads; the compliance cache is keyed on mtimes, so the Standard badge and checks refresh with no separate invalidation. Known limits: not tried against a real KiCad (whether `eeschema`/`pcbnew` open a bare library or footprint file as the standalone editor is unconfirmed, and macOS binary names differ); watching lasts until the app closes; a failed launch is only logged.
- [x] **W12.2 IPC-7351 footprint generator** (L): chip, SOD, SOT, SOIC, QFN, QFP and BGA from datasheet dimensions and the profile's density.
  - **Done:** `footprint_gen::generate` builds chip (RESC), SOD, SOT-23 (3/5/6 pins), SOIC, QFN (with exposed pad), QFP and BGA `.kicad_mod` files from datasheet min/max dimensions and the profile's density (`fab.density`, default B), named by IPC-7351 (`SOIC127P600X175-8N`). Toe/heel/side goals, the Z/G/X formulas with the RMS S term, and tolerances F 0.05 / P 0.025 are cited per table in the module; courtyard excess comes from `fab_data`. The result goes through the profile's own footprint rules (`standardise::fix_footprint`: line widths, silk clipped off pads, courtyard, text, pin-1 marker), and pads are capped to the pitch less the fab's copper clearance. `klm-cli footprint-gen <family> --category … --dry-run|--overwrite` and a Footprint tab write into the global or project library. An Opus review checked the formulas against kicad-footprint-generator's tables; its fixes are in. Known limits: the standard itself was not read, so goals marked "recalled" (chip, BGA pad ratio, F/P) are unverified and the golden tests are the formulas worked by hand, not IPC data (SOIC-8 lands at 1.50 mm pads on 2.70 centres vs PCB Libraries' 1.55 / 2.70); chips smaller than 1608, pull-back QFN, J-lead/SOJ/PLCC and BGA non-collapsing balls are not modelled; chip is always RESC; QFN exposed pad has full paste and no exposed-pad-to-signal gap check; no preview panel (PreviewPanel needs a part in a library), the tab shows the name and the profile fixes only; no 3D model.
- [x] **W12.3 In-app symbol editor** (L): only if W12.1 isn't enough.
  - **Done:** `symbols::edit` (`pins`/`set_pin`/`add_pin`/`remove_pin`, `body`/`set_body`, `text_at`/`set_text_at`, `load`/`save`) edits one symbol's s-expression and leaves everything else as it was; positions snap to 1.27 mm. `save` takes a snapshot (History undoes it) and refuses if the file changed since `load`. The Library tab's "Edit symbol…" opens `tabs/symbol_editor.rs`: preview, pin table (number, name, type, side, x, y, length, add/remove), body rectangle, Reference/Value text positions, "Apply house style" (reuses `symbols::restyle`, applied to the form until Save). Save reloads the Library tab, so checks and the compliance badge refresh. Test: add/move/remove pin, resize body and move text round-trip through the file.
  - **Known limits:** single-unit symbols with a rectangle body only (no drawing of lines/arcs; first rectangle is the body); the preview is KiCad's render of the saved file, so it updates on Save, not while dragging; renumbering pins as a swap (1<->2) is refused (rename through a spare number); not checked by hand in the GUI.
- [x] **W12.4 BOM to library** (M)
  - **Owns:** `core/bom_import.rs` (new), `gui/tabs/bom_import.rs` (new), `klm-cli bom-import`
  - Import a BOM (CSV or Excel/OpenDocument spreadsheet) and build a library from it. For a workbook the sheet and header row are detected (the sheet and row naming the most BOM fields) and can be picked by hand (GUI: Sheet and Header row controls; CLI: `--sheet`, `--header-row`). Columns (MPN, manufacturer, LCSC/DigiKey/Mouser number, quantity, designators, value, footprint) are detected from the header and can be remapped. Each line is looked up in the standard library by MPN first; otherwise each enabled source is searched by supplier number, then MPN. Lines with no match or several different matches are reported and left out.
  - **Done:** `bom_import::{detect, set_column, items_of, resolve, generate, write_back}`. Source-resolved parts are fetched (LCSC includes the EasyEDA conversion) and imported through `batch::plan_with`/`apply`, so naming rules, categories, snapshots and field filling are the normal import path; Standardise then runs on the libraries written and the added parts' compliance is counted. The CLI is `bom-import <csv> [--map field=column]… [--category] [--to] [--project] [--overwrite] [--manifest] [--out] [--dry-run]`; the GUI tab "BOM to library" has Open, a Columns picker, Look up (worker thread), a per-line status badge (stdlib / source / not found / ambiguous), Generate, and Save BOM with library parts (adds "KLM status" and "Library part" columns). Known limits: spreadsheets are read with `calamine` (values only; formulas give their cached result, a merged cell fills only its top-left), and "Save BOM with library parts" still writes CSV with the rows above the header dropped; a stdlib hit is referenced as `KLM_STD_<Category>:<part_id>` in the write-back, not copied into the new library (run `stdlib build` for those); the BOM's manufacturer only narrows candidates when it matches one, so differently spelled makers can show as ambiguous; the same part is fetched once per line, not once per MPN; the fetch comes from the source that found the part only; no tests for the generate step (needs network CAD).
- 3D CAD editing is not planned (it needs a CAD kernel). W11.1 covers positioning, and the model can be opened in FreeCAD.

## Later

- [ ] **Team / shared libraries (Rev):** a shared folder or git repository, conflict detection, locking, rules for merging metadata.
- [ ] **KiCad 9 output:** write files that KiCad 9 can still open.
- [ ] **macOS and Linux builds,** and a cross-platform PCM package.
- [ ] **Native Altium and Eagle/Fusion export (R6)** ⚑: write `.SchLib`/`.PcbLib` and `.lbr` directly.
- [ ] **Shipping cost and delivery time estimates (R8 extension),** once W8.4 is in use.
- [ ] **SnapMagic search inside the app,** if they grant API access.
- [ ] **More CLI commands** as needed, added WP by WP.

---

## Test pass (you, any time)

The full checklist of what's built but not yet checked by hand is in [TESTING.md](TESTING.md).
