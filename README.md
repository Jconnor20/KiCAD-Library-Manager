# KiCad Library Manager (KLM) 0.6.0

One place to find, import, check, price and maintain the parts in your KiCad libraries: symbol, footprint, 3D model, datasheet and sim model, filed the same way every time and registered in KiCad automatically. Parts go to a **global** library (every project) or a **project** library (`<project>/lib`), or both.

Windows is the supported platform. Needs KiCad 9 or 10 installed (`kicad-cli` converts vendor files to the installed KiCad's format). Files are written in the newest installed KiCad's format, so open them with that version.

## Install and run
```
cargo build --release          # target\release\klm.exe (GUI, no console window) and klm-cli.exe
cargo run --release -p klm-gui                                         # standalone
cargo run --release -p klm-gui -- --project path/to/board.kicad_pro   # with a project
```
First run: choose where the global library lives and click **Create global library** (Import tab).

### KiCad toolbar button (plugin / PCM)
`kicad_plugin/` is an [IPC-API plugin](https://dev-docs.kicad.org/en/apis-and-binding/ipc-api/for-addon-developers/index.html) for KiCad 9 and 10. Its button (PCB editor and schematic editor) runs `launch.py`, which asks the open board for its project folder (via [`kicad-python`](https://gitlab.com/kicad/code/kicad-python), MIT, not bundled; KiCad installs it from PyPI into the plugin's own environment, which needs internet the first time) and starts `klm.exe --project <folder>`.

1. In KiCad: *Preferences → Plugins → Enable KiCad API*. Restart KiCad.
2. Build the package with `scripts\package.ps1`; it writes `dist\klm-pcm-<version>.zip` (PCM layout: plugin plus `klm.exe`). Then *Plugin and Content Manager → Install from File…* and choose that zip. Or install by hand: copy `kicad_plugin\*` and `target\release\klm.exe` into `Documents\KiCad\<version>\plugins\klm\`.
3. Restart KiCad. KiCad sets up the plugin's environment in the background; the **Library Manager** button appears when that is done (if not, check *Preferences → Plugins* for the plugin and its error).

KLM is found next to `launch.py`, else via the `KLM_EXE` environment variable, else `klm` on `PATH`. Plugin errors go to `%TEMP%\klm-plugin.log`.

## The tabs
- **Search**: one box over every enabled source (LCSC, DigiKey, Mouser, Nexar, SnapMagic) in one table. Filter to in-stock, star favourites, save searches, open a hit in Library, or send it to Import.
- **Import**: the queue of parts waiting to import (from a watched Downloads folder, **Import ZIP…**, **Import files…**, **Import folder…**, **Fetch from SnapMagic…** or Search). Supports SnapMagic, Ultra Librarian, SamacSys and Octopart ZIPs. **Import…** lets you check MPN and category and tick Global and/or Project.
- **Library**: every part in the global or project library with ✔ / missing / broken for footprint, 3D model, datasheet, sim model and KiCad entry. Actions: **Add**, **Edit selected…**, **Attach to selected…** (datasheet `.pdf`, SPICE `.lib .cir .sub .sp .spi .ibs`, 3D `.step .stp .wrl`; a 3D model goes on the footprint, so other parts sharing it get it too), **Alternatives…** (equivalent-part groups: find online or add by hand), **Copy selected to project**, **Export selected…** (portable ZIP, see below), **Copy as CSV**, **Standardise…**, **Health report…**, **Relink libraries**, **Find missing datasheets**. **Delete** removes the selected parts after a preview, with a snapshot first; footprints, 3D models, datasheets and sim models go too unless another part uses them.
- **BOM**: reads the project's BOM, matches it to your libraries, prices it from the distributors and shows shortfalls. **Export CSV…** writes the result.
- **History**: snapshots of the global or project library, newest first. Roll back to before any entry, mark a snapshot as a version, or delete old ones.
- **Clean up**: finds duplicate, broken and unused entries in KiCad's library tables. Queue remove / fix path / move into KLM, review the preview, then apply. Tables are backed up first; library files are never deleted.
- **Settings** (popup): *General* (global library location, watch folders, `kicad-cli` path), *Sources* (enable each source, API keys, Test), *Search* (cache time, currency, part-name template), *Import* (category rules), *Standards* (symbol and footprint rules), *BOM* (shipping region), *History* (automatic snapshots and versions).

To move the global library, use **Change location…**. KLM moves it (same drive) or, if you already moved it, points at the new folder, and rewrites every path in the library and in KiCad's library list. Boards placed before the move keep the old 3D paths until *Tools → Update Footprints from Library*.

## Export
**Export selected…** (or `klm-cli export`) writes one ZIP any KiCad-reading tool (including Altium and Fusion) can open: `symbols/klm_export.kicad_sym`, `footprints/klm_export.pretty/`, `3dmodels/`, `datasheets/`, `simulation/`, and `sym-lib-table` / `fp-lib-table` to paste into KiCad. Unzip it as a KiCad project folder: model paths use `${KIPRJMOD}/3dmodels/...`. Footprints with the same name but different content are kept apart (`NAME`, `NAME_2`), and the symbols point at the right one; identical footprints are written once.

## Command line
`klm-cli` uses the same settings and libraries as the app, for scripts and CI. `--config <dir>` (or `KLM_CONFIG_DIR`) keeps all files in one folder. Commands that add libraries or restyle refuse to run while KiCad is open.
```
klm-cli check [--json] [--deep] [--project <dir>]    # per-part completeness; --deep adds part-check issues; exit 1 if incomplete
klm-cli relink [--project <dir>]                      # fix old ${KLM_LIB} paths, re-add libraries KiCad dropped
klm-cli history [--json] [--project <dir>]            # list snapshots
klm-cli rollback <id> [--project <dir>]               # restore a snapshot (id from history)
klm-cli standardise [--project <dir>] [--dry-run]     # apply the Settings standards rules
klm-cli restyle <category> <part> [--project <dir>] [--dry-run]   # redraw one symbol in house style
klm-cli health [--json | --csv] [--project <dir>]     # project health report; exit 1 on errors
klm-cli check-project <dir> [--fab <id>]... [--all-fabs] [--format text|json|csv]   # KiCad DRC + ERC with the fab's minimums; exit 1 on errors
klm-cli cleanup [--project <dir>]... [--remove sym:<lib>|fp:<lib>]... [--fix sym:<lib>=<path>]... [--move sym:<lib>]... [--apply]
                                                      # scan KiCad's library tables; preview unless --apply
klm-cli import <zip>... [--category <c>] [--to global|project|both] [--project <dir>] [--overwrite] [--dry-run]
klm-cli export <out.zip> [<part>...] [--category <c>] [--project <dir>]
klm-cli remove <part> [--category <c>] [--project <dir>] [--yes]   # delete a part; preview unless --yes
```
`import` guesses the category when `--category` is not given; `--to` defaults to global. KLM also keeps a small SQLite database (`parts.db` next to `settings.json`) for alternatives, tags and cached searches. It can be rebuilt from the libraries, and deleting it never affects KiCad.

## Layout
```
<root>/symbols/<Category>.kicad_sym           nickname KLM_<Category> (global) / PRJ_<Category> (project)
<root>/footprints/<Category>.pretty/
<root>/3dmodels/<Category>.3dshapes/
<root>/datasheets/<Manufacturer>_<MPN>.pdf
<root>/simulation/<MPN>.lib
```
Paths inside KiCad files are the global library's real folder, or `${KIPRJMOD}/lib` for a project (so a project folder can move). Every symbol carries `MPN`, `Manufacturer`, `Description`, `Datasheet` and supplier part numbers.

KiCad keeps its library list in memory and saves over outside changes, so KLM asks you to close KiCad before it adds a new library. On start-up, and from **Relink libraries**, it re-adds any KLM library KiCad dropped. KiCad's tables are backed up to `.bak` before each change.

## Releases
Every wave bumps the minor version in the workspace `Cargo.toml` (the single source of truth; `scripts\package.ps1` stamps it into the plugin metadata).

## Code
- `crates/klm-core`: all logic, with no UI code (sources, KiCad file editing, library layout).
- `crates/klm-cli`: the `klm-cli` command line.
- `crates/klm-gui`: egui front end. It only draws and calls `klm-core`, so it can be replaced.

`cargo test` runs the unit tests plus an end-to-end import that has the installed `kicad-cli` render the result.

## Licence
GPL-3.0-or-later. See `LICENSE`.
