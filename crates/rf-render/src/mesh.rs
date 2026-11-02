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

/// What skinned vertices have beyond a [`Vertex`]: up to 8 bones (as RDR2's
/// skinned geometry has, in two sets of 4) by their index in the model's bone
/// mapping, and their weights. A second vertex stream, only uploaded for skinned
/// meshes that are drawn posed; the shader reads it as one `vec4<u32>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct VertexSkin {
    /// Unorm8 weights of the first 4 bones, then of the next 4.
    pub weights: [[u8; 4]; 2],
    /// The bones' indices; the first index 255 marks a cloth vertex (whose
    /// other values are cloth vertices, not bones).
    pub indices: [[u8; 4]; 2],
}

impl VertexSkin {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![12 => Uint32x4];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<VertexSkin>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Decodes a geometry's bone weights and indices, `None` when it has neither.
/// Values are read as the C#'s skinning shader does (`index * 255.002`,
/// truncated); a geometry with indices but no weights gives the first bone all
/// the weight.
///
/// Cloth vertices (manes, coat tails) follow a cloth simulation in the game,
/// which isn't done here: each takes the bones of the nearest skinned vertex of
/// the geometry, so that it moves with the body (stiffly) instead of staying in
/// the bind pose. All-cloth geometry keeps its cloth marks and stays as it is.
pub fn decode_skin(g: &Geometry) -> Option<Vec<VertexSkin>> {
    let layout = g.layout;
    let sems: Vec<u8> = (0..layout.element_count()).map(|e| layout.semantic(e) & 7).collect();
    if !sems.contains(&semantic::BLEND_INDICES) {
        return None;
    }
    let mut out = vec![VertexSkin { weights: [[255, 0, 0, 0], [0; 4]], indices: [[0; 4]; 2] }; g.vertex_count()];
    let mut positions = vec![[0.0f32; 3]; g.vertex_count()];
    let mut seen = [0usize; 8];
    for (e, &sem) in sems.iter().enumerate() {
        let index = seen[sem as usize];
        seen[sem as usize] += 1;
        if index > 1 || !matches!(sem, semantic::BLEND_WEIGHTS | semantic::BLEND_INDICES | semantic::POSITION) {
            continue;
        }
        for (v, out) in out.iter_mut().enumerate() {
            let Ok(x) = g.vertex_value(v, e) else { break };
            match sem {
                semantic::BLEND_WEIGHTS => out.weights[index] = to_unorm8(x),
                semantic::BLEND_INDICES => out.indices[index] = x.map(|c| (c * 255.002).clamp(0.0, 255.0) as u8),
                _ if index == 0 => positions[v] = [x[0], x[1], x[2]],
                _ => {}
            }
        }
    }
    attach_cloth(&mut out, &positions);
    Some(out)
}

/// Gives each cloth vertex the bones of the nearest skinned one (sweeping the
/// skinned vertices sorted along x).
fn attach_cloth(skin: &mut [VertexSkin], positions: &[[f32; 3]]) {
    let is_cloth = |s: &VertexSkin| s.indices[0][0] == 255;
    let mut skinned: Vec<usize> = (0..skin.len()).filter(|&v| !is_cloth(&skin[v])).collect();
    if skinned.is_empty() || skinned.len() == skin.len() {
        return;
    }
    skinned.sort_by(|&a, &b| positions[a][0].total_cmp(&positions[b][0]));
    let dist2 = |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum::<f32>();
    for v in 0..skin.len() {
        if !is_cloth(&skin[v]) {
            continue;
        }
        let p = positions[v];
        let start = skinned.partition_point(|&s| positions[s][0] < p[0]);
        let (mut best, mut best_d) = (skinned[start.min(skinned.len() - 1)], f32::MAX);
        // Outwards from the vertex's x both ways, until x alone is too far.
        for &s in &skinned[start..] {
            let dx = positions[s][0] - p[0];
            if dx * dx >= best_d {
                break;
            }
            let d = dist2(positions[s], p);
            if d < best_d {
                (best, best_d) = (s, d);
            }
        }
        for &s in skinned[..start].iter().rev() {
            let dx = p[0] - positions[s][0];
            if dx * dx >= best_d {
                break;
            }
            let d = dist2(positions[s], p);
            if d < best_d {
                (best, best_d) = (s, d);
            }
        }
        skin[v] = skin[best];
    }
}

pub struct GpuMesh {
    pub vertices: wgpu::Buffer,
    /// [`VertexExtra`]s, for terrain.
    pub extra: Option<wgpu::Buffer>,
    /// [`VertexSkin`]s, for skinned meshes uploaded to be drawn posed.
    pub skin: Option<wgpu::Buffer>,
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
        Self::upload_with(ctx, g, extra, false)
    }

    /// Uploads a geometry; `extra` adds the [`VertexExtra`] stream, `skin` the
    /// [`VertexSkin`] one (when the geometry has bone indices).
    pub fn upload_with(ctx: &GpuContext, g: &Geometry, extra: bool, skin: bool) -> Option<Self> {
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
        let skin = skin.then(|| decode_skin(g)).flatten().map(|s| {
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label,
                contents: bytemuck::cast_slice(&s),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        Some(Self {
            vertices: vb,
            extra,
            skin,
            indices: ib,
            index_count,
            lines: g.topology == Topology::LineList,
            min,
            max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloth_vertices_take_the_nearest_skinned_vertex_bones() {
        let bone = |i: u8| VertexSkin { weights: [[255, 0, 0, 0], [0; 4]], indices: [[i, 0, 0, 0], [0; 4]] };
        let cloth = VertexSkin { weights: [[85, 85, 85, 128], [0; 4]], indices: [[255, 1, 2, 3], [0; 4]] };
        let mut skin = [bone(4), cloth, bone(7), cloth];
        let positions = [[0.0, 0.0, 0.0], [0.9, 0.0, 0.3], [1.0, 0.0, 0.0], [-0.2, 5.0, 0.0]];
        attach_cloth(&mut skin, &positions);
        assert_eq!(skin, [bone(4), bone(7), bone(7), bone(4)]);

        // All cloth: nothing to follow, the marks stay.
        let mut all = [cloth, cloth];
        attach_cloth(&mut all, &positions[..2]);
        assert_eq!(all, [cloth, cloth]);
    }
}
