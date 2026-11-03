# Test-data manifest

Vendor downloads can't be redistributed, so download them by hand into `testdata/` (git-ignored).
`crates/klm-core/tests/corpus_e2e.rs` (not written yet) is meant to run over them and skip when the folder is missing.

| # | Part (suggested) | Site | Formats to download | Expected import result |
|---|------------------|------|---------------------|------------------------|
| 1 | Any LDO, e.g. AP2112K-3.3 | Ultra Librarian | KiCad ZIP | symbol + footprint + STEP, KiCad folder preferred |
| 2 | Same part | Ultra Librarian | Altium ZIP (.LibPkg/.SchLib/.PcbLib) | Needs foreign converter; .LibPkg gives "pick the .SchLib/.PcbLib" hint |
| 3 | Same part | Ultra Librarian | Eagle ZIP (.lbr) | Converted by kicad-cli, or clear error with its output |
| 4 | STM32 or similar MCU | SnapEDA | KiCad ZIP | symbol + footprint + 3D model |
| 5 | Connector with .stl model | SnapEDA | ZIP with .stl | model imported with warning if KiCad can't view STL |
| 6 | Op-amp, e.g. LM358 | SamacSys | KiCad ZIP | symbol + footprint + STEP |
| 7 | Footprint-only ZIP | any | ZIP with no symbol | attaches to an existing part by MPN |
| 8 | Fusion 360 library | Autodesk Fusion | .lbr | converted, or reported with kicad-cli output |
