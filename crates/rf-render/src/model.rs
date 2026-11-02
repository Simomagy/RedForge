//! Drawables on the GPU: their geometries and materials, by LOD.

use std::sync::Arc;

use rf_formats::formats::ydr::{DrawableBase, Shader, ShaderParamType, ShaderTexture};
use rf_formats::formats::ytd::{TextureBase, TextureDictionary};
use rf_formats::hash::joaat;

use crate::GpuContext;
use crate::mesh::GpuMesh;
use crate::renderer::{GpuMaterial, Renderer};
use crate::texture::GpuTexture;

/// How a draw blends, from the shader's draw bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AlphaMode {
    #[default]
    Opaque,
    /// Alpha tested (foliage, fences): drawn with the opaque things.
    Cutout,
    /// Blended, after the opaque things (glass).
    Blend,
    /// Blended onto surfaces, without writing depth.
    Decal,
}

impl AlphaMode {
    pub fn from_draw_bucket(bucket: u8) -> Self {
        match bucket & 0x7F {
            1 => Self::Blend,
            2 => Self::Decal,
            3 => Self::Cutout,
            _ => Self::Opaque,
        }
    }
}

/// How a decal blends onto what's under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DecalMode {
    /// Not a decal.
    #[default]
    None,
    /// Opacity from the diffuse alpha and the vertex alpha.
    Decal,
    /// Colour from the vertex colour; opacity from the diffuse map's channels
    /// picked by the masks ("decal_dirt").
    Dirt,
}

/// One texture slot of a material, by its use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Diffuse,
    Normal,
    Material,
    /// The second layer of two-layer shaders.
    Diffuse2,
    Normal2,
    Material2,
    /// Colours picked by the vertex colour's blue and the entity's tint.
    TintPalette,
}

impl Slot {
    pub const ALL: [Slot; 7] = [
        Slot::Diffuse,
        Slot::Normal,
        Slot::Material,
        Slot::Diffuse2,
        Slot::Normal2,
        Slot::Material2,
        Slot::TintPalette,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Slot::Diffuse => "Diffuse",
            Slot::Normal => "Normal",
            Slot::Material => "Material",
            Slot::Diffuse2 => "Diffuse (layer 2)",
            Slot::Normal2 => "Normal (layer 2)",
            Slot::Material2 => "Material (layer 2)",
            Slot::TintPalette => "Tint palette",
        }
    }
}

/// The textures and constants a shader's material uses.
#[derive(Default)]
pub struct MaterialDesc<'a> {
    pub shader_name: String,
    /// By [`Slot`].
    pub textures: [Option<&'a ShaderTexture>; 7],
    pub bumpiness: f32,
    pub alpha: AlphaMode,
    pub decal: DecalMode,
    /// How much of a decal reaches the albedo, normals, material and lighting.
    pub decal_masks: [f32; 4],
    /// For two-layer shaders: the channel of the second vertex colour that
    /// blends the layers.
    pub two_layer: Option<u8>,
}

mod names {
    pub const BUMPINESS: u32 = 0xf6712b81;
    pub const DECAL_OPACITY: [u32; 4] = [0x0bc0cd58, 0x9db47191, 0x2be58df5, 0x39172858]; // decalblendopacitygbuffer0..3
    pub const DIRT_DECAL_MASK: u32 = 0x3e95fa90;

    pub const TWO_LAYER: &[u32] = &[
        0x77d8ab27, // standard_2lyr
        0xb217b263, // standard_2lyr_pxm
        0x1251cae5, // standard_2lyr_pxm_ground
        0xcd8ceb5e, // standard_2lyr_ground
        0xed544c47, // standard_2lyr_procedural
        0x3f92f785, // standard_2lyr_tnt
        0x3bbd0e8c, // standard_2lyr_clothmorph
        0x570fcace, // standard_vehicle_wheel_2lyr
        0x4dc0ac6a, // standard_vehicle_2lyr_alpha
        0x4aa38282, // standard_vehicle_2lyr
        0x3f5f96e3, // standard_vehicle_2lyr_clothmorph
        0x469ebe36, // standard_vehicle_2lyr_decal
        0x81fedbdc, // standard_weapon_2lyr
        0x0b307131, // standard_weapon_2lyr_cutout
    ];
    /// Two-layer shaders blended by the vertex alpha.
    pub const DIRT: &[u32] = &[
        0xa477380d, // standard_dirt_ground
        0x18ac9863, // standard_dirt_alpha
        0xe7aeb297, // standard_dirt_pxm_ground
        0x512937b5, // standard_dirt
        0xaf83d7c6, // standard_dirt_pxm
    ];
    pub const DECAL: &[u32] = &[
        0xf08729d4, // decal
        0x14e49f72, // decal_tnt
        0x1510ebe7, // normal_decal
        0x938e49f1, // normal_decal_tnt
        0x1c1c2570, // normal_spec_decal
        0xa4f28a79, // spec_decal
        0xe0b4c573, // standard_decal
        0xd575813a, // standard_decal_tnt
        0x037206ee, // standard_decal_hbb
        0xd41ddcc3, // standard_decal_blend
        0xec8e6107, // standard_decal_blend_ground
        0x5ee453cc, // standard_decal_ground
        0x0eda3a8b, // standard_decal_heightmap
        0xea9cce3a, // standard_pxm_decal
        0x70746df0, // standard_pxm_decal_ground
        0x35186d39, // standard_vehicle_decal
        0x21609928, // trunk_nodpm_decal_blend
        0x25fc3d8e, // trunk_nodpm_decal_tnt
        0x6a36eb0a, // terrain_decal
        0xf95f3edc, // campfire_decal
        0xbd67c331, // campfire_standard_decal
    ];
    pub const DECAL_NORMAL_ONLY: u32 = 0x5e927617; // standard_decal_normal_only
    pub const DECAL_DIRT: u32 = 0xd94d6305; // decal_dirt
}

