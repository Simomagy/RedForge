//! Archetype thumbnails for the Objects pane (the C#'s `Thumbnails`): each
//! model rendered offscreen into a small texture that egui shows.
//!
//! Only the tiles on screen ask for one. Their models load on worker threads
//! through a [`ModelCache`] of their own (requests scrolled away are dropped
//! before their turn), a few are rendered per frame, and the model is
//! forgotten once its picture is made. The pictures are kept in a bounded LRU;
//! an evicted one's texture is reused for the next, so it stays registered
//! with egui.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::wgpu;
use glam::Vec3;
use rf_render::{GpuModel, OUTPUT_FORMAT, OrbitCamera, RenderTargets, ViewSettings};

use crate::game::Game;
use crate::map::models::{ModelCache, ModelState};
use crate::viewers::Gpu;

/// Thumbnails rendered in one frame at most.
const RENDERS_PER_FRAME: usize = 6;
/// Frame time for uploading decoded models to the GPU.
const UPLOAD_BUDGET: Duration = Duration::from_millis(4);
/// Pictures kept at least, besides twice those on screen.
const MIN_KEPT: usize = 128;
/// Textures of evicted pictures kept for reuse.
const SPARE: usize = 32;
/// How much of the view the model's box may fill.
const FILL: f32 = 0.92;

/// A tile's picture.
pub enum Thumb {
    Ready(egui::TextureId),
    Loading,
    /// Nothing to show: no model, or an empty one.
    None,
}

/// A picture's texture, registered with egui once.
struct Picture {
    texture: wgpu::Texture,
    id: egui::TextureId,
}

enum Entry {
    /// `used` is the last frame it was shown.
    Ready {
        picture: Picture,
        used: u64,
    },
    None,
}

pub struct Thumbnails {
    game: Arc<Game>,
    models: ModelCache,
    /// The pictures' size in pixels.
    size: u32,
    targets: Option<RenderTargets>,
    view: ViewSettings,
    entries: HashMap<u32, Entry>,
    spare: Vec<Picture>,
    frame: u64,
    /// The archetypes shown this frame without a picture yet, in order.
    wanted: Vec<u32>,
    /// Whether some of them are still to come after the last update.
    pending: bool,
    /// How many tiles asked for a picture this frame.
    shown: usize,
    last_evict: Instant,
}

impl Thumbnails {
    pub fn new(game: Arc<Game>, size: u32) -> Self {
        // Fewer workers than the map's: the map streams at the same time.
        let threads = std::thread::available_parallelism().map(|n| n.get() / 4).unwrap_or(2).clamp(2, 4);
        Self {
            // Few parsed files kept: the list is by name, so neighbours tend to
            // share their files, and pictures are made once.
            models: ModelCache::with_threads(game.clone(), threads, 32),
            game,
            size,
            targets: None,
            // Darker than the model viewer's, for tiles in a dark pane.
            view: ViewSettings { background: [0.06, 0.065, 0.075], ..Default::default() },
            entries: HashMap::new(),
            spare: Vec::new(),
            frame: 0,
            wanted: Vec::new(),
            pending: false,
            shown: 0,
            last_evict: Instant::now(),
        }
    }

    /// The colour around the models in the pictures, as egui shows it: for
    /// the tiles that have none, so they all look alike.
    pub fn background(&self) -> egui::Color32 {
        // The renderer's tonemap (RDR2's curve, `post.wgsl`) of the clear
        // colour, exposed to the middle grey as a picture that's mostly
        // background is.
        let curve = |x: f32| {
            let (a, b, c, d, e, f) = (0.1, 0.4, 0.1, 0.2, 0.01, 0.3);
            ((x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f)) - e / f
        };
        let bg = self.view.background.map(|v| v as f32);
        let lum = (bg[0] * 0.299 + bg[1] * 0.587 + bg[2] * 0.114).max(1e-5);
        let [r, g, b] = bg.map(|v| (curve(v * 0.45 / lum) / curve(4.0)).clamp(0.0, 1.0));
        egui::Rgba::from_rgb(r, g, b).into()
    }

