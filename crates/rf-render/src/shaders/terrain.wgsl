// Terrain ("terrain_uber_4lyr"...): four layers of diffuse, normal and material
// maps, mixed by a lookup texture or the vertex colours and by each layer's
// height (the material maps' blue), then tinted towards a low-detail colour
// map. A port of the C# RedForge's BlendShader in its blend mode 0.

struct Terrain {
    bump_scales: vec4<f32>,
    uv_scale_offset: vec4<f32>,
    // The texcoord (1-3) of each layer.
    uv_indices: vec4<f32>,
    // x: the texcoord (1-3) of the lookup texture.
    lookup_index: vec4<f32>,
    tint_levels: vec4<f32>,
    tint_blends: vec4<f32>,
    hbb_scales: vec4<f32>,
    hbb_offsets: vec4<f32>,
};

@group(1) @binding(0) var<uniform> terrain: Terrain;
@group(1) @binding(1) var t_diffuse0: texture_2d<f32>;
@group(1) @binding(2) var t_diffuse1: texture_2d<f32>;
@group(1) @binding(3) var t_diffuse2: texture_2d<f32>;
@group(1) @binding(4) var t_diffuse3: texture_2d<f32>;
@group(1) @binding(5) var t_normal0: texture_2d<f32>;
@group(1) @binding(6) var t_normal1: texture_2d<f32>;
@group(1) @binding(7) var t_normal2: texture_2d<f32>;
@group(1) @binding(8) var t_normal3: texture_2d<f32>;
@group(1) @binding(9) var t_material0: texture_2d<f32>;
@group(1) @binding(10) var t_material1: texture_2d<f32>;
@group(1) @binding(11) var t_material2: texture_2d<f32>;
@group(1) @binding(12) var t_material3: texture_2d<f32>;
@group(1) @binding(13) var t_lookup: texture_2d<f32>;
@group(1) @binding(14) var t_tint: texture_2d<f32>;
@group(1) @binding(15) var t_tint_normal: texture_2d<f32>;
@group(1) @binding(16) var s_wrap: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) colour0: vec4<f32>,
    @location(6) colour1: vec4<f32>,
    @location(7) m0: vec4<f32>,
    @location(8) m1: vec4<f32>,
    @location(9) m2: vec4<f32>,
    @location(10) m3: vec4<f32>,
    @location(12) uv2: vec2<f32>,
    @location(13) tangent1: vec4<f32>,
    @location(14) id: u32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent0: vec4<f32>,
    @location(3) tangent1: vec4<f32>,
    @location(4) uv0: vec2<f32>,
    @location(5) uv1: vec2<f32>,
    @location(6) uv2: vec2<f32>,
    @location(7) colour0: vec4<f32>,
    @location(8) colour1: vec4<f32>,
    @location(9) @interpolate(flat) id: u32,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.position, 1.0);
    let normal_m = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz);
    var o: VertexOut;
    o.clip = clip_of(model, v.position);
    o.world = world.xyz;
    o.normal = normal_m * v.normal;
    o.tangent0 = vec4<f32>(normal_m * v.tangent.xyz, v.tangent.w);
    o.tangent1 = vec4<f32>(normal_m * v.tangent1.xyz, v.tangent1.w);
    o.uv0 = v.uv0;
    o.uv1 = v.uv1;
    o.uv2 = v.uv2;
    o.colour0 = v.colour0;
    o.colour1 = v.colour1;
    o.id = v.id;
    return o;
}

fn layer_uv(i: VertexOut, index: f32) -> vec2<f32> {
    if index == 2.0 {
        return i.uv1;
    }
    if index == 3.0 {
        return i.uv2;
    }
    return i.uv0 * terrain.uv_scale_offset.xy + terrain.uv_scale_offset.zw;
}

fn lookup_uv(i: VertexOut) -> vec2<f32> {
    let index = terrain.lookup_index.x;
    if index == 2.0 {
        return i.uv1;
    }
    if index == 3.0 {
        return i.uv2;
    }
    return i.uv0;
}

// The weight of each layer: the masks (the fourth is what the other three
// leave), scaled by the layer heights, keeping only the highest within 0.1.
fn blend_factors(masks: vec4<f32>, heights: vec4<f32>) -> vec4<f32> {
    let m = vec4<f32>(masks.rgb, saturate(1.0 - (masks.r + masks.g + masks.b)));
    let f0 = m * (heights + terrain.hbb_offsets) * terrain.hbb_scales;
    let top = max(max(f0.r, f0.g), max(f0.b, f0.a)) - 0.1;
    let f = max(f0 - top, vec4<f32>(0.0));
    return f / dot(f, vec4<f32>(1.0));
}

