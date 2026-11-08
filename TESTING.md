# Manual test checklist

These have passed automated tests but haven't been checked by hand in the app or in KiCad. Tick each one off as you go. If something fails, note what you did and what the log panel at the bottom of KLM said.

**Before you start**
- **Run the latest build:** `cargo run --release -p klm-gui`, or the `.exe` in `target\release\`.
- **Your library is safe:** KLM takes a snapshot before every change. If a test goes wrong, roll it back from the **History** tab.
- **Close KiCad** whenever KLM asks. KiCad overwrites its library list while it's open.

---

## KiCad integration

- [ ] **Install the toolbar plugin.** In KiCad: *Plugin and Content Manager → Install from File…*, then pick `dist\klm-pcm-0.4.0.zip`. Expect it to install with no "metadata" error.
- [ ] **Toolbar button.** Open a board in the PCB editor (KiCad's API must be on: *Preferences → Plugins*) and click **Library Manager**. Expect KLM to open with that board's project already chosen.
- [ ] **New category library.** Import a part into a new category while KiCad is open. Expect no close-KiCad prompt, and "N pending" in the top bar. After you reopen KiCad, the new `KLM_<Category>` library is in the symbol chooser.
- [ ] **Placed part is complete.** Place an imported part from KiCad's symbol chooser. Expect its footprint to be assigned, the 3D viewer to show its model, and *Show datasheet* to open the local PDF.
- [ ] **Re-link.** Delete a `KLM_…` entry in KiCad's *Manage Symbol Libraries*, close KiCad, then click **Re-link libraries** in KLM. Expect the entry back.

## Sources

- [ ] **LCSC, several part types.** Search and import a connector, a multi-unit op-amp (e.g. LM358 in a dual package) and a part with odd-shaped pads. Expect the preview and the KiCad footprint to match the datasheet drawing: pin positions, pad sizes, pin-1 location.
- [ ] **Ultra Librarian ZIP.** Download one part's KiCad ZIP. Expect it to appear under **Import** and import cleanly. `corpus_e2e.rs` proves it on four real Ultra Librarian ZIPs (KiCad, Altium, Fusion, combined); the GUI click-through is still yours.
- [ ] **Legacy `.lib` symbol.** Import a ZIP whose symbol is an old-style `.lib` (older SamacSys/UL downloads). Expect the pin/pad check to run on it rather than say it can't be read. `corpus_e2e.rs` proves import and rendering on KiCad's real legacy `Regulator_Linear.lib`; the pin/pad check inside the GUI is still yours.
- [ ] **DigiKey.** Enter your client ID and secret in **Settings → DigiKey**, then click **Test**. Expect results for LM358. Then search a real part from the Search tab.
- [ ] **Mouser.** Same as DigiKey, with your API key.
- [ ] **Nexar (Octopart).** Same as DigiKey, with your client ID and secret.
- [ ] **Keys are stored safely.** After saving a key, close and reopen KLM. Expect it to show "stored", and the key not to appear in `%APPDATA%\KiCAD-Library-Manager\settings.json`.

## Search tab

- [ ] **Sorting.** Click each column header. Price and stock should sort as numbers, and a second click reverses the order.
- [ ] **Filters.** Try source, in stock only, manufacturer, package, maximum price and the text box. Expect "N of M results" to update, and **Clear filters** to reset everything.
- [ ] **Get… after sorting.** Sort or filter the results, then click **Get…**. Expect the part you clicked, not whichever part used to be in that row.
- [ ] **Currency.** Pick a display currency on the Search tab. Expect prices from every source converted, and live rates loaded. (The rate service hasn't been called from the app yet.)

## Multi-source Get

- [ ] **Candidates from several sources.** With LCSC plus at least one distributor enabled, click **Get…** on a part that's on both. Expect the import dialog to list each file type with where it came from, and to fill in more as sources answer.
- [ ] **Swapping a source.** Change one file to a different source. Expect the preview to redraw with the new file.
- [ ] **Mismatch refusal.** Pick a symbol from one source and a footprint from another whose pin numbers differ. Expect Import to be blocked with a message listing the mismatched pins.
- [ ] **Fetch from SnapMagic.** For a part with no CAD files, click **Fetch from SnapMagic…** and download the ZIP. Expect it to be added to the dialog that's already open.
- [ ] **Your edits are kept.** Edit the manufacturer or description while sources are still answering. Expect your text to stay.

## Import tab

- [ ] **Previews.** Open the import dialog. Expect symbol, footprint and 3D thumbnails, a click to enlarge one, and check warnings next to them. Note roughly how long the 3D render takes.
- [ ] **Import all.** Download two or three ZIPs, then click **Import all…**. Expect a dry-run list of what would change, and **Apply** to import them all. Afterwards the **History** tab shows *one* snapshot for the batch.

## Library tab

- [ ] **Checks column.** Expect ✔ for good parts, and a coloured count with the issue list on hover for the others.
- [ ] **Preview.** Click **Preview** on a row. Expect the same viewer as the import dialog.
- [ ] **Edit… on one part.** Change the manufacturer and tags. Expect the change in the table, and in the symbol's fields in KiCad.
- [ ] **Edit… to change category.** Move a part to another category. Expect its symbol, footprint and 3D model to move, and the part to still place correctly in KiCad.
- [ ] **Edit selected.** Tick several rows, click **Edit selected…** and change one field. Expect only that field changed on all of them.
- [ ] **Attach…** Attach a datasheet, a SPICE model and a 3D model, choosing Global, Project or both. Expect each file in that library's own folder.
- [ ] **Copy to project.** Expect the part in the project's `lib\` folder, with a `PRJ_<Category>` library in the project.
- [ ] **Download missing datasheets.** Set one part's datasheet to a web link to a PDF, then click the button. Expect a local PDF and the field pointing at it. A link to a web page rather than a PDF should be left alone.
- [ ] **Batch buttons.** Try **Copy selected to project**, **Attach to selected…** and **Relink all**. Expect a dry-run list before anything changes.

## History tab

- [ ] **Snapshots appear.** After an import, expect a new snapshot showing its time, action and size.
- [ ] **Rollback.** Roll back the import. Expect the part gone and the library exactly as before, including KiCad's library list. Expect a prompt to close KiCad if the list has to change.
- [ ] **Rolling back an older snapshot.** Expect the newer changes after it to be undone too.

## BOM tab

- [ ] **Load a real project.** Open the BOM tab on one of your projects. Expect every line from the schematic, matched to library parts by MPN or supplier number, with "missing" shown for parts without CAD files.
- [ ] **Live prices.** With DigiKey and Mouser keys set, expect stock, price at your quantity and lead time per line, and totals (unit, extended, coverage, longest lead time). Spot-check two lines against the distributor's website, including a quantity that crosses a price break.
- [ ] **Shipping region.** Set a Settings default, then a different project override. Expect DigiKey prices in that region's currency; Mouser and LCSC are labelled as ignoring it.
- [ ] **Offline.** Disconnect the network and reload. Expect cached prices marked stale and one error in the log, not one per line.
- [ ] **CSV export.** Export and open in Excel. Expect the same rows and totals, with no cell turned into a formula.

## Import library (other tools)

- [ ] **Your old `KiCAD_Library`.** Click **Import library…** on the Import tab and pick it. Expect a dry-run list with sensible categories, then after Apply the parts in the Library tab with ✔ and one snapshot in History.
- [ ] **Altium.** Try a real `.SchLib`/`.PcbLib` or `.IntLib`. Expect either imported parts or a clear "kicad-cli can't convert this" message.
- [ ] **EasyEDA.** Same with an `.elib`/`.epro` library export.
- [ ] **Eagle / Fusion.** Same with an `.lbr`.

## Settings

- [ ] **kicad-cli path.** Use **Browse…** to pick `kicad-cli.exe`, click **Test**, then **Use default**. Expect the version to show each time.
- [ ] **Downloads folder.** Change the watched folder. Expect ZIPs downloaded there to be picked up.

## Moving the global library

Do this one last, and on a copy if you're nervous. History can roll it back.

- [ ] **Same drive.** **Change location…** to a new folder. Expect every part to still show ✔, and KiCad to still find them after a restart.
- [ ] **Another drive** (e.g. a USB stick). Expect a copy, a check, then the original removed. If you unplug the drive part-way, the original should be untouched.

## Command line (`target\release\klm-cli.exe`)

- [ ] **`klm-cli check --deep`** lists every part. The exit code is 1 when something is incomplete.
- [ ] **`klm-cli import a.zip b.zip --dry-run`** prints the plan and changes nothing.
- [ ] **`klm-cli history`**, then **`klm-cli rollback <id>`**, restores that snapshot.
- [ ] **`klm-cli health --project <dir>`** on a real project. Expect a readable report, exit code 1 only when there are errors, and the same findings with `--json` and `--csv`.
- [ ] **`klm-cli check-project <dir>`** on a real project, once with the profile's fab and once with `--all-fabs`. Expect KiCad's DRC/ERC findings with references, fab-limit violations naming a `klm_` rule, and the project folder unchanged (no `.kicad_dru` written).
- [ ] **`klm-cli export out.zip <part>...`**, then unzip it and add the `sym-lib-table`/`fp-lib-table` entries to a fresh KiCad project. Expect the symbols to place with their footprints and 3D models showing.
- [ ] **Export into Altium or Fusion.** Import the same ZIP with their KiCad importer. Expect symbols and footprints to come through.

---

## Wave 4.5

- [ ] **Plugin errors.** Rename `klm.exe` in the installed plugin folder and click the button. Expect a message box, and the reason in `%TEMP%\klm-plugin.log`.
- [ ] **Schematic button.** Open a schematic and click **Library Manager**. Expect KLM to open on that project.
- [ ] **Pending libraries.** With KiCad open, import into a new category, then close KiCad. Expect the library to still be in *Manage Symbol Libraries*.
- [ ] **Watch folders.** Add two watch folders, drop a vendor ZIP into each, and expect both to be picked up. A config with the old `downloads_dir` should show that folder in the list.
- [ ] **Key fields.** Type a key, toggle 👁, then save. Expect the saved key not to be shown.
- [ ] **Search.**
  - Unsorted headers show ⇅.
  - Hovering a price lists the price breaks.
  - A DigiKey search with a non-US region shows that currency code.
  - Right-clicking the MPN toggles a favourite.
- [ ] **Vendor ZIPs.** Import Ultra Librarian KiCad, Altium, Fusion and combined ZIPs. Expect each one to convert, with an attached `.stl` showing the STL warning.
- [ ] **Import library from files.** Pick a `.SchLib` and a `.PcbLib` directly. Picking a `.LibPkg` should explain that it's a project.
- [ ] **Library tab.** Sort by each column, combine the filters, and use the detail panel and the right-click menu.
- [ ] **Find missing datasheets.** Delete a part's PDF and run it. Expect the PDF to come back and `Datasheet` to be updated, with a results dialog.
- [ ] **History.**
  - Only the last N minors are kept.
  - A marked version survives more than N changes.
  - The size cap prunes minors.
  - A fresh library shows "No version saved yet".
  - "Make minor" and "Delete" work on a version.
- [ ] **Responsiveness.** Alternatives on a large library, and Standards Dry run and Apply, keep the window responsive.
- [ ] **Log.** **Clear** empties the log, and **Copy** pastes every line.

## Wave 5

- [ ] **Browse by category** (needs DigiKey/Nexar keys). Pick a category from each source and expect filtered results. Thumbnails, including JPEGs, should render. Hover the CAD icons on an LCSC part.
- [ ] **Restyle symbol.**
  - Restyle a resistor, LED, NPN and IC box. Place each one in KiCad 10 and check the pin numbers match the footprint pads, including K/A and B/C/E.
  - The Reference and Value text on IC boxes shouldn't overlap.
  - History undoes the restyle.
  - A MOSFET is refused with a reason.
- [ ] **Restyle on import.** Turn on the house-symbols setting and import a resistor. Expect it to arrive restyled. A refused part keeps its vendor symbol and a note.
- [ ] **Alternatives online.**
  - In Library → Alternatives…, find equivalents for a 10k 0603 and add one. Expect `MPN_Alt1` and the supplier fields in the symbol.
  - Try "Add and import CAD…" and the hand-entered form, and check the panel's layout.
- [ ] **Clean up.**
  - With KiCad closed, run `klm-cli cleanup` against your real config and read the findings for false "broken" ones.
  - Do a `--remove` or `--fix`, first without `--apply`, then with it. Check the table and its `.klm-cleanup-*.bak` backup.
  - On the Clean up tab, check the scan, queue, preview, Apply and the "KiCad is open" warning.
  - Run "Move into KLM" on a symbol library and expect its footprints to come too.
- [ ] **KiCad open.** With KiCad open, run Relink, Change location… and a rollback. Close KiCad and check the tables are right after the next re-check.
- [ ] **Attach ZIPs.** Attach a footprint-only ZIP and then a model-only ZIP to a part from the Library tab.
- [ ] **History kinds.** Run `klm-cli history` and `klm-cli history --json`. Expect each snapshot shown as minor, named or interval.
- [ ] **MOQ.** In LCSC and Nexar search results, check the MOQ and pack size columns are filled.

## Running against a sandbox

Never test against your real library. `klm.exe --config <dir>` / `klm-cli --config <dir>` (or `KLM_CONFIG_DIR=<dir>`) keep settings, the part database, caches, the pending queue, KiCad's global library tables (`<dir>/kicad/<version>/`, made empty if missing) and API keys (`<dir>/credentials.json`, plain text) in `<dir>`. Put a `settings.json` with `global_root` in it first. Set `KLM_USE_REAL_KEYS=1` to read keys missing from the sandbox from the real Credential Manager. Details and proof: `docs/verification/W6.0-sandbox.md`.

## Already verified

- [x] The TE switch (2-1977223-4) datasheet attach: `klm-cli check` shows its datasheet present.
- [x] Your real library passes `klm-cli check --deep`: 3 parts, 0 problems.
- [x] A dry-run import of your SamacSys resistor ZIP changes no files.
- [x] An exported bundle loads with `kicad-cli sym upgrade` and `fp upgrade` (KiCad installed on this machine, run by the W4.4 test after merge).
