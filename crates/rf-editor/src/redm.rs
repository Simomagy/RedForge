//! A RedM resource made from a project's files, as the C# RedForge's
//! `RedMResource` makes it: `<resources folder>/<name>/fxmanifest.lua` and a
//! `stream` folder with the game files. An existing manifest is kept: only the
//! info lines given (author, description, version) and what a map needs
//! (`this_is_a_map`, a `DLC_ITYP_REQUEST` per ytyp) are put in it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::project::is_game_file;

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
    /// The game files written to `stream`.
    pub written: Vec<String>,
    /// Files an export before wrote (`before`) that aren't in the project
    /// any more: taken out of `stream`, so they don't load in game.
    pub removed: Vec<String>,
}

pub fn export(
    resource: &Path,
    files: &[PathBuf],
    info: &BTreeMap<String, String>,
    before: &[String],
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
        } else {
            done.skipped.push(name);
        }
    }
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
        let done = export(&res, &files, &info, &[]).unwrap();
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
        let done = export(&res, &files[..1], &info, &["a.ymap".into(), "b.ytyp".into()]).unwrap();
        // b.ytyp left the project: it goes, with its DLC_ITYP_REQUEST; a file of the user's stays.
        assert_eq!(done.removed, ["b.ytyp"]);
        assert!(!res.join("stream/b.ytyp").exists() && res.join("stream/mine.ydr").exists());
        let without = std::fs::read_to_string(res.join("fxmanifest.lua")).unwrap();
        assert!(!without.contains("b.ytyp") && without.contains("-- mine"));
        export(&res, &files, &info, &done.written).unwrap();
        let again = std::fs::read_to_string(res.join("fxmanifest.lua")).unwrap();
        assert!(again.contains("-- mine") && again.contains("version '1.1.0'") && !again.contains("1.0.0"));
        assert_eq!(again.matches("DLC_ITYP_REQUEST").count(), 1);
        assert_eq!(again.matches("this_is_a_map").count(), 1);
    }
}
