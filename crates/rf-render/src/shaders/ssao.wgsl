// Screen-space ambient occlusion, as the C# RedForge's DeferredSSAOPSMain and
// DeferredSSAOPSBlur (after common.wgsl and deferred.wgsl's vertex shader):
// points in a hemisphere around each pixel's normal are occluded where the
// depth buffer has something in front of them, then a 3x3 blur.

struct Ssao {
    samples: array<vec4<f32>, 32>,
    // A 3x3 grid of random directions turning the samples.
    noise: array<vec4<f32>, 9>,
    // x: radius (m), y: strength, z: the smallest a sample may look, in
    // pixels (its length over its depth, times the view's size), w: samples
    params: vec4<f32>,
};

@group(1) @binding(0) var t_depth: texture_depth_2d;
@group(1) @binding(1) var g_normal: texture_2d<f32>;
@group(1) @binding(2) var<uniform> ssao: Ssao;
@group(1) @binding(3) var t_raw: texture_2d<f32>;

struct FullscreenOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> FullscreenOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return FullscreenOut(vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0));
}

// A point in the camera's view space from normalised device coordinates.
fn view_at(ndc: vec2<f32>, depth: f32) -> vec3<f32> {
    let p = frame.inv_proj * vec4<f32>(ndc, depth, 1.0);
    return p.xyz / p.w;
}

@fragment
fn fs_ssao(f: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(f.clip.xy);
    let depth = textureLoad(t_depth, px, 0);
    if depth == 0.0 {
        return vec4<f32>(0.0);
    }
    let size = vec2<f32>(textureDimensions(t_depth));
    let ndc = vec2<f32>(f.clip.x / size.x * 2.0 - 1.0, 1.0 - f.clip.y / size.y * 2.0);
    let p = view_at(ndc, depth);
    let n = normalize((frame.view * vec4<f32>(textureLoad(g_normal, px, 0).xyz, 0.0)).xyz);
    let noise = ssao.noise[u32(px.y % 3) * 3u + u32(px.x % 3)].xyz;
    let t = normalize(noise - n * dot(noise, n));
    let basis = mat3x3<f32>(t, cross(n, t), n);
    let radius = ssao.params.x;
    let count = u32(ssao.params.w);
    var occluded = 0.0;
    for (var i = 0u; i < count; i++) {
        let sv = ssao.samples[i].xyz * radius;
        let sp = basis * sv + p;
        let c = frame.proj * vec4<f32>(sp, 1.0);
        if c.w <= 0.0 {
            continue;
        }
        let sn = c.xy / c.w;
        let uv = vec2<f32>(sn.x * 0.5 + 0.5, 0.5 - sn.y * 0.5);
        if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) {
            continue;
        }
        let sd = textureLoad(t_depth, vec2<i32>(uv * size), 0);
        if sd == 0.0 {
            continue;
        }
        let sz = view_at(sn, sd).z;
        let far_enough = abs(length(sv) / sz) > ssao.params.z / min(size.x, size.y);
        let in_range = abs(p.z - sz) < radius * 2.0;
        if sz >= sp.z && in_range && far_enough {
            occluded += 1.0;
        }
    }
    return vec4<f32>(saturate(occluded / f32(count) * ssao.params.y), 0.0, 0.0, 1.0);
}

@fragment
fn fs_blur(f: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(f.clip.xy);
    let last = vec2<i32>(textureDimensions(t_raw)) - 1;
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            sum += textureLoad(t_raw, clamp(px + vec2<i32>(x, y), vec2<i32>(0), last), 0).r;
        }
    }
    return vec4<f32>(sum / 9.0, 0.0, 0.0, 1.0);
}
