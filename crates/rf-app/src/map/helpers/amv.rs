//! "Ambient Mask Volume": the items of every placed ambient mask volume zone
//! (`RDR2AMVZone`, the `amv_zone_<n>` ymts), boxes as `RDR2AMVZoneItem`
//! makes them.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use glam::{EulerRot, Mat4, Quat, Vec3};
use rf_formats::formats::databag::DataBag;
use rf_formats::hash::{HashIndex, joaat};

use super::fields;
use super::shapes::Shapes;
use super::{Collect, HelperKey, HelperPart, Mode, PartInfo, rgba};
use crate::game::Game;

/// `RDR2AMVZoneItem`: a box (a unit box through `transform`), and what
/// Properties shows of it.
struct Volume {
    key: HelperKey,
    transform: Mat4,
    centre: Vec3,
    radius: f32,
    info: Arc<PartInfo>,
}

/// The zones' parts, made once their volumes are read (on a worker thread,
/// again when the game data changes).
#[derive(Default)]
pub struct AmbientMasks {
    version: Option<u64>,
    loading: Option<Receiver<Vec<Volume>>>,
    parts: Vec<HelperPart>,
}

impl AmbientMasks {
    pub fn collect(&mut self, c: &Collect, shapes: &mut Shapes, out: &mut Vec<HelperPart>) {
        let version = c.game.data_version();
        if self.version != Some(version) {
            self.version = Some(version);
            let (tx, rx) = channel();
            let game = c.game.clone();
            std::thread::spawn(move || tx.send(volumes(&game)));
            self.loading = Some(rx);
        }
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(volumes) => {
                    let mesh = shapes.unit_box(c.gpu, false);
                    let part = |v: Volume| HelperPart {
                        key: v.key,
                        mesh: mesh.clone(),
                        transform: v.transform,
                        colour: rgba(0.75, 0.75, 0.75, 1.0),
                        centre: v.centre,
                        radius: v.radius,
                        info: v.info,
                    };
                    self.parts = volumes.into_iter().map(part).collect();
                    self.loading = None;
                }
                Err(TryRecvError::Disconnected) => self.loading = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        out.extend(self.parts.iter().cloned());
    }
}

/// The volumes of `RDR2Map`'s placed zones: `amv_zone_0`, `amv_zone_1`... up
/// to the first one missing.
fn volumes(game: &Game) -> Vec<Volume> {
    let data = game.data();
    let mut out = Vec::new();
    let mut zones = 0;
    for i in 0.. {
        let hash = joaat(&format!("amv_zone_{i}"));
        let Some(f) = data.stream_entry("ymt", hash) else { break };
        let Some(doc) = rf_game::load_doc(&game.fs, &game.hashes, f) else { break };
        let zone = super::scenario::file_name(game, f);
        // Numbered as in the list, missing items too.
        let items = doc.bag.bags(AMV_LIST).into_iter().flatten().enumerate();
        for (k, b) in items.filter_map(|(k, b)| Some((k, b.as_ref()?))) {
            let key = HelperKey { mode: Mode::AmbientMaskVolume, source: hash, group: 0, index: k as u32 };
            out.push(volume(key, b, &zone, &game.hashes));
        }
        zones += 1;
    }
    log::info!("{zones} ambient mask volume zones, {} volumes", out.len());
    out
}

const AMV_LIST: u32 = 0xF9E7_4101; // "amvList"
const POSITION: u32 = 0x0116_6164; // "position"
const SCALE: u32 = 0x5003_2CDC; // "scale"
const ROTATION: u32 = 0x77DD_4010; // "rotation"
const BLENDING_MODE: u32 = 0x9B94_14CB; // "blendingMode"
const SOURCE_FOLDER: u32 = 0xCDF7_E0C2; // "sourceFolder"
const IPL_HASH: u32 = 0x2A85_20D8; // "iplHash"

