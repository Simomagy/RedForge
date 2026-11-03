//! The animation viewer: the clips of a clip dictionary (ycd) played on a ped
//! or animal, as the C#'s `AnimViewer`.
//!
//! The ped is put together from its parts as the game's metapeds have it
//! (`rf_game::peds`), skinned to its skeleton and posed by the clip each frame.
//! The C# always starts with `mp_male`; here a creature's clips (`creatures_*`,
//! `amb_creature*`...) start on the animal they name when there's one.
//!
//! For testing: `REDFORGE_ANIM_PED` picks the ped and `REDFORGE_ANIM_TIME`
//! stops the clip at a time (seconds), for the app's automatic screenshots.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Sense, Vec2};
use rf_formats::formats::ycd::{self, ClipDictionary, ClipKind};
use rf_formats::rpf8::FileRef;
use rf_game::peds::{LoadedPed, Peds};
use rf_render::{DebugView, ViewSettings};
use rf_render::{GpuModel, GpuTexture, ModelOptions, NoTextures, OrbitCamera, Pose, PosedModel, RenderTargets};

use super::{Gpu, Job, status_ui};
use crate::game::Game;

/// The ped the C# plays clips on.
const DEFAULT_PED: &str = "mp_male";

struct Loaded {
    dict: ClipDictionary,
    /// The animations of the embedded dictionary by hash: their index in it.
    anims: HashMap<u32, usize>,
    /// The clips' indices in `dict.entries`, sorted by name.
    clips: Vec<usize>,
    peds: Arc<Peds>,
}

impl Loaded {
    fn clip(&self, i: usize) -> &ycd::Clip {
        self.dict.entries[self.clips[i]].clip.as_ref().unwrap()
    }

    fn anim(&self, hash: u32) -> Option<&ycd::Anim> {
        let d = self.dict.anim_dict()?;
        d.entries.get(*self.anims.get(&hash)?)?.anim.as_ref()
    }

    /// The C#'s `ClipInfo`: frames and animations, or why nothing plays.
    fn info(&self, clip: &ycd::Clip) -> String {
        let anims: Vec<&ycd::Anim> = clip.animation_hashes().iter().filter_map(|h| self.anim(*h)).collect();
        if anims.is_empty() {
            return "its animations are in another dictionary, nothing to play".into();
        }
        let frames = anims.iter().map(|a| a.frame_count).max().unwrap_or(0);
        let s = if anims.len() == 1 { "" } else { "s" };
        format!("{frames} frames, {} animation{s}", anims.len())
    }
}

/// A clip's name as listed: without its pack and extension (`pack:/walk.clip`).
fn clip_label(clip: &ycd::Clip) -> &str {
    let name = clip.name.as_deref().unwrap_or("");
    let name = name.rsplit(['/', '\\', ':']).next().unwrap_or(name);
    name.strip_suffix(".clip").unwrap_or(name)
}

/// The ped to start with: for a creature's clips, the animal their name has
/// (`creatures_mammal@cow@...` → `a_c_cow_01`), else the C#'s default.
fn default_ped(peds: &Peds, file_name: &str) -> String {
    let name = file_name.to_lowercase();
    if name.contains("creature") {
        for word in name.trim_end_matches(".ycd").split(['@', '_']).filter(|w| w.len() >= 3) {
            let prefix = format!("a_c_{word}");
            let exact = [format!("{prefix}_01"), prefix.clone()];
            if let Some(p) = exact.iter().find(|p| peds.init_data(p).is_some()) {
                return p.clone();
            }
            if let Some(p) = peds.names().iter().find(|n| n.starts_with(&format!("{prefix}_"))) {
                return p.clone();
            }
        }
    }
    DEFAULT_PED.to_owned()
}

/// A ped on the GPU: its parts, posed by one skeleton.
struct GpuPed {
    name: String,
    pose: Pose,
    models: Vec<GpuModel>,
    min: glam::Vec3,
    max: glam::Vec3,
    missing: Vec<String>,
}

