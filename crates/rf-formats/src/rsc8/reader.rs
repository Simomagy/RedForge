use bytemuck::Pod;

use crate::{Result, bail};

pub const VIRTUAL_BASE: u64 = 0x5000_0000;
pub const PHYSICAL_BASE: u64 = 0x6000_0000;

/// Reads structs out of a resource's segments. Positions are pointers as stored
/// in the file: `0x5…` into the virtual segment, `0x6…` into the physical one.
pub struct Rsc8Reader<'a> {
    data: &'a [u8],
    virtual_size: usize,
    /// The current position, as a pointer.
    pub pos: u64,
}

macro_rules! read_le {
    ($($name:ident: $t:ty),*) => {$(
        pub fn $name(&mut self) -> Result<$t> {
            Ok(<$t>::from_le_bytes(self.take::<{ size_of::<$t>() }>()?))
        }
    )*};
}

impl<'a> Rsc8Reader<'a> {
    /// `data` is the virtual segment followed by the physical one.
    pub fn new(data: &'a [u8], virtual_size: usize) -> Self {
        Self { data, virtual_size, pos: VIRTUAL_BASE }
    }

    /// The offset in the data a pointer refers to.
    pub fn offset_of(&self, ptr: u64) -> Result<usize> {
        if ptr & VIRTUAL_BASE == VIRTUAL_BASE {
            Ok((ptr & 0x0FFF_FFFF) as usize)
        } else if ptr & PHYSICAL_BASE == PHYSICAL_BASE {
            Ok((ptr & 0x1FFF_FFFF) as usize + self.virtual_size)
        } else {
            bail!("invalid pointer 0x{ptr:X}, the file may be corrupt")
        }
    }

    pub fn is_physical(ptr: u64) -> bool {
        ptr & PHYSICAL_BASE == PHYSICAL_BASE
    }

    fn slice(&self, ptr: u64, len: usize) -> Result<&'a [u8]> {
        let off = self.offset_of(ptr)?;
        match self.data.get(off..off + len) {
            Some(s) => Ok(s),
            None => bail!("read of {len} bytes at 0x{ptr:X} is past the end of the data"),
        }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.slice(self.pos, N)?;
        self.pos += N as u64;
        Ok(s.try_into().unwrap())
    }

    read_le!(u8: u8, i8: i8, u16: u16, i16: i16, u32: u32, i32: i32, u64: u64, i64: i64, f32: f32);

    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let s = self.slice(self.pos, len)?;
        self.pos += len as u64;
        Ok(s)
    }

    pub fn skip(&mut self, len: u64) {
        self.pos += len;
    }

    pub fn vec2(&mut self) -> Result<[f32; 2]> {
        Ok([self.f32()?, self.f32()?])
    }
    pub fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    pub fn vec4(&mut self) -> Result<[f32; 4]> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }
    pub fn floats<const N: usize>(&mut self) -> Result<[f32; N]> {
        let mut v = [0.0; N];
        for f in &mut v {
            *f = self.f32()?;
        }
        Ok(v)
    }
    /// A plain-data value of any `Pod` type.
    pub fn pod<T: Pod>(&mut self) -> Result<T> {
        let s = self.bytes(size_of::<T>())?;
        Ok(bytemuck::pod_read_unaligned(s))
    }

    /// Runs `f` with the position at `ptr`, then comes back.
    pub fn at<T>(&mut self, ptr: u64, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let saved = self.pos;
        self.pos = ptr;
        let r = f(self);
        self.pos = saved;
        r
    }

    /// Follows a pointer if it isn't null.
    pub fn follow<T>(&mut self, ptr: u64, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if ptr == 0 { Ok(None) } else { self.at(ptr, f).map(Some) }
    }

    /// Reads a pointer and follows it.
    pub fn ptr<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        let p = self.u64()?;
        self.follow(p, f)
    }

    /// `count` plain-data items at `ptr` (`None` for a null pointer).
    pub fn array_at<T: Pod>(&self, ptr: u64, count: usize) -> Result<Option<Vec<T>>> {
        if ptr == 0 {
            return Ok(None);
        }
        let s = self.slice(ptr, count * size_of::<T>())?;
        Ok(Some(bytemuck::pod_collect_to_vec(s)))
    }

    /// `count` items read with `f`, one after another from `ptr`.
    pub fn items_at<T>(
        &mut self,
        ptr: u64,
        count: usize,
        mut f: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Option<Vec<T>>> {
        self.follow(ptr, |r| (0..count).map(|_| f(r)).collect())
    }

    /// A NUL-terminated string at `ptr`.
    pub fn string_at(&self, ptr: u64) -> Result<Option<String>> {
        if ptr == 0 {
            return Ok(None);
        }
        let off = self.offset_of(ptr)?;
        let Some(rest) = self.data.get(off..) else { bail!("string at 0x{ptr:X} is past the end of the data") };
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Ok(Some(String::from_utf8_lossy(&rest[..end]).into_owned()))
    }

    /// A string of `len` bytes at `ptr` (stops early at a NUL).
    pub fn fixed_string_at(&self, ptr: u64, len: usize) -> Result<Option<String>> {
        if ptr == 0 {
            return Ok(None);
        }
        let s = self.slice(ptr, len)?;
        let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        Ok(Some(String::from_utf8_lossy(&s[..end]).into_owned()))
    }

    /// A NUL-terminated string at the current position (advancing past it).
    pub fn string_here(&mut self) -> Result<String> {
        let s = self.string_at(self.pos)?.unwrap_or_default();
        self.pos += s.len() as u64 + 1;
        Ok(s)
    }
}

/// `rage::atArray`-style header: pointer, u16 count, u16 capacity, u32 padding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrayHeader {
    pub ptr: u64,
    pub count: u16,
    pub capacity: u16,
    pub unknown: u32,
}

impl ArrayHeader {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self { ptr: r.u64()?, count: r.u16()?, capacity: r.u16()?, unknown: r.u32()? })
    }
}

/// The 32-bit count variant: pointer, u32 count, u32 capacity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrayHeader32 {
    pub ptr: u64,
    pub count: u32,
    pub capacity: u32,
}

impl ArrayHeader32 {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self { ptr: r.u64()?, count: r.u32()?, capacity: r.u32()? })
    }
}