// A layer's diffuse, tinted towards the low-detail colour by its material
// map's alpha.
fn tint_layer(c: vec4<f32>, lod: vec4<f32>, material: vec4<f32>, layer: u32) -> vec4<f32> {
    let level = terrain.tint_levels[layer];
    let f = saturate(material.w * terrain.tint_blends[layer]);
    let t = (lod * level) / (level + 1.0);
    return mix(c, t, f);
}

fn layer_normal(
    nm: vec4<f32>,
    layer: u32,
    base: vec3<f32>,
    t0: vec3<f32>,
    b0: vec3<f32>,
    t1: vec3<f32>,
    b1: vec3<f32>,
) -> vec3<f32> {
    let first = terrain.uv_indices[layer] == 1.0;
    return normal_mapping(nm.xy, terrain.bump_scales[layer], base, select(t1, t0, first), select(b1, b0, first));
}

@fragment
fn fs_main(i: VertexOut, @builtin(front_facing) front: bool) -> GBufferOut {
    var n = normalize(i.normal);
    if !front {
        n = -n;
    }
    let t0 = normalize(i.tangent0.xyz);
    let b0 = cross(t0, n) * i.tangent0.w;
    let t1 = normalize(i.tangent1.xyz);
    let b1 = cross(t1, n) * i.tangent1.w;

    let uv0 = layer_uv(i, terrain.uv_indices.x);
    let uv1 = layer_uv(i, terrain.uv_indices.y);
    let uv2 = layer_uv(i, terrain.uv_indices.z);
    let uv3 = layer_uv(i, terrain.uv_indices.w);

    let p0 = textureSample(t_material0, s_wrap, uv0);
    let p1 = textureSample(t_material1, s_wrap, uv1);
    let p2 = textureSample(t_material2, s_wrap, uv2);
    let p3 = textureSample(t_material3, s_wrap, uv3);

    // The masks come from the lookup texture, or from the vertex colours as
    // much as the first colour's alpha says.
    let lookup = vec4<f32>(textureSample(t_lookup, s_wrap, lookup_uv(i)).rgb, 0.0);
    let masks = mix(lookup, vec4<f32>(i.colour0.rgb, i.colour1.r), i.colour0.a);
    let bf = blend_factors(masks, vec4<f32>(p0.z, p1.z, p2.z, p3.z));

    let lod = textureSample(t_tint, s_wrap, i.uv0);
    let base = normal_mapping(textureSample(t_tint_normal, s_wrap, i.uv0).xy, 1.0, n, t0, b0);

    let a0 = tint_layer(textureSample(t_diffuse0, s_wrap, uv0), lod, p0, 0u);
    let a1 = tint_layer(textureSample(t_diffuse1, s_wrap, uv1), lod, p1, 1u);
    let a2 = tint_layer(textureSample(t_diffuse2, s_wrap, uv2), lod, p2, 2u);
    let a3 = tint_layer(textureSample(t_diffuse3, s_wrap, uv3), lod, p3, 3u);
    let n0 = layer_normal(textureSample(t_normal0, s_wrap, uv0), 0u, base, t0, b0, t1, b1);
    let n1 = layer_normal(textureSample(t_normal1, s_wrap, uv1), 1u, base, t0, b0, t1, b1);
    let n2 = layer_normal(textureSample(t_normal2, s_wrap, uv2), 2u, base, t0, b0, t1, b1);
    let n3 = layer_normal(textureSample(t_normal3, s_wrap, uv3), 3u, base, t0, b0, t1, b1);

    let albedo = a0 * bf.x + a1 * bf.y + a2 * bf.z + a3 * bf.w;
    let normal = normalize(n0 * bf.x + n1 * bf.y + n2 * bf.z + n3 * bf.w);
    let p = p0 * bf.x + p1 * bf.y + p2 * bf.z + p3 * bf.w;
    // Roughness in green; terrain isn't metal.
    var c = albedo.rgb;
    if frame.options.x == 4u {
        c = i.colour0.rgb;
    }
    let s = Surface(c, normal, 1.0 - saturate(p.g), 0.0, 1.0, 0.0, 0.0);
    return gbuffer_out(s, vec4<f32>(1.0), i.id, i.clip);
}
