//! Pictures of a part to review: the symbol and footprint as SVG, and the footprint with its 3D model as
//! a PNG, all drawn by `kicad-cli`.
//!
//! Works on loose files (`Sources::from_part`, before an import) and on a part in a library
//! (`Sources::from_library`). `render` blocks for up to several seconds, so callers run it off the UI
//! thread. Pictures are cached in the temp folder, named by a hash of the files they were drawn from.

use crate::kicad_env::{self, run_cli};
use crate::library::{LibraryRoot, property, sanitize};
use crate::part::Part;
use crate::sexpr::{self, Sexpr};
use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Symbol,
    Footprint,
    /// The footprint on a bare board, with its 3D model.
    Model,
}

/// The files to draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    pub symbol: Option<PathBuf>,
    /// Draws only this symbol of the file; `None` draws the first one found (a vendor file holds one).
    pub symbol_name: Option<String>,
    pub footprint: Option<PathBuf>,
    /// Replaces the model the footprint names; `None` keeps it.
    pub model: Option<PathBuf>,
}

impl Sources {
    /// The downloaded files of a part before import; the footprint's own model path points into the vendor's
    /// folder layout, so the model file that came with it is used instead.
    pub fn from_part(part: &Part) -> Self {
        let model = part.model_files.iter().find(|m| matches!(ext(m).as_str(), "step" | "stp")).or_else(|| part.model_files.first());
        Self { symbol: part.symbol_file.clone(), symbol_name: None, footprint: part.footprint_files.first().cloned(), model: model.cloned() }
    }

    /// A part in `root`; the footprint and model are those the symbol points at, when they are in this library.
    pub fn from_library(root: &LibraryRoot, category: &str, symbol_name: &str) -> Self {
        let symbol = root.dir.join("symbols").join(format!("{}.kicad_sym", sanitize(category)));
        let footprint = std::fs::read_to_string(&symbol)
            .ok()
            .and_then(|t| sexpr::parse(&t).ok())
            .and_then(|lib| lib.children("symbol").find(|s| s.arg(0) == Some(symbol_name)).and_then(|s| property(s, "Footprint").map(String::from)))
            .and_then(|f| Some(f.strip_prefix(&format!("{}:", root.nickname(category)))?.to_string()))
            .map(|name| root.dir.join("footprints").join(format!("{}.pretty", sanitize(category))).join(format!("{name}.kicad_mod")));
        // A path variable other than ${KIPRJMOD} can't be resolved here; kicad-cli resolves its own.
        let model = footprint
            .as_ref()
            .and_then(|f| sexpr::parse(&std::fs::read_to_string(f).ok()?).ok())
            .and_then(|fp| fp.child("model")?.arg(0).map(|m| m.replace("${KIPRJMOD}", &root.table_dir.to_string_lossy())))
            .filter(|m| !m.contains("${"))
            .map(PathBuf::from);
        Self { symbol: Some(symbol), symbol_name: Some(symbol_name.to_string()), footprint, model }
    }
}

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase()
}

/// Where pictures are kept between runs. Never cleaned up (about 50 KB each); the OS clears the temp folder.
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("klm-previews")
}

/// Draws `kind` (or finds it in the cache) and returns the image path: an SVG, or a PNG for `Kind::Model`.
/// The error is a short sentence fit to show the user.
pub fn render(kind: Kind, src: &Sources) -> Result<PathBuf> {
    render_in(&cache_dir(), kind, src)
}

fn render_in(cache: &Path, kind: Kind, src: &Sources) -> Result<PathBuf> {
    let (tag, ext) = match kind {
        Kind::Symbol => ("symbol", "svg"),
        Kind::Footprint => ("footprint", "svg"),
        Kind::Model => ("model", "png"),
    };
    let out = cache.join(format!("{tag}-{:016x}.{ext}", cache_key(kind, src)?));
    if out.exists() {
        return Ok(out);
    }
    std::fs::create_dir_all(cache)?;
    let scratch = scratch_dir(cache)?;
    let drawn = match kind {
        Kind::Symbol => draw_symbol(src, &scratch),
        Kind::Footprint => draw_footprint(src, &scratch),
        Kind::Model => draw_model(src, &scratch),
    };
    // A rename can't leave a half-written picture for a concurrent reader.
    let result = drawn.and_then(|f| std::fs::rename(f, &out).context("could not store the picture"));
    let _ = std::fs::remove_dir_all(&scratch);
    result.map(|_| out)
}

