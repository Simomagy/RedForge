//! Shader effects databases (fxdb) from a real RDR2 install. They only run when
//! `REDFORGE_GAME_DIR` points at the game folder.
//!
//! These check the parsed data; `rf compare fxdb` checks the XML against the C#.

use std::path::{Path, PathBuf};

use rf_formats::formats::fxdb::{self, FxdbFile, ShaderStage};
use rf_formats::formats::{self, XmlContext};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

#[test]
fn shader_databases_read_with_their_bytecode() {
    let Some(dir) = std::env::var_os("REDFORGE_GAME_DIR").map(PathBuf::from) else { return };
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    let keys = Rpf8Keys::from_game_folder(&dir).expect("keys");
    let fs = Rpf8FileSystem::open(&dir, keys, &strings).expect("archives");

    // The small ones: D3D12 without compression, D3D12 and Vulkan with Oodle.
    let mut checked = 0;
    for (name, bytecode_magic) in [
        ("sga_win32_50_final_init.fxdb", *b"DXBC"),
        ("sga_win32_60_final_init.fxdb", *b"DXBC"),
        ("sga_vulkan_final_init.fxdb", 0x0723_0203u32.to_le_bytes()),
    ] {
        let Some((path, f)) = fs.files().find(|(p, _)| p.ends_with(name)).map(|(p, f)| (p.to_owned(), f)) else {
            panic!("no {name}")
        };
        let data = fs.extract(f).unwrap_or_else(|e| panic!("{path}: {e}"));
        let db = FxdbFile::read(&data, &strings).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(db.magic, fxdb::MAGIC_SGD1, "{path}");
        assert!(!db.vertex_shaders.is_empty() && !db.pixel_shaders.is_empty() && !db.effects.is_empty(), "{path}");
        for stage in ShaderStage::ALL {
            for s in db.shaders(stage) {
                assert_eq!(s.bytecode.get(..4), Some(&bytecode_magic[..]), "{path}: {stage:?} shader {}", s.name);
            }
        }

        // Every technique's passes are there, and some draw (others are compute
        // passes, or use shaders that are in the main databases).
        let (mut passes, mut drawn) = (0, 0);
        for e in &db.effects {
            for t in &e.techniques {
                assert!(!t.name.is_empty(), "{path}: {}", e.name);
                let tp = e.technique_passes(t).unwrap_or_else(|| panic!("{path}: {}.{}", e.name, t.name));
                for p in tp {
                    passes += 1;
                    let vs = db.pass_shader(e, p, ShaderStage::Vertex);
                    let ps = db.pass_shader(e, p, ShaderStage::Pixel);
                    if vs.is_some() && ps.is_some() {
                        drawn += 1;
                    }
                }
            }
        }
        assert!(drawn > 0, "{path}: none of {passes} passes has vertex and pixel shaders");

        // Constant buffers name their variables, which start inside the buffer.
        let cbuffers = db.vertex_shaders.iter().flat_map(|s| s.data.resources.iter().map(move |r| (s, r)));
        for (s, res) in cbuffers.filter(|(_, r)| r.resource_type() == 0) {
            assert!(!res.name.is_empty(), "{path}: {}", s.name);
            for e in s.data.resource_elements(res) {
                assert!(!e.name.is_empty(), "{path}: {}.{}", s.name, res.name);
                assert!(e.start_offset < res.byte_size, "{path}: {}.{}.{}", s.name, res.name, e.name);
            }
        }

        let cx = XmlContext { hashes: &strings, folder: None };
        let xml = formats::to_xml(name, &data, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RDR2EffectsDatabase version=\"0\">\n"));
        checked += 1;
    }
    assert_eq!(checked, 3);
}
