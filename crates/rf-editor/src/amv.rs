//! Ambient mask volumes for interiors that have none.
//!
//! The game darkens the sky's light and reflections inside a building with
//! "ambient mask volumes": boxes, listed in an `amv_zone_<n>.ymt`, each with
//! two 3D textures (in `amv_zone_<n>.ytd`, named `<uuid>_0` and `<uuid>_1`)
//! holding, for every cell of the box, how much of the sky is seen from there
//! in six directions. An interior without one is lit as if it stood in the
//! open, and its metals, which then reflect a sky that isn't drawn indoors,
//! come out black.
//!
//! This bakes one: rays from every cell against the interior's own geometry,
//! the fraction that get out above the horizon being what the cell sees of
//! the sky.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{EulerRot, Mat4, Quat, Vec3};
use rayon::prelude::*;
use rf_formats::formats::XmlContext;
use rf_formats::formats::databag::{DataBag, Value};
use rf_formats::formats::dds::TextureFormat;
use rf_formats::formats::meta::Meta;
use rf_formats::formats::pso::{PsoFile, pso_schema};
use rf_formats::formats::ydr::{self, DrawableBase, semantic};
use rf_formats::formats::ytd::{
    ShaderResourceView, Texture, TextureBase, TextureDictionary, block_count, block_stride, dimension,
    from_engine_format, srv_dimension, tile_mode,
};
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rsc8::{BlockMap, Rsc8Writer};

use crate::interior::{self, Lighting};
use crate::lod;
use crate::placement::{Placement, is_mlo_instance};

/// The zone file's root class, `AMV__CAMVolumeListSaveData`.
const ZONE_CLASS: u32 = 0x8A2D_8245;
/// The class of a zone's items (its name isn't known).
const ITEM_CLASS: u32 = 0x2395_C18C;
/// The highest zone number the game has of its own (`amv_zone_0` to `182`).
pub const LAST_GAME_ZONE: u32 = 182;

/// The six directions a cell's values are for, in the volume's own axes.
pub const DIRECTIONS: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];

/// A volume: a box turned about the vertical, cut into cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub centre: Vec3,
    /// Half its size along its own axes.
    pub half: Vec3,
    /// Turned by this about Z, in radians, counter-clockwise seen from above.
    pub heading: f32,
    pub cells: [u32; 3],
}

impl Volume {
    /// The volume of an interior: its box (`min` to `max`, in the interior's
    /// space: its first room's, which is the whole interior's) placed as the
    /// interior is, with cells about `cell` metres wide.
    pub fn of_interior(position: Vec3, orientation: Quat, min: Vec3, max: Vec3, cell: f32) -> Self {
        let half = (max - min) * 0.5;
        let centre = position + orientation * ((min + max) * 0.5);
        let heading = orientation.to_euler(EulerRot::ZYX).0;
        let count = |size: f32| ((size * 2.0 / cell.max(0.05)).round() as u32).clamp(2, 48);
        Self { centre, half, heading, cells: [count(half.x), count(half.y), count(half.z)] }
    }

    /// The volume's space (the box being -half to half) to the world.
    pub fn to_world(&self) -> Mat4 {
        Mat4::from_rotation_translation(Quat::from_rotation_z(self.heading), self.centre)
    }

    /// A cell's centre, in the volume's space.
    fn cell_centre(&self, x: u32, y: u32, z: u32) -> Vec3 {
        let at = |i: u32, n: u32, half: f32| ((i as f32 + 0.5) / n as f32 * 2.0 - 1.0) * half;
        Vec3::new(
            at(x, self.cells[0], self.half.x),
            at(y, self.cells[1], self.half.y),
            at(z, self.cells[2], self.half.z),
        )
    }
}

/// Triangles to cast rays against, in a tree of boxes.
pub struct Mesh {
    triangles: Vec<[Vec3; 3]>,
    nodes: Vec<Node>,
}

struct Node {
    min: Vec3,
    max: Vec3,
    /// A leaf's triangles are `start..start + count`; a branch (`count` 0)
    /// has its children at `start` and `start + 1`.
    start: u32,
    count: u32,
}

