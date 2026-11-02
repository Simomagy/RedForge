//! The messages, as the add-on's `link.py` sends and reads them: one JSON
//! object per line. Replies keep the C#'s key order (`ok` last), though the
//! add-on doesn't depend on it.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{Quat, Transform, Vec3};

/// What every listening Blender gets for "Edit in Blender": it asks for the
/// selection then.
pub const EDIT_EVENT: &str = r#"{"event":"edit"}"#;

/// A command from the add-on.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `{"cmd": "hello"}`: which app and project (the add-on doesn't use it).
    Hello,
    /// `{"cmd": "listen"}`: events go to this connection from now on.
    Listen,
    /// `{"cmd": "selection"}`: the selected objects' models and placements.
    Selection,
    /// `{"cmd": "import", "xml": "<folder>/p_x.ydr/p_x.ydr.xml", "file":
    /// "p_x.ydr"}`: an edited model exported by Sollumz, its textures in the
    /// folder `p_x` next to the XML. Missing fields are empty, as the C# reads
    /// them.
    Import { xml: String, file: String },
    /// `{"cmd": "transform", "id": "<session>:<n>", "position": [x, y, z],
    /// "rotation": [w, x, y, z], "scale": [x, y, z]}`: an object moved.
    Transform { id: String, transform: Transform },
    /// Any other `cmd`, as text (empty when there's none).
    Unknown(String),
}

impl Command {
    /// Reads one line (without its line end).
    pub fn parse(line: &str) -> Result<Command, String> {
        let value: Value = serde_json::from_str(line).map_err(|e| format!("Bad request: {e}"))?;
        let Value::Object(map) = value else { return Err("Bad request: not a JSON object".into()) };
        let text = |key: &str| map.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        let cmd = match map.get("cmd") {
            Some(Value::String(cmd)) => cmd.as_str(),
            None | Some(Value::Null) => "",
            Some(other) => return Ok(Command::Unknown(other.to_string())),
        };
        Ok(match cmd {
            "hello" => Command::Hello,
            "listen" => Command::Listen,
            "selection" => Command::Selection,
            "import" => Command::Import { xml: text("xml"), file: text("file") },
            "transform" => {
                let [px, py, pz] = numbers(&map, "position")?;
                let [w, x, y, z] = numbers(&map, "rotation")?;
                let [sx, sy, sz] = numbers(&map, "scale")?;
                let transform = Transform {
                    position: Vec3::new(px, py, pz),
                    rotation: Quat::from_xyzw(x, y, z, w),
                    scale: Vec3::new(sx, sy, sz),
                };
                Command::Transform { id: text("id"), transform }
            }
            other => Command::Unknown(other.to_owned()),
        })
    }
}

/// The first `N` numbers of an array field (the C# reads them as floats and
/// ignores more).
fn numbers<const N: usize>(map: &Map<String, Value>, key: &str) -> Result<[f32; N], String> {
    let bad = || format!("Bad request: {key} should be {N} numbers");
    let items = map.get(key).and_then(Value::as_array).filter(|a| a.len() >= N).ok_or_else(bad)?;
    let mut out = [0.0; N];
    for (o, v) in out.iter_mut().zip(items) {
        *o = v.as_f64().ok_or_else(bad)? as f32;
    }
    Ok(out)
}

/// A model exported for Blender.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Asset {
    /// The game file's name (`p_lantern05x.ydr`).
    pub file: String,
    /// The full path of its XML.
    pub xml: String,
}

/// A selected object for Blender.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Placement {
    /// The bridge's id for it, for `transform`.
    pub id: String,
    /// The archetype's name.
    pub name: String,
    /// Its model's file (an [`Asset`]'s `file`).
    pub file: String,
    /// Which model of the file it uses (see [`crate::ModelSource::piece`]).
    pub piece: String,
    pub position: [f32; 3],
    /// `[w, x, y, z]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Placement {
    pub fn new(id: String, name: String, file: String, piece: String, t: &Transform) -> Self {
        let q = t.rotation;
        Self {
            id,
            name,
            file,
            piece,
            position: t.position.to_array(),
            rotation: [q.w, q.x, q.y, q.z],
            scale: t.scale.to_array(),
        }
    }
}

