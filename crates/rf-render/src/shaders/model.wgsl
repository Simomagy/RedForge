// Game geometry into the G-buffer, or lit as it's drawn for blended draws
// (after common.wgsl and lighting.wgsl).
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
    // x: decal mode (0 none, 1 decal, 2 dirt), y: tint palette (0 none, 1 by
    // the vertex colour and the entity's tint, 2 a ped part's palette),
    // z: two layers, w: the channel of colour1 that blends them
    modes: vec4<u32>,
    // x: normal map (0 tangent-space XY, 1 the normal in RGB),
    // y: material map (0 metal R + roughness G, 1 smoothness G, 2 roughness R),
    // z: water (glossy)
    config: vec4<u32>,
    // x: emission (0-1)
    extra: vec4<f32>,
    // A ped part's palette: the columns (0-255) the diffuse's red, green and
    // blue pick, and the row (0-1) in w.
    palette: vec4<f32>,
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
    // The tint palette row.
    @location(11) tint: f32,
    @location(14) id: u32,
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
    @location(7) @interpolate(flat) id: u32,
};

// A vertex's outputs, from its model-space position, normal and tangent.
fn vertex_out(v: VertexIn, position: vec3<f32>, normal: vec3<f32>, tangent: vec3<f32>) -> VertexOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(position, 1.0);
    let normal_m = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz);
    var o: VertexOut;
    o.clip = frame.view_proj * world;
    o.world = world.xyz;
    o.normal = normal_m * normal;
    o.tangent = vec4<f32>(normal_m * tangent, v.tangent.w);
    o.uv0 = v.uv0;
    o.colour0 = v.colour0;
    o.colour1 = v.colour1;
    o.tint = v.tint;
    o.id = v.id;
    return o;
}

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    return vertex_out(v, v.position, v.normal, v.tangent.xyz);
}

// Skinned meshes drawn posed: the draw's rig, 256 bones each as the 3 rows of
// a 3x4 matrix (the dynamic offset picks the rig).
@group(2) @binding(0) var<storage, read> bones: array<vec4<f32>>;

struct SkinIn {
    // The weights of bones 0-3 and 4-7 (unorm8 each), then their indices.
    @location(12) skin: vec4<u32>,
};

// The C#'s skin mode 2: the matrices of up to 8 bones blended by weight, as
// 3 rows (the columns here). A first index of 255 marks a cloth vertex, which
// isn't simulated: it stays as it is.
fn skin_rows(s: vec4<u32>) -> mat3x4<f32> {
    if (s.z & 0xFFu) == 255u {
        return mat3x4<f32>(vec4<f32>(1.0, 0.0, 0.0, 0.0), vec4<f32>(0.0, 1.0, 0.0, 0.0), vec4<f32>(0.0, 0.0, 1.0, 0.0));
    }
    let w0 = unpack4x8unorm(s.x);
    let w1 = unpack4x8unorm(s.y);
    var m = mat3x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    for (var k = 0u; k < 4u; k++) {
        let a = ((s.z >> (8u * k)) & 0xFFu) * 3u;
        let b = ((s.w >> (8u * k)) & 0xFFu) * 3u;
        m[0] += bones[a] * w0[k] + bones[b] * w1[k];
        m[1] += bones[a + 1u] * w0[k] + bones[b + 1u] * w1[k];
        m[2] += bones[a + 2u] * w0[k] + bones[b + 2u] * w1[k];
    }
    return m;
}

fn skin(m: mat3x4<f32>, p: vec4<f32>) -> vec3<f32> {
    return vec3<f32>(dot(m[0], p), dot(m[1], p), dot(m[2], p));
}

@vertex
fn vs_skinned(v: VertexIn, s: SkinIn) -> VertexOut {
    let m = skin_rows(s.skin);
    let position = skin(m, vec4<f32>(v.position, 1.0));
    let normal = skin(m, vec4<f32>(v.normal, 0.0));
    let tangent = skin(m, vec4<f32>(v.tangent.xyz, 0.0));
    return vertex_out(v, position, normal, tangent);
}

// The palette colour for this vertex, or white where the palette is black.
fn palette_tint(i: VertexOut) -> vec3<f32> {
    let size = textureDimensions(t_tint);
    let uv = vec2<f32>(i.colour0.b, (i.tint + 0.5) / f32(size.y));
    let t = textureSampleLevel(t_tint, s_point, uv, 0.0).rgb;
    return select(vec3<f32>(1.0), t, dot(t, t) != 0.0);
}

