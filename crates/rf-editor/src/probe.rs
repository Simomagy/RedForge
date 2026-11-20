//! Reflection probes for interiors that have none.
//!
//! What a room's metals and glass reflect is a "deferred probe": a picture of
//! the room from a point in it, taken in the six directions of a cube, that
//! the game lights again as the hour changes. It is three cube textures, in
//! four sizes each (a dictionary a size: `<name>_lo` 128, `<name>` 256,
//! `<name>_hi` 512, `<name>_ul` 1024):
//!
//! - `<name>_0`: the colour of what is seen, with how open to the light it is
//!   in the alpha;
//! - `<name>_1`: which way it faces, with 0 in the alpha where it is a window
//!   (or the sky) and light comes in;
//! - `<name>_d`: how far it is, the sky being 1.
//!
//! `<name>` is a hash of the probe's `guid` and of the interior's name
//! ([`texture_name`]); the probe itself is a `CReflectionProbeData` in a room
//! of the interior.
//!
//! This draws one by casting a ray for every pixel against the interior's
//! models, with their own textures.

use std::collections::HashMap;
use std::path::PathBuf;

use glam::{Mat4, Vec2, Vec3};
use rayon::prelude::*;
use rf_formats::formats::XmlContext;
use rf_formats::formats::databag::{DataBag, Value};
use rf_formats::formats::dds::TextureFormat;
use rf_formats::formats::meta::Meta;
use rf_formats::formats::ydr::{self, DrawableBase, ShaderParamType, ShaderTexture, semantic};
use rf_formats::formats::ytd::{
    ShaderResourceView, Texture, TextureBase, TextureDictionary, block_count, block_stride, dimension,
    from_engine_format, srv_dimension, tile_mode,
};
use rf_formats::hash::HashIndex;
use rf_formats::rsc8::{BlockMap, Rsc8Writer};

use crate::amv::{Mesh, Placed, is_proxy};
use crate::lod;
use crate::placement::{Placement, is_mlo_instance};

/// The sizes a probe comes in, with what its dictionary's name ends in.
pub const SIZES: [(u32, &str); 4] = [(1024, "_ul"), (512, "_hi"), (256, ""), (128, "_lo")];

/// The depth texture holds `1 - DEPTH_NEAR / distance` (the distance along
/// the way the face looks): what a camera with its near plane here and no far
/// plane has. The game's own probes fit it to four figures. The sky is 1.
const DEPTH_NEAR: f32 = 0.01;
/// How far a ray goes before what it would meet counts as the sky.
const REACH: f32 = 300.0;

/// The name of a probe's textures: Bob Jenkins' `hashword` of the two halves
/// of its guid and of the hash of its interior's name.
pub fn texture_name(guid: u64, interior: u32) -> u32 {
    let rot = |x: u32, k: u32| x.rotate_left(k);
    let seed = 0xDEAD_BEEFu32.wrapping_add(12);
    let (mut a, mut b, mut c) =
        (seed.wrapping_add((guid >> 32) as u32), seed.wrapping_add(guid as u32), seed.wrapping_add(interior));
    c ^= b;
    c = c.wrapping_sub(rot(b, 14));
    a ^= c;
    a = a.wrapping_sub(rot(c, 11));
    b ^= a;
    b = b.wrapping_sub(rot(a, 25));
    c ^= b;
    c = c.wrapping_sub(rot(b, 16));
    a ^= c;
    a = a.wrapping_sub(rot(c, 4));
    b ^= a;
    b = b.wrapping_sub(rot(a, 14));
    c ^= b;
    c.wrapping_sub(rot(b, 24))
}

/// A texture's pixels, red green blue alpha.
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
}

impl Image {
    /// The colour at a place in the texture (it repeats outside 0 to 1).
    fn at(&self, uv: Vec2) -> [u8; 4] {
        if self.pixels.is_empty() {
            return [128, 128, 128, 255];
        }
        let wrap = |v: f32, n: usize| ((v - v.floor()) * n as f32) as usize % n;
        self.pixels[wrap(uv.y, self.height) * self.width + wrap(uv.x, self.width)]
    }

