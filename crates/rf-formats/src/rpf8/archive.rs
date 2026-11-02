use std::fs::File;

use super::cipher;
use super::compression;
use super::entry::{ENTRY_SIZE, Rpf8Entry, rsc8_header};
use super::{Rpf8Error, Rpf8Keys, read_exact_at};
use crate::hash::HashIndex;

const MAGIC_RPF8: u32 = 0x5250_4638;
const HEADER_SIZE: usize = 16 + 0x100;

#[derive(Clone, Debug)]
pub struct Rpf8Header {
    pub entry_count: u32,
    pub names_length: u32,
    pub decryption_tag: u16,
    pub platform_id: u16,
}

/// An archive: a file on disk, or an `.rpf` entry inside another archive.
pub struct Rpf8Archive {
    /// The file name, e.g. `levels_0.rpf`.
    pub name: String,
    /// Lowercase path from the game folder, `/`-separated:
    /// `levels_0.rpf` or `update_1.rpf/x64/dlcpacks/foo/dlc.rpf`.
    pub path: String,
    /// Index of the file on disk that holds it, in [`super::Rpf8FileSystem`].
    pub root: usize,
    /// Where the archive starts in that file.
    pub start: u64,
    pub size: u64,
    pub header: Rpf8Header,
    pub entries: Vec<Rpf8Entry>,
    /// Each entry's path inside the archive, from the known strings, or
    /// `0x<HASH>.<ext>` when the name isn't known.
    pub names: Vec<Box<str>>,
    /// The archive and entry this one is nested in.
    pub parent: Option<(usize, usize)>,
}

impl Rpf8Archive {
    /// Reads the header and TOC of an archive starting at `start` in `file`.
    #[allow(clippy::too_many_arguments)]
    pub fn read(
        file: &File,
        root: usize,
        start: u64,
        size: u64,
        name: String,
        path: String,
        keys: &Rpf8Keys,
        strings: &HashIndex,
    ) -> Result<Self, Rpf8Error> {
        let mut hb = [0u8; HEADER_SIZE];
        read_exact_at(file, &mut hb, start)?;
        let magic = u32::from_le_bytes(hb[0..4].try_into().unwrap());
        if magic != MAGIC_RPF8 {
            return Err(Rpf8Error::Format(format!("found magic 0x{magic:08X} instead of RPF8")));
        }
        let header = Rpf8Header {
            entry_count: u32::from_le_bytes(hb[4..8].try_into().unwrap()),
            names_length: u32::from_le_bytes(hb[8..12].try_into().unwrap()),
            decryption_tag: u16::from_le_bytes(hb[12..14].try_into().unwrap()),
            platform_id: u16::from_le_bytes(hb[14..16].try_into().unwrap()),
        };
        let toc_len = header.entry_count as usize * ENTRY_SIZE;
        if toc_len as u64 > size {
            return Err(Rpf8Error::Format(format!("{} entries don't fit in {size} bytes", header.entry_count)));
        }
        let mut toc = vec![0u8; toc_len];
        read_exact_at(file, &mut toc, start + HEADER_SIZE as u64)?;
        if let Some(mut c) = keys.cipher(header.platform_id, header.decryption_tag)? {
            c.update(&mut toc);
        }

        let entries: Vec<Rpf8Entry> = toc.as_chunks::<ENTRY_SIZE>().0.iter().map(Rpf8Entry::parse).collect();
        let names = entries.iter().map(|e| entry_name(e, strings).into_boxed_str()).collect();

        Ok(Self { name, path, root, start, size, header, entries, names, parent: None })
    }

    /// The full path of an entry from the game folder.
    pub fn entry_path(&self, index: usize) -> String {
        format!("{}/{}", self.path, self.names[index])
    }