// A ped part's colour (the C#'s `ColourTintMode` 2): the diffuse's red, green
// and blue weigh three colours of the palette.
fn ped_palette(c: vec3<f32>) -> vec3<f32> {
    let size = f32(textureDimensions(t_tint).x);
    let p = material.palette;
    let r = textureSampleLevel(t_tint, s_wrap, vec2<f32>(p.x / size, p.w), 0.0).rgb;
    let g = textureSampleLevel(t_tint, s_wrap, vec2<f32>(p.y / size, p.w), 0.0).rgb;
    let b = textureSampleLevel(t_tint, s_wrap, vec2<f32>(p.z / size, p.w), 0.0).rgb;
    return r * c.r + g * c.g + b * c.b;
}

// What a fragment is: its surface, and how much of each G-buffer target it
// covers (a decal's masks; 1 for anything else).
struct Fragment {
    surface: Surface,
    alphas: vec4<f32>,
    alpha: f32,
};

fn fragment(i: VertexOut, front: bool) -> Fragment {
    let two_layer = material.modes.z != 0u;
    let layer = select(0.0, saturate(i.colour1[material.modes.w & 3u] * 10.0 - 4.5), two_layer);
    let decal_mode = material.modes.x;

    let diffuse = textureSample(t_albedo, s_wrap, i.uv0);
    var albedo_s = diffuse;
    if decal_mode == 2u {
        albedo_s = i.colour0;
    }
    if material.modes.y == 1u {
        albedo_s = vec4<f32>(albedo_s.rgb * palette_tint(i), albedo_s.a);
    } else if material.modes.y == 2u {
        albedo_s = vec4<f32>(ped_palette(albedo_s.rgb), albedo_s.a);
    }
    let diffuse2 = textureSample(t_albedo2, s_wrap, i.uv0);
    if two_layer {
        albedo_s = vec4<f32>(mix(albedo_s.rgb, diffuse2.rgb, layer), 1.0);
    }
    albedo_s *= material.tint;
    var alphas = vec4<f32>(1.0);
    switch decal_mode {
        case 1u: {
            albedo_s.a = diffuse.a * i.colour0.a;
            alphas = material.decal_masks * albedo_s.a;
        }
        case 2u: {
            albedo_s.a = saturate(dot(diffuse.rgb, material.decal_masks.rgb) * i.colour0.a);
            alphas = vec4<f32>(albedo_s.a);
        }
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

    // Without a material map: rough and not metal (the C#'s MeshSmoothness 0).
    var metal = 0.0;
    var smoothness = 0.0;
    if material.params.w > 0.5 {
        let p = mix(textureSample(t_params, s_wrap, i.uv0), textureSample(t_params2, s_wrap, i.uv0), layer);
        switch material.config.y {
            case 1u: { smoothness = p.g; }
            case 2u: { smoothness = 1.0 - p.r; }
            default: {
                metal = p.r;
                smoothness = 1.0 - p.g;
            }
        }
    }
    if material.config.z == 1u {
        smoothness = 0.9;
    }
    if frame.options.x == 4u {
        albedo_s = vec4<f32>(i.colour0.rgb, albedo_s.a);
    }

    let s = Surface(albedo_s.rgb, n, saturate(smoothness), metal, 1.0, 0.0, material.extra.x);
    return Fragment(s, alphas, albedo_s.a);
}

@fragment
fn fs_gbuffer(i: VertexOut, @builtin(front_facing) front: bool) -> GBufferOut {
    let f = fragment(i, front);
    return gbuffer_out(f.surface, f.alphas, i.id, i.clip);
}

// Blended draws (glass, water...), lit as they're drawn over the scene.
struct ForwardOut {
    @location(0) colour: vec4<f32>,
    @location(1) pick: vec2<u32>,
};

@fragment
fn fs_forward(i: VertexOut, @builtin(front_facing) front: bool) -> ForwardOut {
    let f = fragment(i, front);
    var c = light_surface(f.surface, i.world, 1.0);
    switch frame.options.x {
        case 1u, 4u: { c = f.surface.albedo; }
        case 2u: { c = f.surface.normal * 0.5 + 0.5; }
        case 3u: { c = vec3<f32>(f.surface.metal, 1.0 - f.surface.smoothness, 0.0); }
        default: {}
    }
    return ForwardOut(vec4<f32>(c, f.alpha), vec2<u32>(i.id, bitcast<u32>(i.clip.z)));
}

// Shadow maps of cut-out and blended draws: only what's solid enough.
@fragment
fn fs_shadow(i: VertexOut) {
    let a = textureSample(t_albedo, s_wrap, i.uv0).a * material.tint.a;
    if a < max(material.params.y, 0.5) {
        discard;
    }
}