impl GpuPed {
    /// `RDR2Ped.BuildPiece`: the fragment's skeleton, then each part's
    /// skeletons and drawable, with the part's textures.
    fn new(gpu: &Gpu, hashes: &rf_formats::hash::HashIndex, ped: LoadedPed) -> Self {
        let mut pose = Pose::default();
        if let Some(s) = &ped.skeleton {
            pose.attach(s);
        }
        let upload = |t: &Option<rf_formats::formats::ytd::TextureBase>| {
            let t = t.as_ref()?;
            GpuTexture::upload(&gpu.ctx, t).map_err(|e| log::warn!("{e}")).ok().map(Arc::new)
        };
        let mut models = Vec::with_capacity(ped.parts.len());
        let (mut min, mut max) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
        for part in &ped.parts {
            pose.attach_drawable(&part.drawable);
            let options = ModelOptions {
                skin: true,
                overrides: [&part.textures[0], &part.textures[1], &part.textures[2]].map(upload),
                palette: upload(&part.palette).map(|p| (p, part.tints)),
            };
            let d = &part.drawable;
            let m =
                GpuModel::from_drawable_with(&gpu.ctx, &gpu.renderer, &part.name, d, hashes, &mut NoTextures, &options);
            min = min.min(m.min.into());
            max = max.max(m.max.into());
            models.push(m);
        }
        if min.x > max.x {
            (min, max) = pose.bounds().unwrap_or((glam::Vec3::splat(-1.0), glam::Vec3::splat(1.0)));
        }
        Self { name: ped.name, pose, models, min, max, missing: ped.missing }
    }
}

pub struct AnimViewer {
    pub title: String,
    pub path: String,
    game: Arc<Game>,
    job: Job<Loaded>,
    selected: usize,
    filter: String,
    ped_filter: String,
    /// The ped being loaded, by name.
    ped_job: Option<(String, Job<LoadedPed>)>,
    ped: Option<GpuPed>,
    status: String,
    /// Seconds into the clip.
    time: f32,
    playing: bool,
    speed: f32,
    looping: bool,
    last_frame: Option<Instant>,
    camera: OrbitCamera,
    view: ViewSettings,
    targets: Option<RenderTargets>,
    texture_id: Option<egui::TextureId>,
}

impl AnimViewer {
    pub fn open(ctx: &egui::Context, game: &Arc<Game>, file: FileRef) -> Self {
        let path = game.path(file);
        let title = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let g = game.clone();
        let job = Job::spawn(ctx, move || {
            let dict = ycd::read(&g.fs.extract_loose(file)?, &g.hashes)?;
            let anims = dict.anim_dict().map(|d| d.lookup()).transpose()?.unwrap_or_default();
            let mut clips: Vec<usize> = (0..dict.entries.len()).filter(|&i| dict.entries[i].clip.is_some()).collect();
            clips.sort_by_cached_key(|&i| {
                dict.entries[i].clip.as_ref().and_then(|c| c.name.as_deref()).unwrap_or("").to_uppercase()
            });
            // The ped list (a metadata file, quick to read); the shop items it
            // reads for multiplayer peds are kept with it.
            let peds = Arc::new(Peds::load(&g.fs, &g.hashes, &g.data()));
            Ok(Loaded { dict, anims, clips, peds })
        });
        let time = std::env::var("REDFORGE_ANIM_TIME").ok().and_then(|t| t.parse().ok());
        Self {
            title,
            path,
            game: game.clone(),
            job,
            selected: 0,
            filter: String::new(),
            ped_filter: String::new(),
            ped_job: None,
            ped: None,
            status: String::new(),
            time: time.unwrap_or(0.0),
            playing: time.is_none(),
            speed: 1.0,
            looping: true,
            last_frame: None,
            camera: OrbitCamera::default(),
            view: ViewSettings::default(),
            targets: None,
            texture_id: None,
        }
    }

