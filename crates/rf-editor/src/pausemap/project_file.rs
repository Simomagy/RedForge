//! The pause map's part of a project, as the web app writes its project
//! files: `{ app, version, objects, images }`, the pictures inline as data
//! URLs so that a project moves between machines. The same JSON goes in the
//! `.rfproj` (as `pauseMap`) and in a `map-project.json`; Aether
//! Cartography projects are recognised and translated.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};

use super::aether;
use super::project::{APP_ID, MapObject, PROJECT_VERSION, objects_from_json};

/// The pause map's objects and the pictures their stamps use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PauseMapData {
    pub objects: Vec<MapObject>,
    /// Picture files by id (PNG, JPEG...), shared with the undo steps.
    pub pictures: BTreeMap<String, Arc<Vec<u8>>>,
}

impl PauseMapData {
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// As the web app's project file, with the pictures the stamps use.
    pub fn to_json(&self) -> Value {
        let used: std::collections::BTreeSet<&str> = self
            .objects
            .iter()
            .filter_map(|o| match o {
                MapObject::Stamp(s) => Some(s.asset.as_str()),
                _ => None,
            })
            .collect();
        let images: serde_json::Map<String, Value> = self
            .pictures
            .iter()
            .filter(|(id, _)| used.contains(id.as_str()))
            .map(|(id, bytes)| (id.clone(), Value::String(to_data_url(bytes))))
            .collect();
        json!({ "app": APP_ID, "version": PROJECT_VERSION, "objects": self.objects, "images": images })
    }
}

/// Where an opened file came from, and what the user should know about it.
pub struct Opened {
    pub data: PauseMapData,
    pub aether: bool,
    pub notes: Vec<String>,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A web app project, the copy in an exported resource (`{ version,
/// objects }`), or an Aether project. `fonts` are the fonts' names there
/// are, for Aether's labels.
pub fn open(text: &str, fonts: &[String]) -> Result<Opened, String> {
    let json: Value = serde_json::from_str(text)
        .map_err(|_| "This file is not a project: it couldn't be read as JSON.".to_string())?;
    if aether::is_aether(&json) {
        let mut ids = 0;
        let r = aether::import(&json, fonts, |bytes| {
            let p = super::stamps::Picture::decode(bytes.to_vec()).ok()?;
            ids += 1;
            Some((format!("aether{ids:04}"), p.aspect()))
        })?;
        let mut notes = Vec::new();
        if !r.missing_fonts.is_empty() {
            notes.push(format!(
                "Add {} to letter the labels as in Aether; Zilla Slab stands in for now.",
                r.missing_fonts.join(", ")
            ));
        }
        if r.skipped_guarma > 0 {
            notes.push(format!(
                "{} left out: Guarma isn't supported yet.",
                plural(r.skipped_guarma, "Guarma object was", "Guarma objects were")
            ));
        }
        if r.skipped_unreadable > 0 {
            notes.push(format!(
                "{} skipped.",
                plural(r.skipped_unreadable, "damaged object was", "damaged objects were")
            ));
        }
        let data = PauseMapData {
            objects: r.objects,
            pictures: r.pictures.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(),
        };
        return Ok(Opened { data, aether: true, notes });
    }
    let app = json.get("app").and_then(Value::as_str);
    let objects = json.get("objects").and_then(Value::as_array);
    let legacy = app.is_none() && json.get("version").is_some_and(Value::is_number);
    let (Some(objects), true) = (objects, app == Some(APP_ID) || legacy) else {
        return Err("This file is not a Map Editor or Aether Cartography project.".into());
    };
    let data = from_json(&json, objects.clone());
    let mut notes = Vec::new();
    if data.1 > 0 {
        notes.push(format!("{} skipped.", plural(data.1, "damaged object was", "damaged objects were")));
    }
    let missing = missing_pictures(&data.0);
    if missing > 0 {
        notes.push(format!(
            "{} not in this file, so {} show empty: add the picture to them again.",
            plural(missing, "picture is", "pictures are"),
            if missing == 1 { "its stamps" } else { "their stamps" },
        ));
    }
    Ok(Opened { data: data.0, aether: false, notes })
}

/// The pause map from its JSON (as [`PauseMapData::to_json`] writes it).
pub fn read(json: &Value) -> PauseMapData {
    let objects = json.get("objects").and_then(Value::as_array).cloned().unwrap_or_default();
    from_json(json, objects).0
}

fn from_json(json: &Value, objects: Vec<Value>) -> (PauseMapData, usize) {
    let version = json.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
    let (objects, bad) = objects_from_json(version, objects);
    let pictures = json
        .get("images")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter().filter_map(|(id, url)| Some((id.clone(), Arc::new(from_data_url(url.as_str()?)?.1)))).collect()
        })
        .unwrap_or_default();
    (PauseMapData { objects, pictures }, bad)
}

/// How many pictures stamps use that aren't there.
pub fn missing_pictures(d: &PauseMapData) -> usize {
    let mut assets: Vec<&str> = d
        .objects
        .iter()
        .filter_map(|o| match o {
            MapObject::Stamp(s) => Some(s.asset.as_str()),
            _ => None,
        })
        .collect();
    assets.sort();
    assets.dedup();
    assets.iter().filter(|a| !d.pictures.contains_key(**a)).count()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// The kind of picture from its first bytes.
fn mime_of(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xff, 0xd8, ..] => "image/jpeg",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        _ => "application/octet-stream",
    }
}

pub fn to_data_url(bytes: &[u8]) -> String {
    format!("data:{};base64,{}", mime_of(bytes), base64_encode(bytes))
}

/// A base64 data URL's type and bytes.
pub fn from_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    let mime = head.strip_suffix(";base64")?;
    Some((mime.to_owned(), base64_decode(data)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_both_ways() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            assert_eq!(base64_decode(&base64_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        let (mime, bytes) = from_data_url("data:image/png;base64,iVBORw0K").unwrap();
        assert_eq!((mime.as_str(), &bytes[..4]), ("image/png", &[0x89, b'P', b'N', b'G'][..]));
    }

    #[test]
    fn a_project_both_ways() {
        let json = r#"{"app":"rdr-map-editor","version":3,"objects":[
            {"id":"a","type":"stamp","asset":"p1","target":"text","x":1,"y":2,"width":30,"rotation":0,"threshold":0.5,"invert":false,"aspect":1},
            {"id":"b","type":"stamp","asset":"gone","target":"lines","x":1,"y":2,"width":30,"rotation":0,"threshold":0.5,"invert":false,"aspect":1}],
            "images":{"p1":"data:image/png;base64,iVBORw0K"}}"#;
        let o = open(json, &[]).unwrap();
        assert!(!o.aether);
        assert_eq!(o.data.objects.len(), 2);
        assert_eq!(o.data.pictures.len(), 1);
        assert_eq!(o.notes.len(), 1, "{:?}", o.notes);
        let again = read(&o.data.to_json());
        assert_eq!(again, o.data);
        assert!(open(r#"{"hello":1}"#, &[]).is_err());
        // The copy in an exported resource.
        assert_eq!(open(r#"{"version":3,"objects":[]}"#, &[]).unwrap().data.objects.len(), 0);
    }
}
