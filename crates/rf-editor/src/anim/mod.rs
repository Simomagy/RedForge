//! Animations being made: poses keyed on a timeline ([`AnimSource`]), and
//! their baking into a clip dictionary the game plays ([`AnimSource::bake`]).
//!
//! A ycd holds a value for every frame and nothing of how it was made, so an
//! animation is kept as its source (the keys, as JSON) and the ycd is made
//! from it each time.
//!
//! - **The bones** are a ped's skeleton ([`Rig`]), whose rest pose is where
//!   posing starts.
//! - **Reaching** ([`ik::reach`]) poses an arm or a leg from where its hand or
//!   foot should be.
//! - **A base pose** ([`BasePose`]) is a moment of a game clip to show the
//!   other bones in.
//! - **Only the bones that are keyed get a track.** The game leaves the others
//!   as they are, which is what a gesture wants (an arm raised over the ped's
//!   own stance).
//! - **Keys** are Blender's: each number of a bone (its location's X, Y, Z,
//!   its rotation's W, X, Y, Z or Euler angles) has an F-curve
//!   ([`curve::FCurve`]) of keyframes with Bézier handles, and is counted
//!   from the bone's rest pose. Baking reads every frame off the curves and
//!   writes what the game wants: the bone from its parent, in the game's
//!   units and axes.
//! - **Time** is frames at 30 a second, the game's rate. The first frame is 0
//!   and an animation of `frames` frames lasts `(frames - 1) / 30` seconds.
//!
//! What the baked dictionary is, as the game was seen to accept (RedM, build
//! 1491): one `Single` clip without tags, properties or signature, playing
//! one animation with one sequence. Tracks are in the order the game's files
//! have them, positions before rotations, each by bone, then the ped's own place; the
//! root's rotation is always among them (see [`Mover`]). Of a rotation's four
//! numbers three are written, and the game works out the fourth as positive;
//! it's the one that keeps its sign and stays furthest from zero, as in the
//! game's files.

pub mod curve;
pub mod ik;
pub mod rig;

use glam::{Quat, Vec3};
use rf_formats::formats::XmlContext;
use rf_formats::hash::HashIndex;
pub use curve::{FCurve, Handle, Interpolation, Keyframe};
pub use rig::{Rig, RigBone};
use serde::{Deserialize, Serialize};

/// Frames a second: the rate the game's animations are sampled at.
pub const FPS: f32 = 30.0;

/// The most frames of an animation: a minute. The game's own long ones are a
/// single sequence too (974 frames seen), so a long one is baked as a short
/// one is.
pub const MAX_FRAMES: u16 = 1800;

/// An animation as it's made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimSource {
    /// The dictionary's name: the ycd's file name, and what scripts ask for
    /// (`RequestAnimDict`).
    pub dictionary: String,
    /// The clip's name, what scripts play (`TaskPlayAnim`).
    pub clip: String,
    /// How many frames, the first and the last included.
    pub frames: u16,
    #[serde(default)]
    pub bones: Vec<BoneKeys>,
    /// What the ped is doing under the animation, to see it as in the game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<BasePose>,
    /// Where the ped itself is under its root bone, when it isn't standing as
    /// usual: see [`Mover`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mover: Option<Mover>,
}

/// The ped's own place in an animation, which its root bone is given from
/// (the game's tracks 5 and 6 of bone 0).
///
/// The game's clips that move the root all have it, and the game plays an
/// animation without it, or with it and no rotation of the root, with the ped
/// turned half way round (seen in RedM, build 1491). So every animation is
/// baked with both: this one, or a standing ped's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mover {
    pub position: [f32; 3],
    /// x, y, z, w.
    pub rotation: [f32; 4],
}

impl Mover {
    /// A standing ped's, in most of the game's clips: a metre up, half a turn
    /// about the vertical.
    pub const STANDING: Mover = Mover { position: [0.0, 0.0, 1.0], rotation: [0.0, 0.0, 1.0, 0.0] };
}

/// The root bone's tag.
pub const ROOT: u16 = 0;

/// A moment of one of the game's clips: the pose the bones without keys are
/// shown in. The game leaves those bones as the ped has them, so this is only
/// for looking, and isn't baked; to have it in the animation, its bones are
/// keyed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BasePose {
    /// The clip dictionary's path in the game's archives.
    pub file: String,
    /// The clip's name, as it's listed (`walk`, for `pack:/walk.clip`).
    pub clip: String,
    /// Seconds into the clip.
    pub time: f32,
}

/// What a channel animates: Blender's data paths of a pose bone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Path {
    /// X, Y, Z, from the rest position, along the rest pose's axes.
    Location,
    /// W, X, Y, Z, from the rest rotation.
    RotationQuaternion,
    /// X, Y, Z in radians, from the rest rotation, in the bone's order.
    RotationEuler,
}

impl Path {
    /// How many numbers it has.
    pub fn size(self) -> u8 {
        if self == Path::RotationQuaternion { 4 } else { 3 }
    }

    /// A channel's name, as Blender lists it.
    pub fn label(self, index: u8) -> String {
        let axis = |i: u8| ["X", "Y", "Z"].get(usize::from(i)).copied().unwrap_or("?");
        match self {
            Path::Location => format!("{} Location", axis(index)),
            Path::RotationQuaternion => {
                format!("{} Quaternion Rotation", ["W", "X", "Y", "Z"].get(usize::from(index)).copied().unwrap_or("?"))
            }
            Path::RotationEuler => format!("{} Euler Rotation", axis(index)),
        }
    }

