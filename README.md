# KiCad Library Manager (KLM)

KLM is a Windows app for the parts in your KiCad libraries. It finds parts at LCSC, DigiKey, Mouser and Nexar, imports vendor downloads (SnapMagic, Ultra Librarian, SamacSys, Altium, Eagle), and files each part the same way every time: symbol, footprint, 3D model, datasheet and simulation model. It registers the libraries in KiCad for you, checks parts against your own house standard and your PCB maker's limits, prices a project's BOM, and keeps snapshots so any change can be rolled back. Parts go to a **global** library (every project), a **project** library (`<project>/lib`), or both. There is a command-line tool, `klm-cli`, for scripts and CI.

<!-- screenshots: docs/img/library.png, docs/img/search.png, docs/img/project-check.png -->

## Requirements

- Windows 10 or 11 (64-bit). Other systems are not supported yet; see [ROADMAP.md](ROADMAP.md).
- KiCad 9 or 10. KLM runs KiCad's `kicad-cli` to convert vendor files and to render previews, and writes files in the newest installed KiCad's format, so open them with that version.
- Internet access for searching and downloading parts. Importing from files on disk works offline.

## Install

Pick one.

**1. From KiCad's Plugin and Content Manager (updates appear in KiCad)**

1. In KiCad, open *Plugin and Content Manager*, click **Manage…** (repositories) and add this URL:
   `https://raw.githubusercontent.com/Jconnor20/KiCAD-Library-Manager/master/pcm/repository.json`
2. Select the new repository, find **KiCad Library Manager**, click **Install**, then **Apply Pending Changes**.
3. Turn on KiCad's API: *Preferences → Plugins → Enable KiCad API*. Restart KiCad.

**2. From a downloaded PCM package**

