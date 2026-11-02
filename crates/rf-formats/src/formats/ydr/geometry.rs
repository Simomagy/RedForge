//! Geometries (`Rsc8DrawableGeometry`, with the engine `Mesh` XML) and their
//! vertex and index buffers.

use super::util::read_array;
use super::vertex::{self, VertexLayout, format_size};
use crate::formats::ytd::{ShaderResourceView, srv_dimension};
use crate::rsc8::{BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8DrawableVertexElementFormat`: an element's format in a
/// [`VertexFormat`].
pub mod file_format {
    pub const NONE: u8 = 0;
    pub const R32G32B32A32_FLOAT: u8 = 2;
    pub const R32G32B32_FLOAT: u8 = 6;
    pub const R16G16B16A16_FLOAT: u8 = 10;
    pub const R32G32_TYPELESS: u8 = 16;
    pub const D3DX_R10G10B10A2: u8 = 24;
    pub const R8G8B8A8_UNORM: u8 = 28;
    pub const R8G8B8A8_UINT: u8 = 30;
    pub const R16G16_FLOAT: u8 = 34;
}

use super::vertex::vertex_format as vf;
use file_format as ff;

fn engine_format(t: u8) -> u8 {
    match t {
        ff::R32G32B32A32_FLOAT => vf::FLOAT4,
        ff::R32G32B32_FLOAT => vf::FLOAT3,
        ff::R16G16B16A16_FLOAT => vf::HALF4,
        ff::R32G32_TYPELESS => vf::FLOAT2,
        ff::D3DX_R10G10B10A2 => vf::DEC3N,
        ff::R8G8B8A8_UNORM => vf::COLOUR,
        ff::R8G8B8A8_UINT => vf::UBYTE4,
        ff::R16G16_FLOAT => vf::HALF2,
        _ => vf::NONE,
    }
}

fn file_element_format(f: u8) -> u8 {
    match f {
        vf::FLOAT4 => ff::R32G32B32A32_FLOAT,
        vf::FLOAT3 => ff::R32G32B32_FLOAT,
        vf::HALF4 => ff::R16G16B16A16_FLOAT,
        vf::FLOAT2 => ff::R32G32_TYPELESS,
        vf::DEC3N => ff::D3DX_R10G10B10A2,
        vf::COLOUR => ff::R8G8B8A8_UNORM,
        vf::UBYTE4 => ff::R8G8B8A8_UINT,
        vf::HALF2 => ff::R16G16_FLOAT,
        _ => ff::NONE,
    }
}

/// The engine semantic of RAGE's element slot `i` (0-51).
fn engine_semantic(i: usize) -> u64 {
    match i / 4 {
        1 => 3,      // NORMAL
        2 => 6,      // TANGENT
        3 => 7,      // BINORMAL
        4 => 1,      // BLENDWEIGHTS
        5 => 2,      // BLENDINDICES
        6 => 4,      // COLOR
        7..=12 => 5, // TEXCOORD
        _ => 0,      // POSITION
    }
}

/// RAGE's element slot for the `si`th element of engine semantic `s`.
fn file_element_index(s: u64, si: u32) -> usize {
    let base = match s {
        1 => 16,
        2 => 20,
        3 => 4,
        4 => 24,
        5 => 28,
        6 => 8,
        7 => 12,
        _ => 0,
    };
    base + si as usize
}

/// `Rsc8DrawableVertexFormat` (316 bytes, `rage::grcFvf`): the offset, size
/// and format of each of RAGE's 52 element slots, and a packed word with the
/// vertex size and count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexFormat {
    pub offsets: [u32; 52],
    pub sizes: [u8; 52],
    /// See [`file_format`].
    pub types: [u8; 52],
    /// Bit 0: SOA (always set), bits 2-9: vertex size, 10-31: vertex count.
    pub data: u32,
}

impl Default for VertexFormat {
    fn default() -> Self {
        Self { offsets: [0; 52], sizes: [0; 52], types: [0; 52], data: 0 }
    }
}

impl VertexFormat {
    pub const BLOCK_LENGTH: usize = 316;

    /// The format of non-interleaved data of `vertex_count` vertices with this
    /// layout; colours start at slot `colour_semantic_index`.
    pub fn new(layout: &VertexLayout, vertex_count: usize, colour_semantic_index: u32) -> Self {
        let mut f = Self::default();
        let stride = (layout.stride() as u32) & 0xFF;
        let count = (vertex_count as u32) & 0x3F_FFFF;
        let mut semcounts = [0u32; 8];
        semcounts[4] = colour_semantic_index;
        let mut offset = 0u32;
        for i in 0..16 {
            let efmt = ((layout.formats >> (i * 4)) & 0xF) as u8;
            if efmt == vf::NONE {
                break;
            }
            let esiz = format_size(efmt) as u8;
            let esem = (layout.semantics >> (i * 4)) & 0x7;
            let semi = &mut semcounts[esem as usize];
            let fei = file_element_index(esem, *semi);
            if fei >= 52 {
                // The C# fails here (index out of range); keep what fits.
                break;
            }
            f.offsets[fei] = offset;
            f.sizes[fei] = esiz;
            f.types[fei] = file_element_format(efmt);
            offset = offset.wrapping_add((esiz as u32).wrapping_mul(count));
            *semi += 1;
        }
        f.data = 1 + (stride << 2) + (count << 10);
        f
    }

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut f = Self::default();
        for o in &mut f.offsets {
            *o = r.u32()?;
        }
        f.sizes.copy_from_slice(r.bytes(52)?);
        f.types.copy_from_slice(r.bytes(52)?);
        f.data = r.u32()?;
        Ok(f)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        for o in self.offsets {
            b.u32(o);
        }
        b.bytes(&self.sizes);
        b.bytes(&self.types);
        b.u32(self.data);
        w.add(b, false)
    }

    pub fn vertex_size(&self) -> u8 {
        ((self.data >> 2) & 0xFF) as u8
    }

    pub fn vertex_count(&self) -> u32 {
        (self.data >> 10) & 0x3F_FFFF
    }

    /// The first colour slot used (`ColourSemanticIndex`).
    pub fn colour_semantic_index(&self) -> u32 {
        (0..4).find(|&i| self.types[24 + i] != 0).unwrap_or(0) as u32
    }

    /// `GetEngineLayout`: the used slots in order, as a non-interleaved layout.
    pub fn engine_layout(&self) -> VertexLayout {
        let mut l = VertexLayout::default();
        let mut eidx = 0u32;
        for (i, &t) in self.types.iter().enumerate() {
            if t != ff::NONE {
                let i4 = eidx * 4;
                l.formats = l.formats.wrapping_add((engine_format(t) as u64).wrapping_shl(i4));
                l.semantics = l.semantics.wrapping_add(engine_semantic(i).wrapping_shl(i4));
                eidx += 1;
            }
        }
        l.set_non_interleaved(true);
        l
    }
}

/// `Rsc8DrawableVertexBuffer` (64 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct VertexBuffer {
    pub vft: u64,
    pub vertex_count: u32,
    pub vertex_stride: u16,
    pub unk1: u16,
    pub bind_flags: u32,
    pub unknown_ch: u32,
    /// `vertex_count * vertex_stride` bytes; `None` for a null pointer or no bytes.
    pub vertex_data: Option<Vec<u8>>,
    pub pad1: u64,
    pub pad2: u64,
    pub srv: Option<ShaderResourceView>,
    pub vertex_format: Option<VertexFormat>,
}

impl Default for VertexBuffer {
    fn default() -> Self {
        Self {
            vft: 0x0000_0001_4091_2400,
            vertex_count: 0,
            vertex_stride: 0,
            unk1: 0,
            bind_flags: 0x0020_9000,
            unknown_ch: 0,
            vertex_data: None,
            pad1: 0,
            pad2: 0,
            srv: None,
            vertex_format: None,
        }
    }
}

impl VertexBuffer {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut b = Self { vft: r.u64()?, ..Default::default() };
        b.vertex_count = r.u32()?;
        b.vertex_stride = r.u16()?;
        b.unk1 = r.u16()?;
        b.bind_flags = r.u32()?;
        b.unknown_ch = r.u32()?;
        let data_ptr = r.u64()?;
        b.pad1 = r.u64()?;
        b.pad2 = r.u64()?;
        b.srv = r.ptr(ShaderResourceView::read)?;
        b.vertex_format = r.ptr(VertexFormat::read)?;
        let n = (b.vertex_count as usize) * (b.vertex_stride as usize);
        b.vertex_data = read_array::<u8>(r, data_ptr, n)?;
        Ok(b)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(64);
        b.u64(self.vft);
        b.u32(self.vertex_count);
        b.u16(self.vertex_stride);
        b.u16(self.unk1);
        b.u32(self.bind_flags);
        b.u32(self.unknown_ch);
        b.ptr(w.array(self.vertex_data.as_deref()));
        b.u64(self.pad1);
        b.u64(self.pad2);
        b.ptr(self.srv.as_ref().map(|s| write_srv(s, w)));
        b.ptr(self.vertex_format.as_ref().map(|f| f.write(w)));
        w.add(b, false)
    }
}

/// `Rsc8DrawableIndexBuffer` (64 bytes): 16-bit indices.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexBuffer {
    pub vft: u64,
    pub indices_count: u32,
    pub index_size: u16,
    pub unk1: u16,
    pub bind_flags: u32,
    pub unknown_14h: u32,
    /// `None` for a null pointer or no indices.
    pub index_data: Option<Vec<u16>>,
    pub unknown_20h: u64,
    pub unknown_28h: u64,
    pub srv: Option<ShaderResourceView>,
    pub unknown_38h: u64,
}

impl Default for IndexBuffer {
    fn default() -> Self {
        Self {
            vft: 0x0000_0001_4091_23e0,
            indices_count: 0,
            index_size: 2,
            unk1: 0,
            bind_flags: 0x0020_a000,
            unknown_14h: 0,
            index_data: None,
            unknown_20h: 0,
            unknown_28h: 0,
            srv: None,
            unknown_38h: 0,
        }
    }
}

impl IndexBuffer {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut b = Self { vft: r.u64()?, ..Default::default() };
        b.indices_count = r.u32()?;
        b.index_size = r.u16()?;
        b.unk1 = r.u16()?;
        b.bind_flags = r.u32()?;
        b.unknown_14h = r.u32()?;
        let data_ptr = r.u64()?;
        b.unknown_20h = r.u64()?;
        b.unknown_28h = r.u64()?;
        b.srv = r.ptr(ShaderResourceView::read)?;
        b.unknown_38h = r.u64()?;
        b.index_data = read_array::<u16>(r, data_ptr, b.indices_count as usize)?;
        Ok(b)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(64);
        b.u64(self.vft);
        b.u32(self.indices_count);
        b.u16(self.index_size);
        b.u16(self.unk1);
        b.u32(self.bind_flags);
        b.u32(self.unknown_14h);
        b.ptr(w.array(self.index_data.as_deref()));
        b.u64(self.unknown_20h);
        b.u64(self.unknown_28h);
        b.ptr(self.srv.as_ref().map(|s| write_srv(s, w)));
        b.u64(self.unknown_38h);
        w.add(b, false)
    }
}

fn write_srv(s: &ShaderResourceView, w: &mut Rsc8Writer) -> BlockId {
    let mut b = BlockBuf::with_capacity(ShaderResourceView::BLOCK_LENGTH);
    s.write(&mut b);
    w.add(b, false)
}

/// A buffer's SRV as the C# makes one from XML.
fn buffer_srv(unknown_08h: u64) -> ShaderResourceView {
    ShaderResourceView { dimension: srv_dimension::BUFFER, unknown_08h, ..Default::default() }
}

/// `MeshTopology`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Topology {
    #[default]
    TriangleList,
    PointList,
    LineList,
}

impl Topology {
    pub fn name(self) -> &'static str {
        match self {
            Topology::TriangleList => "TriangleList",
            Topology::PointList => "PointList",
            Topology::LineList => "LineList",
        }
    }

    fn parse(s: Option<&str>) -> Self {
        match s.map(str::trim) {
            Some("PointList" | "1") => Topology::PointList,
            Some("LineList" | "2") => Topology::LineList,
            _ => Topology::TriangleList,
        }
    }
}

/// `Rsc8DrawableGeometry` (64 bytes): a mesh with one shader.
///
/// The vertices are in [`VertexBuffer::vertex_data`], laid out as
/// [`Geometry::layout`] says (non-interleaved when read from a file: all the
/// positions, then all the normals...); [`Geometry::vertex_value`] decodes
/// one element. Writing recreates the buffers' counts and vertex format from
/// the layout, like the C#.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub vft: u64,
    pub vertex_buffer: Option<VertexBuffer>,
    pub index_buffer: Option<IndexBuffer>,
    pub bone_ids_pointer: u64,
    pub unknown_20h: u64,
    pub unknown_28h: u32,
    pub bones_count: u16,
    pub unknown_2eh: u16,
    pub unknown_30h: u32,
    pub unknown_34h: u32,
    pub unknown_38h: u64,
    /// From the parent model's shader mappings.
    pub shader_id: u16,
    /// From the parent model's geometry bounds: min and max, with a W each.
    pub aabb: [[f32; 4]; 2],

    // The engine mesh's fields that go in the XML.
    /// The shader's name, set from the drawable's shaders when read.
    pub name: Option<String>,
    /// The bone the mesh is attached to (-1: none), set from the model when
    /// the drawable has a skeleton.
    pub bone_index: i32,
    /// `LineList` for the `cable` shader.
    pub topology: Topology,
    /// The engine layout of the vertex data.
    pub layout: VertexLayout,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            vft: 0,
            vertex_buffer: None,
            index_buffer: None,
            bone_ids_pointer: 0,
            unknown_20h: 0,
            unknown_28h: 0,
            bones_count: 0,
            unknown_2eh: 0,
            unknown_30h: 0,
            unknown_34h: 0,
            unknown_38h: 0,
            shader_id: 0,
            aabb: [[0.0; 4]; 2],
            name: None,
            bone_index: -1,
            topology: Topology::TriangleList,
            layout: VertexLayout::default(),
        }
    }
}

impl Geometry {
    pub const BLOCK_LENGTH: usize = 64;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut g = Self { vft: r.u64()?, ..Default::default() };
        g.vertex_buffer = r.ptr(VertexBuffer::read)?;
        g.index_buffer = r.ptr(IndexBuffer::read)?;
        g.bone_ids_pointer = r.u64()?;
        g.unknown_20h = r.u64()?;
        g.unknown_28h = r.u32()?;
        g.bones_count = r.u16()?;
        g.unknown_2eh = r.u16()?;
        g.unknown_30h = r.u32()?;
        g.unknown_34h = r.u32()?;
        g.unknown_38h = r.u64()?;
        if let Some(vb) = &g.vertex_buffer {
            g.layout = vb.vertex_format.as_ref().map(VertexFormat::engine_layout).unwrap_or_default();
        }
        Ok(g)
    }

    /// The vertices.
    pub fn vertex_data(&self) -> Option<&[u8]> {
        self.vertex_buffer.as_ref()?.vertex_data.as_deref()
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_buffer.as_ref().map_or(0, |b| b.vertex_count as usize)
    }

    /// The size of a vertex as stored (the layout's stride for interleaved data).
    pub fn vertex_stride(&self) -> usize {
        self.vertex_buffer.as_ref().map_or(0, |b| b.vertex_stride as usize)
    }

    pub fn indices(&self) -> Option<&[u16]> {
        self.index_buffer.as_ref()?.index_data.as_deref()
    }

    /// Element `e` of vertex `v`, as up to 4 floats (`Mesh.GetVertexValue`).
    pub fn vertex_value(&self, v: usize, e: usize) -> Result<[f32; 4]> {
        let data = self.vertex_data().unwrap_or(&[]);
        let f = self.layout.format(e);
        let o = vertex::element_offset(
            self.layout.non_interleaved(),
            self.vertex_stride(),
            self.vertex_count(),
            v,
            format_size(f),
            self.layout.element_offset(e),
        );
        vertex::element_value(data, o, f)
    }

    /// Adds the geometry and its buffers, recreating the buffers' counts,
    /// stride and vertex format from the layout as `Write` does.
    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let vb = self.vertex_buffer.as_ref().map(|vb| {
            let mut vb = vb.clone();
            let count = vb.vertex_count as usize;
            let csi = vb.vertex_format.as_ref().map_or(0, VertexFormat::colour_semantic_index);
            vb.vertex_format = Some(VertexFormat::new(&self.layout, count, csi));
            vb.write(w)
        });
        let ib = self.index_buffer.as_ref().map(|ib| {
            let mut ib = ib.clone();
            ib.indices_count = ib.index_data.as_ref().map_or(0, |d| d.len() as u32);
            ib.write(w)
        });
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.ptr(vb);
        b.ptr(ib);
        b.u64(self.bone_ids_pointer);
        b.u64(self.unknown_20h);
        b.u32(self.unknown_28h);
        b.u16(self.bones_count);
        b.u16(self.unknown_2eh);
        b.u32(self.unknown_30h);
        b.u32(self.unknown_34h);
        b.u64(self.unknown_38h);
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        w.u16("ShaderID", self.shader_id);
        let [min, max] = self.aabb;
        if min[..3] != [0.0; 3] {
            w.vec3("BoundingBoxMin", [min[0], min[1], min[2]]);
        }
        if max[..3] != [0.0; 3] {
            w.vec3("BoundingBoxMax", [max[0], max[1], max[2]]);
        }
        if self.bones_count != 0 {
            w.u16("BonesCount", self.bones_count);
        }
        if self.unknown_2eh != 0 {
            w.u16("Unknown_2E", self.unknown_2eh);
        }
        if self.unknown_30h != 0 {
            w.u32("Unknown_30", self.unknown_30h);
        }
        if let Some(s) = self.index_buffer.as_ref().and_then(|b| b.srv.as_ref())
            && s.unknown_08h != 0
        {
            w.u64("ISRVUnknown08", s.unknown_08h);
        }
        if let Some(s) = self.vertex_buffer.as_ref().and_then(|b| b.srv.as_ref())
            && s.unknown_08h != 0
        {
            w.u64("VSRVUnknown08", s.unknown_08h);
        }
        if let Some(f) = self.vertex_buffer.as_ref().and_then(|b| b.vertex_format.as_ref()) {
            let csi = f.colour_semantic_index();
            if csi != 0 {
                w.u32("ColourSemantic", csi);
            }
        }
        // Mesh.Write
        if let Some(name) = self.name.as_deref().filter(|n| !n.is_empty()) {
            w.string("Name", name);
        }
        if self.bone_index != -1 {
            w.i32("BoneIndex", self.bone_index);
        }
        if self.topology != Topology::TriangleList {
            w.string("Topology", self.topology.name());
        }
        if self.layout.is_empty() {
            return Ok(());
        }
        let l = self.layout;
        w.node("VertexLayout", |w| {
            w.string("Semantics", &l.semantics_string());
            w.string("Formats", &l.formats_string());
            if l.non_interleaved() {
                w.bool("NonInterleaved", true);
            }
        });
        let text = vertex::vertex_data_string(&l, self.vertex_data(), self.vertex_count(), self.vertex_stride())?;
        w.string("Vertices", &text);
        w.u16_array("Indices", self.indices());
        Ok(())
    }

    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let mut g = Self { shader_id: node.u16("ShaderID", 0), ..Default::default() };
        let min = node.vec3("BoundingBoxMin");
        let max = node.vec3("BoundingBoxMax");
        g.aabb = [[min[0], min[1], min[2], 0.0], [max[0], max[1], max[2], 0.0]];
        g.bones_count = node.u16("BonesCount", 0);
        g.unknown_2eh = node.u16("Unknown_2E", 0);
        g.unknown_30h = node.u32("Unknown_30", 0);
        // Mesh.Read
        g.name = node.string("Name");
        g.bone_index = node.i32("BoneIndex", -1);
        g.topology = Topology::parse(node.string("Topology").as_deref());
        if let Some(l) = node.node("VertexLayout") {
            let (Some(s), Some(f)) = (l.string("Semantics"), l.string("Formats")) else {
                bail!("a vertex layout without Semantics or Formats (the C# fails on it)");
            };
            g.layout = VertexLayout::from_strings(&s, &f);
            if l.bool("NonInterleaved", false) {
                g.layout.set_non_interleaved(true);
            }
        }
        let (data, count, stride) = if g.layout.is_empty() {
            (None, 0, 0)
        } else {
            let Some(text) = node.string("Vertices") else {
                bail!("a mesh with a vertex layout and no Vertices (the C# fails on it)");
            };
            let (d, c, s) = vertex::parse_vertex_data(&g.layout, &text);
            (Some(d), c, s)
        };
        let indices = node.u16_array("Indices");

        g.index_buffer = Some(IndexBuffer {
            indices_count: indices.as_ref().map_or(0, |i| i.len() as u32),
            index_data: indices,
            srv: Some(buffer_srv(node.u64("ISRVUnknown08", 0))),
            ..Default::default()
        });
        let vsrv = buffer_srv(node.u64("VSRVUnknown08", 0));
        let csi = node.u32("ColourSemantic", 0);
        g.vertex_buffer = Some(VertexBuffer {
            vertex_count: count as u32,
            vertex_stride: stride as u16,
            vertex_format: Some(VertexFormat::new(&g.layout, count, csi)),
            vertex_data: data,
            srv: Some(vsrv),
            ..Default::default()
        });
        Ok(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_format_round_trip() {
        let mut l = VertexLayout::from_strings("PNXCCTTT", "39955222");
        l.set_non_interleaved(true);
        let f = VertexFormat::new(&l, 100, 1);
        assert_eq!(f.colour_semantic_index(), 1);
        assert_eq!(f.engine_layout(), l);
        assert_eq!(f.vertex_count(), 100);
        assert_eq!(f.vertex_size() as usize, l.stride());
        // Element after element.
        assert_eq!(f.offsets[4], 12 * 100);
    }
}
