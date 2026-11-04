//! Editing a part's metadata after import, and moving it between category libraries.

use super::fields::{merge_symbol, rename_symbol, same_geometry};
use super::{LibraryRoot, copy_file, property, sanitize, set_property};
use crate::kicad_env::write_atomic;
use crate::rules::Rules;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Symbol fields [`PartEdit`] may change.
pub const EDIT_FIELDS: &[&str] = &["MPN", "Manufacturer", "Description", "Datasheet", "LCSC", "DigiKey", "Mouser", "Tags"];

/// What to change on a part; anything not listed stays as it is.
#[derive(Clone, Debug, Default)]
pub struct PartEdit {
    /// Move the part to this category's library.
    pub category: Option<String>,
    /// (field, new value). An empty value clears the field; a new MPN also renames the symbol.
    pub fields: Vec<(String, String)>,
}

/// Where the part is after [`LibraryRoot::edit_part`].
#[derive(Debug)]
pub struct Edited {
    pub name: String,
    pub category: String,
    /// The part moved into a category library that KiCad's tables didn't list yet.
    pub new_lib: bool,
}

impl LibraryRoot {
    /// The editable fields of a part as they are now (MPN falls back to the symbol name).
    pub fn part_fields(&self, category: &str, symbol_name: &str) -> Result<Vec<(&'static str, String)>> {
        let (lib, idx) = self.load_symbol(&sanitize(category), symbol_name)?;
        let symbol = &lib.items()[idx];
        Ok(EDIT_FIELDS
            .iter()
            .map(|&k| {
                let v = property(symbol, k).unwrap_or("").trim();
                (k, if k == "MPN" && v.is_empty() { symbol_name } else { v }.to_string())
            })
            .collect())
    }

    /// Applies `edit` to one part. A category change moves the symbol, its footprint and 3D model to the
    /// new category's library and links it in KiCad; the old library stays, even if now empty. Refuses to
    /// replace a part of the same name.
    pub fn edit_part(&self, category: &str, symbol_name: &str, edit: &PartEdit) -> Result<Edited> {
        self.edit_part_with(category, symbol_name, edit, &Rules::default())
    }

    /// [`edit_part`](Self::edit_part) naming the symbol by the user's template when the MPN or manufacturer
    /// changes; the MPN field itself is never rewritten by the template.
    pub fn edit_part_with(&self, category: &str, symbol_name: &str, edit: &PartEdit, rules: &Rules) -> Result<Edited> {
        let cat = sanitize(category);
        let (mut lib, idx) = self.load_symbol(&cat, symbol_name)?;
        let mut symbol = lib.items()[idx].clone();
        let new_cat = edit.category.as_deref().map_or_else(|| cat.clone(), sanitize);
        if new_cat.is_empty() {
            bail!("category must not be empty");
        }

        for (key, value) in &edit.fields {
            if !EDIT_FIELDS.contains(&key.as_str()) {
                bail!("{key} can't be edited");
            }
            set_property(&mut symbol, key, value.trim());
        }
        let renames = edit.fields.iter().any(|(k, v)| (k == "MPN" && !sanitize(v).is_empty()) || (k == "Manufacturer" && !rules.name_template.trim().is_empty()));
        let name = if renames {
            let mpn = property(&symbol, "MPN").map(str::trim).filter(|m| !m.is_empty()).unwrap_or(symbol_name).to_string();
            let maker = property(&symbol, "Manufacturer").unwrap_or("").to_string();
            Some(sanitize(&rules.name(&[("MPN", &mpn), ("Manufacturer", &maker), ("Category", &new_cat)]))).filter(|n| !n.is_empty())
        } else {
            None
        }
        .unwrap_or_else(|| symbol_name.to_string());
        if name != symbol_name {
            rename_symbol(&mut symbol, &name);
            if property(&symbol, "Value") == Some(symbol_name) {
                set_property(&mut symbol, "Value", &name);
            }
        }
        if (new_cat != cat || name != symbol_name) && self.has_part(&new_cat, &name) {
            bail!("{name} is already in {}", self.nickname(&new_cat));
        }

        let _snapshot = crate::snapshot::guard(self, &if new_cat == cat { format!("edit {symbol_name}") } else { format!("move {symbol_name} to {new_cat}") })?;
        if new_cat == cat {
            lib.items_mut()[idx] = symbol;
            write_atomic(&self.symbol_lib(&cat), &sexpr::to_string(&lib))?;
            return Ok(Edited { name, category: cat, new_lib: false });
        }

        // Footprint and 3D model follow the symbol, unless the footprint comes from another library.
        let old_field = property(&symbol, "Footprint").unwrap_or("").to_string();
        let mut new_files = vec![];
        let mut old_files = vec![];
        let mut footprint = None;
        if let Some(fp_name) = old_field.strip_prefix(&format!("{}:", self.nickname(&cat))) {
            let src = self.footprint_file(&cat, fp_name);
            let fp = sexpr::parse(&std::fs::read_to_string(&src).with_context(|| format!("reading {}", src.display()))?)?;
            let (mut new_fp, mut moved) = (fp_name.to_string(), self.moved_footprint(&fp, &cat, &new_cat, fp_name));
            // Same name but different geometry from another part → keep both, like an import does.
            let existing = self.footprint_file(&new_cat, fp_name);
            if existing.exists() && !same_geometry(&std::fs::read_to_string(&existing)?, &moved.0) {
                new_fp = format!("{fp_name}_{name}");
                moved = self.moved_footprint(&fp, &cat, &new_cat, &new_fp);
            }
            let (text, models) = moved;
            set_property(&mut symbol, "Footprint", &format!("{}:{new_fp}", self.nickname(&new_cat)));
            old_files.push(src);
            old_files.extend(models.iter().map(|(from, _)| from.clone()));
            new_files = models;
            footprint = Some((self.footprint_file(&new_cat, &new_fp), text));
        }

        self.ensure_layout()?;
        std::fs::create_dir_all(self.dir.join("footprints").join(format!("{new_cat}.pretty")))?;
        for (from, to) in new_files.iter().filter(|(from, _)| from.is_file()) {
            copy_file(from, to)?;
        }
        if let Some((dest, text)) = footprint {
            write_atomic(&dest, &text)?;
        }
        merge_symbol(&self.symbol_lib(&new_cat), &lib, symbol)?;
        lib.items_mut().remove(idx);
        write_atomic(&self.symbol_lib(&cat), &sexpr::to_string(&lib))?;
        // A footprint another symbol still uses stays where it is.
        if !lib.children("symbol").any(|s| property(s, "Footprint") == Some(old_field.as_str())) {
            for f in old_files {
                let _ = std::fs::remove_file(f);
            }
        }
        Ok(Edited { name, category: new_cat.clone(), new_lib: self.link(&new_cat)? })
    }