impl Mesh {
    pub fn new(mut triangles: Vec<[Vec3; 3]>) -> Self {
        triangles.retain(|t| t.iter().all(|p| p.is_finite()));
        let mut nodes = Vec::new();
        if !triangles.is_empty() {
            nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, start: 0, count: triangles.len() as u32 });
            let mut pending = vec![0usize];
            while let Some(n) = pending.pop() {
                let (start, count) = (nodes[n].start as usize, nodes[n].count as usize);
                let slice = &mut triangles[start..start + count];
                let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
                let (mut cmin, mut cmax) = (Vec3::MAX, Vec3::MIN);
                for t in slice.iter() {
                    for p in t {
                        min = min.min(*p);
                        max = max.max(*p);
                    }
                    let c = (t[0] + t[1] + t[2]) / 3.0;
                    cmin = cmin.min(c);
                    cmax = cmax.max(c);
                }
                nodes[n].min = min;
                nodes[n].max = max;
                let size = cmax - cmin;
                let axis = if size.x >= size.y && size.x >= size.z {
                    0
                } else if size.y >= size.z {
                    1
                } else {
                    2
                };
                if count <= 4 || size[axis] <= 0.0 {
                    continue;
                }
                let mid = count / 2;
                let key = |t: &[Vec3; 3]| t[0][axis] + t[1][axis] + t[2][axis];
                slice.select_nth_unstable_by(mid, |a, b| key(a).total_cmp(&key(b)));
                let child = nodes.len();
                nodes.push(Node { min, max, start: start as u32, count: mid as u32 });
                nodes.push(Node { min, max, start: (start + mid) as u32, count: (count - mid) as u32 });
                nodes[n].start = child as u32;
                nodes[n].count = 0;
                pending.push(child);
                pending.push(child + 1);
            }
        }
        Self { triangles, nodes }
    }

    pub fn len(&self) -> usize {
        self.triangles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Whether anything is hit within `reach` of `origin` along `dir` (either
    /// side of a triangle counts).
    pub fn blocked(&self, origin: Vec3, dir: Vec3, reach: f32) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let inv = dir.recip();
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let node = &self.nodes[stack[top] as usize];
            let (a, b) = ((node.min - origin) * inv, (node.max - origin) * inv);
            let (near, far) = (a.min(b).max_element().max(0.0), a.max(b).min_element().min(reach));
            // Not `near > far`: a ray along a box's face has NaNs here, and is let through.
            if near > far {
                continue;
            }
            if node.count == 0 {
                if top + 2 <= stack.len() {
                    stack[top] = node.start;
                    stack[top + 1] = node.start + 1;
                    top += 2;
                }
                continue;
            }
            let tris = &self.triangles[node.start as usize..(node.start + node.count) as usize];
            if tris.iter().any(|t| hit(origin, dir, t).is_some_and(|d| d < reach)) {
                return true;
            }
        }
        false
    }
}

/// Where along the ray it meets the triangle, from either side.
fn hit(origin: Vec3, dir: Vec3, t: &[Vec3; 3]) -> Option<f32> {
    let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - t[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let d = e2.dot(q) * inv;
    (d > 1e-4).then_some(d)
}

/// A drawable's triangles (its highest level of detail), through `to`, onto
/// `out`. Glass and decals are left out: light goes through them.
pub fn drawable_triangles(d: &DrawableBase, to: Mat4, out: &mut Vec<[Vec3; 3]>) {
    let Some(lod) = d.lods.iter().flatten().next() else { return };
    for g in lod.models().flat_map(|m| m.geometries()) {
        let shader = g.name.as_deref().unwrap_or_default();
        if shader.contains("glass") || shader.contains("decal") {
            continue;
        }
        let Some(indices) = g.indices() else { continue };
        let Some(element) = (0..g.layout.element_count()).find(|e| g.layout.semantic(*e) == semantic::POSITION) else {
            continue;
        };
        let positions: Vec<Vec3> = (0..g.vertex_count())
            .map(|v| g.vertex_value(v, element).map_or(Vec3::NAN, |p| to.transform_point3(Vec3::new(p[0], p[1], p[2]))))
            .collect();
        for t in indices.as_chunks::<3>().0 {
            let corner = |i: u16| positions.get(i as usize).copied().unwrap_or(Vec3::NAN);
            out.push([corner(t[0]), corner(t[1]), corner(t[2])]);
        }
    }
}

/// How a volume is baked.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Rays cast from a cell for each of its six directions.
    pub rays: u32,
    /// How far a ray goes before it counts as having reached the sky.
    pub reach: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { rays: 96, reach: 60.0 }
    }
}

