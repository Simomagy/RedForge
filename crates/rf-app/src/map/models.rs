//! Models by archetype, for the map and the Objects pane's thumbnails: decoded
//! on worker threads, uploaded to the GPU a few per frame, and dropped when
//! unused for a while.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rf_formats::formats::ydr::DrawableBase;
use rf_formats::formats::ytd::TextureDictionary;
use rf_formats::rpf8::FileRef;
use rf_render::assets::{read_model, read_texture_dictionary};
use rf_render::{GpuContext, GpuModel, GpuTexture, Renderer, TextureSource};
use rf_world::{ModelFiles, ModelKind};

use crate::game::Game;

/// Parsed files shared by the models that use them (a ydd or a texture
/// dictionary serves many archetypes).
#[derive(Default)]
struct FileCache {
    models: HashMap<FileRef, Arc<Vec<(u32, DrawableBase)>>>,
    dictionaries: HashMap<FileRef, Option<Arc<TextureDictionary>>>,
}

/// A decoded model, waiting for its GPU upload.
struct Decoded {
    archetype: u32,
    name: String,
    drawable: Option<DrawableBase>,
    txds: Vec<(FileRef, Arc<TextureDictionary>)>,
}

/// Where an archetype's model is at.
#[derive(Clone)]
pub enum ModelState {
    Loading,
    Ready(Arc<GpuModel>),
    /// No model (assetless archetypes, missing files).
    None,
}

struct Entry {
    slot: ModelState,
    last_used: Instant,
}

/// The models waiting for a worker, oldest first.
#[derive(Default)]
struct Queue {
    jobs: VecDeque<(u32, ModelFiles)>,
    /// Set when the cache goes: the workers stop.
    closed: bool,
}

#[derive(Default)]
struct Requests {
    queue: Mutex<Queue>,
    added: Condvar,
}

pub struct ModelCache {
    game: Arc<Game>,
    requests: Arc<Requests>,
    decoded: Receiver<Decoded>,
    models: HashMap<u32, Entry>,
    /// GPU textures by dictionary and lowercase name, shared between models.
    textures: HashMap<(FileRef, String), Option<Arc<GpuTexture>>>,
    files: Arc<Mutex<FileCache>>,
    /// How many parsed model files are kept at most (and half as many texture
    /// dictionaries) when evicting.
    kept_files: usize,
    /// Set when a model was forgotten: what only it used can go.
    trim: bool,
    pub uploaded_last_frame: usize,
    /// The game data's version the models came from.
    data_version: u64,
}

fn decode(game: &Game, files: &Mutex<FileCache>, archetype: u32, mf: &ModelFiles) -> Decoded {
    let name = game.hashes.name(archetype);
    let mut out = Decoded { archetype, name: name.clone(), drawable: None, txds: Vec::new() };
    let cached = files.lock().unwrap().models.get(&mf.file).cloned();
    let drawables = match cached {
        Some(d) => d,
        None => {
            let path = game.path(mf.file);
            let parsed = game
                .fs
                .extract_loose(mf.file)
                .map_err(rf_formats::Error::from)
                .and_then(|data| read_model(&path, &data, &game.hashes));
            let list: Vec<(u32, DrawableBase)> = match parsed {
                Ok(m) => m.name_hashes.into_iter().zip(m.drawables).map(|(h, (_, d))| (h, d)).collect(),
                Err(e) => {
                    log::warn!("{path}: {e}");
                    Vec::new()
                }
            };
            let list = Arc::new(list);
            files.lock().unwrap().models.insert(mf.file, list.clone());
            list
        }
    };
    out.drawable = match mf.kind {
        ModelKind::Ydd => drawables.iter().find(|(h, _)| *h == mf.entry).map(|(_, d)| d.clone()),
        ModelKind::Ydr | ModelKind::Yft => drawables.first().map(|(_, d)| d.clone()),
    };
    for &txd in &mf.txds {
        let cached = files.lock().unwrap().dictionaries.get(&txd).cloned();
        let dict = match cached {
            Some(d) => d,
            None => {
                let d = game
                    .fs
                    .extract_loose(txd)
                    .map_err(rf_formats::Error::from)
                    .and_then(|data| read_texture_dictionary(&data, &game.hashes))
                    .map(Arc::new)
                    .ok();
                files.lock().unwrap().dictionaries.insert(txd, d.clone());
                d
            }
        };
        if let Some(d) = dict {
            out.txds.push((txd, d));
        }
    }
    out
}

/// Textures for one model upload: its dictionaries, through the shared cache.
struct CachedTextures<'a> {
    ctx: &'a GpuContext,
    txds: &'a [(FileRef, Arc<TextureDictionary>)],
    cache: &'a mut HashMap<(FileRef, String), Option<Arc<GpuTexture>>>,
}

impl TextureSource for CachedTextures<'_> {
    fn texture(&mut self, name: &str) -> Option<Arc<GpuTexture>> {
        let lower = name.to_lowercase();
        for (file, dict) in self.txds {
            let key = (*file, lower.clone());
            if let Some(t) = self.cache.get(&key) {
                if t.is_some() {
                    return t.clone();
                }
                continue;
            }
            let found = dict.textures.as_ref().and_then(|v| {
                v.iter().flatten().find(|t| t.base.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            });
            let uploaded = found.and_then(|t| GpuTexture::upload(self.ctx, &t.base).ok().map(Arc::new));
            self.cache.insert(key, uploaded.clone());
            if uploaded.is_some() {
                return uploaded;
            }
        }
        None
    }
}

