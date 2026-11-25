//! One function per Settings page; each edits the page's copy of `Settings`.

use super::super::page_kit;
use super::super::ui_kit::{Level, Section, Status, badge, status_cell};
use super::super::{part_info, standards};
use super::{Cmd, KICAD_CLI, SettingsTab};
use eframe::egui::{self, Ui};
use klm_core::cache::DEFAULT_TTL_MINUTES;
use klm_core::credentials::{self, SOURCES, SourceInfo};
use klm_core::kicad_env;
use klm_core::rules::{MpnCase, TEMPLATE_FIELDS};
use klm_core::settings::{AutoVersion, History, Settings};

pub fn title(s: Section) -> &'static str {
    match s {
        Section::General | Section::Libraries => "General › Libraries",
        Section::WatchFolders => "General › Watch folders",
        Section::KiCad => "General › KiCad",
        Section::Sources | Section::Lcsc => "Sources › LCSC",
        Section::DigiKey => "Sources › DigiKey",
        Section::Mouser => "Sources › Mouser",
        Section::Nexar => "Sources › Nexar",
        Section::SnapMagic => "Sources › SnapMagic",
        Section::Search => "Workflow › Search",
        Section::Import => "Workflow › Import",
        Section::Standards | Section::Symbols => "Standards › Symbols",
        Section::Footprints => "Standards › Footprints",
        Section::PartInfo => "Standards › Part information",
        Section::Bom => "Workflow › BOM",
        Section::History => "Workflow › History",
    }
}

pub(super) fn source_info(s: Section) -> Option<&'static SourceInfo> {
    let name = match s {
        Section::Lcsc | Section::Sources => "LCSC",
        Section::DigiKey => "DigiKey",
        Section::Mouser => "Mouser",
        Section::Nexar => "Nexar",
        Section::SnapMagic => "SnapMagic",
        _ => return None,
    };
    SOURCES.iter().find(|i| i.name == name)
}

/// The Libraries page has nothing to restore: the library's place is changed, not set.
pub fn has_defaults(s: Section) -> bool {
    !matches!(s, Section::General | Section::Libraries)
}

pub fn restore(d: &mut Settings, s: Section) {
    let def = Settings::default();
    match s {
        Section::WatchFolders => d.watch_dirs = def.watch_dirs,
        Section::KiCad => d.kicad_cli = None,
        Section::Search => (d.cache_ttl_minutes, d.display_currency) = (def.cache_ttl_minutes, def.display_currency),
        Section::Import => (d.rules, d.house_symbols_on_import, d.sim) = (def.rules, def.house_symbols_on_import, def.sim),
        Section::Symbols => standards::reset_symbols(&mut d.standard.rules),
        Section::Footprints => standards::reset_footprints(&mut d.standard.rules),
        Section::PartInfo => d.standard.classes.clear(),
        Section::Bom => (d.shipping_region, d.bom_fields) = (def.shipping_region, def.bom_fields),
        Section::History => d.history = History::default(),
        s => {
            if let Some(info) = source_info(s) {
                d.source_enabled.remove(info.name);
            }
        }
    }
}

/// A titled card of options filling the row.
fn group<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let r = page_kit::card(ui, Some(title), add);
    ui.add_space(8.0);
    r
}

/// Two groups side by side when there is room, else one under the other.
fn two<T>(ui: &mut Ui, t: &mut T, a: impl FnOnce(&mut Ui, &mut T), b: impl FnOnce(&mut Ui, &mut T)) {
    if ui.available_width() >= 640.0 {
        ui.columns(2, |c| {
            a(&mut c[0], t);
            b(&mut c[1], t);
        });
    } else {
        a(ui, t);
        b(ui, t);
    }
}

