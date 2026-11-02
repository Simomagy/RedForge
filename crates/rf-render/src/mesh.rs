//! Game geometry as GPU vertex and index buffers.

use bytemuck::{Pod, Zeroable};
use rf_formats::formats::ydr::{Geometry, Topology, semantic};
use wgpu::util::DeviceExt;

use crate::GpuContext;

/// The one vertex layout every game geometry is converted to.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// xyz, and the bitangent's sign in w.
    pub tangent: [f32; 4],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    pub colour0: [u8; 4],
    pub colour1: [u8; 4],
}

impl Vertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x2, 4 => Float32x2, 5 => Unorm8x4, 6 => Unorm8x4
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// What terrain vertices have beyond a [`Vertex`]: a third texcoord and a
/// second tangent. A second vertex stream, only uploaded for terrain.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct VertexExtra {
    pub uv2: [f32; 2],
    pub tangent1: [f32; 4],
}

impl VertexExtra {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![12 => Float32x2, 13 => Float32x4];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<VertexExtra>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

pub struct GpuMesh {
    pub vertices: wgpu::Buffer,
    /// [`VertexExtra`]s, for terrain.
    pub extra: Option<wgpu::Buffer>,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
    pub lines: bool,
    /// Local-space bounds.
    pub min: [f32; 3],
    pub max: [f32; 3],
}

fn to_unorm8(v: [f32; 4]) -> [u8; 4] {
    v.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// Decodes a geometry's vertices into [`Vertex`]es. Elements are matched by
/// semantic, in order (the first texcoord is uv0, the second uv1...).
pub fn decode_vertices(g: &Geometry) -> Vec<Vertex> {
    let n = g.vertex_count();
    let layout = g.layout;
    let mut out =
        vec![
            Vertex { normal: [0.0, 0.0, 1.0], tangent: [1.0, 0.0, 0.0, 1.0], colour0: [255; 4], ..Default::default() };
            n
        ];
    let mut seen = [0usize; 8];
    for e in 0..layout.element_count() {
        let sem = layout.semantic(e) as usize & 7;
        let index = seen[sem];
        seen[sem] += 1;
        for (v, out) in out.iter_mut().enumerate() {
            let Ok(x) = g.vertex_value(v, e) else { break };
            match (sem as u8, index) {
                (semantic::POSITION, 0) => out.position = [x[0], x[1], x[2]],
                (semantic::NORMAL, 0) => out.normal = [x[0], x[1], x[2]],
                (semantic::TANGENT, 0) => out.tangent = [x[0], x[1], x[2], if x[3] < 0.0 { -1.0 } else { 1.0 }],
                (semantic::TEXCOORD, 0) => out.uv0 = [x[0], x[1]],
                (semantic::TEXCOORD, 1) => out.uv1 = [x[0], x[1]],
                (semantic::COLOR, 0) => out.colour0 = to_unorm8(x),
                (semantic::COLOR, 1) => out.colour1 = to_unorm8(x),
                _ => {}
            }
        }
    }
    out
}

/// Decodes a geometry's third texcoord and second tangent.
pub fn decode_extra(g: &Geometry) -> Vec<VertexExtra> {
    let n = g.vertex_count();
    let layout = g.layout;
    let mut out = vec![VertexExtra { tangent1: [1.0, 0.0, 0.0, 1.0], ..Default::default() }; n];
    let mut seen = [0usize; 8];
    for e in 0..layout.element_count() {
        let sem = layout.semantic(e) as usize & 7;
        let index = seen[sem];
        seen[sem] += 1;
        for (v, out) in out.iter_mut().enumerate() {
            let Ok(x) = g.vertex_value(v, e) else { break };
            match (sem as u8, index) {
                (semantic::TANGENT, 1) => out.tangent1 = [x[0], x[1], x[2], if x[3] < 0.0 { -1.0 } else { 1.0 }],
                (semantic::TEXCOORD, 2) => out.uv2 = [x[0], x[1]],
                _ => {}
            }
        }
    }
    out
}

impl GpuMesh {
    /// Uploads a geometry; `extra` adds the [`VertexExtra`] stream.
    pub fn upload(ctx: &GpuContext, g: &Geometry, extra: bool) -> Option<Self> {
        let indices = g.indices()?;
        if indices.is_empty() || g.vertex_count() == 0 {
            return None;
        }
        let vertices = decode_vertices(g);
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for v in &vertices {
            for i in 0..3 {
                min[i] = min[i].min(v.position[i]);
                max[i] = max[i].max(v.position[i]);
            }
        }
        // Index buffers must be a multiple of 4 bytes.
        let mut idx: Vec<u16> = indices.to_vec();
        let index_count = idx.len() as u32;
        if idx.len() % 2 == 1 {
            idx.push(0);
        }
        let label = g.name.as_deref();
        let vb = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label,
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label,
            contents: bytemuck::cast_slice(&idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        let extra = extra.then(|| {
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label,
                contents: bytemuck::cast_slice(&decode_extra(g)),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        Some(Self { vertices: vb, extra, indices: ib, index_count, lines: g.topology == Topology::LineList, min, max })
    }
}