/// A fresh empty folder under `parent`.
pub(crate) fn scratch_dir(parent: &Path) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let dir = parent.join(format!("tmp-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Hash of everything the picture depends on: the contents of the files, so an edited file is redrawn.
/// A model can be megabytes, so it is hashed by size and modification time instead.
fn cache_key(kind: Kind, src: &Sources) -> Result<u64> {
    let read = |p: &Option<PathBuf>, what: &str| -> Result<Vec<u8>> {
        let p = p.as_ref().with_context(|| format!("No {what} in this part"))?;
        std::fs::read(p).with_context(|| format!("The {what} file can't be read"))
    };
    let mut h = std::hash::DefaultHasher::new();
    match kind {
        Kind::Symbol => (read(&src.symbol, "symbol")?, &src.symbol_name).hash(&mut h),
        Kind::Footprint => read(&src.footprint, "footprint")?.hash(&mut h),
        Kind::Model => {
            read(&src.footprint, "footprint")?.hash(&mut h);
            if let Some(m) = &src.model {
                let meta = std::fs::metadata(m).with_context(|| format!("The 3D model {} is missing", m.display()))?;
                (m, meta.len(), meta.modified().ok()).hash(&mut h);
            }
        }
    }
    Ok(h.finish())
}

/// The file in `dir` with this extension, preferring one whose name ends in `prefer`.
fn find_output(dir: &Path, ext: &str, prefer: &str) -> Result<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == ext)).collect();
    files.sort();
    files
        .iter()
        .find(|p| p.to_string_lossy().ends_with(prefer))
        .or(files.first())
        .cloned()
        .context("kicad-cli drew nothing (is the file a valid KiCad file?)")
}

fn draw_symbol(src: &Sources, scratch: &Path) -> Result<PathBuf> {
    let file = src.symbol.as_ref().context("No symbol in this part")?;
    let mut lib = file.clone();
    if ext(file) != "kicad_sym" {
        // Legacy .lib files draw as nothing, so bring them to the current format first.
        lib = scratch.join("sym.kicad_sym");
        kicad_env::upgrade_symbols(file, &lib)?;
    }
    let out = scratch.join("sym");
    std::fs::create_dir_all(&out)?;
    let mut args: Vec<&OsStr> = vec!["sym".as_ref(), "export".as_ref(), "svg".as_ref(), "-o".as_ref(), out.as_os_str()];
    if let Some(name) = &src.symbol_name {
        args.extend([OsStr::new("-s"), OsStr::new(name)]);
    }
    args.push(lib.as_os_str());
    run_cli(&args).map_err(short)?;
    find_output(&out, "svg", "_unit1.svg")
}

/// Layers worth looking at when judging a footprint; the mask layer is left out because it covers the pads.
const LAYERS: &str = "F.Cu,F.Paste,F.SilkS,F.Fab,F.CrtYd";

fn draw_footprint(src: &Sources, scratch: &Path) -> Result<PathBuf> {
    let file = src.footprint.as_ref().context("No footprint in this part")?;
    // `kicad-cli fp export` wants a .pretty folder, and names the footprint after its file.
    let pretty = scratch.join("fp.pretty");
    std::fs::create_dir_all(&pretty)?;
    std::fs::copy(file, pretty.join("fp.kicad_mod"))?;
    let out = scratch.join("svg");
    std::fs::create_dir_all(&out)?;
    let args: [&OsStr; 9] = ["fp".as_ref(), "export".as_ref(), "svg".as_ref(), "--fp".as_ref(), "fp".as_ref(), "-l".as_ref(), LAYERS.as_ref(), "-o".as_ref(), out.as_os_str()];
    run_cli(&[&args[..], &[pretty.as_os_str()]].concat()).map_err(short)?;
    find_output(&out, "svg", "fp.svg")
}

/// What a board file needs to load; the 3D viewer draws the footprint on a bare board without an outline.
const BOARD_HEAD: &str = r#"(kicad_pcb (version 20260206) (generator "klm") (generator_version "10.0")
 (general (thickness 1.6) (legacy_teardrops no)) (paper "A4")
 (layers (0 "F.Cu" signal) (2 "B.Cu" signal) (9 "F.Adhes" user "F.Adhesive") (11 "B.Adhes" user "B.Adhesive") (13 "F.Paste" user) (15 "B.Paste" user)
  (5 "F.SilkS" user "F.Silkscreen") (7 "B.SilkS" user "B.Silkscreen") (1 "F.Mask" user) (3 "B.Mask" user) (17 "Dwgs.User" user "User.Drawings")
  (25 "Edge.Cuts" user) (27 "Margin" user) (31 "F.CrtYd" user "F.Courtyard") (29 "B.CrtYd" user "B.Courtyard") (35 "F.Fab" user) (33 "B.Fab" user))
 (setup (pad_to_mask_clearance 0))