    /// An archetype's picture, for a tile on screen this frame; asks for it
    /// when there's none yet.
    pub fn get(&mut self, archetype: u32) -> Thumb {
        self.shown += 1;
        match self.entries.get_mut(&archetype) {
            Some(Entry::Ready { picture, used }) => {
                *used = self.frame;
                Thumb::Ready(picture.id)
            }
            Some(Entry::None) => Thumb::None,
            None => {
                self.wanted.push(archetype);
                Thumb::Loading
            }
        }
    }

    /// Whether pictures asked for are still to come, after [`Thumbnails::update`].
    pub fn busy(&self) -> bool {
        self.pending
    }

    /// Makes the pictures new at another size (pixels).
    pub fn set_size(&mut self, gpu: &Gpu, size: u32) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.targets = None;
        self.entries.retain(|_, e| match e {
            Entry::Ready { picture, .. } => {
                gpu.free_texture(Some(picture.id));
                false
            }
            Entry::None => true,
        });
        for p in self.spare.drain(..) {
            gpu.free_texture(Some(p.id));
        }
    }

    /// Once a frame, after the tiles: loads the models of the pictures asked
    /// for, renders a few, and evicts the oldest beyond the limit.
    pub fn update(&mut self, gpu: &Gpu) {
        let wanted = std::mem::take(&mut self.wanted);
        let set: HashSet<u32> = wanted.iter().copied().collect();
        self.models.cancel_unless(|a| set.contains(&a));
        self.models.upload(&gpu.ctx, &gpu.renderer, UPLOAD_BUDGET);
        let mut rendered = 0;
        self.pending = false;
        for archetype in wanted {
            match self.models.state(archetype) {
                ModelState::None => {
                    self.entries.insert(archetype, Entry::None);
                }
                ModelState::Ready(model) if rendered < RENDERS_PER_FRAME => {
                    rendered += 1;
                    let entry = match self.render(gpu, archetype, &model) {
                        Some(picture) => Entry::Ready { picture, used: self.frame },
                        None => Entry::None,
                    };
                    self.entries.insert(archetype, entry);
                    self.models.forget(archetype);
                }
                ModelState::Loading | ModelState::Ready(_) => self.pending = true,
            }
        }
        self.evict(gpu);
        self.shown = 0;
        self.frame += 1;
    }

    /// Renders a model into a picture; `None` when it has nothing to draw.
    fn render(&mut self, gpu: &Gpu, archetype: u32, model: &GpuModel) -> Option<Picture> {
        if model.draw_count() == 0 {
            return None;
        }
        let (min, max) = self.bounds(archetype, model);
        let size = self.size;
        let targets = self.targets.get_or_insert_with(|| gpu.renderer.create_targets(&gpu.ctx, size, size));
        gpu.renderer.render_model(&gpu.ctx, targets, &framing(min, max), &self.view, model);
        let picture = self.spare.pop().unwrap_or_else(|| new_picture(gpu, size));
        let mut encoder =
            gpu.ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("thumbnail") });
        encoder.copy_texture_to_texture(
            targets.output.as_image_copy(),
            picture.texture.as_image_copy(),
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
        gpu.ctx.queue.submit([encoder.finish()]);
        Some(picture)
    }

    /// The box to frame: the archetype's, or the model's when the archetype
    /// has none.
    fn bounds(&self, archetype: u32, model: &GpuModel) -> (Vec3, Vec3) {
        let from_model = (Vec3::from(model.min), Vec3::from(model.max));
        let Some(a) = self.game.data().archetypes.get(&archetype).cloned() else { return from_model };
        let (min, max) = (Vec3::from(a.bb_min), Vec3::from(a.bb_max));
        let size = max - min;
        if min.is_finite() && max.is_finite() && size.min_element() >= 0.0 && size.length() > 0.01 {
            (min, max)
        } else {
            from_model
        }
    }

    /// Keeps the pictures shown recently: twice as many as on screen, and at
    /// least [`MIN_KEPT`].
    fn evict(&mut self, gpu: &Gpu) {
        let capacity = (self.shown * 2).max(MIN_KEPT);
        let ready = self.entries.values().filter(|e| matches!(e, Entry::Ready { .. })).count();
        // A little over the limit before evicting, not to sort every frame.
        if ready > capacity + capacity / 8 {
            let mut by_age: Vec<(u64, u32)> = self
                .entries
                .iter()
                .filter_map(|(a, e)| match e {
                    Entry::Ready { used, .. } if *used < self.frame => Some((*used, *a)),
                    _ => None,
                })
                .collect();
            by_age.sort_unstable();
            for (_, a) in by_age.into_iter().take(ready - capacity) {
                if let Some(Entry::Ready { picture, .. }) = self.entries.remove(&a) {
                    self.spare.push(picture);
                }
            }
            for p in self.spare.drain(SPARE.min(self.spare.len())..) {
                gpu.free_texture(Some(p.id));
            }
        }
        // Models loaded but scrolled away before their picture was made.
        if self.last_evict.elapsed() > Duration::from_secs(2) {
            self.models.evict(Duration::from_secs(10));
            self.last_evict = Instant::now();
        }
    }

    /// Frees the pictures' textures.
    pub fn free(&mut self, gpu: &Gpu) {
        for (_, e) in self.entries.drain() {
            if let Entry::Ready { picture, .. } = e {
                gpu.free_texture(Some(picture.id));
            }
        }
        for p in self.spare.drain(..) {
            gpu.free_texture(Some(p.id));
        }
    }
}

