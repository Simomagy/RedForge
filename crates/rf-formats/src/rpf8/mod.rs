//! RPF8, the archive format of RDR2.
//!
//! Read-only: RedM loads loose files from a resource's `stream` folder, so there's
//! no need to write archives.

mod archive;
pub mod cipher;
pub mod compression;
mod entry;
mod filesystem;
pub mod keys;

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

pub use archive::{Rpf8Archive, Rpf8Header};
pub use entry::{ENTRY_SIZE, Rpf8Entry, ext_name, rsc8_header};
pub use filesystem::{FileRef, LOOSE, LoadError, Rpf8FileSystem};
pub use keys::Rpf8Keys;

#[derive(Debug, thiserror::Error)]
pub enum Rpf8Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("keys: {0}")]
    Keys(String),
    #[error("invalid RPF8: {0}")]
    Format(String),
    #[error("decompression failed: {0}")]
    Decompress(String),
}

/// Reads exactly `buf.len()` bytes at `offset`, without moving a shared cursor, so
/// several threads can read the same file.
pub(crate) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < buf.len() {
            let n = std::os::windows::fs::FileExt::seek_read(file, &mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
}

/// Finds a file in a folder ignoring case (the game's own files are named for
/// Windows, where case doesn't matter).
pub fn find_case_insensitive(folder: &Path, name: &str) -> Option<PathBuf> {
    let exact = folder.join(name);
    if exact.is_file() {
        return Some(exact);
    }
    std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}