/// A baked volume: for every cell (X fastest, then Y, then Z), how much of
/// the sky it sees (0 to 1) towards each of [`DIRECTIONS`].
pub struct Baked {
    pub cells: [u32; 3],
    pub values: Vec<[f32; 6]>,
}

/// Bakes `volume` against `mesh` (in the world's space).
pub fn bake(volume: &Volume, mesh: &Mesh, settings: &Settings) -> Baked {
    let [nx, ny, nz] = volume.cells;
    let to_world = volume.to_world();
    let rays = settings.rays.max(1);
    let values = (0..nx * ny * nz)
        .into_par_iter()
        .map(|i| {
            let (x, y, z) = (i % nx, i / nx % ny, i / (nx * ny));
            let origin = to_world.transform_point3(volume.cell_centre(x, y, z));
            // The same pattern of rays turned differently for each cell, so that
            // neighbouring cells don't share its bands.
            let turn = unit(i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B);
            DIRECTIONS.map(|axis| {
                let axis = to_world.transform_vector3(axis);
                let (side, up) = axis.any_orthonormal_pair();
                let open = (0..rays)
                    .filter(|&k| {
                        // Cosine-weighted over the half of the sphere around `axis`.
                        let (u, v) = ((k as f32 + 0.5) / rays as f32, (radical_inverse(k) + turn).fract());
                        let (r, angle) = (u.sqrt(), v * std::f32::consts::TAU);
                        let dir = side * (r * angle.cos()) + up * (r * angle.sin()) + axis * (1.0 - u).sqrt();
                        // The sky is above the horizon: a ray that gets out downwards
                        // meets the ground, which isn't among the triangles.
                        dir.z > 0.0 && !mesh.blocked(origin, dir, settings.reach)
                    })
                    .count();
                open as f32 / rays as f32
            })
        })
        .collect();
    Baked { cells: volume.cells, values }
}

/// The bits of `i` mirrored about the binary point: 1 is 0.5, 2 is 0.25...
fn radical_inverse(i: u32) -> f32 {
    i.reverse_bits() as f32 / 4_294_967_296.0
}

/// A number from 0 to 1 out of a hash.
fn unit(mut h: u32) -> f32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    (h >> 8) as f32 / 16_777_216.0
}

/// Where each texture's red, green and blue come from: an index into
/// [`DIRECTIONS`]. As the game's own volumes have them (a bake of Tumbleweed's
/// saloon lines up with the game's this way and no other): +X, -X and +Y in
/// the first texture, -Y, up and down in the second.
const CHANNELS: [[usize; 3]; 2] = [[0, 1, 2], [3, 4, 5]];

impl Baked {
    /// The volume's two textures, `<uuid>_0` and `<uuid>_1`, as the game
    /// stores them: 3D, a slice a layer of cells going up.
    pub fn textures(&self, uuid: u64) -> [Texture; 2] {
        let [nx, ny, nz] = self.cells;
        [0, 1].map(|t| {
            let mut data = Vec::with_capacity(self.values.len() * 4);
            for z in 0..nz {
                for y in 0..ny {
                    for x in 0..nx {
                        let cell = &self.values[(x + nx * (y + ny * z)) as usize];
                        let [r, g, b] = CHANNELS[t].map(|d| cell[d]);
                        data.extend_from_slice(&pack_r11g11b10(r, g, b).to_le_bytes());
                    }
                }
            }
            volume_texture(format!("{uuid}_{t}"), nx as u16, ny as u16, nz as u16, data)
        })
    }
}

