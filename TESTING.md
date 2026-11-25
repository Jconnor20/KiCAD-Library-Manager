# Release checklist

A manual pass to do before each release, on top of the automated tests. Items are ordered by risk: the ones most likely to break for a new user come first. Tick each one, or note what you did, what you expected and what the log (status bar → **Show log**) said.

**Before you start**

- Use a release build: `cargo build --release`, or the zip you are about to publish.
- Test on a copy first. Run `klm.exe --config <dir>` (or set `KLM_CONFIG_DIR=<dir>`) to keep settings, `parts.db`, caches, KiCad's library tables (`<dir>\kicad\<version>\`) and API keys (`<dir>\credentials.json`, plain text) out of your real setup. Put a `settings.json` with `global_root` in the folder first if you want to skip the first-run page. Set `KLM_USE_REAL_KEYS=1` to read keys missing from the folder from the real Credential Manager.
- KLM takes a snapshot before every change. If a test goes wrong, roll it back from **History**.
- Have KiCad 10 installed, and KiCad 9 for the install items marked (9).

## Install and KiCad

- [ ] **PCM install from the repository URL.** In KiCad 10: *Plugin and Content Manager → Manage…*, add `https://raw.githubusercontent.com/Jconnor20/KiCAD-Library-Manager/master/pcm/repository.json`, select it, install **KiCad Library Manager**, **Apply Pending Changes**. Expect the install to finish with no "metadata" error and the version to match the release. (9) Repeat on KiCad 9.
- [ ] **PCM install from file.** Download `klm-pcm-<version>.zip` from the release. *Install from File…*, choose it. Expect the same result. Restart KiCad with *Preferences → Plugins → Enable KiCad API* on.
- [ ] **Toolbar button, PCB editor.** Open a board and click **Library Manager**. Expect KLM to open with that board's project chosen (top bar shows its folder name). First run may take a minute while KiCad sets up the plugin's Python environment.
- [ ] **Toolbar button, schematic editor.** Same in the schematic editor.
- [ ] **Plugin errors are visible.** Rename `klm.exe` inside the installed plugin folder and click the button. Expect a message box and the reason in `%TEMP%\klm-plugin.log`. Rename it back.
- [ ] **Standalone zip.** Unzip `klm-<version>-windows-x64.zip` into a fresh folder, run `klm.exe`, and run `klm-cli.exe` with no arguments. Expect the app to open (after SmartScreen's **More info → Run anyway**) and the CLI to print its command list.
- [ ] **PCM update path.** With the previous release installed through the repository, publish the new tag and check KiCad offers the update after refreshing the repository. (Skip for the first release.)

## First run and setup

- [ ] **First-run page.** Run with an empty `KLM_CONFIG_DIR`. Expect the "Set up your global library" page with three steps. Pick an empty folder and click **Create global library**. Expect the library folder created, `KLM_*` entries (once you import) in KiCad's tables, and the app to open on Library.
- [ ] **Set-up checklist.** Expect "Set up: ✔ Global library, ✔ KiCad found" and open items for a source and a watch folder, each with an **Open** button to the right Settings page. Enable LCSC and add a watch folder. Expect the checklist to disappear.
- [ ] **Change location.** Top bar → Library → **Change location…** to a new folder on the same drive. Expect every part still shows complete, and KiCad finds them after a restart. Then once to another drive: expect a copy, a check, then the original removed.

## Import (highest-traffic path)

- [ ] **Search and import an LCSC part end to end.** Search for `LM358` or a connector, click Import, check the preview, import to Global. Open KiCad, place it from the `KLM_<Category>` library. Expect the footprint assigned, the 3D model in KiCad's 3D viewer, and the datasheet opening from the symbol.
- [ ] **Import with KiCad open.** Import a part into a new category while KiCad is open. Expect no blocking prompt, "pending" in the log or top bar, and after you close and reopen KiCad the new `KLM_<Category>` library is in the symbol chooser.
- [ ] **Vendor ZIPs.** Import a SnapEDA/SnapMagic, an Ultra Librarian and a SamacSys ZIP: drop them in a watch folder, or use **Import ZIP…**. Expect each to appear in the queue, show a preview and import. An attached `.stl` should show a warning.
- [ ] **Altium and Eagle.** With **Import files…**, import a real `.SchLib` plus `.PcbLib`, and an Eagle `.lbr`. Expect parts to appear in the queue, or a clear message (EasyEDA Pro `.elibz2` should say KiCad cannot convert it).
- [ ] **Import all.** Queue two or three ZIPs, click **Import all…**. Expect a dry-run list, **Apply**, and one snapshot for the batch in History.

## Library

- [ ] **Edit in KiCad.** On a part, **Open in KiCad → Symbol in KiCad's editor** and **Footprint in KiCad's editor**. Expect each to open as a standalone editor, and saving a change there to re-check the part in KLM within a few seconds.
- [ ] **In-app symbol editor.** **Edit symbol…**: move a pin, change the body size, save. Expect the Library row re-checked. In History, roll the change back and expect the symbol as before.
- [ ] **Reimport and Delete.** **Reimport** a part from its source and expect your edits replaced after the confirmation. **Delete…** a part and read the preview: expect the symbol, and any footprint, model, datasheet and sim model no other part uses. After Delete, History can roll it back.
- [ ] **Standardise.** Run **Standardise…** on a copy of your library: read the dry run, apply, open the library in KiCad. Expect fields set to hidden to show as hidden (`(hide yes)` in the `.kicad_sym`), Reference and Value where the house template puts them, and no broken pin numbers. Roll it back from History.
- [ ] **3D fit fix.** Pick a part whose model is off-centre or turned. Expect a 3D fit warning, and after Standardise the model correctly placed in KiCad's 3D viewer (centred on the footprint, right way up).
- [ ] **Attach and Copy to project.** **Attach…** a datasheet, a SPICE model and a 3D model, then **Copy to project**. Expect each file in the right folder and a `PRJ_<Category>` library in the project.
- [ ] **Find missing datasheets.** Delete one part's PDF and run **More → Find missing datasheets**. Expect the PDF back and the `Datasheet` field updated.

## Standards and project checks

- [ ] **Footprint generator.** *Standards → Create*: generate an SOIC-8, a chip 0603 and a QFN. Open each in KiCad's footprint editor. Expect pads at the sizes the page shows, a courtyard and a pin-1 mark, and a clean DRC when placed on a board.
- [ ] **Project check, one fab.** Open a real project, *Standards → Project check*, run it. Expect KiCad's DRC and ERC findings with references, fab-limit violations naming a `klm_` rule, and the project folder unchanged (no `.kicad_dru` written).
- [ ] **Project check, All fabs.** Tick **All fabs** and run. Expect one pass/fail line per fab.
- [ ] **KiCad tables clean-up.** *Standards → KiCad tables*: scan, queue one remove and one fix path, read the preview, apply with KiCad closed. Expect a `.klm-cleanup-*.bak` beside the table and the library files untouched.

## BOM

- [ ] **Prices and assembler.** With DigiKey and Mouser keys set, open *BOM* on a project and click **Read BOM and prices**. Expect stock, price at your board count, lead time and totals. Spot-check two lines on the distributor's site. Switch **Assembler** between JLCPCB and DigiKey/Mouser and click **Use best alternates**. Expect the alternates to change with the assembler.
- [ ] **BOM export.** **Export CSV…** and open it in a spreadsheet. Expect the same rows and totals, and no cell turned into a formula.
- [ ] **BOM import.** *Import → From a BOM*: open a CSV, then an `.xlsx`. Expect the right sheet, header row and columns, **Look up** to give a status per line, and **Generate** to add the found parts to the library.

## Sources and keys

- [ ] **DigiKey, Mouser and Nexar.** In *Settings → Sources*, enter keys, **Save keys**, **Test**. Expect "✔ results" for each, then a real search for each in Search. Close and reopen KLM: keys show as stored, and are not in `settings.json`.

## History and relink

- [ ] **Rollback.** After an import, roll back to before it in History. Expect the part gone and the library as before, including KiCad's library list.
- [ ] **Relink with KiCad open and closed.** Delete a `KLM_…` entry in KiCad's *Manage Symbol Libraries* and save. With KiCad closed, **More → Relink libraries**: expect the entry back. Repeat with KiCad open: expect it queued, and applied after KiCad closes.

## Security and updates

- [ ] **Refuses unsafe links.** Set a symbol's `Datasheet` to `C:\Windows\System32\calc.exe` (and another to a `\\server\share\x.pdf` path) and click **Open datasheet** on each part in Library. Expect nothing to launch and a message in the log.
- [ ] **Update check.** *Settings → General → Updates*: expect "Check for updates at start-up" on by default and a note that only GitHub is asked. With it off, expect no request at start-up. To see the badge, install a build whose version is lower than the latest release; expect "Update <version>" in the status bar, opening the release page when clicked.

## Command line

- [ ] **Core commands.** In `klm-cli.exe`: `check --deep`, `history` then `rollback <id>`, `import a.zip --dry-run` (changes nothing), `health --project <dir>` and `check-project <dir>`. Expect readable output and exit code 1 only when something is wrong.
- [ ] **Export.** `klm-cli export out.zip <part>`, unzip, add the `sym-lib-table` and `fp-lib-table` entries to a fresh KiCad project. Expect the symbols to place with footprints and 3D models showing.

---

## Automated tests

```
cargo test --workspace
```

This runs the unit tests and the end-to-end tests. Tests that need `kicad-cli` skip themselves when KiCad is not installed. The GUI tests render offscreen and take about a minute and a half. Set `KLM_CONFIG_DIR` first to be sure nothing touches your real setup:

```
$env:KLM_CONFIG_DIR = "$env:TEMP\klm-sandbox"; cargo test --workspace
```

CI also runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo deny check`.

### Live API tests

These call the real distributors and need your keys in Windows Credential Manager (or a sandbox with `KLM_USE_REAL_KEYS=1`). They are skipped by default:

```
cargo test -p klm-core --test live_api -- --ignored live
```

The `live_record_*` tests save each raw reply, with secrets replaced, under `crates/klm-core/tests/fixtures/live/`. Run them when a distributor changes its API, and before a release if Mouser or Nexar keys are available (see [LIMITS.md](LIMITS.md)).

### Scale timings

```
cargo test --release -p klm-core --test scale_e2e -- --ignored scale --nocapture
```

Builds a 2,000-part library and prints how long listing, checking and the alternatives scan take. It does not fail on the numbers.

### Real-file corpus

`crates/klm-core/tests/corpus_e2e.rs` imports real vendor files from `testdata/` (git-ignored; see [docs/testdata-manifest.md](docs/testdata-manifest.md) for what to download) and renders them with `kicad-cli`. A case whose file is missing is skipped. Renders land in `target/corpus-renders/`.