"#;

fn draw_model(src: &Sources, scratch: &Path) -> Result<PathBuf> {
    let file = src.footprint.as_ref().context("No footprint in this part")?;
    // The board loader wants the current footprint format, which `fp upgrade` produces.
    let (fp_in, fp_out) = (scratch.join("in.pretty"), scratch.join("out.pretty"));
    std::fs::create_dir_all(&fp_in)?;
    std::fs::copy(file, fp_in.join("fp.kicad_mod"))?;
    kicad_env::upgrade_footprints(&fp_in, &fp_out).map_err(short)?;
    let mut fp = sexpr::parse(&std::fs::read_to_string(fp_out.join("fp.kicad_mod"))?)?;
    let items = fp.items_mut();
    let layer = items.iter().position(|c| c.head() == Some("layer")).unwrap_or(0);
    items.insert(layer + 1, Sexpr::list("at", vec![Sexpr::Atom("0".into()), Sexpr::Atom("0".into())]));
    match (&src.model, items.iter_mut().find(|c| c.head() == Some("model"))) {
        (Some(m), _) if !m.is_file() => bail!("The 3D model file {} is missing", m.file_name().unwrap_or_default().to_string_lossy()),
        (Some(m), Some(node)) => node.set_arg(0, Sexpr::str(&m.to_string_lossy().replace('\\', "/"))),
        (Some(m), None) => {
            let xyz = |v: &str| Sexpr::list("xyz", vec![Sexpr::Atom(v.into()); 3]);
            let uri = Sexpr::str(&m.to_string_lossy().replace('\\', "/"));
            let [off, scale, rot] = [("offset", "0"), ("scale", "1"), ("rotate", "0")].map(|(k, v)| Sexpr::list(k, vec![xyz(v)]));
            items.push(Sexpr::list("model", vec![uri, off, scale, rot]));
        }
        (None, Some(_)) => {}
        (None, None) => bail!("No 3D model in this part"),
    }
    let board = scratch.join("b.kicad_pcb");
    std::fs::write(&board, format!("{BOARD_HEAD}{})\n", sexpr::to_string(&fp)))?;
    let png = scratch.join("model.png");
    let args: [&OsStr; 15] = [
        "pcb".as_ref(), "render".as_ref(), "-o".as_ref(), png.as_os_str(), "-w".as_ref(), "480".as_ref(), "-h".as_ref(), "360".as_ref(),
        "--rotate".as_ref(), "-45,0,45".as_ref(), "--zoom".as_ref(), "0.8".as_ref(), "--background".as_ref(), "opaque".as_ref(), board.as_os_str(),
    ];
    run_cli(&args).map_err(short)?;
    Ok(png)
}