/// A 3D `R11G11B10_FLOAT` texture as the game's volumes are.
fn volume_texture(name: String, width: u16, height: u16, depth: u16, data: Vec<u8>) -> Texture {
    const FLAGS: u32 = 0x1800_8002;
    let format = TextureFormat::R11G11B10F;
    let file_format = from_engine_format(format);
    let base = TextureBase {
        name: Some(name),
        format,
        width,
        height,
        depth,
        mip_levels: 1,
        dimension: dimension::TEXTURE_3D,
        tile_mode: tile_mode::STANDARD,
        flags: FLAGS,
        file_format,
        block_stride: block_stride(file_format),
        block_count: block_count(file_format, width, height, depth, 1, FLAGS),
        data: Some(data),
        ..Default::default()
    };
    let srv = ShaderResourceView { dimension: srv_dimension::TEXTURE_3D, ..Default::default() };
    Texture { base, srv, ..Default::default() }
}

/// Three floats in 32 bits: red and green with 11 bits (6 of mantissa, 5 of
/// exponent), blue with 10 (5 and 5), none with a sign.
pub fn pack_r11g11b10(r: f32, g: f32, b: f32) -> u32 {
    small_float(r, 6) | small_float(g, 6) << 11 | small_float(b, 5) << 22
}

/// `v` as a float with no sign, 5 bits of exponent and `bits` of mantissa.
fn small_float(v: f32, bits: u32) -> u32 {
    if v.is_nan() || v <= 0.0 {
        return 0;
    }
    let top = (1 << bits) - 1;
    // The largest one there is (the exponent of all ones being infinity).
    if v >= 65024.0 {
        return 30 << bits | top;
    }
    let raw = v.to_bits();
    let exponent = (raw >> 23) as i32 - 127 + 15;
    let mantissa = raw & 0x7F_FFFF;
    let shift = 23 - bits;
    if exponent <= 0 {
        // Too small for an exponent: counted in steps of 2^-14 / 2^bits.
        let steps = (v * (1u32 << (14 + bits)) as f32).round() as u32;
        return steps.min(1 << bits);
    }
    // Rounded to the nearest; a mantissa that overflows carries into the exponent.
    ((exponent as u32) << bits | mantissa >> shift) + (mantissa >> (shift - 1) & 1)
}

/// The item of a zone file for a baked volume of an interior.
pub fn zone_item(volume: &Volume, uuid: u64) -> Option<DataBag> {
    let mut item = pso_schema().schema.create(ITEM_CLASS)?;
    item.set_bool("enabled", true);
    item.set_vec3("position", volume.centre.into());
    // Only the first number is used: the heading, in degrees, clockwise.
    item.set_vec3("rotation", [-volume.heading.to_degrees(), 0.0, 0.0]);
    item.set_vec3("scale", volume.half.into());
    item.set_vec3("falloffScaleMin", [1.0; 3]);
    item.set_vec3("falloffScaleMax", [1.1; 3]);
    item.set_f32("falloffPower", 1.0);
    item.set_f32("distance", -1.0);
    item.set_i32("cellCountX", volume.cells[0] as i32);
    item.set_i32("cellCountY", volume.cells[1] as i32);
    item.set_i32("cellCountZ", volume.cells[2] as i32);
    for plane in ["clipPlane0", "clipPlane1", "clipPlane2", "clipPlane3"] {
        item.set_vec4(plane, [0.0, 0.0, 0.0, 1.0]);
    }
    for blend in ["clipPlaneBlend0", "clipPlaneBlend1", "clipPlaneBlend2", "clipPlaneBlend3"] {
        item.set_f32(blend, 1.0);
    }
    // BM_Lerp.
    item.set_enum_value("blendingMode", 1);
    item.set_bool("natural", true);
    item.set_bool("interior", true);
    item.set_u64("uuid", uuid);
    Some(item)
}

