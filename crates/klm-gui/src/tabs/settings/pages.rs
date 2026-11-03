//! One function per Settings page; each edits the popup's copy of `Settings`.

use super::super::ui_kit::{Level, Section, Status, badge, status_cell};
use super::super::standards;
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
        Section::Search => "Search",
        Section::Import => "Import",
        Section::Standards | Section::Symbols => "Standards › Symbols",
        Section::Footprints => "Standards › Footprints",
        Section::Bom => "BOM",
        Section::History => "History",
    }
}

fn source_info(s: Section) -> Option<&'static SourceInfo> {
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
        Section::Import => (d.rules, d.house_symbols_on_import) = (def.rules, def.house_symbols_on_import),
        Section::Symbols => standards::reset_symbols(&mut d.standards),
        Section::Footprints => standards::reset_footprints(&mut d.standards),
        Section::Bom => d.shipping_region = def.shipping_region,
        Section::History => d.history = History::default(),
        s => {
            if let Some(info) = source_info(s) {
                d.source_enabled.remove(info.name);
            }
        }
    }
}

/// A titled group of options filling the row.
fn group<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong(title);
            ui.add_space(2.0);
            add(ui)
        })
        .inner
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
            Section::General | Section::Libraries => group(ui, "Global library", |ui| {
                ui.label(live.global_root.as_ref().map_or("Not set up yet: import a part to choose one.".into(), |g| g.display().to_string()));
                ui.weak("Where KLM keeps the parts that every project shares.");
                if live.global_root.is_some() && ui.button("Change location…").on_hover_text("Move the library, or point KLM at where you moved it").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("New location for the global library").pick_folder() {
                        cmd = Some(Cmd::MoveLibrary(dir));
                    }
                }
            }),
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
                if ui.button("Add folder…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Folder to watch for vendor ZIPs").pick_folder() {
                        if !d.watch_dirs.contains(&dir) {
                            d.watch_dirs.push(dir);
                        }
                    }
                }
            }),
            Section::KiCad => group(ui, KICAD_CLI, |ui| {
                ui.weak("Used to upgrade imported parts to your KiCad's format. Set it if KiCad isn't installed in the usual place.");
                ui.label(match &d.kicad_cli {
                    Some(p) => p.display().to_string(),
                    None => kicad_env::kicad_cli().map_or("none found".into(), |p| format!("{} (found automatically)", p.display())),
                });
                ui.horizontal(|ui| {
                    if ui.button("Browse…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().set_title("kicad-cli executable").pick_file() {
                            d.kicad_cli = Some(f);
                        }
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
                        ui.weak(format!("Placeholders: {}. Empty means just the MPN.", TEMPLATE_FIELDS.iter().map(|f| format!("{{{f}}}")).collect::<Vec<_>>().join(" ")));
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
                    })
                },
            ),
            Section::Standards | Section::Symbols => group(ui, "Symbols", |ui| {
                ui.weak("Fixes that make parts follow the KiCad Library Conventions. Untick a rule to leave that alone. The Library tab runs them.");
                standards::symbols(ui, &mut d.standards);
            }),
            Section::Footprints => group(ui, "Footprints", |ui| {
                ui.weak("Fixes that make parts follow the KiCad Library Conventions. Untick a rule to leave that alone. The Library tab runs them.");
                standards::footprints(ui, &mut d.standards);
            }),
            Section::Bom => group(ui, "Shipping region", |ui| {
                ui.weak("Country code (US, DE, ...) used for BOM stock and prices; each project can override it. Only DigiKey uses it so far.");
                ui.add(egui::TextEdit::singleline(&mut d.shipping_region).hint_text("e.g. DE").desired_width(80.0));
            }),
            Section::History => {
                two(
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
                )
            }
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
            ui.weak("Kept in Windows Credential Manager, not in the settings file. Saved at once, whether or not you press OK.");
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
