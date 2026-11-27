//! Aether Cartography projects (`aether-project.json`). Their map space is
//! the same main text atlas, so positions carry over; their styles are
//! translated.

use serde_json::Value;

use super::families::Layer;
use super::fonts::DEFAULT_FONT;
use super::path::{Point, from_points};
use super::project::*;

/// One of Aether's font keys: how to know its face among the fonts there
/// are, and the face's name for the user.
struct FontFace {
    key: &'static str,
    matches: fn(&str) -> bool,
    face: &'static str,
}

fn heavy(n: &str) -> bool {
    n.contains("demi") || n.contains("bold") || n.contains("semi")
}

const FONT_FACES: [FontFace; 4] = [
    FontFace { key: "rdrlino", matches: |n| n.contains("lino"), face: "RDR Lino" },
    FontFace { key: "hapna_demi", matches: |n| n.contains("hapna") && heavy(n), face: "Hapna Slab Serif DemiBold" },
    FontFace { key: "hapna", matches: |n| n.contains("hapna") && !heavy(n), face: "Hapna Slab Serif" },
    FontFace { key: "zilla", matches: |n| n.contains("zilla"), face: "Zilla Slab" },
];

/// The main map's text atlas: tile 512, stride 510, 10 x 8.
const TILE: f64 = 512.0;
const STRIDE: f64 = 510.0;
const COLS: f64 = 10.0;
const ROWS: f64 = 8.0;
/// Aether's Mexico is a window into the main atlases, five tile rows down.
const MEXICO_DY: f64 = 5.0 * STRIDE;

pub struct AetherImport {
    pub objects: Vec<MapObject>,
    /// Pictures of the stamps: id, file.
    pub pictures: Vec<(String, Vec<u8>)>,
    pub skipped_guarma: usize,
    pub skipped_unreadable: usize,
    /// Faces the project uses that aren't among the fonts.
    pub missing_fonts: Vec<String>,
}

fn num(v: &Value, key: &str, fallback: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(fallback)
}

pub fn is_aether(json: &Value) -> bool {
    json.get("app").and_then(Value::as_str) == Some("aether-cartography")
}