/// Textures the C# RedForge doesn't map onto its default shader.
const UNUSED_TEXTURES: &[u32] = &[
    0x3c4af3d4, 0xfa0446c7, 0xffb38cd5, 0x8eb4938d, 0x739e3d0d, 0xc7f84d36, // material B, speculartex2
    0x399a985f, // detailtexture
    0x97eaa692, 0x38ebcdb3, 0xf7864a2a, 0x63031fb5, 0xe4ce95ac, 0x10306a3f, 0x5500718e, 0xb44cade5, 0x04b87bd6,
    0xcc155f4b, 0x18346e42, 0xc8a5eaa5, 0xbfdcc9b6, 0x1d5c320b, 0x7f794826, 0x7d0d22b5, 0x8fc59620, 0x85261019,
    0x87c3e3a9, 0x0cd27763, 0x1888e777, 0xf3b6cac3, 0x51b156a7, 0x0d43cdcd, 0x97071119, 0xd5b8a0fd, 0xe038a645,
    0xe9f32960, 0x56d855c5, 0x4d50db90, 0x73a205af, 0x1e3f6af2, 0xa3f1be65, 0xe17a6bff, 0x9dacf178, 0x0ac1ba81,
    0xbdeafe25, 0xce035f37, 0xd15eebcd, 0x4ab802cd, 0xf855de4e, 0x01f8f194, 0xf0b67341, 0xcce63921, 0x1ab3f264,
    0x72e4f4b6, 0xadd19fd3, 0xe992eea9, 0x9878de34, 0xcea74024, 0x4f7b90df, 0xb98434a7, 0xefb6e324, 0x7b0b2e84,
    0x07538bfc, 0xc3adf7ff, 0xc7ec4e28, 0xbf67f3e8, 0x81e387cc, 0x9143179f, 0xa2ed3af3, 0xd2b459c8, 0x03884d2f,
    0xf72bcd78, 0x78dd395b, 0xb095a74d, 0x7c0055b9, 0x6239614e, 0x2a617f90, 0xe32da2c8, 0x6148ffd2, 0x0814a7fc,
    0x3cdb799d, 0x642529e2, 0x8d3bca95, 0x1f009b95, 0x30e2ee87, 0xeaed8eb0, 0x0b957ed6, 0xb9f4bdac, 0x640111c6,
    0xd877fab2, 0x7b21acbf, 0x49d08baa, 0xdf186a0e,
];

