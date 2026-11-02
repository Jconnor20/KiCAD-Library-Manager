# KiCad Library Manager (KLM)

One place to pull parts into KiCad: symbol, footprint, 3D model, datasheet and sim model, filed the same way every time and registered in KiCad automatically. Parts can go to a **global** library (every project) or a **project** library (`<project>/lib`), or both.

Vendor ZIP import (SnapMagic, Ultra Librarian, SamacSys, Octopart), the Library tab's per-part actions and the KiCad toolbar button work. Search sources (LCSC, DigiKey, Mouser, Nexar) are developed on other branches.

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

Each row in the Library tab has:
- **Attach…**: pick a datasheet (`.pdf`), SPICE model (`.lib .cir .sub .sp .spi .ibs`) or 3D model (`.step .stp .wrl`). KLM files it under the standard name and sets `Datasheet`, `Sim.Library`/`Sim.Name`/`Sim.Device`, or the footprint's 3D model path (keeping its offset and rotation). A 3D model goes on the footprint, so other parts using that footprint get it too.
- **Copy to project** (Global view, needs a project): copies the symbol, footprint, 3D model, datasheet and sim model into `<project>/lib` with `PRJ_` paths. A copy that is already there is only replaced on Shift-click. A new project category library is added to KiCad's list, so KLM asks you to close KiCad first.
- **Open datasheet**: opens the local PDF or the web page.

To move the global library, click **Change location…** and pick the new folder. KLM moves it (same drive) or, if you already moved it yourself, just points at the new folder. Either way it rewrites every path in the library and in KiCad's library list. Boards placed before the move keep the old 3D paths until *Tools → Update Footprints from Library*.

Needs KiCad 9 or 10 installed (`kicad-cli` converts vendor files to the installed KiCad's format). Files are written in the newest installed KiCad's format, so open them with that version.

## KiCad toolbar button
`kicad_plugin/` is an [IPC-API plugin](https://dev-docs.kicad.org/en/apis-and-binding/ipc-api/for-addon-developers/index.html) for KiCad 9 and 10. Its PCB editor toolbar button runs `launch.py`, which asks the open board for its project folder (via [`kicad-python`](https://gitlab.com/kicad/code/kicad-python), imported as `kipy`) and starts `klm.exe --project <folder>`.

`kicad-python` is MIT-licensed (copyright The KiCad Developers). It is not bundled; KiCad installs it from PyPI into the plugin's own virtual environment, which needs internet the first time (KiCad uses `requirements.txt`).

Install:
1. In KiCad: *Preferences → Plugins → Enable KiCad API*. Restart KiCad.
2. Build the package with `scripts\package.ps1`, then *Plugin and Content Manager → Install from File…* and choose `dist\klm-pcm-<version>.zip`. Or install by hand: copy `kicad_plugin\*` and `target
elease\klm.exe` into one folder `Documents\KiCad\<version>\plugins\klm\`.
3. Restart KiCad and open the PCB editor. KiCad sets up the plugin's environment in the background; the **Library Manager** button appears on the toolbar when that is done (if not, check *Preferences → Plugins* for the plugin and its error).

KLM is found next to `launch.py`, else via the `KLM_EXE` environment variable, else `klm` on `PATH`.

## Build and package
```
cargo build --release          # target
elease\klm.exe (no console window)
scripts\package.ps1            # builds, then writes dist\klm-pcm-<version>.zip (PCM layout, plugin + klm.exe)
```

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
