// HDR to the screen: exposure and a filmic (Hable) curve, as RDR2's tonemap.

struct Params {
    // x: exposure, y: white point, z: 1 = pass the values through (debug views)
    values: vec4<f32>,
};

@group(0) @binding(0) var t_hdr: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    // A triangle covering the screen.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VertexOut;
    o.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

fn hable(x: vec3<f32>) -> vec3<f32> {
    let a = 0.22;
    let b = 0.30;
    let c = 0.10;
    let d = 0.20;
    let e = 0.01;
    let f = 0.30;
    return ((x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f)) - e / f;
}

@fragment
fn fs_main(i: VertexOut) -> @location(0) vec4<f32> {
    let hdr = textureLoad(t_hdr, vec2<i32>(i.clip.xy), 0);
    if params.values.z > 0.5 {
        return vec4<f32>(hdr.rgb, 1.0);
    }
    let white = vec3<f32>(params.values.y);
    let mapped = hable(hdr.rgb * params.values.x) / hable(white);
    return vec4<f32>(clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