    /// Reads an entry's bytes as stored (still encrypted and compressed). For a
    /// resource, the 16 garbage header bytes are skipped.
    pub fn read_stored(&self, file: &File, index: usize) -> Result<Vec<u8>, Rpf8Error> {
        let e = &self.entries[index];
        let (pos, len) = if e.is_resource() {
            if e.size() < 0x10 {
                return Ok(Vec::new());
            }
            (e.offset() + 0x10, e.size() - 0x10)
        } else {
            (e.offset(), e.file_size())
        };
        if pos + len > self.size {
            return Err(Rpf8Error::Format(format!(
                "entry at 0x{pos:X}+0x{len:X} is outside the archive (0x{:X} bytes)",
                self.size
            )));
        }
        let mut buf = vec![0u8; len as usize];
        read_exact_at(file, &mut buf, self.start + pos)?;
        Ok(buf)
    }

    /// Deciphers an entry's stored bytes in place (signature included, removed).
    pub fn decrypt(&self, index: usize, data: &mut Vec<u8>, keys: &Rpf8Keys) -> Result<(), Rpf8Error> {
        let e = &self.entries[index];
        let Some(mut c) = keys.cipher(self.header.platform_id, e.key_id as u16)? else {
            return Ok(());
        };
        let compressed = e.compressor() != 0;
        // The size of the chunks the game streams the file in changes what's ciphered.
        let chunk_size: i64 = if e.is_resource() {
            if compressed { 0x80000 } else { e.decompressed_size() as i64 }
        } else if compressed {
            0x2000
        } else {
            0x1000
        };
        if e.is_signature_protected() && data.len() > 0x100 {
            data.truncate(data.len() - 0x100);
        }
        let header_len = if e.is_resource() { 0x10 } else { 0 };
        cipher::decrypt_strided(&mut c, e.enc_config, data, chunk_size, header_len);
        Ok(())
    }

    /// Decompresses deciphered bytes.
    pub fn decompress(&self, index: usize, data: Vec<u8>) -> Result<Vec<u8>, Rpf8Error> {
        let e = &self.entries[index];
        let size = e.decompressed_size() as usize;
        match e.compressor() {
            0 => Ok(data),
            1 => compression::inflate(&data, size),
            2 => compression::oodle_decompress(&data, size),
            c => Err(Rpf8Error::Decompress(format!("unknown compressor {c}"))),
        }
    }

    /// An entry's data, decrypted and decompressed. For a resource that's the
    /// virtual and physical segments, without an `RSC8` header.
    pub fn extract(&self, file: &File, index: usize, keys: &Rpf8Keys) -> Result<Vec<u8>, Rpf8Error> {
        let mut data = self.read_stored(file, index)?;
        self.decrypt(index, &mut data, keys)?;
        self.decompress(index, data)
    }

    /// An entry as a loose file: resources get their `RSC8` header and stay
    /// compressed, as the game loads them from a `stream` folder; other files are
    /// decompressed.
    pub fn extract_loose(&self, file: &File, index: usize, keys: &Rpf8Keys) -> Result<Vec<u8>, Rpf8Error> {
        let e = &self.entries[index];
        if !e.is_resource() {
            return self.extract(file, index, keys);
        }
        let mut data = self.read_stored(file, index)?;
        self.decrypt(index, &mut data, keys)?;
        let header = rsc8_header(e.version(), e.virtual_flags, e.physical_flags, e.compressor());
        let mut out = Vec::with_capacity(16 + data.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&data);
        Ok(out)
    }
}

/// An entry's path inside its archive, `/`-separated.
fn entry_name(e: &Rpf8Entry, strings: &HashIndex) -> String {
    strings.with(e.name_hash, |name| match name {
        Some(name) => {
            let name = name.replace('\\', "/");
            if name.rsplit('/').next().is_some_and(|n| n.contains('.')) {
                name
            } else {
                format!("{name}.{}", e.extension())
            }
        }
        None => format!("0x{:X}.{}", e.name_hash, e.extension()),
    })
}
