//! Expression dictionaries from a real RDR2 install. They only run when
//! `REDFORGE_GAME_DIR` points at the game folder; otherwise they pass without
//! checking anything.

use std::path::{Path, PathBuf};

use rf_formats::formats::XmlContext;
use rf_formats::formats::yed::{self, Expression, ExpressionStream, OpData};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

fn open() -> Option<(Rpf8FileSystem, HashIndex)> {
    let game = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt");
    strings.load_file(&path).expect("strings.txt");
    let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
    let fs = Rpf8FileSystem::open(&game, keys, &strings).expect("archives");
    Some((fs, strings))
}

/// `n` files with an extension, spread over all of them.
fn sample(fs: &Rpf8FileSystem, ext: &str, n: usize) -> Vec<(String, Vec<u8>)> {
    let all: Vec<_> = fs.files().filter(|(p, _)| p.ends_with(ext)).collect();
    let step = (all.len() / n).max(1);
    all.iter()
        .step_by(step)
        .take(n)
        .map(|(p, f)| (p.to_string(), fs.extract_loose_deflate(*f).unwrap_or_else(|e| panic!("{p}: {e}"))))
        .collect()
}

/// The streams, with the jumps' offsets in buffers 1 and 2 cleared: the C#
/// works them out differently from the game (buffer 2's is usually 12 more).
fn without_jump_offsets(e: &Expression) -> Option<Vec<Option<ExpressionStream>>> {
    let mut s = e.streams.clone();
    for op in s.iter_mut().flatten().flatten().flat_map(|s| s.instructions.iter_mut()) {
        if let OpData::Jump { data1_offset, data2_offset, .. } = &mut op.data {
            *data1_offset = 0;
            *data2_offset = 0;
        }
    }
    s
}

#[test]
fn expression_dictionaries_convert_both_ways() {
    let Some((fs, hashes)) = open() else { return };
    let cx = XmlContext { hashes: &hashes, folder: None };
    let mut expressions = 0;
    for (path, data) in sample(&fs, ".yed", 40) {
        let d = yed::read(&data, &hashes).unwrap_or_else(|e| panic!("{path}: {e}"));
        let xml = yed::to_xml(&data, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = yed::from_xml(&xml, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let d2 = yed::read(&back, &hashes).unwrap_or_else(|e| panic!("{path} written back: {e}"));
        assert_eq!(yed::to_xml(&back, &cx).unwrap(), xml, "{path}");

        // What the C# works out from the XML (variables and their indices,
        // stream sizes) is what the game has; jump offsets aren't.
        let by_name = |e: &&Expression| e.name.clone();
        let mut a: Vec<&Expression> = d.entries.iter().filter_map(|e| e.expression.as_ref()).collect();
        let mut b: Vec<&Expression> = d2.entries.iter().filter_map(|e| e.expression.as_ref()).collect();
        a.sort_by_key(by_name);
        b.sort_by_key(by_name);
        assert_eq!(a.len(), b.len(), "{path}");
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(
                x.variables.as_deref().unwrap_or_default(),
                y.variables.as_deref().unwrap_or_default(),
                "{path}"
            );
            assert_eq!(x.max_stream_size, y.max_stream_size, "{path}: {:?}", x.name);
            // Debug, so that NaNs compare equal.
            let (sx, sy) = (format!("{:?}", without_jump_offsets(x)), format!("{:?}", without_jump_offsets(y)));
            assert!(sx == sy, "{path}: {:?}", x.name);
            expressions += 1;
        }
    }
    assert!(expressions > 0, "no expressions checked");
}
