//! What the user draws on the pause map, as the Map Editor web app keeps it
//! (`map-project.json`, version 3): coordinates in map units (text atlas
//! pixels), one flat list of objects, last on top.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::families::Layer;
use super::path::{PathNode, Point, from_points};

pub const PROJECT_VERSION: u32 = 3;
/// What marks the web app's project files.
pub const APP_ID: &str = "rdr-map-editor";

fn is_false(b: &bool) -> bool {
    !*b
}

/// What every layer has: naming, visibility, locking and its group.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Common {
    pub id: String,
    /// Shown in the layer list instead of the automatic name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Left out of the map and the export.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Can't be picked or changed on the map.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// The id of the group it's in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineStyle {
    Road,
    Trail,
    Railroad,
    Boundary,
    Solid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineObject {
    #[serde(flatten)]
    pub common: Common,
    pub style: LineStyle,
    /// Bézier anchors with handles.
    pub nodes: Vec<PathNode>,
    #[serde(default)]
    pub closed: bool,
    /// Map units.
    pub width: f64,
    /// Map units between dashes, dots or rail markers.
    #[serde(default)]
    pub spacing: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupObject {
    #[serde(flatten)]
    pub common: Common,
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
}

/// Takes the game's own marks away under a brush stroke; never what's drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EraseObject {
    #[serde(flatten)]
    pub common: Common,
    /// Text, lines or relief.
    pub target: Layer,
    pub points: Vec<Point>,
    /// Map units.
    pub radius: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelObject {
    #[serde(flatten)]
    pub common: Common,
    pub text: String,
    /// The font's family name.
    pub font: String,
    /// Map units (the em).
    pub size: f64,
    /// The centre.
    pub x: f64,
    pub y: f64,
    /// Degrees.
    #[serde(default)]
    pub rotation: f64,
    /// Extra letter spacing, a fraction of the size.
    #[serde(default)]
    pub tracking: f64,
    /// The arc's bend, a fraction of the size (negative bends down).
    #[serde(default)]
    pub curve: f64,
    /// Map units added around every stroke (negative thins).
    #[serde(default)]
    pub weight: f64,
    #[serde(default)]
    pub caps: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildingShape {
    Rect,
    L,
    U,
    T,
    Courtyard,
    Station,
}

/// A footprint: its outline in the line family, a grey fill in the relief one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildingObject {
    #[serde(flatten)]
    pub common: Common,
    pub shape: BuildingShape,
    /// The centre.
    pub x: f64,
    pub y: f64,
    /// Map units.
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub rotation: f64,
    /// The outline's width (a station's ring), map units.
    pub outline: f64,
    /// A station's centre dot's diameter; 30% of the ring when not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dot: Option<f64>,
}

/// A picture turned into ink: its dark pixels (or light ones, inverted)
/// become map lettering or linework.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StampObject {
    #[serde(flatten)]
    pub common: Common,
    /// The picture's id in the project's pictures.
    pub asset: String,
    /// Text or lines.
    pub target: Layer,
    /// The centre.
    pub x: f64,
    pub y: f64,
    /// Map units; the height follows the picture.
    pub width: f64,
    #[serde(default)]
    pub rotation: f64,
    /// 0 to 1: the luminance below which a pixel is ink.
    pub threshold: f64,
    #[serde(default)]
    pub invert: bool,
    /// The picture's height over its width, so that the footprint is known
    /// without it.
    pub aspect: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MapObject {
    Line(LineObject),
    Erase(EraseObject),
    Label(LabelObject),
    Building(BuildingObject),
    Stamp(StampObject),
    Group(GroupObject),
}

impl MapObject {
    pub fn common(&self) -> &Common {
        match self {
            MapObject::Line(o) => &o.common,
            MapObject::Erase(o) => &o.common,
            MapObject::Label(o) => &o.common,
            MapObject::Building(o) => &o.common,
            MapObject::Stamp(o) => &o.common,
            MapObject::Group(o) => &o.common,
        }
    }
    pub fn common_mut(&mut self) -> &mut Common {
        match self {
            MapObject::Line(o) => &mut o.common,
            MapObject::Erase(o) => &mut o.common,
            MapObject::Label(o) => &mut o.common,
            MapObject::Building(o) => &mut o.common,
            MapObject::Stamp(o) => &mut o.common,
            MapObject::Group(o) => &mut o.common,
        }
    }
    pub fn id(&self) -> &str {
        &self.common().id
    }
    pub fn is_group(&self) -> bool {
        matches!(self, MapObject::Group(_))
    }
    /// The centre and rotation of the ones placed by their centre.
    pub fn boxed(&self) -> Option<(f64, f64, f64)> {
        match self {
            MapObject::Label(o) => Some((o.x, o.y, o.rotation)),
            MapObject::Building(o) => Some((o.x, o.y, o.rotation)),
            MapObject::Stamp(o) => Some((o.x, o.y, o.rotation)),
            _ => None,
        }
    }
}

/// A line style's name and defaults.
pub struct StyleInfo {
    pub style: LineStyle,
    pub label: &'static str,
    pub width: f64,
    pub spacing: f64,
}

pub const LINE_STYLES: [StyleInfo; 5] = [
    StyleInfo { style: LineStyle::Road, label: "Road", width: 1.0, spacing: 0.0 },
    StyleInfo { style: LineStyle::Trail, label: "Trail", width: 0.8, spacing: 5.0 },
    StyleInfo { style: LineStyle::Railroad, label: "Railroad", width: 1.4, spacing: 12.0 },
    StyleInfo { style: LineStyle::Boundary, label: "Boundary", width: 0.9, spacing: 4.0 },
    StyleInfo { style: LineStyle::Solid, label: "Solid line", width: 0.6, spacing: 0.0 },
];

pub fn style_info(style: LineStyle) -> &'static StyleInfo {
    LINE_STYLES.iter().find(|s| s.style == style).unwrap()
}