impl SettingsTab {
    pub(super) fn page(&mut self, ui: &mut Ui, d: &mut Settings, live: &Settings) -> Option<Cmd> {
        let mut cmd = None;
        match self.section {
            Section::General | Section::Libraries => {
                group(ui, "Global library", |ui| {
                    ui.label(live.global_root.as_ref().map_or("Not set up yet: import a part to choose one.".into(), |g| g.display().to_string()));
                    ui.weak("Where KLM keeps the parts that every project shares.");
                    if live.global_root.is_some()
                        && ui.button("Change location…").on_hover_text("Move the library, or point KLM at where you moved it").clicked()
                        && let Some(dir) = rfd::FileDialog::new().set_title("New location for the global library").pick_folder()
                    {
                        cmd = Some(Cmd::MoveLibrary(dir));
                    }
                });
                group(ui, "Appearance", |ui| {
                    let (mut pref, before) = (ui.ctx().options(|o| o.theme_preference), ui.ctx().options(|o| o.theme_preference));
                    ui.horizontal(|ui| {
                        for (p, label) in
                            [(egui::ThemePreference::System, "System"), (egui::ThemePreference::Light, "Light"), (egui::ThemePreference::Dark, "Dark")]
                        {
                            ui.selectable_value(&mut pref, p, label);
                        }
                    });
                    ui.weak("Dark is Bench, light is Datasheet; System follows the OS. Applied at once, not part of Save.");
                    if pref != before {
                        ui.ctx().set_theme(pref);
                    }
                });
            }
            Section::WatchFolders => group(ui, "Folders watched for vendor ZIPs", |ui| {
                ui.weak("KLM watches these for downloads from SnapMagic, Ultra Librarian and SamacSys. With none added, the OS Downloads folder is used.");
                let mut remove = None;
                for (i, dir) in d.watch_dirs.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                        ui.label(dir.display().to_string());
                    });
                }
                if let Some(i) = remove {
                    d.watch_dirs.remove(i);
                }
                if d.watch_dirs.is_empty() {
                    ui.label(d.downloads().first().map_or("none found".into(), |p| format!("{} (default)", p.display())));
                }
                if ui.button("Add folder…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_title("Folder to watch for vendor ZIPs").pick_folder()
                    && !d.watch_dirs.contains(&dir)
                {
                    d.watch_dirs.push(dir);
                }
            }),
            Section::KiCad => group(ui, KICAD_CLI, |ui| {
                ui.weak("Used to upgrade imported parts to your KiCad's format. Set it if KiCad isn't installed in the usual place.");
                ui.label(match &d.kicad_cli {
                    Some(p) => p.display().to_string(),
                    None => kicad_env::kicad_cli().map_or("none found".into(), |p| format!("{} (found automatically)", p.display())),
                });
                ui.horizontal(|ui| {
                    if ui.button("Browse…").clicked()
                        && let Some(f) = rfd::FileDialog::new().set_title("kicad-cli executable").pick_file()
                    {
                        d.kicad_cli = Some(f);
                    }
                    if d.kicad_cli.is_some() && ui.button("Use default").clicked() {
                        d.kicad_cli = None;
                    }
                    if self.test_row(ui, KICAD_CLI, true, "Runs kicad-cli version") {
                        cmd = Some(Cmd::Test(KICAD_CLI));
                    }
                });
            }),
            Section::Lcsc | Section::DigiKey | Section::Mouser | Section::Nexar | Section::SnapMagic | Section::Sources => {
                cmd = self.source(ui, d, source_info(self.section).expect("a source page"));
            }
            Section::Search => two(
                ui,
                d,
                |ui, d| {
                    group(ui, "Result cache", |ui| {
                        let mut minutes = d.cache_ttl_minutes.unwrap_or(DEFAULT_TTL_MINUTES);
                        ui.horizontal(|ui| {
                            ui.label("Reuse search results for");
                            if ui.add(egui::DragValue::new(&mut minutes).range(0..=100_000).suffix(" min")).changed() {
                                d.cache_ttl_minutes = Some(minutes);
                            }
                        });
                        ui.weak("0 turns the cache off. The default is a day (1440 min).");
                    })
                },
                |ui, d| {
                    group(ui, "Prices", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Display currency");
                            ui.add(egui::TextEdit::singleline(&mut d.display_currency).hint_text("e.g. EUR").desired_width(70.0));
                        });
                        ui.weak("Empty shows each source's own currency.");
                    })
                },
            ),
            Section::Import => two(
                ui,
                d,
                |ui, d| {
                    group(ui, "Category rules", |ui| {
                        ui.weak("Checked in order before the built-in guess: text containing the keyword (a word or a package such as SOT-23) goes to that category.");
                        let mut remove = None;
                        for (i, r) in d.rules.categories.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut r.keyword).hint_text("keyword or package").desired_width(120.0));
                                ui.label("→");
                                ui.add(egui::TextEdit::singleline(&mut r.category).hint_text("category").desired_width(120.0));
                                if ui.small_button("Remove").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                        if let Some(i) = remove {
                            d.rules.categories.remove(i);
                        }
                        if ui.button("Add category rule").clicked() {
                            d.rules.categories.push(Default::default());
                        }
                    })
                },
                |ui, d| {
                    group(ui, "Part names", |ui| {
                        let rules = &mut d.rules;
                        ui.horizontal(|ui| {
                            ui.label("Name template");
                            ui.add(egui::TextEdit::singleline(&mut rules.name_template).hint_text("{Manufacturer}_{MPN}").desired_width(f32::INFINITY));
                        });
                        ui.weak(format!(
                            "Placeholders: {}. Empty means just the MPN.",
                            TEMPLATE_FIELDS.iter().map(|f| format!("{{{f}}}")).collect::<Vec<_>>().join(" ")
                        ));
                        ui.horizontal(|ui| {
                            ui.label("MPN case");
                            for (case, label) in [(MpnCase::Keep, "Keep"), (MpnCase::Upper, "UPPER"), (MpnCase::Lower, "lower")] {
                                ui.selectable_value(&mut rules.mpn_case, case, label);
                            }
                        });
                        ui.weak(format!("Example: {}", rules.name(&[("MPN", "Lm358n"), ("Manufacturer", "TI"), ("Package", "DIP-8")])));
                    });
                    group(ui, "House-style symbols", |ui| {
                        ui.checkbox(&mut d.house_symbols_on_import, "Redraw imported symbols in the house style (resistor, capacitor, IC box…)")
                            .on_hover_text("Parts that can't be mapped with certainty keep the vendor's symbol. \"Restyle symbol\" in the Library tab does the same for existing parts.");
                    });
                    group(ui, "SPICE simulation models", |ui| {
                        ui.checkbox(&mut d.sim.auto_sim, "Find close SPICE models automatically on import")
                            .on_hover_text("When an imported part has no SPICE file, search existing library parts, KiCad built-in models, and your SPICE folders for an exact or close variant.");
                        ui.add_enabled(
                            d.sim.auto_sim,
                            egui::Checkbox::new(&mut d.sim.auto_generate_from_datasheet, "Generate from datasheet PDF / specs & similar components"),
                        )
                        .on_hover_text("When no exact/close match exists, parse electrical ratings from the component's datasheet & parameters and adapt a similar model if available.");
                        ui.weak("Extra local SPICE folders (.lib, .cir, .sub, .sp, .mod):");
                        let mut remove = None;
                        for (i, dir) in d.sim.spice_dirs.iter().enumerate() {
                            ui.horizontal(|ui| {
                                if ui.small_button("Remove").clicked() {
                                    remove = Some(i);
                                }
                                ui.label(dir.display().to_string());
                            });
                        }
                        if let Some(i) = remove {
                            d.sim.spice_dirs.remove(i);
                        }
                        if ui.button("Add SPICE folder…").clicked()
                            && let Some(dir) = rfd::FileDialog::new().set_title("Folder containing SPICE models").pick_folder()
                            && !d.sim.spice_dirs.contains(&dir)
                        {
                            d.sim.spice_dirs.push(dir);
                        }
                    })
                },
            ),
            Section::Standards | Section::Symbols | Section::Footprints => {
                if let Some(e) = group(ui, "Standard profile", |ui| standards::profile(ui, d)) {
                    cmd = Some(Cmd::Log(Level::Error, e));
                }
                if matches!(self.section, Section::Footprints) {
                    group(ui, "Manufacturer", |ui| {
                        let fab = &mut d.standard.fab;
                        ui.weak("Footprints and boards are held to these. With several fabs ticked the strictest value of each limit applies.");
                        for f in klm_core::fab_data::fabs() {
                            let mut on = fab.fabs.contains(&f.id);
                            if ui.checkbox(&mut on, &f.name).on_hover_text(format!("{}\nchecked {}", f.source, f.checked)).changed() {
                                fab.fabs.retain(|i| *i != f.id);
                                if on {
                                    fab.fabs.push(f.id);
                                }
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.label("IPC class");
                            for c in 1..=3u8 {
                                ui.selectable_value(&mut fab.ipc_class, c, c.to_string());
                            }
                            ui.label("Density");
                            for c in ['A', 'B', 'C'] {
                                ui.selectable_value(&mut fab.density, c, c.to_string());
                            }
                        });
                        let l = klm_core::fab_data::limits(fab);
                        let mm = |v: Option<f64>| v.map_or("-".into(), |v| format!("{v:.3}"));
                        ui.weak(format!(
                            "Track {} · gap {} · drill {} · ring {} · via {}/{} · mask web {} · edge {} · courtyard +{} (mm)",
                            mm(l.min_track),
                            mm(l.min_clearance),
                            mm(l.min_drill),
                            mm(l.min_annular_ring),
                            mm(l.min_via_drill),
                            mm(l.min_via_diameter),
                            mm(l.min_mask_web),
                            mm(l.min_edge_clearance),
                            mm(l.courtyard_excess)
                        ));
                    });
                }
                let footprints = matches!(self.section, Section::Footprints);
                group(ui, if footprints { "Footprints" } else { "Symbols" }, |ui| {
                    ui.weak("Fixes that make parts follow the active profile (KiCad Library Conventions by default). Untick a rule to leave that alone. The Library tab runs them.");
                    if footprints { standards::footprints(ui, &mut d.standard.rules) } else { standards::symbols(ui, &mut d.standard.rules) }
                });
                group(ui, "House templates", |ui| standards::house_templates(ui, &mut d.standard));
            }
            Section::PartInfo => part_info::page(ui, &mut d.standard, &mut self.info_class),
            Section::Bom => {
                group(ui, "Shipping region", |ui| {
                    ui.weak("Country code (US, DE, ...) used for BOM stock and prices; each project can override it. Only DigiKey uses it so far.");
                    ui.add(egui::TextEdit::singleline(&mut d.shipping_region).hint_text("e.g. DE").desired_width(80.0));
                });
                group(ui, "Field names", |ui| {
                    ui.weak("The schematic fields that hold each part number, if your symbols name them differently. A project can override them in its .klm-bom.json.");
                    let f = &mut d.bom_fields;
                    for (label, v) in [
                        ("MPN", &mut f.mpn),
                        ("Manufacturer", &mut f.manufacturer),
                        ("LCSC", &mut f.lcsc),
                        ("DigiKey", &mut f.digikey),
                        ("Mouser", &mut f.mouser),
                    ] {
                        ui.horizontal(|ui| {
                            ui.add_sized([100.0, 20.0], egui::Label::new(label));
                            ui.add(egui::TextEdit::singleline(v).desired_width(200.0));
                        });
                    }
                });
            }
            Section::History => two(
                ui,
                &mut d.history,
                |ui, h| {
                    group(ui, "Automatic snapshots", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Keep the last");
                            ui.add(egui::DragValue::new(&mut h.keep_minors).range(1..=500));
                            ui.label("per library");
                        });
                        ui.horizontal(|ui| {
                            ui.label("Size cap");
                            let mut cap = h.max_minor_mb.unwrap_or(0);
                            ui.add(egui::DragValue::new(&mut cap).range(0..=100_000).suffix(" MB"));
                            h.max_minor_mb = (cap > 0).then_some(cap);
                        });
                        ui.weak("0 means no cap. Versions are never removed.");
                    })
                },
                |ui, h| {
                    group(ui, "Versions", |ui| {
                        egui::ComboBox::from_label("Make a version automatically").selected_text(format!("{:?}", h.auto_version)).show_ui(ui, |ui| {
                            for v in [AutoVersion::Off, AutoVersion::Daily, AutoVersion::Weekly, AutoVersion::Monthly] {
                                ui.selectable_value(&mut h.auto_version, v, format!("{v:?}"));
                            }
                        });
                    })
                },
            ),
        }
        cmd
    }

    /// The Test button and its last result; true when clicked.
    fn test_row(&mut self, ui: &mut Ui, what: &'static str, enabled: bool, hint: &str) -> bool {
        let running = self.testing.contains(what);
        let clicked = ui.add_enabled(enabled && !running, egui::Button::new("Test")).on_hover_text(hint).clicked();
        if running {
            ui.spinner();
        }
        match self.results.get(what) {
            Some(Ok(found)) => badge(ui, &format!("✔ {found}"), Level::Ok),
            Some(Err(why)) => badge(ui, &format!("✘ {}", why.chars().take(90).collect::<String>()), Level::Error).on_hover_text(why),
            None => return clicked,
        };
        clicked
    }

    fn source(&mut self, ui: &mut Ui, d: &mut Settings, info: &'static SourceInfo) -> Option<Cmd> {
        let mut cmd = None;
        group(ui, "Status", |ui| {
            if info.searchable {
                let mut on = d.is_enabled(info.name);
                if ui.checkbox(&mut on, "Enabled").on_hover_text("A source is searched when it is ticked and has all its keys").changed() {
                    d.source_enabled.insert(info.name.into(), on);
                }
            } else {
                ui.weak("Kept for later: there is no search source for it yet.");
            }
        });
        group(ui, "API keys", |ui| {
            if info.fields.is_empty() {
                ui.weak(format!("{} needs no keys.", info.name));
                return;
            }
            ui.weak("Kept in Windows Credential Manager, not in the settings file. Saved at once, whether or not you press Save.");
            for (key, label) in info.fields {
                let id = (info.name, *key);
                ui.horizontal(|ui| {
                    ui.label(*label);
                    status_cell(ui, if self.stored.contains(&id) { Status::Ok } else { Status::Missing }, "stored key");
                });
                ui.horizontal(|ui| {
                    let show = self.shown.contains(&id);
                    let text = self.inputs.entry(id).or_default();
                    ui.add(egui::TextEdit::singleline(text).password(!show).hint_text(*label).desired_width((ui.available_width() - 40.0).max(120.0)));
                    if ui.selectable_label(show, "👁").on_hover_text("Show or hide what you type").clicked() && !self.shown.remove(&id) {
                        self.shown.insert(id);
                    }
                });
            }
            ui.horizontal(|ui| {
                let typed = info.fields.iter().any(|f| self.inputs.get(&(info.name, f.0)).is_some_and(|t| !t.trim().is_empty()));
                if ui.add_enabled(typed, egui::Button::new("Save keys")).clicked() {
                    for (key, _) in info.fields {
                        let text = self.inputs.remove(&(info.name, *key)).unwrap_or_default();
                        if text.trim().is_empty() {
                            continue;
                        }
                        if let Err(e) = credentials::set(info.name, key, text.trim()) {
                            cmd = Some(Cmd::Log(Level::Error, format!("{}: {e:#}", info.name)));
                        }
                    }
                    self.refresh();
                    cmd.get_or_insert(Cmd::Rebuild);
                }
                if ui.button("Clear keys").clicked() {
                    for (key, _) in info.fields {
                        self.inputs.remove(&(info.name, *key));
                        if let Err(e) = credentials::clear(info.name, key) {
                            cmd = Some(Cmd::Log(Level::Error, format!("{}: {e:#}", info.name)));
                        }
                    }
                    self.refresh();
                    cmd.get_or_insert(Cmd::Rebuild);
                }
            });
        });
        if info.searchable {
            group(ui, "Test", |ui| {
                let complete = info.fields.iter().all(|f| self.stored.contains(&(info.name, f.0)));
                ui.horizontal(|ui| {
                    if self.test_row(ui, info.name, complete, "Runs a search for LM358 with the stored keys") {
                        cmd = Some(Cmd::Test(info.name));
                    }
                });
                if !complete {
                    ui.weak("Save the keys first.");
                }
            });
        }
        cmd
    }
}
