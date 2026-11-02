//! The RPF8 keys. They aren't shipped as such: `magic.dat` holds them obfuscated
//! and AES-encrypted, and the AES key is found inside the user's own `RDR2.exe`.

use std::path::Path;
use std::sync::Arc;

use aes::Aes256;
use aes::cipher::{BlockCipherDecrypt, KeyInit};
use rayon::prelude::*;
use sha1::{Digest, Sha1};

use super::Rpf8Error;
use super::cipher::{Cipher, CipherKey, CipherTables, Round};
use crate::dotnet::DotNetRandom;
use crate::hash::joaat_bytes;

/// SHA-1 of the 32-byte AES key stored in `RDR2.exe`.
const PC_AES_KEY_HASH: [u8; 20] = [
    0xA0, 0x79, 0x61, 0x28, 0xA7, 0x75, 0x72, 0x0A, 0xC2, 0x04, 0xD9, 0x81, 0x9F, 0x68, 0xC1, 0x72, 0xE3, 0x95, 0x2C,
    0x6D,
];

static MAGIC: &[u8] = include_bytes!("../../data/magic.dat");

const KEY_COUNT: usize = 166;
const KEYS_LEN: usize = KEY_COUNT * 18 * 2 * 8 + 16;
const TABLES_LEN: usize = 630_864;

/// Everything needed to decrypt RPF8 archives.
pub struct Rpf8Keys {
    pub aes_key: [u8; 32],
    keys: Vec<CipherKey>,
    iv: [u8; 16],
    tables: Arc<CipherTables>,
}

/// Scans an executable for the 32-byte AES key, by SHA-1, at 8-byte alignment.
pub fn find_aes_key(exe: &[u8]) -> Option<[u8; 32]> {
    const BLOCK: usize = 1 << 20;
    const ALIGN: usize = 8;
    // Like the original search, only whole 1 MiB blocks are scanned.
    let blocks = exe.len() / BLOCK;
    (0..blocks).into_par_iter().find_map_any(|k| {
        let base = k * BLOCK;
        (0..BLOCK / ALIGN).find_map(|i| {
            let pos = base + i * ALIGN;
            let window = exe.get(pos..pos + 32)?;
            (Sha1::digest(window).as_slice() == PC_AES_KEY_HASH).then(|| window.try_into().unwrap())
        })
    })
}

fn aes_decrypt_ecb(data: &mut [u8], key: &[u8; 32]) {
    let aes = Aes256::new_from_slice(key).expect("32-byte key");
    for block in data.as_chunks_mut::<16>().0 {
        aes.decrypt_block(block.into());
    }
}

/// A little-endian reader over the inflated key blob.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u64(&mut self) -> u64 {
        let v = u64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let v = self.data[self.pos..self.pos + N].try_into().unwrap();
        self.pos += N;
        v
    }
}

impl Rpf8Keys {
    /// Finds the AES key in `<game>/RDR2.exe` and decodes the keys with it.
    pub fn from_game_folder(folder: &Path) -> Result<Self, Rpf8Error> {
        let exe = super::find_case_insensitive(folder, "rdr2.exe")
            .ok_or_else(|| Rpf8Error::Keys(format!("RDR2.exe not found in {}", folder.display())))?;
        let data = std::fs::read(&exe)?;
        let aes_key =
            find_aes_key(&data).ok_or_else(|| Rpf8Error::Keys(format!("AES key not found in {}", exe.display())))?;
        Self::from_aes_key(aes_key)
    }

    /// Decodes `magic.dat` with a known AES key.
    pub fn from_aes_key(aes_key: [u8; 32]) -> Result<Self, Rpf8Error> {
        // Undo the byte streams that were added to it.
        let mut rnd = DotNetRandom::new(joaat_bytes(&aes_key) as i32);
        let len = MAGIC.len();
        let mut streams = vec![0u8; len * 4];
        for s in streams.chunks_exact_mut(len) {
            rnd.next_bytes(s);
        }
        let mut data: Vec<u8> = (0..len)
            .map(|i| {
                MAGIC[i]
                    .wrapping_sub(streams[i])
                    .wrapping_sub(streams[len + i])
                    .wrapping_sub(streams[2 * len + i])
                    .wrapping_sub(streams[3 * len + i])
            })
            .collect();
        drop(streams);

        aes_decrypt_ecb(&mut data, &aes_key);

        let mut blob = Vec::with_capacity(KEYS_LEN + TABLES_LEN);
        let mut inflater = flate2::read::DeflateDecoder::new(&data[..]);
        std::io::Read::read_to_end(&mut inflater, &mut blob)
            .map_err(|e| Rpf8Error::Keys(format!("magic.dat doesn't inflate (wrong AES key?): {e}")))?;
        if blob.len() < KEYS_LEN + TABLES_LEN {
            return Err(Rpf8Error::Keys(format!(
                "magic.dat inflated to {} bytes, expected {}",
                blob.len(),
                KEYS_LEN + TABLES_LEN
            )));
        }

        let mut r = Reader { data: &blob, pos: 0 };
        let keys = (0..KEY_COUNT)
            .map(|_| {
                let mut k: CipherKey = [[0; 2]; 18];
                for pair in &mut k {
                    pair[0] = r.u64();
                    pair[1] = r.u64();
                }
                k
            })
            .collect();
        let iv = r.bytes::<16>();

        let mut init = Box::new([[0u64; 256]; 16]);
        for t in init.iter_mut() {
            for v in t.iter_mut() {
                *v = r.u64();
            }
        }
        let rounds = (0..17)
            .map(|_| {
                let mut lookup = Box::new([0u64; 4096]);
                for v in lookup.iter_mut() {
                    *v = r.u64();
                }
                let mut masks = [[0u64; 16]; 16];
                let mut xor = [0u32; 16];
                for i in 0..16 {
                    for v in masks[i].iter_mut() {
                        *v = r.u64();
                    }
                    xor[i] = r.u32();
                    if xor[i] > 0xFFF {
                        return Err(Rpf8Error::Keys("invalid xor value in the cipher tables".into()));
                    }
                }
                Ok(Round { lookup, masks, xor })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut end_masks = [[0u64; 8]; 16];
        for m in end_masks.iter_mut() {
            for v in m.iter_mut() {
                *v = r.u64();
            }
        }
        let mut end_tables = Box::new([[0u8; 256]; 16]);
        for t in end_tables.iter_mut() {
            *t = r.bytes::<256>();
        }
        let end_xor = r.bytes::<16>();

        let tables = Arc::new(CipherTables { init, rounds, end_masks, end_tables, end_xor });
        Ok(Self { aes_key, keys, iv, tables })
    }

    /// The cipher for an archive's TOC (its decryption tag) or a file (its key id),
    /// or `None` when it isn't encrypted.
    pub fn cipher(&self, platform_id: u16, tag: u16) -> Result<Option<Cipher>, Rpf8Error> {
        if tag == 0xFF {
            return Ok(None);
        }
        if platform_id != b'y' as u16 {
            return Err(Rpf8Error::Format(format!("unknown platform id 0x{platform_id:X}")));
        }
        let index = match tag {
            0xC0 => 163,
            0xC5 => 164,
            0xC6 => 165,
            t => t as usize,
        };
        let key = self.keys.get(index).ok_or_else(|| Rpf8Error::Format(format!("unknown key 0x{tag:X}")))?;
        Ok(Some(Cipher::new(*key, self.iv, self.tables.clone())))
    }
}
