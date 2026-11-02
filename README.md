# KiCad Library Manager (KLM)

One place to pull parts into KiCad: symbol, footprint, 3D model, datasheet and sim model, filed the same way every time and registered in KiCad automatically. Parts can go to a **global** library (every project) or a **project** library (`<project>/lib`), or both.

Status: phase 1 of 4. Vendor ZIP import (SnapMagic, Ultra Librarian, SamacSys, Octopart) works. LCSC search, DigiKey/Mouser/Nexar search and the KiCad toolbar button come next.

## Use
```
cargo run --release -p klm-gui              # standalone
cargo run --release -p klm-gui -- --project path/to/board.kicad_pro
```
1. First run: choose where the global library lives and click **Create global library**.
2. Optionally **Choose project…** to enable project imports.
3. Download a part's **KiCad** ZIP from SnapMagic, Ultra Librarian or SamacSys. It shows up under *Parts to import* (or use **Import ZIP…**).
4. **Import…** → check MPN and category → tick Global and/or Project → **Import**.
5. The **Library** tab lists every part with ✔ / missing / broken for its footprint, 3D model, datasheet, sim model and KiCad library entry. Tick *Only incomplete* to see what needs fixing.

To move the global library, click **Change location…** and pick the new folder. KLM moves it (same drive) or, if you already moved it yourself, just points at the new folder. Either way it rewrites every path in the library and in KiCad's library list. Boards placed before the move keep the old 3D paths until *Tools → Update Footprints from Library*.

Needs KiCad 9 or 10 installed (`kicad-cli` converts vendor files to the installed KiCad's format). Files are written in the newest installed KiCad's format, so open them with that version.

## Layout
```
<root>/symbols/<Category>.kicad_sym           nickname KLM_<Category> (global) / PRJ_<Category> (project)
<root>/footprints/<Category>.pretty/
<root>/3dmodels/<Category>.3dshapes/
<root>/datasheets/<Manufacturer>_<MPN>.pdf
<root>/simulation/<MPN>.lib
```
Paths inside KiCad files are the global library's real folder, or `${KIPRJMOD}/lib` for a project (so a project folder can move). Every symbol carries `MPN`, `Manufacturer`, `Description`, `Datasheet` and supplier part numbers.

KiCad keeps its library list in memory and saves over outside changes, so KLM asks you to close KiCad before it adds a new library. On start-up, and from **Re-link libraries**, it re-adds any KLM library KiCad dropped. KiCad's tables are backed up to `.bak` before each change.

## Code
- `crates/klm-core`: all logic, with no UI code (sources, KiCad file editing, library layout).
- `crates/klm-gui`: egui front end. It only draws and calls `klm-core`, so it can be replaced.

`cargo test` runs the unit tests plus an end-to-end import that has the installed `kicad-cli` render the result.
