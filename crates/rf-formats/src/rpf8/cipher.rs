//! The RPF8 cipher: a table-driven 128-bit block cipher (a white-box AES) run in
//! CBC mode, and the "strided" scheme that only ciphers parts of a file.
//!
//! Only decryption exists; RedForge never writes encrypted data.

use std::sync::Arc;

pub const BLOCK_SIZE: usize = 16;

/// One of the 17 middle rounds.
pub struct Round {
    pub lookup: Box<[u64; 4096]>,
    pub masks: [[u64; 16]; 16],
    pub xor: [u32; 16],
}

/// The tables shared by every key.
pub struct CipherTables {
    pub init: Box<[[u64; 256]; 16]>,
    pub rounds: Vec<Round>,
    pub end_masks: [[u64; 8]; 16],
    pub end_tables: Box<[[u8; 256]; 16]>,
    pub end_xor: [u8; 16],
}

/// A key: 18 pairs of round values (the first pair is unused by decryption).
pub type CipherKey = [[u64; 2]; 18];

/// A cipher for one key, with its running CBC IV.
pub struct Cipher {
    key: CipherKey,
    iv: [u8; 16],
    tables: Arc<CipherTables>,
}

#[inline(always)]
fn splat(data: &[u8; 16]) -> [u64; 16] {
    const M: u64 = 0x0101_0101_0101_0101;
    let mut t = [0u64; 16];
    for j in 0..8 {
        t[j] = M * data[7 - j] as u64;
        t[8 + j] = M * data[15 - j] as u64;
    }
    t
}

impl Cipher {
    pub fn new(key: CipherKey, iv: [u8; 16], tables: Arc<CipherTables>) -> Self {
        Self { key, iv, tables }
    }

    /// Decrypts the whole blocks of `data` in place; a partial block at the end is
    /// left as it is. The IV carries over to the next call.
    pub fn update(&mut self, data: &mut [u8]) {
        let mut iv = self.iv;
        for block in data.as_chunks_mut::<BLOCK_SIZE>().0 {
            let next_iv = *block;
            self.decrypt_block(block, &iv);
            iv = next_iv;
        }
        self.iv = iv;
    }

    fn decrypt_block(&self, data: &mut [u8; 16], iv: &[u8; 16]) {
        let tables = &*self.tables;

        // Round A
        let init = &*tables.init;
        let mut r0 = 0u64;
        let mut r1 = 0u64;
        for j in 0..8 {
            r0 ^= init[j][data[j] as usize];
            r1 ^= init[8 + j][data[8 + j] as usize];
        }
        data[0..8].copy_from_slice(&r0.to_le_bytes());
        data[8..16].copy_from_slice(&r1.to_le_bytes());

        // The 17 lookup rounds
        for (i, round) in tables.rounds.iter().enumerate() {
            let sel: u32 = if !(2..16).contains(&i) { 0xFF00 } else { 0x39C6 };
            let kp = &self.key[i + 1];
            let t = splat(data);
            for (o, &k) in kp.iter().enumerate() {
                let off = o * 8;
                let s = sel >> off;
                let mut r = k;
                for i in 0..8 {
                    let io = i + off;
                    let input = if (s >> i) & 1 == 1 { &t[8..16] } else { &t[0..8] };
                    let masks = &round.masks[io];
                    let mut lower = 0u64;
                    let mut upper = 0u64;
                    for c in 0..8 {
                        lower ^= masks[8 + c] & input[c];
                        upper ^= masks[c] & input[c];
                    }

                    lower ^= lower >> 1;
                    lower ^= lower >> 2;
                    lower ^= lower >> 4;
                    lower &= 0x0101_0101_0101_0101;
                    lower |= lower >> 7;
                    lower |= lower >> 14;
                    lower |= lower >> 28;

                    upper ^= upper << 1;
                    upper ^= upper >> 2;
                    upper ^= upper >> 4;
                    upper &= 0x0202_0202_0202_0202;
                    upper |= upper << 7;
                    upper |= upper >> 14;

                    let idx = (lower & 0xFF) ^ (upper & 0xF00) ^ round.xor[io] as u64;
                    r ^= round.lookup[idx as usize];
                }
                data[off..off + 8].copy_from_slice(&r.to_le_bytes());
            }
        }

        // Round D
        let t = splat(data);
        for i in 0..16 {
            let input = if i < 8 { &t[0..8] } else { &t[8..16] };
            let masks = &tables.end_masks[i];
            let mut v = 0u64;
            for c in 0..8 {
                v ^= masks[c] & input[c];
            }
            v ^= v >> 1;
            v ^= v >> 2;
            v ^= v >> 4;
            v &= 0x0101_0101_0101_0101;
            v |= v >> 7;
            v |= v >> 14;
            v |= v >> 28;
            data[i] = tables.end_tables[i][((v as u8) ^ tables.end_xor[i]) as usize] ^ iv[i];
        }
    }
}

