//! Shader groups, shaders and their parameters (`Rsc8ShaderGroup`,
//! `Rsc8Shader`, `Rsc8ShaderParams`, `Rsc8ShaderParam`).

use std::collections::HashMap;

use super::util::{ArrHeader, node_items, read_array, read_ptr_arr, write_ptr_arr};
use crate::formats::ytd::{TextureBase, TextureDictionary, dimension, tile_mode};
use crate::hash::{HashIndex, joaat};
use crate::rsc8::{BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8ShaderParamType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderParamType {
    Texture = 0,
    Unknown = 1,
    Sampler = 2,
    CBuffer = 3,
}

impl ShaderParamType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Texture => "Texture",
            Self::Unknown => "Unknown",
            Self::Sampler => "Sampler",
            Self::CBuffer => "CBuffer",
        }
    }

    /// `Enum.TryParse`, with `Texture` (0) when it fails.
    fn parse(s: Option<&str>) -> Self {
        match s.map(str::trim) {
            Some("Unknown" | "1") => Self::Unknown,
            Some("Sampler" | "2") => Self::Sampler,
            Some("CBuffer" | "3") => Self::CBuffer,
            _ => Self::Texture,
        }
    }
}

/// `Rsc8ShaderParamInfo` (8 bytes): a parameter's name and packed type,
/// index, offset and length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct ShaderParamInfo {
    pub name: u32,
    pub data: u32,
}

impl ShaderParamInfo {
    pub fn param_type(&self) -> ShaderParamType {
        match self.data & 3 {
            0 => ShaderParamType::Texture,
            1 => ShaderParamType::Unknown,
            2 => ShaderParamType::Sampler,
            _ => ShaderParamType::CBuffer,
        }
    }
    /// Like the C#, this also clears bit 2.
    pub fn set_param_type(&mut self, t: ShaderParamType) {
        self.data = (self.data & 0xFFFF_FFF8).wrapping_add(t as u32 & 3);
    }
    /// The texture index, or the sampler/unknown index.
    pub fn index(&self) -> u8 {
        ((self.data >> 2) & 0xFF) as u8
    }
    pub fn set_index(&mut self, v: u8) {
        self.data = (self.data & 0xFFFF_FC03).wrapping_add((v as u32) << 2);
    }
    pub fn cbuffer_index(&self) -> u8 {
        ((self.data >> 2) & 0x3F) as u8
    }
    pub fn set_cbuffer_index(&mut self, v: u8) {
        self.data = (self.data & 0xFFFF_FF03).wrapping_add((v as u32 & 0x3F) << 2);
    }
    pub fn param_offset(&self) -> u16 {
        ((self.data >> 8) & 0xFFF) as u16
    }
    pub fn set_param_offset(&mut self, v: u16) {
        self.data = (self.data & 0xFFF0_00FF).wrapping_add((v as u32 & 0xFFF) << 8);
    }
    pub fn param_length(&self) -> u16 {
        ((self.data >> 20) & 0xFFF) as u16
    }
    pub fn set_param_length(&mut self, v: u16) {
        self.data = (self.data & 0x000F_FFFF).wrapping_add((v as u32 & 0xFFF) << 20);
    }
}

/// `Rsc8ShaderParamData` (8 + 8 per parameter bytes): the counts of each kind
/// of parameter, and each parameter's info.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShaderParamData {
    pub num_buffers: u8,
    pub num_textures: u8,
    pub num_unknowns: u8,
    pub num_samplers: u8,
    pub num_params: u32,
    /// `None` when there are none.
    pub params: Option<Vec<ShaderParamInfo>>,
}

