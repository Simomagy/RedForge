/// A 24-byte RPF8 TOC entry. RPF8 has no directories and no stored names: the
/// path is known only by its hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rpf8Entry {
    pub name_hash: u32,
    pub enc_config: u8,
    /// 0xFF when the file isn't encrypted.
    pub key_id: u8,
    pub ext_id: u8,
    pub flags: u8,
    pub data1: u64,
    pub virtual_flags: u32,
    pub physical_flags: u32,
}

pub const ENTRY_SIZE: usize = 24;

impl Rpf8Entry {
    pub fn parse(b: &[u8; ENTRY_SIZE]) -> Self {
        Self {
            name_hash: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            enc_config: b[4],
            key_id: b[5],
            ext_id: b[6],
            flags: b[7],
            data1: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            virtual_flags: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            physical_flags: u32::from_le_bytes(b[20..24].try_into().unwrap()),
        }
    }

    pub fn to_bytes(&self) -> [u8; ENTRY_SIZE] {
        let mut b = [0u8; ENTRY_SIZE];
        b[0..4].copy_from_slice(&self.name_hash.to_le_bytes());
        b[4] = self.enc_config;
        b[5] = self.key_id;
        b[6] = self.ext_id;
        b[7] = self.flags;
        b[8..16].copy_from_slice(&self.data1.to_le_bytes());
        b[16..20].copy_from_slice(&self.virtual_flags.to_le_bytes());
        b[20..24].copy_from_slice(&self.physical_flags.to_le_bytes());
        b
    }

    pub fn is_resource(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn is_signature_protected(&self) -> bool {
        self.flags & 2 != 0
    }
    pub fn is_encrypted(&self) -> bool {
        self.key_id != 0xFF
    }
    /// 0 = stored, 1 = deflate, 2 = Oodle Kraken.
    pub fn compressor(&self) -> u8 {
        ((self.data1 >> 59) & 0x1F) as u8
    }
    /// The stored size; 0 when it has to come from the flags.
    pub fn size(&self) -> u64 {
        (self.data1 & 0xFFF_FFFF) << 4
    }
    /// Offset from the start of the archive.
    pub fn offset(&self) -> u64 {
        ((self.data1 >> 28) & 0x7FFF_FFFF) << 4
    }
    pub fn virtual_size(&self) -> u32 {
        self.virtual_flags & 0xFFFF_FFF0
    }
    pub fn physical_size(&self) -> u32 {
        self.physical_flags & 0xFFFF_FFF0
    }
    /// The resource version, from the low nibbles of the flags.
    pub fn version(&self) -> u32 {
        ((self.virtual_flags & 0xF) << 4) + (self.physical_flags & 0xF)
    }
    /// The size once decompressed: the two segments of a resource, or for a binary
    /// file the two flag words, which then hold sizes.
    pub fn decompressed_size(&self) -> u64 {
        if self.is_resource() {
            self.virtual_size() as u64 + self.physical_size() as u64
        } else {
            self.virtual_flags as u64 + self.physical_flags as u64
        }
    }
    /// The number of bytes the file takes in the archive.
    pub fn file_size(&self) -> u64 {
        match self.size() {
            0 => self.virtual_size() as u64 + self.physical_size() as u64,
            s => s,
        }
    }

    pub fn is_archive_like(&self) -> bool {
        self.enc_config == 0 && self.ext_id == 0 && self.key_id == 0xFF && self.compressor() == 0 && !self.is_resource()
    }

    /// The extension the entry's ext id stands for, used when the name is unknown.
    pub fn extension(&self) -> &'static str {
        if self.is_archive_like() {
            return "rpf";
        }
        ext_name(self.ext_id).filter(|_| self.ext_id != 0).unwrap_or("bin")
    }
}

/// The extension of an `Rpf8FileExt` id.
pub fn ext_name(id: u8) -> Option<&'static str> {
    Some(match id {
        0 => "rpf",
        1 => "ymf",
        2 => "ydr",
        3 => "yft",
        4 => "ydd",
        5 => "ytd",
        6 => "ybn",
        7 => "ybd",
        8 => "ypd",
        9 => "ybs",
        10 => "ysd",
        11 => "ymt",
        12 => "ysc",
        13 => "ycs",
        64 => "mrf",
        65 => "cut",
        66 => "gfx",
        67 => "ycd",
        68 => "yld",
        69 => "ypmd",
        70 => "ypm",
        71 => "yed",
        72 => "ypt",
        73 => "ymap",
        74 => "ytyp",
        75 => "ych",
        76 => "yldb",
        77 => "yjd",
        78 => "yad",
        79 => "ynv",
        80 => "yhn",
        81 => "ypl",
        82 => "ynd",
        83 => "yvr",
        84 => "ywr",
        85 => "ynh",
        86 => "yfd",
        87 => "yas",
        _ => return None,
    })
}

/// The 16-byte `RSC8` header a resource needs outside an archive. Inside RPFs
/// those bytes are garbage that the game replaces at runtime.
pub fn rsc8_header(version: u32, virtual_flags: u32, physical_flags: u32, compressor: u8) -> [u8; 16] {
    let mut h = [0u8; 16];
    h[0..4].copy_from_slice(b"RSC8");
    h[4] = version as u8;
    h[5] = compressor.wrapping_sub(1) & 0x1F;
    h[8..12].copy_from_slice(&virtual_flags.to_le_bytes());
    h[12..16].copy_from_slice(&physical_flags.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_fields() {
        let mut b = [0u8; ENTRY_SIZE];
        b[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        b[5] = 0xFF;
        b[6] = 73;
        b[7] = 1;
        // size 0x30 (3 << 4), offset 0x200 (0x20 << 4), compressor 2
        let data1: u64 = 3 | (0x20 << 28) | (2 << 59);
        b[8..16].copy_from_slice(&data1.to_le_bytes());
        b[16..20].copy_from_slice(&0x2003u32.to_le_bytes());
        b[20..24].copy_from_slice(&0x1001u32.to_le_bytes());
        let e = Rpf8Entry::parse(&b);
        assert_eq!(e.to_bytes(), b);
        assert_eq!(e.size(), 0x30);
        assert_eq!(e.offset(), 0x200);
        assert_eq!(e.compressor(), 2);
        assert!(e.is_resource() && !e.is_encrypted());
        assert_eq!(e.version(), 0x31);
        assert_eq!(e.decompressed_size(), 0x3000);
        assert_eq!(e.extension(), "ymap");
    }
}