impl ModelCache {
    /// The map's cache: half the cores (2 to 8) decoding, and plenty of
    /// parsed files kept for the models around the camera.
    pub fn new(game: Arc<Game>) -> Self {
        let threads = std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(4).clamp(2, 8);
        Self::with_threads(game, threads, 2000)
    }

    /// A cache with `threads` workers, keeping up to `kept_files` parsed model
    /// files between evictions.
    pub fn with_threads(game: Arc<Game>, threads: usize, kept_files: usize) -> Self {
        let (dec_tx, dec_rx) = channel();
        let data_version = game.data_version();
        let files = Arc::new(Mutex::new(FileCache::default()));
        // A few workers share one queue.
        let requests = Arc::new(Requests::default());
        for _ in 0..threads.max(1) {
            let (r, tx, g, f) = (requests.clone(), dec_tx.clone(), game.clone(), files.clone());
            std::thread::spawn(move || {
                loop {
                    let (archetype, mf) = {
                        let mut q = r.queue.lock().unwrap();
                        loop {
                            if q.closed {
                                return;
                            }
                            if let Some(job) = q.jobs.pop_front() {
                                break job;
                            }
                            q = r.added.wait(q).unwrap();
                        }
                    };
                    if tx.send(decode(&g, &f, archetype, &mf)).is_err() {
                        return;
                    }
                }
            });
        }
        Self {
            game,
            requests,
            decoded: dec_rx,
            models: HashMap::new(),
            textures: HashMap::new(),
            files,
            kept_files,
            trim: false,
            uploaded_last_frame: 0,
            data_version,
        }
    }

    /// The model of an archetype if it's ready; starts loading it if not.
    pub fn get(&mut self, archetype: u32) -> Option<Arc<GpuModel>> {
        match self.state(archetype) {
            ModelState::Ready(m) => Some(m),
            _ => None,
        }
    }

    /// Where an archetype's model is at; starts loading it the first time.
    pub fn state(&mut self, archetype: u32) -> ModelState {
        let now = Instant::now();
        if let Some(e) = self.models.get_mut(&archetype) {
            e.last_used = now;
            return e.slot.clone();
        }
        let version = self.game.data_version();
        if version != self.data_version {
            // Other game data (the project's files changed): every model again.
            self.data_version = version;
            self.models.clear();
            self.textures.clear();
            *self.files.lock().unwrap() = FileCache::default();
            self.requests.queue.lock().unwrap().jobs.clear();
        }
        let slot = match rf_world::model_files(&self.game.data(), archetype) {
            Some(mf) => {
                self.requests.queue.lock().unwrap().jobs.push_back((archetype, mf));
                self.requests.added.notify_one();
                ModelState::Loading
            }
            None => ModelState::None,
        };
        self.models.insert(archetype, Entry { slot: slot.clone(), last_used: now });
        slot
    }

    /// Drops the requests still waiting for a worker that aren't `wanted` any
    /// more (scrolled away), so the workers get to the wanted ones sooner.
    pub fn cancel_unless(&mut self, wanted: impl Fn(u32) -> bool) {
        let mut q = self.requests.queue.lock().unwrap();
        q.jobs.retain(|(archetype, _)| {
            let keep = wanted(*archetype);
            if !keep {
                self.models.remove(archetype);
            }
            keep
        });
    }

    /// Drops an archetype's model now; the next [`ModelCache::state`] loads it
    /// again.
    pub fn forget(&mut self, archetype: u32) {
        self.trim |= self.models.remove(&archetype).is_some();
    }

    /// Uploads decoded models for up to `budget` of frame time.
    pub fn upload(&mut self, ctx: &GpuContext, renderer: &Renderer, budget: Duration) {
        let start = Instant::now();
        self.uploaded_last_frame = 0;
        while start.elapsed() < budget {
            let Ok(d) = self.decoded.try_recv() else { break };
            // Forgotten while it was being decoded: not wanted any more.
            if !self.models.contains_key(&d.archetype) {
                continue;
            }
            let slot = match &d.drawable {
                Some(drawable) => {
                    let mut source = CachedTextures { ctx, txds: &d.txds, cache: &mut self.textures };
                    let m = GpuModel::from_drawable(ctx, renderer, &d.name, drawable, &self.game.hashes, &mut source);
                    ModelState::Ready(Arc::new(m))
                }
                None => ModelState::None,
            };
            if let Some(e) = self.models.get_mut(&d.archetype) {
                e.slot = slot;
            }
            self.uploaded_last_frame += 1;
        }
    }

    /// Drops models (and the textures and parsed files only they used) not
    /// drawn for `unused_for`.
    pub fn evict(&mut self, unused_for: Duration) {
        let now = Instant::now();
        let before = self.models.len();
        self.models
            .retain(|_, e| matches!(e.slot, ModelState::Loading) || now.duration_since(e.last_used) < unused_for);
        if self.models.len() != before || std::mem::take(&mut self.trim) {
            self.textures.retain(|_, t| t.as_ref().is_none_or(|t| Arc::strong_count(t) > 1));
            let mut files = self.files.lock().unwrap();
            if files.models.len() > self.kept_files {
                files.models.clear();
            }
            if files.dictionaries.len() > self.kept_files / 2 {
                files.dictionaries.clear();
            }
        }
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        let ready = self.models.values().filter(|e| matches!(e.slot, ModelState::Ready(_))).count();
        let loading = self.models.values().filter(|e| matches!(e.slot, ModelState::Loading)).count();
        (ready, loading, self.textures.values().filter(|t| t.is_some()).count())
    }
}

impl Drop for ModelCache {
    fn drop(&mut self) {
        self.requests.queue.lock().unwrap().closed = true;
        self.requests.added.notify_all();
    }
}