    /// The value of a bone at rest.
    pub fn rest(self, index: u8) -> f32 {
        if self == Path::RotationQuaternion && index == 0 { 1.0 } else { 0.0 }
    }
}

/// How a bone's rotation is keyed: Blender's rotation modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RotationMode {
    #[default]
    Quaternion,
    Xyz,
    Xzy,
    Yxz,
    Yzx,
    Zxy,
    Zyx,
}

impl RotationMode {
    pub const ALL: [RotationMode; 7] = [
        RotationMode::Quaternion,
        RotationMode::Xyz,
        RotationMode::Xzy,
        RotationMode::Yxz,
        RotationMode::Yzx,
        RotationMode::Zxy,
        RotationMode::Zyx,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RotationMode::Quaternion => "Quaternion (WXYZ)",
            RotationMode::Xyz => "XYZ Euler",
            RotationMode::Xzy => "XZY Euler",
            RotationMode::Yxz => "YXZ Euler",
            RotationMode::Yzx => "YZX Euler",
            RotationMode::Zxy => "ZXY Euler",
            RotationMode::Zyx => "ZYX Euler",
        }
    }

    /// The channels a rotation is keyed in.
    pub fn path(self) -> Path {
        if self == RotationMode::Quaternion { Path::RotationQuaternion } else { Path::RotationEuler }
    }

    /// Blender's "XYZ" turns about X first, then Y, then Z, about the fixed
    /// axes: glam's ZYX (its letters go from the last turn applied).
    fn euler(self) -> glam::EulerRot {
        match self {
            RotationMode::Quaternion | RotationMode::Xyz => glam::EulerRot::ZYX,
            RotationMode::Xzy => glam::EulerRot::YZX,
            RotationMode::Yxz => glam::EulerRot::ZXY,
            RotationMode::Yzx => glam::EulerRot::XZY,
            RotationMode::Zxy => glam::EulerRot::YXZ,
            RotationMode::Zyx => glam::EulerRot::XYZ,
        }
    }

    /// The three letters' axes, first turn first.
    fn axes(self) -> [usize; 3] {
        match self {
            RotationMode::Quaternion | RotationMode::Xyz => [0, 1, 2],
            RotationMode::Xzy => [0, 2, 1],
            RotationMode::Yxz => [1, 0, 2],
            RotationMode::Yzx => [1, 2, 0],
            RotationMode::Zxy => [2, 0, 1],
            RotationMode::Zyx => [2, 1, 0],
        }
    }

    /// A rotation as this mode's numbers: W, X, Y, Z, or the X, Y, Z angles.
    pub fn values(self, q: Quat) -> Vec<f32> {
        if self == RotationMode::Quaternion {
            return vec![q.w, q.x, q.y, q.z];
        }
        // glam gives the angles last turn first.
        let (c, b, a) = q.to_euler(self.euler());
        let mut xyz = [0.0; 3];
        for (axis, angle) in self.axes().into_iter().zip([a, b, c]) {
            xyz[axis] = angle;
        }
        xyz.to_vec()
    }

    /// The rotation of this mode's numbers.
    pub fn rotation(self, v: &[f32]) -> Quat {
        if self == RotationMode::Quaternion {
            let q = Quat::from_xyzw(v[1], v[2], v[3], v[0]);
            return if q.length_squared() < 1e-12 { Quat::IDENTITY } else { q.normalize() };
        }
        let [a, b, c] = self.axes().map(|axis| v[axis]);
        Quat::from_euler(self.euler(), c, b, a)
    }
}

/// One animated number of a bone: Blender's F-curve with its data path and
/// array index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub path: Path,
    pub index: u8,
    #[serde(flatten)]
    pub curve: FCurve,
    /// Not drawn in the Graph Editor (not saved).
    #[serde(skip)]
    pub hidden: bool,
}

/// One bone's animation.
///
/// As in Blender, what's keyed is the bone from its rest pose: a location of
/// nothing and a rotation of nothing are the bone at rest. The rest pose is
/// kept here for the file to be baked without the skeleton.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoneKeys {
    /// The bone's tag, as in the skeleton and in a ycd's bone ids.
    pub bone: u16,
    /// The bone's name, for whoever reads the file. Not used.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The bone at rest, from its parent: x, y, z, w.
    #[serde(default = "identity")]
    pub rest_rotation: [f32; 4],
    #[serde(default)]
    pub rest_position: [f32; 3],
    #[serde(default)]
    pub rotation_mode: RotationMode,
    #[serde(default)]
    pub channels: Vec<Channel>,
    /// The keys of the files before F-curves: read, turned into channels,
    /// never written.
    #[serde(default, skip_serializing)]
    rotations: Vec<OldKey<[f32; 4]>>,
    #[serde(default, skip_serializing)]
    positions: Vec<OldKey<[f32; 3]>>,
}

fn identity() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

impl Default for BoneKeys {
    fn default() -> Self {
        BoneKeys {
            bone: 0,
            name: String::new(),
            rest_rotation: identity(),
            rest_position: [0.0; 3],
            rotation_mode: RotationMode::default(),
            channels: Vec::new(),
            rotations: Vec::new(),
            positions: Vec::new(),
        }
    }
}

