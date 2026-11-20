//! A RedM resource made from a project's files, as the C# RedForge's
//! `RedMResource` makes it: `<resources folder>/<name>/fxmanifest.lua` and a
//! `stream` folder with the game files. An existing manifest is kept: only the
//! info lines given (author, description, version) and what a map needs
//! (`this_is_a_map`, a `DLC_ITYP_REQUEST` per ytyp) are put in it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rf_formats::hash::HashIndex;

use crate::amv;
use crate::anim::AnimSource;
use crate::project::{is_animation, is_game_file};

/// RedM won't start a resource without it.
const WARNING: &str = "rdr3_warning 'I acknowledge that this is a prerelease build of RedM, and I am aware my resources *will* become incompatible once RedM ships.'";

/// The manifest's info lines.
pub const INFO_KEYS: [&str; 3] = ["author", "description", "version"];

/// A resource name for server.cfg: lowercase letters, digits, `-` and `_`.
pub fn safe_name(name: &str) -> String {
    let mapped: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let mut out = String::new();
    for c in mapped.chars() {
        if !(c == '_' && out.ends_with('_')) {
            out.push(c);
        }
    }
    let out = out.trim_matches('_').to_owned();
    if out.is_empty() { "my_map".into() } else { out }
}

/// A Lua string in single quotes.
fn lua_string(s: &str) -> String {
    let s = s.replace('\\', "\\\\").replace('\'', "\\'").replace('\r', "").replace('\n', " ");
    format!("'{s}'")
}

/// The info line `key 'value'` (or `"value"`) in a manifest's text: the
/// range of the line (without its line break) and the value.
fn find_info(text: &str, key: &str) -> Option<(std::ops::Range<usize>, String)> {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let end = start + line.trim_end_matches(['\r', '\n']).len();
        let body = &text[start..end];
        let rest = body.trim_start_matches([' ', '\t']);
        if let Some(after) = rest.strip_prefix(key)
            && after.starts_with([' ', '\t'])
        {
            let after = after.trim_start_matches([' ', '\t']).trim_end_matches([' ', '\t']);
            let quote = after.chars().next().filter(|q| *q == '\'' || *q == '"');
            if let Some(q) = quote
                && after.len() >= 2
                && after.ends_with(q)
            {
                let value = &after[1..after.len() - 1];
                let value = value.replace("\\'", "'").replace("\\\"", "\"").replace("\\\\", "\\");
                return Some((start..end, value));
            }
        }
        start += line.len();
    }
    None
}

/// Author, description and version from an existing resource's manifest.
pub fn read_info(resource: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(resource.join("fxmanifest.lua")) else { return BTreeMap::new() };
    INFO_KEYS.iter().filter_map(|k| Some((k.to_string(), find_info(&text, k)?.1))).collect()
}

/// Copies the game files into `resource/stream` and writes the manifest, or
/// updates an existing one with only what's needed. Returns the files left
/// out (not game files).
/// What an export did.
#[derive(Debug, Default)]
pub struct Exported {
    /// The files that aren't game files, left out.
    pub skipped: Vec<String>,
    /// The game files written to `stream`: the project's, and the ycd baked
    /// from each of its animations (named after its dictionary).
    pub written: Vec<String>,
    /// Files an export before wrote (`before`) that aren't in the project
    /// any more: taken out of `stream`, so they don't load in game.
    pub removed: Vec<String>,
    /// How many of the project's interiors got an ambient mask volume, baked
    /// into an `amv_zone_<n>.ymt` and `.ytd` (among `written`).
    pub volumes: usize,
    /// What to tell of the interiors: those left without a volume, and what
    /// else their lighting lacks.
    pub notes: Vec<String>,
}

