//! Drawables on the GPU: their geometries and materials, by LOD.

use std::sync::Arc;

use rf_formats::formats::ydr::{DrawableBase, Shader, ShaderParamType, ShaderTexture};
use rf_formats::formats::ytd::{TextureBase, TextureDictionary};
use rf_formats::hash::joaat;

use crate::GpuContext;
use crate::mesh::GpuMesh;
use crate::renderer::{GpuMaterial, MaterialKind, Renderer};
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
    pub normal_map: NormalMap,
    pub material_map: MaterialMap,
    /// Water, drawn as a flat glossy colour until there's a water shader.
    pub water: bool,
    /// A ped part's palette tinting (the C#'s `ColourTintMode` 2): the diffuse's
    /// red, green and blue weigh three colours of the tint palette, at these
    /// columns (0-255) and this row (0-1, in w).
    pub palette: Option<[f32; 4]>,
    /// How much the surface glows with its own colour (0-1): the shader's
    /// "intensity" or "emissivemultiplier" / 40, as the C#'s `EmissiveMult`.
    pub emissive: f32,
}

/// What a normal map holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NormalMap {
    /// Tangent-space XY in red and green.
    #[default]
    TangentXY,
    /// The normal itself, in RGB (terrain LODs).
    Direct,
}

/// What a material map ("_ma") holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaterialMap {
    /// Metalness in red, roughness in green.
    #[default]
    MetalRoughness,
    /// Smoothness in green (terrain LODs).
    Smoothness,
    /// Roughness in red (terrain SLODs).
    Roughness,
}

/// The constants of the terrain shaders ("terrain_uber_4lyr"...), as the C#
/// `BlendShader` names them.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainParams {
    /// Normal map strength per layer ("bumpiness").
    pub bump_scales: [f32; 4],
    /// Scale and offset of texcoord 0 for layers that use it ("uvscalebias").
    pub uv_scale_offset: [f32; 4],
    /// The texcoord (1-3) of each layer ("uvset").
    pub uv_indices: [f32; 4],
    /// The texcoord (1-3) of the lookup texture, in x ("lookupuvset").
    pub lookup_index: [f32; 4],
    /// How much the tint texture colours each layer ("tintintensity"...).
    pub tint_levels: [f32; 4],
    pub tint_blends: [f32; 4],
    /// Height blending ("hbbrangescale1234", "hbbrangebias1234").
    pub hbb_scales: [f32; 4],
    pub hbb_offsets: [f32; 4],
}

impl Default for TerrainParams {
    fn default() -> Self {
        Self {
            bump_scales: [1.0; 4],
            uv_scale_offset: [1.0, 1.0, 0.0, 0.0],
            uv_indices: [1.0; 4],
            lookup_index: [1.0; 4],
            tint_levels: [0.0; 4],
            tint_blends: [0.0; 4],
            hbb_scales: [1.0; 4],
            hbb_offsets: [0.0; 4],
        }
    }
}

/// Terrain texture slots: diffuse 0-3, normal 0-3, material 0-3, then these.
pub mod terrain_slot {
    pub const LOOKUP: usize = 12;
    pub const TINT: usize = 13;
    pub const TINT_NORMAL: usize = 14;
    pub const COUNT: usize = 15;
}

/// A terrain shader's textures and constants (`SetupTerrainShader`).
pub struct TerrainDesc<'a> {
    pub shader_name: String,
    pub textures: [Option<&'a ShaderTexture>; terrain_slot::COUNT],
    pub params: TerrainParams,
}

