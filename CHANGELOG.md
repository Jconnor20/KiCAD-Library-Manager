# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [1.0.0] - 2026-11-25

The first public release. It covers the whole product as it stands.

### Added

**Finding parts**
- Search over LCSC, DigiKey, Mouser and Nexar in one table: sort, filter, favourites, saved and recent searches, a cache, prices converted to one currency, MOQ and pack size, and "already in your library" badges.
- Browse by category (DigiKey, Nexar) and thumbnails; CAD availability shown before import for LCSC/EasyEDA parts.
- Get a part's files from several sources, with each file's origin shown and a pin/pad check that blocks a symbol and footprint that do not match.
- Keys for each source are stored in Windows Credential Manager.

**Importing**
- Import vendor ZIPs (SnapMagic, Ultra Librarian, SamacSys, Octopart), KiCad libraries, Altium `.SchLib`/`.PcbLib`/`.IntLib` and Eagle `.lbr`, through `kicad-cli`. STL models are attached with a warning.
- Watch folders for downloaded ZIPs, drag and drop, and a queue with a symbol, footprint and 3D preview and part checks for each part.
- Import all: a batch with a dry run and one snapshot. Category rules and part-name templates apply on import.
- Import from a BOM (CSV or spreadsheet): detect sheet, header and columns, look up each line in the standard library and the enabled sources, and generate a library.
- Attach a datasheet, SPICE model, 3D model, or a footprint-only or model-only ZIP to an existing part.
- A starter standard library of verified manifests (`klm-cli stdlib`), currently resistors, that references KiCad's stock symbols and footprints.

**Library**
- Global and project libraries, filed the same way every time and registered in KiCad's library tables, including while KiCad is open (queued and re-checked when it closes).
- Library page with status for symbol, footprint, 3D model, datasheet, simulation model and KiCad entry; sort, filter, multi-select, edit, copy to project, reimport, delete (with a preview), and a Standard badge.
- Edit in KiCad's symbol or footprint editor with a re-check on save, and an in-app symbol editor for pins, body and text.
- Alternatives and duplicates: unit-aware matching, package aliases, find equivalents online or add them by hand, and `MPN_AltN` fields written into the symbol.
- Find missing datasheets by MPN, find or generate SPICE models, repair 3D links, and relink libraries KiCad dropped.
- Move the global library and have every path rewritten.
- History: automatic and named snapshots with rollback, retention limits and a list of changed files.
- Export selected parts as a portable ZIP that Altium, Fusion and KiCad can read.
- Clean up KiCad's own library tables: find duplicate, broken and unused entries, then remove, fix or move them into KLM.

**Standards**
- Standard profiles: KLC rules, per-class symbol and footprint templates, field schemas, and Description, Value and Keywords templates. Import and export profiles to share them.
- Standardise with a dry run: symbol text, pin length and line widths, footprint line widths, text placement, origin and pin-1 marker, hidden fields.
- House-style symbols for resistors, capacitors, inductors, diodes, LEDs, transistors and IC boxes, from built-in templates or a symbol you drew, with a graphical template editor.
- PCB maker minimums (JLCPCB, PCBWay, OSH Park, Eurocircuits, Aisler) plus IPC-6012 and IPC-7351 choices; a footprint fab check with safe fixes.
- A 3D fit check and fix that compares a model's size and position with the footprint.
- IPC-7351B footprint generator for chip, SOD, SOT, SOIC, QFN, QFP and BGA.
- Library health report, as a page, as `klm-cli health` and as JSON or CSV.
- **Project check:** KiCad DRC and ERC on the open project with the active profile's PCB maker limits added as rules, or "All fabs" to see which makers could build the board (page and `klm-cli check-project`).

**BOM**
- Read a project's BOM, match it to your libraries, and price it from the distributors for your board count and shipping region, with totals, shortfalls, lead times, per-line alternates and JLCPCB assembly status (basic, preferred, extended, stock, lifecycle).
- **Assembler** choice (JLCPCB or DigiKey/Mouser) and **Use best alternates**.
- Configurable BOM field names per project, request rate limiting, and CSV export.

**App and integration**
- Windows GUI with a Dark (Bench) and Light (Datasheet) theme, a Ctrl+K box for parts, pages and actions, a log drawer, and a first-run page with a set-up checklist.
- KiCad PCM package: a toolbar button in the PCB and schematic editors that opens KLM on the open project (KiCad 9 and 10).
- `klm-cli` for scripts and CI: `check`, `relink`, `repair-3d`, `history`, `rollback`, `standardise`, `restyle`, `health`, `check-project`, `footprint-gen`, `cleanup`, `import`, `remove`, `export`, `sim`, `lookup`, `stdlib` and `bom-import`.
- `KLM_CONFIG_DIR` / `--config` to keep all of KLM's files in one folder.
- An update check at start-up that shows an **Update** badge linking to the release page. It asks GitHub's latest-release API and sends nothing else; turn it off in *Settings → General → Updates*. KLM does not update itself.
- CI (format, clippy, tests, `cargo deny`), tag-driven releases that attach `klm-pcm-<version>.zip` and `klm-<version>-windows-x64.zip` to a GitHub Release, and a KiCad PCM repository at `https://raw.githubusercontent.com/Jconnor20/KiCAD-Library-Manager/master/pcm/repository.json`.

### Security

- Only `http(s)` links and local, non-UNC PDFs are opened from a datasheet field. Other schemes and executable paths are refused.
- UNC paths (`\\server\share`) in library files are never opened or followed, so a library file cannot make Windows contact another server.
- ZIP import reads each entry as a stream and stops at 200 MB per entry, 1 GB in total and 10,000 entries.
- S-expression files are parsed to a nesting depth of 512 at most.
- Downloads are `https` only (localhost excepted for tests), size-capped, and a redirect may not leave `https`.
- Temporary files are created with unpredictable names instead of process-ID names. One shared HTTP client replaces ad-hoc ones.
- Part and file names are sanitised: control characters, leading or trailing dots and Windows device names (`CON`, `NUL`, ...) are removed or changed.
- Updated the `url` crate to 2.5.9 for RUSTSEC-2026-0592.

### Fixed

- History, Clean up, relink and BOM Swap now run in the background, so the window no longer freezes on a large library. The `kicad-cli` lookup is cached instead of repeated every frame.
- Malformed third-party symbol and footprint files no longer crash the app: the places that assumed a file's structure now report an error.
- The app and `klm-cli relink` now share one relink implementation, so they behave the same.