impl ShaderParamData {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut d = Self {
            num_buffers: r.u8()?,
            num_textures: r.u8()?,
            num_unknowns: r.u8()?,
            num_samplers: r.u8()?,
            num_params: r.u32()?,
            params: None,
        };
        d.params = read_array(r, r.pos, d.num_params as usize)?;
        Ok(d)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(8 + self.num_params as usize * 8);
        b.u8(self.num_buffers);
        b.u8(self.num_textures);
        b.u8(self.num_unknowns);
        b.u8(self.num_samplers);
        b.u32(self.num_params);
        if let Some(p) = &self.params {
            b.bytes(bytemuck::cast_slice(p));
        }
        b.pad_to(8 + self.num_params as usize * 8);
        w.add(b, false)
    }
}

/// What a texture parameter points to.
#[derive(Clone, Debug, PartialEq)]
pub enum ShaderTexture {
    /// A texture of the shader group's own dictionary, by index.
    Embedded(usize),
    /// A texture by name, found elsewhere by the game.
    External(TextureBase),
}

impl ShaderTexture {
    fn base<'a>(&'a self, txd: Option<&'a TextureDictionary>) -> Option<&'a TextureBase> {
        match self {
            Self::External(t) => Some(t),
            Self::Embedded(i) => Some(&txd?.textures.as_ref()?.get(*i)?.as_ref()?.base),
        }
    }
}

/// `Rsc8ShaderParam`: one parameter, with its value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderParam {
    pub info: ShaderParamInfo,
    pub texture: Option<ShaderTexture>,
    pub sampler: u8,
    pub unk_val: u64,
    /// A CBuffer value of up to 4 floats.
    pub vector: [f32; 4],
    /// A CBuffer value of more than 4 floats, in vec4s.
    pub array: Option<Vec<[f32; 4]>>,
}

impl ShaderParam {
    fn write_xml(&self, w: &mut XmlWriter, txd: Option<&TextureDictionary>) {
        let info = &self.info;
        w.string("@type", info.param_type().name());
        w.hash("@name", info.name);
        match info.param_type() {
            ShaderParamType::Texture => {
                w.u8("@index", info.index());
                if let Some(t) = &self.texture {
                    // A dangling embedded index can't come from a file or XML.
                    let base = t.base(txd);
                    w.string("@texture", base.and_then(|b| b.name.as_deref()).unwrap_or(""));
                    let flags = base.map_or(0, |b| b.flags);
                    if flags != 0x1000_0000 {
                        w.u32("@flags", flags);
                    }
                }
            }
            ShaderParamType::Unknown => w.u8("@index", info.index()),
            ShaderParamType::Sampler => {
                w.u8("@index", info.index());
                w.u8("@sampler", self.sampler);
            }
            ShaderParamType::CBuffer => {
                w.u8("@buffer", info.cbuffer_index());
                w.u16("@offset", info.param_offset());
                w.u16("@length", info.param_length());
                let n = (info.param_length() / 4) as usize;
                match n {
                    0 => {}
                    1..=4 => {
                        for (id, v) in ["@x", "@y", "@z", "@w"].iter().zip(&self.vector).take(n) {
                            w.f32(id, *v);
                        }
                    }
                    _ => w.vec4_array("Array", self.array.as_deref()),
                }
            }
        }
    }

    fn read_xml(node: XmlNode) -> Self {
        let mut p = Self::default();
        let mut info = ShaderParamInfo::default();
        info.set_param_type(ShaderParamType::parse(node.attr("type")));
        info.name = node.attr("name").map_or(0, joaat);
        match info.param_type() {
            ShaderParamType::Texture => {
                info.set_index(node.u8("@index", 0));
                if let Some(name) = node.attr("texture").filter(|n| !n.is_empty()) {
                    p.texture = Some(ShaderTexture::External(TextureBase {
                        name: Some(name.to_owned()),
                        flags: node.u32("@flags", 0x1000_0000),
                        dimension: dimension::TEXTURE_2D,
                        tile_mode: tile_mode::AUTO,
                        ..Default::default()
                    }));
                }
            }
            ShaderParamType::Unknown => info.set_index(node.u8("@index", 0)),
            ShaderParamType::Sampler => {
                info.set_index(node.u8("@index", 0));
                p.sampler = node.u8("@sampler", 0);
            }
            ShaderParamType::CBuffer => {
                info.set_cbuffer_index(node.u8("@buffer", 0));
                info.set_param_offset(node.u16("@offset", 0));
                info.set_param_length(node.u16("@length", 0));
                let n = (info.param_length() / 4) as usize;
                match n {
                    0 => {}
                    1..=4 => {
                        for (d, id) in p.vector.iter_mut().zip(["@x", "@y", "@z", "@w"]).take(n) {
                            *d = node.f32(id, 0.0);
                        }
                    }
                    _ => p.array = node.vec4_array("Array"),
                }
            }
        }
        p.info = info;
        p
    }
}

