//! One RedForge at a time for projects: a project opened from the file
//! manager (`redforge file.rfproj`) goes to the RedForge already running, if
//! there is one, instead of starting another.
//!
//! The running one listens on a local TCP port (the Blender add-on's is the
//! next one down), for a line `RedForge open <path>`, and answers `ok`.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const PORT: u16 = 47811;
const HELLO: &str = "RedForge open ";

fn address() -> SocketAddr {
    (Ipv4Addr::LOCALHOST, PORT).into()
}

/// The project the command line names, as an absolute path (the running
/// RedForge may be somewhere else).
pub fn project_argument() -> Option<PathBuf> {
    let arg = std::env::args_os().skip(1).find(|a| !a.to_string_lossy().starts_with('-'))?;
    let file = PathBuf::from(arg);
    if !file.extension().is_some_and(|e| e.eq_ignore_ascii_case("rfproj")) {
        log::warn!("{} isn't a RedForge project (.rfproj)", file.display());
        return None;
    }
    Some(std::path::absolute(&file).unwrap_or(file))
}

/// Gives the project to a RedForge already running; false when there's none
/// (or it didn't answer), and this one opens it.
pub fn hand_over(file: &Path) -> bool {
    let send = || -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&address(), Duration::from_millis(300))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        writeln!(stream, "{HELLO}{}", file.display())?;
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer)?;
        Ok(answer.trim() == "ok")
    };
    match send() {
        Ok(true) => {
            log::info!("{} opened in the RedForge already running", file.display());
            true
        }
        _ => false,
    }
}

/// Listens for the projects later RedForges hand over, calling `wake` for
/// each (to repaint). None when another RedForge has the port already.
pub fn listen(wake: impl Fn() + Send + 'static) -> Option<Receiver<PathBuf>> {
    let listener = match TcpListener::bind(address()) {
        Ok(l) => l,
        Err(e) => {
            log::debug!("not taking projects from other RedForges: {e}");
            return None;
        }
    };
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("projects from other RedForges".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                let Ok(mut reader) = stream.try_clone().map(BufReader::new) else { continue };
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let Some(path) = line.trim_end_matches(['\r', '\n']).strip_prefix(HELLO) else { continue };
                if tx.send(PathBuf::from(path)).is_err() {
                    return;
                }
                wake();
                let _ = (&stream).write_all(b"ok\n");
            }
        })
        .ok()?;
    Some(rx)
}
