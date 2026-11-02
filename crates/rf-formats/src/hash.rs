//! Jenkins one-at-a-time hashing ("joaat") and the hash → string index.
//!
//! RAGE identifies almost everything by the joaat hash of a lowercase name. The
//! game files don't contain the names, so they come from a dictionary of known
//! strings (`strings.txt`).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::RwLock;

/// Jenkins one-at-a-time hash of raw bytes. No case folding: callers lowercase.
pub const fn joaat_bytes(data: &[u8]) -> u32 {
    let mut h: u32 = 0;
    let mut i = 0;
    while i < data.len() {
        h = h.wrapping_add(data[i] as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
        i += 1;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h = h.wrapping_add(h << 15);
    h
}

/// Jenkins one-at-a-time hash of a string.
///
/// Like RedForge's C# `JenkHash.GenHash`, each char is truncated to a byte, so
/// non-ASCII text hashes the same way it did there.
pub fn joaat(text: &str) -> u32 {
    let mut h: u32 = 0;
    for c in text.chars() {
        h = h.wrapping_add((c as u32) & 0xFF);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h = h.wrapping_add(h << 15);
    h
}

/// A joaat hash. `Display` shows the known name, or `0x` and 8 hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct JenkHash(pub u32);

impl JenkHash {
    pub fn of(text: &str) -> Self {
        Self(joaat(text))
    }
}

impl fmt::Debug for JenkHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JenkHash(0x{:08X})", self.0)
    }
}

impl From<u32> for JenkHash {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

/// The known strings, by hash. Shared between threads; names learnt while
/// reading files (like RedForge's `JenkIndex.Ensure`) are added on the fly.
#[derive(Default)]
pub struct HashIndex {
    strings: RwLock<HashMap<u32, Box<str>>>,
}

impl HashIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads a `strings.txt` dictionary: one string per line; `//` starts a comment
    /// line, and anything after `=` on a line is ignored.
    pub fn load_file(&self, path: &Path) -> io::Result<usize> {
        let text = std::fs::read_to_string(path)?;
        Ok(self.load_str(&text))
    }

    pub fn load_str(&self, text: &str) -> usize {
        let mut map = self.strings.write().unwrap();
        let before = map.len();
        map.reserve(text.len() / 24);
        for line in text.lines() {
            let s = line.split('=').next().unwrap_or("").trim();
            if s.is_empty() || s.starts_with("//") {
                continue;
            }
            map.insert(joaat(s), s.into());
        }
        map.len() - before
    }

    /// Adds one string, keeping an existing one with the same hash.
    pub fn ensure(&self, s: &str) -> u32 {
        let h = joaat(s);
        if h != 0 && !self.strings.read().unwrap().contains_key(&h) {
            self.strings.write().unwrap().entry(h).or_insert_with(|| s.into());
        }
        h
    }

    pub fn get(&self, hash: u32) -> Option<String> {
        self.strings.read().unwrap().get(&hash).map(|s| s.to_string())
    }

    /// Calls `f` with the name of a hash, without copying it.
    pub fn with<R>(&self, hash: u32, f: impl FnOnce(Option<&str>) -> R) -> R {
        f(self.strings.read().unwrap().get(&hash).map(|s| &**s))
    }

    pub fn len(&self) -> usize {
        self.strings.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The name of a hash, or `0x` and its 8 hex digits.
    pub fn name(&self, hash: u32) -> String {
        self.get(hash).unwrap_or_else(|| format!("0x{hash:08X}"))
    }

    /// How a hash is written in XML: its name, or for an unknown hash a made-up
    /// name that hashes to the same value (`AbCdEfG_0x12345678`), so it survives
    /// a round trip through tools that rehash names. Empty for hash 0.
    pub fn xml_name(&self, hash: u32) -> String {
        if hash == 0 {
            return String::new();
        }
        self.get(hash).unwrap_or_else(|| collision_name(hash, false))
    }
}

/// A name of 7 letters, `_0x` and the 8 hex digits that hashes to `hash`, as
/// RedForge's `JenkIndex.GenerateCollision` makes it (`lowercase`: 8 lowercase
/// letters and lowercase hex).
pub fn collision_name(hash: u32, lowercase: bool) -> String {
    let mut suffix = format!("_0x{hash:08X}");
    if lowercase {
        suffix = suffix.to_lowercase();
    }

    // Step the hash back to just after the last letter, then back over the suffix.
    let mut h = hash;
    h = h.wrapping_mul(0x3FFF_8001); // inverse of h += h << 15
    h ^= (h >> 11) ^ (h >> 22);
    h = h.wrapping_mul(0x38E3_8E39); // inverse of h += h << 3
    for &c in suffix.as_bytes().iter().rev() {
        h ^= (h >> 6) ^ (h >> 12) ^ (h >> 18) ^ (h >> 24) ^ (h >> 30);
        h = h.wrapping_mul(0xC00F_FC01); // inverse of h += h << 10
        h = h.wrapping_sub(c as u32);
    }

    let (mut buf, depth, chars): (Vec<u8>, usize, &[u8]) = if lowercase {
        (vec![b'a'; 8], 7, b"abcdefghijklmnopqrstuvwxyz")
    } else {
        (vec![b'A'; 7], 6, b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz")
    };
    if find_preimage(h, &mut buf, depth, chars, lowercase) {
        let name = format!("{}{suffix}", std::str::from_utf8(&buf).unwrap());
        if joaat(&name) == hash {
            return name;
        }
    }
    format!("HASH_ERROR_0x{hash:08X}")
}

/// Finds letters for `buf[0..=depth]` that the hash function steps to `hash`.
fn find_preimage(mut hash: u32, buf: &mut [u8], depth: usize, chars: &[u8], lowercase: bool) -> bool {
    hash ^= (hash >> 6) ^ (hash >> 12) ^ (hash >> 18) ^ (hash >> 24) ^ (hash >> 30);
    hash = hash.wrapping_mul(0xC00F_FC01); // inverse of hash += hash << 10

    const Z: u32 = b'Z' as u32;
    const A_LOWER: u32 = b'a' as u32;
    const Z_LOWER: u32 = b'z' as u32;
    let lowest = if lowercase { A_LOWER } else { b'A' as u32 };
    // At the lowest levels, give up early when no solution is possible.
    match depth {
        0 => {
            if hash < lowest || hash > Z_LOWER || (!lowercase && hash > Z && hash < A_LOWER) {
                return false;
            }
            buf[0] = hash as u8;
            return true;
        }
        1 if hash > Z_LOWER * 1043 => return false,
        2 if hash > Z_LOWER * 1_084_746 => return false,
        _ => {}
    }
    for &ch in chars {
        if find_preimage(hash.wrapping_sub(ch as u32), buf, depth - 1, chars, lowercase) {
            buf[depth] = ch;
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joaat_known_values() {
        assert_eq!(joaat(""), 0);
        // The classic test vector for Jenkins one-at-a-time.
        assert_eq!(joaat("a"), 0xCA2E9442);
        assert_eq!(joaat("The quick brown fox jumps over the lazy dog"), 0x519E91F5);
        assert_eq!(joaat_bytes(b"a"), joaat("a"));
    }

    #[test]
    fn index_parses_lines() {
        let idx = HashIndex::new();
        let n = idx.load_str("//== header\nfoo\n bar = ignored\n\n//comment\n");
        assert_eq!(n, 2);
        assert_eq!(idx.get(joaat("foo")).as_deref(), Some("foo"));
        assert_eq!(idx.get(joaat("bar")).as_deref(), Some("bar"));
        assert_eq!(idx.name(0x1234), "0x00001234");
    }

    #[test]
    fn collision_names_hash_back() {
        // Seen in RedForge's XML for an unknown shader texture name.
        assert_eq!(collision_name(0xE9F32960, false), "oIZmQFA_0xE9F32960");
        for h in [1u32, 0xDEADBEEF, 0x12345678, 0xFFFFFFFF] {
            let n = collision_name(h, false);
            assert_eq!(joaat(&n), h, "{n}");
            assert_eq!(joaat(&collision_name(h, true)), h);
        }
    }
}