/// A reply to a command.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Hello {
        project: Option<String>,
    },
    /// Done, nothing to say (`listen`, `transform`).
    Done,
    Selection {
        assets: Vec<Asset>,
        placements: Vec<Placement>,
        /// The models that couldn't be exported, `"<file>: <why>"`.
        errors: Vec<String>,
    },
    /// The file an `import` made.
    Imported {
        file: String,
    },
    Error(String),
}

impl Reply {
    /// The reply's line, without its line end.
    pub fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct Hello<'a> {
            app: &'a str,
            project: Option<&'a str>,
            ok: bool,
        }
        #[derive(Serialize)]
        struct Done {
            ok: bool,
        }
        #[derive(Serialize)]
        struct Selection<'a> {
            assets: &'a [Asset],
            placements: &'a [Placement],
            errors: &'a [String],
            ok: bool,
        }
        #[derive(Serialize)]
        struct Imported<'a> {
            file: &'a str,
            ok: bool,
        }
        #[derive(Serialize)]
        struct Error<'a> {
            ok: bool,
            error: &'a str,
        }
        let json = match self {
            Reply::Hello { project } => {
                serde_json::to_string(&Hello { app: "RedForge", project: project.as_deref(), ok: true })
            }
            Reply::Done => serde_json::to_string(&Done { ok: true }),
            Reply::Selection { assets, placements, errors } => {
                serde_json::to_string(&Selection { assets, placements, errors, ok: true })
            }
            Reply::Imported { file } => serde_json::to_string(&Imported { file, ok: true }),
            Reply::Error(error) => serde_json::to_string(&Error { ok: false, error }),
        };
        // (only plain structs of strings and numbers: serialising can't fail)
        json.expect("a reply serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The lines as link.py writes them: json.dumps({"cmd": cmd, **args}), and
    // the listener's own bytes.
    #[test]
    fn reads_the_add_ons_commands() {
        assert_eq!(Command::parse(r#"{"cmd": "listen"}"#), Ok(Command::Listen));
        assert_eq!(Command::parse(r#"{"cmd": "selection"}"#), Ok(Command::Selection));
        assert_eq!(Command::parse(r#"{"cmd": "hello"}"#), Ok(Command::Hello));
        let line = r#"{"cmd": "import", "xml": "/tmp/RedForge/Blender/send/p_lantern05x.ydr/p_lantern05x.ydr.xml", "file": "p_lantern05x.ydr"}"#;
        assert_eq!(
            Command::parse(line),
            Ok(Command::Import {
                xml: "/tmp/RedForge/Blender/send/p_lantern05x.ydr/p_lantern05x.ydr.xml".into(),
                file: "p_lantern05x.ydr".into()
            })
        );
        // A ytyp from Send YTYPs (Windows path, escaped).
        let line = r#"{"cmd": "import", "xml": "C:\\Users\\me\\AppData\\Local\\Temp\\RedForge\\Blender\\send\\props.ytyp\\props.ytyp.rsc.xml", "file": "props.ytyp"}"#;
        let Ok(Command::Import { xml, file }) = Command::parse(line) else { panic!() };
        assert!(xml.starts_with(r"C:\Users\") && xml.ends_with(r"\props.ytyp.rsc.xml"));
        assert_eq!(file, "props.ytyp");

        // Python's floats are doubles; the position is the working origin plus
        // the location, the rotation Blender's [w, x, y, z].
        let line = r#"{"cmd": "transform", "id": "3f9a01bc:2", "position": [-321.5000000001, 812.25, 118.0], "rotation": [0.7071067811865476, 0.0, 0.0, 0.7071067811865475], "scale": [1.0, 1.0, 2.5]}"#;
        let Ok(Command::Transform { id, transform }) = Command::parse(line) else { panic!() };
        assert_eq!(id, "3f9a01bc:2");
        assert_eq!(transform.position, Vec3::new(-321.5, 812.25, 118.0));
        assert_eq!(transform.rotation, Quat::from_xyzw(0.0, 0.0, 0.70710677, 0.70710677));
        assert_eq!(transform.scale, Vec3::new(1.0, 1.0, 2.5));
        // Integers are numbers too.
        let line =
            r#"{"cmd": "transform", "id": "x", "position": [1, 2, 3], "rotation": [1, 0, 0, 0], "scale": [1, 1, 1]}"#;
        assert!(matches!(Command::parse(line), Ok(Command::Transform { .. })));
    }

    #[test]
    fn odd_requests() {
        assert_eq!(Command::parse(r#"{"cmd": "dance"}"#), Ok(Command::Unknown("dance".into())));
        assert_eq!(Command::parse(r#"{"what": 1}"#), Ok(Command::Unknown(String::new())));
        assert_eq!(Command::parse(r#"{"cmd": 5}"#), Ok(Command::Unknown("5".into())));
        // Missing fields read as empty, as in the C#: the server says what's wrong.
        assert_eq!(Command::parse(r#"{"cmd": "import"}"#), Ok(Command::Import { xml: "".into(), file: "".into() }));
        assert!(Command::parse("not json").is_err());
        assert!(Command::parse("").is_err());
        assert!(Command::parse("[1, 2]").is_err());
        let no_scale = r#"{"cmd": "transform", "id": "x", "position": [1, 2, 3], "rotation": [1, 0, 0, 0]}"#;
        assert!(Command::parse(no_scale).unwrap_err().contains("scale"));
        let short =
            r#"{"cmd": "transform", "id": "x", "position": [1, 2], "rotation": [1, 0, 0, 0], "scale": [1, 1, 1]}"#;
        assert!(Command::parse(short).unwrap_err().contains("position"));
        let text =
            r#"{"cmd": "transform", "id": "x", "position": [1, 2, "3"], "rotation": [1, 0, 0, 0], "scale": [1, 1, 1]}"#;
        assert!(Command::parse(text).is_err());
    }

    #[test]
    fn writes_the_replies_the_add_on_reads() {
        assert_eq!(
            Reply::Hello { project: Some("Valentine fixes".into()) }.to_json(),
            r#"{"app":"RedForge","project":"Valentine fixes","ok":true}"#
        );
        assert_eq!(Reply::Hello { project: None }.to_json(), r#"{"app":"RedForge","project":null,"ok":true}"#);
        assert_eq!(Reply::Done.to_json(), r#"{"ok":true}"#);
        assert_eq!(
            Reply::Imported { file: "p_lantern05x.ydr".into() }.to_json(),
            r#"{"file":"p_lantern05x.ydr","ok":true}"#
        );
        assert_eq!(
            Reply::Error("Not found: C:\\x \"y\"".into()).to_json(),
            r#"{"ok":false,"error":"Not found: C:\\x \"y\""}"#
        );
        assert_eq!(EDIT_EVENT, r#"{"event":"edit"}"#);

        let t = Transform {
            position: Vec3::new(-321.5, 812.25, 118.0),
            rotation: Quat::from_xyzw(0.0, 0.0, 0.6, 0.8),
            scale: Vec3::new(1.0, 1.0, 2.5),
        };
        let reply = Reply::Selection {
            assets: vec![Asset {
                file: "p_lantern05x.ydr".into(),
                xml: "/e/p_lantern05x.ydr/p_lantern05x.ydr.xml".into(),
            }],
            placements: vec![Placement::new(
                "3f9a01bc:1".into(),
                "p_lantern05x".into(),
                "p_lantern05x.ydr".into(),
                "p_lantern05x".into(),
                &t,
            )],
            errors: vec!["p_other.ydd: no such file".into()],
        };
        assert_eq!(
            reply.to_json(),
            concat!(
                r#"{"assets":[{"file":"p_lantern05x.ydr","xml":"/e/p_lantern05x.ydr/p_lantern05x.ydr.xml"}],"#,
                r#""placements":[{"id":"3f9a01bc:1","name":"p_lantern05x","file":"p_lantern05x.ydr","piece":"p_lantern05x","#,
                r#""position":[-321.5,812.25,118.0],"rotation":[0.8,0.0,0.0,0.6],"scale":[1.0,1.0,2.5]}],"#,
                r#""errors":["p_other.ydd: no such file"],"ok":true}"#
            )
        );
        // What edit_selection reads from it.
        let v: Value = serde_json::from_str(&reply.to_json()).unwrap();
        assert_eq!(v["ok"], true);
        let p = &v["placements"][0];
        for key in ["id", "name", "file", "piece", "position", "rotation", "scale"] {
            assert!(p.get(key).is_some(), "{key}");
        }
        assert_eq!(v["assets"][0]["file"], p["file"]);
    }
}