pub fn export(
    resource: &Path,
    files: &[PathBuf],
    info: &BTreeMap<String, String>,
    before: &[String],
    zone: u32,
    hashes: &HashIndex,
) -> Result<Exported, String> {
    let io = |e: std::io::Error, p: &Path| format!("{}: {e}", p.display());
    let stream = resource.join("stream");
    std::fs::create_dir_all(&stream).map_err(|e| io(e, &stream))?;
    let mut done = Exported::default();
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if is_game_file(f) {
            let to = stream.join(&name);
            std::fs::copy(f, &to).map_err(|e| io(e, f))?;
            done.written.push(name);
        } else if is_animation(f) {
            // An animation being made goes as the dictionary the game plays.
            let text = std::fs::read_to_string(f).map_err(|e| io(e, f))?;
            let anim = AnimSource::from_json(&text).map_err(|e| format!("{name}: {e}"))?;
            let ycd = anim.bake().map_err(|e| format!("{name}: {e}"))?;
            let baked = format!("{}.ycd", anim.dictionary);
            let to = stream.join(&baked);
            std::fs::write(&to, ycd).map_err(|e| io(e, &to))?;
            done.written.push(baked);
        } else {
            done.skipped.push(name);
        }
    }
    // An interior with no ambient mask volume is lit as if outdoors, its metals
    // black by day: one is baked for each into zone `zone`, unless the project
    // has a zone of its own.
    let baked = amv::bake_project(files, zone, hashes);
    if let Some(zone) = &baked.zone {
        for (ext, data) in [("ymt", &zone.ymt), ("ytd", &zone.ytd)] {
            let name = format!("{}.{ext}", amv::zone_name(zone.number));
            let to = stream.join(&name);
            std::fs::write(&to, data).map_err(|e| io(e, &to))?;
            done.written.push(name);
        }
    }
    done.volumes = baked.volumes;
    done.notes = baked.notes;
    // Only what RedForge wrote goes; other files in stream stay.
    for name in before {
        let gone = !done.written.iter().any(|w| w.eq_ignore_ascii_case(name));
        let file = stream.join(name);
        if gone && file.is_file() && std::fs::remove_file(&file).is_ok() {
            done.removed.push(name.clone());
        }
    }

    let manifest = resource.join("fxmanifest.lua");
    let existing = std::fs::read_to_string(&manifest).ok();
    let mut text =
        existing.clone().unwrap_or_else(|| format!("fx_version 'cerulean'\r\ngame 'rdr3'\r\n{WARNING}\r\n\r\n"));
    if !text.ends_with('\n') {
        text.push_str("\r\n");
    }
    // The ytyps taken out of stream don't stay requested.
    for name in done.removed.iter().filter(|n| n.to_lowercase().ends_with(".ytyp")) {
        let quoted = format!("'stream/{name}'");
        text = text.split_inclusive('\n').filter(|line| !line.contains(&quoted)).collect();
    }
    let mut added = String::new();
    if !text.contains("rdr3_warning") {
        added.push_str(&format!("{WARNING}\r\n"));
    }
    for key in INFO_KEYS {
        let Some(value) = info.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()) else { continue };
        let line = format!("{key} {}", lua_string(value));
        match find_info(&text, key) {
            Some((range, _)) => text.replace_range(range, &line),
            None => added.push_str(&format!("{line}\r\n")),
        }
    }
    if !added.is_empty() {
        added.push_str("\r\n");
    }
    // Custom archetypes only load with this_is_a_map and a DLC_ITYP_REQUEST per ytyp.
    if !text.contains("this_is_a_map") {
        added.push_str("this_is_a_map 'yes'\r\n");
    }
    let mut ytyps: Vec<String> = std::fs::read_dir(&stream)
        .map_err(|e| io(e, &stream))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_lowercase().ends_with(".ytyp"))
        .map(|n| format!("stream/{n}"))
        .collect();
    ytyps.sort();
    let missing: Vec<&String> = ytyps.iter().filter(|y| !text.contains(y.as_str())).collect();
    if !missing.is_empty() {
        added.push_str("\r\nfiles {\r\n");
        for y in &missing {
            added.push_str(&format!("    '{y}',\r\n"));
        }
        added.push_str("}\r\n");
        for y in &missing {
            added.push_str(&format!("data_file 'DLC_ITYP_REQUEST' '{y}'\r\n"));
        }
    }
    if !added.is_empty() && !text.ends_with("\n\n") && !text.ends_with("\r\n\r\n") {
        text.push_str("\r\n");
    }
    text.push_str(&added);
    if existing.as_deref() != Some(text.as_str()) {
        std::fs::write(&manifest, &text).map_err(|e| io(e, &manifest))?;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(safe_name(" My Map: Valentine!! "), "my_map_valentine");
        assert_eq!(safe_name("***"), "my_map");
        assert_eq!(safe_name("ok-name_2"), "ok-name_2");
    }

    #[test]
    fn an_interior_gets_an_ambient_mask_volume() {
        use rf_formats::formats::databag::Value;
        use rf_formats::formats::meta::Meta;
        use rf_formats::formats::{self, XmlContext};
        use rf_formats::hash::joaat;

        let dir = tempfile::tempdir().unwrap();
        let hashes = HashIndex::new();
        // A model, the interior with it in its one room, and the ymap placing the interior.
        let model = include_str!("../../rf-formats/tests/data/ydr/features.ydr.xml");
        let cx = XmlContext { hashes: &hashes, folder: None };
        let shell = formats::from_xml("shell.ydr.xml", model, &cx).unwrap();
        let mut entity = Meta::create("CEntityDef").unwrap();
        entity.set_hash("archetypeName", joaat("shell"));
        entity.set_vec4("rotation", [0.0, 0.0, 0.0, 1.0]);
        entity.set_f32("scaleXY", 1.0);
        entity.set_f32("scaleZ", 1.0);
        let mut room = Meta::create("CMloRoomDef").unwrap();
        room.set_vec3("bbMin", [-2.0; 3]);
        room.set_vec3("bbMax", [2.0; 3]);
        let mut mlo = Meta::create("CMloArchetypeDef").unwrap();
        mlo.set_hash("name", joaat("house"));
        mlo.set_object("entities", Some(Value::Bags(vec![Some(entity)])));
        mlo.set_object("rooms", Some(Value::Bags(vec![Some(room)])));
        let mut ytyp = Meta::create("CMapTypes").unwrap();
        ytyp.set_object("archetypes", Some(Value::Bags(vec![Some(mlo)])));
        let mut instance = Meta::create("CMloInstanceDef").unwrap();
        instance.set_hash("archetypeName", joaat("house"));
        instance.set_vec3("position", [100.0, 200.0, 30.0]);
        instance.set_vec4("rotation", [0.0, 0.0, 0.0, 1.0]);
        let mut map = Meta::create("CMapData").unwrap();
        map.set_object("entities", Some(Value::Bags(vec![Some(instance)])));
        let files: Vec<PathBuf> = ["shell.ydr", "house.ytyp", "town.ymap"].iter().map(|f| dir.path().join(f)).collect();
        std::fs::write(&files[0], shell).unwrap();
        std::fs::write(&files[1], Meta::new(Some(ytyp)).save().unwrap()).unwrap();
        std::fs::write(&files[2], Meta::new(Some(map)).save().unwrap()).unwrap();

        let res = dir.path().join("res/town");
        let done = export(&res, &files, &BTreeMap::new(), &[], 184, &hashes).unwrap();
        assert_eq!(done.volumes, 1, "{:?}", done.notes);
        let zone = amv::zone_name(184);
        assert_eq!(done.written[3..], [format!("{zone}.ymt"), format!("{zone}.ytd")]);
        assert!(res.join("stream").join(format!("{zone}.ytd")).exists());
        // The entity has none of the usual flags: said, not fixed. (Its only room is
        // the whole of it, which needs no time cycle or probe.)
        assert_eq!(done.notes.len(), 1, "{:?}", done.notes);
        assert!(done.notes[0].contains("1 of 1 entities"), "{:?}", done.notes);

        // With a zone of its own in the project, nothing is baked, and the one baked before goes.
        let own = dir.path().join("amv_zone_200.ymt");
        std::fs::write(&own, b"x").unwrap();
        let all: Vec<PathBuf> = files.iter().cloned().chain([own]).collect();
        let again = export(&res, &all, &BTreeMap::new(), &done.written, 184, &hashes).unwrap();
        assert_eq!((again.volumes, again.notes.len()), (0, 0));
        assert_eq!(again.removed, [format!("{zone}.ymt"), format!("{zone}.ytd")]);
    }

    #[test]
    fn exports_and_keeps_an_existing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let files: Vec<PathBuf> = ["a.ymap", "b.ytyp", "notes.txt"].iter().map(|f| src.join(f)).collect();
        for f in &files {
            std::fs::write(f, b"x").unwrap();
        }
        let res = dir.path().join("res/my_map");
        let mut info =
            BTreeMap::from([("author".to_string(), "Prysma".to_string()), ("version".into(), "1.0.0".into())]);
        let done = export(&res, &files, &info, &[], amv::FIRST_FREE_ZONE, &HashIndex::new()).unwrap();
        assert_eq!(done.skipped, ["notes.txt"]);
        assert_eq!(done.written, ["a.ymap", "b.ytyp"]);
        assert!(res.join("stream/a.ymap").exists() && res.join("stream/b.ytyp").exists());
        let text = std::fs::read_to_string(res.join("fxmanifest.lua")).unwrap();
        assert!(text.starts_with("fx_version 'cerulean'\r\ngame 'rdr3'\r\nrdr3_warning"));
        assert!(text.contains("author 'Prysma'\r\n") && text.contains("this_is_a_map 'yes'"));
        assert!(text.contains("data_file 'DLC_ITYP_REQUEST' 'stream/b.ytyp'"));
        assert_eq!(read_info(&res)["version"], "1.0.0");

        // A hand edit stays; a changed version replaces the line; nothing is added twice.
        std::fs::write(res.join("fxmanifest.lua"), text.clone() + "-- mine\r\n").unwrap();
        info.insert("version".into(), "1.1.0".into());
        std::fs::write(res.join("stream/mine.ydr"), b"x").unwrap();
        let done = export(
            &res,
            &files[..1],
            &info,
            &["a.ymap".into(), "b.ytyp".into()],
            amv::FIRST_FREE_ZONE,
            &HashIndex::new(),
        )
        .unwrap();
        // b.ytyp left the project: it goes, with its DLC_ITYP_REQUEST; a file of the user's stays.
        assert_eq!(done.removed, ["b.ytyp"]);
        assert!(!res.join("stream/b.ytyp").exists() && res.join("stream/mine.ydr").exists());
        let without = std::fs::read_to_string(res.join("fxmanifest.lua")).unwrap();
        assert!(!without.contains("b.ytyp") && without.contains("-- mine"));
        export(&res, &files, &info, &done.written, amv::FIRST_FREE_ZONE, &HashIndex::new()).unwrap();
        let again = std::fs::read_to_string(res.join("fxmanifest.lua")).unwrap();
        assert!(again.contains("-- mine") && again.contains("version '1.1.0'") && !again.contains("1.0.0"));
        assert_eq!(again.matches("DLC_ITYP_REQUEST").count(), 1);
        assert_eq!(again.matches("this_is_a_map").count(), 1);
    }

    #[test]
    fn animations_go_as_the_dictionary_the_game_plays() {
        use crate::anim::BoneKeys;
        let dir = tempfile::tempdir().unwrap();
        let mut anim =
            AnimSource { dictionary: "my@wave".into(), clip: "wave".into(), frames: 31, ..Default::default() };
        let mut arm = BoneKeys::new(40269, "SKEL_R_UpperArm", glam::Quat::IDENTITY, glam::Vec3::ZERO);
        arm.key_rotation(0.0, glam::Quat::IDENTITY);
        anim.bones.push(arm);
        let file = dir.path().join("wave.rfanim.json");
        std::fs::write(&file, anim.to_json()).unwrap();
        let res = dir.path().join("res");
        let done =
            export(&res, std::slice::from_ref(&file), &BTreeMap::new(), &[], amv::FIRST_FREE_ZONE, &HashIndex::new())
                .unwrap();
        assert_eq!(done.written, ["my@wave.ycd"]);
        assert!(done.skipped.is_empty());
        assert_eq!(std::fs::read(res.join("stream/my@wave.ycd")).unwrap(), anim.bake().unwrap());

        // Its dictionary renamed: the one written before goes.
        anim.dictionary = "my@hello".into();
        std::fs::write(&file, anim.to_json()).unwrap();
        let again = export(
            &res,
            std::slice::from_ref(&file),
            &BTreeMap::new(),
            &done.written,
            amv::FIRST_FREE_ZONE,
            &HashIndex::new(),
        )
        .unwrap();
        assert_eq!((again.written, again.removed), (vec!["my@hello.ycd".to_string()], vec!["my@wave.ycd".to_string()]));

        // One that can't be baked stops the export, saying which.
        anim.bones.clear();
        std::fs::write(&file, anim.to_json()).unwrap();
        let e = export(&res, &[file], &BTreeMap::new(), &[], amv::FIRST_FREE_ZONE, &HashIndex::new()).unwrap_err();
        assert!(e.starts_with("wave.rfanim.json: "), "{e}");
    }
}
