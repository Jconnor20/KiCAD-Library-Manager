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

## Open questions

1. **Licence:** still undecided. `Cargo.toml` says GPL-3.0-or-later for now, and there's no `LICENSE` file until you choose.

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

- [ ] **W3.1 Standardise to KLC + overrides (R4)** (L) ⚑
  - **Owns:** `core/standardise/` (new), the "Standards" section of Settings
  - Automatic fixes for symbols: text sizes, pin length, line widths, field positions.
  - Automatic fixes for footprints: silkscreen/fab/courtyard line widths, reference and value placement, origin, pin-1 marking.
  - Each rule can be switched off or changed in Settings.
  - Always runs as a dry run first and takes a W2.4 snapshot.
  - **Needs:** W1.2, W2.4
- [ ] **W3.2 Alternatives and duplicates (R7; Rev "dedup")** (L) ⚑
  - **Owns:** `core/alternatives.rs` (new), the "Alternatives" panel
  - Find likely-equivalent parts by normalised MPN, package and value.
  - You confirm each grouping. Nothing is ever merged automatically, because footprints may differ.
  - Mark parts as preferred, substitute or obsolete; stored in the part store.
  - Write the `MPN_AltN` and supplier fields into the symbol.
  - **Needs:** W0.3, W1.2
- [ ] **W3.3 Naming and category rules (Rev)** (M)
  - **Owns:** `core/rules.rs` (new), the "Naming" section of Settings
  - Your own rules for choosing a category (keyword or package → category).
  - Name templates such as `{Manufacturer}_{MPN}`, and MPN case normalisation.
  - Used by import, the metadata editor and standardise.
  - **Needs:** W1.5
- [ ] **W3.4 Search cache, history and favourites (Rev)** (M)
  - **Owns:** `core/cache.rs` (new) and the history/favourites side panel in `tabs/search.rs`
  - Cache distributor results for a set time to save API calls.
  - Recent searches, saved searches, favourite parts, recently imported.
  - **Needs:** W0.3, W1.1

## Wave 4: Projects, BOM and other tools (4 in parallel)

- [ ] **W4.1 BOM import and enrichment (R8; Rev "BOM")** (L) ⚑
  - **Owns:** `core/bom.rs` (new), a new "BOM" tab
  - Read a project's BOM with `kicad-cli sch export bom`.
  - Match each line by MPN, manufacturer or supplier number.
  - Show missing CAD files, alternatives, stock, price at your quantity, lead time and country of origin.
  - Totals: unit cost, extended cost, distributor coverage, longest lead time.
  - Shipping region: a Settings default plus a per-project override (where you are, or where the board is made). Used for stock, lead time and shipping where the API supports it.
  - Export to CSV.
  - **Needs:** W3.2, working distributor keys
- [ ] **W4.2 Project health report (Rev)** (M)
  - **Owns:** `core/health.rs` (new), `klm-cli health`
  - Scan a whole project for missing or broken symbols, footprints, 3D models, datasheets and sim models.
  - Also flag duplicate MPNs, inconsistent metadata, orphaned files and invalid library references.
  - Export as CSV or JSON. Usable in CI through the CLI.
  - **Needs:** W1.2
- [ ] **W4.3 Import from other tools (R5)** (L) ⚑
  - **Owns:** `core/sources/foreign.rs` (new), "Import library…" on the Import tab
  - **Existing KiCad libraries** first, including your old `KiCAD_Library`: split into KLM categories, with fields filled in where possible.
  - Then Altium, Eagle/Fusion and EasyEDA libraries, converted through KiCad's own importers where `kicad-cli` supports them. Check each format first and say so where it doesn't.
  - **Needs:** W3.3 (categorising)
- [ ] **W4.4 Portable export (R6)** (M)
  - **Owns:** `core/export.rs` (new)
  - A ZIP of the selected parts as standalone KiCad libraries with relative paths, importable by Altium and Fusion.
  - Add the `klm-cli export` command.

## Wave 5: Later

- [ ] **Team / shared libraries (Rev):** a shared folder or git repository, conflict detection, locking, rules for merging metadata.
- [ ] **KiCad 9 output:** write files that KiCad 9 can still open.
- [ ] **macOS and Linux builds,** and a cross-platform PCM package.
- [ ] **Native Altium and Eagle/Fusion export (R6)** ⚑: write `.SchLib`/`.PcbLib` and `.lbr` directly.
- [ ] **Shipping cost and delivery time estimates (R8 extension),** once W4.1 is in use.
- [ ] **SnapMagic search inside the app,** if they grant API access.
- [ ] **More CLI commands** as needed, added WP by WP.

---

## Test pass (you, any time)

- [ ] Install the KiCad plugin from `dist\klm-pcm-0.1.0.zip` and click the toolbar button in the PCB editor.
- [ ] Add DigiKey, Mouser and Nexar keys in Settings and use **Test** on each.
- [ ] Retry attaching the TE switch datasheet.
- [ ] LCSC imports of a connector, a multi-unit op-amp and a part with odd-shaped pads.
- [ ] One Ultra Librarian ZIP.
