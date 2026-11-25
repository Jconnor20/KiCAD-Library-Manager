# Known limits

What KLM does not do, or does only partly, today. Fixed items are removed from this list. For what is planned, see [ROADMAP.md](ROADMAP.md). Steps for checking a release by hand are in [TESTING.md](TESTING.md).

## Import

- **Vendor formats.** Altium (`.SchLib`, `.PcbLib`, `.IntLib`), Ultra Librarian, SamacSys, SnapMagic, legacy KiCad `.lib`, Eagle `.lbr` and LCSC/EasyEDA parts import and render on real files. EasyEDA Pro `.elibz2` does not: KiCad 9 and 10 both refuse to convert it, and KLM reports that.
- **Fusion 360.** Only Eagle `.lbr` files are read. Fusion's own library format is not.
- **A `.PcbLib` alone** converts to footprints but cannot become a part, because it has no symbol.
- **Only the first 3D model** of a footprint with several is carried over from a converted library.
- **STL models** are attached with a warning. Whether KiCad's 3D viewer shows them has not been confirmed.
- **A footprint-only or 3D-only ZIP** can be attached to an existing part. Imported on its own it fails with "part has no symbol file", unless an Import dialog for the same MPN is open.
- **Existing schematics keep the old library names** after you import a library; they do not point at the imported KLM copies.
- **The watch folders** only see ZIPs saved after KLM starts, or after the folder list changes.
- **Footprint preview** on import from Search can take several seconds, because it renders through `kicad-cli`.

## Sources and distributors

- **Mouser and Nexar replies have not been checked against live data.** DigiKey and LCSC fields are confirmed against recorded replies. For Mouser (no key was available) and Nexar (its free plan allows 10 parts and was used up), field names come from documentation and schema checks. Mouser lead time, lifecycle wording and order multiples, and Nexar stock, price, MOQ and pack size, may be wrong or empty. Re-run the live tests with your keys (see [TESTING.md](TESTING.md)).
- **No category browsing for Mouser.** Nexar category browsing is built but unverified, for the same reason.
- **CAD availability** (symbol, footprint, 3D) is known before import only for LCSC/EasyEDA parts. Nothing shows a SPICE model before import.
- **No country of origin.** None of the recorded replies carries it.
- **Lead time.** DigiKey is confirmed; LCSC has none; Nexar has it per offer but KLM does not read it.
- **DigiKey token requests** timed out at the app's 20 s limit in 3 of 5 live runs; the others took under 2 s. Retry if a search fails with a timeout.
- **Search results are cached** (a day by default, changeable in Settings → Workflow → Search), so stock and prices can be that old. Changing the stored fields of a part can make old cache entries unreadable; KLM then searches again.
- **SnapMagic has no in-app search.** KLM opens the part's page so you can download the ZIP yourself. A SnapMagic key can be stored but nothing uses it.
- **Currency.** Quotes that cannot be converted are left out of BOM totals, so coverage drops when rates fail to load.
- **Large BOMs** may hit a distributor's API quota. KLM spaces its requests but cannot know your quota.

## Library and checks

- **Orphans are matched by file name**, so two same-named files in different folders can hide each other.
- **Alternatives match package by footprint name** with an alias table. The same package under vendor-specific names that are not in the table will not group. SOIC bodies are told apart by width only for 3.9, 5.3 and 7.5 mm; other widths group only with the same name.
- **Edit in KiCad** opens the symbol library or footprint in KiCad's own editor and re-checks the part when you save. It watches the file only while KLM is running, and a failed launch is only logged. Starting `eeschema` or `pcbnew` standalone this way has not been confirmed on a real KiCad.
- **Source cross-check is not wired in.** The code that flags a field that disagrees with distributor data for the part's MPN exists but nothing calls it.
- **Compliance badge cache** is per library: any changed symbol library, footprint, model or profile recomputes all of it.
- **History.** Retention settings apply to every library. Automatic version dates use UTC, "weekly" means 7 days and "monthly" means 30. Edits to the snapshot files made outside KLM need **Refresh**.
- **Datasheet finder.** With no distributor enabled it can only retry the existing link. Search results are used only when their MPN matches the part's.

## Standardise and profiles

