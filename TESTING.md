# Manual test checklist

These have passed automated tests but haven't been checked by hand in the app or in KiCad. Tick each one off as you go. If something fails, note what you did and what the log panel at the bottom of KLM said.

**Before you start**
- **Run the latest build:** `cargo run --release -p klm-gui`, or the `.exe` in `target\release\`.
- **Your library is safe:** KLM takes a snapshot before every change. If a test goes wrong, roll it back from the **History** tab.
- **Close KiCad** whenever KLM asks. KiCad overwrites its library list while it's open.

---

## KiCad integration

- [ ] **Install the toolbar plugin.** In KiCad: *Plugin and Content Manager → Install from File…*, then pick `dist\klm-pcm-0.1.0.zip`. Expect it to install with no "metadata" error.
- [ ] **Toolbar button.** Open a board in the PCB editor (KiCad's API must be on: *Preferences → Plugins*) and click **Library Manager**. Expect KLM to open with that board's project already chosen.
- [ ] **New category library.** Import a part into a new category while KiCad is open. Expect KLM to ask you to close KiCad first. After you reopen KiCad, the new `KLM_<Category>` library is in the symbol chooser.
- [ ] **Placed part is complete.** Place an imported part from KiCad's symbol chooser. Expect its footprint to be assigned, the 3D viewer to show its model, and *Show datasheet* to open the local PDF.
- [ ] **Re-link.** Delete a `KLM_…` entry in KiCad's *Manage Symbol Libraries*, close KiCad, then click **Re-link libraries** in KLM. Expect the entry back.

## Sources

- [ ] **LCSC, several part types.** Search and import a connector, a multi-unit op-amp (e.g. LM358 in a dual package) and a part with odd-shaped pads. Expect the preview and the KiCad footprint to match the datasheet drawing: pin positions, pad sizes, pin-1 location.
- [ ] **Ultra Librarian ZIP.** Download one part's KiCad ZIP. Expect it to appear under **Import** and import cleanly. (Only SamacSys and SnapMagic downloads have been tried so far.)
- [ ] **Legacy `.lib` symbol.** Import a ZIP whose symbol is an old-style `.lib` (older SamacSys/UL downloads). Expect the pin/pad check to run on it rather than say it can't be read. This fix is unconfirmed on a real file.
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

---

## Already verified

- [x] The TE switch (2-1977223-4) datasheet attach: `klm-cli check` shows its datasheet present.
- [x] Your real library passes `klm-cli check --deep`: 3 parts, 0 problems.
- [x] A dry-run import of your SamacSys resistor ZIP changes no files.
