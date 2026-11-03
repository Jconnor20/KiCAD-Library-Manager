use eframe::egui;
use klm_core::alternatives::{self, LibPart, ROLES};
use klm_core::library::LibraryRoot;
use klm_core::store::Store;
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
        Self { root: root.clone(), parts: Vec::new(), groups: Vec::new(), rx: Some(rx) }
    }

    /// Returns false once closed.
    pub fn window(&mut self, ctx: &egui::Context, log: &mut Vec<String>) -> bool {
        let mut open = true;
        egui::Window::new("Alternatives and duplicates").open(&mut open).default_width(720.0).show(ctx, |ui| {
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
                        log.push(match confirm(&self.root, &members) {
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