impl<'a> TerrainDesc<'a> {
    /// The terrain description of a shader, if it's a terrain shader.
    pub fn from_shader(shader: &'a Shader, shader_name: String) -> Option<Self> {
        if !names::TERRAIN.contains(&shader.name_hash) {
            return None;
        }
        let mut d =
            TerrainDesc { shader_name, textures: [None; terrain_slot::COUNT], params: TerrainParams::default() };
        for p in shader.parameters.as_ref().and_then(|p| p.params.as_deref()).unwrap_or(&[]) {
            match p.info.param_type() {
                ShaderParamType::Texture => {
                    let slot = match p.info.name {
                        0xce7b968e => 0, // diffusetexture_layer0
                        0xd883aa9e => 1,
                        0x32b9df09 => 2,
                        0x3d10f3b7 => 3,
                        0x27dbd2be => 4, // bumptexture_layer0
                        0x7e1f7f44 => 5,
                        0x4c759bf1 => 6,
                        0xde84c011 => 7,
                        0x3022de38 => 8, // materialmapatexture_layer0
                        0x42110214 => 9,
                        0x0cd6179f => 10,
                        0x3ea37b39 => 11,
                        0xc075f9b6 => terrain_slot::LOOKUP,      // lookuptexture
                        0x3fb7964f => terrain_slot::TINT,        // tinttexture
                        0x1a016c7e => terrain_slot::TINT_NORMAL, // tintbumptexture
                        _ => continue,
                    };
                    d.textures[slot] = p.texture.as_ref();
                }
                ShaderParamType::CBuffer => {
                    let t = &mut d.params;
                    match p.info.name {
                        0xf6712b81 => t.bump_scales = p.vector,     // bumpiness
                        0xc8ccec64 => t.uv_scale_offset = p.vector, // uvscalebias
                        0x724bed95 => t.uv_indices = p.vector,      // uvset
                        0xf354f896 => t.lookup_index = p.vector,    // lookupuvset
                        0xdb26e4de => t.tint_levels = p.vector,     // tintintensity
                        0x1caef69a => t.tint_blends = p.vector,     // tintblendamount
                        0xb2339a6c => t.hbb_scales = p.vector,      // hbbrangescale1234
                        0x03754af2 => t.hbb_offsets = p.vector,     // hbbrangebias1234
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Some(d)
    }
}

mod names {
    pub const BUMPINESS: u32 = 0xf6712b81;
    pub const INTENSITY: u32 = 0xefcd993d;
    pub const EMISSIVE_MULTIPLIER: u32 = 0x5eebed48;
    pub const DECAL_OPACITY: [u32; 4] = [0x0bc0cd58, 0x9db47191, 0x2be58df5, 0x39172858]; // decalblendopacitygbuffer0..3
    pub const DIRT_DECAL_MASK: u32 = 0x3e95fa90;

    /// The ped shaders a part's palette tints (`RDR2Ped.ApplyTextures`).
    pub const PED_PALETTE: &[u32] = &[
        0x34d90761, // ped
        0x7574b09b, // ped_standard
        0x7dd2a845, // ped_standard_cloth
        0xfe562866, // ped_standard_hair
        0xcc5d1333, // ped_standard_hair_lod
        0x92ca7852, // ped_standard_fur
        0x1f747353, // ped_standard_alpha
        0xa9dfb1e7, // ped_standard_cutout
        0x89a1cf72, // ped_standard_eye
        0x801d25b1, // ped_standard_eye_emissive
        0x1280e07f, // ped_standard_eye_retroreflect
        0x15c42150, // ped_standard_wrinkle
        0xf74149b5, // ped_standard_eye_cap
        0x0a156515, // ped_standard_hair_alpha
        0x2a6e3cff, // ped_standard_hair_normal
        0xc499b7a7, // ped_standard_hair_cloth
        0x05e14454, // ped_standard_hat
        0xe5acf189, // ped_standard_cloth_wrinkle
        0x86acf99a, // ped_slod_tint
        0x934fa33f, // ped_hair_cutout_alpha
        0x7b10334d, // ped_standard_skin_tint
        0x8b21e952, // ped_emissive_additive_alpha_flipbook
        0xf1345b4c, // horse_standard
    ];

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

    pub const TERRAIN: &[u32] = &[
        0x39dc7754, // terrain_uber_2lyr_pxm_hbb
        0xe87260a0, // terrain_uber_4lyr
        0x9f6397e3, // terrain_uber_4lyr_hbb
        0x3c497e4a, // terrain_uber_4lyr_pxm
        0x770b903e, // terrain_uber_4lyr_pxm_hbb
        0xa91fc138, // terrain_uber_4lyr_spx_dm_hbb
        0xa592dae9, // terrain_uber_4lyr_spx_dm_hbb_1111
        0x435b4e43, // terrain_uber_4lyr_spx_dm_hbb_1121
        0xded0f9db, // terrain_uber_4lyr_spx_dm_hbb_1221
        0x61b67683, // terrain_uber_4lyr_spx_dm_hbb_2111
        0x27a95f41, // terrain_uber_4lyr_spx_dm_hbb_2221
        0xe775ca5c, // terrain_uber_4lyr_mud
        0xa9b256d3, // terrain_uber_4lyr_mud_dm_hbb
        0x2cb19c4a, // terrain_uber_4lyr_quicksand_dm_hbb
        0x7584b19b, // terrain_uber_4lyr_snow_dm_hbb
        0x6e803e23, // terrain_uber_4lyr_snowglt_pxm_hbb
        0xaeaf82e3, // terrain_uber_4lyr_snowglt_spx_dm_hbb
        0x4f7d3fbe, // terrain_uber_3+1lyr_snow_dm_hbb
    ];
    pub const TERRAIN_LOD: &[u32] = &[
        0xd76ad078, // terrain_lod_grass_wetness
        0x42a96d26, // terrain_lod_2lyr
        0x31873a26, // terrain_lod_2lyr_grass_wetness
    ];
    pub const TERRAIN_SLOD: u32 = 0x3614acb6; // terrain_slod

    pub const WATER: &[u32] = &[
        0x07569ca0, // water_fountain
        0x38b8a4bb, // water_river
        0x27a48605, // water_bucket
        0xb4714d47, // water_river_highlod
        0x8c9f00e0, // water_river_cave_highlod
        0x489b68b7, // water_poolenv
        0xdb72e78f, // water_riverlod
        0x8b6aa6a4, // water_river_slod3
        0xa6765cbb, // water_river_slod2
        0x942fb82e, // water_river_slod1
        0xc4e3ff8f, // water_riverwithfoam
        0x414956f0, // water_riverwithfoam_highlod
        0x54376245, // water_riverwithpollenlayer_highlod
        0x274a9836, // water_river_oilfilm_highlod
        0x3d79b35e, // water_waterfall_highlod
        0xdb1550f6, // water_river_alpha_highlod
        0xa956b9cc, // water_river_alpha
        0x5013be81, // water_bath
    ];
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
                    names::INTENSITY | names::EMISSIVE_MULTIPLIER => d.emissive = (p.vector[0] * 0.025).clamp(0.0, 1.0),
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
        if names::WATER.contains(&name) || shader.draw_bucket & 0x7F == 6 {
            d.water = true;
            d.textures = [None; 7];
        }
        if names::TERRAIN_LOD.contains(&name) {
            (d.normal_map, d.material_map) = (NormalMap::Direct, MaterialMap::Smoothness);
        } else if name == names::TERRAIN_SLOD {
            (d.normal_map, d.material_map) = (NormalMap::Direct, MaterialMap::Roughness);
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
    /// For drawing posed, a skinned mesh's rig: its index in [`GpuModel::rigs`].
    pub rig: Option<u16>,
    /// For drawing posed, the bone (by tag) an unskinned mesh moves with.
    pub bone: Option<u16>,
}

/// What [`GpuModel::from_drawable_with`] does beyond uploading a drawable.
#[derive(Default)]
pub struct ModelOptions {
    /// Uploads the bone weights and indices of skinned meshes and keeps their
    /// rigs, for drawing posed ([`Renderer::render_posed`]).
    pub skin: bool,
    /// Textures replacing every material's diffuse, normal and material maps
    /// (a ped part's own, as the C#'s `ApplyTextures`).
    pub overrides: [Option<Arc<GpuTexture>>; 3],
    /// A ped part's tint palette and the columns its diffuse's red, green and
    /// blue pick, for the ped shaders.
    pub palette: Option<(Arc<GpuTexture>, [u8; 3])>,
}

pub struct GpuModel {
    pub name: String,
    /// The draws of each LOD (high, medium, low, very low).
    pub lods: [Vec<GpuDraw>; 4],
    /// With [`ModelOptions::skin`]: what the bone indices of each skinned
    /// model's vertices stand for, as bone tags (the model's bone mapping, or
    /// the drawable's skeleton).
    pub rigs: Vec<Vec<u16>>,
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
        Self::from_drawable_with(ctx, renderer, name, d, hashes, source, &ModelOptions::default())
    }

    /// [`GpuModel::from_drawable`], with [`ModelOptions`].
    pub fn from_drawable_with(
        ctx: &GpuContext,
        renderer: &Renderer,
        name: &str,
        d: &DrawableBase,
        hashes: &rf_formats::hash::HashIndex,
        source: &mut dyn TextureSource,
        options: &ModelOptions,
    ) -> Self {
        let bones = d.skeleton.as_ref().and_then(|s| s.bones.as_deref()).unwrap_or(&[]);
        let mut rigs: Vec<Vec<u16>> = Vec::new();
        let group = d.shader_group.as_ref();
        let shaders: &[Option<Shader>] = group.and_then(|g| g.shaders.as_deref()).unwrap_or(&[]);
        let mut resolver = Resolver {
            ctx,
            embedded: group.and_then(|g| g.texture_dictionary.as_ref()),
            uploaded: Vec::new(),
            source,
            missing: Vec::new(),
        };
        // Per shader: its material, or `None` for helper geometry not drawn.
        let mut materials: Vec<Option<Option<Arc<GpuMaterial>>>> = vec![None; shaders.len()];
        let mut lods: [Vec<GpuDraw>; 4] = Default::default();
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for (li, lod) in d.lods.iter().enumerate() {
            let Some(lod) = lod else { continue };
            for model in lod.models.iter().flatten().flatten() {
                // `SetSkeleton`: a skinned model's vertices index its bone mapping
                // (or the skeleton); an unskinned one may follow a bone.
                let (mut rig, mut bone) = (None, None);
                if options.skin && model.has_skin == 1 {
                    let tags = match &model.bone_mapping {
                        Some(m) => m.clone(),
                        None => bones.iter().map(|b| b.tag).collect(),
                    };
                    if !tags.is_empty() {
                        rig = Some(rigs.len() as u16);
                        rigs.push(tags);
                    }
                } else if options.skin {
                    bone = bones.get(model.bone_index as usize).map(|b| b.tag);
                }
                for g in model.geometries.iter().flatten().flatten() {
                    let si = g.shader_id as usize;
                    let shader = shaders.get(si).and_then(Option::as_ref);
                    let (material, alpha) = match shader {
                        Some(s) => {
                            if materials[si].is_none() {
                                let name = hashes.name(s.name_hash);
                                let m = match TerrainDesc::from_shader(s, name.clone()) {
                                    Some(t) => {
                                        let textures = t.textures.map(|t| resolver.resolve(t));
                                        Some(renderer.create_terrain_material(ctx, &t, textures))
                                    }
                                    None => {
                                        let mut desc = MaterialDesc::from_shader(s, name);
                                        let mut textures = desc.textures.map(|t| resolver.resolve(t));
                                        let slots = [Slot::Diffuse, Slot::Normal, Slot::Material];
                                        for (slot, t) in slots.into_iter().zip(&options.overrides) {
                                            if let Some(t) = t {
                                                textures[slot as usize] = Some(t.clone());
                                            }
                                        }
                                        if let Some((palette, tints)) = &options.palette
                                            && names::PED_PALETTE.contains(&s.name_hash)
                                        {
                                            // The C#'s fixed row ("TODO: configure this").
                                            let [r, g, b] = tints.map(f32::from);
                                            desc.palette = Some([r, g, b, 0.1]);
                                            textures[Slot::TintPalette as usize] = Some(palette.clone());
                                        }
                                        let helper = textures[Slot::Diffuse as usize].is_none()
                                            && desc.texture(Slot::Diffuse).is_some_and(is_placeholder);
                                        (!helper).then(|| renderer.create_material(ctx, &desc, textures))
                                    }
                                };
                                materials[si] = Some(m.map(Arc::new));
                            }
                            let Some(m) = materials[si].clone().flatten() else { continue };
                            (m, AlphaMode::from_draw_bucket(s.draw_bucket))
                        }
                        None => (renderer.default_material(), AlphaMode::Opaque),
                    };
                    let terrain = material.kind == MaterialKind::Terrain;
                    let Some(mesh) = GpuMesh::upload_with(ctx, g, terrain, rig.is_some()) else { continue };
                    for i in 0..3 {
                        min[i] = min[i].min(mesh.min[i]);
                        max[i] = max[i].max(mesh.max[i]);
                    }
                    let rig = rig.filter(|_| mesh.skin.is_some());
                    lods[li].push(GpuDraw { mesh, material, alpha, rig, bone });
                }
            }
        }
        if min[0] > max[0] {
            (min, max) = (d.bounding_box_min, d.bounding_box_max);
        }
        Self { name: name.to_owned(), lods, rigs, min, max, lod_dists: d.lod_dists, missing_textures: resolver.missing }
    }
}

/// RAGE's "no texture" placeholder, which helper geometry (smoke and light
/// volumes...) has as its diffuse: the game doesn't draw it.
fn is_placeholder(t: &ShaderTexture) -> bool {
    matches!(t, ShaderTexture::External(b) if b.data.is_none()
        && b.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("givemechecker")))
}

/// The joaat of a texture name as shader parameters store them (lowercase).
pub fn texture_hash(name: &str) -> u32 {
    joaat(&name.to_lowercase())
}
