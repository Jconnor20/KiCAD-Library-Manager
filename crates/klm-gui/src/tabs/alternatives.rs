use eframe::egui;
use crate::{AppCtx, Msg as AppMsg, new_work_dir};
use klm_core::alternatives::{self, LibPart, ROLES};
use klm_core::library::LibraryRoot;
use klm_core::part::Part;
use klm_core::sources::combine;
use klm_core::store::{Alternative, Store};
use std::sync::mpsc::{Receiver, channel};

/// Per suggested group: (index into `parts`, role, ticked).
type Groups = Vec<Vec<(usize, String, bool)>>;

/// Suggested groups of equivalent parts in one library. The user ticks the members, picks a role for
/// each and confirms; only then is the group stored and written into the symbols.
pub struct AlternativesPanel {
    root: LibraryRoot,
    parts: Vec<LibPart>,
    /// Per suggested group: (index into `parts`, role, ticked).
    groups: Groups,
    /// The scan runs on a worker; this is where its result arrives.
    rx: Option<Receiver<(Vec<LibPart>, Groups)>>,
    /// The library part the online and manual alternates are added to.
    base: usize,
    /// Distributor results for `base` as (source index, part); the search runs on a worker.
    online: Vec<(usize, Part)>,
    searching: Option<Receiver<Vec<(usize, Part)>>>,
    /// Hand-entered alternate: MPN, manufacturer, LCSC, DigiKey, Mouser.
    manual: [String; 5],
}

impl AlternativesPanel {
    pub fn new(root: &LibraryRoot, ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let (r, ctx) = (root.clone(), ctx.clone());
        std::thread::spawn(move || {
            let parts = alternatives::scan(&r);
            let store = Store::default_path().and_then(|p| Store::open(&p).ok());
            let groups = alternatives::suggest(&parts)
                .into_iter()
                .map(|g| {
                    g.into_iter()
                        .enumerate()
                        .map(|(n, i)| {
                            // A stored role wins; otherwise the first part is the suggested preferred one.
                            let stored = store.as_ref().and_then(|s| alternatives::alternatives_of(s, &parts[i].mpn).ok()).and_then(|grp| {
                                grp.into_iter().find(|a| klm_core::matching::same_mpn(&a.mpn, &parts[i].mpn)).map(|a| a.role)
                            });
                            (i, stored.unwrap_or_else(|| if n == 0 { "preferred" } else { "substitute" }.into()), true)
                        })
                        .collect()
                })
                .collect();
            let _ = tx.send((parts, groups));
            ctx.request_repaint();
        });
        Self { root: root.clone(), parts: Vec::new(), groups: Vec::new(), rx: Some(rx), base: 0, online: Vec::new(), searching: None, manual: Default::default() }
    }

