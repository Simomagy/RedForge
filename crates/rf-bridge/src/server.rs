//! The server: a thread accepting connections on 127.0.0.1, a thread per
//! connection reading its requests, and the app's thread answering what needs
//! the editor through [`Bridge::poll`].

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rf_formats::hash::HashIndex;

use crate::model::{convert_xml, default_export_folder, export_model};
use crate::protocol::{Asset, Command, EDIT_EVENT, Placement, Reply};
use crate::{BridgeHost, ModelLoader, Transform, messages};

/// How long writing to Blender may block (events are written from the app's
/// thread).
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// For requests that were waiting on the app when it closed.
const CLOSING: &str = "RedForge is closing.";

/// The Blender bridge's server. Stops when dropped.
pub struct Bridge<K> {
    port: u16,
    /// Starts every object id, so ids from before a restart are told apart.
    session: String,
    /// The objects sent to Blender, by id (`<session>:<n>`, n from 1).
    objects: Vec<(String, K)>,
    export_folder: PathBuf,
    calls: Receiver<Call>,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

/// What the server's threads share.
struct Shared {
    stopping: AtomicBool,
    next_connection: AtomicU64,
    /// Every open connection, to close them when stopping.
    connections: Mutex<HashMap<u64, TcpStream>>,
    /// The connections that asked for events.
    listeners: Mutex<Vec<(u64, Arc<Writer>)>>,
    /// Gets the app to call `poll` (a repaint).
    wake: Box<dyn Fn() + Send + Sync>,
}

/// A connection's writing side: replies and events come from different
/// threads, a line at a time.
type Writer = Mutex<TcpStream>;

/// What a connection needs from the app's thread, with where the answer goes.
enum Call {
    Hello(Sender<Option<String>>),
    Hashes(Sender<Option<Arc<HashIndex>>>),
    Selection(Sender<Result<ExportJob, String>>),
    Import { file: String, data: Vec<u8>, reply: Sender<Result<(), String>> },
    Transform { id: String, transform: Transform, reply: Sender<Result<(), String>> },
}

/// A `selection` once the app has said what's selected: the models to export
/// (on the connection's thread) and the placements to send with them.
struct ExportJob {
    hashes: Arc<HashIndex>,
    folder: PathBuf,
    placements: Vec<Placement>,
    models: Vec<(String, ModelLoader)>,
}

impl<K: Clone + PartialEq> Bridge<K> {
    /// Listens on 127.0.0.1 at `port` (0 picks a free one). `wake` is called
    /// from the server's threads when there's something for [`Bridge::poll`]
    /// or the listener count changed.
    pub fn start(port: u16, wake: impl Fn() + Send + Sync + 'static) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        let (calls_tx, calls) = channel();
        let shared = Arc::new(Shared {
            stopping: AtomicBool::new(false),
            next_connection: AtomicU64::new(1),
            connections: Mutex::new(HashMap::new()),
            listeners: Mutex::new(Vec::new()),
            wake: Box::new(wake),
        });
        let s = shared.clone();
        let accept =
            thread::Builder::new().name("Blender bridge".into()).spawn(move || accept_loop(listener, s, calls_tx))?;
        Ok(Self {
            port,
            session: new_session(),
            objects: Vec::new(),
            export_folder: default_export_folder(),
            calls,
            shared,
            accept: Some(accept),
        })
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// This run's session, which starts the object ids.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// Where models are exported for Blender (one folder per model file):
    /// [`default_export_folder`] unless changed.
    pub fn export_folder(&self) -> &Path {
        &self.export_folder
    }

    /// Exports models somewhere else. The add-on only cleans up and reloads
    /// pictures in the default folder.
    pub fn set_export_folder(&mut self, folder: PathBuf) {
        self.export_folder = folder;
    }

    /// How many Blenders are listening (shown as "Blender connected").
    pub fn listeners(&self) -> usize {
        self.shared.listeners.lock().unwrap().len()
    }

    /// Tells the listening Blenders to edit the selection: each asks for it.
    /// False when none is listening.
    pub fn edit_selection(&self) -> bool {
        // (copied, so a connection's lock is never taken while holding the list's)
        let listeners: Vec<Arc<Writer>> =
            self.shared.listeners.lock().unwrap().iter().map(|(_, w)| w.clone()).collect();
        for writer in &listeners {
            let _ = write_line(writer, EDIT_EVENT);
        }
        !listeners.is_empty()
    }

    /// Answers the requests waiting on the app; call it once per frame on the
    /// app's thread (the thread `host` lives on).
    pub fn poll<H: BridgeHost<Object = K>>(&mut self, host: &mut H) {
        // (a send fails when the requester went away, which is no matter)
        while let Ok(call) = self.calls.try_recv() {
            match call {
                Call::Hello(reply) => drop(reply.send(host.project_name())),
                Call::Hashes(reply) => drop(reply.send(host.hashes())),
                Call::Selection(reply) => drop(reply.send(self.selection(host))),
                Call::Import { file, data, reply } => drop(reply.send(host.import(&file, data))),
                Call::Transform { id, transform, reply } => drop(reply.send(self.transform(host, &id, transform))),
            }
        }
    }

    /// The selected objects, with ids, and their models (each file once).
    fn selection<H: BridgeHost<Object = K>>(&mut self, host: &mut H) -> Result<ExportJob, String> {
        let items = host.selection();
        let hashes = host.hashes();
        let Some(hashes) = hashes.filter(|_| !items.is_empty()) else { return Err(messages::SELECT_FIRST.into()) };
        let mut placements = Vec::with_capacity(items.len());
        let mut models: Vec<(String, ModelLoader)> = Vec::new();
        for item in items {
            let id = self.object_id(&item.object);
            let model = item.model;
            placements.push(Placement::new(id, item.name, model.file.clone(), model.piece, &item.transform));
            if !models.iter().any(|(f, _)| f.eq_ignore_ascii_case(&model.file)) {
                models.push((model.file, model.load));
            }
        }
        Ok(ExportJob { hashes, folder: self.export_folder.clone(), placements, models })
    }

    /// The id of an object sent to Blender: the one it had, or a new one.
    fn object_id(&mut self, object: &K) -> String {
        if let Some((id, _)) = self.objects.iter().find(|(_, o)| o == object) {
            return id.clone();
        }
        let id = format!("{}:{}", self.session, self.objects.len() + 1);
        self.objects.push((id.clone(), object.clone()));
        id
    }

    fn transform<H: BridgeHost<Object = K>>(&self, host: &mut H, id: &str, transform: Transform) -> Result<(), String> {
        match self.objects.iter().find(|(i, _)| i == id) {
            Some((_, object)) => host.transform(object, transform),
            None if id.starts_with(&self.session) => Err(messages::UNKNOWN_OBJECT.into()),
            None => Err(messages::RESTARTED.into()),
        }
    }
}

impl<K> Drop for Bridge<K> {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Wake the accept loop, so it sees it's stopping and lets go of the port.
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let woke = TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok();
        // Blender's listener sees RedForge go, and waits for it again.
        for stream in self.shared.connections.lock().unwrap().values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if woke && let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// 8 hex digits, new each run (the C# took them from a new GUID).
fn new_session() -> String {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    h.write_u32(std::process::id());
    format!("{:08x}", h.finish() as u32)
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, calls: Sender<Call>) {
    for stream in listener.incoming() {
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
        match stream {
            Ok(stream) => {
                let id = shared.next_connection.fetch_add(1, Ordering::Relaxed);
                let (shared, calls) = (shared.clone(), calls.clone());
                let spawned = thread::Builder::new()
                    .name(format!("Blender bridge {id}"))
                    .spawn(move || Connection::serve(stream, id, shared, calls));
                if let Err(e) = spawned {
                    log::warn!("Blender bridge: {e}");
                }
            }
            Err(e) => {
                log::warn!("Blender bridge: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn write_line(writer: &Writer, line: &str) -> io::Result<()> {
    let mut w = writer.lock().unwrap();
    w.write_all(format!("{line}\n").as_bytes())
}

/// One connection from Blender.
struct Connection {
    id: u64,
    shared: Arc<Shared>,
    calls: Sender<Call>,
    writer: Arc<Writer>,
}

impl Connection {
    fn serve(stream: TcpStream, id: u64, shared: Arc<Shared>, calls: Sender<Call>) {
        let _ = stream.set_nodelay(true);
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let (Ok(reading), Ok(writing), Ok(kept)) = (stream.try_clone(), stream.try_clone(), stream.try_clone()) else {
            return;
        };
        shared.connections.lock().unwrap().insert(id, kept);
        let conn = Connection { id, shared, calls, writer: Arc::new(Mutex::new(writing)) };
        if !conn.shared.stopping.load(Ordering::SeqCst) {
            conn.read_requests(reading);
        }
        conn.close();
        let _ = stream.shutdown(Shutdown::Both);
    }

    fn read_requests(&self, stream: TcpStream) {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return, // (Blender went away)
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            let reply = match Command::parse(text.trim_end_matches(['\n', '\r'])) {
                Ok(Command::Listen) => {
                    self.listen();
                    continue;
                }
                Ok(command) => self.handle(command),
                Err(e) => Reply::Error(e),
            };
            if write_line(&self.writer, &reply.to_json()).is_err() {
                return;
            }
        }
    }

    /// Events go to this connection from now on. The reply goes first: the
    /// add-on takes the first line for it.
    fn listen(&self) {
        {
            let mut w = self.writer.lock().unwrap();
            let mut listeners = self.shared.listeners.lock().unwrap();
            if !listeners.iter().any(|(id, _)| *id == self.id) {
                listeners.push((self.id, self.writer.clone()));
            }
            drop(listeners);
            let _ = w.write_all(format!("{}\n", Reply::Done.to_json()).as_bytes());
        }
        (self.shared.wake)();
    }

    fn close(&self) {
        let removed = {
            let mut listeners = self.shared.listeners.lock().unwrap();
            let before = listeners.len();
            listeners.retain(|(id, _)| *id != self.id);
            listeners.len() != before
        };
        if removed {
            (self.shared.wake)();
        }
        self.shared.connections.lock().unwrap().remove(&self.id);
    }

    fn handle(&self, command: Command) -> Reply {
        let result = match command {
            Command::Hello => self.ui(Call::Hello).map(|project| Reply::Hello { project }),
            Command::Listen => Ok(Reply::Done), // (handled before)
            Command::Selection => self.ui(Call::Selection).and_then(|job| job).map(export),
            Command::Import { xml, file } => self.import(&xml, &file),
            Command::Transform { id, transform } => {
                self.ui(|reply| Call::Transform { id, transform, reply }).and_then(|r| r).map(|_| Reply::Done)
            }
            Command::Unknown(cmd) => Err(format!("Unknown command: {cmd}")),
        };
        result.unwrap_or_else(Reply::Error)
    }

    /// Asks the app's thread, and waits for the answer.
    fn ui<T>(&self, call: impl FnOnce(Sender<T>) -> Call) -> Result<T, String> {
        let (reply, answer) = channel();
        self.calls.send(call(reply)).map_err(|_| CLOSING.to_owned())?;
        (self.shared.wake)();
        answer.recv().map_err(|_| CLOSING.to_owned())
    }

    /// An edited model from Blender: converted here, then to the app for the
    /// project.
    fn import(&self, xml: &str, file: &str) -> Result<Reply, String> {
        let path = Path::new(xml);
        if !path.is_file() {
            return Err(format!("Not found: {xml}"));
        }
        // (only the name: the app decides where it goes)
        let file = file.rsplit(['/', '\\']).next().unwrap_or_default().to_owned();
        if file.is_empty() || file == "." || file == ".." {
            return Err(format!("Not a file name to import as: {file:?}"));
        }
        let hashes = self.ui(Call::Hashes)?.ok_or(messages::LOADING)?;
        let data = guarded(|| convert_xml(path, &hashes))?;
        self.ui(|reply| Call::Import { file: file.clone(), data, reply })??;
        Ok(Reply::Imported { file })
    }
}

/// Exports the selection's models (each into its own folder, emptied first)
/// and makes the reply: an error only when none could be exported.
fn export(job: ExportJob) -> Reply {
    let mut assets = Vec::new();
    let mut errors = Vec::new();
    for (file, load) in job.models {
        let folder = job.folder.join(&file);
        let exported = guarded(|| {
            // (the name makes a folder that gets deleted: it must be just a name)
            if file.is_empty() || file.contains(['/', '\\']) || file == "." || file == ".." {
                return Err("not a file name".into());
            }
            if folder.exists() {
                std::fs::remove_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
            }
            std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
            export_model(&file, &load()?, &job.hashes, &folder)
        });
        match exported {
            Ok(xml) => assets.push(Asset { file, xml: xml.to_string_lossy().into_owned() }),
            Err(e) => errors.push(format!("{file}: {e}")),
        }
    }
    if assets.is_empty() {
        return Reply::Error(errors.join("\n"));
    }
    Reply::Selection { assets, placements: job.placements, errors }
}

/// Runs file work that could panic on a bad file, as an error instead (the
/// connection would close without a reply).
fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("unknown error");
        Err(format!("failed: {message}"))
    })
}