/// A texture for a picture: the renderer's output format, seen by egui as
/// plain UNORM (as `RenderTargets::display_view`).
fn new_picture(gpu: &Gpu, size: u32) -> Picture {
    let display = OUTPUT_FORMAT.remove_srgb_suffix();
    let texture = gpu.ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thumbnail"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[display],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor { format: Some(display), ..Default::default() });
    let mut id = None;
    gpu.show_texture(&mut id, &view);
    Picture { texture, id: id.expect("registered") }
}

/// A camera looking at a box from the model viewer's angle, from just far
/// enough for all of it to fit a square view.
fn framing(min: Vec3, max: Vec3) -> OrbitCamera {
    let mut camera = OrbitCamera { target: (min + max) * 0.5, distance: 1.0, ..Default::default() };
    let back = (camera.eye() - camera.target).normalize_or(Vec3::Y);
    let right = (-back).cross(Vec3::Z).normalize_or(Vec3::X);
    let up = right.cross(-back);
    let fit = (camera.fov_y * 0.5).tan() * FILL;
    let mut distance = 0.0f32;
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 != 0 { max.x } else { min.x },
            if i & 2 != 0 { max.y } else { min.y },
            if i & 4 != 0 { max.z } else { min.z },
        );
        let v = corner - camera.target;
        // The corner's distance from the eye along the view must reach its
        // offset across the view over `fit`.
        let across = v.dot(right).abs().max(v.dot(up).abs());
        distance = distance.max(v.dot(back) + across / fit);
    }
    camera.distance = distance.max(0.05);
    camera
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_fits_the_box() {
        let (min, max) = (Vec3::new(-1.0, -2.0, 0.0), Vec3::new(3.0, 1.0, 5.0));
        let camera = framing(min, max);
        let view = camera.camera_view(1.0);
        let mut widest = 0.0f32;
        for i in 0..8 {
            let c = Vec3::new(
                if i & 1 != 0 { max.x } else { min.x },
                if i & 2 != 0 { max.y } else { min.y },
                if i & 4 != 0 { max.z } else { min.z },
            );
            let ndc = view.project(c).unwrap();
            widest = widest.max(ndc.x.abs()).max(ndc.y.abs());
        }
        assert!(widest <= FILL + 1e-4, "{widest}");
        assert!(widest > FILL - 0.05, "{widest}");
    }
}