/// A key of the files before F-curves: a whole rotation or position, as the
/// game has it (not from the rest pose).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
struct OldKey<T> {
    frame: u16,
    value: T,
    #[serde(default)]
    ease: OldEase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OldEase {
    #[default]
    Smooth,
    Linear,
    Hold,
}

/// What's wrong with an animation that can't be baked.
#[derive(Debug, PartialEq)]
pub enum AnimError {
    /// A name is empty, or has characters a dictionary or clip name can't.
    Name(String),
    /// Fewer than 2 frames, or more than [`MAX_FRAMES`].
    Frames(u16),
    /// No bone has a key.
    Empty,
    /// A bone is there twice.
    BoneTwice(u16),
    /// Writing the dictionary failed.
    Format(String),
}

impl std::fmt::Display for AnimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnimError::Name(n) => write!(f, "\"{n}\" can't be a name: letters, digits, _ @ and - only"),
            AnimError::Frames(n) => write!(f, "{n} frames: an animation has 2 to {MAX_FRAMES}"),
            AnimError::Empty => write!(f, "no bone has a key"),
            AnimError::BoneTwice(b) => write!(f, "bone {b} is there twice"),
            AnimError::Format(e) => write!(f, "writing the dictionary: {e}"),
        }
    }
}

impl std::error::Error for AnimError {}

impl BoneKeys {
    /// A bone with nothing keyed, resting as given (from its parent).
    pub fn new(bone: u16, name: &str, rest_rotation: Quat, rest_position: Vec3) -> Self {
        BoneKeys {
            bone,
            name: name.to_owned(),
            rest_rotation: rest_rotation.to_array(),
            rest_position: rest_position.to_array(),
            ..Default::default()
        }
    }

    pub fn channel(&self, path: Path, index: u8) -> Option<&Channel> {
        self.channels.iter().find(|c| c.path == path && c.index == index)
    }

