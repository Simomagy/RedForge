//! The animation editor: an animation made by posing a ped's bones at frames
//! ([`rf_editor::anim::AnimSource`]), saved as its keys and exported as the
//! ycd the game plays.
//!
//! The ped is shown as the animation has it at the frame chosen: the bones
//! with keys posed, the others at rest (in the game they keep what the ped
//! was doing). A bone is selected by clicking its joint, or from the list, and
//! turned with the gizmo. With Reach the gizmo moves the bone instead, and the
//! two bones above it follow (a hand pulls its arm, a foot its leg). Bones
//! with keys show in the accent colour.
//!
//! Keying is Blender's. With Auto Key on (the record button of the timeline)
//! a change makes a key at the frame shown; off, the change is only shown
//! until the frame changes, unless I keys it. Alt+I deletes the selected
//! bone's keys at the frame, Alt+G and Alt+R put its location and rotation
//! back at rest. In the view G moves the selected bone and R turns it,
//! following the mouse until a click (X, Y, Z keep to an axis; right click or
//! Esc puts it back). There's no scale: the game takes none from a clip. The panel on the right has the bone's numbers (a field is
//! green when animated, yellow on a key, orange changed and not keyed) and
//! the active keyframe's. The Dope Sheet and the Graph Editor are below
//! ([`super::anim_sheet`]). Space plays, the arrows step a frame (left,
//! right) or go to the keys before and after (down, up).
//!
//! For testing: `REDFORGE_ANIM_PED` picks the ped, `REDFORGE_ANIM_BONE` selects
//! a bone by name, `REDFORGE_ANIM_FRAME` goes to a frame and
//! `REDFORGE_ANIM_REACH=x,y,z` reaches for that point of the model with the
//! selected bone, `REDFORGE_ANIM_BASE=<path>` stands the ped in a game clip
//! dictionary's first clip and `REDFORGE_ANIM_KEY_BASE=<file>` keys that pose
//! and saves the animation, for the app's automatic screenshots.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use glam::{Mat4, Quat};
use rf_editor::anim::{
    AnimSource, BasePose, BoneKeys, FPS, Handle, Interpolation, MAX_FRAMES, Mover, Path, ROOT, RotationMode,
};
use rf_formats::formats::ycd::{self, ClipDictionary};
use rf_formats::rpf8::FileRef;
use rf_game::peds::{LoadedPed, Peds};
use rf_render::{CameraView, DebugView, OrbitCamera, PosedModel, RenderTargets, ViewSettings};
use transform_gizmo_egui::math::Transform;
use transform_gizmo_egui::{Gizmo, GizmoConfig, GizmoInteraction, GizmoMode, GizmoOrientation};

use super::anim::{DEFAULT_PED, GpuPed, clip_label, load_ped};
use super::anim_sheet::{History, Mode, Sheet, unit};
use super::{Gpu, Job, status_ui};
use crate::game::Game;
use crate::theme::{self, icon};

/// The file an animation's keys are saved in.
pub const EXTENSION: &str = "rfanim.json";

/// How near a joint a click has to be, in points.
const PICK_RADIUS: f32 = 12.0;

/// A clip dictionary of the game's, for the base pose.
struct Base {
    dict: ClipDictionary,
    /// The animations of the embedded dictionary by hash: their index in it.
    anims: std::collections::HashMap<u32, usize>,
    /// The clips as they're listed, with their index in `dict.entries`.
    clips: Vec<(String, usize)>,
}

impl Base {
    fn clip(&self, name: &str) -> Option<&ycd::Clip> {
        let (_, i) = self.clips.iter().find(|(n, _)| n == name)?;
        self.dict.entries[*i].clip.as_ref()
    }

    fn anim(&self, hash: u32) -> Option<&ycd::Anim> {
        self.dict.anim_dict()?.entries.get(*self.anims.get(&hash)?)?.anim.as_ref()
    }
}

pub struct AnimEditor {
    pub title: String,
    /// The file's path, or a made-up one for an animation not saved yet: what
    /// tells this tab from another.
    pub path: String,
    game: Arc<Game>,
    peds: Job<Arc<Peds>>,
    ped_filter: String,
    ped_job: Option<(String, Job<LoadedPed>)>,
    ped: Option<GpuPed>,
    source: AnimSource,
    /// The base pose's dictionary: read, and being read (by path).
    base: Option<Base>,
    base_job: Option<(String, Job<Base>)>,
    base_search: String,
    /// The game's clip dictionaries, listed the first time one is looked for.
    ycds: Option<Vec<(String, FileRef)>>,
    file: Option<PathBuf>,
    history: History,
    sheet: Sheet,
    /// Blender's Auto Key: a change keys the frame shown.
    auto_key: bool,
    /// What was changed and not keyed, by bone, path and index: shown until
    /// the frame changes.
    loose: HashMap<(u16, Path, u8), f32>,
    /// The frame the ped was last posed at.
    posed_frame: f32,
    /// A field of the side panel is being changed: one undo step.
    editing: bool,
    /// Where the sheet was last frame: its own keys are its own there.
    sheet_rect: Rect,
    /// G, R or S under way in the view.
    modal: Option<Modal>,
    /// The frame shown: between frames while playing.
    frame: f32,
    playing: bool,
    last_frame: Option<Instant>,
    /// The selected bone's tag.
    selected: Option<u16>,
    bone_filter: String,
    /// Shows every bone's joint, not only the main ones (`SKEL_`).
    all_bones: bool,
    gizmo: Gizmo,
    /// Whether the left button went down on the gizmo: that drag isn't the camera's.
    gizmo_press: bool,
    /// A gizmo drag is one undo step.
    turning: bool,
    /// The gizmo moves the selected bone, the two above it following (a hand
    /// pulls its arm), instead of turning it.
    reach: bool,
    /// Whether the animation's file is in the open project (`None` without
    /// a project): the app says, before each [`AnimEditor::ui`].
    pub in_project: Option<bool>,
    /// Add to project was pressed: the app adds [`AnimEditor::file`] and
    /// takes this.
    pub add_to_project: bool,
    /// Reaching leaves the bone itself turned as it was in the model (a hand
    /// keeps facing the way it did while its arm moves).
    keep_turn: bool,
    /// `REDFORGE_ANIM_REACH`: a point to reach for once the ped is posed.
    test_reach: Option<glam::Vec3>,
    /// `REDFORGE_ANIM_KEY_BASE` was done.
    test_keyed: bool,
    status: String,
    failed: bool,
    camera: OrbitCamera,
    view: ViewSettings,
    targets: Option<RenderTargets>,
    texture_id: Option<egui::TextureId>,
}

fn rows(m: Mat4) -> mint::RowMatrix4<f64> {
    let r = |i: usize| {
        let v = m.row(i).as_dvec4();
        mint::Vector4 { x: v.x, y: v.y, z: v.z, w: v.w }
    };
    mint::RowMatrix4 { x: r(0), y: r(1), z: r(2), w: r(3) }
}

fn to_screen(rect: Rect, ndc: glam::Vec2) -> Pos2 {
    Pos2::new(rect.min.x + (ndc.x + 1.0) * 0.5 * rect.width(), rect.min.y + (1.0 - ndc.y) * 0.5 * rect.height())
}

#[derive(Clone, Copy, PartialEq)]
enum Transform3 {
    Grab,
    Rotate,
}

/// Blender's G and R in the view (no S: the game takes no scale from a clip): the selected bone follows the mouse
/// until a click (or Enter) keeps it or a right click (or Esc) puts it back.
struct Modal {
    kind: Transform3,
    tag: u16,
    /// Where the mouse was when the key was pressed.
    origin: Pos2,
    /// X, Y or Z pressed: the axis, and whether it's the bone's own (pressed twice).
    axis: Option<(usize, bool)>,
    /// The animation and what wasn't keyed, as they were.
    bones: Vec<BoneKeys>,
    loose: HashMap<(u16, Path, u8), f32>,
}

/// The fields' colours, as Blender's: animated, on a key, changed and not keyed.
const ANIMATED: Color32 = Color32::from_rgb(58, 104, 58);
const KEYED: Color32 = Color32::from_rgb(140, 122, 28);
const CHANGED: Color32 = Color32::from_rgb(160, 88, 28);

impl AnimEditor {
    /// A new animation: two seconds, no keys.
    pub fn new(ctx: &egui::Context, game: &Arc<Game>, number: usize) -> Self {
        let source = AnimSource {
            dictionary: "my@animation".into(),
            clip: "clip".into(),
            frames: 61,
            bones: Vec::new(),
            base: None,
            mover: None,
        };
        let name = if number > 1 { format!("New animation {number}") } else { "New animation".to_owned() };
        Self::with(ctx, game, source, None, name)
    }

