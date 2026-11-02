//! The bridge over TCP with a client that talks like the add-on's `link.py`:
//! a connection per command, written with `json.dumps`'s spacing, one reply
//! line read back; and listener connections for events. The host is a fake
//! with models made from rf-formats' test XML.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rf_bridge::{Bridge, BridgeHost, ModelData, ModelSource, Quat, SelectedObject, Transform, Vec3, messages};
use rf_formats::formats::{self, XmlContext};
use rf_formats::hash::HashIndex;
use serde_json::{Value, json};

const DRAWABLE: &str = include_str!("../../rf-formats/tests/data/ydr/features.ydr.xml");
const DICTIONARY: &str = include_str!("../../rf-formats/tests/data/ydr/dict.ydd.xml");

struct Host {
    hashes: Option<Arc<HashIndex>>,
    project: Option<String>,
    /// Selected objects: key, model file.
    selected: Vec<(u32, &'static str)>,
    drawable: Vec<u8>,
    dictionary: Vec<u8>,
    imported: Vec<(String, Vec<u8>)>,
    moved: Vec<(u32, Transform)>,
}

fn placed(object: u32) -> Transform {
    Transform {
        position: Vec3::new(-321.5 + object as f32, 812.25, 118.0),
        rotation: Quat::from_rotation_z(0.5),
        scale: Vec3::ONE,
    }
}

impl BridgeHost for Host {
    type Object = u32;

    fn project_name(&self) -> Option<String> {
        self.project.clone()
    }

    fn hashes(&self) -> Option<Arc<HashIndex>> {
        self.hashes.clone()
    }

    fn selection(&mut self) -> Vec<SelectedObject<u32>> {
        self.selected
            .iter()
            .map(|&(object, file)| {
                let (piece, data) = match file {
                    "features.ydr" => ("features", Ok(self.drawable.clone())),
                    "dict.ydd" => ("aardvark", Ok(self.dictionary.clone())),
                    _ => ("broken", Err("Unable to load file".to_owned())),
                };
                let load = Box::new(move || data.map(|data| ModelData { data, texture_dictionaries: Vec::new() }));
                SelectedObject {
                    object,
                    name: format!("prop_{object}"),
                    transform: placed(object),
                    model: ModelSource { file: file.into(), piece: piece.into(), load },
                }
            })
            .collect()
    }

    fn import(&mut self, file: &str, data: Vec<u8>) -> Result<(), String> {
        if self.project.is_none() {
            return Err(messages::NO_PROJECT.into());
        }
        self.imported.push((file.into(), data));
        Ok(())
    }

    fn transform(&mut self, object: &u32, transform: Transform) -> Result<(), String> {
        self.moved.push((*object, transform));
        Ok(())
    }
}

fn host() -> Host {
    let hashes = Arc::new(HashIndex::new());
    // (the game's strings would know the drawables' names)
    for name in ["aardvark", "zebra"] {
        hashes.ensure(name);
    }
    let cx = XmlContext { hashes: &hashes, folder: None };
    let drawable = formats::from_xml("features.ydr.xml", DRAWABLE, &cx).unwrap();
    let dictionary = formats::from_xml("dict.ydd.xml", DICTIONARY, &cx).unwrap();
    Host {
        hashes: Some(hashes),
        project: Some("Test project".into()),
        selected: Vec::new(),
        drawable,
        dictionary,
        imported: Vec::new(),
        moved: Vec::new(),
    }
}

/// `json.dumps` of a value: `", "` and `": "` between items.
fn dumps(v: &Value) -> String {
    match v {
        Value::Array(items) => format!("[{}]", items.iter().map(dumps).collect::<Vec<_>>().join(", ")),
        Value::Object(map) => {
            let items: Vec<String> = map.iter().map(|(k, v)| format!("{}: {}", json!(k), dumps(v))).collect();
            format!("{{{}}}", items.join(", "))
        }
        other => other.to_string(),
    }
}

/// `link.request`: one connection, one line each way. Returns the reply.
fn request(port: u16, cmd: &str, args: &[(&str, Value)]) -> Value {
    let mut line = format!("{{\"cmd\": {}", json!(cmd));
    for (key, value) in args {
        line += &format!(", {}: {}", json!(key), dumps(value));
    }
    line += "}\n";
    let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    sock.write_all(line.as_bytes()).unwrap();
    let mut reply = String::new();
    BufReader::new(sock).read_line(&mut reply).unwrap();
    assert!(reply.ends_with('\n'), "{reply:?}");
    serde_json::from_str(&reply).unwrap()
}

/// Runs a client while this thread, the app's, polls the bridge.
fn run<T: Send>(bridge: &mut Bridge<u32>, host: &mut Host, client: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        let client = s.spawn(client);
        let start = Instant::now();
        while !client.is_finished() {
            bridge.poll(host);
            assert!(start.elapsed() < Duration::from_secs(60), "the client hangs");
            std::thread::sleep(Duration::from_millis(1));
        }
        client.join().unwrap()
    })
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rf-bridge-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn error(reply: &Value) -> &str {
    assert_eq!(reply["ok"], false, "{reply}");
    reply["error"].as_str().unwrap()
}

