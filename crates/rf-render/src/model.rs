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

/// The textures and constants a shader's material uses.
#[derive(Default)]
pub struct MaterialDesc<'a> {
    pub shader_name: String,
    pub albedo: Option<&'a ShaderTexture>,
    pub normal: Option<&'a ShaderTexture>,
    pub params: Option<&'a ShaderTexture>,
    pub bumpiness: f32,
    pub alpha: AlphaMode,
}

mod names {
    pub const DIFFUSE: &[u32] = &[0xc8e8c282, 0x346394ea]; // diffusetex, lyr0diffusetex
    pub const DIFFUSE_FALLBACK: &[u32] = &[0x7485dafb, 0x0df47048, 0x7d4f12e2]; // dirt, diffusetex2, lyr1
    pub const NORMAL: &[u32] = &[0x1e77bf4c, 0xd7c38241, 0x84c69350]; // bumptex, bumptexture, lyr0normaltex
    pub const NORMAL_FALLBACK: &[u32] = &[0x5462abc0, 0x58b6e707]; // bumptexture2, lyr1normaltex
    pub const PARAMS: &[u32] = &[0xc6f46e79, 0x9e3722e4, 0x2bb2ede8]; // speculartex, speculartexture, lyr0materialatex
    pub const PARAMS_FALLBACK: &[u32] = &[0x4d56d7fe]; // lyr1materialatex
    pub const BUMPINESS: u32 = 0xf6712b81;
}

impl<'a> MaterialDesc<'a> {
    /// Picks the textures the way the C# RedForge maps RDR2 shaders onto its
    /// default shader.
    pub fn from_shader(shader: &'a Shader, shader_name: String) -> Self {
        let mut d = MaterialDesc {
            shader_name,
            bumpiness: 1.0,
            alpha: AlphaMode::from_draw_bucket(shader.draw_bucket),
            ..Default::default()
        };
        let params = shader.parameters.as_ref().and_then(|p| p.params.as_deref()).unwrap_or(&[]);
        let find = |hashes: &[u32]| {
            hashes.iter().find_map(|h| {
                params
                    .iter()
                    .find(|p| p.info.name == *h && p.info.param_type() == ShaderParamType::Texture)?
                    .texture
                    .as_ref()
            })
        };
        d.albedo = find(names::DIFFUSE).or_else(|| find(names::DIFFUSE_FALLBACK));
        d.normal = find(names::NORMAL).or_else(|| find(names::NORMAL_FALLBACK));
        d.params = find(names::PARAMS).or_else(|| find(names::PARAMS_FALLBACK));
        if let Some(b) = params.iter().find(|p| p.info.name == names::BUMPINESS) {
            d.bumpiness = b.vector[0];
        }
        d
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
                                let albedo = resolver.resolve(desc.albedo);
                                let normal = resolver.resolve(desc.normal);
                                let params = resolver.resolve(desc.params);
                                materials[si] =
                                    Some(Arc::new(renderer.create_material(ctx, &desc, albedo, normal, params)));
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