    /// The texture's pixels: of the first of its mips no wider or higher than
    /// `most`, which is all a probe shows of it.
    pub fn of_texture(t: &TextureBase, most: usize) -> Option<Self> {
        use TextureFormat as F;
        let data = t.data.as_deref()?;
        let block = |f: F| match f {
            F::BC1 | F::BC1SRGB | F::BC4 => Some(8),
            F::BC2 | F::BC2SRGB | F::BC3 | F::BC3SRGB | F::BC5 | F::BC7 | F::BC7SRGB => Some(16),
            _ => None,
        };
        let bytes = |w: usize, h: usize| match block(t.format) {
            Some(b) => w.div_ceil(4) * h.div_ceil(4) * b,
            None => w * h * 4,
        };
        let (mut w, mut h, mut offset) = (t.width as usize, t.height as usize, 0);
        for _ in 1..t.mip_levels.max(1) {
            if w.max(h) <= most {
                break;
            }
            offset += bytes(w, h);
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
        let data = data.get(offset..offset + bytes(w, h))?;
        let mut out = vec![0u32; w * h];
        let decoded = match t.format {
            F::BC1 | F::BC1SRGB => texture2ddecoder::decode_bc1(data, w, h, &mut out),
            F::BC2 | F::BC2SRGB => texture2ddecoder::decode_bc2(data, w, h, &mut out),
            F::BC3 | F::BC3SRGB => texture2ddecoder::decode_bc3(data, w, h, &mut out),
            F::BC4 => texture2ddecoder::decode_bc4(data, w, h, &mut out),
            F::BC5 => texture2ddecoder::decode_bc5(data, w, h, &mut out),
            F::BC7 | F::BC7SRGB => texture2ddecoder::decode_bc7(data, w, h, &mut out),
            // As the decoders give them: blue in the low byte.
            F::A8R8G8B8 | F::A8R8G8B8SRGB => {
                out.iter_mut()
                    .zip(data.as_chunks::<4>().0)
                    .for_each(|(o, p)| *o = u32::from_le_bytes([p[0], p[1], p[2], p[3]]));
                Ok(())
            }
            F::A8B8G8R8 | F::A8B8G8R8SRGB => {
                out.iter_mut()
                    .zip(data.as_chunks::<4>().0)
                    .for_each(|(o, p)| *o = u32::from_le_bytes([p[2], p[1], p[0], p[3]]));
                Ok(())
            }
            _ => Err("a format a probe doesn't read"),
        };
        decoded.ok()?;
        let pixels = out.into_iter().map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]).collect();
        Some(Self { width: w, height: h, pixels })
    }
}

/// What a triangle is made of.
pub struct Material {
    /// Its colour map, if the model has it in it.
    pub colour: Option<Image>,
    /// Glass: seen through, and where light comes in.
    pub glass: bool,
}

/// A triangle's corners: which way each faces, and its place in the texture.
struct Corners {
    normals: [Vec3; 3],
    uvs: [Vec2; 3],
    material: u32,
}

/// The models of an interior and of the building around it, to be drawn.
pub struct Scene {
    mesh: Mesh,
    /// The triangles' corners, in the order the mesh was made from.
    corners: Vec<Corners>,
    points: Vec<[Vec3; 3]>,
    materials: Vec<Material>,
}

/// The shader parameters that name a model's colour map, as the app's
/// renderer takes them: `diffusetex`, `lyr0diffusetex`.
const COLOUR_MAPS: [u32; 2] = [0xC8E8_C282, 0x3463_94EA];