impl<'a> MaterialDesc<'a> {
    /// Picks the textures and constants the way the C# RedForge maps RDR2
    /// shaders onto its default shader (`Rsc8Drawable.SetupDefaultShader`).
    pub fn from_shader(shader: &'a Shader, shader_name: String) -> Self {
        let name = shader.name_hash;
        let two_layer = if names::DIRT.contains(&name) {
            Some(3)
        } else if names::TWO_LAYER.contains(&name) {
            Some(0)
        } else {
            None
        };
        let mut d = MaterialDesc {
            shader_name,
            bumpiness: 1.0,
            alpha: AlphaMode::from_draw_bucket(shader.draw_bucket),
            decal_masks: [1.0; 4],
            two_layer,
            ..Default::default()
        };
        let layered = two_layer.is_some();
        let params = shader.parameters.as_ref().and_then(|p| p.params.as_deref()).unwrap_or(&[]);
        let mut t: [Option<&ShaderTexture>; 7] = [None; 7];
        let (diffuse, normal, material) = (Slot::Diffuse as usize, Slot::Normal as usize, Slot::Material as usize);
        let (diffuse2, normal2, material2) =
            (Slot::Diffuse2 as usize, Slot::Normal2 as usize, Slot::Material2 as usize);
        for p in params {
            match p.info.param_type() {
                ShaderParamType::Texture => {
                    let Some(tex) = p.texture.as_ref() else { continue };
                    match p.info.name {
                        0xc8e8c282 | 0x346394ea => t[diffuse] = Some(tex), // diffusetex, lyr0diffusetex
                        0x7485dafb => {
                            // dirtdiffusetex
                            if layered && t[diffuse2].is_none() {
                                t[diffuse2] = Some(tex);
                            } else if t[diffuse].is_none() {
                                t[diffuse] = Some(tex);
                            }
                        }
                        0x0df47048 | 0x7d4f12e2 => {
                            // diffusetex2, lyr1diffusetex
                            if layered {
                                t[diffuse2] = Some(tex);
                            } else if t[diffuse].is_none() {
                                t[diffuse] = Some(tex);
                            }
                        }
                        0x1e77bf4c | 0xd7c38241 | 0x84c69350 => {
                            // bumptex, bumptexture, lyr0normaltex
                            t[normal] = Some(tex);
                            if layered && t[normal2].is_none() {
                                t[normal2] = Some(tex);
                            }
                        }
                        0x5462abc0 | 0x58b6e707 => {
                            // bumptexture2, lyr1normaltex
                            if layered {
                                t[normal2] = Some(tex);
                            } else if t[normal].is_none() {
                                t[normal] = Some(tex);
                            }
                        }
                        0xc6f46e79 | 0x9e3722e4 | 0x2bb2ede8 => {
                            // speculartex, speculartexture, lyr0materialatex
                            t[material] = Some(tex);
                            if layered && t[material2].is_none() {
                                t[material2] = Some(tex);
                            }
                        }
                        0x4d56d7fe => {
                            // lyr1materialatex
                            if layered {
                                t[material2] = Some(tex);
                            } else if t[material].is_none() {
                                t[material] = Some(tex);
                            }
                        }
                        0x2c8d7182 => t[Slot::TintPalette as usize] = Some(tex), // tintpalettetex
                        h if UNUSED_TEXTURES.contains(&h) => {}
                        _ => {
                            if t[diffuse].is_none() {
                                t[diffuse] = Some(tex);
                            }
                        }
                    }
                }
                ShaderParamType::CBuffer => match p.info.name {
                    names::BUMPINESS => d.bumpiness = p.vector[0],
                    names::DIRT_DECAL_MASK => d.decal_masks = p.vector,
                    h => {
                        if let Some(i) = names::DECAL_OPACITY.iter().position(|&o| o == h) {
                            d.decal_masks[i] = p.vector[0];
                        }
                    }
                },
                _ => {}
            }
        }
        d.textures = t;
        if names::DECAL.contains(&name) {
            d.decal = DecalMode::Decal;
        } else if name == names::DECAL_NORMAL_ONLY {
            d.decal = DecalMode::Decal;
            d.decal_masks = [0.0, 1.0, 0.0, 0.0];
        } else if name == names::DECAL_DIRT {
            d.decal = DecalMode::Dirt;
        }
        d
    }

    pub fn texture(&self, slot: Slot) -> Option<&'a ShaderTexture> {
        self.textures[slot as usize]
    }
}

/// Finds a texture by name for a model: the app looks in the model's own and
/// related texture dictionaries.
pub trait TextureSource {
    fn texture(&mut self, name: &str) -> Option<Arc<GpuTexture>>;
}

/// No textures beyond a drawable's own.
pub struct NoTextures;

impl TextureSource for NoTextures {
    fn texture(&mut self, _name: &str) -> Option<Arc<GpuTexture>> {
        None
    }
}

pub struct GpuDraw {
    pub mesh: GpuMesh,
    pub material: Arc<GpuMaterial>,
    pub alpha: AlphaMode,
}

pub struct GpuModel {
    pub name: String,
    /// The draws of each LOD (high, medium, low, very low).
    pub lods: [Vec<GpuDraw>; 4],
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Up to which distance each LOD is drawn (the drawable's `LodDistHigh`...).
    pub lod_dists: [f32; 4],
    /// Texture names that couldn't be found.
    pub missing_textures: Vec<String>,
}

impl GpuModel {
    pub fn highest_lod(&self) -> usize {
        self.lods.iter().position(|l| !l.is_empty()).unwrap_or(0)
    }

    /// The LOD to draw at a distance: the first that reaches it, or the
    /// lowest there is.
    pub fn lod_for_distance(&self, distance: f32) -> usize {
        let mut last = self.highest_lod();
        for i in 0..4 {
            if self.lods[i].is_empty() {
                continue;
            }
            last = i;
            if distance <= self.lod_dists[i] {
                return i;
            }
        }
        last
    }