/// `Rsc8ShaderParams`: one block of variable length holding 4 copies of each
/// constant buffer (pointers to them, then their data), the texture pointers
/// (4 per texture), the unknowns and the sampler indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderParams {
    /// `None` when the shader has no parameter data.
    pub params: Option<Vec<ShaderParam>>,
    /// The counts and infos, the same as the shader's parameter data.
    pub data: ShaderParamData,
    pub data_size: i32,
    /// Each constant buffer's size in bytes.
    pub buffer_sizes: Option<Vec<u32>>,
    pub buffers_count: u32,
    /// The 4 copies of the buffers' data.
    pub buffers_data_size: u32,
    /// The texture pointers (4 per texture); `None` for a null pointer.
    pub textures: Option<Vec<Option<ShaderTexture>>>,
    pub unknowns: Option<Vec<u64>>,
    pub samplers: Option<Vec<u8>>,
    pub textures_offset: u64,
    pub unknowns_offset: u64,
}

/// The pointers to a shader group's own textures, to recognise them in
/// shader parameters.
type EmbeddedPtrs = HashMap<u64, usize>;

impl ShaderParams {
    /// Reads the block at the reader's position, as `Rsc8Shader` does with its
    /// parameter data's counts and the texture and unknown pointers.
    fn read(
        r: &mut Rsc8Reader,
        pdat: &ShaderParamData,
        textures_ptr: u64,
        unknowns_ptr: u64,
        data_size: i32,
        embedded: &EmbeddedPtrs,
        hashes: &HashIndex,
    ) -> Result<Self> {
        let mut p = Self { data: pdat.clone(), data_size, ..Default::default() };
        let infos = pdat.params.clone().unwrap_or_default();
        let bcnt = pdat.num_buffers as usize;
        let spos = r.pos;
        let buffer_ptrs = read_array::<u64>(r, spos, bcnt * 4)?;
        p.buffers_count = bcnt as u32;
        let mut sizes = vec![0u32; bcnt];
        if let Some(bp) = &buffer_ptrs {
            for (i, s) in sizes.iter_mut().enumerate() {
                *s = bp[i + 1].wrapping_sub(bp[i]) as u32;
            }
        }
        p.buffer_sizes = Some(sizes);
        if textures_ptr != 0 {
            p.textures_offset = textures_ptr.wrapping_sub(spos);
        }
        if unknowns_ptr != 0 {
            p.unknowns_offset = unknowns_ptr.wrapping_sub(spos);
        }
        let (mut p0, mut p1) = (0u64, 0u64);
        if let Some(bp) = buffer_ptrs.as_ref().filter(|bp| bp.len() > bcnt) {
            p0 = bp[0];
            p1 = bp[bcnt];
        }
        let ptrslen = bcnt * 8 * 4;
        let bufslen = (p1.wrapping_sub(p0) as i32).wrapping_mul(4);
        let texslen = pdat.num_textures as usize * 8 * 4;
        let unkslen = pdat.num_unknowns as usize * 8 * 4;
        p.buffers_data_size = bufslen as u32;

        if textures_ptr != 0 {
            let n = pdat.num_textures as usize * 4;
            let ptrs = read_array::<u64>(r, textures_ptr, n)?;
            let mut v = Vec::with_capacity(n);
            for i in 0..n {
                let Some(ptr) = ptrs.as_ref().map(|p| p[i]) else { break };
                v.push(match (ptr, embedded.get(&ptr)) {
                    (0, _) => None,
                    (_, Some(&e)) => Some(ShaderTexture::Embedded(e)),
                    _ => Some(ShaderTexture::External(r.at(ptr, |r| TextureBase::read(r, hashes))?)),
                });
            }
            p.textures = Some(v);
        }
        if unknowns_ptr != 0 {
            p.unknowns = read_array::<u64>(r, unknowns_ptr, pdat.num_unknowns as usize * 4)?;
        }
        if pdat.num_samplers > 0 {
            let at = spos.wrapping_add((ptrslen as i64 + bufslen as i64 + texslen as i64 + unkslen as i64) as u64);
            p.samplers = read_array::<u8>(r, at, pdat.num_samplers as usize)?;
        }

        let mut params = Vec::with_capacity(infos.len());
        for info in infos {
            let mut sp = ShaderParam { info, ..Default::default() };
            match info.param_type() {
                ShaderParamType::Texture => {
                    sp.texture = p.textures.as_ref().and_then(|t| t.get(info.index() as usize).cloned().flatten());
                }
                ShaderParamType::Unknown => {
                    sp.unk_val = p.unknowns.as_ref().and_then(|u| u.get(info.index() as usize).copied()).unwrap_or(0);
                }
                ShaderParamType::Sampler => {
                    sp.sampler = p.samplers.as_ref().and_then(|s| s.get(info.index() as usize).copied()).unwrap_or(0);
                }
                ShaderParamType::CBuffer => {
                    let cbi = info.cbuffer_index() as usize;
                    let base = buffer_ptrs.as_ref().and_then(|b| b.get(cbi).copied()).unwrap_or(0);
                    if base == 0 {
                        bail!("error reading shader parameter buffer (the C# fails on it)");
                    }
                    r.pos = base + info.param_offset() as u64;
                    let n = info.param_length() / 4;
                    match n {
                        0 => {}
                        1..=4 => {
                            for v in sp.vector.iter_mut().take(n as usize) {
                                *v = r.f32()?;
                            }
                        }
                        _ => {
                            let arrsiz = (info.param_length() / 16) as usize;
                            sp.array = read_array::<[f32; 4]>(r, r.pos, arrsiz)?;
                        }
                    }
                }
            }
            params.push(sp);
        }
        r.pos = spos;
        p.params = Some(params);
        Ok(p)
    }