impl Scene {
    /// The interior's own entities' models and those its ymap has around it,
    /// among `files` (by the hash of their name), in the world's space.
    pub fn of_interior(interior: &Placed, files: &HashMap<u32, PathBuf>, hashes: &HashIndex) -> Result<Self, String> {
        let placed = Placement::of_entity(interior.instance);
        let to_world = Mat4::from_rotation_translation(placed.orientation, placed.position);
        let cx = XmlContext { hashes, folder: None };
        let mut scene =
            Self { mesh: Mesh::new(Vec::new()), corners: Vec::new(), points: Vec::new(), materials: Vec::new() };
        let inside = lod::entities(interior.mlo).iter().flatten().map(|d| (d, to_world));
        let around =
            lod::entities(interior.map).iter().flatten().filter(|e| !is_mlo_instance(e)).map(|d| (d, Mat4::IDENTITY));
        for (def, parent) in inside.chain(around) {
            let Some(path) = files.get(&def.hash("archetypeName")) else { continue };
            if is_proxy(path, def.u32("flags")) {
                continue;
            }
            let read = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
            let data = std::fs::read(path).map_err(|e| read(&e))?;
            let drawable = ydr::read(&data, &cx).map_err(|e| read(&e))?;
            scene.add(&drawable.base, parent * Placement::of_entity(def).matrix());
        }
        scene.mesh = Mesh::new(scene.points.clone());
        Ok(scene)
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// A drawable's triangles (its highest level of detail), through `to`.
    /// Decals are left out: they lie on what is under them.
    fn add(&mut self, d: &DrawableBase, to: Mat4) {
        let Some(lod) = d.lods.iter().flatten().next() else { return };
        let group = d.shader_group.as_ref();
        let shaders = group.and_then(|g| g.shaders.as_deref()).unwrap_or_default();
        let textures = group.and_then(|g| g.texture_dictionary.as_ref()).and_then(|t| t.textures.as_deref());
        // A material a shader of the drawable, made when first met.
        let mut made: HashMap<u16, u32> = HashMap::new();
        let turn = to.inverse().transpose();
        for g in lod.models().flat_map(|m| m.geometries()) {
            let name = g.name.as_deref().unwrap_or_default();
            if name.contains("decal") {
                continue;
            }
            let Some(indices) = g.indices() else { continue };
            let element = |semantic: u8| (0..g.layout.element_count()).find(|e| g.layout.semantic(*e) == semantic);
            let Some(position) = element(semantic::POSITION) else { continue };
            let (normal, uv) = (element(semantic::NORMAL), element(semantic::TEXCOORD));
            let material = *made.entry(g.shader_id).or_insert_with(|| {
                let params =
                    shaders.get(g.shader_id as usize).and_then(|s| s.as_ref()).and_then(|s| s.parameters.as_ref());
                let maps = params.and_then(|p| p.params.as_deref()).unwrap_or_default().iter();
                let mut maps = maps.filter(|p| p.info.param_type() == ShaderParamType::Texture);
                let colour =
                    maps.find(|p| COLOUR_MAPS.contains(&p.info.name)).and_then(|p| match p.texture.as_ref()? {
                        ShaderTexture::Embedded(i) => Image::of_texture(&textures?.get(*i)?.as_ref()?.base, 512),
                        ShaderTexture::External(t) => Image::of_texture(t, 512),
                    });
                self.materials.push(Material { colour, glass: name.contains("glass") });
                self.materials.len() as u32 - 1
            });
            let value = |v: usize, e: Option<usize>| e.and_then(|e| g.vertex_value(v, e).ok()).unwrap_or_default();
            let vertices: Vec<(Vec3, Vec3, Vec2)> = (0..g.vertex_count())
                .map(|v| {
                    let (p, n, t) = (value(v, Some(position)), value(v, normal), value(v, uv));
                    let facing = turn.transform_vector3(Vec3::new(n[0], n[1], n[2])).normalize_or_zero();
                    (to.transform_point3(Vec3::new(p[0], p[1], p[2])), facing, Vec2::new(t[0], t[1]))
                })
                .collect();
            for t in indices.as_chunks::<3>().0 {
                let Some(v) = t.iter().map(|i| vertices.get(*i as usize)).collect::<Option<Vec<_>>>() else { continue };
                self.points.push([v[0].0, v[1].0, v[2].0]);
                self.corners.push(Corners {
                    normals: [v[0].1, v[1].1, v[2].1],
                    uvs: [v[0].2, v[1].2, v[2].2],
                    material,
                });
            }
        }
    }

    /// The triangles alone, for what only needs to know where they are.
    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }
}

