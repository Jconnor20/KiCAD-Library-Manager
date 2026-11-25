//! Attaching files to a part and copying it between libraries, checked by asking the real
//! `kicad-cli` to render the result. Skipped when KiCad isn't installed.

use klm_core::library::{AttachKind, Check, LibraryRoot, property};
use klm_core::part::Part;
use klm_core::{kicad_env, sexpr};
use std::path::{Path, PathBuf};
use std::process::Command;

const SYMBOL_V6: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "TESTPART" (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "TESTPART" (id 1) (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Vendor:SOT-23-ACME" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "" (id 3) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "TESTPART_0_1" (rectangle (start -2.54 1.27) (end 2.54 -1.27) (stroke (width 0.254)) (fill (type background))))
    (symbol "TESTPART_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
)"#;

const FOOTPRINT_V5: &str = r#"(module SOT-23-ACME (layer F.Cu) (tedit 5A02FF57)
  (fp_text reference REF** (at 0 -2.5) (layer F.SilkS) (effects (font (size 1 1) (thickness 0.15))))
  (fp_text value SOT-23-ACME (at 0 2.5) (layer F.Fab) (effects (font (size 1 1) (thickness 0.15))))
  (pad 1 smd rect (at -1 0.95) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask))
  (model ${OLDVENDOR}/sot23.wrl (offset (xyz 0 0 0.5)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 90)))
)"#;

fn kicad_available() -> bool {
    kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success()))
}