/// The first line of kicad-cli's complaint; the rest is usually progress noise.
fn short(e: anyhow::Error) -> anyhow::Error {
    let msg = format!("{e:#}");
    let line = msg.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("kicad-cli failed:")).or(msg.lines().next()).unwrap_or("kicad-cli failed");
    anyhow::anyhow!("{line}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const SYMBOL: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "TESTPART" (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Value" "TESTPART" (id 1) (at 0 -2.54 0) (effects (font (size 1.27 1.27))))
    (symbol "TESTPART_0_1" (rectangle (start -2.54 1.27) (end 2.54 -1.27) (stroke (width 0.254)) (fill (type background))))
    (symbol "TESTPART_1_1" (pin passive line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
)"#;

    const FOOTPRINT: &str = r#"(module TEST-FP (layer F.Cu) (tedit 5A02FF57)
  (fp_text reference REF** (at 0 -2.5) (layer F.SilkS) (effects (font (size 1 1) (thickness 0.15))))
  (fp_line (start -1.5 -1.5) (end 1.5 -1.5) (layer F.CrtYd) (width 0.05))
  (pad 1 smd rect (at -1 0.95) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask))
  (pad 2 smd rect (at 1 0.95) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask))
)"#;

    fn kicad_available() -> bool {
        kicad_env::kicad_cli().is_ok_and(|c| Command::new(c).arg("version").output().is_ok_and(|o| o.status.success()))
    }

    fn sources(name: &str) -> (PathBuf, Sources) {
        let dir = crate::test_dir(name);
        let (sym, fp) = (dir.join("t.kicad_sym"), dir.join("t.kicad_mod"));
        std::fs::write(&sym, SYMBOL).unwrap();
        std::fs::write(&fp, FOOTPRINT).unwrap();
        // One of KiCad's own models, so the 3D viewer has a real STEP file to load.
        let model = kicad_env::kicad_cli().unwrap().ancestors().nth(2).map(|d| d.join("share/kicad/3dmodels/Resistor_SMD.3dshapes/R_0603_1608Metric.step"));
        (dir.join("cache"), Sources { symbol: Some(sym), symbol_name: None, footprint: Some(fp), model: model.filter(|m| m.is_file()) })
    }

    #[test]
    fn symbol_and_footprint_render_to_svg() {
        if !kicad_available() {
            return eprintln!("kicad-cli not found; skipping");
        }
        let (cache, src) = sources("preview_svg");
        for kind in [Kind::Symbol, Kind::Footprint] {
            let svg = render_in(&cache, kind, &src).unwrap();
            let text = std::fs::read_to_string(&svg).unwrap();
            assert!(text.contains("<svg") && text.len() > 500, "{kind:?}: {text}");
        }
    }

    #[test]
    fn footprint_with_model_renders_to_png() {
        if !kicad_available() {
            return eprintln!("kicad-cli not found; skipping");
        }
        let (cache, src) = sources("preview_png");
        let png = render_in(&cache, Kind::Model, &src).unwrap();
        assert!(std::fs::read(&png).unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
        let bare = Sources { model: None, ..src };
        assert_eq!(render_in(&cache, Kind::Model, &bare).unwrap_err().to_string(), "No 3D model in this part");
    }

    #[test]
    fn library_part_renders_from_its_library_files() {
        if !kicad_available() {
            return eprintln!("kicad-cli not found; skipping");
        }
        let (cache, mut part_src) = sources("preview_library");
        part_src.symbol_name = None;
        let part = Part {
            mpn: "TESTPART".into(),
            symbol_file: part_src.symbol.clone(),
            footprint_files: part_src.footprint.clone().into_iter().collect(),
            model_files: part_src.model.clone().into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(Sources::from_part(&part), part_src);
        let root = LibraryRoot::project(&cache.parent().unwrap().join("prj"));
        root.add_part(&part, "Test", true).unwrap();
        let src = Sources::from_library(&root, "Test", "TESTPART");
        assert!(src.footprint.as_ref().is_some_and(|f| f.is_file()), "{src:?}");
        assert_eq!(src.model.is_some(), part_src.model.is_some());
        for kind in [Kind::Symbol, Kind::Footprint] {
            assert!(render_in(&cache, kind, &src).unwrap().is_file());
        }
        if part_src.model.is_some() {
            assert!(render_in(&cache, Kind::Model, &src).unwrap().is_file());
        }
    }

    #[test]
    fn missing_files_give_a_short_reason() {
        let dir = crate::test_dir("preview_missing");
        let err = |kind, src: &Sources| render_in(&dir, kind, src).unwrap_err().to_string();
        assert_eq!(err(Kind::Symbol, &Sources::default()), "No symbol in this part");
        assert_eq!(err(Kind::Footprint, &Sources::default()), "No footprint in this part");
        let gone = Sources { footprint: Some(dir.join("gone.kicad_mod")), ..Default::default() };
        assert!(err(Kind::Footprint, &gone).contains("can't be read"));
    }

    #[test]
    fn cached_picture_is_reused_until_the_file_changes() {
        if !kicad_available() {
            return eprintln!("kicad-cli not found; skipping");
        }
        let (cache, src) = sources("preview_cache");
        let first = render_in(&cache, Kind::Footprint, &src).unwrap();
        let stamp = std::fs::metadata(&first).unwrap().modified().unwrap();
        let t = std::time::Instant::now();
        assert_eq!(render_in(&cache, Kind::Footprint, &src).unwrap(), first);
        assert_eq!(std::fs::metadata(&first).unwrap().modified().unwrap(), stamp, "redrawn");
        assert!(t.elapsed().as_millis() < 500, "a cache hit must not run kicad-cli");

        let fp = src.footprint.as_ref().unwrap();
        std::fs::write(fp, FOOTPRINT.replace("(at 1 0.95)", "(at 1.2 0.95)").replace("-1.5 -1.5", "-2 -2")).unwrap();
        assert_ne!(render_in(&cache, Kind::Footprint, &src).unwrap(), first);
    }
}
