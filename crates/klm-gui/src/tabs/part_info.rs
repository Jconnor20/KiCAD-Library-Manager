//! Settings › Standards › Part information: edits one class's schema (`fields::ClassInfo`) in the active profile.
//! A class is copied from its built-in default into the profile the first time it is changed.

use super::ui_kit::{Level, badge};
use eframe::egui::{self, Ui};
use klm_core::fields::{ClassInfo, FieldDef, default_class, render};
use klm_core::standardise::profile::Profile;

pub const CLASSES: [&str; 8] = ["Resistor", "Capacitor", "Inductor", "Diode", "Led", "Transistor", "Ic", "Generic"];
const UNITS: [&str; 7] = ["F", "Ω", "H", "V", "A", "W", "%"];

/// "a, b ,, c" → ["a", "b", "c"].
fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|v| !v.is_empty()).map(String::from).collect()
}

/// A value for the preview: an allowed one, else an example in the field's unit, else the field's name.
fn sample(fd: &FieldDef) -> String {
    if let Some(a) = fd.allowed.first() {
        return a.clone();
    }
    match (fd.name.as_str(), fd.unit.as_deref()) {
        ("Manufacturer", _) => "Acme".into(),
        ("MPN", _) => "ABC-123".into(),
        ("Package", _) => "0603".into(),
        (_, Some("F")) => "100nF".into(),
        (_, Some("Ω")) => "10kΩ".into(),
        (_, Some("H")) => "4.7uH".into(),
        (_, Some("V")) => "16V".into(),
        (_, Some("A")) => "1A".into(),
        (_, Some("W")) => "100mW".into(),
        (_, Some("%")) => "5%".into(),
        (n, _) => n.into(),
    }
}

/// Moves item `i` by `by` places; false when it would leave the list.
fn shift(v: &mut [FieldDef], i: usize, by: isize) -> bool {
    match i.checked_add_signed(by).filter(|j| *j < v.len()) {
        Some(j) => {
            v.swap(i, j);
            true
        }
        None => false,
    }
}

/// A comma-separated box for a list. The text is kept in egui memory so a trailing comma survives until the next item is typed.
fn list_edit(ui: &mut Ui, id: egui::Id, list: &mut Vec<String>, hint: &str, width: f32) {
    let mut text = ui.data_mut(|d| d.get_temp::<String>(id)).filter(|t| split_list(t) == *list).unwrap_or_else(|| list.join(", "));
    if ui.add(egui::TextEdit::singleline(&mut text).hint_text(hint).desired_width(width)).changed() {
        *list = split_list(&text);
    }
    ui.data_mut(|d| d.insert_temp(id, text));
}

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

pub fn page(ui: &mut Ui, p: &mut Profile, class: &mut usize) {
    let key = CLASSES[*class];
    ui.horizontal_wrapped(|ui| {
        for (i, c) in CLASSES.iter().enumerate() {
            if ui.selectable_label(*class == i, *c).clicked() {
                *class = i;
            }
        }
    });
    let custom = p.classes.contains_key(key);
    let mut info = p.class(key);
    let before = info.clone();
    ui.horizontal(|ui| {
        badge(ui, if custom { "Customised in this profile" } else { "Built-in default" }, if custom { Level::Info } else { Level::Ok });
        if ui.add_enabled(custom, egui::Button::new("Reset class to default")).clicked() {
            p.classes.remove(key);
            info = default_class(key);
        }
    });
    group(ui, "Fields", |ui| fields(ui, key, &mut info));
    group(ui, "Templates", |ui| templates(ui, &mut info));
    // Editing back to the built-in leaves no override behind.
    if info != before {
        if info == default_class(key) {
            p.classes.remove(key);
        } else {
            p.classes.insert(key.into(), info);
        }
    }
}

