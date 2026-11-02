//! `Rsc8BoundGeometry` and `Rsc8BoundBVH`: triangle meshes and their primitives
//! (spheres, capsules, boxes, cylinders), with materials per polygon.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use super::bounds::Bounds;
use super::bvh::{self, BuildItem, Bvh};
use super::material::BoundMaterial;
use super::math::{self, Mat4, Vec3};
use super::reader::{BoundsReader, Tag};
use crate::rsc8::{BlockBuf, BlockId, Rsc8Writer};
use crate::xml::number::{f32_to_string, parse_f32};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8BoundPolyTriangle`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PolyTriangle {
    pub tri_area: f32,
    pub vert_index1: u16,
    pub vert_index2: u16,
    pub vert_index3: u16,
    pub edge_index1: i16,
    pub edge_index2: i16,
    pub edge_index3: i16,
}

/// `Rsc8BoundPolySphere`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PolySphere {
    pub vert_index: u16,
    pub sphere_type: u16,
    pub sphere_radius: f32,
    pub unused0: u32,
    pub unused1: u32,
}

/// `Rsc8BoundPolyCapsule`, also `Rsc8BoundPolyCylinder`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PolyCapsule {
    pub vert_index1: u16,
    pub capsule_type: u16,
    pub capsule_radius: f32,
    pub vert_index2: u16,
    pub unused0: u16,
    pub unused1: u32,
}

/// `Rsc8BoundPolyBox`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PolyBox {
    pub box_type: u32,
    pub vert_index1: i16,
    pub vert_index2: i16,
    pub vert_index3: i16,
    pub vert_index4: i16,
    pub unused0: u32,
}

/// A polygon's 16 bytes, by its type (the low 3 bits of byte 3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PolygonKind {
    Triangle(PolyTriangle),
    Sphere(PolySphere),
    Capsule(PolyCapsule),
    Box(PolyBox),
    Cylinder(PolyCapsule),
    /// Types 5 to 7, which the C# neither reads nor writes.
    Other(u8),
}

/// `Rsc8BoundPolygon`: a polygon and the index of its material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundPolygon {
    pub kind: PolygonKind,
    pub material_index: i32,
}

impl BoundPolygon {
    fn poly_type(&self) -> u8 {
        match self.kind {
            PolygonKind::Triangle(_) => 0,
            PolygonKind::Sphere(_) => 1,
            PolygonKind::Capsule(_) => 2,
            PolygonKind::Box(_) => 3,
            PolygonKind::Cylinder(_) => 4,
            PolygonKind::Other(t) => t,
        }
    }

    fn from_bytes(d: &[u8], material_index: i32) -> Self {
        let read = |d: &[u8]| -> [u8; 16] { d[..16].try_into().unwrap() };
        let b = read(d);
        let kind = match b[3] & 7 {
            0 => PolygonKind::Triangle(bytemuck::cast(b)),
            1 => PolygonKind::Sphere(bytemuck::cast(b)),
            2 => PolygonKind::Capsule(bytemuck::cast(b)),
            3 => PolygonKind::Box(bytemuck::cast(b)),
            4 => PolygonKind::Cylinder(bytemuck::cast(b)),
            t => PolygonKind::Other(t),
        };
        Self { kind, material_index }
    }

    /// The 16 bytes, with the type in the low bits of byte 3.
    fn to_bytes(self) -> [u8; 16] {
        let mut b: [u8; 16] = match &self.kind {
            PolygonKind::Triangle(p) => bytemuck::cast(*p),
            PolygonKind::Sphere(p) => bytemuck::cast(*p),
            PolygonKind::Capsule(p) | PolygonKind::Cylinder(p) => bytemuck::cast(*p),
            PolygonKind::Box(p) => bytemuck::cast(*p),
            PolygonKind::Other(_) => [0; 16],
        };
        b[3] = (b[3] & 0xF8) | (self.poly_type() & 7);
        b
    }

    /// The polygon's line in the XML (`MetaString`); `None` for other types.
    pub fn meta_string(&self) -> Option<String> {
        let m = self.material_index;
        Some(match &self.kind {
            PolygonKind::Box(p) => {
                format!("Box {m} {} {} {} {}", p.vert_index1, p.vert_index2, p.vert_index3, p.vert_index4)
            }
            PolygonKind::Sphere(p) => format!("Sph {m} {} {}", p.vert_index, f32_to_string(p.sphere_radius)),
            PolygonKind::Capsule(p) => {
                format!("Cap {m} {} {} {}", p.vert_index1, p.vert_index2, f32_to_string(p.capsule_radius))
            }
            PolygonKind::Cylinder(p) => {
                format!("Cyl {m} {} {} {}", p.vert_index1, p.vert_index2, f32_to_string(p.capsule_radius))
            }
            PolygonKind::Triangle(p) => format!("Tri {m} {} {} {}", p.vert_index1, p.vert_index2, p.vert_index3),
            PolygonKind::Other(_) => return None,
        })
    }

    /// Parses a `MetaString` line. Like the C#, an unknown type gives a
    /// triangle, and unparsable numbers give 0.
    pub fn from_meta_string(s: &str) -> Self {
        let parts: Vec<&str> = s.split(' ').map(str::trim).filter(|p| !p.is_empty()).collect();
        let mut p = Self { kind: PolygonKind::Triangle(PolyTriangle::default()), material_index: 0 };
        if parts.len() < 2 {
            return p;
        }
        p.material_index = parse_int(parts[1]).and_then(|v| i32::try_from(v).ok()).unwrap_or(0);
        let u16_at = |i: usize| parse_int(parts[i]).and_then(|v| u16::try_from(v).ok()).unwrap_or(0);
        let i16_at = |i: usize| parse_int(parts[i]).and_then(|v| i16::try_from(v).ok()).unwrap_or(0);
        let f32_at = |i: usize| parse_f32(parts[i]).unwrap_or(0.0);
        match parts[0] {
            "Box" => {
                let mut b = PolyBox::default();
                if parts.len() >= 6 {
                    b.vert_index1 = i16_at(2);
                    b.vert_index2 = i16_at(3);
                    b.vert_index3 = i16_at(4);
                    b.vert_index4 = i16_at(5);
                }
                p.kind = PolygonKind::Box(b);
            }
            "Sph" => {
                let mut b = PolySphere::default();
                if parts.len() >= 4 {
                    b.vert_index = u16_at(2);
                    b.sphere_radius = f32_at(3);
                }
                p.kind = PolygonKind::Sphere(b);
            }
            "Cap" | "Cyl" => {
                let mut b = PolyCapsule::default();
                if parts.len() >= 5 {
                    b.vert_index1 = u16_at(2);
                    b.vert_index2 = u16_at(3);
                    b.capsule_radius = f32_at(4);
                }
                p.kind = if parts[0] == "Cap" { PolygonKind::Capsule(b) } else { PolygonKind::Cylinder(b) };
            }
            "Tri" => {
                let mut b = PolyTriangle::default();
                if parts.len() >= 5 {
                    b.vert_index1 = u16_at(2);
                    b.vert_index2 = u16_at(3);
                    b.vert_index3 = u16_at(4);
                }
                p.kind = PolygonKind::Triangle(b);
            }
            _ => {}
        }
        p
    }

    /// The triangle's vertex indices; (0, 0, 0) for other polygons, whose
    /// `Triangle` struct the C# leaves empty.
    fn triangle_indices(&self) -> [usize; 3] {
        match &self.kind {
            PolygonKind::Triangle(t) => [t.vert_index1 as usize, t.vert_index2 as usize, t.vert_index3 as usize],
            _ => [0; 3],
        }
    }

    fn set_edge(&mut self, edge: u8, poly: i16) {
        if let PolygonKind::Triangle(t) = &mut self.kind {
            match edge {
                1 => t.edge_index1 = poly,
                2 => t.edge_index2 = poly,
                3 => t.edge_index3 = poly,
                _ => {}
            }
        }
    }

    /// The polygon's box (`BoxMin`/`BoxMax`), from vertex positions.
    fn bounds(&self, vertex: &impl Fn(i32) -> Vec3) -> (Vec3, Vec3) {
        match &self.kind {
            PolygonKind::Box(b) => {
                let v = [b.vert_index1, b.vert_index2, b.vert_index3, b.vert_index4].map(|i| vertex(i as i32));
                (
                    math::min(math::min(math::min(v[0], v[1]), v[2]), v[3]),
                    math::max(math::max(math::max(v[0], v[1]), v[2]), v[3]),
                )
            }
            PolygonKind::Sphere(s) => {
                let v = vertex(s.vert_index as i32);
                let r = math::splat(s.sphere_radius);
                (math::sub(v, r), math::add(v, r))
            }
            PolygonKind::Capsule(c) | PolygonKind::Cylinder(c) => {
                let (v1, v2) = (vertex(c.vert_index1 as i32), vertex(c.vert_index2 as i32));
                let r = math::splat(c.capsule_radius);
                (math::sub(math::min(v1, v2), r), math::add(math::max(v1, v2), r))
            }
            PolygonKind::Triangle(t) => {
                let v = [t.vert_index1, t.vert_index2, t.vert_index3].map(|i| vertex(i as i32));
                (math::min(math::min(v[0], v[1]), v[2]), math::max(math::max(v[0], v[1]), v[2]))
            }
            PolygonKind::Other(_) => ([0.0; 3], [0.0; 3]),
        }
    }
}

/// An integer as .NET's `int.TryParse` reads one (sign, digits).
fn parse_int(s: &str) -> Option<i64> {
    s.trim().parse::<i64>().ok()
}

/// `Rsc8BoundGeometry`'s own fields.
///
/// Vertices are kept as positions relative to `center_geom` (the quantized
/// shorts times `quantum`); writing quantizes them again with a new quantum.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundGeometry {
    pub unknown_70h: u64,
    pub unknown_80h: u16,
    pub unknown_82h: u16,
    pub quantum: Vec3,
    pub quantum_w: f32,
    pub center_geom: Vec3,
    pub center_geom_w: f32,
    pub unknown_d8h: u64,
    pub unknown_e0h: u64,
    pub unknown_e8h: u64,
    pub unknown_100h: u64,
    pub unknown_108h: u64,
    pub unknown_110h: u64,
    pub unknown_122h: u16,
    pub unknown_124h: u32,
    pub unknown_128h: u64,
    pub vertices: Option<Vec<Vec3>>,
    /// The vertices pulled in by the margin (plain geometries).
    pub vertices_shrunk: Option<Vec<Vec3>>,
    pub vertex_colours: Option<Vec<[u8; 4]>>,
    /// For each of the 8 octants, the shrunk vertices that are extreme in it.
    pub octants: Option<Vec<Option<Vec<u32>>>>,
    pub materials: Option<Vec<BoundMaterial>>,
    pub material_colours: Option<Vec<[u8; 4]>>,
    pub polygons: Vec<BoundPolygon>,
}

/// `Rsc8BoundBVH`'s fields beyond the geometry's.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundBvhPart {
    /// Only read when the pointer is above 0xFFFF, like the C#.
    pub bvh: Option<Bvh>,
    pub unknown_138h: u64,
    pub unknown_140h: u16,
    pub unknown_142h: u16,
    pub unknown_144h: u32,
    pub unknown_148h: u64,
}

impl Default for BoundBvhPart {
    fn default() -> Self {
        Self { bvh: None, unknown_138h: 0, unknown_140h: 0xFFFF, unknown_142h: 0, unknown_144h: 0, unknown_148h: 0 }
    }
}

impl BoundBvhPart {
    pub(crate) fn read(br: &mut BoundsReader) -> Result<Self> {
        let r = &mut *br.r;
        let ptr = r.u64()?;
        let mut p = Self {
            bvh: None,
            unknown_138h: r.u64()?,
            unknown_140h: r.u16()?,
            unknown_142h: r.u16()?,
            unknown_144h: r.u32()?,
            unknown_148h: r.u64()?,
        };
        if ptr > 0xFFFF {
            p.bvh = br.block(Tag::Bvh, ptr, Bvh::read)?;
        }
        Ok(p)
    }
}

fn dequantize(v: &[[i16; 3]], q: Vec3) -> Vec<Vec3> {
    v.iter().map(|s| math::mul(s.map(|c| c as f32), q)).collect()
}

/// `new Vector3S(v / quantum)`: truncation, saturating to an int, then wrapping
/// to a short (.NET's conversions).
fn quantize(v: &[Vec3], q: Vec3) -> Vec<[i16; 3]> {
    v.iter().map(|v| math::div(*v, q).map(|c| c as i32 as i16)).collect()
}

/// The arrays written for a geometry, as `UpdateStuffBeforeSaving` makes them.
struct Prepared {
    geom: BoundGeometry,
    vertices_data: Option<Vec<[i16; 3]>>,
    shrunk_data: Option<Vec<[i16; 3]>>,
    vertices_count: u32,
    polygons_data: Option<Vec<u8>>,
    polygon_materials: Option<Vec<u8>>,
    materials: Vec<BoundMaterial>,
    materials_count: u8,
    material_colours_count: u8,
    bvh: Option<Bvh>,
}

impl BoundGeometry {
    pub(crate) fn read(br: &mut BoundsReader) -> Result<Self> {
        let r = &mut *br.r;
        let mut g = BoundGeometry { unknown_70h: r.u64()?, ..Default::default() };
        let shrunk_ptr = r.u64()?;
        g.unknown_80h = r.u16()?;
        g.unknown_82h = r.u16()?;
        let vertices2_count = r.u32()? as usize;
        let polygons_ptr = r.u64()?;
        g.quantum = r.vec3()?;
        g.quantum_w = r.f32()?;
        g.center_geom = r.vec3()?;
        g.center_geom_w = r.f32()?;
        let vertices_ptr = r.u64()?;
        let colours_ptr = r.u64()?;
        let octant_counts_ptr = r.u64()?;
        let octant_ptrs_ptr = r.u64()?;
        let vertices_count = r.u32()? as usize;
        let polygons_count = r.u32()?;
        g.unknown_d8h = r.u64()?;
        g.unknown_e0h = r.u64()?;
        g.unknown_e8h = r.u64()?;
        let materials_ptr = r.u64()?;
        let material_colours_ptr = r.u64()?;
        g.unknown_100h = r.u64()?;
        g.unknown_108h = r.u64()?;
        g.unknown_110h = r.u64()?;
        let polygon_materials_ptr = r.u64()?;
        let materials_count = r.u8()? as usize;
        let material_colours_count = r.u8()? as usize;
        g.unknown_122h = r.u16()?;
        g.unknown_124h = r.u32()?;
        g.unknown_128h = r.u64()?;

        let shrunk: Option<Vec<[i16; 3]>> = br.array(Tag::Vector3S, shrunk_ptr, vertices2_count)?;
        g.vertices_shrunk = shrunk.map(|v| dequantize(&v, g.quantum));
        let polygons_data: Option<Vec<u8>> =
            br.array(Tag::U8, polygons_ptr, polygons_count.wrapping_mul(16) as usize)?;
        let vertices: Option<Vec<[i16; 3]>> = br.array(Tag::Vector3S, vertices_ptr, vertices2_count)?;
        g.vertices = vertices.map(|v| dequantize(&v, g.quantum));
        g.vertex_colours = br.array(Tag::Colour, colours_ptr, vertices_count)?;

        let counts: Option<Vec<u32>> = br.array(Tag::U32, octant_counts_ptr, 8)?;
        let pointers: Option<Vec<u64>> = br.array(Tag::U64, octant_ptrs_ptr, 8)?;
        if let (Some(counts), Some(pointers)) = (counts, pointers) {
            let mut octants = Vec::with_capacity(8);
            for i in 0..8 {
                let (Some(&p), Some(&n)) = (pointers.get(i), counts.get(i)) else { bail!("short octant arrays") };
                octants.push(br.array(Tag::U32, p, n as usize)?);
            }
            g.octants = Some(octants);
        }

        let mut materials: Option<Vec<BoundMaterial>> =
            br.array(Tag::Material, materials_ptr, materials_count.max(4))?;
        g.material_colours = br.array(Tag::Colour, material_colours_ptr, material_colours_count)?;
        let polygon_materials: Option<Vec<u8>> = br.array(Tag::U8, polygon_materials_ptr, polygons_count as usize)?;
        // The array is padded to 4 materials: drop the padding.
        if let Some(m) = &mut materials
            && materials_count < 4
        {
            if m.len() < materials_count {
                bail!("{} materials, {materials_count} expected", m.len());
            }
            m.truncate(materials_count);
        }
        g.materials = materials;

        if polygons_count > 0 {
            let Some(data) = polygons_data else { bail!("{polygons_count} polygons but no polygon data") };
            if data.len() < polygons_count as usize * 16 {
                bail!("{} bytes of polygons, {polygons_count} polygons", data.len());
            }
            let mats = polygon_materials.as_deref().unwrap_or_default();
            g.polygons = (0..polygons_count as usize)
                .map(|i| BoundPolygon::from_bytes(&data[i * 16..], mats.get(i).map_or(0, |&m| m as i32)))
                .collect();
        }
        Ok(g)
    }

    /// `GetVertexPos`: a vertex in the parent's space (out of range: the origin).
    fn vertex_pos(&self, i: i32, center: Vec3, transform: &Mat4) -> Vec3 {
        let v = self.vertices.as_deref().unwrap_or_default();
        let p = if i >= 0 && (i as usize) < v.len() { v[i as usize] } else { [0.0; 3] };
        math::transform(math::add(p, center), transform)
    }

    /// `CalculateVertNormals`.
    fn vertex_normals(&self, transform: &Mat4) -> Result<Vec<Vec3>> {
        let n = self.vertices.as_ref().map_or(0, |v| v.len());
        let mut normals = vec![[0.0f32; 3]; n];
        let pos = |i: usize| self.vertex_pos(i as i32, self.center_geom, transform);
        for poly in &self.polygons {
            let [i1, i2, i3] = poly.triangle_indices();
            let (p1, p2, p3) = (pos(i1), pos(i2), pos(i3));
            let normal = math::safe_normal(math::cross(math::sub(p1, p2), math::sub(p3, p2)));
            for i in [i1, i2, i3] {
                let Some(v) = normals.get_mut(i) else { bail!("vertex index {i} out of range ({n} vertices)") };
                *v = math::add(*v, normal);
            }
        }
        for v in &mut normals {
            if *v != [0.0; 3] {
                *v = math::safe_normal(*v);
            }
        }
        Ok(normals)
    }

    /// `CalculateOctants`: per octant, the shrunk vertices no other one is
    /// beyond in that octant's direction.
    fn octants_of(verts: &[Vec3]) -> Vec<Option<Vec<u32>>> {
        const FLIP: [Vec3; 8] = [
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
            [1.0, -1.0, 1.0],
            [-1.0, -1.0, 1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [1.0, -1.0, -1.0],
            [-1.0, -1.0, -1.0],
        ];
        let shadowed = |v1: Vec3, v2: Vec3, o: usize| {
            let d = math::mul(math::sub(v2, v1), FLIP[o]);
            d[0] >= 0.0 && d[1] >= 0.0 && d[2] >= 0.0
        };
        (0..8)
            .map(|o| {
                let mut ind: Vec<u32> = Vec::new();
                for i1 in 0..verts.len() {
                    let v = verts[i1];
                    let mut next = Vec::new();
                    let mut add = true;
                    for &i2 in &ind {
                        let v2 = verts[i2 as usize];
                        if shadowed(v, v2, o) {
                            add = false;
                            break;
                        }
                        if !shadowed(v2, v, o) {
                            next.push(i2);
                        }
                    }
                    if add {
                        next.push(i1 as u32);
                        ind = next;
                    }
                }
                Some(ind)
            })
            .collect()
    }

    /// `CalculateQuantum`: the quantum and centre from the bound's box.
    fn calculate_quantum(&mut self, b: &Bounds) {
        let c = math::scale(math::add(b.box_min, b.box_max), 0.5);
        let half = math::scale(math::sub(b.box_max, b.box_min), 0.5);
        self.quantum = math::div_scalar(half, 32767.0);
        self.center_geom = c;
    }

    /// `UpdateEdgeIndices`: each triangle edge gets the triangle across it, or -1.
    fn update_edges(polys: &mut [BoundPolygon]) {
        struct Edge {
            t1: usize,
            e1: u8,
            t2: Option<(usize, u8)>,
        }
        let mut edges: Vec<Edge> = Vec::new();
        let mut index: HashMap<(usize, usize), usize> = HashMap::new();
        for (pi, poly) in polys.iter_mut().enumerate() {
            if !matches!(poly.kind, PolygonKind::Triangle(_)) {
                continue;
            }
            let [a, b, c] = poly.triangle_indices();
            for (eid, (v1, v2)) in [(1u8, (a, b)), (2, (b, c)), (3, (c, a))] {
                let key = (v1.min(v2), v1.max(v2));
                match index.get(&key) {
                    Some(&ei) => {
                        if edges[ei].t2.is_some() {
                            poly.set_edge(eid, edges[ei].t1 as i16);
                        } else {
                            edges[ei].t2 = Some((pi, eid));
                        }
                    }
                    None => {
                        index.insert(key, edges.len());
                        edges.push(Edge { t1: pi, e1: eid, t2: None });
                    }
                }
            }
        }
        for e in &edges {
            match e.t2 {
                None => polys[e.t1].set_edge(e.e1, -1),
                Some((t2, e2)) => {
                    polys[e.t1].set_edge(e.e1, t2 as i16);
                    polys[t2].set_edge(e2, e.t1 as i16);
                }
            }
        }
    }

    /// `Rsc8BoundBVH.BuildBVH`: a BVH of the polygons' boxes (grown by the
    /// margin), which reorders the polygons by node.
    fn build_bvh(&mut self, b: &Bounds) -> Result<Option<Bvh>> {
        if self.polygons.is_empty() {
            return Ok(None);
        }
        if self.vertices.is_none() {
            bail!("geometry has polygons but no vertices");
        }
        let margin = math::splat(b.margin);
        let (center, transform) = (self.center_geom, b.transform);
        let mut items: Vec<Option<BuildItem>> = self
            .polygons
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (min, max) = p.bounds(&|v| self.vertex_pos(v, center, &transform));
                Some(BuildItem { min: math::sub(min, margin), max: math::add(max, margin), index: i as i32 })
            })
            .collect();
        let (tree, order) = bvh::build(&mut items, 4, None);
        if let Some(order) = order {
            self.polygons = order.iter().map(|&i| self.polygons[i]).collect();
        }
        // The edges are remapped here in the C#, but recomputed before writing.
        Ok(Some(tree))
    }

    /// Everything `Write` recomputes, in the C#'s order.
    fn prepare(&self, b: &Bounds, is_bvh: bool) -> Result<Prepared> {
        let mut g = self.clone();
        let mut bvh = None;
        if is_bvh {
            g.calculate_quantum(b);
            bvh = g.build_bvh(b)?;
        }
        // UpdateStuffBeforeSaving
        if is_bvh {
            g.vertices_shrunk = None;
            g.octants = None;
        } else {
            let Some(verts) = &g.vertices else { bail!("geometry has no vertices") };
            let normals = g.vertex_normals(&b.transform)?;
            let shrunk: Vec<Vec3> =
                verts.iter().zip(&normals).map(|(v, n)| math::add(*v, math::scale(*n, -b.margin))).collect();
            g.octants = Some(Self::octants_of(&shrunk));
            g.vertices_shrunk = Some(shrunk);
        }
        g.calculate_quantum(b);
        // BuildMaterials
        let Some(mats) = &g.materials else { bail!("geometry has no materials") };
        let material_colours_count = g.material_colours.as_ref().map_or(0, |c| c.len()) as u8;
        let materials_count = mats.len() as u8;
        let mut materials = mats.clone();
        while materials.len() < 4 {
            materials.push(BoundMaterial::default());
        }
        Self::update_edges(&mut g.polygons);
        if !g.polygons.is_empty() && g.vertices.is_none() {
            bail!("geometry has polygons but no vertices");
        }
        // UpdateTriangleAreas: the C# measures each triangle with its first
        // vertex three times, which gives NaN, so float.Epsilon.
        for p in &mut g.polygons {
            if let PolygonKind::Triangle(t) = &mut p.kind {
                t.tri_area = f32::from_bits(1);
            }
        }
        // UpdateVerticesData
        let nonempty = |v: &Option<Vec<Vec3>>| v.as_ref().filter(|v| !v.is_empty()).map(|v| quantize(v, g.quantum));
        let vertices_data = nonempty(&g.vertices);
        let shrunk_data = nonempty(&g.vertices_shrunk);
        let vertices_count = g.vertices.as_ref().map_or(0, |v| v.len()) as u32;
        // UpdatePolygonsData
        let (polygons_data, polygon_materials) = if g.polygons.is_empty() {
            (None, None)
        } else {
            let data: Vec<u8> = g.polygons.iter().flat_map(|p| p.to_bytes()).collect();
            let mats: Vec<u8> = g.polygons.iter().map(|p| p.material_index as u8).collect();
            (Some(data), Some(mats))
        };
        Ok(Prepared {
            geom: g,
            vertices_data,
            shrunk_data,
            vertices_count,
            polygons_data,
            polygon_materials,
            materials,
            materials_count,
            material_colours_count,
            bvh,
        })
    }

    /// Writes the geometry's fields after the base ones (and the BVH part's).
    pub(crate) fn write(
        &self,
        bounds: &Bounds,
        part: Option<&BoundBvhPart>,
        w: &mut Rsc8Writer,
        b: &mut BlockBuf,
    ) -> Result<()> {
        let p = self.prepare(bounds, part.is_some())?;
        let g = &p.geom;
        let polygons_count = g.polygons.len() as u32;
        let shrunk = w.array(p.shrunk_data.as_deref());
        let polygons = w.array(p.polygons_data.as_deref());
        let vertices = w.array(p.vertices_data.as_deref());
        let colours = w.array(g.vertex_colours.as_deref());
        let (octant_counts, octant_ptrs) = match &g.octants {
            None => (None, None),
            Some(oct) => {
                let counts: Vec<u32> = oct.iter().map(|o| o.as_ref().map_or(0, |o| o.len() as u32)).collect();
                let mut empty: Option<BlockId> = None;
                let ids: Vec<Option<BlockId>> = oct
                    .iter()
                    .map(|o| match o {
                        None => None,
                        // Empty lists all become .NET's one empty array.
                        Some(o) if o.is_empty() => Some(*empty.get_or_insert_with(|| w.array(Some(&o[..])).unwrap())),
                        Some(o) => w.array(Some(&o[..])),
                    })
                    .collect();
                (w.array(Some(&counts[..])), Some(w.pointer_array(&ids)))
            }
        };
        let materials = w.array(Some(&p.materials[..]));
        let material_colours = w.array(g.material_colours.as_deref());
        let polygon_materials = w.array(p.polygon_materials.as_deref());

        b.u64(g.unknown_70h);
        b.ptr(shrunk);
        b.u16(g.unknown_80h);
        b.u16(g.unknown_82h);
        b.u32(p.vertices_count);
        b.ptr(polygons);
        b.vec3(g.quantum);
        b.f32(g.quantum_w);
        b.vec3(g.center_geom);
        b.f32(g.center_geom_w);
        b.ptr(vertices);
        b.ptr(colours);
        b.ptr(octant_counts);
        b.ptr(octant_ptrs);
        b.u32(p.vertices_count);
        b.u32(polygons_count);
        b.u64(g.unknown_d8h);
        b.u64(g.unknown_e0h);
        b.u64(g.unknown_e8h);
        b.ptr(materials);
        b.ptr(material_colours);
        b.u64(g.unknown_100h);
        b.u64(g.unknown_108h);
        b.u64(g.unknown_110h);
        b.ptr(polygon_materials);
        b.u8(p.materials_count);
        b.u8(p.material_colours_count);
        b.u16(g.unknown_122h);
        b.u32(g.unknown_124h);
        b.u64(g.unknown_128h);

        if let Some(part) = part {
            let bvh = p.bvh.as_ref().map(|t| t.write(w));
            b.ptr(bvh);
            b.u64(part.unknown_138h);
            b.u16(part.unknown_140h);
            b.u16(part.unknown_142h);
            b.u32(part.unknown_144h);
            b.u64(part.unknown_148h);
        }
        Ok(())
    }

    pub(crate) fn write_xml(&self, w: &mut XmlWriter) {
        if let Some(m) = &self.materials {
            w.node_array("Materials", Some(&m[..]), |w, m| m.write_xml(w));
        }
        w.colour_array("MaterialColours", self.material_colours.as_deref());
        if let Some(v) = &self.vertices {
            let mut s = String::new();
            for v in v {
                s.push_str(&f32_to_string(v[0]));
                s.push_str(", ");
                s.push_str(&f32_to_string(v[1]));
                s.push_str(", ");
                s.push_str(&f32_to_string(v[2]));
                s.push('\n');
            }
            w.string("Vertices", &s);
        }
        w.colour_array("VertexColours", self.vertex_colours.as_deref());
        if !self.polygons.is_empty() {
            let mut s = String::new();
            for p in &self.polygons {
                if let Some(line) = p.meta_string() {
                    s.push_str(&line);
                }
                s.push('\n');
            }
            w.string("Polygons", &s);
        }
    }

    pub(crate) fn read_xml(n: &XmlNode) -> Self {
        let lines = |id: &str| -> Vec<String> {
            n.string(id)
                .map(|s| s.split('\n').map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let mut g = BoundGeometry {
            // Items with type="null" are default materials, not left out.
            materials: n.node_array("Materials").map(|items| {
                items
                    .into_iter()
                    .map(|i| i.map_or_else(BoundMaterial::default, |i| BoundMaterial::read_xml(&i)))
                    .collect()
            }),
            material_colours: n.colour_array("MaterialColours").filter(|c| !c.is_empty()),
            vertex_colours: n.colour_array("VertexColours").filter(|c| !c.is_empty()),
            ..Default::default()
        };
        let verts: Vec<Vec3> = lines("Vertices").iter().map(|l| parse_vector3(l)).collect();
        g.vertices = (!verts.is_empty()).then_some(verts);
        g.polygons = lines("Polygons").iter().map(|l| BoundPolygon::from_meta_string(l)).collect();
        g
    }
}

/// `FloatUtil.ParseVector3String`: "x, y, z", missing or bad values 0.
fn parse_vector3(s: &str) -> Vec3 {
    let mut v = [0.0f32; 3];
    for (d, part) in v.iter_mut().zip(s.split(',')) {
        *d = parse_f32(part.trim()).unwrap_or(0.0);
    }
    v
}