    pub fn free(&mut self, gpu: &Gpu) {
        gpu.free_texture(self.texture_id.take());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu) {
        if self.job.poll()
            && let Job::Done(l) = &self.job
        {
            let ped = std::env::var("REDFORGE_ANIM_PED").unwrap_or_else(|_| default_ped(&l.peds, &self.title));
            self.ped_job = Some(load_ped(ui.ctx(), &self.game, &l.peds, ped));
        }
        let Job::Done(loaded) = &self.job else {
            status_ui(ui, &self.job, &self.title);
            return;
        };
        if let Some((name, job)) = &mut self.ped_job
            && job.poll()
        {
            let name = name.clone();
            match std::mem::replace(job, Job::Failed(String::new())) {
                Job::Done(ped) => {
                    let ped = GpuPed::new(gpu, &self.game.hashes, ped);
                    self.camera.frame(ped.min, ped.max);
                    self.status = format!("Playing on {name}");
                    if !ped.missing.is_empty() {
                        self.status += &format!(" ({} parts not found)", ped.missing.len());
                    }
                    self.ped = Some(ped);
                }
                Job::Failed(e) => self.status = format!("Can't load {name}: {e}"),
                Job::Running(_) => unreachable!(),
            }
            self.ped_job = None;
        }
        if loaded.clips.is_empty() {
            ui.label("This clip dictionary has no clips.");
            return;
        }
        let peds = loaded.peds.clone();
        self.selected = self.selected.min(loaded.clips.len() - 1);
        let clip = loaded.clip(self.selected);
        let duration = clip.duration();

        // Advance the time.
        let now = Instant::now();
        if self.playing
            && let Some(last) = self.last_frame
        {
            self.time += (now - last).as_secs_f32() * self.speed;
            if !self.looping && self.time >= duration {
                self.time = duration;
                self.playing = false;
            }
        }
        self.last_frame = Some(now);
        if self.looping && duration > 0.0 {
            self.time = self.time.rem_euclid(duration);
        }

        let mut pick_ped = None;
        egui::Panel::top(egui::Id::new(("anim-bar", &self.path))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let play = if self.playing {
                    crate::theme::icon_text(crate::theme::icon::PAUSE, "Pause")
                } else {
                    crate::theme::icon_text(crate::theme::icon::PLAY, "Play")
                };
                if ui.button(play).clicked() {
                    self.playing = !self.playing;
                    if self.playing && !self.looping && self.time >= duration {
                        self.time = 0.0;
                    }
                }
                let mut t = self.time;
                let slider = egui::Slider::new(&mut t, 0.0..=duration.max(0.001)).show_value(false);
                if ui.add_sized([280.0, 18.0], slider).changed() {
                    // Dragged: stay on that frame.
                    self.playing = false;
                    self.time = t;
                }
                ui.label(format!("{:.2} / {duration:.2} s", self.time.min(duration)));
                ui.separator();
                ui.add(egui::DragValue::new(&mut self.speed).range(0.05..=4.0).speed(0.01).prefix("Speed ×"));
                ui.checkbox(&mut self.looping, "Loop");
                ui.separator();
                ui.label("Ped");
                let current = self
                    .ped_job
                    .as_ref()
                    .map(|(n, _)| n.as_str())
                    .or(self.ped.as_ref().map(|p| p.name.as_str()))
                    .unwrap_or("")
                    .to_owned();
                egui::ComboBox::from_id_salt(("anim-ped", &self.path)).width(240.0).selected_text(&current).show_ui(
                    ui,
                    |ui| {
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
                    },
                );
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(loaded.info(clip)).weak());
                ui.separator();
                if let Some((name, _)) = &self.ped_job {
                    ui.spinner();
                    ui.label(format!("Loading {name}…"));
                } else {
                    ui.label(&self.status);
                }
            });
        });
        if let Some(name) = pick_ped
            && self.ped_job.as_ref().is_none_or(|(n, _)| *n != name)
            && self.ped.as_ref().is_none_or(|p| p.name != name)
        {
            self.ped_job = Some(load_ped(ui.ctx(), &self.game, &peds, name));
        }

        let mut pick_clip = None;
        egui::Panel::left(egui::Id::new(("anim-clips", &self.path))).default_size(260.0).show(ui, |ui| {
            ui.label(egui::RichText::new(&self.path).small().weak());
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter clips"));
            ui.separator();
            let f = self.filter.to_lowercase();
            let shown: Vec<usize> =
                (0..loaded.clips.len()).filter(|&i| clip_label(loaded.clip(i)).to_lowercase().contains(&f)).collect();
            let row = ui.spacing().interact_size.y;
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row, shown.len(), |ui, range| {
                for &i in &shown[range] {
                    let c = loaded.clip(i);
                    let multi = if matches!(c.kind, ClipKind::Multi(_)) { " (multi)" } else { "" };
                    if ui.selectable_label(i == self.selected, format!("{}{multi}", clip_label(c))).clicked() {
                        pick_clip = Some(i);
                    }
                }
            });
        });
        if let Some(i) = pick_clip {
            // `SelectClip`: from the start, playing.
            self.selected = i;
            self.time = 0.0;
            self.playing = true;
        }

        let clip = loaded.clip(self.selected);
        if let Some(p) = &mut self.ped {
            p.pose.apply_clip(clip, |h| loaded.anim(h), self.time, self.looping);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            viewport_ui(
                ui,
                gpu,
                &mut self.camera,
                &self.view,
                &mut self.targets,
                &mut self.texture_id,
                self.ped.as_ref(),
            )
        });
        if self.playing || self.ped_job.is_some() {
            ui.ctx().request_repaint();
        }
    }
}

