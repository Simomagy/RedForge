//! Drawables (ydr) and drawable dictionaries (ydd).
//!
//! `features.ydr.xml` and `dict.ydd.xml` are hand-written to use what game
//! files rarely have (joint limits, every vertex format the files can store,
//! array and unknown shader parameters, the cable shader's line topology,
//! extra skeletons, sampler dictionaries, null and empty LODs...); the
//! `.cs.` files are what the C# RedForge reads back from the file it writes
//! from them (through `rf-oracle`). The game-file test needs
//! `REDFORGE_GAME_DIR`; the C# comparison of game files is `rf compare ydr
//! --write` and `rf compare ydd --write`.

use std::path::{Path, PathBuf};

use rf_formats::formats::ydr::{self, ShaderTexture, Topology};
use rf_formats::formats::{self, XmlContext};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

const FEATURES: &str = include_str!("data/ydr/features.ydr.xml");
const FEATURES_CS: &str = include_str!("data/ydr/features.cs.ydr.xml");
const DICT: &str = include_str!("data/ydr/dict.ydd.xml");
const DICT_CS: &str = include_str!("data/ydr/dict.cs.ydd.xml");

fn strings() -> HashIndex {
    let h = HashIndex::new();
    h.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    h
}

#[test]
fn drawable_round_trips_like_the_csharp() {
    let hashes = strings();
    let cx = XmlContext { hashes: &hashes, folder: None };
    let file = formats::from_xml("features.ydr.xml", FEATURES, &cx).unwrap();
    let xml = formats::to_xml("features.ydr", &file, &cx).unwrap();
    assert_eq!(xml, FEATURES_CS);
    // And again: what was read back writes and reads back the same.
    let file = formats::from_xml("features.ydr.xml", &xml, &cx).unwrap();
    assert_eq!(formats::to_xml("features.ydr", &file, &cx).unwrap(), FEATURES_CS);

    let d = ydr::read(&file, &cx).unwrap();
    let models: Vec<_> = d.base.models().collect();
    assert_eq!(models.len(), 2);
    let g = models[0].geometries().next().unwrap();
    assert_eq!(g.name.as_deref(), Some("cable"));
    assert_eq!(g.topology, Topology::LineList);
    assert_eq!(g.bone_index, 1);
    assert_eq!(g.layout.semantics_string(), "PNXWICTTT");
    // Decoded vertex data: the second vertex's position and half-float UV.
    assert_eq!(g.vertex_value(1, 0).unwrap(), [2.0, 1.0, 2.0, 0.0]);
    assert_eq!(g.vertex_value(1, 6).unwrap()[0], 65504.0);
    assert_eq!(g.indices(), Some(&[0u16, 1, 2, 2, 1, 0][..]));
    // The LOD masks come from the models.
    assert_eq!(d.base.mask(0), 253);
    let skel = d.base.skeleton.as_ref().unwrap();
    assert_eq!(skel.bones.as_ref().unwrap().len(), 3);
    assert_eq!(skel.mapping_block.as_ref().unwrap().indices, Some(vec![-1, 0, 1]));
    let joints = d.base.joint_limits.as_ref().unwrap();
    assert_eq!(joints.rotation_limits.as_ref().unwrap()[0].bone_tag, 12345);
    assert_eq!(d.sampler_dictionary.as_ref().unwrap().samplers.as_ref().unwrap().len(), 2);
}

#[test]
fn dictionary_round_trips_like_the_csharp() {
    let hashes = strings();
    let cx = XmlContext { hashes: &hashes, folder: None };
    let file = formats::from_xml("dict.ydd.xml", DICT, &cx).unwrap();
    let xml = formats::to_xml("dict.ydd", &file, &cx).unwrap();
    assert_eq!(xml, DICT_CS);
    let d = ydr::read_dictionary(&file, &cx).unwrap();
    // Sorted by hash: "zebra" before "aardvark".
    let names: Vec<_> = d.drawables.iter().flatten().flatten().map(|d| d.base.name.clone().unwrap()).collect();
    assert_eq!(names, ["zebra", "aardvark"]);
    assert!(d.hashes.as_ref().unwrap().is_sorted());
}

fn game() -> Option<(Rpf8FileSystem, HashIndex)> {
    let game = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = strings();
    let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
    let fs = Rpf8FileSystem::open(&game, keys, &strings).expect("archives");
    Some((fs, strings))
}

fn dir_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn embedded_textures(d: &ydr::DrawableBase) -> usize {
    let shaders = d.shader_group.iter().flat_map(|g| g.shaders.iter().flatten().flatten());
    shaders
        .flat_map(|s| s.parameters.iter().flat_map(|p| p.params.iter().flatten()))
        .filter(|p| matches!(p.texture, Some(ShaderTexture::Embedded(_))))
        .count()
}

/// The XML with what changes on every trip through XML (in the C# too) left
/// out: "-0" vertex values are written as "0", and collision material names
/// (see `ybn`'s known issues) are dropped.
fn normalized(xml: &str) -> String {
    let fix = |line: &str| {
        if line.trim_start().starts_with("<MaterialName>") {
            return String::new();
        }
        line.split('\t')
            .map(|e| e.split(' ').map(|v| if v == "-0" { "0" } else { v }).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\t")
    };
    xml.lines().map(fix).collect::<Vec<_>>().join("\n")
}

/// Game drawables and dictionaries convert to XML, back, and to the same XML
/// (but for what [`normalized`] leaves out) and DDS files again; parameters
/// using the drawable's own textures still point to them.
#[test]
fn game_files_round_trip_through_xml() {
    let Some((fs, hashes)) = game() else { return };
    let tmp = std::env::temp_dir().join(format!("rf-ydr-game-test-{}", std::process::id()));
    let (mut checked, mut embedded) = (0, 0);
    for ext in [".ydr", ".ydd"] {
        let mut files: Vec<_> = fs.files().filter(|(p, _)| p.ends_with(ext)).collect();
        files.sort();
        let step = files.len() / 25;
        for (path, f) in files.iter().step_by(step) {
            let name = path.rsplit('/').next().unwrap();
            let xml_name = format!("{name}.xml");
            let data = fs.extract_loose_deflate(*f).unwrap();
            let (a, b) = (tmp.join(format!("{checked}a")), tmp.join(format!("{checked}b")));
            let cxa = XmlContext { hashes: &hashes, folder: Some(a.clone()) };
            let xml = formats::to_xml(name, &data, &cxa).unwrap_or_else(|e| panic!("{path}: {e}"));
            let bin = formats::from_xml(&xml_name, &xml, &cxa).unwrap_or_else(|e| panic!("{path}: {e}"));
            let cxb = XmlContext { hashes: &hashes, folder: Some(b.clone()) };
            let xml2 = formats::to_xml(name, &bin, &cxb).unwrap();
            assert_eq!(normalized(&xml), normalized(&xml2), "{path}");
            assert_eq!(dir_files(&a), dir_files(&b), "{path}: DDS files differ");
            let bin2 = formats::from_xml(&xml_name, &xml2, &cxb).unwrap();
            assert_eq!(normalized(&formats::to_xml(name, &bin2, &cxb).unwrap()), normalized(&xml2), "{path}");
            if ext == ".ydr" {
                let before = embedded_textures(&ydr::read(&data, &cxa).unwrap().base);
                assert_eq!(before, embedded_textures(&ydr::read(&bin, &cxa).unwrap().base), "{path}");
                embedded += before;
            }
            checked += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(checked >= 50, "only {checked} files checked");
    assert!(embedded > 0, "no parameter used an embedded texture");
}