/// The name of zone `number`'s files, without the extension.
pub fn zone_name(number: u32) -> String {
    format!("amv_zone_{number}")
}

/// A zone file (`amv_zone_<number>.ymt`) with these items.
pub fn zone_file(number: u32, items: Vec<DataBag>) -> rf_formats::Result<Vec<u8>> {
    let mut root = pso_schema()
        .schema
        .create(ZONE_CLASS)
        .ok_or_else(|| rf_formats::Error::Format("no schema for an ambient mask volume zone".into()))?;
    root.set_object("amvList", Some(Value::Bags(items.into_iter().map(Some).collect())));
    root.set_hash("txdName", joaat(&zone_name(number)));
    Ok(PsoFile::from_root(&root)?.write())
}

/// A zone's texture dictionary (`amv_zone_<number>.ytd`).
pub fn zone_textures(textures: Vec<Texture>) -> Vec<u8> {
    let mut dictionary = TextureDictionary::build(textures);
    dictionary.block_map = Some(BlockMap::default());
    let mut w = Rsc8Writer::new();
    dictionary.write(&mut w);
    w.build(2)
}

/// The first zone number after the game's own, which is what a zone made here
/// gets unless told otherwise. The game only loads the numbers that follow its
/// own with none missing (it loads 183, not 212 with nothing in between): two
/// resources on a server take 183 and 184, not the same one.
pub const FIRST_FREE_ZONE: u32 = LAST_GAME_ZONE + 1;

/// The uuid of an interior's `index`th volume: its textures' name.
pub fn uuid_for(interior: u32, index: u32) -> u64 {
    (interior as u64) << 32 | (index as u64 + 1)
}

/// The model files (ydr) among `paths`, by the hash of their name: what an
/// archetype of the same name is drawn with.
pub fn model_files(paths: impl IntoIterator<Item = PathBuf>) -> HashMap<u32, PathBuf> {
    let mut files = HashMap::new();
    for path in paths {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ydr"))
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            files.insert(joaat(&stem.to_ascii_lowercase()), path);
        }
    }
    files
}

/// Whether a model isn't what's seen of the interior: the stand-ins drawn only
/// into shadows or reflections (so flagged, or so named: the shadow ones have
/// no flag of their own), which are closed where the interior is open.
fn is_proxy(path: &Path, flags: u32) -> bool {
    const ONLY_IN_A_PASS: u32 = 0x2A00_0000;
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase();
    flags & ONLY_IN_A_PASS != 0
        || ["shadowproxy", "shadow_proxy", "ref_proxy", "refproxy", "lightbox"].iter().any(|p| name.contains(p))
}

/// Triangles in the world's space, and the models they are of with how many each gave.
pub type Occluders = (Vec<[Vec3; 3]>, Vec<(PathBuf, usize)>);

/// An interior of a ytyp and the entity of a ymap that places it.
#[derive(Clone, Copy)]
pub struct Placed<'a> {
    pub mlo: &'a DataBag,
    pub map: &'a DataBag,
    pub instance: &'a DataBag,
}