    /// The block's length: the data size, at least 8.
    pub fn block_length(&self) -> usize {
        self.data_size.max(8) as usize
    }

    /// Adds the block; `embedded` are the blocks of the group's own textures.
    fn write(&self, w: &mut Rsc8Writer, embedded: &[Option<BlockId>]) -> Result<BlockId> {
        let id = w.reserve();
        let len = self.block_length();
        let sizes = self.buffer_sizes.as_deref().unwrap_or(&[]);
        let ptrslen = self.buffers_count as usize * 8 * 4;
        let bufslen = self.buffers_data_size as usize;
        let texslen = self.data.num_textures as usize * 8 * 4;
        let unkslen = self.data.num_unknowns as usize * 8 * 4;

        // Everything but the pointers, at its place in the block.
        let mut data = vec![0u8; len.max(ptrslen + bufslen + texslen + unkslen)];
        let mut put = |at: usize, bytes: &[u8]| -> Result<()> {
            if at + bytes.len() > len {
                bail!("a shader parameter is past the end of its block (the C# fails on it)");
            }
            data[at..at + bytes.len()].copy_from_slice(bytes);
            Ok(())
        };
        if let Some(params) = &self.params {
            let bufsiz = bufslen / 4;
            let mut bufoffs = Vec::with_capacity(sizes.len());
            let mut o = 0u32;
            for s in sizes {
                bufoffs.push(o);
                o = o.wrapping_add(*s);
            }
            for i in 0..4 {
                let basepos = ptrslen + i * bufsiz;
                for p in params {
                    if p.info.param_type() != ShaderParamType::CBuffer {
                        continue;
                    }
                    let Some(&cbo) = bufoffs.get(p.info.cbuffer_index() as usize) else { continue };
                    let at = basepos + cbo as usize + p.info.param_offset() as usize;
                    let n = p.info.param_length() / 4;
                    let floats: Vec<f32> = match n {
                        0 => Vec::new(),
                        1..=4 => p.vector[..n as usize].to_vec(),
                        _ => p.array.iter().flatten().flatten().copied().collect(),
                    };
                    let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_le_bytes()).collect();
                    put(at, &bytes)?;
                }
            }
        }
        let tex_at = ptrslen + bufslen;
        let unk_at = tex_at + texslen;
        if let Some(u) = &self.unknowns {
            let bytes: Vec<u8> = u.iter().flat_map(|v| v.to_le_bytes()).collect();
            put(unk_at, &bytes)?;
        }
        if let Some(s) = &self.samplers {
            put(unk_at + unkslen, s)?;
        }

