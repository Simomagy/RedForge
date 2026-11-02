//! The bridge with the add-on's own `link.py` (blender/redforge_bridge), run
//! by Python outside Blender: its requests, its listener thread and its
//! errors, against a fake host. Skipped when there's no `python3`.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use rf_bridge::{Bridge, BridgeHost, ModelData, ModelSource, Quat, SelectedObject, Transform, Vec3};
use rf_formats::formats::{self, XmlContext};
use rf_formats::hash::HashIndex;
use serde_json::Value;

const DRAWABLE: &str = include_str!("../../rf-formats/tests/data/ydr/features.ydr.xml");

/// What Blender does with link.py, in order; prints LISTENING once its
/// listener is connected, then waits for the edit event.
const SCRIPT: &str = r#"
import json, sys, time
sys.path.insert(0, sys.argv[1])
import link

port = int(sys.argv[2])
out = {"hello": link.request(port, "hello")}
link.start(port)
for _ in range(1000):
    if link.connected():
        break
    time.sleep(0.01)
out["connected"] = link.connected()
print("LISTENING", flush=True)
out["event"] = link.events.get(timeout=20)
selection = link.request(port, "selection")
out["selection"] = selection
p = selection["placements"][0]
out["transform"] = link.request(port, "transform", id=p["id"], position=[v + 1.5 for v in p["position"]],
                                rotation=p["rotation"], scale=p["scale"])
asset = selection["assets"][0]
out["import"] = link.request(port, "import", xml=asset["xml"], file=asset["file"])
try:
    link.request(port, "transform", id="0badcafe:1", position=[0, 0, 0], rotation=[1, 0, 0, 0], scale=[1, 1, 1])
except link.LinkError as ex:
    out["error"] = str(ex)
link.stop()
print(json.dumps(out), flush=True)
"#;

struct Host {
    hashes: Arc<HashIndex>,
    drawable: Vec<u8>,
    imported: Vec<String>,
    moved: Vec<Transform>,
}

fn placed() -> Transform {
    Transform { position: Vec3::new(-321.5, 812.25, 118.0), rotation: Quat::from_rotation_z(0.5), scale: Vec3::ONE }
}

impl BridgeHost for Host {
    type Object = u32;

    fn project_name(&self) -> Option<String> {
        Some("Add-on test".into())
    }

    fn hashes(&self) -> Option<Arc<HashIndex>> {
        Some(self.hashes.clone())
    }

    fn selection(&mut self) -> Vec<SelectedObject<u32>> {
        let data = self.drawable.clone();
        let load = Box::new(move || Ok(ModelData { data, texture_dictionaries: Vec::new() }));
        let model = ModelSource { file: "features.ydr".into(), piece: "features".into(), load };
        vec![SelectedObject { object: 7, name: "features".into(), transform: placed(), model }]
    }

    fn import(&mut self, file: &str, _data: Vec<u8>) -> Result<(), String> {
        self.imported.push(file.into());
        Ok(())
    }

    fn transform(&mut self, _object: &u32, transform: Transform) -> Result<(), String> {
        self.moved.push(transform);
        Ok(())
    }
}

#[test]
fn the_add_ons_link_works_with_it() {
    if Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let hashes = Arc::new(HashIndex::new());
    let drawable =
        formats::from_xml("features.ydr.xml", DRAWABLE, &XmlContext { hashes: &hashes, folder: None }).unwrap();
    let mut host = Host { hashes, drawable, imported: Vec::new(), moved: Vec::new() };
    let mut bridge = Bridge::<u32>::start(0, || {}).unwrap();
    let dir = std::env::temp_dir().join(format!("rf-bridge-add-on-{}", std::process::id()));
    bridge.set_export_folder(dir.clone());

    let add_on = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../blender/redforge_bridge");
    let mut python = Command::new("python3")
        .args(["-I", "-B"]) // (no __pycache__ in the add-on)
        .arg("-c")
        .arg(SCRIPT)
        .arg(&add_on)
        .arg(bridge.port().to_string())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (lines_tx, lines) = channel();
    let stdout = python.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = lines_tx.send(line);
        }
    });

    // Be the app: poll, and send "Edit in Blender" once its listener is there.
    let start = Instant::now();
    let mut out = None;
    while out.is_none() {
        assert!(start.elapsed() < Duration::from_secs(60), "link.py hangs");
        bridge.poll(&mut host);
        match lines.recv_timeout(Duration::from_millis(1)) {
            Ok(line) if line == "LISTENING" => {
                assert_eq!(bridge.listeners(), 1);
                assert!(bridge.edit_selection());
            }
            Ok(line) => out = Some(serde_json::from_str::<Value>(&line).unwrap()),
            Err(_) => {}
        }
    }
    assert!(python.wait().unwrap().success());
    let out = out.unwrap();

    assert_eq!(out["hello"]["project"], "Add-on test");
    assert_eq!(out["connected"], true);
    assert_eq!(out["event"]["event"], "edit");
    let selection = &out["selection"];
    assert_eq!(selection["placements"][0]["id"], format!("{}:1", bridge.session()));
    assert_eq!(selection["assets"][0]["file"], "features.ydr");
    assert_eq!(out["transform"]["ok"], true);
    assert_eq!(out["import"]["file"], "features.ydr");
    assert_eq!(out["error"], rf_bridge::messages::RESTARTED);
    assert_eq!(host.imported, ["features.ydr"]);
    assert_eq!(host.moved.len(), 1);
    assert!((host.moved[0].position - placed().position - Vec3::splat(1.5)).length() < 1e-3);
    assert!(host.moved[0].rotation.dot(placed().rotation).abs() > 0.999_999);
    // link.stop() closed its listener.
    let start = Instant::now();
    while bridge.listeners() > 0 {
        assert!(start.elapsed() < Duration::from_secs(10), "the listener stays");
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(bridge);
    let _ = std::fs::remove_dir_all(&dir);
}