    /// An animation's keys from their file.
    pub fn open(ctx: &egui::Context, game: &Arc<Game>, file: PathBuf) -> anyhow::Result<Self> {
        let source = AnimSource::from_json(&std::fs::read_to_string(&file)?)?;
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Self::with(ctx, game, source, Some(file), name))
    }

    fn with(ctx: &egui::Context, game: &Arc<Game>, source: AnimSource, file: Option<PathBuf>, title: String) -> Self {
        let g = game.clone();
        let peds = Job::spawn(ctx, move || Ok(Arc::new(Peds::load(&g.fs, &g.hashes, &g.data()))));
        let frame = std::env::var("REDFORGE_ANIM_FRAME").ok().and_then(|f| f.parse().ok()).unwrap_or(0.0);
        let mut source = source;
        if let Ok(file) = std::env::var("REDFORGE_ANIM_BASE") {
            source.base = Some(BasePose { file, ..Default::default() });
        }
        let base_job = source.base.as_ref().map(|b| (b.file.clone(), read_base(ctx, game, b.file.clone())));
        let mut sheet = Sheet::default();
        // `REDFORGE_ANIM_SHEET=graph`, for the app's automatic screenshots.
        if std::env::var("REDFORGE_ANIM_SHEET").is_ok_and(|m| m == "graph") {
            sheet.mode = Mode::Graph;
        }
        Self {
            path: file.as_ref().map_or_else(|| title.clone(), |f| f.display().to_string()),
            title,
            game: game.clone(),
            peds,
            ped_filter: String::new(),
            ped_job: None,
            ped: None,
            source,
            base: None,
            base_job,
            base_search: String::new(),
            ycds: None,
            file,
            history: History::default(),
            sheet,
            auto_key: true,
            loose: HashMap::new(),
            posed_frame: frame,
            editing: false,
            sheet_rect: Rect::NOTHING,
            modal: None,
            frame,
            playing: false,
            last_frame: None,
            selected: None,
            bone_filter: String::new(),
            all_bones: false,
            gizmo: Gizmo::default(),
            gizmo_press: false,
            turning: false,
            reach: false,
            in_project: None,
            add_to_project: false,
            keep_turn: true,
            test_reach: None,
            test_keyed: false,
            status: String::new(),
            failed: false,
            camera: OrbitCamera::default(),
            view: ViewSettings::default(),
            targets: None,
            texture_id: None,
        }
    }

    pub fn free(&mut self, gpu: &Gpu) {
        gpu.free_texture(self.texture_id.take());
    }

    /// Whether there are changes that aren't saved.
    pub fn is_dirty(&self) -> bool {
        self.history.dirty
    }

    /// The whole frame shown.
    fn at(&self) -> f32 {
        self.frame.round().clamp(0.0, f32::from(self.source.frames.saturating_sub(1)))
    }

    fn say(&mut self, text: impl Into<String>) {
        (self.status, self.failed) = (text.into(), false);
    }

    fn fail(&mut self, text: impl Into<String>) {
        (self.status, self.failed) = (text.into(), true);
    }

    /// Before a change: what undo goes back to.
    fn step(&mut self) {
        self.history.step(&self.source);
    }

    fn undo(&mut self) {
        self.history.undo(&mut self.source);
        self.loose.clear();
    }

    fn redo(&mut self) {
        self.history.redo(&mut self.source);
        self.loose.clear();
    }

    fn keys(&self, tag: u16) -> Option<&BoneKeys> {
        self.source.bones.iter().find(|b| b.bone == tag)
    }

    /// The bone's entry in the animation: made, on the ped's rest pose, when
    /// it has none (the root's from the ped's own place: see `pose`).
    fn entry(&mut self, tag: u16, name: &str) -> usize {
        if let Some(i) = self.source.bones.iter().position(|b| b.bone == tag) {
            return i;
        }
        let (mut rotation, mut position) = self
            .ped
            .as_ref()
            .and_then(|p| {
                let b = &p.pose.bones()[p.pose.bone_index(tag)?];
                Some((b.rest_rotation, b.rest_translation))
            })
            .unwrap_or((Quat::IDENTITY, glam::Vec3::ZERO));
        if tag == ROOT {
            let turn = mover_turn(self.source.mover).inverse();
            (rotation, position) = (turn * rotation, turn * position);
        }
        self.source.bones.push(BoneKeys::new(tag, name, rotation, position));
        self.source.bones.len() - 1
    }

    /// A number of a bone at the frame shown: changed and not keyed, or the curve's.
    fn value(&self, b: &BoneKeys, path: Path, index: u8) -> f32 {
        self.loose.get(&(b.bone, path, index)).copied().unwrap_or_else(|| b.value(path, index, self.at()))
    }

    /// A bone's rotation, from its parent as the ped is shown, at a frame:
    /// keyed there (Auto Key, or `key`), or only shown until the frame changes.
    fn write_rotation(&mut self, tag: u16, name: &str, frame: f32, rotation: Quat, key: bool) {
        // The root's keys are from the ped's own place: see `pose`.
        let rotation = if tag == ROOT { mover_turn(self.source.mover).inverse() * rotation } else { rotation };
        let i = self.entry(tag, name);
        let path = self.source.bones[i].rotation_mode.path();
        if key {
            self.source.bones[i].key_rotation(frame, rotation);
            self.loose.retain(|k, _| !(k.0 == tag && k.1 == path));
        } else {
            let near: Vec<f32> = (0..path.size()).map(|n| self.value(&self.source.bones[i], path, n)).collect();
            for (n, v) in self.source.bones[i].turn_values(rotation, &near).into_iter().enumerate() {
                self.loose.insert((tag, path, n as u8), v);
            }
        }
    }

    fn write_position(&mut self, tag: u16, name: &str, frame: f32, position: glam::Vec3, key: bool) {
        let position = if tag == ROOT { mover_turn(self.source.mover).inverse() * position } else { position };
        let i = self.entry(tag, name);
        if key {
            self.source.bones[i].key_position(frame, position);
            self.loose.retain(|k, _| !(k.0 == tag && k.1 == Path::Location));
        } else {
            for (n, v) in self.source.bones[i].location_values(position).into_iter().enumerate() {
                self.loose.insert((tag, Path::Location, n as u8), v);
            }
        }
    }

    fn set_rotation(&mut self, tag: u16, name: &str, frame: f32, rotation: Quat) {
        self.write_rotation(tag, name, frame, rotation, self.auto_key);
    }

    /// I: keys the bone as it's shown, at the frame shown. Its rotation, and
    /// its location when that's animated or was changed (most bones only turn).
    fn insert_key(&mut self, tag: u16) {
        let name = self.bone_name(tag);
        self.step();
        let i = self.entry(tag, &name);
        let frame = self.at();
        let path = self.source.bones[i].rotation_mode.path();
        let used = |p: Path| self.source.bones[i].has(p) || self.loose.keys().any(|k| k.0 == tag && k.1 == p);
        let mut paths = vec![path];
        paths.extend([Path::Location].into_iter().filter(|p| used(*p)));
        for path in paths {
            for n in 0..path.size() {
                let v = self.value(&self.source.bones[i], path, n);
                self.source.bones[i].channel_or_new(path, n).curve.insert(frame, v);
            }
        }
        self.loose.retain(|k, _| k.0 != tag);
        self.say(format!("Keyed {name} at frame {frame:.0}"));
    }

    /// Alt+G and Alt+R: a path of the bone back at rest, keyed with Auto Key.
    fn clear(&mut self, tag: u16, path: Path) {
        let name = self.bone_name(tag);
        if self.auto_key {
            self.step();
        }
        let i = self.entry(tag, &name);
        let path = if path == Path::Location { path } else { self.source.bones[i].rotation_mode.path() };
        let frame = self.at();
        for n in 0..path.size() {
            if self.auto_key {
                self.source.bones[i].channel_or_new(path, n).curve.insert(frame, path.rest(n));
            } else {
                self.loose.insert((tag, path, n), path.rest(n));
            }
        }
    }

    /// Takes a bone's keys at a frame away.
    fn remove_key(&mut self, tag: u16, frame: f32) {
        if let Some(b) = self.source.bones.iter_mut().find(|b| b.bone == tag) {
            for c in &mut b.channels {
                if let Some(k) = c.curve.key_at(frame) {
                    c.curve.keys.remove(k);
                    c.curve.recalc();
                }
            }
            b.prune();
        }
        self.loose.retain(|k, _| k.0 != tag);
    }

    /// The ped as the animation has it at the frame shown.
    fn pose(&mut self) {
        let Some(ped) = &mut self.ped else { return };
        let frame = self.frame.clamp(0.0, f32::from(self.source.frames.saturating_sub(1)));
        // What was changed and not keyed is shown until the frame changes.
        if frame.round() != self.posed_frame {
            self.loose.clear();
            self.posed_frame = frame.round();
        }
        // What the ped is doing under the animation, or its rest pose.
        let under = self.source.base.as_ref().and_then(|p| {
            let base = self.base.as_ref()?;
            Some((base, base.clip(&p.clip)?, p.time))
        });
        // A clip and the keys say where the root is from the ped's own place,
        // which is turned (half way round, standing): it's shown from there,
        // as the game does, for the ped to face the way it does at rest.
        let keyed = mover_turn(self.source.mover);
        let (mut turn_rotation, mut turn_position) = (None, None);
        match under {
            // (A clip's root is already shown from the clip's own.)
            Some((base, clip, time)) => ped.pose.apply_clip(clip, |h| base.anim(h), time, false),
            None => ped.pose.reset(),
        }
        for b in &self.source.bones {
            if let Some(i) = ped.pose.bone_index(b.bone) {
                let has = |path: Path| b.has(path) || self.loose.keys().any(|k| k.0 == b.bone && k.1 == path);
                let number = |path: Path, n: u8| {
                    self.loose.get(&(b.bone, path, n)).copied().unwrap_or_else(|| b.value(path, n, frame))
                };
                let turn = b.rotation_mode.path();
                let rotation = has(turn).then(|| {
                    let v: Vec<f32> = (0..turn.size()).map(|n| number(turn, n)).collect();
                    b.local_rotation(&v)
                });
                let position = has(Path::Location)
                    .then(|| b.local_position(glam::Vec3::from_array([0, 1, 2].map(|n| number(Path::Location, n)))));
                ped.pose.set_local(i, rotation, position);
                if b.bone == ROOT {
                    turn_rotation = rotation.map(|_| keyed).or(turn_rotation);
                    turn_position = position.map(|_| keyed).or(turn_position);
                }
            }
        }
        if let Some(i) = ped.pose.bone_index(ROOT) {
            let (rotation, position) = (ped.pose.bones()[i].rotation, ped.pose.bones()[i].translation);
            ped.pose.set_local(i, turn_rotation.map(|t| t * rotation), turn_position.map(|t| t * position));
        }
        ped.pose.update();
    }

    fn save(&mut self, ask: bool) {
        let file = match (&self.file, ask) {
            (Some(f), false) => Some(f.clone()),
            _ => rfd::FileDialog::new()
                .set_title("Save the animation")
                .set_file_name(format!("{}.{EXTENSION}", self.source.dictionary))
                .add_filter("RedForge animation", &["json"])
                .save_file(),
        };
        let Some(file) = file else { return };
        // The bones nothing was keyed on aren't the file's.
        let mut source = self.source.clone();
        source.bones.retain(BoneKeys::is_keyed);
        match std::fs::write(&file, source.to_json()) {
            Ok(()) => {
                self.title = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.path = file.display().to_string();
                self.say(format!("Saved {}", file.display()));
                self.file = Some(file);
                self.history.dirty = false;
            }
            Err(e) => self.fail(format!("Can't save {}: {e}", file.display())),
        }
    }

    /// Writes `<dictionary>.ycd`: next to the animation's file, or where asked.
    fn export(&mut self) {
        let data = match self.source.bake() {
            Ok(d) => d,
            Err(e) => return self.fail(format!("Can't export: {e}")),
        };
        let folder = match self.file.as_ref().and_then(|f| f.parent()) {
            Some(f) => Some(f.to_owned()),
            None => rfd::FileDialog::new().set_title("Export the ycd to").pick_folder(),
        };
        let Some(folder) = folder else { return };
        let file = folder.join(format!("{}.ycd", self.source.dictionary));
        match std::fs::write(&file, data) {
            Ok(()) => self.say(format!("Exported {}", file.display())),
            Err(e) => self.fail(format!("Can't write {}: {e}", file.display())),
        }
    }

    /// The file it's saved in, once it is.
    pub fn file(&self) -> Option<&std::path::Path> {
        self.file.as_deref()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu) {
        if self.peds.poll()
            && let Job::Done(peds) = &self.peds
        {
            let ped = std::env::var("REDFORGE_ANIM_PED").unwrap_or_else(|_| DEFAULT_PED.to_owned());
            self.ped_job = Some(load_ped(ui.ctx(), &self.game, peds, ped));
        }
        let Job::Done(peds) = &self.peds else {
            status_ui(ui, &self.peds, "the peds");
            return;
        };
        let peds = peds.clone();
        if let Some((name, job)) = &mut self.ped_job
            && job.poll()
        {
            let name = name.clone();
            match std::mem::replace(job, Job::Failed(String::new())) {
                Job::Done(ped) => {
                    let ped = GpuPed::new(gpu, &self.game.hashes, ped);
                    self.camera.frame(ped.min, ped.max);
                    if let Ok(bone) = std::env::var("REDFORGE_ANIM_BONE") {
                        self.selected = ped
                            .pose
                            .bones()
                            .iter()
                            .find(|b| b.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&bone)))
                            .map(|b| b.tag);
                    }
                    self.ped = Some(ped);
                    self.rebase_old();
                    self.test_reach = std::env::var("REDFORGE_ANIM_REACH").ok().and_then(|p| {
                        let c: Vec<f32> = p.split(',').filter_map(|c| c.trim().parse().ok()).collect();
                        (c.len() == 3).then(|| glam::Vec3::new(c[0], c[1], c[2]))
                    });
                }
                Job::Failed(e) => self.fail(format!("Can't load {name}: {e}")),
                Job::Running(_) => unreachable!(),
            }
            self.ped_job = None;
        }

        if let Some((file, job)) = &mut self.base_job
            && job.poll()
        {
            let file = file.clone();
            match std::mem::replace(job, Job::Failed(String::new())) {
                Job::Done(base) => {
                    // The clip named, or the first.
                    if let Some(p) = self.source.base.as_mut().filter(|p| p.file == file)
                        && base.clip(&p.clip).is_none()
                    {
                        p.clip = base.clips.first().map(|(n, _)| n.clone()).unwrap_or_default();
                        p.time = 0.0;
                    }
                    self.base = Some(base);
                }
                Job::Failed(e) => {
                    self.source.base = None;
                    self.fail(format!("Can't read {file}: {e}"));
                }
                Job::Running(_) => unreachable!(),
            }
            self.base_job = None;
        }

        // The keys Blender has, with the pointer anywhere on the editor.
        let over = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| ui.max_rect().contains(p));
        if over && self.modal.is_none() && !ui.ctx().egui_wants_keyboard_input() {
            self.shortcuts(ui);
        }

        // Advance the frame.
        let now = Instant::now();
        let last = self.source.frames.saturating_sub(1) as f32;
        if self.playing
            && let Some(before) = self.last_frame
            && last > 0.0
        {
            self.frame = (self.frame + (now - before).as_secs_f32() * FPS).rem_euclid(last);
        }
        self.last_frame = Some(now);
        self.frame = self.frame.clamp(0.0, last);

        self.bar_ui(ui, &peds);
        self.timeline_ui(ui);
        self.bones_ui(ui);
        self.item_ui(ui);
        self.pose();
        // `REDFORGE_ANIM_KEY_BASE=<file>`: once the ped stands in the base pose,
        // keys it and saves the animation there (for testing).
        if self.ped.is_some()
            && self.base.is_some()
            && let Ok(file) = std::env::var("REDFORGE_ANIM_KEY_BASE")
            && !self.test_keyed
        {
            self.test_keyed = true;
            self.key_base();
            if let Err(e) = std::fs::write(&file, self.source.to_json()) {
                log::error!("{file}: {e}");
            }
            self.pose();
        }
        if let Some(to) = self.test_reach.take() {
            self.reach = true;
            if let Some(limb) = self.limb() {
                let frame = self.at();
                self.reach_for(limb, to, frame);
                self.pose();
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.viewport_ui(ui, gpu));
        if self.playing || self.ped_job.is_some() {
            ui.ctx().request_repaint();
        }
    }

    /// Playing, the frame, the names, the ped, and the file.
    fn bar_ui(&mut self, ui: &mut egui::Ui, peds: &Arc<Peds>) {
        let mut pick_ped = None;
        egui::Panel::top(egui::Id::new(("anim-edit-bar", &self.path))).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.add_enabled(self.history.can_undo(), egui::Button::new("Undo")).clicked() {
                    self.undo();
                }
                if ui.add_enabled(self.history.can_redo(), egui::Button::new("Redo")).clicked() {
                    self.redo();
                }
                ui.separator();
                ui.label("Ped");
                let current = self
                    .ped_job
                    .as_ref()
                    .map(|(n, _)| n.as_str())
                    .or(self.ped.as_ref().map(|p| p.name.as_str()))
                    .unwrap_or("")
                    .to_owned();
                egui::ComboBox::from_id_salt(("anim-edit-ped", &self.path))
                    .width(200.0)
                    .selected_text(&current)
                    .show_ui(ui, |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.ped_filter).hint_text("Filter"));
                        let f = self.ped_filter.to_lowercase();
                        let names: Vec<&String> = peds.names().iter().filter(|n| n.contains(&f)).collect();
                        let row = ui.spacing().interact_size.y;
                        egui::ScrollArea::vertical().max_height(320.0).show_rows(ui, row, names.len(), |ui, range| {
                            for n in &names[range] {
                                if ui.selectable_label(**n == current, n.as_str()).clicked() {
                                    pick_ped = Some((*n).clone());
                                }
                            }
                        });
                    });
                if self.ped_job.is_some() {
                    ui.spinner();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Dictionary");
                let mut name = self.source.dictionary.clone();
                if ui.add(egui::TextEdit::singleline(&mut name).desired_width(170.0)).changed() {
                    self.step();
                    self.source.dictionary = name;
                }
                ui.label("Clip");
                let mut name = self.source.clip.clone();
                if ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0)).changed() {
                    self.step();
                    self.source.clip = name;
                }
                ui.separator();
                if ui.button("Save").clicked() {
                    self.save(false);
                }
                if ui.button("Save as…").clicked() {
                    self.save(true);
                }
                let tip = "The ycd the game plays, named after the dictionary, for a resource's stream folder";
                if ui.add(theme::primary("Export ycd")).on_hover_text(tip).clicked() {
                    self.export();
                }
                if self.in_project == Some(false) {
                    let tip = match self.file {
                        Some(_) => "The project's RedM export then bakes it into the resource's stream folder",
                        None => "Save it first",
                    };
                    let add = ui.add_enabled(self.file.is_some(), egui::Button::new("Add to project"));
                    if add.on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                        self.add_to_project = true;
                    }
                }
                ui.separator();
                // What's wrong with it comes before what was last done.
                match self.source.check() {
                    Err(e) if self.source.bones.iter().any(BoneKeys::is_keyed) || !matches!(e, rf_editor::anim::AnimError::Empty) => {
                        ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                    }
                    _ if self.failed => {
                        ui.colored_label(ui.visuals().error_fg_color, &self.status);
                    }
                    _ => {
                        ui.label(egui::RichText::new(&self.status).weak());
                    }
                }
            });
        });
        let ctx = ui.ctx().clone();
        egui::Panel::top(egui::Id::new(("anim-edit-base", &self.path))).show(ui, |ui| self.base_ui(ui, &ctx));
        if let Some(name) = pick_ped
            && self.ped_job.as_ref().is_none_or(|(n, _)| *n != name)
            && self.ped.as_ref().is_none_or(|p| p.name != name)
        {
            self.ped_job = Some(load_ped(ui.ctx(), &self.game, peds, name));
        }
    }

    /// The base pose: a game clip to stand the ped in, the moment of it, and
    /// keying it.
    fn base_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label("Base pose").on_hover_text(
                "What the ped is doing under the animation: a moment of one of the game's clips. \
                 Only for looking, as the game leaves the bones without keys as the ped has them.",
            );
            let current = self.source.base.as_ref().map(|b| b.file.rsplit('/').next().unwrap_or(&b.file).to_owned());
            let mut pick = None;
            ui.menu_button(current.as_deref().unwrap_or("Rest pose"), |ui| {
                ui.set_min_width(380.0);
                let field =
                    egui::TextEdit::singleline(&mut self.base_search).hint_text("Search the game's clip dictionaries");
                ui.add(field.desired_width(f32::INFINITY)).request_focus();
                let game = &self.game;
                let all = self.ycds.get_or_insert_with(|| {
                    game.paths.iter().filter(|(p, _)| p.ends_with(".ycd")).map(|(p, f)| (p.clone(), *f)).collect()
                });
                let words: Vec<String> =
                    self.base_search.to_lowercase().split_whitespace().map(str::to_owned).collect();
                if words.is_empty() {
                    ui.label(egui::RichText::new("idle, walk, sit… (several words narrow it down)").weak());
                    return;
                }
                let found: Vec<&(String, FileRef)> =
                    all.iter().filter(|(p, _)| words.iter().all(|w| p.contains(w.as_str()))).take(300).collect();
                let row = ui.spacing().interact_size.y;
                egui::ScrollArea::vertical().max_height(360.0).show_rows(ui, row, found.len(), |ui, range| {
                    for (path, _) in &found[range] {
                        let name = path.rsplit('/').next().unwrap_or(path);
                        if ui.selectable_label(false, name).on_hover_text(path).clicked() {
                            pick = Some(path.clone());
                        }
                    }
                });
                if found.is_empty() {
                    ui.label(egui::RichText::new("No clip dictionary has that in its name.").weak());
                }
            });
            if let Some(file) = pick {
                self.step();
                self.base = None;
                self.base_job = Some((file.clone(), read_base(ctx, &self.game, file.clone())));
                self.source.base = Some(BasePose { file, ..Default::default() });
                ui.close();
            }
            if self.base_job.is_some() {
                ui.spinner();
            }
            let Some(pose) = self.source.base.clone() else { return };
            let Some(base) = &self.base else { return };
            let mut chosen = pose.clone();
            egui::ComboBox::from_id_salt(("anim-edit-base-clip", &self.path))
                .width(200.0)
                .selected_text(&chosen.clip)
                .show_ui(ui, |ui| {
                    for (name, _) in &base.clips {
                        if ui.selectable_label(*name == chosen.clip, name).clicked() {
                            chosen.clip = name.clone();
                            chosen.time = 0.0;
                        }
                    }
                });
            let duration = base.clip(&chosen.clip).map_or(0.0, |c| c.duration());
            ui.add(egui::DragValue::new(&mut chosen.time).range(0.0..=duration).speed(0.01).prefix("at ").suffix(" s"));
            let keyable = base.clip(&chosen.clip).is_some() && self.ped.is_some();
            let tip = "A key at this frame for every bone the clip moves, as they are now: \
                       the pose is then part of the animation";
            let key = ui.add_enabled(keyable, egui::Button::new("Key this pose")).on_hover_text(tip).clicked();
            let clear = ui.button("Rest pose").on_hover_text("Back to the skeleton's rest pose").clicked();
            if chosen != pose {
                self.step();
                self.source.base = Some(chosen);
            }
            if key {
                self.key_base();
            }
            if clear {
                self.step();
                self.source.base = None;
                self.base = None;
                self.base_job = None;
            }
        });
    }

    /// A key at the frame shown for every bone the base pose's clip moves, as
    /// they're shown: rotations, and the positions the clip has.
    fn key_base(&mut self) {
        let frame = self.at();
        let (tracks, mover): (Vec<(u16, u8)>, _) = {
            let (Some(base), Some(pose)) = (&self.base, &self.source.base) else { return };
            let Some(clip) = base.clip(&pose.clip) else { return };
            let mover = rf_render::anim::clip_mover(clip, |h| base.anim(h), pose.time)
                .map(|(position, rotation)| Mover { position, rotation });
            let mut tracks: Vec<(u16, u8)> = clip
                .animation_hashes()
                .iter()
                .filter_map(|h| base.anim(*h))
                .flat_map(|a| a.bone_ids.iter().flatten())
                .filter(|b| (b.type_id, b.track_id) == (1, 1) || (b.type_id, b.track_id) == (0, 0))
                .map(|b| (b.bone_id, b.type_id))
                .collect();
            tracks.sort_unstable();
            tracks.dedup();
            (tracks, mover)
        };
        self.step();
        // The root is where the clip has it from the ped's own place there.
        if tracks.iter().any(|(tag, _)| *tag == ROOT) {
            self.source.mover = mover;
        }
        let mut count = 0;
        for (tag, kind) in tracks {
            let Some((rotation, position)) = self.ped.as_ref().and_then(|p| {
                let b = &p.pose.bones()[p.pose.bone_index(tag)?];
                Some((b.rotation, b.translation))
            }) else {
                continue;
            };
            let name = self.bone_name(tag);
            if kind == 1 {
                self.write_rotation(tag, &name, frame, rotation, true);
                count += 1;
            } else {
                self.write_position(tag, &name, frame, position, true);
            }
        }
        self.say(format!("Keyed {count} bones at frame {frame}"));
    }

    /// The selected bone and its keys, the bones with keys, and every bone by name.
    fn bones_ui(&mut self, ui: &mut egui::Ui) {
        // (tag, name) of the ped's bones.
        let all: Vec<(u16, String)> = self
            .ped
            .iter()
            .flat_map(|p| p.pose.bones())
            .map(|b| (b.tag, b.name.clone().unwrap_or_else(|| format!("bone {}", b.tag))))
            .collect();
        let name_of = |tag: u16, source: &AnimSource| {
            all.iter()
                .find(|(t, _)| *t == tag)
                .map(|(_, n)| n.clone())
                .or_else(|| {
                    source.bones.iter().find(|b| b.bone == tag).map(|b| b.name.clone()).filter(|n| !n.is_empty())
                })
                .unwrap_or_else(|| format!("bone {tag}"))
        };
        egui::Panel::left(egui::Id::new(("anim-edit-bones", &self.path))).default_size(250.0).show(ui, |ui| {
            match self.selected {
                Some(tag) => {
                    let name = name_of(tag, &self.source);
                    ui.label(egui::RichText::new(&name).font(theme::strong(theme::SIZE)));
                    let can_reach = self.limb().is_some();
                    ui.horizontal(|ui| {
                        if ui.selectable_label(!self.reach, "Turn").on_hover_text("Turn the bone").clicked() {
                            self.reach = false;
                        }
                        let tip = "Move the bone, the two above it following: a hand pulls its arm, a foot its leg";
                        let reach = ui.add_enabled(can_reach, egui::Button::selectable(self.reach, "Reach"));
                        if reach.on_hover_text(tip).clicked() {
                            self.reach = true;
                        }
                    });
                    if self.reach && can_reach {
                        let tip = "The bone stays turned as it is while its limb moves: a hand keeps facing the \
                                   same way. Off, it turns with the bone above it";
                        ui.checkbox(&mut self.keep_turn, "Keep its turn").on_hover_text(tip);
                    }
                }
                None => {
                    ui.label(egui::RichText::new("Click a joint on the ped, or a bone here, to pose it.").weak());
                }
            }
            ui.separator();
            let keyed: Vec<(u16, usize)> =
                self.source.bones.iter().filter(|b| b.is_keyed()).map(|b| (b.bone, b.key_frames().len())).collect();
            ui.label(egui::RichText::new(format!("Bones with keys ({})", keyed.len())).weak());
            let mut pick = None;
            // A few rows' worth: a whole body's keyed bones don't push the search below out of sight.
            let most = (ui.available_height() * 0.35).max(90.0);
            egui::ScrollArea::vertical().id_salt("keyed-bones").max_height(most).show(ui, |ui| {
                for (tag, count) in keyed {
                    let note = if count == 1 { "1 key".to_owned() } else { format!("{count} keys") };
                    let row = theme::row(ui, self.selected == Some(tag), icon::BONE, &name_of(tag, &self.source), &note);
                    if row.clicked() {
                        pick = Some(tag);
                    }
                }
            });
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.bone_filter).hint_text("Search bones"));
            ui.checkbox(&mut self.all_bones, "Show every joint").on_hover_text("Not only the main bones (SKEL_)");
            let f = self.bone_filter.to_lowercase();
            let shown: Vec<&(u16, String)> = all
                .iter()
                .filter(|(_, n)| (self.all_bones || !f.is_empty() || main_bone(n)) && n.to_lowercase().contains(&f))
                .collect();
            let row = ui.spacing().interact_size.y;
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row, shown.len(), |ui, range| {
                for (tag, name) in &shown[range] {
                    if ui.selectable_label(self.selected == Some(*tag), name).clicked() {
                        pick = Some(*tag);
                    }
                }
            });
            if pick.is_some() {
                self.selected = pick;
            }
        });
    }


    /// The files before F-curves had the bones as the game has them, not from
    /// a rest pose: stood on the ped's, their curves are what Blender's would be.
    fn rebase_old(&mut self) {
        let Some(ped) = &self.ped else { return };
        let turn = mover_turn(self.source.mover).inverse();
        for b in self.source.bones.iter_mut().filter(|b| b.was_old() && b.is_keyed()) {
            let Some(bone) = ped.pose.bone_index(b.bone).map(|i| &ped.pose.bones()[i]) else { continue };
            let (mut rotation, mut position) = (bone.rest_rotation, bone.rest_translation);
            if b.bone == ROOT {
                (rotation, position) = (turn * rotation, turn * position);
            }
            b.rebase(rotation, position);
        }
    }

    /// One undo step for a field dragged or typed in, however many changes it makes.
    fn begin_edit(&mut self) {
        if !self.editing {
            self.step();
            self.editing = true;
        }
    }

    /// The frames with keys to jump between: the selected bone's when it has
    /// some, or every bone's.
    fn key_frames(&self) -> Vec<f32> {
        let of_selected = self.selected.and_then(|tag| self.keys(tag)).filter(|b| b.is_keyed());
        let mut frames: Vec<f32> = match of_selected {
            Some(b) => b.key_frames(),
            None => self.source.bones.iter().flat_map(BoneKeys::key_frames).collect(),
        };
        frames.sort_by(f32::total_cmp);
        frames.dedup();
        frames
    }

    fn jump(&mut self, forward: bool) {
        let at = self.at();
        let frames = self.key_frames();
        let to = if forward {
            frames.iter().find(|f| **f > at + 1e-3).copied()
        } else {
            frames.iter().rev().find(|f| **f < at - 1e-3).copied()
        };
        if let Some(f) = to {
            self.playing = false;
            self.frame = f;
        }
    }

    /// Blender's keys, with the pointer anywhere on the editor.
    fn shortcuts(&mut self, ui: &egui::Ui) {
        let key = |k: egui::Key| ui.input(|i| i.key_pressed(k));
        let (alt, shift, ctrl) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift, i.modifiers.command));
        let last = f32::from(self.source.frames.saturating_sub(1));
        if key(egui::Key::Space) {
            self.playing = !self.playing;
        }
        for (k, by) in [(egui::Key::ArrowLeft, -1.0), (egui::Key::ArrowRight, 1.0)] {
            if key(k) {
                self.playing = false;
                self.frame = if shift { if by < 0.0 { 0.0 } else { last } } else { (self.at() + by).clamp(0.0, last) };
            }
        }
        if key(egui::Key::ArrowUp) {
            self.jump(true);
        }
        if key(egui::Key::ArrowDown) {
            self.jump(false);
        }
        if ctrl && key(egui::Key::Z) {
            if shift { self.redo() } else { self.undo() }
        }
        let Some(tag) = self.selected else { return };
        let over_sheet = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| self.sheet_rect.contains(p));
        if key(egui::Key::I) {
            if alt {
                self.step();
                let frame = self.at();
                self.remove_key(tag, frame);
            } else if !over_sheet {
                self.insert_key(tag);
            }
        }
        if alt && key(egui::Key::G) {
            self.clear(tag, Path::Location);
        }
        if alt && key(egui::Key::R) {
            self.clear(tag, Path::RotationQuaternion);
        }
    }

    /// The timeline's header (the editor shown, Auto Key, playing, the frame
    /// and the last one) over the Dope Sheet or the Graph Editor.
    fn timeline_ui(&mut self, ui: &mut egui::Ui) {
        let id = egui::Id::new(("anim-edit-time", &self.path));
        egui::Panel::bottom(id).resizable(true).default_size(240.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.sheet.mode, Mode::DopeSheet, "Dope Sheet");
                ui.selectable_value(&mut self.sheet.mode, Mode::Graph, "Graph Editor");
                ui.separator();
                let tip = "Only the selected bone's keys";
                ui.toggle_value(&mut self.sheet.only_selected, "Only Show Selected").on_hover_text(tip);
                ui.separator();
                let tip = "Auto Key: a change keys the frame shown. Off, a change is only shown until the \
                           frame changes, unless I keys it";
                if theme::tool(ui, self.auto_key, icon::RECORD, tip).clicked() {
                    self.auto_key = !self.auto_key;
                }
                let last = f32::from(self.source.frames.saturating_sub(1));
                if theme::tool(ui, false, icon::SKIP_BACK, "To the first frame (Shift+Left)").clicked() {
                    (self.playing, self.frame) = (false, 0.0);
                }
                if theme::tool(ui, false, icon::CARET_DOUBLE_LEFT, "To the key before (Down)").clicked() {
                    self.jump(false);
                }
                let play = if self.playing { icon::PAUSE } else { icon::PLAY };
                if theme::tool(ui, self.playing, play, "Play (Space)").clicked() {
                    self.playing = !self.playing;
                }
                if theme::tool(ui, false, icon::CARET_DOUBLE_RIGHT, "To the key after (Up)").clicked() {
                    self.jump(true);
                }
                if theme::tool(ui, false, icon::SKIP_FORWARD, "To the last frame (Shift+Right)").clicked() {
                    (self.playing, self.frame) = (false, last);
                }
                ui.separator();
                let mut f = self.at();
                if ui.add(egui::DragValue::new(&mut f).range(0.0..=last).speed(0.2).max_decimals(0)).on_hover_text("The frame shown").changed() {
                    (self.playing, self.frame) = (false, f.round());
                }
                ui.label(egui::RichText::new("Start 0").weak()).on_hover_text("The game's animations start at frame 0");
                let mut end = self.source.frames.saturating_sub(1);
                let field = egui::DragValue::new(&mut end).range(1..=MAX_FRAMES - 1).prefix("End ");
                if ui.add(field).on_hover_text("The last frame played: 30 frames a second").changed() {
                    self.begin_edit();
                    self.source.frames = end + 1;
                }
                ui.label(egui::RichText::new(format!("{:.2} s of {:.2}", self.frame / FPS, last / FPS)).weak());
            });
            self.sheet_rect = ui.available_rect_before_wrap();
            let frames = self.source.clone();
            if self.sheet.ui(ui, &mut self.source, &mut self.history, &mut self.frame, &mut self.selected) {
                self.playing = false;
            }
            // Keys moved or deleted under what was changed and not keyed: that's gone.
            if frames.bones != self.source.bones {
                self.loose.clear();
            }
        });
    }

    /// One number of the selected bone: Blender's field, green when
    /// animated, yellow on a key, orange changed and not keyed. Right click
    /// has the keyframe menu.
    fn field(&mut self, ui: &mut egui::Ui, tag: u16, name: &str, path: Path, n: u8) {
        let frame = self.at();
        let bone = self.keys(tag);
        let curve = bone.map_or(path.rest(n), |b| b.value(path, n, frame));
        let channel = bone.and_then(|b| b.channel(path, n)).filter(|c| !c.curve.keys.is_empty());
        let (animated, on_key) = (channel.is_some(), channel.is_some_and(|c| c.curve.key_at(frame).is_some()));
        let loose = self.loose.get(&(tag, path, n)).copied();
        let changed = loose.is_some_and(|v| (v - curve).abs() > 1e-6);
        let tint = if changed {
            Some(CHANGED)
        } else if on_key {
            Some(KEYED)
        } else if animated {
            Some(ANIMATED)
        } else {
            None
        };
        let u = unit(path);
        let mut shown = loose.unwrap_or(curve) * u;
        let letter = match path {
            Path::RotationQuaternion => ["W", "X", "Y", "Z"][usize::from(n)],
            _ => ["X", "Y", "Z"][usize::from(n)],
        };
        let (speed, decimals, suffix) = match path {
            Path::Location => (0.002, 4, " m"),
            Path::RotationEuler => (0.5, 2, "°"),
            Path::RotationQuaternion => (0.004, 4, ""),
        };
        let response = ui
            .scope(|ui| {
                if let Some(c) = tint {
                    let w = &mut ui.visuals_mut().widgets;
                    for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                        (state.weak_bg_fill, state.bg_fill) = (c, c);
                    }
                }
                let field = egui::DragValue::new(&mut shown)
                    .speed(speed)
                    .max_decimals(decimals)
                    .prefix(format!("{letter}   "))
                    .suffix(suffix);
                ui.add_sized([ui.available_width(), 22.0], field)
            })
            .inner;
        if response.changed() {
            let v = shown / u;
            if self.auto_key {
                self.begin_edit();
                let i = self.entry(tag, name);
                self.source.bones[i].channel_or_new(path, n).curve.insert(frame, v);
                self.loose.remove(&(tag, path, n));
            } else {
                self.entry(tag, name);
                self.loose.insert((tag, path, n), v);
            }
        }
        // 0: insert all, 1: insert this one, 2: delete all, 3: delete this one, 4: clear.
        let mut act = None;
        response.context_menu(|ui| {
            for (i, label) in
                ["Insert Keyframes", "Insert Single Keyframe", "Delete Keyframes", "Delete Single Keyframe", "Clear Keyframes"]
                    .into_iter()
                    .enumerate()
            {
                let possible = i < 2 || animated;
                if ui.add_enabled(possible, egui::Button::new(label)).clicked() {
                    act = Some(i);
                    ui.close();
                }
            }
        });
        let Some(act) = act else { return };
        self.step();
        let i = self.entry(tag, name);
        let all: Vec<u8> = (0..path.size()).collect();
        let which: &[u8] = if act == 1 || act == 3 { std::slice::from_ref(&n) } else { &all };
        for &m in which {
            match act {
                0 | 1 => {
                    let v = self.value(&self.source.bones[i], path, m);
                    self.source.bones[i].channel_or_new(path, m).curve.insert(frame, v);
                    self.loose.remove(&(tag, path, m));
                }
                2 | 3 => {
                    if let Some(c) = self.source.bones[i].channel_mut(path, m)
                        && let Some(k) = c.curve.key_at(frame)
                    {
                        c.curve.keys.remove(k);
                        c.curve.recalc();
                    }
                }
                _ => {
                    if let Some(c) = self.source.bones[i].channel_mut(path, m) {
                        c.curve.keys.clear();
                    }
                }
            }
        }
        self.source.bones[i].prune();
    }

    /// Blender's side panel: the selected bone's location and rotation, and
    /// the active keyframe.
    fn item_ui(&mut self, ui: &mut egui::Ui) {
        if !ui.input(|i| i.pointer.any_down()) && !ui.ctx().egui_wants_keyboard_input() {
            self.editing = false;
        }
        let id = egui::Id::new(("anim-edit-item", &self.path));
        egui::Panel::right(id).default_size(230.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("anim-edit-item-scroll").show(ui, |ui| {
                ui.label(egui::RichText::new("Item").font(theme::strong(theme::SIZE)));
                let Some(tag) = self.selected else {
                    ui.label(egui::RichText::new("No bone selected.").weak());
                    return;
                };
                let name = self.bone_name(tag);
                ui.label(&name);
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Location").weak());
                for n in 0..3 {
                    self.field(ui, tag, &name, Path::Location, n);
                }
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Rotation").weak());
                let mode = self.keys(tag).map(|b| b.rotation_mode).unwrap_or_default();
                for n in 0..mode.path().size() {
                    self.field(ui, tag, &name, mode.path(), n);
                }
                let mut chosen = mode;
                egui::ComboBox::from_id_salt(("anim-edit-mode", &self.path))
                    .width(ui.available_width())
                    .selected_text(chosen.label())
                    .show_ui(ui, |ui| {
                        for m in RotationMode::ALL {
                            ui.selectable_value(&mut chosen, m, m.label());
                        }
                    });
                if chosen != mode {
                    self.step();
                    let i = self.entry(tag, &name);
                    self.source.bones[i].set_rotation_mode(chosen);
                    self.loose.retain(|k, _| k.0 != tag || k.1 == Path::Location);
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Insert Keyframe").on_hover_text("I").clicked() {
                        self.insert_key(tag);
                    }
                    let frame = self.at();
                    let here = self.keys(tag).is_some_and(|b| b.channels.iter().any(|c| c.curve.key_at(frame).is_some()));
                    if ui.add_enabled(here, egui::Button::new("Delete")).on_hover_text("Alt+I").clicked() {
                        self.step();
                        self.remove_key(tag, frame);
                    }
                });
                ui.separator();
                self.active_key_ui(ui);
            });
        });
    }

    /// The key last clicked in the sheet: Blender's Active Keyframe.
    fn active_key_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Active Keyframe").font(theme::strong(theme::SIZE)));
        let Some((before, path)) = self.sheet.active_key(&mut self.source).map(|(c, i)| (c.curve.keys[i], c.path)) else {
            ui.label(egui::RichText::new("Click a key in the sheet below.").weak());
            return;
        };
        let u = unit(path);
        let mut key = before;
        let wide = ui.available_width();
        egui::ComboBox::from_id_salt(("anim-edit-interp", &self.path))
            .width(wide)
            .selected_text(key.interpolation.label())
            .show_ui(ui, |ui| {
                for kind in Interpolation::ALL {
                    ui.selectable_value(&mut key.interpolation, kind, kind.label());
                }
            });
        let (mut frame, mut value) = (key.frame, key.value * u);
        ui.add_sized([wide, 22.0], egui::DragValue::new(&mut frame).speed(0.2).max_decimals(0).prefix("Frame   "));
        ui.add_sized([wide, 22.0], egui::DragValue::new(&mut value).speed(0.01 * u.max(1.0)).prefix("Value   "));
        key.shift(frame.round() - key.frame, value / u - key.value);
        for right in [false, true] {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(if right { "Right Handle" } else { "Left Handle" }).weak());
            let kind = if right { &mut key.right_type } else { &mut key.left_type };
            egui::ComboBox::from_id_salt(("anim-edit-handle", right, &self.path))
                .width(wide)
                .selected_text(kind.label())
                .show_ui(ui, |ui| {
                    for h in Handle::ALL {
                        ui.selectable_value(kind, h, h.label());
                    }
                });
            let at = if right { key.right } else { key.left };
            let (mut hf, mut hv) = (at[0], at[1] * u);
            ui.add_sized([wide, 22.0], egui::DragValue::new(&mut hf).speed(0.1).max_decimals(2).prefix("Frame   "));
            ui.add_sized([wide, 22.0], egui::DragValue::new(&mut hv).speed(0.01 * u.max(1.0)).prefix("Value   "));
            if hf != at[0] || hv != at[1] * u {
                // Placed by hand, a handle isn't automatic any more.
                let kind = if right { &mut key.right_type } else { &mut key.left_type };
                if matches!(*kind, Handle::Auto | Handle::AutoClamped | Handle::Vector) {
                    *kind = Handle::Free;
                }
                *(if right { &mut key.right } else { &mut key.left }) = [hf, hv / u];
            }
        }
        if key != before {
            self.begin_edit();
            if let Some((channel, i)) = self.sheet.active_key(&mut self.source) {
                channel.curve.keys[i] = key;
                channel.curve.keys[i].selected = true;
                channel.curve.tidy();
            }
            if let Some(a) = &mut self.sheet.active {
                a.frame = key.frame;
            }
        }
    }

    /// The 3D view: the ped, its joints over it, and the gizmo on the selected bone.
    fn viewport_ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu) {
        let size = ui.available_size().max(Vec2::splat(16.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
        let aspect = rect.width() / rect.height().max(1.0);
        let view = self.camera.camera_view(aspect);

        let on_gizmo = self.modal_ui(ui, &response, &view, rect) || self.gizmo_ui(ui, &response, &view, rect);
        if on_gizmo {
            // The bone turned: show it turned this frame.
            self.pose();
        } else {
            let d = response.drag_delta();
            if response.dragged_by(egui::PointerButton::Primary) && d != Vec2::ZERO {
                self.camera.orbit(d.x * 0.008, d.y * 0.008);
            }
            if (response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle))
                && d != Vec2::ZERO
            {
                self.camera.pan(d.x / rect.height(), d.y / rect.height());
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom((-scroll * 0.002).exp());
            }
        }
        let Some(ped) = &self.ped else {
            if let Some((name, _)) = &self.ped_job {
                ui.put(rect, egui::Label::new(format!("Loading {name}…")));
            }
            return;
        };
        if response.double_clicked() {
            self.camera.frame(ped.min, ped.max);
        }
        if self.targets.as_ref().is_none_or(|t| t.width != w || t.height != h) {
            self.targets = Some(gpu.renderer.create_targets(&gpu.ctx, w, h));
        }
        let t = self.targets.as_ref().unwrap();
        let models: Vec<PosedModel> =
            ped.models.iter().map(|m| PosedModel { model: m, lod: m.highest_lod() }).collect();
        let settings = ViewSettings { debug: DebugView::Lit, ..self.view.clone() };
        gpu.renderer.render_posed_orbit(&gpu.ctx, t, &self.camera, &settings, &ped.pose, &models);
        let id = gpu.show_texture(&mut self.texture_id, &t.display_view);
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(id, rect, uv, Color32::WHITE);

        // The joints, and the bones between them.
        let painter = ui.painter().with_clip_rect(rect);
        let bones = ped.pose.bones();
        let shown: Vec<bool> = bones
            .iter()
            .map(|b| {
                self.all_bones
                    || self.selected == Some(b.tag)
                    || self.source.bones.iter().any(|k| k.bone == b.tag && k.is_keyed())
                    || b.name.as_deref().is_some_and(main_bone)
            })
            .collect();
        let at: Vec<Option<Pos2>> = (0..bones.len())
            .map(|i| {
                let m = ped.pose.model(i).filter(|_| shown[i])?;
                view.project(m.w_axis.truncate()).map(|ndc| to_screen(rect, ndc))
            })
            .collect();
        for (i, b) in bones.iter().enumerate() {
            // To the nearest bone up that's shown.
            let mut parent = b.parent;
            while let Some(p) = parent.filter(|p| !shown[*p]) {
                parent = bones[p].parent;
            }
            if let (Some(a), Some(to)) = (at[i], parent.and_then(|p| at[p])) {
                painter.line_segment([a, to], Stroke::new(1.5, Color32::from_white_alpha(110)));
            }
        }
        let mut nearest: Option<(f32, u16)> = None;
        let click = response.clicked().then(|| response.interact_pointer_pos()).flatten().filter(|_| !on_gizmo);
        for (i, b) in bones.iter().enumerate() {
            let Some(p) = at[i] else { continue };
            let keyed = self.source.bones.iter().any(|k| k.bone == b.tag && k.is_keyed());
            let selected = self.selected == Some(b.tag);
            let colour = if keyed || selected { theme::ACCENT } else { Color32::from_white_alpha(200) };
            painter.circle_filled(p, if selected { 5.5 } else { 3.0 }, colour);
            if selected {
                painter.circle_stroke(p, 8.0, Stroke::new(1.5, theme::ACCENT));
            }
            if let Some(c) = click {
                let d = c.distance(p);
                if d <= PICK_RADIUS && nearest.is_none_or(|(best, _)| d < best) {
                    nearest = Some((d, b.tag));
                }
            }
        }
        if click.is_some() {
            self.selected = nearest.map(|(_, tag)| tag);
        }
        if let Some(name) = response.hover_pos().filter(|_| !on_gizmo).and_then(|c| {
            let mut best: Option<(f32, usize)> = None;
            for (i, p) in at.iter().enumerate() {
                if let Some(d) = p.map(|p| c.distance(p)).filter(|d| *d <= PICK_RADIUS)
                    && best.is_none_or(|(b, _)| d < b)
                {
                    best = Some((d, i));
                }
            }
            best.and_then(|(_, i)| bones[i].name.clone())
        }) {
            response.clone().on_hover_text_at_pointer(name);
        }
        self.paint_gizmo(ui, rect);
    }

    fn bone_name(&self, tag: u16) -> String {
        self.ped
            .as_ref()
            .and_then(|p| p.pose.bones()[p.pose.bone_index(tag)?].name.clone())
            .unwrap_or_else(|| format!("bone {tag}"))
    }

    /// The selected bone as the end of a limb: itself, the two bones above it,
    /// and what the top one hangs from (their indices in the pose).
    fn limb(&self) -> Option<[usize; 3]> {
        let ped = self.ped.as_ref()?;
        let end = ped.pose.bone_index(self.selected?)?;
        let middle = ped.pose.bones()[end].parent?;
        let top = ped.pose.bones()[middle].parent?;
        Some([top, middle, end])
    }

    /// G and R, as in Blender's view. G moves the bone in the view's plane
    /// (or, with Reach, its limb to where it's pulled), R turns it about the
    /// view's axis; X, Y, Z keep to an axis of the model, pressed
    /// again to the bone's own. Returns whether one is under way.
    fn modal_ui(&mut self, ui: &egui::Ui, response: &egui::Response, view: &CameraView, rect: Rect) -> bool {
        let pointer = ui.input(|i| i.pointer.clone());
        let cursor = pointer.latest_pos().unwrap_or(rect.center());
        let key = |k: egui::Key| ui.input(|i| i.key_pressed(k));
        if self.modal.is_none() {
            let plain = ui.input(|i| i.modifiers.is_none()) && !ui.ctx().egui_wants_keyboard_input();
            let kind = if !response.hovered() || !plain {
                None
            } else if key(egui::Key::G) {
                Some(Transform3::Grab)
            } else if key(egui::Key::R) {
                Some(Transform3::Rotate)
            } else {
                None
            };
            let (Some(kind), Some(tag)) = (kind, self.selected) else { return false };
            if self.ped.as_ref().and_then(|p| p.pose.bone_index(tag)).is_none() {
                return false;
            }
            if self.auto_key {
                self.step();
            }
            self.playing = false;
            self.modal =
                Some(Modal { kind, tag, origin: cursor, axis: None, bones: self.source.bones.clone(), loose: self.loose.clone() });
        }
        let Some(mut m) = self.modal.take() else { return false };
        // From how it was, each frame: nothing adds up.
        (self.source.bones, self.loose) = (m.bones.clone(), m.loose.clone());
        self.pose();
        if key(egui::Key::Escape) || pointer.button_pressed(egui::PointerButton::Secondary) {
            if self.auto_key {
                self.history.forget();
            }
            return true;
        }
        for (k, n) in [(egui::Key::X, 0), (egui::Key::Y, 1), (egui::Key::Z, 2)] {
            if key(k) {
                m.axis = match m.axis {
                    Some((a, false)) if a == n => Some((n, true)),
                    Some((a, true)) if a == n => None,
                    _ => Some((n, false)),
                };
            }
        }
        let Some((model, parent, limb)) = self.ped.as_ref().and_then(|p| {
            let i = p.pose.bone_index(m.tag)?;
            let parent = p.pose.bones()[i].parent.and_then(|b| p.pose.model(b)).unwrap_or(Mat4::IDENTITY);
            Some((p.pose.model(i)?, parent, self.limb().filter(|_| self.reach)))
        }) else {
            return false;
        };
        let (_, turned, at) = model.to_scale_rotation_translation();
        let (_, parent_turn, _) = parent.to_scale_rotation_translation();
        let eye = view.view.inverse();
        let (right, up, from) = (eye.x_axis.truncate().normalize(), eye.y_axis.truncate().normalize(), eye.w_axis.truncate());
        let towards = (from - at).normalize_or_zero();
        let pivot = view.project(at).map_or(rect.center(), |ndc| to_screen(rect, ndc));
        let axis = m.axis.map(|(n, own)| {
            let a = [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z][n];
            if own { turned * a } else { a }
        });
        let (name, frame) = (self.bone_name(m.tag), self.at());
        let what = match m.kind {
            Transform3::Rotate => {
                // The angle the mouse has gone round the bone, the way it's seen.
                let (a, b) = (m.origin - pivot, cursor - pivot);
                let angle = -(b.y.atan2(b.x) - a.y.atan2(a.x));
                let turn = match axis {
                    Some(a) => Quat::from_axis_angle(a, if a.dot(towards) >= 0.0 { angle } else { -angle }),
                    None => Quat::from_axis_angle(towards, angle),
                };
                self.set_rotation(m.tag, &name, frame, parent_turn.inverse() * (turn * turned).normalize());
                let degrees = (angle.to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
                format!("Rotate {degrees:.1}°")
            }
            Transform3::Grab => {
                // How far a point of the screen is at the bone's depth.
                let depth = (at - from).dot(-towards).abs().max(1e-3);
                let per_point = 2.0 * depth * (self.camera.fov_y * 0.5).tan() / rect.height().max(1.0);
                let d = cursor - m.origin;
                let mut by = (right * d.x - up * d.y) * per_point;
                if let Some(a) = axis {
                    by = a * by.dot(a);
                }
                match limb {
                    Some(limb) => self.reach_for(limb, at + by, frame),
                    None => {
                        let position = parent.inverse().transform_point3(at + by);
                        self.write_position(m.tag, &name, frame, position, self.auto_key);
                    }
                }
                format!("Move {:.3}  {:.3}  {:.3} m", by.x, by.y, by.z)
            }
        };
        self.pose();
        // What's being done, the line to the mouse, and the axis kept to.
        let painter = ui.painter().with_clip_rect(rect);
        let along = match m.axis {
            Some((n, own)) => format!(" along {} {}", if own { "local" } else { "global" }, ["X", "Y", "Z"][n]),
            None => String::new(),
        };
        let font = egui::FontId::proportional(theme::SIZE);
        painter.text(rect.left_top() + Vec2::new(12.0, 10.0), egui::Align2::LEFT_TOP, format!("{what}{along}"), font, theme::TEXT);
        if m.kind == Transform3::Rotate {
            painter.line_segment([pivot, cursor], Stroke::new(1.0, Color32::from_white_alpha(140)));
        }
        if let (Some(a), Some((n, _))) = (axis, m.axis) {
            let tint = [Color32::from_rgb(235, 80, 80), Color32::from_rgb(110, 200, 70), Color32::from_rgb(80, 140, 240)][n];
            let ends = [at - a * 5.0, at + a * 5.0].map(|p| view.project(p).map(|ndc| to_screen(rect, ndc)));
            if let [Some(p), Some(q)] = ends {
                painter.line_segment([p, q], Stroke::new(1.5, tint));
            }
        }
        if pointer.button_pressed(egui::PointerButton::Primary) || key(egui::Key::Enter) {
            self.say(format!("{what}{along}: {name}"));
        } else {
            self.modal = Some(m);
        }
        ui.ctx().request_repaint();
        true
    }

    /// The gizmo on the selected bone. Turning it keys the bone at the frame
    /// shown; with Reach, moving it keys the two bones above, which go where
    /// the bone is pulled (an arm by its hand). One undo step a drag. Returns
    /// whether it has the mouse.
    fn gizmo_ui(&mut self, ui: &egui::Ui, response: &egui::Response, view: &CameraView, rect: Rect) -> bool {
        let limb = self.limb().filter(|_| self.reach);
        let bone = self.selected.and_then(|tag| {
            let ped = self.ped.as_ref()?;
            let i = ped.pose.bone_index(tag)?;
            let model = ped.pose.model(i)?;
            let parent = ped.pose.bones()[i].parent.and_then(|p| ped.pose.model(p)).unwrap_or(Mat4::IDENTITY);
            Some((tag, model, parent))
        });
        let Some((tag, model, parent)) = bone else {
            self.turning = false;
            self.gizmo_press = false;
            return false;
        };
        let (_, rotation, position) = model.to_scale_rotation_translation();
        // The gizmo works in OpenGL's clip space: give it its own projection.
        let projection = glam::camera::rh::proj::opengl::perspective(
            self.camera.fov_y,
            rect.width() / rect.height().max(1.0),
            0.01,
            20_000.0,
        );
        self.gizmo.update_config(GizmoConfig {
            view_matrix: rows(view.view),
            projection_matrix: rows(projection),
            viewport: rect,
            modes: if limb.is_some() { GizmoMode::all_translate() } else { GizmoMode::all_rotate() },
            orientation: if limb.is_some() { GizmoOrientation::Global } else { GizmoOrientation::Local },
            pixels_per_point: ui.ctx().pixels_per_point(),
            ..*self.gizmo.config()
        });
        let pointer = ui.input(|i| i.pointer.clone());
        let cursor = pointer.hover_pos().unwrap_or_default();
        let pressed = pointer.button_pressed(egui::PointerButton::Primary);
        if pressed {
            self.gizmo_press = response.hovered() && self.gizmo.pick_preview((cursor.x, cursor.y));
        }
        let v = |v: glam::Vec3| mint::Vector3 { x: v.x as f64, y: v.y as f64, z: v.z as f64 };
        // Reaching moves a point; turning turns the bone where it is.
        let shown = if limb.is_some() { Quat::IDENTITY } else { rotation };
        let q = mint::Quaternion { v: v(shown.xyz()), s: shown.w as f64 };
        let target = Transform::from_scale_rotation_translation(v(glam::Vec3::ONE), q, v(position));
        let interaction = GizmoInteraction {
            cursor_pos: (cursor.x, cursor.y),
            hovered: response.hovered() || self.turning,
            drag_started: pressed && self.gizmo_press,
            dragging: pointer.button_down(egui::PointerButton::Primary) && self.gizmo_press,
        };
        match self.gizmo.update(interaction, &[target]) {
            Some((_, moved)) => {
                if !self.turning {
                    if self.auto_key {
                        self.step();
                    }
                    self.turning = true;
                    self.playing = false;
                }
                let frame = self.at();
                match (moved.first(), limb) {
                    (Some(t), Some(limb)) => {
                        let to =
                            glam::Vec3::new(t.translation.x as f32, t.translation.y as f32, t.translation.z as f32);
                        self.reach_for(limb, to, frame);
                    }
                    (Some(t), None) => {
                        let r = t.rotation;
                        let turned = Quat::from_xyzw(r.v.x as f32, r.v.y as f32, r.v.z as f32, r.s as f32).normalize();
                        // Relative to the parent, as a key is.
                        let (_, parent, _) = parent.to_scale_rotation_translation();
                        let name = self.bone_name(tag);
                        self.set_rotation(tag, &name, frame, parent.inverse() * turned);
                    }
                    (None, _) => {}
                }
            }
            None => self.turning = false,
        }
        if !pointer.button_down(egui::PointerButton::Primary) {
            self.gizmo_press = false;
        }
        self.gizmo_press || self.gizmo.is_focused() || self.turning
    }

    /// Keys a limb's two upper bones at a frame so that its end is at a point
    /// of the model (or as near as the limb is long).
    fn reach_for(&mut self, [top, middle, end]: [usize; 3], to: glam::Vec3, frame: f32) {
        let Some(ped) = &self.ped else { return };
        let pose = &ped.pose;
        let (Some(t), Some(m), Some(e)) = (pose.model(top), pose.model(middle), pose.model(end)) else { return };
        let above = pose.bones()[top].parent.and_then(|p| pose.model(p)).unwrap_or(Mat4::IDENTITY);
        let (_, upper, top_at) = t.to_scale_rotation_translation();
        let (_, lower, middle_at) = m.to_scale_rotation_translation();
        let (_, above, _) = above.to_scale_rotation_translation();
        let Some(r) = rf_editor::anim::ik::reach(top_at, middle_at, e.w_axis.truncate(), upper, lower, to) else {
            return;
        };
        let (top_tag, middle_tag, end_tag) = (pose.bones()[top].tag, pose.bones()[middle].tag, pose.bones()[end].tag);
        let (top_name, middle_name) = (self.bone_name(top_tag), self.bone_name(middle_tag));
        let (_, end_turn, _) = e.to_scale_rotation_translation();
        // Each relative to its parent, as keys are.
        self.set_rotation(top_tag, &top_name, frame, above.inverse() * r.upper);
        self.set_rotation(middle_tag, &middle_name, frame, r.upper.inverse() * r.lower);
        if self.keep_turn {
            let end_name = self.bone_name(end_tag);
            self.set_rotation(end_tag, &end_name, frame, (r.lower.inverse() * end_turn).normalize());
        }
    }

    fn paint_gizmo(&self, ui: &egui::Ui, rect: Rect) {
        if self.selected.is_none() || self.ped.is_none() || self.modal.is_some() {
            return;
        }
        let draw = self.gizmo.draw();
        let mesh = egui::Mesh {
            indices: draw.indices,
            vertices: draw
                .vertices
                .into_iter()
                .zip(draw.colors)
                .map(|(pos, [r, g, b, a])| egui::epaint::Vertex {
                    pos: pos.into(),
                    uv: Pos2::ZERO,
                    color: egui::Rgba::from_rgba_premultiplied(r, g, b, a).into(),
                })
                .collect(),
            ..Default::default()
        };
        ui.painter().with_clip_rect(rect).add(mesh);
    }
}

/// The bones a pose is mostly made with: the skeleton's own, without the
/// helpers the game drives itself (rolls, scales, attachment points).
fn main_bone(name: &str) -> bool {
    name.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("SKEL_"))
}

/// How the ped's own place is turned: a standing ped's when the animation has none.
fn mover_turn(mover: Option<Mover>) -> Quat {
    Quat::from_array(mover.unwrap_or(Mover::STANDING).rotation).normalize()
}

/// Reads a clip dictionary of the game's, for the base pose.
fn read_base(ctx: &egui::Context, game: &Arc<Game>, path: String) -> Job<Base> {
    let g = game.clone();
    Job::spawn(ctx, move || {
        let file = g.fs.find(&path).ok_or_else(|| anyhow::anyhow!("it isn't in the game's archives"))?;
        let dict = ycd::read(&g.fs.extract_loose(file)?, &g.hashes)?;
        let anims = dict.anim_dict().map(|d| d.lookup()).transpose()?.unwrap_or_default();
        let mut clips: Vec<(String, usize)> = dict
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((clip_label(e.clip.as_ref()?).to_owned(), i)))
            .collect();
        clips.sort();
        Ok(Base { dict, anims, clips })
    })
}