fn fresh(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("klm-e2e-tools").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read(p: &Path) -> sexpr::Sexpr {
    sexpr::parse(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn field_in(root: &LibraryRoot, cat: &str, key: &str) -> Option<String> {
    let lib = read(&root.dir.join(format!("symbols/{cat}.kicad_sym")));
    property(lib.child("symbol").unwrap(), key).map(String::from)
}

fn field(root: &LibraryRoot, key: &str) -> Option<String> {
    field_in(root, "ICs", key)
}

/// A part shaped like the one in the report: digits and dashes in the MPN, a space in the maker.
fn switch_part(src: &Path) -> Part {
    std::fs::write(src.join("t.kicad_sym"), SYMBOL_V6).unwrap();
    std::fs::write(src.join("fp.kicad_mod"), FOOTPRINT_V5).unwrap();
    Part {
        mpn: "2-1977223-4".into(),
        manufacturer: "TE Connectivity".into(),
        symbol_file: Some(src.join("t.kicad_sym")),
        footprint_files: vec![src.join("fp.kicad_mod")],
        ..Default::default()
    }
}

fn renders(cli: &Path, root: &LibraryRoot) {
    let out = fresh("svg");
    for (kind, lib) in [("sym", "symbols/ICs.kicad_sym"), ("fp", "footprints/ICs.pretty")] {
        let r = Command::new(cli).args([kind, "export", "svg", "-o"]).arg(&out).arg(root.dir.join(lib)).output().unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    }
    assert!(std::fs::read_dir(&out).unwrap().count() >= 2);
}

#[test]
fn attach_and_copy_to_project() {
    if !kicad_available() {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let src = fresh("src");
    std::fs::write(src.join("t.kicad_sym"), SYMBOL_V6).unwrap();
    std::fs::write(src.join("fp.kicad_mod"), FOOTPRINT_V5).unwrap();
    // A part with only a symbol and footprint; everything else gets attached below.
    let part = Part {
        mpn: "TP-100".into(),
        manufacturer: "ACME".into(),
        symbol_file: Some(src.join("t.kicad_sym")),
        footprint_files: vec![src.join("fp.kicad_mod")],
        ..Default::default()
    };
    let global = LibraryRoot::global(fresh("global"), fresh("config"));
    global.add_part(&part, "ICs", false).unwrap();
    let g = global.dir.to_string_lossy().replace('\\', "/");
    let cli = kicad_env::kicad_cli().unwrap();

    // Each kind lands in its folder under the standard name and sets the right field.
    std::fs::write(src.join("any.pdf"), "%PDF").unwrap();
    std::fs::write(src.join("m.cir"), "* x\n.SUBCKT TESTMODEL 1 2\n.ENDS\n").unwrap();
    std::fs::write(src.join("body.step"), "ISO-10303-21;").unwrap();
    for f in ["any.pdf", "m.cir", "body.step"] {
        let path = src.join(f);
        global.attach_file("ICs", "TP-100", AttachKind::of(&path).unwrap(), &path).unwrap();
    }
    assert_eq!(field(&global, "Datasheet").unwrap(), format!("{g}/datasheets/ACME_TP-100.pdf"));
    assert_eq!(field(&global, "Sim.Library").unwrap(), format!("{g}/simulation/TP-100.cir"));
    assert_eq!((field(&global, "Sim.Name").as_deref(), field(&global, "Sim.Device").as_deref()), (Some("TESTMODEL"), Some("SUBCKT")));
    let fp_path = global.dir.join("footprints/ICs.pretty/SOT-23-ACME.kicad_mod");
    let model = read(&fp_path);
    let model = model.child("model").unwrap();
    assert_eq!(model.arg(0), Some(format!("{g}/3dmodels/ICs.3dshapes/SOT-23-ACME.step").as_str()));
    assert!(sexpr::to_string(model).contains("xyz 0 0 90"), "existing rotation kept");
    assert!(global.dir.join("3dmodels/ICs.3dshapes/SOT-23-ACME.step").is_file());
    let p = &global.list_parts()[0];
    assert_eq!([p.footprint, p.model, p.datasheet, p.sim], [Check::Ok; 4]);
    assert_eq!(p.datasheet_target.as_deref(), Some(format!("{g}/datasheets/ACME_TP-100.pdf").as_str()));

    // A model in another format replaces the old file.
    std::fs::write(src.join("body.wrl"), "#VRML V2.0").unwrap();
    global.attach_file("ICs", "TP-100", AttachKind::Model, &src.join("body.wrl")).unwrap();
    assert!(global.dir.join("3dmodels/ICs.3dshapes/SOT-23-ACME.wrl").is_file());
    assert!(!global.dir.join("3dmodels/ICs.3dshapes/SOT-23-ACME.step").exists());
    assert_eq!(global.list_parts()[0].model, Check::Ok);
    global.attach_file("ICs", "TP-100", AttachKind::Model, &src.join("body.step")).unwrap();

    // Wrong kind is refused; re-attaching the file already in place doesn't empty it.
    assert!(global.attach_file("ICs", "TP-100", AttachKind::Model, &src.join("any.pdf")).is_err());
    assert!(global.attach_file("ICs", "NOPE", AttachKind::Datasheet, &src.join("any.pdf")).is_err());
    let ds = global.dir.join("datasheets/ACME_TP-100.pdf");
    global.attach_file("ICs", "TP-100", AttachKind::Datasheet, &ds).unwrap();
    assert_eq!(std::fs::read_to_string(&ds).unwrap(), "%PDF");
    renders(&cli, &global);

    // Copy to a project: paths follow, nickname switches, KiCad loads the copy.
    let project_dir = fresh("project");
    let project = LibraryRoot::project(&project_dir);
    assert!(global.copy_part_to("ICs", "TP-100", &project, false).unwrap(), "new library gets linked");
    assert!(!project.needs_link("ICs"));
    assert_eq!(field(&project, "Footprint").unwrap(), "PRJ_ICs:SOT-23-ACME");
    assert_eq!(field(&project, "Datasheet").unwrap(), "${KIPRJMOD}/lib/datasheets/ACME_TP-100.pdf");
    assert_eq!(field(&project, "Sim.Library").unwrap(), "${KIPRJMOD}/lib/simulation/TP-100.cir");
    let pfp = read(&project.dir.join("footprints/ICs.pretty/SOT-23-ACME.kicad_mod"));
    assert_eq!(pfp.child("model").unwrap().arg(0), Some("${KIPRJMOD}/lib/3dmodels/ICs.3dshapes/SOT-23-ACME.step"));
    let p = &project.list_parts()[0];
    assert_eq!([p.footprint, p.model, p.datasheet, p.sim], [Check::Ok; 4]);
    assert!(p.in_kicad);
    assert!(!std::fs::read_to_string(project_dir.join("sym-lib-table")).unwrap().contains(&g));
    renders(&cli, &project);
    // The original is untouched.
    assert_eq!(field(&global, "Footprint").unwrap(), "KLM_ICs:SOT-23-ACME");

    // A second copy is refused unless asked; asking replaces rather than duplicates.
    let err = global.copy_part_to("ICs", "TP-100", &project, false).unwrap_err().to_string();
    assert!(err.contains("already in PRJ_ICs"), "{err}");
    assert!(!global.copy_part_to("ICs", "TP-100", &project, true).unwrap());
    assert_eq!(read(&project.dir.join("symbols/ICs.kicad_sym")).children("symbol").count(), 1);

    // A different footprint of the same name in the target is not silently replaced.
    let other = LibraryRoot::project(&fresh("project2"));
    other.ensure_layout().unwrap();
    std::fs::create_dir_all(other.dir.join("footprints/ICs.pretty")).unwrap();
    std::fs::write(other.dir.join("footprints/ICs.pretty/SOT-23-ACME.kicad_mod"), "(footprint \"SOT-23-ACME\")").unwrap();
    assert!(global.copy_part_to("ICs", "TP-100", &other, false).unwrap_err().to_string().contains("different footprint"));
    assert!(!other.dir.join("symbols/ICs.kicad_sym").exists(), "nothing written after a refusal");
}

/// The reported case: a datasheet attached to an imported part in the global library must end up in
/// that library's `datasheets/` and in the symbol's `Datasheet` field.
#[test]
fn attach_datasheet_to_imported_switch() {
    if !kicad_available() {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let src = fresh("sw_src");
    let global = LibraryRoot::global(fresh("sw_global"), fresh("sw_config"));
    global.add_part(&switch_part(&src), "Switches", false).unwrap();
    assert_eq!(global.list_parts()[0].datasheet, Check::Missing);

    // A picked file name with spaces and a capital extension, as downloads often have.
    let pdf = src.join("TE 2-1977223-4 datasheet.PDF");
    std::fs::write(&pdf, "%PDF").unwrap();
    let p = global.list_parts().remove(0);
    global.attach_file(&p.category, &p.name, AttachKind::of(&pdf).unwrap(), &pdf).unwrap();

    let g = global.dir.to_string_lossy().replace('\\', "/");
    assert_eq!(field_in(&global, "Switches", "Datasheet").unwrap(), format!("{g}/datasheets/TE_Connectivity_2-1977223-4.pdf"));
    assert_eq!(std::fs::read_to_string(global.dir.join("datasheets/TE_Connectivity_2-1977223-4.pdf")).unwrap(), "%PDF");
    assert_eq!(global.list_parts()[0].datasheet, Check::Ok);
}

/// What the Attach dialog does for each ticked library: copy the part over if it's missing there,
/// then attach, so every file lands in that library's own folders.
#[test]
fn attach_lands_in_each_library() {
    if !kicad_available() {
        eprintln!("kicad-cli not found; skipping");
        return;
    }
    let src = fresh("each_src");
    let global = LibraryRoot::global(fresh("each_global"), fresh("each_config"));
    let project = LibraryRoot::project(&fresh("each_project"));
    global.add_part(&switch_part(&src), "Switches", false).unwrap();
    assert!(!project.has_part("Switches", "2-1977223-4"));
    global.copy_part_to("Switches", "2-1977223-4", &project, false).unwrap();

    for (root, tag) in [(&global, "global"), (&project, "project")] {
        for (file, text) in [
            ("ds.pdf", "%PDF"),
            (
                "m.cir",
                "* x
.SUBCKT M 1 2
.ENDS
",
            ),
            ("body.step", "ISO-10303-21;"),
        ] {
            let path = src.join(format!("{tag}-{file}"));
            std::fs::write(&path, format!("{tag} {text}")).unwrap();
            root.attach_file("Switches", "2-1977223-4", AttachKind::of(&path).unwrap(), &path).unwrap();
        }
    }

    for (root, tag, var) in [(&global, "global", global.dir.to_string_lossy().replace('\\', "/")), (&project, "project", "${KIPRJMOD}/lib".to_string())] {
        assert_eq!(field_in(root, "Switches", "Datasheet").unwrap(), format!("{var}/datasheets/TE_Connectivity_2-1977223-4.pdf"));
        assert_eq!(field_in(root, "Switches", "Sim.Library").unwrap(), format!("{var}/simulation/2-1977223-4.cir"));
        let fp = read(&root.dir.join("footprints/Switches.pretty/SOT-23-ACME.kicad_mod"));
        assert_eq!(fp.child("model").unwrap().arg(0), Some(format!("{var}/3dmodels/Switches.3dshapes/SOT-23-ACME.step").as_str()));
        // Each library keeps its own copy of the file, not a link to the other's.
        for (rel, ends) in [
            ("datasheets/TE_Connectivity_2-1977223-4.pdf", "%PDF"),
            (
                "simulation/2-1977223-4.cir",
                ".ENDS
",
            ),
            ("3dmodels/Switches.3dshapes/SOT-23-ACME.step", "10303-21;"),
        ] {
            let text = std::fs::read_to_string(root.dir.join(rel)).unwrap();
            assert!(text.starts_with(tag) && text.ends_with(ends), "{rel}: {text:?}");
        }
        let p = &root.list_parts()[0];
        assert_eq!([p.footprint, p.model, p.datasheet, p.sim], [Check::Ok; 4]);
    }
}
