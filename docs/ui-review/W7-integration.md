# Wave 7 integration: test plan and results

W7.0–W7.5 were built on separate branches from W7.0, then merged together (2e53f94). This page covers what only works once they are together: the links between tabs and to Settings pages, which each branch had to point at `Section::General` or the clipboard. All checks run headless in the sandbox, as for the other pages.

## Test plan

| ID | What is checked | Steps | Expected | How |
|---|---|---|---|---|
| X1 | Search's "No sources enabled" link | Search with no source, click "Enable one in Settings" | Settings opens on Sources | `search::s6_no_sources_links_to_settings` |
| X2 | Search's "Open in Library" | results with LM358 in the library, click its "In library" badge | Library tab, filtered to LM358 only | `search::s3_in_library_badge_and_open_in_library`, snapshot |
| X3 | BOM's "Search" on a missing part | BOM, click Search on the missing capacitor | Search tab, a search run for that MPN (no clipboard step) | `other_tabs::b3_missing_part_actions` |
| X4 | Import's "Watch folders…" | click it | Settings on General › Watch folders | `other_tabs::i3_watch_folder_link_opens_settings` |
| X5 | History's "Retention settings…" | click it | Settings on History | `other_tabs::h5_retention_link` |
| X6 | BOM's "Set in Settings…" (region) | click it with no region | Settings on BOM | `other_tabs::b5_region_link` |
| X7 | First-run checklist links | first run, click the first Open | Settings on General › Libraries (others: KiCad, Sources, Watch folders) | `shell::s9_first_run_checklist` |
| X8 | Search's side list on a narrow window | Search at 820×560 | list starts closed (W7.4), opens with "Saved & recent", glyphs draw | `bugs::b4_search_sidebar_icons_have_glyphs` |
| X9 | Nothing else regressed | `cargo test --workspace` | all suites pass, no warnings | full run |

## Results

| ID | Result | Evidence |
|---|---|---|
| X1 | PASS | test only |
| X2 | PASS: filter box holds LM358, "1 shown" | [snapshot](W7-integration/library-opened-from-search-1280x800.png) |
| X3 | PASS: the MPN is the latest recent search. With no source enabled Search shows its "no sources" view rather than the box | test only |
| X4 | PASS | test only |
| X5 | PASS | test only |
| X6 | PASS | test only |
| X7 | PASS | test only |
| X8 | PASS: the test opens the list first on narrow windows | test only |
| X9 | PASS: every suite ok (klm-gui 89), 0 warnings | full run |

## Still open after Wave 7

- b7: some widgets have no accessible name (`bugs::b7_every_widget_has_an_accessible_name` is still `should_panic`).
- Restyle's Apply recomputes the change instead of applying the reviewed one.
- A missing-datasheet cell runs the finder for the whole library (core has no per-part finder).
- BOM's alternate swap is view-only: not written to the schematic or the alternatives store.
- Not checked by hand in a real window or with KiCad: that is TESTING.md's manual pass.