Download `klm-pcm-<version>.zip` from the [Releases](https://github.com/Jconnor20/KiCAD-Library-Manager/releases) page. In KiCad's *Plugin and Content Manager*, click **Install from File…** and choose the zip. Then turn on the KiCad API as above.

**3. Standalone**

Download `klm-<version>-windows-x64.zip` from Releases, unzip it anywhere, and run `klm.exe`. `klm-cli.exe` is in the same zip.

The programs are not code-signed, so Windows SmartScreen may warn on first run. Click **More info**, then **Run anyway**.

### The KiCad toolbar button

The plugin adds a **Library Manager** button to the PCB editor and the schematic editor. It asks KiCad for the open project's folder and starts `klm.exe --project <folder>`. KiCad installs the plugin's Python helper ([`kicad-python`](https://gitlab.com/kicad/code/kicad-python)) from PyPI the first time, which needs internet, and may take a minute before the button appears. If it never appears, check *Preferences → Plugins* in KiCad for the error. Plugin errors are also written to `%TEMP%\klm-plugin.log`. The plugin finds `klm.exe` next to `launch.py`, else through the `KLM_EXE` environment variable, else on `PATH`.

## First run

On a fresh install KLM opens a set-up page:

1. **Library location**: an empty folder, or an existing KLM library to reuse.
2. **KiCad**: shows whether KLM found your KiCad configuration. KLM adds its libraries to KiCad's global library tables.
3. **Where to find parts**: which sources are on, and which watch folders to scan for vendor ZIPs.

Nothing is written until you click **Create global library**. You can also skip this and set up later in Settings.

After that, a **Set up** checklist at the top of each page shows what is still open: global library, KiCad found, a source enabled, a watch folder. Each open item has a button that goes straight to the right Settings page. **Hide** closes the checklist.

## Pages

The rail on the left holds the pages. The top bar shows the global library and the open project (both open menus), a search box for parts, pages and actions (**Ctrl+K**), and whether KiCad is open. The bar at the bottom shows the watch folder, background work, errors and warnings (click to open the log) and, when a newer release exists, an **Update** badge.

- **Search**: one box over every enabled source (LCSC, DigiKey, Mouser, Nexar) in one table. Sort and filter, star favourites, save searches, **Browse by category** (DigiKey and Nexar), and see which CAD files exist before you import. A badge marks parts already in your library. Send a hit to Import, or open it in Library.
- **Import**
  - **Parts**: the queue of parts waiting to import. It fills from the watch folders, from **Import ZIP…**, **Import files…**, **Import folder…**, drag and drop, **Fetch from SnapMagic…** (opens SnapMagic so you can download the ZIP) or from Search. It understands SnapMagic, Ultra Librarian, SamacSys and Octopart ZIPs, KiCad libraries, Altium `.SchLib`/`.PcbLib`/`.IntLib` and Eagle/Fusion `.lbr`. Each part gets a preview, checks, and a choice of category and of Global and/or Project. **Import all…** shows a dry run of a batch first.
  - **From a BOM**: open a BOM (CSV or spreadsheet), check the detected sheet, header row and columns, look up every line (the standard library first, then your enabled sources), and generate a library from the lines it found.
- **Library**: every part in the global or project library, with a status cell for symbol, footprint, 3D model, datasheet, simulation model and KiCad entry, plus a Standard badge (does the part follow your house standard). Filter and sort, then act on the ticked parts: **Edit…**, **Copy to project**, **Attach…** (datasheet `.pdf`, SPICE `.lib .cir .sub .sp .spi .ibs`, 3D `.step .stp .wrl`), **Export…** (portable ZIP), **Delete…** (previews first). A part's own menu has **Preview**, **Reimport**, **Edit symbol…** (pins, body and text in the app), **Open in KiCad** (symbol or footprint editor; saving there re-checks the part) and **Alternatives**. **Standardise…** shows a dry run, then applies. **More** has **Find missing datasheets**, **Find / Generate SPICE models**, **Relink libraries** and **Repair 3D links…**. Every change takes a snapshot first.
- **BOM**: reads the open project's BOM with `kicad-cli`, matches each line to your libraries, and prices it from the enabled distributors for your board count and shipping region. It shows totals, shortfalls, lead times and, per line, alternatives and JLCPCB assembly status. **Assembler** picks the target: JLCPCB, or DigiKey/Mouser. **Use best alternates** swaps in the alternate that suits that assembler. **Export CSV…** writes the result.
- **Standards**: the house standard and the checks against it.
  - **Classes**: for each kind of part (resistor, capacitor, inductor, diode, LED, transistor, IC, generic), how many parts follow the standard, a graphical editor for its symbol template, and what happens to new parts on import. Profiles can be exported and imported to share. Field lists and description templates are under *Settings → Standards → Part information*.
  - **Create**: an IPC-7351B footprint generator (chip, SOD, SOT, SOIC, QFN, QFP, BGA) from datasheet dimensions. The symbol half is a preview only for now.
  - **Library health**: parts that are incomplete, off-standard, broken or share an MPN, grouped by problem.
  - **Project check**: runs KiCad's DRC and ERC on the open project with the active profile's PCB maker minimums added as rules. Tick **All fabs** to see which makers could build the board. Your project files are not changed: the check runs on a temporary copy.
  - **KiCad tables**: scans KiCad's own library tables for duplicate, broken and unused libraries. Queue remove, fix path or move into KLM, review the preview, then apply. Tables are backed up first, and library files are never deleted.
- **History**: snapshots of the global or project library, newest first. Roll back to before any entry, mark a snapshot as a named version, or delete old ones. Automatic snapshots and how many to keep are set under *Settings → Workflow → History*.
- **Settings** (bottom of the rail): *General* (libraries, appearance, update check, watch folders, `kicad-cli` path), *Sources* (one page per source), *Workflow* (Search, Import, BOM, History) and *Standards* (Symbols, Footprints with the PCB maker choice, Part information).

To move the global library, use **Change location…** (top bar, Library menu). KLM moves it, or points at the new folder if you already moved it, and rewrites every path in the library and in KiCad's library list. Boards placed before the move keep the old 3D paths until you run *Tools → Update Footprints from Library* in KiCad.

## Sources and API keys

| Source | What it gives | Key needed |
|---|---|---|
| LCSC | Search, stock, price, and the CAD files (EasyEDA) that KLM converts to KiCad | No |
| DigiKey | Search, stock, price, lead time, categories, datasheets | Yes: client ID and secret |
| Mouser | Search, stock, price | Yes: API key |
| Nexar (Octopart) | Search, stock, price, categories | Yes: client ID and secret |
| SnapMagic | Link to the part's page, where you download the CAD ZIP yourself | A key can be stored, but nothing uses it yet |

Add keys in *Settings → Sources → \<source\>* and press **Test**. Keys are stored in Windows Credential Manager, never in `settings.json`. KLM works with no keys at all: LCSC search and every import from files need none.

## Command line

`klm-cli` uses the same settings and libraries as the app. Run `klm-cli` with no arguments to print every command with its flags. Commands that add libraries or restyle refuse to run while KiCad is open.

| Command | What it does |
|---|---|
| `check` | Per-part completeness of the library; `--deep` adds part checks; exits 1 if anything is incomplete |
| `relink` | Fix old `${KLM_LIB}` paths and re-add libraries KiCad dropped |
| `repair-3d` | Preview or repair KLM 3D model paths and missing model assignments on a board's footprints |
| `history` | List snapshots |
| `rollback <id>` | Restore a snapshot |
| `standardise` | Apply the active profile's rules to the library (`--dry-run` to preview) |
| `restyle <category> <part>` | Redraw one symbol in the house style |
| `health` | Project health report as text, JSON or CSV; exits 1 on errors |
| `check-project <dir>` | KiCad DRC and ERC with a PCB maker's minimums, for one fab or `--all-fabs` |
| `footprint-gen <family>` | Write an IPC-7351B footprint from datasheet dimensions |
| `cleanup` | Scan KiCad's library tables; preview unless `--apply` |
| `import <zip>...` | Import vendor ZIPs into the global or project library |
| `remove <part>` | Delete a part and the files nobody else uses; preview unless `--yes` |
| `export <out.zip> [<part>...]` | Write parts, footprints and 3D models to one portable ZIP |
| `sim [<part>]` | Find close SPICE models for a part, or generate one with `--generate` |
| `lookup <mpn>...` | Show what every enabled source knows about each MPN |
| `stdlib verify` / `stdlib build` | Check the standard-library manifests against live sources, or build the `KLM_STD_<Category>` libraries |
| `bom-import <bom.csv\|bom.xlsx>` | Build a library from a BOM file |

`--config <dir>` (or the `KLM_CONFIG_DIR` environment variable) keeps all of KLM's files, KiCad's library tables and API keys in one folder, which is how to try things without touching your real library.

## Export

**Export…** in the Library page, or `klm-cli export`, writes one ZIP that any tool that reads KiCad libraries (including Altium and Fusion) can open: `symbols/klm_export.kicad_sym`, `footprints/klm_export.pretty/`, `3dmodels/`, `datasheets/`, `simulation/`, and `sym-lib-table` and `fp-lib-table` to paste into KiCad. Unzip it as a KiCad project folder: model paths use `${KIPRJMOD}/3dmodels/...`.

## Library layout

```
<root>/symbols/<Category>.kicad_sym           nickname KLM_<Category> (global) / PRJ_<Category> (project)
<root>/footprints/<Category>.pretty/
<root>/3dmodels/<Category>.3dshapes/
<root>/datasheets/<Manufacturer>_<MPN>.pdf
<root>/simulation/<MPN>.lib
```

Paths inside KiCad files use the global library's real folder, or `${KIPRJMOD}/lib` for a project, so a project folder can move. Every symbol carries `MPN`, `Manufacturer`, `Description`, `Datasheet` and supplier part numbers.

KiCad keeps its library list in memory and saves over outside changes. KLM therefore adds new libraries while KiCad is open but queues the table edit and re-checks it when KiCad closes. On start-up, and from **Relink libraries**, it re-adds any KLM library KiCad dropped. KiCad's tables are backed up to `.bak` before each change.

## Data and privacy

- **Settings and data** live in `%APPDATA%\KiCAD-Library-Manager\`: `settings.json`, `parts.db` (a SQLite database for alternatives, tags and cached searches; it can be rebuilt from the libraries, and deleting it never affects KiCad), `standards\` (your profiles), `thumbs\`, caches and window position. Set `KLM_CONFIG_DIR` to use another folder.
- **API keys** are in Windows Credential Manager, not in these files.
- **Network calls**, and nothing else:
  - the distributors you have enabled (LCSC/EasyEDA and JLCPCB's part list, DigiKey, Mouser, Nexar), for searches and prices;
  - downloads of datasheets, images and 3D models from links those sources give. Only `https` links are fetched, with a size limit, and only `http(s)` links and local PDFs are ever opened;
  - currency rates from `api.frankfurter.dev`, to show prices in one currency in Search and the BOM;
  - an update check: one anonymous request to GitHub's latest-release API at start-up, which sends only the app's name and version. It only shows a badge that links to the release page; KLM never updates itself. Turn it off in *Settings → General → Updates*.
- KLM has no telemetry and no account.

## Build from source

You need Rust (stable) and, to run the full tests, KiCad 9 or 10.

```
cargo build --release          # target\release\klm.exe (GUI) and klm-cli.exe
cargo run --release -p klm-gui -- --project path\to\board.kicad_pro
scripts\package.ps1            # dist\klm-pcm-<version>.zip, the KiCad PCM package
```

The code is three crates: `klm-core` (all logic, no UI), `klm-cli` and `klm-gui` (an egui front end that only draws and calls `klm-core`). See [CONTRIBUTING.md](CONTRIBUTING.md).

## More

- [LIMITS.md](LIMITS.md): what KLM does not do well yet, and what has not been checked by hand
- [ROADMAP.md](ROADMAP.md): what is planned
- [CHANGELOG.md](CHANGELOG.md): what changed in each release
- [CONTRIBUTING.md](CONTRIBUTING.md): building, testing and releasing
- [SECURITY.md](SECURITY.md): reporting a vulnerability

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