    /// Equivalents from distributors, and alternates typed in by hand, for one library part.
    fn beyond_library(&mut self, ui: &mut egui::Ui, cx: &mut AppCtx) {
        let Some(base) = self.parts.get(self.base).cloned() else { return };
        ui.heading("Alternates from outside the library");
        egui::ComboBox::from_label("For part").selected_text(format!("{} ({})", base.mpn, base.category)).show_ui(ui, |ui| {
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for (i, p) in self.parts.iter().enumerate() {
                    if ui.selectable_label(i == self.base, format!("{} ({})", p.mpn, p.category)).clicked() {
                        (self.base, self.online) = (i, Vec::new());
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            let can = self.searching.is_none() && !cx.sources.is_empty();
            if ui.add_enabled(can, egui::Button::new("Find equivalents online")).on_disabled_hover_text("Enable a distributor in Settings").clicked() {
                let (tx, rx) = channel();
                let (sources, ctx, query, base) = (cx.sources.clone(), cx.ctx.clone(), alternatives::search_query(&base), base.clone());
                std::thread::spawn(move || {
                    let mut found = vec![];
                    for (i, src) in sources.iter().enumerate() {
                        // A source that fails just contributes nothing.
                        let hits = src.search(&query).unwrap_or_default();
                        found.extend(alternatives::equivalents(&base, hits).into_iter().map(|p| (i, p)));
                    }
                    let _ = tx.send(found);
                    ctx.request_repaint();
                });
                self.searching = Some(rx);
            }
            if self.searching.is_some() {
                ui.spinner();
            }
        });
        let mut add = None;
        egui::Grid::new("alt_online").striped(true).show(ui, |ui| {
            for (n, (src, p)) in self.online.iter().enumerate() {
                ui.label(&p.mpn);
                ui.label(&p.manufacturer);
                ui.label(&p.package);
                ui.label(cx.sources.get(*src).map_or("", |s| s.name()));
                if ui.button("Add as alternate").clicked() {
                    add = Some((n, false));
                }
                if ui.button("Add and import CAD…").clicked() {
                    add = Some((n, true));
                }
                ui.end_row();
            }
        });
        if let Some((n, cad)) = add {
            let (src, part) = self.online[n].clone();
            self.add(cx, &base, Alternative::from(&part), Some((src, part)).filter(|_| cad));
        }
        ui.horizontal(|ui| {
            for (field, hint) in self.manual.iter_mut().zip(["MPN", "Manufacturer", "LCSC", "DigiKey", "Mouser"]) {
                ui.add(egui::TextEdit::singleline(field).hint_text(hint).desired_width(110.0));
            }
            if ui.button("Add by hand").clicked() {
                let ids = ["LCSC", "DigiKey", "Mouser"].into_iter().zip(&self.manual[2..]).filter(|(_, id)| !id.trim().is_empty());
                let alt = Alternative {
                    mpn: self.manual[0].trim().into(),
                    manufacturer: self.manual[1].trim().into(),
                    supplier_ids: ids.map(|(s, id)| (s.to_string(), id.trim().to_string())).collect(),
                    role: "substitute".into(),
                };
                self.add(cx, &base, alt, None);
                self.manual = Default::default();
            }
        });
    }

    /// Stores the alternate, writes it into the symbols and, if asked, opens the import dialog for its CAD.
    fn add(&mut self, cx: &mut AppCtx, base: &LibPart, alt: Alternative, cad: Option<(usize, Part)>) {
        let mpn = alt.mpn.clone();
        let done = Store::default_path()
            .ok_or_else(|| anyhow::anyhow!("no config folder for the parts database"))
            .and_then(|p| Store::open(&p))
            .and_then(|store| alternatives::add_alternate(&self.root, &store, base, alt));
        match done {
            Ok(written) => cx.log.push(format!("Added {mpn} as an alternate of {}; wrote it into {written} symbols", base.mpn)),
            Err(e) => return cx.log.push(format!("Could not add {mpn}: {e:#}")),
        }
        if let Some((from, part)) = cad {
            let work = new_work_dir();
            cx.send(AppMsg::Import(super::import::Msg::Gather { part: part.clone(), work: work.clone(), waiting: cx.sources.len() }));
            let (tx, sources, ctx) = (cx.tx.clone(), cx.sources.clone(), cx.ctx.clone());
            std::thread::spawn(move || {
                combine::gather(&sources, &part, from, &work, |source, found| {
                    let _ = tx.send(AppMsg::Import(super::import::Msg::Answer { work: work.clone(), source: source.to_string(), found }));
                    ctx.request_repaint();
                });
            });
        }
    }

    /// Returns false once closed.
    pub fn window(&mut self, cx: &mut AppCtx) -> bool {
        let mut open = true;
        let ctx = cx.ctx.clone();
        egui::Window::new("Alternatives and duplicates").open(&mut open).default_width(720.0).show(&ctx, |ui| {
            if let Some(rx) = &self.rx {
                match rx.try_recv() {
                    Ok((parts, groups)) => {
                        (self.parts, self.groups, self.rx) = (parts, groups, None);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Scanning the library…");
                        });
                        return;
                    }
                    Err(_) => self.rx = None,
                }
            }
            if let Some(rx) = &self.searching {
                if let Ok(found) = rx.try_recv() {
                    (self.online, self.searching) = (found, None);
                }
            }
            self.beyond_library(ui, cx);
            ui.separator();
            if self.groups.is_empty() {
                ui.label("No likely-equivalent parts found in this library.");
                return;
            }
            ui.label("Parts with the same package and the same MPN or value. Tick the ones that really are equivalent, pick their roles and confirm. Nothing is merged; each symbol gets MPN_AltN fields for the BOM.");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (g, group) in self.groups.iter_mut().enumerate() {
                    ui.separator();
                    egui::Grid::new(("alt_group", g)).num_columns(6).striped(true).show(ui, |ui| {
                        for (i, role, ticked) in group.iter_mut() {
                            let p = &self.parts[*i];
                            ui.checkbox(ticked, "");
                            ui.label(&p.mpn);
                            ui.label(&p.manufacturer);
                            ui.label(&p.value);
                            ui.label(&p.footprint).on_hover_text(format!("Category {}", p.category));
                            egui::ComboBox::from_id_salt(("alt_role", g, *i)).selected_text(role.as_str()).show_ui(ui, |ui| {
                                for r in ROLES {
                                    ui.selectable_value(role, r.to_string(), *r);
                                }
                            });
                            ui.end_row();
                        }
                    });
                    if ui.button("Confirm group").clicked() {
                        let members: Vec<_> = group.iter().filter(|m| m.2).map(|(i, role, _)| self.parts[*i].as_alternative(role)).collect();
                        cx.log.push(match confirm(&self.root, &members) {
                            Ok((first, written)) => format!("Grouped {} parts with {first} preferred; wrote alternatives into {written} symbols", members.len()),
                            Err(e) => format!("Could not group the parts: {e:#}"),
                        });
                    }
                }
            });
        });
        open
    }
}

/// Stores the group, then writes it into the symbols. (preferred MPN, symbols written)
fn confirm(root: &LibraryRoot, members: &[klm_core::store::Alternative]) -> anyhow::Result<(String, usize)> {
    let store = Store::open(&Store::default_path().ok_or_else(|| anyhow::anyhow!("no config folder for the parts database"))?)?;
    let group = alternatives::confirm(&store, members)?;
    Ok((group[0].mpn.clone(), alternatives::write_back(root, &group)?))
}