/// Imports an Aether project; `fonts` are the fonts' names there are.
/// `picture` decodes a stamp's data URL into a picture's id and aspect.
pub fn import(
    json: &Value,
    fonts: &[String],
    mut picture: impl FnMut(&[u8]) -> Option<(String, f64)>,
) -> Result<AetherImport, String> {
    let Some(objects) = json.get("objects").and_then(Value::as_array) else {
        return Err("This file is not an Aether Cartography project.".into());
    };
    let version = num(json, "version", 0.0);
    // Before version 13 Aether laid tiles edge to edge, two pixels wider at
    // every seam.
    let seam =
        |v: f64, count: f64| if version >= 13.0 { v } else { v - 2.0 * (v / TILE).floor().clamp(0.0, count - 1.0) };
    let mut out = AetherImport {
        objects: Vec::new(),
        pictures: Vec::new(),
        skipped_guarma: 0,
        skipped_unreadable: 0,
        missing_fonts: Vec::new(),
    };
    let font_for = |key: Option<&str>, missing: &mut Vec<String>| -> String {
        if let Some(entry) = FONT_FACES.iter().find(|f| Some(f.key) == key) {
            if let Some(f) = fonts.iter().find(|f| (entry.matches)(&f.to_lowercase())) {
                return f.clone();
            }
            if entry.face != "Zilla Slab" && !missing.iter().any(|m| m == entry.face) {
                missing.push(entry.face.into());
            }
        }
        DEFAULT_FONT.into()
    };

    for value in objects {
        if !value.is_object() {
            continue;
        }
        let map = value.get("map").and_then(Value::as_str);
        if map == Some("guarma") {
            out.skipped_guarma += 1;
            continue;
        }
        let dy = if map == Some("mexico") { MEXICO_DY } else { 0.0 };
        let at = |x: Option<f64>, y: Option<f64>| -> Option<Point> {
            match (x, y) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some([seam(x, COLS), seam(y + dy, ROWS)]),
                _ => None,
            }
        };
        let points = |list: Option<&Value>| -> Vec<Point> {
            list.and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .filter_map(|p| at(p.get(0).and_then(Value::as_f64), p.get(1).and_then(Value::as_f64)))
                        .collect()
                })
                .unwrap_or_default()
        };
        let xy = || at(value.get("x").and_then(Value::as_f64), value.get("y").and_then(Value::as_f64));
        let common =
            Common { id: new_id(), hidden: value.get("visible") == Some(&Value::Bool(false)), ..Default::default() };
        let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
        let rot = num(value, "rot", 0.0).clamp(-180.0, 180.0);
        let object = match kind {
            "border" => {
                let pts = points(value.get("points"));
                let style = match value.get("line_style").and_then(Value::as_str) {
                    Some("road") => LineStyle::Road,
                    Some("trail") => LineStyle::Trail,
                    Some("railroad") => LineStyle::Railroad,
                    Some("solid") => LineStyle::Solid,
                    _ => LineStyle::Boundary,
                };
                let width = num(value, "line_width", 0.75).clamp(0.25, 4.0);
                let spacing = num(value, "line_spacing", num(value, "gap", 8.0)).clamp(2.0, 32.0);
                if pts.len() < 2 {
                    None
                } else {
                    // Aether draws a railroad 3.5 times its width with markers
                    // every 6.5 spacings, a trail as dashes 0.9 spacing long,
                    // and boundary dots a spacing apart.
                    let smooth = value.get("smooth") == Some(&Value::Bool(true));
                    Some(MapObject::Line(LineObject {
                        common,
                        style,
                        nodes: from_points(&pts, smooth),
                        closed: false,
                        width: if style == LineStyle::Railroad { (width * 3.5).max(2.5) } else { width },
                        spacing: match style {
                            LineStyle::Railroad => (spacing * 6.5).max(34.0),
                            LineStyle::Trail => spacing * 0.9,
                            LineStyle::Boundary => spacing + 0.8,
                            _ => spacing,
                        },
                    }))
                }
            }
            "label" => xy().map(|p| {
                let size = num(value, "px", 29.0).clamp(8.0, 200.0);
                let grow = num(value, "grow", (5.0 / size.max(1.0) + 0.02).clamp(0.05, 0.34));
                let text = match value.get("text") {
                    Some(Value::String(s)) => s.chars().take(64).collect(),
                    Some(Value::Null) | None => String::new(),
                    Some(v) => v.to_string(),
                };
                MapObject::Label(LabelObject {
                    common,
                    text,
                    font: font_for(value.get("font").and_then(Value::as_str), &mut out.missing_fonts),
                    size,
                    x: p[0],
                    y: p[1],
                    rotation: rot,
                    tracking: num(value, "tracking", 0.14).clamp(0.0, 3.0),
                    curve: (num(value, "curve", 0.0) / 100.0).clamp(-2.0, 2.0),
                    // Aether's outline is ~4.3 units outside the ink's
                    // threshold; ours is on it.
                    weight: (((grow * size - 4.3) * 10.0).round() / 10.0).clamp(-1.0, 4.0),
                    caps: true,
                })
            }),
            "building" => xy().map(|p| {
                let shape = match value.get("building_shape").and_then(Value::as_str) {
                    Some("l_shape") => BuildingShape::L,
                    Some("u_shape") => BuildingShape::U,
                    Some("t_shape") => BuildingShape::T,
                    Some("courtyard") => BuildingShape::Courtyard,
                    Some("station") => BuildingShape::Station,
                    _ => BuildingShape::Rect,
                };
                MapObject::Building(BuildingObject {
                    common,
                    shape,
                    x: p[0],
                    y: p[1],
                    w: num(value, "w", 14.0).clamp(2.0, 120.0),
                    h: num(value, "h", 9.0).clamp(2.0, 120.0),
                    rotation: rot,
                    outline: num(value, "line_width", 0.75).clamp(0.25, 3.0),
                    dot: None,
                })
            }),
            "cover" | "line_erase" => {
                let target = if kind == "cover" { Layer::Text } else { Layer::Lines };
                let mut pts = points(value.get("points"));
                let mut radius = num(value, "radius", if kind == "cover" { 26.0 } else { 12.0 });
                // Old rectangular covers: one stroke through the box's middle.
                if pts.is_empty() && kind == "cover" {
                    let (x, y, w, h) = (
                        num(value, "x", f64::NAN),
                        num(value, "y", f64::NAN),
                        num(value, "w", f64::NAN),
                        num(value, "h", f64::NAN),
                    );
                    if let (Some(a), Some(b)) = (at(Some(x), Some(y + h / 2.0)), at(Some(x + w), Some(y + h / 2.0))) {
                        pts = vec![[a[0] + h / 2.0, a[1]], [b[0] - h / 2.0, b[1]]];
                        radius = h.abs() / 2.0;
                    }
                }
                (!pts.is_empty()).then(|| {
                    MapObject::Erase(EraseObject { common, target, points: pts, radius: radius.clamp(2.0, 120.0) })
                })
            }
            "stamp" => {
                let src = value.get("src").and_then(Value::as_str).unwrap_or_default();
                let pic = super::project_file::from_data_url(src)
                    .filter(|(mime, _)| matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/webp"));
                match (xy(), pic) {
                    (Some(p), Some((_, bytes))) => picture(&bytes).map(|(asset, aspect)| {
                        out.pictures.push((asset.clone(), bytes));
                        let name = value.get("name").and_then(Value::as_str).map(|n| n.chars().take(40).collect());
                        MapObject::Stamp(StampObject {
                            common: Common { name, ..common },
                            asset,
                            aspect,
                            x: p[0],
                            y: p[1],
                            target: if value.get("channel").and_then(Value::as_str) == Some("lines") {
                                Layer::Lines
                            } else {
                                Layer::Text
                            },
                            width: num(value, "w", 200.0).clamp(8.0, 4000.0),
                            rotation: rot,
                            threshold: num(value, "threshold", 0.5).clamp(0.05, 0.95),
                            invert: value.get("invert") == Some(&Value::Bool(true)),
                        })
                    }),
                    _ => None,
                }
            }
            _ => None,
        };
        match object {
            Some(o) => out.objects.push(o),
            None => out.skipped_unreadable += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_aether_project() {
        let path =
            std::path::Path::new("/run/media/msstu/Duomo/RedM/RedForge/pry-rdrmapeditor/.tiles/aether-project.json");
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let json: Value = serde_json::from_str(&text).unwrap();
        assert!(is_aether(&json));
        let fonts = vec![DEFAULT_FONT.to_owned()];
        let r = import(&json, &fonts, |_| Some(("pic".into(), 1.0))).unwrap();
        assert_eq!(r.objects.len() + r.skipped_unreadable + r.skipped_guarma, 35);
        assert_eq!(r.skipped_unreadable, 0);
        let count = |k: &str| r.objects.iter().filter(|o| serde_json::to_value(o).unwrap()["type"] == k).count();
        assert_eq!((count("label"), count("line"), count("building"), count("erase")), (15, 5, 3, 12));
    }
}