/// What a pixel of a probe shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// The colour of what is seen, as its texture has it.
    pub colour: [u8; 3],
    /// How open to the light it is, 0 to 1.
    pub open: f32,
    /// Which way it faces, in the world.
    pub normal: Vec3,
    /// Whether it is solid: not a window, and not the sky.
    pub solid: bool,
    /// How far it is along the ray; `None` for the sky.
    pub distance: Option<f32>,
}

/// What is seen from `origin` along `dir`.
pub fn look(scene: &Scene, origin: Vec3, dir: Vec3, reach: f32) -> Sample {
    let (mut from, mut gone, mut window) = (origin, 0.0, false);
    // Through as many panes of glass as there are.
    for _ in 0..8 {
        let Some((d, id)) = scene.mesh.nearest(from, dir, reach - gone) else { break };
        let corners = &scene.corners[id as usize];
        let material = &scene.materials[corners.material as usize];
        gone += d;
        let at = from + dir * d;
        if material.glass {
            window = true;
            from = at + dir * 1e-3;
            continue;
        }
        // Where in the triangle: the weights of its corners.
        let [a, b, c] = scene.points[id as usize];
        let (ab, ac, ap) = (b - a, c - a, at - a);
        let (d00, d01, d11, d20, d21) = (ab.dot(ab), ab.dot(ac), ac.dot(ac), ap.dot(ab), ap.dot(ac));
        let det = d00 * d11 - d01 * d01;
        let (v, w) =
            if det.abs() > 1e-12 { ((d11 * d20 - d01 * d21) / det, (d00 * d21 - d01 * d20) / det) } else { (0.0, 0.0) };
        let weights = [1.0 - v - w, v, w];
        let blend = |i: usize| weights[i];
        let uv = corners.uvs[0] * blend(0) + corners.uvs[1] * blend(1) + corners.uvs[2] * blend(2);
        let flat = ab.cross(ac).normalize_or_zero();
        let smooth = corners.normals[0] * blend(0) + corners.normals[1] * blend(1) + corners.normals[2] * blend(2);
        let normal = if smooth.length_squared() > 1e-6 { smooth.normalize() } else { flat };
        // Towards the one looking, whichever way the triangle was wound.
        let normal = if normal.dot(dir) > 0.0 { -normal } else { normal };
        let colour = material.colour.as_ref().map_or([128, 128, 128, 255], |c| c.at(uv));
        return Sample {
            colour: [colour[0], colour[1], colour[2]],
            open: openness(scene, at + normal * 0.02, normal),
            normal,
            solid: !window,
            distance: Some(gone),
        };
    }
    Sample { colour: [255; 3], open: 1.0, normal: Vec3::Z, solid: false, distance: None }
}

/// How open a point of a surface is: the part of a few rays around the way it
/// faces that meet nothing within a few metres. (Turned differently at every
/// point, so that the rays' pattern doesn't show as shapes on the walls.)
fn openness(scene: &Scene, at: Vec3, normal: Vec3) -> f32 {
    const RAYS: u32 = 16;
    const NEAR: f32 = 6.0;
    let (side, up) = normal.any_orthonormal_pair();
    let seed = (at.x * 12.9898 + at.y * 78.233 + at.z * 37.719).sin() * 43_758.547;
    let turn = (seed - seed.floor()) * std::f32::consts::TAU;
    let open = (0..RAYS)
        .filter(|&k| {
            let (u, angle) = ((k as f32 + 0.5) / RAYS as f32, k as f32 * 2.399_963 + turn);
            let r = u.sqrt();
            let dir = side * (r * angle.cos()) + up * (r * angle.sin()) + normal * (1.0 - u).sqrt();
            !scene.mesh.blocked(at, dir, NEAR)
        })
        .count();
    // Never quite shut: the game's own are about a quarter open in a small room.
    0.15 + 0.85 * open as f32 / RAYS as f32
}

