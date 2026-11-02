//! Jenkins one-at-a-time hashing ("joaat") and the hash → string index.
//!
//! RAGE identifies almost everything by the joaat hash of a lowercase name. The
//! game files don't contain the names, so they come from a dictionary of known
//! strings (`strings.txt`).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;

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

/// The known strings, by hash.
#[derive(Default)]
pub struct HashIndex {
    strings: HashMap<u32, Box<str>>,
}

impl HashIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads a `strings.txt` dictionary: one string per line; `//` starts a comment
    /// line, and anything after `=` on a line is ignored.
    pub fn load_file(&mut self, path: &Path) -> io::Result<usize> {
        let text = std::fs::read_to_string(path)?;
        Ok(self.load_str(&text))
    }

    pub fn load_str(&mut self, text: &str) -> usize {
        let before = self.strings.len();
        self.strings.reserve(text.len() / 24);
        for line in text.lines() {
            let s = line.split('=').next().unwrap_or("").trim();
            if s.is_empty() || s.starts_with("//") {
                continue;
            }
            self.strings.insert(joaat(s), s.into());
        }
        self.strings.len() - before
    }

    /// Adds one string, keeping an existing one with the same hash.
    pub fn ensure(&mut self, s: &str) -> u32 {
        let h = joaat(s);
        if h != 0 {
            self.strings.entry(h).or_insert_with(|| s.into());
        }
        h
    }

    pub fn get(&self, hash: u32) -> Option<&str> {
        self.strings.get(&hash).map(|s| &**s)
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// The name of a hash, or `0x` and its 8 hex digits.
    pub fn name(&self, hash: u32) -> String {
        match self.get(hash) {
            Some(s) => s.to_owned(),
            None => format!("0x{hash:08X}"),
        }
    }
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
        let mut idx = HashIndex::new();
        let n = idx.load_str("//== header\nfoo\n bar = ignored\n\n//comment\n");
        assert_eq!(n, 2);
        assert_eq!(idx.get(joaat("foo")), Some("foo"));
        assert_eq!(idx.get(joaat("bar")), Some("bar"));
        assert_eq!(idx.name(0x1234), "0x00001234");
    }
}