- **Wires can come loose.** Moving a footprint's origin, lengthening a pin or restyling a symbol moves connection points, so schematics and boards that later update from the library can lose connections. The dry run lists every such change before anything is written.
- **ICs have no house template.** They get the automatic box symbol.
- **Restyle refuses** MOSFETs, Zener/TVS/Schottky diodes, polarised capacitors, batteries, thermistors, multi-unit symbols, `extends` symbols and power symbols, and two visible pins stacked on one point.
- **Standardise skips** symbols with two pins or fewer, power symbols and hidden pins.
- **Pin-1 marker on large pads.** On pads larger than about 6 mm the added marker can land more than 2 mm from pin 1, and the check may still report it missing.
- **Value is rebuilt only for resistors, capacitors and inductors.** Other classes keep their Value.
- **Parameter autofill.** DigiKey and LCSC fill class fields; Mouser fills almost none and the Nexar mapping is unverified against a real reply.
- **Template preview** in the Part information editor uses sample values, not a part from your library.
- **Naming rules** (name template, category rules) live in Settings, not in the standard profile, so a shared profile does not carry them.
- **Create → Symbol** shows what a pasted pin table would give but cannot write a new symbol into a library yet.
- **Fab outline style** and the `${REFERENCE}` fab text are not part of the footprint style.

## Fab and project checks

- **Fab data has gaps.** Values a PCB maker does not publish are left empty: PCBWay via drill and pad, mask web and edge; OSH Park via pad and silk text height; Eurocircuits track, clearance, drill, ring and via (its table is an image); Aisler via pad. The IPC-6012 class 3 annular ring (0.05 mm) and the IPC-7351 courtyard excess come from secondary sources because the standards are paywalled. Each fab entry carries its source page and the date it was checked; the standard 2-layer process is assumed.
- **Footprint fab check approximations.** Trapezoid and custom pads are treated as their size rectangle. Inner copper layers, drill offsets and silk text stroke width are not checked. Silk circles and arcs are never flagged, and only `fp_line` silk is clipped off pads (rectangles and polygons stay flagged). Round courtyards are not grown. Fixes run only when the profile names a fab.
- **Project check.** There is no rule for the mask web (KiCad reads it only from board setup) or for courtyard excess. The silk width rule covers silk text only. Sub-sheets kept in subfolders are not copied, so ERC does not see them. KiCad 9 labels KLM's rule violations "board setup constraints". "All fabs" mode lists which fabs pass, not each fab's findings.
- **A rules file KiCad cannot parse is dropped silently** by `kicad-cli`. KLM's rule syntax is checked against KiCad 9.0 and 10.0.7 only.

## 3D fit

- **The bounding box is approximate.** It is read from point data in STEP and WRL files. Nested STEP transforms and WRL `Transform` nodes are not followed, and `.stpz` and `.wrz` files are skipped.
- **Only multiples of 90 degrees** are exact for X and Y rotation.
- **Rotation direction** follows KiCad's model matrix (angles negated). The result has been checked against KiCad's source, not yet on a real model in the 3D viewer.
- **The fix** (centre the model, pick the best quarter turn) runs only when the model's size fits the footprint, so a wrong model is warned about, never moved. There is no Settings switch for it. Only the footprint's first model is checked.

## BOM and assembly

- **JLCPCB status is partial.** "Not stocked by JLC at all" cannot be told apart from "LCSC not asked, or offline", so only "out of stock at JLC" is flagged. Lifecycle comes from DigiKey and Mouser, and Mouser's wording is matched loosely. There is no extended-part setup fee in the summary.
- **Swap needs the part in the library.**
- **Shipping region applies to DigiKey only.** Mouser and LCSC ignore it. There is no shipping cost or delivery estimate.
- **Spreadsheet BOMs** are read with values only: formulas give their cached result, and a merged cell fills only its top-left cell. **Save BOM with library parts** writes CSV and drops the rows above the header.
- **A standard-library hit is referenced**, as `KLM_STD_<Category>:<part_id>`, not copied into the library you generate. Run `klm-cli stdlib build` to create those libraries.
- **BOM import lookups.** The BOM's manufacturer narrows candidates only when it matches one exactly, so differently spelled makers can show as ambiguous. The same part is fetched once per line, not once per MPN. The generate step has no automated test.
- **The standard library is small.** `stdlib/` currently holds resistors only. Both release zips ship it next to the executable, and `--manifest <dir>` points at a different folder.

