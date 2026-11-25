# Test-data manifest

`testdata/` is git-ignored: the files below are downloaded by hand (nothing in them is ever executed) and
`crates/klm-core/tests/corpus_e2e.rs` skips any case whose file is missing. Renders land in `target/corpus-renders/`.

## Corpus

| ID | File (in `testdata/`) | Format | Source | Licence | Size | SHA-256 |
|----|------|--------|--------|---------|------|---------|
| C01 | `corpus/nils_battery.SchLib` | Altium SchLib | https://raw.githubusercontent.com/NilsMinor/AltiumLibrary/master/sch/battery.SchLib | GPL-3.0 (repo LICENSE) | 17 408 | `86962d7dafc96216e44649eca31c68b8e6c5cf7ec1e5238a777cb88485b62d73` |
| C02 | `corpus/kicad_mounting_holes.SchLib` | Altium SchLib | https://raw.githubusercontent.com/KiCad/kicad-source-mirror/master/qa/data/eeschema/plugins/altium/issue22943/mounting_holes.SchLib | GPL-3.0 (KiCad source) | 6 656 | `b510e91114cb2365117e01ece6c9709a11e0f1eaf1b016bef38419aa9beb56a0` |
| C03 | `corpus/kicad_esp32_wroom.PcbLib` | Altium PcbLib | https://raw.githubusercontent.com/KiCad/kicad-source-mirror/master/qa/data/pcbnew/plugins/altium/pcblib/Espressif%20ESP32-WROOM-32.PcbLib | GPL-3.0 (KiCad source) | 236 544 | `02070d8beb61ca37770d77d6d54109a1fdd045ddd9b6ad565c0f5cda1cbe2068` |
| C04 | `corpus/cc0_Everlight.IntLib` | Altium IntLib | https://raw.githubusercontent.com/francescosacco/HardwareLibrary/master/Everlight.IntLib | CC0-1.0 (repo) | 21 504 | `20a92281a7a000167a1c5047c2215dd3e637e1d2d208cbb7210b602fc331bc8c` |
| C05 | `corpus/kicad_LS2K0300.elibz2` | EasyEDA Pro v3 library | https://raw.githubusercontent.com/KiCad/kicad-source-mirror/master/qa/data/eeschema/plugins/easyedapro/LS2K0300_Symbol.elibz2 | "provided by contributor for QA testing" (KiCad `sources.txt`), no stated open licence: local test use only | 26 843 | `7bf1579aab7d5deda390869d853cb9dbfce34968849edacd7888d97fb56cfb72` |
| C06a | `corpus/lcsc_C8734.json` | EasyEDA/LCSC component JSON (what the app consumes) | https://easyeda.com/api/products/C8734/components?version=6.4.19.5 (STM32F103C8T6) | public API reply, local test use only | 91 466 | `0c4bedebf84b578ce153da34c247c6cb0140df48453aba2ac12048dc259a9d8e` |
| C06b | `corpus/lcsc_C14663.json` | same | https://easyeda.com/api/products/C14663/components?version=6.4.19.5 (100 nF 0402/0603 cap) | same | 13 600 | `0042a04f61593b8dba941127e35bf2ca23bb4984328a36576bfd559f6719970c` |
| C07 | `corpus/kicad_Regulator_Linear.lib` | legacy KiCad `.lib` | https://raw.githubusercontent.com/KiCad/kicad-symbols/master/Regulator_Linear.lib | CC-BY-SA 4.0 with KiCad library exception | 142 533 | `c3dcc5192df1ad5d2f206c281a81e72238be3f261e583ea360c5965b1d97a30c` |
| C08a | `ul_4515DO-DS3BK004DP (1).zip` | Ultra Librarian ZIP, KiCad v6 + loose STL | ultralibrarian.com download of part 4515DO-DS3BK004DP | vendor download, **not redistributable** | 126 916 | `71ba55ba01169f53867a9aa9dae18ff360a4d01164da73ace719e844e136a9d3` |
| C08b | `ul_4515DO-DS3BK004DP (2).zip` | Ultra Librarian ZIP, Altium + STL | same | same | 148 444 | `b27915313288a6a3a1e7043b06bf3dcc37e0785b25c6de6e5456e87463f19d09` |
| C08c | `ul_4515DO-DS3BK004DP (3).zip` | Ultra Librarian ZIP, Fusion 360 `.lbr` + STL | same | same | 122 707 | `a91145959c719b38c54abf94e085b7639741c062942b476a6591b0f2691e3a89` |
| C08d | `ul_4515DO-DS3BK004DP.zip` | Ultra Librarian ZIP, KiCad + Altium + Fusion | same | same | 37 381 | `bd7f3fbfd301e92ef3d76fb93dea48755e0651fd5ad458991da8c0207f51ded9` |
| C09 | built by the test from the `.stl` in C08a | STL-only ZIP | derived from the Ultra Librarian download | as C08 | ~0.5 MB | not stored |

## More vendor downloads worth trying

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