/// `RDR2AMVZoneItem.GetFields`.
const FIELDS: [(u32, &str); 28] = [
    (0x5B12_990F, "enabled"),
    (0x9AD8_489E, "falloffScaleMin"),
    (0x8209_FBE6, "falloffScaleMax"),
    (0x9AE0_CFEF, "samplingOffsetStrength"),
    (0x6319_36CD, "falloffPower"),
    (0xD6FF_9582, "distance"),
    (0x4F39_D6A7, "cellCountX"),
    (0x3CF8_3224, "cellCountY"),
    (0xEFF9_9828, "cellCountZ"),
    (0xB7C2_57ED, "clipPlane0"),
    (0xC554_F312, "clipPlane1"),
    (0x1FF0_A850, "clipPlane2"),
    (0x3794_D798, "clipPlane3"),
    (0x0196_9CF3, "clipPlaneBlend0"),
    (0x106C_3A9E, "clipPlaneBlend1"),
    (0x1F11_57E8, "clipPlaneBlend2"),
    (0x2E1F_F605, "clipPlaneBlend3"),
    (BLENDING_MODE, "blendingMode"),
    (0xB531_7599, "layer"),
    (0x458F_864B, "order"),
    (0xFFE1_A346, "natural"),
    (0x403A_9A9E, "attachedToDoor"),
    (0x2169_93EA, "interior"),
    (0x3E5A_DF3C, "exterior"),
    (0x1AE1_333E, "vehicleInterior"),
    (SOURCE_FOLDER, "sourceFolder"),
    (0x5A7C_A146, "uuid"),
    (IPL_HASH, "iplHash"),
];

/// `RDR2AMVZoneBlendingMode`.
const BLENDING_MODES: [&str; 5] = ["Overwrite", "Lerp", "Min", "Max", "Multiply"];

/// The orientation of an item's `rotation`: euler degrees, with the Z
/// rotation in X ("for some reason", says the C#).
fn orientation(rotation: [f32; 3]) -> Quat {
    let [x, y, z] = rotation.map(f32::to_radians);
    // `CreateFromEulerAnglesDeg(rot.Y, rot.Z, -rot.X)`: roll, pitch, yaw.
    Quat::from_euler(EulerRot::ZYX, -x, z, y)
}

fn volume(key: HelperKey, b: &DataBag, zone: &str, hashes: &HashIndex) -> Volume {
    let (position, scale, rotation) = (b.vec3(POSITION), b.vec3(SCALE), b.vec3(ROTATION));
    let text = |v: [f32; 3]| v.map(|f| f.to_string()).join(", ");
    let mut list = vec![
        ("position".to_owned(), text(position)),
        ("scale".to_owned(), text(scale)),
        ("rotation".to_owned(), text(rotation)),
    ];
    for (hash, name) in FIELDS {
        let value = match hash {
            BLENDING_MODE => {
                let v = b.enum_value(hash);
                BLENDING_MODES.get(v as usize).map_or_else(|| v.to_string(), |s| (*s).to_owned())
            }
            // Hashes, though the files have `iplHash` as a number.
            SOURCE_FOLDER | IPL_HASH => match b.u32(hash) {
                0 => String::new(),
                h => hashes.name(h),
            },
            _ => fields::text(b, hash, None, hashes).unwrap_or_default(),
        };
        list.push((name.to_owned(), value));
    }
    let (centre, size) = (Vec3::from(position), Vec3::from(scale) * 2.0);
    let info = PartInfo {
        type_name: "Ambient Mask Volume".into(),
        name: format!("Ambient Mask Volume {}", key.index),
        owner: zone.to_owned(),
        fields: list,
    };
    Volume {
        key,
        transform: Mat4::from_scale_rotation_translation(size, orientation(rotation), centre),
        centre,
        radius: size.length() * 0.5,
        info: Arc::new(info),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `QuaternionExt.CreateFromEulerAnglesRad`.
    fn csharp_euler(e: Vec3) -> Quat {
        let (r, p, y) = (e.x * 0.5, e.y * 0.5, e.z * 0.5);
        let (cr, sr, cp, sp, cy, sy) = (r.cos(), r.sin(), p.cos(), p.sin(), y.cos(), y.sin());
        Quat::from_xyzw(
            sr * cp * cy - cr * sp * sy,
            cr * sp * cy + sr * cp * sy,
            cr * cp * sy - sr * sp * cy,
            cr * cp * cy + sr * sp * sy,
        )
    }

    #[test]
    fn orientation_as_the_csharp() {
        for rot in [[0.0, 0.0, 90.0], [30.0, -45.0, 10.0], [-170.0, 5.0, 120.0]] {
            let r = Vec3::from(rot);
            let want = csharp_euler(Vec3::new(r.y, r.z, -r.x) * std::f32::consts::PI / 180.0);
            let got = orientation(rot);
            assert!(got.dot(want).abs() > 0.99999, "{rot:?}: {got} vs {want}");
        }
    }
}