    pub fn channel_mut(&mut self, path: Path, index: u8) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.path == path && c.index == index)
    }

    /// The channel, made when it isn't there. Channels stay in Blender's
    /// order: location, then rotation, each by index.
    pub fn channel_or_new(&mut self, path: Path, index: u8) -> &mut Channel {
        let order = |c: &Channel| (c.path != Path::Location, c.index);
        let at = match self.channels.iter().position(|c| c.path == path && c.index == index) {
            Some(i) => i,
            None => {
                let new = Channel { path, index, curve: FCurve::default(), hidden: false };
                let i = self.channels.partition_point(|c| order(c) < order(&new));
                self.channels.insert(i, new);
                i
            }
        };
        &mut self.channels[at]
    }

    /// Whether anything has a key.
    pub fn is_keyed(&self) -> bool {
        self.channels.iter().any(|c| !c.curve.keys.is_empty())
    }

    /// Whether a path has keys.
    pub fn has(&self, path: Path) -> bool {
        self.channels.iter().any(|c| c.path == path && !c.curve.keys.is_empty())
    }

    /// The channels without keys go.
    pub fn prune(&mut self) {
        self.channels.retain(|c| !c.curve.keys.is_empty());
    }

    /// One number at a frame: the channel's, or the rest value without it.
    pub fn value(&self, path: Path, index: u8, frame: f32) -> f32 {
        self.channel(path, index).and_then(|c| c.curve.evaluate(frame)).unwrap_or(path.rest(index))
    }

    /// The rotation from rest at a frame, or `None` with no rotation keyed.
    pub fn turn(&self, frame: f32) -> Option<Quat> {
        let path = self.rotation_mode.path();
        if !self.has(path) {
            return None;
        }
        let v: Vec<f32> = (0..path.size()).map(|i| self.value(path, i, frame)).collect();
        Some(self.rotation_mode.rotation(&v))
    }

    /// The location from rest at a frame, or `None` with no location keyed.
    pub fn location(&self, frame: f32) -> Option<Vec3> {
        self.has(Path::Location).then(|| Vec3::from_array([0, 1, 2].map(|i| self.value(Path::Location, i, frame))))
    }

    fn rest(&self) -> (Quat, Vec3) {
        (Quat::from_array(self.rest_rotation).normalize(), Vec3::from_array(self.rest_position))
    }

    /// The bone's rotation from its parent at a frame, as the game has it,
    /// or `None` with no rotation keyed.
    pub fn rotation(&self, frame: f32) -> Option<Quat> {
        Some((self.rest().0 * self.turn(frame)?).normalize())
    }

    /// The bone's position from its parent at a frame, or `None` with no
    /// location keyed.
    pub fn position(&self, frame: f32) -> Option<Vec3> {
        let (rotation, position) = self.rest();
        Some(position + rotation * self.location(frame)?)
    }

    /// The bone's rotation from its parent, of this bone's rotation numbers
    /// from rest (W, X, Y, Z or the angles).
    pub fn local_rotation(&self, values: &[f32]) -> Quat {
        (self.rest().0 * self.rotation_mode.rotation(values)).normalize()
    }

    /// The bone's position from its parent, of a location from rest.
    pub fn local_position(&self, location: Vec3) -> Vec3 {
        let (rotation, position) = self.rest();
        position + rotation * location
    }

    /// A rotation from the parent as this bone's numbers from rest: W, X, Y,
    /// Z or the angles. A quaternion comes on the side of `near` (the curves
    /// then don't jump to the same rotation's other four numbers), angles
    /// within half a turn of it.
    pub fn turn_values(&self, rotation: Quat, near: &[f32]) -> Vec<f32> {
        let turn = (self.rest().0.inverse() * rotation).normalize();
        let mut v = self.rotation_mode.values(turn);
        if self.rotation_mode == RotationMode::Quaternion {
            if v.iter().zip(near).map(|(a, b)| a * b).sum::<f32>() < 0.0 {
                v.iter_mut().for_each(|c| *c = -*c);
            }
        } else {
            for (a, n) in v.iter_mut().zip(near) {
                *a += ((n - *a) / std::f32::consts::TAU).round() * std::f32::consts::TAU;
            }
        }
        v
    }

    /// A position from the parent as this bone's location from rest.
    pub fn location_values(&self, position: Vec3) -> [f32; 3] {
        let (rotation, rest) = self.rest();
        (rotation.inverse() * (position - rest)).to_array()
    }

    /// Keys the rotation (from the parent) at a frame, in every rotation channel.
    pub fn key_rotation(&mut self, frame: f32, rotation: Quat) {
        let path = self.rotation_mode.path();
        let near: Vec<f32> = (0..path.size()).map(|i| self.value(path, i, frame)).collect();
        for (i, v) in self.turn_values(rotation, &near).into_iter().enumerate() {
            self.channel_or_new(path, i as u8).curve.insert(frame, v);
        }
    }

    /// Keys the position (from the parent) at a frame, in every location channel.
    pub fn key_position(&mut self, frame: f32, position: Vec3) {
        for (i, v) in self.location_values(position).into_iter().enumerate() {
            self.channel_or_new(Path::Location, i as u8).curve.insert(frame, v);
        }
    }

    /// Changes how the rotation is keyed, the keys with it: at every frame a
    /// rotation channel has a key, the same rotation in the new mode's
    /// channels (their handles automatic).
    pub fn set_rotation_mode(&mut self, mode: RotationMode) {
        if mode == self.rotation_mode {
            return;
        }
        let old = self.rotation_mode.path();
        let mut frames: Vec<f32> =
            self.channels.iter().filter(|c| c.path == old).flat_map(|c| c.curve.keys.iter().map(|k| k.frame)).collect();
        frames.sort_by(f32::total_cmp);
        frames.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
        let (rest, _) = self.rest();
        let turns: Vec<(f32, Quat)> = frames.iter().filter_map(|f| Some((*f, rest * self.turn(*f)?))).collect();
        self.channels.retain(|c| c.path != old);
        self.rotation_mode = mode;
        for (frame, rotation) in turns {
            self.key_rotation(frame, rotation);
        }
    }

    /// The frames with a key in any channel, in order.
    pub fn key_frames(&self) -> Vec<f32> {
        let mut frames: Vec<f32> = self.channels.iter().flat_map(|c| c.curve.keys.iter().map(|k| k.frame)).collect();
        frames.sort_by(f32::total_cmp);
        frames.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
        frames
    }

    /// Stands the bone on another rest pose, what it does staying the same:
    /// every key's rotation and position worked out again from it. For the
    /// files before F-curves, whose keys weren't from a rest pose.
    pub fn rebase(&mut self, rest_rotation: Quat, rest_position: Vec3) {
        let frames = self.key_frames();
        let poses: Vec<(f32, Option<Quat>, Option<Vec3>)> =
            frames.iter().map(|f| (*f, self.rotation(*f), self.position(*f))).collect();
        type Kinds = Vec<(f32, Interpolation)>;
        let kinds: Vec<(Path, u8, Kinds)> = self
            .channels
            .iter()
            .map(|c| (c.path, c.index, c.curve.keys.iter().map(|k| (k.frame, k.interpolation)).collect()))
            .collect();
        self.channels.clear();
        (self.rest_rotation, self.rest_position) = (rest_rotation.to_array(), rest_position.to_array());
        for (frame, rotation, position) in poses {
            if let Some(r) = rotation {
                self.key_rotation(frame, r);
            }
            if let Some(p) = position {
                self.key_position(frame, p);
            }
        }
        // As they went from key to key.
        for (path, index, keys) in kinds {
            let Some(c) = self.channel_mut(path, index) else { continue };
            for (frame, interpolation) in keys {
                if let Some(i) = c.curve.key_at(frame) {
                    c.curve.keys[i].interpolation = interpolation;
                }
            }
        }
    }

    /// Whether the file had the keys of before F-curves (they're channels
    /// now, from a rest pose of nothing: see [`BoneKeys::rebase`]).
    pub fn was_old(&self) -> bool {
        self.rest_rotation == identity() && self.rest_position == [0.0; 3]
    }

    /// The keys of the files before F-curves, as channels.
    fn convert_old(&mut self) {
        let kind = |e: OldEase| match e {
            OldEase::Smooth => Interpolation::Bezier,
            OldEase::Linear => Interpolation::Linear,
            OldEase::Hold => Interpolation::Constant,
        };
        let (mut rotations, mut positions) = (std::mem::take(&mut self.rotations), std::mem::take(&mut self.positions));
        rotations.sort_by_key(|k| k.frame);
        positions.sort_by_key(|k| k.frame);
        for k in &rotations {
            self.key_rotation(f32::from(k.frame), Quat::from_array(k.value).normalize());
        }
        for k in &positions {
            self.key_position(f32::from(k.frame), Vec3::from_array(k.value));
        }
        for c in &mut self.channels {
            for key in &mut c.curve.keys {
                let ease = if c.path == Path::Location {
                    positions.iter().find(|k| f32::from(k.frame) == key.frame).map(|k| k.ease)
                } else {
                    rotations.iter().find(|k| f32::from(k.frame) == key.frame).map(|k| k.ease)
                };
                if let Some(e) = ease {
                    key.interpolation = kind(e);
                }
            }
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_@-".contains(&b))
}

/// The game's sequence size for an animation of one sequence: the frames
/// rounded up to 16, less one (24 frames: 31, 72: 79).
fn sequence_size(frames: u16) -> u16 {
    (frames + 16) / 16 * 16 - 1
}

fn number(v: f32) -> String {
    // A float's shortest text that reads back as itself; no exponents below
    // 1e-5, which `{}` writes as 0.00001.
    if v == 0.0 { "0".into() } else { format!("{v}") }
}

/// A number over the frames: one value when it never changes.
fn float_channel(values: &[f32], xml: &mut String) {
    let (min, max) = values.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    if max - min < 1e-7 {
        xml.push_str(&format!("<Item type=\"StaticFloat\"><Value value=\"{}\" /></Item>", number(values[0])));
    } else {
        xml.push_str("<Item type=\"QuantizeFloat\"><Values>");
        for v in values {
            xml.push_str(&number(*v));
            xml.push(' ');
        }
        xml.push_str("</Values></Item>");
    }
}

/// A rotation track. Its frames are first made to follow each other (a
/// quaternion and its negative are the same rotation, and the game blends
/// between frames number by number); then the component that keeps its sign
/// and stays furthest from zero is left out, positive, for the game to work
/// out. When every component crosses zero, all four are written.
fn rotation_track(frames: &[Quat], xml: &mut String) {
    let mut q: Vec<[f32; 4]> = Vec::with_capacity(frames.len());
    for f in frames {
        let mut a = f.to_array();
        if let Some(prev) = q.last()
            && prev.iter().zip(&a).map(|(x, y)| x * y).sum::<f32>() < 0.0
        {
            a = a.map(|c| -c);
        }
        q.push(a);
    }
    let steady = |i: usize| {
        let (lo, hi) = q.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v[i]), hi.max(v[i])));
        if lo >= 0.0 {
            Some((lo, 1.0))
        } else if hi <= 0.0 {
            Some((-hi, -1.0))
        } else {
            None
        }
    };
    let left_out =
        (0..4).filter_map(|i| steady(i).map(|(least, sign)| (i, least, sign))).max_by(|a, b| a.1.total_cmp(&b.1));
    xml.push_str("<Item><Channels>");
    match left_out {
        Some((out, _, sign)) => {
            for i in (0..4).filter(|i| *i != out) {
                float_channel(&q.iter().map(|v| v[i] * sign).collect::<Vec<_>>(), xml);
            }
            xml.push_str(&format!("<Item type=\"CachedQuaternion1\"><QuatIndex value=\"{out}\" /></Item>"));
        }
        None => {
            for i in 0..4 {
                float_channel(&q.iter().map(|v| v[i]).collect::<Vec<_>>(), xml);
            }
        }
    }
    xml.push_str("</Channels></Item>");
}

