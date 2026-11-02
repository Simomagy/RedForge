// The passes over the whole view (after common.wgsl and lighting.wgsl): the
// G-buffer lit by the sun and the sky (the C#'s directional and ambient light
// passes, in one), and the background where nothing is drawn.

@group(1) @binding(0) var t_depth: texture_depth_2d;
@group(1) @binding(1) var g_albedo: texture_2d<f32>;
@group(1) @binding(2) var g_normal: texture_2d<f32>;
@group(1) @binding(3) var g_params: texture_2d<f32>;
@group(1) @binding(5) var t_ssao: texture_2d<f32>;
@group(1) @binding(6) var t_lit: texture_2d<f32>;

struct FullscreenOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> FullscreenOut {
    // A triangle covering the view.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return FullscreenOut(vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0));
}

// The world point drawn at a pixel, from its depth.
fn world_at(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec2<f32>(pixel.x * frame.viewport.z * 2.0 - 1.0, 1.0 - pixel.y * frame.viewport.w * 2.0);
    let p = frame.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    return p.xyz / p.w;
}

@fragment
fn fs_lighting(f: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(f.clip.xy);
    let depth = textureLoad(t_depth, px, 0);
    if depth == 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let albedo = textureLoad(g_albedo, px, 0);
    let params = textureLoad(g_params, px, 0);
    let n = normalize(textureLoad(g_normal, px, 0).xyz);
    let s = Surface(albedo.rgb, n, params.r, params.g, 1.0, 0.0, params.b);
    let ssao = select(0.0, textureLoad(t_ssao, px, 0).r, frame.options.y != 0u);
    switch frame.options.x {
        case 1u, 4u: { return vec4<f32>(s.albedo, 1.0); }
        case 2u: { return vec4<f32>(s.normal * 0.5 + 0.5, 1.0); }
        case 3u: { return vec4<f32>(s.metal, 1.0 - s.smoothness, 0.0, 1.0); }
        case 5u: { return vec4<f32>(vec3<f32>(1.0 - ssao), 1.0); }
        default: {}
    }
    let world = world_at(f.clip.xy, depth);
    return vec4<f32>(light_surface(s, world, (1.0 - ssao) * s.ao), 1.0);
}

// Without a sky: the lit scene over the background colour.
@fragment
fn fs_background(f: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(f.clip.xy);
    if textureLoad(t_depth, px, 0) == 0.0 {
        return frame.background;
    }
    return textureLoad(t_lit, px, 0);
}