impl Placed<'_> {
    /// The hash of the interior's name.
    pub fn name(&self) -> u32 {
        self.mlo.hash("name")
    }

    /// The volume around the whole of it (its first room's box), with cells
    /// about `cell` metres wide.
    pub fn volume(&self, cell: f32) -> Option<Volume> {
        let whole = self.mlo.bags("rooms")?.iter().flatten().next()?;
        let (min, max) = (Vec3::from(whole.vec3("bbMin")), Vec3::from(whole.vec3("bbMax")));
        let placed = Placement::of_entity(self.instance);
        ((max - min).min_element() > 0.0)
            .then(|| Volume::of_interior(placed.position, placed.orientation, min, max, cell))
    }

    /// What blocks the light in and around it, in the world's space: the
    /// models in `files` of its own entities and of the entities its ymap has
    /// around it (the building it is in). With each model used, its triangles.
    pub fn occluders(&self, files: &HashMap<u32, PathBuf>, hashes: &HashIndex) -> Result<Occluders, String> {
        let placed = Placement::of_entity(self.instance);
        let to_world = Mat4::from_rotation_translation(placed.orientation, placed.position);
        let cx = XmlContext { hashes, folder: None };
        let (mut triangles, mut used) = (Vec::new(), Vec::new());
        let inside = lod::entities(self.mlo).iter().flatten().map(|d| (d, to_world));
        let around =
            lod::entities(self.map).iter().flatten().filter(|e| !is_mlo_instance(e)).map(|d| (d, Mat4::IDENTITY));
        for (def, parent) in inside.chain(around) {
            let Some(path) = files.get(&def.hash("archetypeName")) else { continue };
            if is_proxy(path, def.u32("flags")) {
                continue;
            }
            let read = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
            let data = std::fs::read(path).map_err(|e| read(&e))?;
            let drawable = ydr::read(&data, &cx).map_err(|e| read(&e))?;
            let before = triangles.len();
            drawable_triangles(&drawable.base, parent * Placement::of_entity(def).matrix(), &mut triangles);
            used.push((path.clone(), triangles.len() - before));
        }
        Ok((triangles, used))
    }
}

/// The interiors of `ytyps` that `maps` place, each with the first entity
/// that places it.
pub fn placed<'a>(ytyps: &'a [DataBag], maps: &'a [DataBag]) -> Vec<Placed<'a>> {
    let mlos =
        ytyps.iter().flat_map(|y| interior::archetypes(y).iter().flatten()).filter(|a| a.bags("rooms").is_some());
    mlos.filter_map(|mlo| {
        let places = |e: &&DataBag| is_mlo_instance(e) && e.hash("archetypeName") == mlo.hash("name");
        maps.iter().find_map(|map| {
            let instance = lod::entities(map).iter().flatten().find(places)?;
            Some(Placed { mlo, map, instance })
        })
    })
    .collect()
}

/// A zone's two files.
pub struct ZoneFiles {
    pub number: u32,
    /// `amv_zone_<number>.ymt`.
    pub ymt: Vec<u8>,
    /// `amv_zone_<number>.ytd`.
    pub ytd: Vec<u8>,
}

/// What was baked for a project.
#[derive(Default)]
pub struct ProjectBake {
    /// The zone with the volumes, if any was baked.
    pub zone: Option<ZoneFiles>,
    /// How many interiors have a volume in it.
    pub volumes: usize,
    /// Interiors left without one, and why; what else an interior's lighting lacks.
    pub notes: Vec<String>,
}

/// What [`Lighting`] found, in words.
pub fn lighting_notes(name: &str, l: &Lighting) -> Vec<String> {
    let mut notes = Vec::new();
    if !l.no_time_cycle.is_empty() {
        notes.push(format!("{name}: no time cycle (lit as outdoors) in {}", l.no_time_cycle.join(", ")));
    }
    if !l.no_probe.is_empty() {
        notes.push(format!("{name}: no reflection probe (nothing for metals to reflect) in {}", l.no_probe.join(", ")));
    }
    if l.unflagged > 0 {
        notes.push(format!(
            "{name}: {} of {} entities have neither of the flags the game's interior entities have ({})",
            l.unflagged,
            l.entities,
            interior::USUAL_ENTITY_FLAGS
        ));
    }
    notes
}

/// Whether a file is a zone of ambient mask volumes (`amv_zone_<n>.ymt`).
pub fn is_zone_file(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    name.starts_with("amv_zone_") && name.ends_with(".ymt")
}