        let mut b = BlockBuf::with_capacity(len);
        let mut offset = ptrslen as u64;
        for _ in 0..4 {
            for s in sizes {
                b.ptr_offset(Some(id), offset);
                offset = offset.wrapping_add(*s as u64);
            }
        }
        match &self.textures {
            Some(texs) if tex_at >= b.len() => {
                b.bytes(&data[b.len()..tex_at]);
                for t in texs.iter().take(texslen.min(len.saturating_sub(tex_at)) / 8) {
                    let target = match t {
                        None => None,
                        Some(ShaderTexture::External(t)) => Some(t.write(w)),
                        Some(ShaderTexture::Embedded(i)) => embedded.get(*i).copied().flatten(),
                    };
                    b.ptr(target);
                }
            }
            _ => {}
        }
        let at = b.len().min(len);
        b.bytes(&data[at..len]);
        w.set(id, b, false);
        Ok(id)
    }

    fn write_xml(&self, w: &mut XmlWriter, txd: Option<&TextureDictionary>) {
        if self.buffers_count > 0 {
            w.u32_array("BufferSizes", self.buffer_sizes.as_deref());
        }
        let params = self.params.as_deref();
        w.node_array("Items", params, |w, p| p.write_xml(w, txd));
    }

    fn read_xml(node: XmlNode) -> Result<Self> {
        let mut p = Self { buffer_sizes: node.u32_array("BufferSizes"), ..Default::default() };
        p.buffers_count = p.buffer_sizes.as_ref().map_or(0, |b| b.len() as u32);
        let bds = p.buffer_sizes.iter().flatten().fold(0u32, |a, b| a.wrapping_add(*b));
        p.buffers_data_size = bds.wrapping_mul(4);
        p.params = node_items(node, "Items", |n| Ok(ShaderParam::read_xml(n)))?;
        let params = p.params.as_deref().unwrap_or(&[]);
        let count = |t| params.iter().filter(|p| p.info.param_type() == t).count();
        let (texcnt, unkcnt, smpcnt) =
            (count(ShaderParamType::Texture), count(ShaderParamType::Unknown), count(ShaderParamType::Sampler));
        p.data = ShaderParamData {
            num_buffers: p.buffers_count as u8,
            num_textures: texcnt as u8,
            num_unknowns: unkcnt as u8,
            num_samplers: smpcnt as u8,
            num_params: params.len() as u32,
            params: Some(params.iter().map(|p| p.info).collect()),
        };
        let mut texarr = vec![None; texcnt * 4];
        let mut unkarr = vec![0u64; unkcnt * 4];
        let mut smparr = vec![0u8; smpcnt];
        for sp in params {
            let i = sp.info.index() as usize;
            match sp.info.param_type() {
                ShaderParamType::Texture if i < texcnt => texarr[i] = sp.texture.clone(),
                ShaderParamType::Unknown if i < unkcnt => unkarr[i] = sp.unk_val,
                ShaderParamType::Sampler if i < smpcnt => smparr[i] = sp.sampler,
                _ => {}
            }
        }
        p.textures = Some(texarr);
        p.unknowns = Some(unkarr);
        p.samplers = Some(smparr);
        let ptrslen = p.buffers_count as i32 * 8 * 4;
        let bufslen = p.buffers_data_size as i32;
        let texslen = p.data.num_textures as i32 * 8 * 4;
        let unkslen = p.data.num_unknowns as i32 * 8 * 4;
        p.data_size = ptrslen + bufslen + texslen + unkslen + p.data.num_samplers as i32;
        p.textures_offset = (ptrslen + bufslen) as u64;
        p.unknowns_offset = p.textures_offset + texslen as u64;
        Ok(p)
    }

    /// Points texture parameters whose name (lowercased) is one of `td`'s to
    /// that texture, as `Rsc8ShaderGroup.Read` does for imported textures.
    fn use_embedded(&mut self, td: &HashMap<u32, usize>) {
        let swap = |t: &mut Option<ShaderTexture>| {
            if let Some(ShaderTexture::External(b)) = t {
                let h = b.name.as_deref().map_or(0, |n| joaat(&n.to_lowercase()));
                if h != 0
                    && let Some(&i) = td.get(&h)
                {
                    *t = Some(ShaderTexture::Embedded(i));
                }
            }
        };
        for p in self.params.iter_mut().flatten() {
            swap(&mut p.texture);
        }
        for t in self.textures.iter_mut().flatten() {
            swap(t);
        }
    }
}

