//! The worker threads that read collisions (ybns, and the bounds in models)
//! and make their parts, off the UI thread.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use rf_formats::formats::XmlContext;
use rf_formats::formats::ybn::{self, Bounds};
use rf_formats::formats::{ydr, yft};
use rf_formats::rpf8::FileRef;
use rf_game::materials::Materials;
use rf_world::{ModelFiles, ModelKind};

use super::bounds::{self, Part};
use crate::game::Game;

/// What a collision is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    /// A ybn, by name hash.
    Ybn(u32),
    /// An archetype's model, by name hash.
    Archetype(u32),
}

pub enum Job {
    Ybn(u32, FileRef),
    Model(u32, ModelFiles),
}

impl Job {
    fn owner(&self) -> Owner {
        match self {
            Job::Ybn(h, _) => Owner::Ybn(*h),
            Job::Model(a, _) => Owner::Archetype(*a),
        }
    }
}

/// A collision read and made into parts: none when there's nothing to show
/// (no bounds, or a file that doesn't read).
pub struct Loaded {
    pub owner: Owner,
    /// The ybn's file name, or the archetype's name.
    pub name: String,
    pub parts: Vec<Part>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Set when the loader goes: the workers stop.
    closed: bool,
}

/// A drawable dictionary's bounds, by entry name hash.
type DictionaryBounds = Arc<Vec<(u32, Option<Bounds>)>>;

/// What the workers share.
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    added: Condvar,
    materials: OnceLock<Materials>,
    /// The bounds of the drawable dictionaries read lately: a ydd serves many
    /// archetypes.
    dictionaries: Mutex<HashMap<FileRef, DictionaryBounds>>,
}

/// How many dictionaries' bounds are kept at most.
const KEPT_DICTIONARIES: usize = 64;

pub struct Loader {
    shared: Arc<Shared>,
    pub done: Receiver<Loaded>,
}

impl Loader {
    pub fn new(game: Arc<Game>, threads: usize) -> Self {
        let (tx, done) = channel();
        let shared = Arc::new(Shared::default());
        for _ in 0..threads.max(1) {
            let (s, g, tx): (_, _, Sender<Loaded>) = (shared.clone(), game.clone(), tx.clone());
            std::thread::spawn(move || {
                loop {
                    let job = {
                        let mut q = s.queue.lock().unwrap();
                        loop {
                            if q.closed {
                                return;
                            }
                            if let Some(job) = q.jobs.pop_front() {
                                break job;
                            }
                            q = s.added.wait(q).unwrap();
                        }
                    };
                    if tx.send(load(&g, &s, job)).is_err() {
                        return;
                    }
                }
            });
        }
        Self { shared, done }
    }

    pub fn request(&self, job: Job) {
        self.shared.queue.lock().unwrap().jobs.push_back(job);
        self.shared.added.notify_one();
    }

    /// Drops the jobs still waiting that aren't `wanted` any more, and gives
    /// their owners.
    pub fn cancel_unless(&self, wanted: impl Fn(Owner) -> bool) -> Vec<Owner> {
        let mut q = self.shared.queue.lock().unwrap();
        let mut cancelled = Vec::new();
        q.jobs.retain(|j| {
            let keep = wanted(j.owner());
            if !keep {
                cancelled.push(j.owner());
            }
            keep
        });
        cancelled
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().closed = true;
        self.shared.added.notify_all();
    }
}

fn load(game: &Game, shared: &Shared, job: Job) -> Loaded {
    let owner = job.owner();
    let (name, path, read) = match &job {
        Job::Ybn(_, file) => {
            let path = game.path(*file);
            let name = path.rsplit('/').next().unwrap_or_default().to_owned();
            (name, path, read_ybn(game, *file))
        }
        Job::Model(archetype, mf) => (game.hashes.name(*archetype), game.path(mf.file), model_bounds(game, shared, mf)),
    };
    let parts = match read {
        Ok(Some(b)) => bounds::parts(&b, shared.materials.get_or_init(|| Materials::load(&game.fs))),
        Ok(None) => Vec::new(),
        Err(e) => {
            log::warn!("{path}: {e}");
            Vec::new()
        }
    };
    Loaded { owner, name, parts }
}

fn read_ybn(game: &Game, file: FileRef) -> rf_formats::Result<Option<Bounds>> {
    let data = game.fs.extract_loose(file).map_err(rf_formats::Error::from)?;
    ybn::read(&data).map(Some)
}

/// The bounds of an archetype's model (the C#'s `Piece.Collider`): a
/// drawable's own, or a fragment's first physics LOD's (else its drawable's).
fn model_bounds(game: &Game, shared: &Shared, mf: &ModelFiles) -> rf_formats::Result<Option<Bounds>> {
    let cx = XmlContext { hashes: &game.hashes, folder: None };
    let read = || game.fs.extract_loose(mf.file).map_err(rf_formats::Error::from);
    match mf.kind {
        ModelKind::Ydr => Ok(ydr::read(&read()?, &cx)?.bounds),
        ModelKind::Yft => {
            let f = yft::read(&read()?, &cx)?;
            let lod = f.physics_lod_group.and_then(|g| g.physics_lod1).and_then(|l| l.bounds);
            Ok(lod.or(f.drawable.and_then(|d| d.bounds)))
        }
        ModelKind::Ydd => {
            let cached = shared.dictionaries.lock().unwrap().get(&mf.file).cloned();
            let entries = match cached {
                Some(e) => e,
                None => {
                    let dict = ydr::read_dictionary(&read()?, &cx)?;
                    let hashes = dict.hashes.unwrap_or_default();
                    let drawables = dict.drawables.unwrap_or_default();
                    let list: Vec<_> =
                        hashes.into_iter().zip(drawables).map(|(h, d)| (h, d.and_then(|d| d.bounds))).collect();
                    let list = Arc::new(list);
                    let mut cache = shared.dictionaries.lock().unwrap();
                    if cache.len() >= KEPT_DICTIONARIES {
                        cache.clear();
                    }
                    cache.insert(mf.file, list.clone());
                    list
                }
            };
            Ok(entries.iter().find(|(h, _)| *h == mf.entry).and_then(|(_, b)| b.clone()))
        }
    }
}