/// Starts reading a ped's files (`ChangePed`/`LoadPed`), named with the job.
fn load_ped(ctx: &egui::Context, game: &Arc<Game>, peds: &Arc<Peds>, name: String) -> (String, Job<LoadedPed>) {
    let (g, p, n) = (game.clone(), peds.clone(), name.clone());
    let job = Job::spawn(ctx, move || {
        let def =
            p.definition(&g.fs, &g.hashes, &g.data(), &n).ok_or_else(|| anyhow::anyhow!("there's no such ped"))?;
        Ok(rf_game::peds::load_ped(&g.fs, &g.hashes, &g.data(), &def)?)
    });
    (name, job)
}

/// The 3D view: orbit, pan and zoom as the model viewer, the ped drawn posed.
fn viewport_ui(
    ui: &mut egui::Ui,
    gpu: &Gpu,
    camera: &mut OrbitCamera,
    view: &ViewSettings,
    targets: &mut Option<RenderTargets>,
    texture_id: &mut Option<egui::TextureId>,
    ped: Option<&GpuPed>,
) {
    let size = ui.available_size().max(Vec2::splat(16.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    let (w, h) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
    let d = response.drag_delta();
    if response.dragged_by(egui::PointerButton::Primary) && d != Vec2::ZERO {
        camera.orbit(d.x * 0.008, d.y * 0.008);
    }
    if (response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle))
        && d != Vec2::ZERO
    {
        camera.pan(d.x / rect.height(), d.y / rect.height());
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            camera.zoom((-scroll * 0.002).exp());
        }
    }
    if response.double_clicked()
        && let Some(p) = ped
    {
        camera.frame(p.min, p.max);
    }
    let Some(ped) = ped else { return };
    if targets.as_ref().is_none_or(|t| t.width != w || t.height != h) {
        *targets = Some(gpu.renderer.create_targets(&gpu.ctx, w, h));
    }
    let t = targets.as_ref().unwrap();
    let models: Vec<PosedModel> = ped.models.iter().map(|m| PosedModel { model: m, lod: m.highest_lod() }).collect();
    let view = ViewSettings { debug: DebugView::Lit, ..view.clone() };
    gpu.renderer.render_posed_orbit(&gpu.ctx, t, camera, &view, &ped.pose, &models);
    let id = gpu.show_texture(texture_id, &t.display_view);
    ui.painter().image(
        id,
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}