/// `Rsc8Shader` (64 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct Shader {
    pub name_hash: u32,
    /// `"meta"`.
    pub unk_04: u32,
    /// `None` when there's no parameter data to read them with.
    pub parameters: Option<ShaderParams>,
    /// The texture and unknown pointers as read; written from `parameters`.
    pub texture_refs: u64,
    pub unknowns_ptr: u64,
    pub parameter_data: Option<ShaderParamData>,
    pub pad_28: u64,
    pub pad_30: u64,
    pub unk_38: u8,
    /// The draw bucket, with 0x80 as a flag.
    pub draw_bucket: u8,
    pub parameter_data_size: u16,
    /// `1 << draw_bucket`.
    pub draw_bucket_mask: u8,
    pub unk_3d: u8,
    pub unk_3e: u16,
}

impl Default for Shader {
    fn default() -> Self {
        Self {
            name_hash: 0,
            unk_04: 0x6d65_7461,
            parameters: None,
            texture_refs: 0,
            unknowns_ptr: 0,
            parameter_data: None,
            pad_28: 0,
            pad_30: 0,
            unk_38: 0,
            draw_bucket: 0,
            parameter_data_size: 0,
            draw_bucket_mask: 0,
            unk_3d: 0xFF,
            unk_3e: 0,
        }
    }
}

impl Shader {
    fn read(r: &mut Rsc8Reader, embedded: &EmbeddedPtrs, hashes: &HashIndex) -> Result<Self> {
        let mut s = Self { name_hash: r.u32()?, unk_04: r.u32()?, ..Default::default() };
        let params_ptr = r.u64()?;
        s.texture_refs = r.u64()?;
        s.unknowns_ptr = r.u64()?;
        s.parameter_data = r.ptr(ShaderParamData::read)?;
        s.pad_28 = r.u64()?;
        s.pad_30 = r.u64()?;
        s.unk_38 = r.u8()?;
        s.draw_bucket = r.u8()?;
        s.parameter_data_size = r.u16()?;
        s.draw_bucket_mask = r.u8()?;
        s.unk_3d = r.u8()?;
        s.unk_3e = r.u16()?;
        if let Some(pdat) = &s.parameter_data {
            let (tp, up, size) = (s.texture_refs, s.unknowns_ptr, s.parameter_data_size as i32);
            s.parameters = r.follow(params_ptr, |r| ShaderParams::read(r, pdat, tp, up, size, embedded, hashes))?;
        }
        Ok(s)
    }

