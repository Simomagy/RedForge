// A texture as stored, for the texture viewer: one mip and layer, the channels
// picked, alpha over a checkerboard. Written so that the 8-bit sRGB target
// ends up holding the texture's own values.

struct Params {
    // x: mip, y: layer, z: mode, w: unused
    values: vec4<f32>,
    // The target size in pixels.
    size: vec4<f32>,
};

@group(0) @binding(0) var t: texture_2d_array<f32>;
@group(0) @binding(1) var s: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VertexOut;
    o.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(i: VertexOut) -> @location(0) vec4<f32> {
    let uv = i.clip.xy / params.size.xy;
    let c = textureSampleLevel(t, s, uv, i32(params.values.y), params.values.x);
    let mode = u32(params.values.z);
    var rgb: vec3<f32>;
    var alpha = 1.0;
    switch mode {
        // RGB over a checkerboard, by alpha.
        case 0u: { rgb = c.rgb; alpha = c.a; }
        case 1u: { rgb = c.rgb; }
        case 2u: { rgb = vec3<f32>(c.a); }
        case 3u: { rgb = vec3<f32>(c.r); }
        case 4u: { rgb = vec3<f32>(c.g); }
        case 5u: { rgb = vec3<f32>(c.b); }
        // A normal map with X and Y in red and green.
        case 6u: {
            let xy = c.rg * 2.0 - 1.0;
            let z = sqrt(max(1.0 - dot(xy, xy), 0.0));
            rgb = vec3<f32>(xy, z) * 0.5 + 0.5;
        }
        default: { rgb = c.rgb; }
    }
    let cell = (u32(i.clip.x) / 8u + u32(i.clip.y) / 8u) % 2u;
    let checker = select(vec3<f32>(0.42), vec3<f32>(0.58), cell == 0u);
    let shown = mix(checker, clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), alpha);
    return vec4<f32>(to_linear(shown), 1.0);
}