    /// `fp` as it reads in `new_cat`'s library under `name`, with the 3D models it keeps in this library
    /// repointed there; also the (old file, new file) pairs to copy.
    fn moved_footprint(&self, fp: &Sexpr, cat: &str, new_cat: &str, name: &str) -> (String, Vec<(PathBuf, PathBuf)>) {
        let (var, old_dir) = (self.var(), format!("3dmodels/{cat}.3dshapes"));
        let mut fp = fp.clone();
        fp.set_arg(0, Sexpr::str(name));
        let mut files = vec![];
        for model in fp.items_mut().iter_mut().filter(|c| c.head() == Some("model")) {
            let Some(file) = model.arg(0).and_then(|m| m.strip_prefix(&format!("{var}/{old_dir}/"))).map(String::from) else { continue };
            let ext = Path::new(&file).extension().and_then(|e| e.to_str()).unwrap_or("step");
            let rel = format!("3dmodels/{new_cat}.3dshapes/{name}.{ext}");
            files.push((self.dir.join(&old_dir).join(&file), self.dir.join(&rel)));
            model.set_arg(0, Sexpr::str(&format!("{var}/{rel}")));
        }
        (sexpr::to_string(&fp), files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Category `ICs` with OLD (two units) and OTHER sharing footprint FP1, and SOLO with FP2 and a model.
    fn fixture(name: &str) -> LibraryRoot {
        let dir = crate::test_dir(name);
        let root = LibraryRoot::global(dir.join("lib"), dir.join("config"));
        root.ensure_layout().unwrap();
        std::fs::create_dir_all(&root.table_dir).unwrap();
        let g = root.var();
        let part = |name: &str, fp: &str, units: usize| {
            let units: String = (1..=units).map(|u| format!(r#"(symbol "{name}_{u}_1")"#)).collect();
            format!(
                r#"(symbol "{name}" (property "Value" "{name}" (at 0 0 0)) (property "Footprint" "KLM_ICs:{fp}" (at 0 0 0)) (property "Manufacturer" "ACME" (at 0 0 0)) (property "Description" "desc" (at 0 0 0)) {units})"#
            )
        };
        let sym = format!(
            r#"(kicad_symbol_lib (version 20251024) (generator "test") {} {} {})"#,
            part("OLD", "FP1", 2),
            part("OTHER", "FP1", 1),
            part("SOLO", "FP2", 1)
        );
        std::fs::write(root.symbol_lib("ICs"), sym).unwrap();
        let pretty = root.dir.join("footprints/ICs.pretty");
        std::fs::create_dir_all(&pretty).unwrap();
        std::fs::write(pretty.join("FP1.kicad_mod"), r#"(footprint "FP1" (pad "1" smd rect (at 0 0) (size 1 1)))"#).unwrap();
        let fp2 = format!(r#"(footprint "FP2" (pad "1" smd rect (at 0 0) (size 2 2)) (model "{g}/3dmodels/ICs.3dshapes/FP2.step" (offset (xyz 0 0 1))))"#);
        std::fs::write(pretty.join("FP2.kicad_mod"), fp2).unwrap();
        std::fs::create_dir_all(root.dir.join("3dmodels/ICs.3dshapes")).unwrap();
        std::fs::write(root.dir.join("3dmodels/ICs.3dshapes/FP2.step"), "STEP").unwrap();
        root
    }

    fn edit(fields: &[(&str, &str)], category: Option<&str>) -> PartEdit {
        PartEdit { category: category.map(String::from), fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    fn names(root: &LibraryRoot, cat: &str) -> Vec<String> {
        let lib = sexpr::parse(&std::fs::read_to_string(root.symbol_lib(cat)).unwrap()).unwrap();
        lib.children("symbol").filter_map(|s| s.arg(0).map(String::from)).collect()
    }

    fn field(root: &LibraryRoot, cat: &str, name: &str, key: &str) -> Option<String> {
        let (lib, idx) = root.load_symbol(cat, name).unwrap();
        property(&lib.items()[idx], key).map(String::from)
    }

    #[test]
    fn edits_and_clears_fields() {
        let root = fixture("edit_fields");
        let e = edit(&[("Manufacturer", " Globex "), ("Description", ""), ("Tags", "ldo,3v3"), ("LCSC", "C42")], None);
        let done = root.edit_part("ICs", "SOLO", &e).unwrap();
        assert_eq!((done.name.as_str(), done.category.as_str(), done.new_lib), ("SOLO", "ICs", false));
        let get = |k| field(&root, "ICs", "SOLO", k);
        assert_eq!(get("Manufacturer").as_deref(), Some("Globex"));
        assert_eq!(get("Description").as_deref(), Some(""));
        assert_eq!((get("Tags").as_deref(), get("LCSC").as_deref()), (Some("ldo,3v3"), Some("C42")));
        assert_eq!(field(&root, "ICs", "OLD", "Manufacturer").as_deref(), Some("ACME"), "other parts untouched");
        assert!(root.edit_part("ICs", "SOLO", &edit(&[("Footprint", "x")], None)).is_err(), "only the listed fields");
        let fields = root.part_fields("ICs", "SOLO").unwrap();
        assert_eq!(fields[0], ("MPN", "SOLO".to_string()));
        assert_eq!(fields[1], ("Manufacturer", "Globex".to_string()));
    }

    #[test]
    fn mpn_renames_symbol_and_units() {
        let root = fixture("edit_rename");
        let done = root.edit_part("ICs", "OLD", &edit(&[("MPN", "NEW/2")], None)).unwrap();
        assert_eq!(done.name, "NEW_2");
        assert_eq!(names(&root, "ICs"), ["NEW_2", "OTHER", "SOLO"], "renamed in place");
        let (lib, idx) = root.load_symbol("ICs", "NEW_2").unwrap();
        let units: Vec<_> = lib.items()[idx].children("symbol").filter_map(|u| u.arg(0)).collect();
        assert_eq!(units, ["NEW_2_1_1", "NEW_2_2_1"]);
        assert_eq!(field(&root, "ICs", "NEW_2", "MPN").as_deref(), Some("NEW/2"));
        assert_eq!(field(&root, "ICs", "NEW_2", "Value").as_deref(), Some("NEW_2"));
        let err = root.edit_part("ICs", "NEW_2", &edit(&[("MPN", "OTHER")], None)).unwrap_err().to_string();
        assert!(err.contains("already in KLM_ICs"), "{err}");
    }

    #[test]
    fn name_template_names_the_symbol_but_not_the_mpn() {
        let root = fixture("edit_template");
        let rules = Rules { name_template: "{Manufacturer}_{MPN}".into(), ..Rules::default() };
        let done = root.edit_part_with("ICs", "OLD", &edit(&[("MPN", "NEW-1")], None), &rules).unwrap();
        assert_eq!(done.name, "ACME_NEW-1");
        assert_eq!(field(&root, "ICs", "ACME_NEW-1", "MPN").as_deref(), Some("NEW-1"), "MPN field is not rewritten");
        // A manufacturer edit re-names with the template; without a template it leaves the name alone.
        let done = root.edit_part_with("ICs", "SOLO", &edit(&[("Manufacturer", "Globex")], None), &rules).unwrap();
        assert_eq!(done.name, "Globex_SOLO");
        assert_eq!(root.edit_part("ICs", "OTHER", &edit(&[("Manufacturer", "Globex")], None)).unwrap().name, "OTHER");
    }

    #[test]
    fn category_move_takes_footprint_and_model() {
        let root = fixture("edit_move");
        let g = root.var();
        let done = root.edit_part("ICs", "SOLO", &edit(&[], Some("Power"))).unwrap();
        assert_eq!((done.category.as_str(), done.new_lib), ("Power", true));
        assert!(!root.needs_link("Power"));
        assert_eq!(names(&root, "Power"), ["SOLO"]);
        assert_eq!(field(&root, "Power", "SOLO", "Footprint").as_deref(), Some("KLM_Power:FP2"));
        let fp = sexpr::parse(&std::fs::read_to_string(root.footprint_file("Power", "FP2")).unwrap()).unwrap();
        let model = fp.child("model").unwrap();
        assert_eq!(model.arg(0), Some(format!("{g}/3dmodels/Power.3dshapes/FP2.step").as_str()));
        assert!(sexpr::to_string(model).contains("xyz 0 0 1"));
        assert!(root.dir.join("3dmodels/Power.3dshapes/FP2.step").is_file());
        assert!(!root.footprint_file("ICs", "FP2").exists() && !root.dir.join("3dmodels/ICs.3dshapes/FP2.step").exists());
        assert_eq!(names(&root, "ICs"), ["OLD", "OTHER"]);

        // A footprint another part still uses is copied, not taken away.
        root.edit_part("ICs", "OLD", &edit(&[], Some("Power"))).unwrap();
        assert!(root.footprint_file("ICs", "FP1").exists() && root.footprint_file("Power", "FP1").exists());
        // The last user takes the (identical) copy already there and leaves the old library valid but empty.
        root.edit_part("ICs", "OTHER", &edit(&[], Some("Power"))).unwrap();
        assert!(!root.footprint_file("ICs", "FP1").exists());
        assert_eq!(field(&root, "Power", "OTHER", "Footprint").as_deref(), Some("KLM_Power:FP1"));
        assert!(names(&root, "ICs").is_empty());
        assert_eq!(root.list_parts().len(), 3);
    }

    #[test]
    fn footprint_with_same_name_but_other_geometry_is_kept() {
        let root = fixture("edit_fp_clash");
        std::fs::create_dir_all(root.dir.join("footprints/Power.pretty")).unwrap();
        std::fs::write(root.footprint_file("Power", "FP2"), r#"(footprint "FP2" (pad "1" smd rect (at 0 0) (size 9 9)))"#).unwrap();
        root.edit_part("ICs", "SOLO", &edit(&[], Some("Power"))).unwrap();
        assert_eq!(field(&root, "Power", "SOLO", "Footprint").as_deref(), Some("KLM_Power:FP2_SOLO"));
        assert!(std::fs::read_to_string(root.footprint_file("Power", "FP2")).unwrap().contains("size 9 9"), "other part's footprint intact");
        assert!(root.footprint_file("Power", "FP2_SOLO").exists() && root.dir.join("3dmodels/Power.3dshapes/FP2_SOLO.step").is_file());
    }

    #[test]
    fn refuses_to_overwrite_a_part() {
        let root = fixture("edit_refuse");
        root.edit_part("ICs", "OTHER", &edit(&[], Some("Power"))).unwrap();
        // Another OTHER in ICs would clash in Power.
        std::fs::write(root.symbol_lib("ICs"), std::fs::read_to_string(root.symbol_lib("Power")).unwrap()).unwrap();
        let before = std::fs::read_to_string(root.symbol_lib("ICs")).unwrap();
        let err = root.edit_part("ICs", "OTHER", &edit(&[("Manufacturer", "X")], Some("Power"))).unwrap_err().to_string();
        assert!(err.contains("OTHER is already in KLM_Power"), "{err}");
        assert_eq!(std::fs::read_to_string(root.symbol_lib("ICs")).unwrap(), before, "nothing written after a refusal");
        assert_eq!(field(&root, "Power", "OTHER", "Manufacturer").as_deref(), Some("ACME"));
    }

    #[test]
    fn bulk_edit_applies_one_change_to_each_part() {
        let root = fixture("edit_bulk");
        let e = edit(&[("Manufacturer", "Globex")], Some("Misc"));
        for name in ["OLD", "OTHER", "SOLO"] {
            root.edit_part("ICs", name, &e).unwrap();
        }
        assert_eq!(names(&root, "Misc"), ["OLD", "OTHER", "SOLO"]);
        for name in ["OLD", "OTHER", "SOLO"] {
            assert_eq!(field(&root, "Misc", name, "Manufacturer").as_deref(), Some("Globex"));
        }
        assert!(root.list_parts().iter().all(|p| p.category == "Misc" && p.footprint == super::super::Check::Ok));
    }
}