/// Presets like the game's own lettering (cap height and stroke).
pub struct LabelPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub note: &'static str,
    pub size: f64,
    pub tracking: f64,
    pub weight: f64,
    pub caps: bool,
}

pub const LABEL_PRESETS: [LabelPreset; 4] = [
    LabelPreset {
        id: "region",
        label: "Region",
        note: "Like NEW HANOVER",
        size: 52.0,
        tracking: 1.4,
        weight: 1.9,
        caps: true,
    },
    LabelPreset {
        id: "district",
        label: "District",
        note: "Like THE HEARTLANDS",
        size: 35.0,
        tracking: 0.6,
        weight: 0.6,
        caps: true,
    },
    LabelPreset {
        id: "town",
        label: "Town",
        note: "Like VALENTINE",
        size: 29.0,
        tracking: 0.14,
        weight: 0.9,
        caps: true,
    },
    LabelPreset {
        id: "place",
        label: "Place",
        note: "Like Cumberland Falls",
        size: 18.0,
        tracking: 0.14,
        weight: 0.7,
        caps: false,
    },
];

pub const BUILDING_SHAPES: [(BuildingShape, &str); 6] = [
    (BuildingShape::Rect, "Block"),
    (BuildingShape::L, "L-shape"),
    (BuildingShape::U, "U-shape"),
    (BuildingShape::T, "T-shape"),
    (BuildingShape::Courtyard, "Courtyard"),
    (BuildingShape::Station, "Station"),
];

/// Saved objects brought up to date, as JSON:
/// - 1 to 2: a label's weight was a fraction of its size with the outline
///   4.3 units outside the ink; it's map units added around the stroke now.
/// - 2 to 3: lines were point lists, optionally smoothed; they're Bézier
///   paths now.
pub fn migrate(version: u32, objects: Vec<Value>) -> Vec<Value> {
    objects
        .into_iter()
        .map(|mut o| {
            let kind = o.get("type").and_then(Value::as_str).unwrap_or_default().to_owned();
            if version < 2 && kind == "label" {
                let (w, s) = (num(&o, "weight"), num(&o, "size"));
                o["weight"] = (((w * s - 4.3) * 10.0).round() / 10.0).clamp(-1.0, 4.0).into();
            }
            if version < 3 && kind == "line" && o.get("points").is_some() {
                let points: Vec<Point> = serde_json::from_value(o["points"].clone()).unwrap_or_default();
                let smooth = o.get("smooth").and_then(Value::as_bool).unwrap_or(false);
                let m = o.as_object_mut().unwrap();
                m.remove("points");
                m.remove("smooth");
                m.insert("nodes".into(), serde_json::to_value(from_points(&points, smooth)).unwrap());
                m.insert("closed".into(), false.into());
            }
            o
        })
        .collect()
}