#[test]
fn commands_like_the_add_on() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let mut bridge = Bridge::<u32>::start(0, move || {
        w.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    let dir = temp_dir("commands");
    bridge.set_export_folder(dir.join("edit"));
    let port = bridge.port();
    let session = bridge.session().to_owned();
    assert_eq!(session.len(), 8);
    assert!(session.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    let mut host = host();

    // hello
    let reply = run(&mut bridge, &mut host, || request(port, "hello", &[]));
    assert_eq!(reply, json!({"app": "RedForge", "project": "Test project", "ok": true}));
    assert!(wakes.load(Ordering::SeqCst) > 0, "a request wakes the app");
    host.project = None;
    let reply = run(&mut bridge, &mut host, || request(port, "hello", &[]));
    assert_eq!(reply, json!({"app": "RedForge", "project": null, "ok": true}));

    // selection: nothing selected, then three objects, two sharing a model,
    // and one whose model can't be read.
    let reply = run(&mut bridge, &mut host, || request(port, "selection", &[]));
    assert_eq!(error(&reply), messages::SELECT_FIRST);
    host.selected = vec![(10, "features.ydr"), (20, "dict.ydd"), (30, "features.ydr"), (40, "broken.ydr")];
    let reply = run(&mut bridge, &mut host, || request(port, "selection", &[]));
    assert_eq!(reply["ok"], true, "{reply}");
    let assets = reply["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2);
    let edit = dir.join("edit");
    for (asset, file) in assets.iter().zip(["features.ydr", "dict.ydd"]) {
        assert_eq!(asset["file"], file);
        let xml = edit.join(file).join(format!("{file}.xml"));
        assert_eq!(asset["xml"], xml.to_string_lossy().as_ref());
        assert!(xml.is_file());
    }
    let dict_xml = std::fs::read_to_string(edit.join("dict.ydd").join("dict.ydd.xml")).unwrap();
    assert!(dict_xml.contains("<Hash>aardvark</Hash>"), "the piece is a drawable's Hash");
    assert_eq!(reply["errors"], json!(["broken.ydr: Unable to load file"]));
    let placements = reply["placements"].as_array().unwrap();
    let ids: Vec<&str> = placements.iter().map(|p| p["id"].as_str().unwrap()).collect();
    let expected: Vec<String> = (1..=4).map(|n| format!("{session}:{n}")).collect();
    assert_eq!(ids, expected);
    let first = &placements[0];
    assert_eq!(first["name"], "prop_10");
    assert_eq!(first["file"], "features.ydr");
    assert_eq!(first["piece"], "features");
    assert_eq!(placements[1]["piece"], "aardvark");
    let t = placed(10);
    let number = |v: &Value| v.as_f64().unwrap() as f32;
    let position: Vec<f32> = first["position"].as_array().unwrap().iter().map(number).collect();
    assert_eq!(position, t.position.to_array());
    let rotation: Vec<f32> = first["rotation"].as_array().unwrap().iter().map(number).collect();
    assert_eq!(rotation, [t.rotation.w, t.rotation.x, t.rotation.y, t.rotation.z]);
    assert_eq!(first["scale"], json!([1.0, 1.0, 1.0]));

    // Again, with an old file left in the export: the folder is emptied, the
    // objects keep their ids.
    std::fs::write(edit.join("features.ydr").join("old.txt"), "x").unwrap();
    host.selected = vec![(30, "features.ydr")];
    let reply = run(&mut bridge, &mut host, || request(port, "selection", &[]));
    assert_eq!(reply["placements"][0]["id"], format!("{session}:3"));
    assert!(!edit.join("features.ydr").join("old.txt").exists());
    // Only models that can't be read: an error.
    host.selected = vec![(40, "broken.ydr")];
    let reply = run(&mut bridge, &mut host, || request(port, "selection", &[]));
    assert_eq!(error(&reply), "broken.ydr: Unable to load file");

    // transform, as Send Changes sends it (Python doubles): turned 90° about Z.
    let id = format!("{session}:2");
    let half = std::f64::consts::FRAC_1_SQRT_2;
    let args = [
        ("id", json!(id)),
        ("position", json!([-300.5000000001, 800.0, 120.25])),
        ("rotation", json!([half, 0.0, 0.0, half])),
        ("scale", json!([1.0, 1.0, 2.0])),
    ];
    let reply = run(&mut bridge, &mut host, || request(port, "transform", &args));
    assert_eq!(reply, json!({"ok": true}));
    assert_eq!(host.moved.len(), 1);
    let (object, moved) = host.moved[0];
    assert_eq!(object, 20);
    assert_eq!(moved.position, Vec3::new(-300.5, 800.0, 120.25));
    assert!(moved.rotation.dot(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)) > 0.999_999);
    assert_eq!(moved.scale, Vec3::new(1.0, 1.0, 2.0));
    let mut args = args.clone();
    args[0].1 = json!(format!("{session}:99"));
    let reply = run(&mut bridge, &mut host, || request(port, "transform", &args));
    assert_eq!(error(&reply), messages::UNKNOWN_OBJECT);
    args[0].1 = json!("0badcafe:2");
    let reply = run(&mut bridge, &mut host, || request(port, "transform", &args));
    assert_eq!(error(&reply), messages::RESTARTED);
    let reply = run(&mut bridge, &mut host, || request(port, "transform", &args[..3]));
    assert!(error(&reply).contains("scale"));
    assert_eq!(host.moved.len(), 1);

    // import: Sollumz's export of an edited model, as Send Changes does it.
    let send = dir.join("send").join("features.ydr");
    std::fs::create_dir_all(&send).unwrap();
    let xml = send.join("features.ydr.xml");
    std::fs::write(&xml, DRAWABLE).unwrap();
    let args = [("xml", json!(xml.to_string_lossy())), ("file", json!("features.ydr"))];
    let reply = run(&mut bridge, &mut host, || request(port, "import", &args));
    assert_eq!(error(&reply), messages::NO_PROJECT);
    host.project = Some("Test project".into());
    let reply = run(&mut bridge, &mut host, || request(port, "import", &args));
    assert_eq!(reply, json!({"file": "features.ydr", "ok": true}));
    assert_eq!(host.imported.len(), 1);
    assert_eq!(host.imported[0], ("features.ydr".to_owned(), host.drawable.clone()));
    let missing = [("xml", json!(send.join("nothing.ydr.xml").to_string_lossy())), ("file", json!("nothing.ydr"))];
    let reply = run(&mut bridge, &mut host, || request(port, "import", &missing));
    assert!(error(&reply).starts_with("Not found: "));
    let reply = run(&mut bridge, &mut host, || request(port, "import", &[]));
    assert_eq!(error(&reply), "Not found: ");
    host.hashes = None;
    let reply = run(&mut bridge, &mut host, || request(port, "import", &args));
    assert_eq!(error(&reply), messages::LOADING);
    let reply = run(&mut bridge, &mut host, || request(port, "selection", &[]));
    assert_eq!(error(&reply), messages::SELECT_FIRST);
    assert_eq!(host.imported.len(), 1);

    // Unknown commands and bad lines get an error, and the connection goes on.
    let reply = run(&mut bridge, &mut host, || request(port, "dance", &[]));
    assert_eq!(error(&reply), "Unknown command: dance");
    let replies = run(&mut bridge, &mut host, || {
        let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        sock.write_all(b"not json\r\n{\"cmd\": \"hello\"}\n").unwrap();
        let mut lines = BufReader::new(sock).lines();
        let mut next = || serde_json::from_str::<Value>(&lines.next().unwrap().unwrap()).unwrap();
        (next(), next())
    });
    assert!(error(&replies.0).starts_with("Bad request"));
    assert_eq!(replies.1["app"], "RedForge");

    drop(bridge);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Like link.py's listener thread: listen, take the reply, then read events.
fn listen(port: u16) -> BufReader<TcpStream> {
    let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    sock.write_all(b"{\"cmd\": \"listen\"}\n").unwrap();
    let mut reader = BufReader::new(sock);
    let mut reply = String::new();
    reader.read_line(&mut reply).unwrap();
    assert_eq!(reply, "{\"ok\":true}\n");
    reader
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn listeners_get_edit_events() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let bridge = Bridge::<u32>::start(0, move || {
        w.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    let port = bridge.port();
    assert_eq!(bridge.listeners(), 0);
    assert!(!bridge.edit_selection(), "nobody listens");

    let mut a = listen(port);
    // (listening needs nothing from the app: no poll here)
    assert_eq!(bridge.listeners(), 1);
    assert!(wakes.load(Ordering::SeqCst) > 0, "the count changed");
    let mut b = listen(port);
    assert_eq!(bridge.listeners(), 2);

    assert!(bridge.edit_selection());
    for reader in [&mut a, &mut b] {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "{\"event\":\"edit\"}\n");
        let event: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(event["event"], "edit");
    }

    // Blender closing one connection: one listener left.
    drop(b);
    wait_for("the closed listener goes", || bridge.listeners() == 1);
    assert!(bridge.edit_selection());
    let mut line = String::new();
    a.read_line(&mut line).unwrap();
    assert_eq!(line, "{\"event\":\"edit\"}\n");

    // RedForge closing: the listener sees the end, and the port is free.
    drop(bridge);
    let mut line = String::new();
    assert_eq!(a.read_line(&mut line).unwrap_or(0), 0, "{line:?}");
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    let again = Bridge::<u32>::start(port, || {}).expect("the port can be used again");
    drop(again);
}

#[test]
fn requests_end_when_the_app_closes() {
    let bridge = Bridge::<u32>::start(0, || {}).unwrap();
    let port = bridge.port();
    // A request that waits on the app, which closes without answering: the
    // add-on sees the connection close ("RedForge closed the connection").
    let client = std::thread::spawn(move || {
        let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        sock.write_all(b"{\"cmd\": \"hello\"}\n").unwrap();
        let mut line = String::new();
        BufReader::new(sock).read_line(&mut line).unwrap_or(0)
    });
    std::thread::sleep(Duration::from_millis(50));
    drop(bridge);
    assert_eq!(client.join().unwrap(), 0);
}

#[test]
fn port_in_use() {
    let a = Bridge::<u32>::start(0, || {}).unwrap();
    assert!(Bridge::<u32>::start(a.port(), || {}).is_err());
}
