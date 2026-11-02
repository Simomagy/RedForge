// What every scene shader shares: the frame's constants and the G-buffer.

// The sun's cascaded shadow maps.
struct Shadows {
    // World to each cascade's clip space.
    cascades: array<mat4x4<f32>, 4>,
    // The size of a texel of each cascade, in metres.
    texels: vec4<f32>,
    // The depth each cascade spans, in metres.
    depths: vec4<f32>,
    // x: cascades (0 = no shadows), y: the band blended into the next cascade
    // (in UV), z: one over the map's size
    params: vec4<f32>,
};

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // xyz: towards the sun (or the moon).
    sun_dir: vec4<f32>,
    sun_colour: vec4<f32>,
    // The ambient light's fill (the C#'s Colour1), and its second channel's
    // colours facing down and up (Colour3, Colour4).
    fill: vec4<f32>,
    ambient2_down: vec4<f32>,
    ambient2_up: vec4<f32>,
    // Where nothing is drawn, without a sky.
    background: vec4<f32>,
    // x: debug view (0 lit, 1 albedo, 2 normals, 3 material, 4 vertex colour,
    // 5 SSAO), y: SSAO on
    options: vec4<u32>,
    // xy: the view's size in pixels, zw: one over it.
    viewport: vec4<f32>,
    shadows: Shadows,
};

@group(0) @binding(0) var<uniform> frame: Frame;

const PI: f32 = 3.14159265;

// A surface as the G-buffer holds it (the C#'s albedo, normal, params and
// irradiance).
struct Surface {
    albedo: vec3<f32>,
    normal: vec3<f32>,
    smoothness: f32,
    metal: f32,
    ao: f32,
    // How much of the ambient light's second channel it gets.
    ambient2: f32,
    emissive: f32,
};

// The G-buffer, and for picking the drawn instance's id (0 for nothing) with
// the depth's bits. Each target's alpha is how much of it a decal covers. The
// C#'s params and irradiance targets are one here: smoothness, metalness and
// emission (RDR2's materials have no occlusion and no second ambient channel).
struct GBufferOut {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) params: vec4<f32>,
    @location(3) pick: vec2<u32>,
};

fn gbuffer_out(s: Surface, alphas: vec4<f32>, id: u32, frag: vec4<f32>) -> GBufferOut {
    return GBufferOut(
        vec4<f32>(s.albedo, alphas.x),
        vec4<f32>(s.normal, alphas.y),
        vec4<f32>(s.smoothness, s.metal, s.emissive, alphas.z),
        vec2<u32>(id, bitcast<u32>(frag.z)),
    );
}

// A tangent-space normal map value applied to a surface, as RedForge's
// `NormalMapping`: the bitangent is cross(tangent, normal) * tangent.w.
fn normal_mapping(nm: vec2<f32>, strength: f32, n: vec3<f32>, t: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let xy = nm * 2.0 - 1.0;
    let z = sqrt(abs(1.0 - dot(xy, xy)));
    let s = xy * strength;
    return normalize(t * s.x + b * s.y + n * z);
}