fn num(o: &Value, key: &str) -> f64 {
    o.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Objects from JSON (migrated first); the ones that can't be read are
/// counted and left out.
pub fn objects_from_json(version: u32, objects: Vec<Value>) -> (Vec<MapObject>, usize) {
    let mut bad = 0;
    let out = migrate(version, objects)
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).map_err(|_| bad += 1).ok())
        .collect();
    (out, bad)
}

/// A new layer id: 8 hex digits, as the web app makes them.
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNT.fetch_add(1, Ordering::Relaxed));
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:08x}", h.finish() as u32)
}

/// What the layer list calls an object the user hasn't named.
pub fn object_name(o: &MapObject) -> String {
    if let Some(n) = &o.common().name {
        return n.clone();
    }
    match o {
        MapObject::Line(l) => style_info(l.style).label.into(),
        MapObject::Label(l) if l.text.trim().is_empty() => "Empty label".into(),
        MapObject::Label(l) => l.text.trim().into(),
        MapObject::Building(b) => BUILDING_SHAPES.iter().find(|(s, _)| *s == b.shape).unwrap().1.into(),
        MapObject::Stamp(_) => "Image stamp".into(),
        MapObject::Group(_) => "Group".into(),
        MapObject::Erase(e) => {
            let what = match e.target {
                Layer::Text => "labels",
                Layer::Lines => "roads",
                _ => "fills",
            };
            format!("Eraser · {what}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_smoothed_lines_keep_their_curve() {
        let points = [[0.0, 0.0], [40.0, 30.0], [90.0, 10.0], [140.0, 60.0]];
        let old = serde_json::json!({ "id": "a", "type": "line", "style": "road", "width": 1, "spacing": 0, "points": points, "smooth": true });
        let (objs, bad) = objects_from_json(2, vec![old]);
        assert_eq!(bad, 0);
        let MapObject::Line(l) = &objs[0] else { panic!() };
        assert_eq!(l.nodes.iter().map(|n| n.p).collect::<Vec<_>>(), points.to_vec());
        assert!(l.nodes[1].inn.is_some() && l.nodes[1].out.is_some());
        assert!(from_points(&points, false).iter().all(|n| n.inn.is_none() && n.out.is_none()));
    }

    #[test]
    fn json_round_trip_like_the_web_app() {
        let json = r#"{"id":"k1","type":"label","text":"Hello","font":"Zilla Slab Bold","size":29,"x":10.5,"y":20,"rotation":0,"tracking":0.14,"curve":0,"weight":0.9,"caps":true,"group":"g"}"#;
        let o: MapObject = serde_json::from_str(json).unwrap();
        assert_eq!(object_name(&o), "Hello");
        let back = serde_json::to_value(&o).unwrap();
        assert_eq!(back["type"], "label");
        assert_eq!(back["group"], "g");
        assert!(back.get("hidden").is_none() && back.get("name").is_none());
        let again: MapObject = serde_json::from_value(back).unwrap();
        assert_eq!(again, o);
    }

    #[test]
    fn the_sample_project_reads() {
        let path = std::path::Path::new("/run/media/msstu/Duomo/RedM/RedForge/pry-rdrmapeditor/map-project.json");
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let v: Value = serde_json::from_str(&text).unwrap();
        let objects = v["objects"].as_array().unwrap().clone();
        let n = objects.len();
        let (objs, bad) = objects_from_json(v["version"].as_u64().unwrap() as u32, objects);
        assert_eq!((objs.len(), bad), (n, 0));
    }
}