/// Zone `number` for a project's interiors: a volume for each interior its
/// ytyps have and its ymaps place, baked against its models. Nothing for a project
/// with a zone of its own among its files (it has seen to it), or with no
/// interior.
pub fn bake_project(files: &[PathBuf], number: u32, hashes: &HashIndex) -> ProjectBake {
    if files.iter().any(|f| is_zone_file(f)) {
        return ProjectBake::default();
    }
    let metas = |ext: &str| -> Vec<DataBag> {
        let of_kind = files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)));
        of_kind.filter_map(|f| Meta::read(&std::fs::read(f).ok()?).ok()?.root).collect()
    };
    let (ytyps, maps) = (metas("ytyp"), metas("ymap"));
    let interiors = placed(&ytyps, &maps);
    if interiors.is_empty() {
        return ProjectBake::default();
    }
    let models = model_files(files.iter().cloned());
    let (mut items, mut textures, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    for (index, interior) in interiors.iter().enumerate() {
        let name = hashes.name(interior.name());
        notes.extend(lighting_notes(&name, &interior::lighting(interior.mlo)));
        let Some(volume) = interior.volume(0.5) else {
            notes.push(format!("{name}: no ambient mask volume, its first room has no size"));
            continue;
        };
        let triangles = match interior.occluders(&models, hashes) {
            Ok((triangles, _)) if !triangles.is_empty() => triangles,
            Ok(_) => {
                notes.push(format!("{name}: no ambient mask volume, none of its models is in the project"));
                continue;
            }
            Err(e) => {
                notes.push(format!("{name}: no ambient mask volume ({e})"));
                continue;
            }
        };
        let uuid = uuid_for(interior.name(), index as u32);
        let Some(item) = zone_item(&volume, uuid) else { continue };
        let baked = bake(&volume, &Mesh::new(triangles), &Settings::default());
        items.push(item);
        textures.extend(baked.textures(uuid));
    }
    if items.is_empty() {
        return ProjectBake { notes, ..Default::default() };
    }
    let volumes = items.len();
    match zone_file(number, items) {
        Ok(ymt) => ProjectBake { zone: Some(ZoneFiles { number, ymt, ytd: zone_textures(textures) }), volumes, notes },
        Err(e) => {
            notes.push(format!("no ambient mask volume ({e})"));
            ProjectBake { notes, ..Default::default() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box open at the top, 4 m wide and deep and 3 m high, from the ground.
    fn open_box() -> Mesh {
        let (a, b, h) = (-2.0, 2.0, 3.0);
        let quad = |p: [Vec3; 4]| [[p[0], p[1], p[2]], [p[0], p[2], p[3]]];
        let v = Vec3::new;
        let mut t = Vec::new();
        t.extend(quad([v(a, a, 0.0), v(b, a, 0.0), v(b, b, 0.0), v(a, b, 0.0)]));
        t.extend(quad([v(a, a, 0.0), v(b, a, 0.0), v(b, a, h), v(a, a, h)]));
        t.extend(quad([v(a, b, 0.0), v(b, b, 0.0), v(b, b, h), v(a, b, h)]));
        t.extend(quad([v(a, a, 0.0), v(a, b, 0.0), v(a, b, h), v(a, a, h)]));
        t.extend(quad([v(b, a, 0.0), v(b, b, 0.0), v(b, b, h), v(b, a, h)]));
        Mesh::new(t)
    }

    #[test]
    fn rays_stop_at_walls_and_leave_through_openings() {
        let mesh = open_box();
        let inside = Vec3::new(0.0, 0.0, 1.0);
        assert!(mesh.blocked(inside, Vec3::X, 60.0));
        assert!(mesh.blocked(inside, Vec3::NEG_Z, 60.0));
        assert!(!mesh.blocked(inside, Vec3::Z, 60.0));
        // Not as far as the wall.
        assert!(!mesh.blocked(inside, Vec3::X, 1.5));
    }

    #[test]
    fn a_cell_sees_the_sky_only_where_it_is_open() {
        let volume =
            Volume { centre: Vec3::new(0.0, 0.0, 1.5), half: Vec3::new(1.0, 1.0, 1.0), heading: 0.0, cells: [2; 3] };
        let baked = bake(&volume, &open_box(), &Settings::default());
        assert_eq!(baked.values.len(), 8);
        for cell in &baked.values {
            // Down: the floor. Up: the opening, and the walls around it.
            assert_eq!(cell[5], 0.0);
            assert!(cell[4] > 0.3 && cell[4] < 1.0, "{cell:?}");
            // Sideways: what shows of the opening above the wall, less than upwards.
            assert!(cell[0] > 0.0 && cell[0] < cell[4], "{cell:?}");
        }
        // The upper cells see more of it than the lower ones, upwards and sideways.
        assert!(baked.values[4][4] > baked.values[0][4]);
        assert!(baked.values[4][0] > baked.values[0][0]);
    }

    #[test]
    fn the_volume_of_an_interior_follows_it() {
        let turned = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let v = Volume::of_interior(
            Vec3::new(10.0, 20.0, 5.0),
            turned,
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(4.0, 1.0, 3.0),
            0.5,
        );
        // The box's centre (2, 0, 1.5) a quarter turn on: (0, 2, 1.5).
        assert!(v.centre.abs_diff_eq(Vec3::new(10.0, 22.0, 6.5), 1e-4), "{:?}", v.centre);
        assert!((v.heading - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(v.half, Vec3::new(2.0, 1.0, 1.5));
        assert_eq!(v.cells, [8, 4, 6]);
        let item = zone_item(&v, 7).unwrap();
        assert_eq!(item.vec3("rotation"), [-90.0, 0.0, 0.0]);
        assert_eq!(item.vec3("scale"), [2.0, 1.0, 1.5]);
        assert_eq!(item.u64("uuid"), 7);
    }

    /// The value three packed floats unpack to.
    fn unpack(v: u32, bits: u32) -> f32 {
        let (e, m) = ((v >> bits) & 31, v & ((1 << bits) - 1));
        let m = m as f32 / (1 << bits) as f32;
        if e == 0 { m * 2f32.powi(-14) } else { (1.0 + m) * 2f32.powi(e as i32 - 15) }
    }

    #[test]
    fn small_floats_pack_to_the_nearest() {
        for v in [0.0f32, 1.0, 0.5, 0.25, 0.75, 0.01, 0.3, 0.999, 1e-5, 2.0, 1000.0] {
            let packed = pack_r11g11b10(v, v, v);
            let (r, g, b) = (unpack(packed & 0x7FF, 6), unpack(packed >> 11 & 0x7FF, 6), unpack(packed >> 22, 5));
            assert!((r - v).abs() <= v / 64.0 + 1e-6, "{v}: red {r}");
            assert_eq!(r, g);
            assert!((b - v).abs() <= v / 32.0 + 1e-6, "{v}: blue {b}");
        }
        assert_eq!(pack_r11g11b10(1.0, 0.0, 1.0), 0x3C0 | 0x1E0 << 22);
        assert_eq!(pack_r11g11b10(-1.0, f32::NAN, 0.0), 0);
    }

    #[test]
    fn a_zone_reads_back() {
        let volume = Volume { centre: Vec3::new(1.0, 2.0, 3.0), half: Vec3::ONE, heading: 0.5, cells: [3, 4, 2] };
        let baked = Baked { cells: volume.cells, values: vec![[0.0, 0.1, 0.2, 0.3, 0.4, 0.5]; 24] };
        let file = zone_file(200, vec![zone_item(&volume, 42).unwrap()]).unwrap();
        let hashes = rf_formats::hash::HashIndex::new();
        let (root, _) = PsoFile::read(&file, &hashes).unwrap().root().unwrap().unwrap();
        assert_eq!(root.hash("txdName"), joaat("amv_zone_200"));
        let items = root.bags("amvList").unwrap();
        assert_eq!(items.len(), 1);
        let item = items[0].as_ref().unwrap();
        assert_eq!(item.u64("uuid"), 42);
        assert_eq!(item.i32("cellCountY"), 4);
        assert!(item.bool("interior") && !item.bool("exterior"));

        let [a, b] = baked.textures(42);
        assert_eq!(a.name(), Some("42_0"));
        assert_eq!((a.base.width, a.base.height, a.base.depth), (3, 4, 2));
        assert_eq!(a.base.data.as_ref().unwrap().len(), 24 * 4);
        assert_eq!(b.name(), Some("42_1"));
        let ytd = zone_textures(vec![a, b]);
        assert_eq!(&ytd[..4], b"RSC8");
    }
}