    pub fn draw_count(&self) -> usize {
        self.lods.iter().map(Vec::len).sum()
    }
}

struct Resolver<'a, 's> {
    ctx: &'a GpuContext,
    embedded: Option<&'a TextureDictionary>,
    uploaded: Vec<Option<Arc<GpuTexture>>>,
    source: &'s mut dyn TextureSource,
    missing: Vec<String>,
}

impl Resolver<'_, '_> {
    fn upload(&self, t: &TextureBase) -> Option<Arc<GpuTexture>> {
        match GpuTexture::upload(self.ctx, t) {
            Ok(t) => Some(Arc::new(t)),
            Err(e) => {
                log::warn!("{e}");
                None
            }
        }
    }

    fn resolve(&mut self, t: Option<&ShaderTexture>) -> Option<Arc<GpuTexture>> {
        let t = t?;
        let name = match t {
            ShaderTexture::Embedded(i) => {
                let i = *i;
                if self.uploaded.len() <= i {
                    self.uploaded.resize(i + 1, None);
                }
                if self.uploaded[i].is_none() {
                    let tex = self.embedded?.textures.as_ref()?.get(i)?.as_ref()?;
                    self.uploaded[i] = self.upload(&tex.base);
                }
                return self.uploaded[i].clone();
            }
            ShaderTexture::External(base) => {
                if base.data.is_some() {
                    return self.upload(base);
                }
                base.name.clone().unwrap_or_default()
            }
        };
        // A name can also be one of the drawable's own textures.
        if let Some(i) = self.embedded.and_then(|d| {
            d.textures.as_ref()?.iter().position(|t| {
                t.as_ref().and_then(|t| t.base.name.as_deref()).is_some_and(|n| n.eq_ignore_ascii_case(&name))
            })
        }) {
            return self.resolve(Some(&ShaderTexture::Embedded(i)));
        }
        let found = self.source.texture(&name);
        if found.is_none() && !name.is_empty() && !self.missing.contains(&name) {
            self.missing.push(name);
        }
        found
    }
}

impl GpuModel {
    /// Uploads a drawable's geometries with their materials.
    pub fn from_drawable(
        ctx: &GpuContext,
        renderer: &Renderer,
        name: &str,
        d: &DrawableBase,
        hashes: &rf_formats::hash::HashIndex,
        source: &mut dyn TextureSource,
    ) -> Self {
        let group = d.shader_group.as_ref();
        let shaders: &[Option<Shader>] = group.and_then(|g| g.shaders.as_deref()).unwrap_or(&[]);
        let mut resolver = Resolver {
            ctx,
            embedded: group.and_then(|g| g.texture_dictionary.as_ref()),
            uploaded: Vec::new(),
            source,
            missing: Vec::new(),
        };
        let mut materials: Vec<Option<Arc<GpuMaterial>>> = vec![None; shaders.len()];
        let mut lods: [Vec<GpuDraw>; 4] = Default::default();
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for (li, lod) in d.lods.iter().enumerate() {
            let Some(lod) = lod else { continue };
            for model in lod.models.iter().flatten().flatten() {
                for g in model.geometries.iter().flatten().flatten() {
                    let Some(mesh) = GpuMesh::upload(ctx, g) else { continue };
                    let si = g.shader_id as usize;
                    let shader = shaders.get(si).and_then(Option::as_ref);
                    let (material, alpha) = match shader {
                        Some(s) => {
                            let desc = MaterialDesc::from_shader(s, hashes.name(s.name_hash));
                            let alpha = desc.alpha;
                            if materials[si].is_none() {
                                let textures = desc.textures.map(|t| resolver.resolve(t));
                                materials[si] = Some(Arc::new(renderer.create_material(ctx, &desc, textures)));
                            }
                            (materials[si].clone().unwrap(), alpha)
                        }
                        None => (renderer.default_material(), AlphaMode::Opaque),
                    };
                    for i in 0..3 {
                        min[i] = min[i].min(mesh.min[i]);
                        max[i] = max[i].max(mesh.max[i]);
                    }
                    lods[li].push(GpuDraw { mesh, material, alpha });
                }
            }
        }
        if min[0] > max[0] {
            (min, max) = (d.bounding_box_min, d.bounding_box_max);
        }
        Self { name: name.to_owned(), lods, min, max, lod_dists: d.lod_dists, missing_textures: resolver.missing }
    }
}

/// The joaat of a texture name as shader parameters store them (lowercase).
pub fn texture_hash(name: &str) -> u32 {
    joaat(&name.to_lowercase())
}