## Footprint generator

- **The IPC-7351 standard itself was not read.** Formulas follow published tables as recalled and cross-checked against `kicad-footprint-generator`; the chip, BGA pad-ratio and F/P values are the least certain. SOIC-8 gives 1.50 mm pads on 2.70 mm centres where PCB Libraries gives 1.55 and 2.70.
- **Not modelled:** chips smaller than 1608, pull-back QFN, J-lead/SOJ/PLCC, non-collapsing BGA balls. Chip is always RESC.
- **QFN exposed pad** has full paste and no exposed-pad-to-signal gap check.
- **No 3D model** is generated.

## Symbol editor

- **Single-unit symbols with a rectangle body only.** There is no drawing of lines or arcs, and the first rectangle is taken as the body. Use KiCad's editor for those.
- **The preview** is KiCad's render of the saved file, so it updates on Save, not while you drag.
- **Swapping two pin numbers** (1 and 2) is refused; rename through a spare number.

## KiCad integration

- **KiCad open.** New libraries are written while KiCad runs and re-checked when it closes. Rollback, move-global and relink apply at once and queue their table edits, and the queue is replayed when KiCad closes. An outside edit that adds an entry survives; an outside change or removal of an entry the restore also lists is overwritten. Old `${KLM_LIB}` path rewrites still need KiCad closed. Whether KiCad picks up a library table edited while it runs has not been confirmed.
- **Clean up scans only the newest KiCad version's config**; older ones are listed as not scanned. Footprint-only libraries cannot be moved into KLM. Path resolution (environment variables, text variables, relative project paths) is modelled on KiCad's behaviour, not taken from its source.
- **Stock library variables** (`${KICADn_SYMBOL_DIR}` and the footprint and 3D equivalents) resolve to the newest installed KiCad whatever `n` is. An unresolvable variable is trusted, not reported.
- **Repair 3D links** handles KLM libraries only. Stock and external footprint libraries are out of scope.
- **The toolbar plugin** needs KiCad's API enabled and, the first time, internet access for `kicad-python`. If `kicad-python` lacks a schematic call, the button opens KLM with no project.
- **Output format.** KLM writes the newest installed KiCad's format. KiCad 9 cannot open files written for a KiCad 10 install.

## Platform

- **Windows only**, 64-bit. API keys use Windows Credential Manager, and KiCad is found through its Windows install and config paths.
- **Not code-signed.** SmartScreen may warn on first run.
- **KLM does not show its version** in the app. Use the release you downloaded.
- **Update check only notifies.** There is no self-update; install the new release the way you installed this one.
- **UNC paths** in library files (`\\server\share`) are never opened or followed, on purpose.

## Not yet verified by hand

These pass automated tests but have not been tried by a person in the app, in KiCad or on a real project. They will be trimmed as [TESTING.md](TESTING.md) is worked through.

- The KiCad PCM install (from the repository URL and from file), on KiCad 9 and 10, and the toolbar button in the PCB and schematic editors.
- Working with KiCad open: queued library tables, Relink, Change location and rollback, and what KiCad does with them afterwards.
- Placing imported parts in KiCad and seeing footprint, 3D model and datasheet.
- Hidden fields: that KiCad reads the `(hide yes)` flag Standardise writes as hidden.
- Standardise on an existing library, including the fab fixes once a fab is chosen.
- The 3D fit fix in KiCad's 3D viewer.
- Footprint generator output opening in KiCad and passing DRC.
- Project check on a real project, for one fab and for All fabs.
- Edit in KiCad (standalone symbol and footprint editors, and re-check on save), the in-app symbol editor, Reimport and Delete.
- DigiKey, Mouser and Nexar searches and BOM prices with real keys; the BOM Assembler choice and Use best alternates.
- BOM import from CSV and spreadsheets, and the look-up from sources.
- Importing real Altium and Eagle libraries through the app (the files are covered by automated corpus tests).
- Moving the global library, on the same drive and to another.
- The update badge against a real newer release.