    fn write(&self, w: &mut Rsc8Writer, embedded: &[Option<BlockId>]) -> Result<BlockId> {
        let Some(p) = &self.parameters else {
            bail!("shader \"{:08X}\" is missing its parameters (the C# fails on it)", self.name_hash);
        };
        let pid = p.write(w, embedded)?;
        let mut b = BlockBuf::with_capacity(64);
        b.u32(self.name_hash);
        b.u32(self.unk_04);
        b.ptr(Some(pid));
        b.ptr_offset(p.textures.as_ref().map(|_| pid), p.textures_offset);
        b.ptr_offset(p.unknowns.as_ref().map(|_| pid), p.unknowns_offset);
        b.ptr(self.parameter_data.as_ref().map(|d| d.write(w)));
        b.u64(self.pad_28);
        b.u64(self.pad_30);
        b.u8(self.unk_38);
        b.u8(self.draw_bucket);
        b.u16(self.parameter_data_size);
        b.u8(self.draw_bucket_mask);
        b.u8(self.unk_3d);
        b.u16(self.unk_3e);
        Ok(w.add(b, false))
    }

    fn write_xml(&self, w: &mut XmlWriter, txd: Option<&TextureDictionary>) {
        w.hash("Name", self.name_hash);
        w.u8("DrawBucket", self.draw_bucket & 0x7F);
        if self.draw_bucket >= 0x80 {
            w.bool("DrawBucketFlag", true);
        }
        if self.unk_3e != 0 {
            w.u16("Unknown_3E", self.unk_3e);
        }
        if let Some(p) = &self.parameters {
            w.node("Parameters", |w| p.write_xml(w, txd));
        }
    }

    fn read_xml(node: XmlNode) -> Result<Self> {
        let mut s = Self { name_hash: node.string("Name").map_or(0, |n| joaat(&n)), ..Default::default() };
        s.draw_bucket = node.u8("DrawBucket", 0);
        if node.bool("DrawBucketFlag", false) {
            s.draw_bucket |= 0x80;
        }
        s.unk_3e = node.u16("Unknown_3E", 0);
        s.parameters = node.node("Parameters").map(ShaderParams::read_xml).transpose()?;
        s.parameter_data = s.parameters.as_ref().map(|p| p.data.clone());
        s.parameter_data_size = s.parameters.as_ref().map_or(0, |p| p.data_size as u16);
        s.draw_bucket_mask = 1u32.wrapping_shl(s.draw_bucket as u32) as u8;
        Ok(s)
    }
}

/// `Rsc8ShaderGroup` (64 bytes): the shaders, and the textures embedded in
/// the drawable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderGroup {
    pub vft: u64,
    pub texture_dictionary: Option<TextureDictionary>,
    /// `None` for a null pointer or no shaders.
    pub shaders: Option<Vec<Option<Shader>>>,
    pub shaders_capacity: u16,
    pub shaders_unknown: u32,
    pub unk20_array: u64,
    pub unk20_count: u16,
    pub unk20_size: u16,
    pub unk20_pad: u32,
    pub unk30: u64,
    pub unk38: u64,
}

impl ShaderGroup {
    pub const BLOCK_LENGTH: usize = 64;