/// `rage::AES::UnpackConfig`: which parts of a file are ciphered.
/// Returns (head length, block length, block stride).
fn unpack_config(config: u8) -> (i64, i64, i64) {
    let mut head = 0;
    let mut block_len = 0;
    let mut block_stride = 0;
    let head_config = config & 0b11;
    if head_config != 0 {
        head = 0x400i64 << (head_config * 2);
    }
    let length_config = (config >> 2) & 0b111;
    if length_config != 0 {
        let stride_config = (config >> 5) & 0b111;
        block_len = 0x400i64 << length_config;
        block_stride = (stride_config as i64 + 1) << 16;
    }
    (head, block_len, block_stride)
}

/// Deciphers the parts of a file that RAGE ciphers: a head, blocks at a fixed
/// stride, and the last 0x400 bytes. This mirrors the game's quirks, including
/// one where the ranges depend on the chunks the file is streamed in.
///
/// `header_len` is how many bytes before `data` count toward the streaming
/// chunks: the game streams a resource's entry from its 16-byte header, so its
/// first chunk boundary falls at `chunk_size - 16` in the data.
pub fn decrypt_strided(cipher: &mut Cipher, config: u8, data: &mut [u8], chunk_size: i64, header_len: i64) {
    let file_size = data.len() as i64;
    let tail_offset = file_size - 0x400;
    let (head_length, block_length, block_stride) = unpack_config(config);

    struct State<'a> {
        cipher: &'a mut Cipher,
        data: &'a mut [u8],
        /// How far into the file the cipher has advanced.
        pos: i64,
        remaining: i64,
        /// The end of the last ciphered range.
        offset: i64,
        file_size: i64,
    }

    impl State<'_> {
        fn update(&mut self, offs: i64, cipher: bool) -> bool {
            let n = (offs - self.pos).min(self.remaining);
            debug_assert!(n >= 0);
            if n > 0 {
                if cipher {
                    let start = self.pos as usize;
                    self.cipher.update(&mut self.data[start..start + n as usize]);
                }
                self.pos += n;
                self.remaining -= n;
            }
            self.pos == offs
        }

        fn update_range(&mut self, start: i64, end: i64) -> bool {
            let start = start.max(self.offset);
            let end = end.min(self.file_size);
            if start < end {
                if self.pos < end {
                    if self.pos < start && !self.update(start, false) {
                        return false;
                    }
                    if !self.update(end, true) {
                        return false;
                    }
                    self.offset = end;
                }
                return true;
            }
            false
        }
    }

    let remaining = data.len() as i64;
    let mut st = State { cipher, data, pos: 0, remaining, offset: 0, file_size };

    st.update_range(0, head_length);

    if head_length < tail_offset {
        if block_length != 0 || block_stride != 0 {
            let boundary = block_stride - header_len;
            if block_stride == chunk_size && tail_offset < boundary && boundary < file_size {
                // rage::AES::Cipher has a bug: when the tail straddles the first
                // chunk boundary, the part of it before the boundary isn't ciphered.
                // (Checked against every file of the game near the boundary.)
                st.offset = boundary;
            } else {
                let start_block = if st.offset != 0 { block_stride } else { 0 };
                let end_block = tail_offset - block_length;
                let mut block = start_block;
                while block <= end_block {
                    if !st.update_range(block, block + block_length) {
                        return;
                    }
                    block += block_stride;
                }
            }
        }
        st.update_range(tail_offset, file_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unusual_byte_groupings)] // grouped by config field
    fn config_unpacking() {
        assert_eq!(unpack_config(0), (0, 0, 0));
        assert_eq!(unpack_config(0b01), (0x1000, 0, 0));
        assert_eq!(unpack_config(0b11), (0x10000, 0, 0));
        // length 1 (0x800), stride 0 (0x10000)
        assert_eq!(unpack_config(0b000_001_00), (0, 0x800, 0x10000));
        // length 7 (0x20000), stride 7 (0x80000)
        assert_eq!(unpack_config(0b111_111_00), (0, 0x20000, 0x80000));
    }
}
