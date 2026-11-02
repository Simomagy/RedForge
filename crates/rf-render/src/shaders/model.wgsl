// Game geometry, lit by a sun and a sky, into an HDR target (after
// common.wgsl).
//
// Materials follow RDR2's "standard" shaders: a diffuse map, a normal map with
// XY in its red and green channels, and a material map ("_ma") with metalness in
// red and roughness in green. Two-layer shaders blend a second set by a channel
// of the second vertex colour; a tint palette colours the diffuse by the vertex
// colour's blue (column) and the entity's tint (row). Decals take their opacity
// from the diffuse and vertex alphas, scaled by how much reaches the albedo.

struct Material {
    tint: vec4<f32>,
    // x: bumpiness, y: alpha cut-off (0 = none), z: has normal map, w: has material map
    params: vec4<f32>,
    // How much of a decal reaches the albedo, normals, material and lighting.
    decal_masks: vec4<f32>,
    // x: decal mode (0 none, 1 decal, 2 dirt), y: has tint palette,
    // z: two layers, w: the channel of colour1 that blends them
    modes: vec4<u32>,
    // x: normal map (0 tangent-space XY, 1 the normal in RGB),
    // y: material map (0 metal R + roughness G, 1 smoothness G, 2 roughness R),
    // z: water (glossy)
    config: vec4<u32>,
};

@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var t_albedo: texture_2d<f32>;
@group(1) @binding(2) var t_normal: texture_2d<f32>;
@group(1) @binding(3) var t_params: texture_2d<f32>;
@group(1) @binding(4) var s_wrap: sampler;
@group(1) @binding(5) var t_albedo2: texture_2d<f32>;
@group(1) @binding(6) var t_normal2: texture_2d<f32>;
@group(1) @binding(7) var t_params2: texture_2d<f32>;
@group(1) @binding(8) var t_tint: texture_2d<f32>;
@group(1) @binding(9) var s_point: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) colour0: vec4<f32>,
    @location(6) colour1: vec4<f32>,
    // The instance's transform, by columns.
    @location(7) m0: vec4<f32>,
    @location(8) m1: vec4<f32>,
    @location(9) m2: vec4<f32>,
    @location(10) m3: vec4<f32>,
    // x: the tint palette row
    @location(11) extra: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) colour0: vec4<f32>,
    @location(5) colour1: vec4<f32>,
    @location(6) @interpolate(flat) tint: f32,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.position, 1.0);
    let normal_m = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz);
    var o: VertexOut;
    o.clip = frame.view_proj * world;
    o.world = world.xyz;
    o.normal = normal_m * v.normal;
    o.tangent = vec4<f32>(normal_m * v.tangent.xyz, v.tangent.w);
    o.uv0 = v.uv0;
    o.colour0 = v.colour0;
    o.colour1 = v.colour1;
    o.tint = v.extra.x;
    return o;
}

// The palette colour for this vertex, or white where the palette is black.
fn palette_tint(i: VertexOut) -> vec3<f32> {
    let size = textureDimensions(t_tint);
    let uv = vec2<f32>(i.colour0.b, (i.tint + 0.5) / f32(size.y));
    let t = textureSampleLevel(t_tint, s_point, uv, 0.0).rgb;
    return select(vec3<f32>(1.0), t, dot(t, t) != 0.0);
}

fn shade(i: VertexOut, front: bool) -> vec4<f32> {
    let two_layer = material.modes.z != 0u;
    let layer = select(0.0, saturate(i.colour1[material.modes.w & 3u] * 10.0 - 4.5), two_layer);
    let decal_mode = material.modes.x;

    let diffuse = textureSample(t_albedo, s_wrap, i.uv0);
    var albedo_s = diffuse;
    if decal_mode == 2u {
        albedo_s = i.colour0;
    }
    if material.modes.y != 0u {
        albedo_s = vec4<f32>(albedo_s.rgb * palette_tint(i), albedo_s.a);
    }
    let diffuse2 = textureSample(t_albedo2, s_wrap, i.uv0);
    if two_layer {
        albedo_s = vec4<f32>(mix(albedo_s.rgb, diffuse2.rgb, layer), 1.0);
    }
    albedo_s *= material.tint;
    switch decal_mode {
        case 1u: { albedo_s.a = diffuse.a * i.colour0.a * material.decal_masks.x; }
        case 2u: { albedo_s.a = saturate(dot(diffuse.rgb, material.decal_masks.rgb) * i.colour0.a); }
        default: {}
    }
    if (material.params.y > 0.0 && albedo_s.a < material.params.y) || (decal_mode != 0u && albedo_s.a <= 0.0) {
        discard;
    }

    var n = normalize(i.normal);
    if !front {
        n = -n;
    }
    if material.params.z > 0.5 {
        let nm = mix(textureSample(t_normal, s_wrap, i.uv0), textureSample(t_normal2, s_wrap, i.uv0), layer);
        if material.config.x == 1u {
            n = normalize(nm.xyz * 2.0 - 1.0);
        } else {
            let t = normalize(i.tangent.xyz - n * dot(n, i.tangent.xyz));
            let b = cross(t, n) * i.tangent.w;
            n = normal_mapping(nm.xy, material.params.x, n, t, b);
        }
    }

    var metal = 0.0;
    var roughness = 0.6;
    if material.params.w > 0.5 {
        let p = mix(textureSample(t_params, s_wrap, i.uv0), textureSample(t_params2, s_wrap, i.uv0), layer);
        switch material.config.y {
            case 1u: { roughness = 1.0 - p.g; }
            case 2u: { roughness = p.r; }
            default: {
                metal = p.r;
                roughness = p.g;
            }
        }
        roughness = clamp(roughness, 0.04, 1.0);
    }
    if material.config.z == 1u {
        roughness = 0.1;
    }

    return lit(i.world, albedo_s, n, metal, roughness, i.colour0);
}

@fragment
fn fs_main(i: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade(i, front);
}