/// The six faces of a cube texture: for each, the way it looks, then what is
/// to the right and what is up in its picture. As the game's own probes have
/// them (found by drawing the probe of Tumbleweed's saloon and laying it over
/// the game's): those of any cube texture (+X, -X, +Y, -Y, +Z, -Z, with Y as
/// its up), given the world's own axes.
pub const FACES: [[Vec3; 3]; 6] = [
    [Vec3::X, Vec3::NEG_Z, Vec3::Y],
    [Vec3::NEG_X, Vec3::Z, Vec3::Y],
    [Vec3::Y, Vec3::X, Vec3::NEG_Z],
    [Vec3::NEG_Y, Vec3::X, Vec3::Z],
    [Vec3::Z, Vec3::X, Vec3::Y],
    [Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y],
];

/// A probe drawn: its six faces, each `size` pixels wide and high, a row at
/// a time from the top.
pub struct Drawn {
    pub size: u32,
    pub faces: Vec<Vec<Sample>>,
}

/// Draws the probe at `origin`, `size` pixels a side.
pub fn draw(scene: &Scene, origin: Vec3, size: u32) -> Drawn {
    let faces = FACES
        .iter()
        .map(|[forward, right, up]| {
            (0..size * size)
                .into_par_iter()
                .map(|i| {
                    let at = |p: u32| (p as f32 + 0.5) / size as f32 * 2.0 - 1.0;
                    let dir = (*forward + *right * at(i % size) - *up * at(i / size)).normalize();
                    look(scene, origin, dir, REACH)
                })
                .collect()
        })
        .collect();
    Drawn { size, faces }
}

impl Drawn {
    /// Half the size: each pixel the four it was.
    pub fn halved(&self) -> Self {
        let (n, half) = (self.size as usize, self.size as usize / 2);
        let faces = self
            .faces
            .iter()
            .map(|face| {
                (0..half * half)
                    .map(|i| {
                        let (x, y) = (i % half * 2, i / half * 2);
                        let four =
                            [face[y * n + x], face[y * n + x + 1], face[(y + 1) * n + x], face[(y + 1) * n + x + 1]];
                        let mean = |f: &dyn Fn(&Sample) -> f32| four.iter().map(f).sum::<f32>() / 4.0;
                        let near: Vec<f32> = four.iter().filter_map(|s| s.distance).collect();
                        Sample {
                            colour: [0, 1, 2].map(|c| mean(&|s| s.colour[c] as f32).round() as u8),
                            open: mean(&|s| s.open),
                            normal: four.iter().map(|s| s.normal).sum::<Vec3>().normalize_or(Vec3::Z),
                            // A window if most of it is; the sky if all of it is.
                            solid: four.iter().filter(|s| s.solid).count() >= 2,
                            distance: (!near.is_empty()).then(|| near.iter().sum::<f32>() / near.len() as f32),
                        }
                    })
                    .collect()
            })
            .collect();
        Self { size: half as u32, faces }
    }