fn fields(ui: &mut Ui, key: &str, info: &mut ClassInfo) {
    ui.weak("Aliases are the names a source uses for the field's parameter (\"Voltage - Rated\"); they fill the field on import. Allowed values: leave empty for any.");
    let (mut remove, mut up, mut down, mut renamed) = (None, None, None, None);
    let n = info.fields.len();
    for (i, fd) in info.fields.iter_mut().enumerate() {
        ui.push_id((key, i), |ui| {
            ui.horizontal(|ui| {
                let was = fd.name.clone();
                if ui.add(egui::TextEdit::singleline(&mut fd.name).hint_text("name").desired_width(110.0)).changed() {
                    renamed = Some((was, fd.name.clone()));
                }
                ui.checkbox(&mut fd.required, "required");
                ui.checkbox(&mut fd.visible, "visible");
                egui::ComboBox::from_id_salt("unit").width(50.0).selected_text(fd.unit.as_deref().unwrap_or("text")).show_ui(ui, |ui| {
                    ui.selectable_value(&mut fd.unit, None, "text");
                    for u in UNITS {
                        ui.selectable_value(&mut fd.unit, Some(u.into()), u);
                    }
                });
                if ui.add_enabled(i > 0, egui::Button::new("▲").small()).on_hover_text("Move up").clicked() {
                    up = Some(i);
                }
                if ui.add_enabled(i + 1 < n, egui::Button::new("▼").small()).on_hover_text("Move down").clicked() {
                    down = Some(i);
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let w = ((ui.available_width() - 150.0) / 2.0).max(80.0);
                ui.label("allowed");
                list_edit(ui, ui.make_persistent_id("allowed"), &mut fd.allowed, "any value", w);
                ui.label("aliases");
                list_edit(ui, ui.make_persistent_id("aliases"), &mut fd.aliases, "source parameter names", w);
            });
        });
    }
    if let Some((was, now)) = renamed {
        rename_placeholder(info, &was, &now);
    }
    if let Some(i) = up {
        shift(&mut info.fields, i, -1);
    } else if let Some(i) = down {
        shift(&mut info.fields, i, 1);
    } else if let Some(i) = remove {
        info.fields.remove(i);
    }
    if ui.button("Add field").clicked() {
        info.fields.push(FieldDef { name: "New field".into(), ..Default::default() });
    }
}

/// Keeps the templates pointing at a renamed field. Left alone while another field still has the old
/// name, so typing through a name another field uses doesn't take over its placeholders.
fn rename_placeholder(info: &mut ClassInfo, was: &str, now: &str) {
    if info.fields.iter().any(|f| f.name == was) {
        return;
    }
    let (was, now) = (format!("{{{was}}}"), format!("{{{now}}}"));
    for t in [&mut info.description, &mut info.value, &mut info.keywords] {
        *t = t.replace(&was, &now);
    }
}

fn templates(ui: &mut Ui, info: &mut ClassInfo) {
    let names: Vec<String> = info.fields.iter().map(|f| f.name.clone()).collect();
    ui.weak(format!("Placeholders: {}. An empty template leaves that property alone. The preview uses a sample part.", names.iter().map(|n| format!("{{{n}}}")).collect::<Vec<_>>().join(" ")));
    let samples: Vec<(String, String)> = info.fields.iter().map(|f| (f.name.clone(), sample(f))).collect();
    let get = |n: &str| samples.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    for (label, t) in [("Description", &mut info.description), ("Value", &mut info.value), ("Keywords", &mut info.keywords)] {
        ui.horizontal(|ui| {
            ui.add_sized([80.0, 20.0], egui::Label::new(label));
            ui.add(egui::TextEdit::singleline(t).desired_width(f32::INFINITY));
        });
        ui.horizontal(|ui| {
            ui.add_space(88.0);
            ui.weak(format!("Preview: {}", render(t, &get)));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_reordering() {
        assert_eq!(split_list("a, b ,, c"), ["a", "b", "c"]);
        assert!(split_list(" , ").is_empty());
        let mut v: Vec<FieldDef> = ["A", "B", "C"].map(|n| FieldDef { name: n.into(), ..Default::default() }).into();
        assert!(shift(&mut v, 0, 1) && !shift(&mut v, 0, -1) && !shift(&mut v, 2, 1));
        assert_eq!(v.iter().map(|f| f.name.as_str()).collect::<String>(), "BAC");
    }

    #[test]
    fn renaming_a_field_follows_into_templates() {
        let mut info = ClassInfo { description: "Cap {Voltage} {V}".into(), fields: vec![FieldDef { name: "Volts".into(), ..Default::default() }, FieldDef { name: "V".into(), ..Default::default() }], ..Default::default() };
        rename_placeholder(&mut info, "Voltage", "Volts");
        assert_eq!(info.description, "Cap {Volts} {V}");
        // "V" is still another field's name, so its placeholder stays.
        rename_placeholder(&mut info, "V", "Vx");
        assert_eq!(info.description, "Cap {Volts} {V}");
    }

    #[test]
    fn preview_samples_follow_unit_and_allowed() {
        let f = |name: &str, unit: Option<&str>, allowed: &[&str]| FieldDef { name: name.into(), unit: unit.map(Into::into), allowed: allowed.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        assert_eq!(sample(&f("Capacitance", Some("F"), &[])), "100nF");
        assert_eq!(sample(&f("Polarity", None, &["NPN", "PNP"])), "NPN");
        assert_eq!(sample(&f("Whatever", None, &[])), "Whatever");
    }
}