    /// Reads the group at the reader's position. Texture names are added to
    /// `hashes`, like the C#'s `JenkIndex.Ensure`.
    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        let mut g = Self { vft: r.u64()?, ..Default::default() };
        let td_ptr = r.u64()?;
        g.texture_dictionary = r.follow(td_ptr, |r| TextureDictionary::read(r, hashes))?;
        // Where the dictionary's textures are, to recognise them in parameters.
        let mut embedded = EmbeddedPtrs::new();
        if g.texture_dictionary.is_some() {
            let h = r.at(td_ptr + 0x30, |r| ArrHeader::read(r))?;
            for (i, p) in read_array::<u64>(r, h.ptr, h.count as usize)?.unwrap_or_default().into_iter().enumerate() {
                if p != 0 {
                    embedded.entry(p).or_insert(i);
                }
            }
        }
        let sh = ArrHeader::read(r)?;
        g.shaders = read_ptr_arr(r, &sh, |r| Shader::read(r, &embedded, hashes))?;
        g.shaders_capacity = sh.capacity;
        g.shaders_unknown = sh.unknown;
        g.unk20_array = r.u64()?;
        g.unk20_count = r.u16()?;
        g.unk20_size = r.u16()?;
        g.unk20_pad = r.u32()?;
        g.unk30 = r.u64()?;
        g.unk38 = r.u64()?;
        Ok(g)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        let (td, embedded) = match &self.texture_dictionary {
            Some(td) => {
                let (id, texs) = td.write_with_textures(w);
                (Some(id), texs)
            }
            None => (None, Vec::new()),
        };
        b.ptr(td);
        let shaders = match &self.shaders {
            Some(v) => Some(
                v.iter().map(|s| s.as_ref().map(|s| s.write(w, &embedded)).transpose()).collect::<Result<Vec<_>>>()?,
            ),
            None => None,
        };
        write_ptr_arr(&mut b, w, shaders.as_deref(), self.shaders_capacity, self.shaders_unknown);
        b.u64(self.unk20_array);
        b.u16(self.unk20_count);
        b.u16(self.unk20_size);
        b.u32(self.unk20_pad);
        b.u64(self.unk30);
        b.u64(self.unk38);
        w.set(id, b, false);
        Ok(id)
    }

    /// The shader at `index`, if there is one.
    pub fn shader(&self, index: usize) -> Option<&Shader> {
        self.shaders.as_ref()?.get(index)?.as_ref()
    }

    /// The group's XML: the texture dictionary (DDS files to `w.externals`)
    /// and the shaders.
    pub fn write_xml(&self, w: &mut XmlWriter) {
        let txd = self.texture_dictionary.as_ref();
        if let Some(td) = txd {
            w.node("TextureDictionary", |w| td.write_xml(w));
        }
        if let Some(shaders) = &self.shaders {
            w.node_array_opt("Shaders", Some(shaders), |w, s| s.write_xml(w, txd));
        }
    }

    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let mut g = Self {
            texture_dictionary: node.node("TextureDictionary").map(TextureDictionary::read_xml).transpose()?,
            ..Default::default()
        };
        let shaders = node_items(node, "Shaders", Shader::read_xml)?;
        g.shaders_capacity = shaders.as_ref().map_or(0, |s| s.len() as u16);
        g.shaders = shaders.map(|v| v.into_iter().map(Some).collect());
        // Parameters pointing to an imported texture use the embedded one.
        if let (Some(td), Some(shaders)) = (&g.texture_dictionary, &mut g.shaders) {
            let mut map = HashMap::new();
            let n = td.textures.as_ref().map_or(0, |t| t.len()).min(td.hashes.as_ref().map_or(0, |h| h.len()));
            for i in 0..n {
                if td.textures.as_ref().unwrap()[i].is_some() {
                    map.insert(td.hashes.as_ref().unwrap()[i], i);
                }
            }
            for s in shaders.iter_mut().flatten() {
                if let Some(p) = &mut s.parameters {
                    p.use_embedded(&map);
                }
            }
        }
        Ok(g)
    }
}