fn position_track(frames: &[Vec3], xml: &mut String) {
    xml.push_str("<Item><Channels>");
    for i in 0..3 {
        float_channel(&frames.iter().map(|v| v[i]).collect::<Vec<_>>(), xml);
    }
    xml.push_str("</Channels></Item>");
}

impl AnimSource {
    /// The animation's length in seconds.
    pub fn duration(&self) -> f32 {
        f32::from(self.frames.saturating_sub(1)) / FPS
    }

    /// The source from its JSON.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        // Scale was keyed for a while, and the game takes none from a clip
        // (not on a ped's bones, nor on its SCALE_ ones: tried in RedM, build
        // 1491): those channels are left out.
        let mut json: serde_json::Value = serde_json::from_str(text)?;
        for bone in json.get_mut("bones").and_then(|b| b.as_array_mut()).into_iter().flatten() {
            if let Some(channels) = bone.get_mut("channels").and_then(|c| c.as_array_mut()) {
                channels.retain(|c| c.get("path").and_then(|p| p.as_str()) != Some("scale"));
            }
        }
        let mut source: AnimSource = serde_json::from_value(json)?;
        for b in &mut source.bones {
            b.convert_old();
            b.channels.iter_mut().for_each(|c| c.curve.tidy());
        }
        Ok(source)
    }

    /// The source as JSON, for a file.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("an animation is always JSON")
    }

    /// What can't be baked, if anything.
    pub fn check(&self) -> Result<(), AnimError> {
        for name in [&self.dictionary, &self.clip] {
            if !valid_name(name) {
                return Err(AnimError::Name(name.clone()));
            }
        }
        if !(2..=MAX_FRAMES).contains(&self.frames) {
            return Err(AnimError::Frames(self.frames));
        }
        if !self.bones.iter().any(BoneKeys::is_keyed) {
            return Err(AnimError::Empty);
        }
        for (i, b) in self.bones.iter().enumerate() {
            if self.bones[..i].iter().any(|o| o.bone == b.bone) {
                return Err(AnimError::BoneTwice(b.bone));
            }
        }
        Ok(())
    }

    /// The clip dictionary as CodeX XML, which [`rf_formats`] turns into a ycd.
    fn to_xml(&self) -> String {
        let frames = (0..self.frames).map(f32::from);
        // The game's order: positions (track 0), then rotations (track 1), each by bone.
        let mut bones: Vec<&BoneKeys> = self.bones.iter().collect();
        bones.sort_by_key(|b| b.bone);
        let (mut ids, mut tracks) = (String::new(), String::new());
        let id = |bone: u16, kind: u8, track: u8| {
            let mut s = String::from("<Item>");
            for (tag, v) in [("BoneId", u32::from(bone)), ("TypeId", u32::from(kind)), ("TrackId", u32::from(track))] {
                if v != 0 {
                    s.push_str(&format!("<{tag} value=\"{v}\" />"));
                }
            }
            s + "</Item>"
        };
        for b in bones.iter().filter(|b| b.has(Path::Location)) {
            ids.push_str(&id(b.bone, 0, 0));
            position_track(&frames.clone().filter_map(|f| b.position(f)).collect::<Vec<_>>(), &mut tracks);
        }
        // The root's rotation is always there, with the ped's own place below:
        // the game plays an animation without both (one of an arm alone,
        // say) with the ped turned half way round. Not keyed, it's the root
        // of a ped standing where it is.
        let turned = |b: &BoneKeys| b.has(b.rotation_mode.path());
        if !bones.iter().any(|b| b.bone == ROOT && turned(b)) {
            ids.push_str(&id(ROOT, 1, 1));
            rotation_track(&[Quat::IDENTITY], &mut tracks);
        }
        for b in bones.iter().filter(|b| turned(b)) {
            ids.push_str(&id(b.bone, 1, 1));
            rotation_track(&frames.clone().filter_map(|f| b.rotation(f)).collect::<Vec<_>>(), &mut tracks);
        }
        // The ped's own place, after them.
        {
            let Mover { position: [px, py, pz], rotation: [x, y, z, w] } = self.mover.unwrap_or(Mover::STANDING);
            ids.push_str(&id(ROOT, 0, 5));
            tracks.push_str(&format!(
                "<Item><Channels><Item type=\"StaticVector3\"><Value x=\"{}\" y=\"{}\" z=\"{}\" /></Item></Channels></Item>",
                number(px),
                number(py),
                number(pz)
            ));
            ids.push_str(&id(ROOT, 1, 6));
            tracks.push_str(&format!(
                "<Item><Channels><Item type=\"StaticQuaternion\"><Value x=\"{}\" y=\"{}\" z=\"{}\" w=\"{}\" /></Item></Channels></Item>",
                number(x),
                number(y),
                number(z),
                number(w)
            ));
        }
        let (dict, clip, n, duration) = (&self.dictionary, &self.clip, self.frames, number(self.duration()));
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<RDR2ClipDictionary version="0">
<Name>{dict}</Name>
<Clips><Item type="Single">
<Name>pack:/{clip}.clip</Name>
<Dict>{dict}</Dict>
<Unknown_3C value="1" />
<Properties />
<Animation>{clip}</Animation>
<StartTime value="0" />
<EndTime value="{duration}" />
<Speed value="1" />
</Item></Clips>
<Animations><Item><Name>{clip}</Name><DictIndex value="0" /></Item></Animations>
<AnimDict version="0">
<Name>{dict}-0</Name>
<Animations><Item>
<Name>pack:/{clip}.anim</Name>
<Flags value="256" />
<DictIndex value="0" />
<FrameCount value="{n}" />
<SequenceSize value="{size}" />
<Duration value="{duration}" />
<BoneIds>{ids}</BoneIds>
<Sequences><Item><FrameCount value="{n}" /><Tracks>{tracks}</Tracks></Item></Sequences>
</Item></Animations>
</AnimDict>
</RDR2ClipDictionary>
"#,
            size = sequence_size(self.frames),
        )
    }

    /// The ycd the game plays: `<dictionary>.ycd`, for a resource's `stream`.
    pub fn bake(&self) -> Result<Vec<u8>, AnimError> {
        self.check()?;
        let cx = XmlContext { hashes: &HashIndex::new(), folder: None };
        rf_formats::formats::ycd::from_xml(&self.to_xml(), &cx).map_err(|e| AnimError::Format(format!("{e:#}")))
    }
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::ycd;

    use super::*;

    const ARM: u16 = 40269;
    const FOREARM: u16 = 46065;

    fn turn_z(degrees: f32) -> Quat {
        Quat::from_rotation_z(degrees.to_radians())
    }

    /// The arm resting a quarter turned, moved and turned; the forearm turned.
    fn raise() -> AnimSource {
        let rest = Quat::from_rotation_x(0.5);
        let mut arm = BoneKeys::new(ARM, "SKEL_R_UpperArm", rest, Vec3::new(0.1, 0.2, 0.3));
        for (frame, degrees) in [(0.0, 0.0), (30.0, -80.0), (60.0, 0.0)] {
            arm.key_rotation(frame, rest * turn_z(degrees));
        }
        arm.key_position(0.0, Vec3::new(0.1, 0.2, 0.3));
        arm.key_position(60.0, Vec3::new(0.1, 0.2, 0.5));
        let mut forearm = BoneKeys::new(FOREARM, "SKEL_R_Forearm", Quat::IDENTITY, Vec3::ZERO);
        forearm.rotation_mode = RotationMode::Xyz;
        for (frame, degrees) in [(0.0, 10.0), (60.0, -50.0)] {
            forearm.key_rotation(frame, turn_z(degrees));
        }
        AnimSource {
            dictionary: "rf@test".into(),
            clip: "raise".into(),
            frames: 61,
            bones: vec![arm, forearm],
            base: None,
            mover: None,
        }
    }

    #[test]
    fn keys_are_from_the_rest_pose() {
        let a = raise();
        let arm = &a.bones[0];
        // At rest: a rotation of nothing, a location of nothing.
        assert!((arm.value(Path::RotationQuaternion, 0, 0.0) - 1.0).abs() < 1e-6);
        assert!(arm.location(0.0).unwrap().abs_diff_eq(Vec3::ZERO, 1e-6));
        // And back to the bone from its parent.
        let rest = Quat::from_rotation_x(0.5);
        assert!(arm.rotation(30.0).unwrap().abs_diff_eq(rest * turn_z(-80.0), 1e-5));
        assert!(arm.position(60.0).unwrap().abs_diff_eq(Vec3::new(0.1, 0.2, 0.5), 1e-6));
        // The location is along the rest pose's axes.
        let l = arm.location(60.0).unwrap();
        assert!((rest * l).abs_diff_eq(Vec3::new(0.0, 0.0, 0.2), 1e-6));
        // Euler channels are angles.
        let forearm = &a.bones[1];
        assert!((forearm.value(Path::RotationEuler, 2, 60.0) - (-50.0f32).to_radians()).abs() < 1e-5);
        assert_eq!(forearm.channels.len(), 3);
        // A bone with nothing keyed has no rotation to give.
        assert_eq!(BoneKeys::default().rotation(0.0), None);
    }

    #[test]
    fn every_rotation_mode_gives_the_rotation_back() {
        let q = Quat::from_euler(glam::EulerRot::XYZ, 0.4, -0.9, 1.3);
        for mode in RotationMode::ALL {
            let back = mode.rotation(&mode.values(q));
            assert!(back.abs_diff_eq(q, 1e-5) || back.abs_diff_eq(-q, 1e-5), "{mode:?}");
        }
        // Blender's XYZ: about X, then Y, then Z, each about the fixed axes.
        let v = [0.3, 0.5, 0.7];
        let want = Quat::from_rotation_z(0.7) * Quat::from_rotation_y(0.5) * Quat::from_rotation_x(0.3);
        assert!(RotationMode::Xyz.rotation(&v).abs_diff_eq(want, 1e-6));
        let want = Quat::from_rotation_x(0.3) * Quat::from_rotation_y(0.5) * Quat::from_rotation_z(0.7);
        assert!(RotationMode::Zyx.rotation(&v).abs_diff_eq(want, 1e-6));
    }

    #[test]
    fn changing_the_rotation_mode_keeps_the_keys() {
        let mut a = raise();
        let before: Vec<Quat> = [0.0, 30.0, 60.0].map(|f| a.bones[0].rotation(f).unwrap()).to_vec();
        a.bones[0].set_rotation_mode(RotationMode::Yxz);
        assert_eq!(a.bones[0].channels.iter().filter(|c| c.path == Path::RotationEuler).count(), 3);
        assert!(!a.bones[0].has(Path::RotationQuaternion) && a.bones[0].has(Path::Location));
        for (f, q) in [0.0, 30.0, 60.0].into_iter().zip(before) {
            let now = a.bones[0].rotation(f).unwrap();
            assert!(now.abs_diff_eq(q, 1e-4) || now.abs_diff_eq(-q, 1e-4), "at {f}");
        }
    }

    #[test]
    fn quaternion_keys_stay_on_one_side() {
        let mut b = BoneKeys::new(1, "", Quat::IDENTITY, Vec3::ZERO);
        b.key_rotation(0.0, turn_z(170.0));
        // The same rotation written the other way round is keyed this way round.
        b.key_rotation(10.0, -turn_z(175.0));
        let w = |f| b.value(Path::RotationQuaternion, 0, f);
        assert!(w(0.0) > 0.0 && w(10.0) > 0.0);
    }

    #[test]
    fn baked_dictionary_reads_back() {
        let source = raise();
        let file = source.bake().unwrap();
        let hashes = HashIndex::new();
        let dict = ycd::read(&file, &hashes).unwrap();
        assert_eq!(dict.name.as_deref(), Some("rf@test"));
        assert_eq!(dict.clips().len(), 1);
        let anims = dict.anims();
        let anim = anims.values().next().unwrap();
        assert_eq!((anim.frame_count, anim.sequence_size, anim.flags), (61, 63, 256));
        assert!((anim.duration - 2.0).abs() < 1e-6);

        // Positions first, then rotations (the root's always), each by bone; then the ped's own place.
        let ids: Vec<(u16, u8, u8)> =
            anim.bone_ids.as_ref().unwrap().iter().map(|b| (b.bone_id, b.type_id, b.track_id)).collect();
        assert_eq!(ids, [(ARM, 0, 0), (0, 1, 1), (ARM, 1, 1), (FOREARM, 1, 1), (0, 0, 5), (0, 1, 6)]);

        // Every frame of every track is what the curves say, to the quantisation.
        let by_bone = |bone: u16| source.bones.iter().find(|b| b.bone == bone).unwrap();
        for frame in 0..source.frames {
            let at = anim.frame_position(f32::from(frame) / FPS);
            let p = anim.evaluate(&at, 0);
            let want = by_bone(ARM).position(f32::from(frame)).unwrap();
            assert!(Vec3::new(p[0], p[1], p[2]).abs_diff_eq(want, 1e-4), "position at {frame}");
            for (track, bone) in [(2, ARM), (3, FOREARM)] {
                let q = Quat::from_array(anim.evaluate(&at, track));
                let want = by_bone(bone).rotation(f32::from(frame)).unwrap();
                assert!(q.abs_diff_eq(want, 2e-3) || q.abs_diff_eq(-want, 2e-3), "bone {bone} at {frame}: {q} for {want}");
            }
        }
    }

    #[test]
    fn what_cant_be_baked() {
        let mut a = raise();
        a.dictionary = "Bad Name".into();
        assert_eq!(a.check(), Err(AnimError::Name("Bad Name".into())));
        let mut a = raise();
        a.frames = MAX_FRAMES + 1;
        assert_eq!(a.check(), Err(AnimError::Frames(MAX_FRAMES + 1)));
        let mut a = raise();
        a.bones.iter_mut().for_each(|b| b.channels.clear());
        assert_eq!(a.check(), Err(AnimError::Empty));
        let mut a = raise();
        a.bones.push(a.bones[0].clone());
        assert_eq!(a.check(), Err(AnimError::BoneTwice(ARM)));
        // Keys past the last frame are fine, as in Blender: they're not played.
        let mut a = raise();
        a.frames = 20;
        assert_eq!(a.check(), Ok(()));
    }

    #[test]
    fn json_and_back() {
        let mut a = raise();
        a.base = Some(BasePose { file: "anim_0.rpf/a/b.ycd".into(), clip: "idle".into(), time: 0.5 });
        a.bones[0].channels[0].curve.keys[0].interpolation = Interpolation::Linear;
        let back = AnimSource::from_json(&a.to_json()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn files_before_curves_still_open() {
        let old = r#"{"dictionary":"rf@old","clip":"c","frames":61,"bones":[
            {"bone":40269,"name":"SKEL_R_UpperArm",
             "rotations":[{"frame":0,"value":[0,0,0,1],"ease":"linear"},{"frame":30,"value":[0,0,0.6427876,0.7660444],"ease":"hold"}],
             "positions":[{"frame":0,"value":[0.1,0.2,0.3]}]}]}"#;
        let mut a = AnimSource::from_json(old).unwrap();
        let b = &a.bones[0];
        assert!(b.was_old());
        assert!(b.rotation(30.0).unwrap().abs_diff_eq(turn_z(80.0), 1e-5));
        assert!(b.position(10.0).unwrap().abs_diff_eq(Vec3::new(0.1, 0.2, 0.3), 1e-6));
        // As they went from key to key.
        assert!(b.rotation(15.0).unwrap().abs_diff_eq(turn_z(40.0), 2e-2));
        assert!(b.rotation(45.0).unwrap().abs_diff_eq(turn_z(80.0), 1e-5));
        let w = b.channel(Path::RotationQuaternion, 0).unwrap();
        assert_eq!(w.curve.keys[0].interpolation, Interpolation::Linear);
        assert_eq!(w.curve.keys[1].interpolation, Interpolation::Constant);
        a.bake().unwrap();

        // On the skeleton's rest pose it's the same animation.
        let rest = Quat::from_rotation_y(0.7);
        a.bones[0].rebase(rest, Vec3::new(0.1, 0.0, 0.0));
        let b = &a.bones[0];
        assert!(!b.was_old());
        assert!(b.rotation(30.0).unwrap().abs_diff_eq(turn_z(80.0), 1e-5));
        assert!(b.position(0.0).unwrap().abs_diff_eq(Vec3::new(0.1, 0.2, 0.3), 1e-6));
        assert_eq!(b.channel(Path::RotationQuaternion, 0).unwrap().curve.keys[1].interpolation, Interpolation::Constant);
    }

    #[test]
    fn the_peds_own_place_is_always_there() {
        let ids = |a: &AnimSource| {
            let dict = ycd::read(&a.bake().unwrap(), &HashIndex::new()).unwrap();
            let anims = dict.anims();
            let anim = anims.values().next().unwrap();
            let at = anim.frame_position(0.1);
            let ids = anim.bone_ids.as_ref().unwrap();
            let i = ids.iter().position(|b| b.track_id == 6).unwrap();
            assert_eq!(anim.evaluate(&at, i), [0.0, 0.0, 1.0, 0.0]);
            assert_eq!(anim.evaluate(&at, i - 1)[..3], [0.0, 0.0, 1.0]);
            ids.iter().map(|b| (b.bone_id, b.type_id, b.track_id)).collect::<Vec<_>>()
        };
        let key = |bone: u16| {
            let mut b = BoneKeys::new(bone, "", Quat::IDENTITY, Vec3::ZERO);
            b.key_rotation(0.0, Quat::IDENTITY);
            b
        };
        let mut a = AnimSource { dictionary: "a@b".into(), clip: "c".into(), frames: 10, ..Default::default() };
        a.bones = vec![key(40269)];
        assert_eq!(ids(&a), [(0, 1, 1), (40269, 1, 1), (0, 0, 5), (0, 1, 6)]);
        a.bones.push(key(ROOT));
        assert_eq!(ids(&a), [(0, 1, 1), (40269, 1, 1), (0, 0, 5), (0, 1, 6)]);
    }
}