    /// The three textures, named after `name`: colour, facing and depth.
    pub fn textures(&self, name: u32) -> [Texture; 3] {
        let n = self.size as usize;
        let (mut colour, mut facing, mut depth) = (Vec::new(), Vec::new(), Vec::new());
        for (face, [forward, right, up]) in self.faces.iter().zip(&FACES) {
            let at = |p: usize| (p as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let rgba = |f: &dyn Fn(&Sample) -> [u8; 4]| face.iter().map(f).collect::<Vec<_>>();
            let unit = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            colour.extend(bc3(n, &rgba(&|s| [s.colour[0], s.colour[1], s.colour[2], unit(s.open)])));
            let way = |s: &Sample| (s.normal * 0.5 + 0.5).to_array().map(unit);
            facing.extend(bc3(n, &rgba(&|s| [way(s)[0], way(s)[1], way(s)[2], if s.solid { 255 } else { 0 }])));
            for (i, s) in face.iter().enumerate() {
                // How far along the way the face looks, not along the ray.
                let dir = (*forward + *right * at(i % n) - *up * at(i / n)).normalize();
                let value = match s.distance {
                    None => u16::MAX,
                    Some(d) => ((1.0 - DEPTH_NEAR / (d * dir.dot(*forward)).max(DEPTH_NEAR)) * 65534.0) as u16,
                };
                depth.extend_from_slice(&value.to_le_bytes());
            }
        }
        [
            cube_texture(format!("{name}_0"), TextureFormat::BC3SRGB, self.size as u16, colour),
            cube_texture(format!("{name}_1"), TextureFormat::BC3, self.size as u16, facing),
            cube_texture(format!("{name}_d"), TextureFormat::R16U, self.size as u16, depth),
        ]
    }
}

/// A cube texture as the game's probes are: six faces, no smaller copies.
fn cube_texture(name: String, format: TextureFormat, size: u16, data: Vec<u8>) -> Texture {
    const FLAGS: u32 = 0x1800_8002;
    let file_format = from_engine_format(format);
    let base = TextureBase {
        name: Some(name),
        format,
        width: size,
        height: size,
        depth: 6,
        mip_levels: 1,
        dimension: dimension::TEXTURE_CUBE,
        tile_mode: tile_mode::STANDARD,
        flags: FLAGS,
        file_format,
        block_stride: block_stride(file_format),
        block_count: block_count(file_format, size, size, 6, 1, FLAGS),
        data: Some(data),
        ..Default::default()
    };
    let srv = ShaderResourceView { dimension: srv_dimension::TEXTURE_CUBE, ..Default::default() };
    Texture { base, srv, ..Default::default() }
}

/// A dictionary of a probe's three textures.
pub fn dictionary(textures: [Texture; 3]) -> Vec<u8> {
    let mut dictionary = TextureDictionary::build(textures.into());
    dictionary.block_map = Some(BlockMap::default());
    let mut w = Rsc8Writer::new();
    dictionary.write(&mut w);
    w.build(2)
}

/// A picture `n` pixels a side packed as BC3: in blocks of four by four
/// pixels, each with two colours and two alphas and, for every pixel, which
/// of the steps between them it is nearest.
pub fn bc3(n: usize, pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(n * n);
    for (by, bx) in (0..n / 4).flat_map(|y| (0..n / 4).map(move |x| (y, x))) {
        let block: [[u8; 4]; 16] = std::array::from_fn(|i| pixels[(by * 4 + i / 4) * n + bx * 4 + i % 4]);
        // Alpha: the lowest and the highest, and six steps between.
        let (lo, hi) = block.iter().fold((255u8, 0u8), |(lo, hi), p| (lo.min(p[3]), hi.max(p[3])));
        let steps: [u32; 8] = std::array::from_fn(|i| match i {
            0 => hi as u32,
            1 => lo as u32,
            i => ((8 - i as u32) * hi as u32 + (i as u32 - 1) * lo as u32) / 7,
        });
        let mut bits = 0u64;
        for (i, p) in block.iter().enumerate() {
            let nearest = (0..8).min_by_key(|s| steps[*s].abs_diff(p[3] as u32)).unwrap_or(0);
            bits |= (if hi == lo { 0 } else { nearest as u64 }) << (3 * i);
        }
        out.extend_from_slice(&[hi, lo]);
        out.extend_from_slice(&bits.to_le_bytes()[..6]);

        // Colour: the two ends of the box the block's colours are in, along its longest side's diagonal.
        let (mut min, mut max) = ([255u8; 3], [0u8; 3]);
        for p in &block {
            for c in 0..3 {
                min[c] = min[c].min(p[c]);
                max[c] = max[c].max(p[c]);
            }
        }
        let pack = |c: [u8; 3]| ((c[0] as u16 >> 3) << 11) | ((c[1] as u16 >> 2) << 5) | (c[2] as u16 >> 3);
        let unpack = |v: u16| {
            let (r, g, b) = ((v >> 11) as u32, (v >> 5 & 63) as u32, (v & 31) as u32);
            [(r << 3 | r >> 2) as i32, (g << 2 | g >> 4) as i32, (b << 3 | b >> 2) as i32]
        };
        let (mut c0, mut c1) = (pack(max), pack(min));
        if c0 < c1 {
            std::mem::swap(&mut c0, &mut c1);
        }
        let (a, b) = (unpack(c0), unpack(c1));
        let palette: [[i32; 3]; 4] =
            [a, b, std::array::from_fn(|c| (2 * a[c] + b[c]) / 3), std::array::from_fn(|c| (a[c] + 2 * b[c]) / 3)];
        let mut bits = 0u32;
        for (i, p) in block.iter().enumerate() {
            let far = |q: &[i32; 3]| (0..3).map(|c| (q[c] - p[c] as i32).pow(2)).sum::<i32>();
            let nearest = (0..4).min_by_key(|s| far(&palette[*s])).unwrap_or(0);
            bits |= (if c0 == c1 { 0 } else { nearest as u32 }) << (2 * i);
        }
        out.extend_from_slice(&c0.to_le_bytes());
        out.extend_from_slice(&c1.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
    }
    out
}

/// A room's probe, as a `CReflectionProbeData`: a box in the interior's
/// space (`min` to `max`), drawn from `offset` away from its middle.
pub fn probe_data(min: Vec3, max: Vec3, offset: Vec3, guid: u64) -> Option<DataBag> {
    let mut probe = Meta::create("CReflectionProbeData")?;
    probe.set_vec3("minExtents", min.into());
    probe.set_vec3("maxExtents", max.into());
    probe.set_vec4("rotation", [0.0, 0.0, 0.0, 1.0]);
    probe.set_vec3("centerOffset", offset.into());
    probe.set_vec3("influenceExtents", [1.0; 3]);
    probe.set_u8("probePriority", 255);
    probe.set_u64("guid", guid);
    Some(probe)
}

/// Puts `probe` in the interior's first room after the one that is the whole
/// of it (or in that one, if it is the only one).
pub fn add_to_room(mlo: &mut DataBag, probe: DataBag) -> bool {
    let Some(rooms) = mlo.bags_mut("rooms") else { return false };
    let index = if rooms.len() > 1 { 1 } else { 0 };
    let Some(room) = rooms.get_mut(index).and_then(|r| r.as_mut()) else { return false };
    room.set_object("reflectionProbes", Some(Value::Bags(vec![Some(probe)])));
    true
}

/// A probe made for an interior: what goes in its room, and its textures'
/// four dictionaries, by file name.
pub struct Baked {
    pub data: DataBag,
    pub dictionaries: Vec<(String, Vec<u8>)>,
}

/// Whether an interior has a probe in any of its rooms.
pub fn has_probe(mlo: &DataBag) -> bool {
    mlo.bags("rooms")
        .unwrap_or_default()
        .iter()
        .flatten()
        .any(|r| r.bags("reflectionProbes").is_some_and(|p| !p.is_empty()))
}

/// A probe for the whole of an interior (its first room's box), drawn from
/// the middle of it at the height of the eyes, or from the nearest place to
/// that which is indoors. `None` with no model of it among `files`.
pub fn bake_interior(
    interior: &Placed,
    files: &HashMap<u32, PathBuf>,
    hashes: &HashIndex,
) -> Result<Option<Baked>, String> {
    let Some(whole) = interior.mlo.bags("rooms").and_then(|r| r.iter().flatten().next()) else { return Ok(None) };
    let (min, max) = (Vec3::from(whole.vec3("bbMin")), Vec3::from(whole.vec3("bbMax")));
    if (max - min).min_element() <= 0.0 {
        return Ok(None);
    }
    let scene = Scene::of_interior(interior, files, hashes)?;
    if scene.is_empty() {
        return Ok(None);
    }
    let placed = Placement::of_entity(interior.instance);
    let to_world = |p: Vec3| placed.position + placed.orientation * p;
    let middle = (min + max) * 0.5;
    let eyes = Vec3::new(middle.x, middle.y, (min.z + 1.6).min(middle.z.max(min.z + 0.5 * (max.z - min.z).min(3.2))));
    // The places to try: a grid of the box, nearest the eyes first.
    let steps = |lo: f32, hi: f32| {
        (0..=((hi - lo) / 0.5) as u32).map(move |i| lo + 0.25 + i as f32 * 0.5).filter(move |v| *v < hi)
    };
    let mut places: Vec<Vec3> = steps(min.x, max.x)
        .flat_map(|x| steps(min.y, max.y).flat_map(move |y| steps(min.z, max.z).map(move |z| Vec3::new(x, y, z))))
        .collect();
    places.sort_by(|a, b| a.distance_squared(eyes).total_cmp(&b.distance_squared(eyes)));
    let from = std::iter::once(eyes)
        .chain(places)
        .find(|p| crate::amv::indoors(scene.mesh(), to_world(*p), REACH))
        .unwrap_or(eyes);

    let guid = crate::amv::uuid_for(interior.name(), 0);
    let name = texture_name(guid, interior.name());
    let Some(data) = probe_data(min, max, from - middle, guid) else { return Ok(None) };
    let mut drawn = draw(&scene, to_world(from), SIZES[0].0);
    let mut dictionaries = Vec::new();
    for (size, suffix) in SIZES {
        while drawn.size > size {
            drawn = drawn.halved();
        }
        dictionaries.push((format!("{name}{suffix}.ytd"), dictionary(drawn.textures(name))));
    }
    Ok(Some(Baked { data, dictionaries }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_the_one_the_game_looks_for() {
        // Tumbleweed's saloon and Benedict Point's shack: their probes' textures are in the game's files.
        assert_eq!(texture_name(0x4FB7_D4C2_5891_51A2, rf_formats::hash::joaat("tum_saloon_int")), 3_148_245_906);
        assert_eq!(texture_name(0x45EB_CCCE_0E6C_3645, rf_formats::hash::joaat("ben_shack_int")), 899_458_108);
    }

    #[test]
    fn bc3_keeps_flat_colours_and_alphas() {
        let pixels = vec![[200, 100, 48, 77]; 64];
        let packed = bc3(8, &pixels);
        assert_eq!(packed.len(), 4 * 16);
        let mut out = vec![0u32; 64];
        texture2ddecoder::decode_bc3(&packed, 8, 8, &mut out).unwrap();
        for p in out {
            let [b, g, r, a] = p.to_le_bytes();
            assert_eq!(a, 77);
            assert!(r.abs_diff(200) <= 8 && g.abs_diff(100) <= 4 && b.abs_diff(48) <= 8, "{r} {g} {b}");
        }
        // Two halves: each keeps its own.
        let halves: Vec<[u8; 4]> =
            (0..16).map(|i| if i % 4 < 2 { [255, 255, 255, 255] } else { [0, 0, 0, 0] }).collect();
        let mut out = vec![0u32; 16];
        texture2ddecoder::decode_bc3(&bc3(4, &halves), 4, 4, &mut out).unwrap();
        assert_eq!(out[0], 0xFFFF_FFFF);
        assert_eq!(out[3], 0);
    }
}
