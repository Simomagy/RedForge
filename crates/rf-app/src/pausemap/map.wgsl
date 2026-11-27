// The pause map: each layer a family of tiles in a texture array, with a
// grid lookup (the tile's slot plus one, or 0 for a stub whose value is the
// second channel). Ported from the Map Editor web app's WebGL shader.

struct Geo {
    // cols, rows, tile, stride
    grid: vec4f,
    // sx, sy, ox, oy: atlas = (map + o) * s
    fit: vec4f,
}

struct U {
    geo: array<Geo, 4>,
    // text, lines, water, relief: 1 shown and loaded
    show: vec4f,
    // map x and y at the view's top-left, device pixels per map unit, grid on
    camera: vec4f,
    // the view's top-left in device pixels, the map's width and height
    view: vec4f,
    // the tile seams' spacing (the text atlas's stride)
    seam: vec4f,
    backdrop: vec4f,
    grid_color: vec4f,
    seam_color: vec4f,
}

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var t_text: texture_2d_array<f32>;
@group(0) @binding(3) var t_lines: texture_2d_array<f32>;
@group(0) @binding(4) var t_water: texture_2d_array<f32>;
@group(0) @binding(5) var t_relief: texture_2d_array<f32>;
@group(0) @binding(6) var i_text: texture_2d<u32>;
@group(0) @binding(7) var i_lines: texture_2d<u32>;
@group(0) @binding(8) var i_water: texture_2d<u32>;
@group(0) @binding(9) var i_relief: texture_2d<u32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

fn atlas_of(g: Geo, map: vec2f) -> vec2f {
    return (map + g.fit.zw) * g.fit.xy;
}

// The layer's value at atlas pixel `p` (255 outside): the tile whose inside
// holds p with a texel to spare on each side, so that filtering never reads
// past the pixels a tile shares with the next.
fn sample_layer(t: texture_2d_array<f32>, idx: texture_2d<u32>, g: Geo, p: vec2f, dx: vec2f, dy: vec2f) -> f32 {
    let stride = g.grid.w;
    let size = g.grid.z;
    let atlas = (g.grid.xy - 1.0) * stride + size;
    let cell = vec2i(clamp(floor((p - 1.0) / stride), vec2f(0.0), g.grid.xy - 1.0));
    let info = textureLoad(idx, cell, 0).rg;
    let uv = (p - vec2f(cell) * stride) / size;
    let v = textureSampleGrad(t, smp, uv, i32(max(info.r, 1u) - 1u), dx / size, dy / size).r * 255.0;
    let value = select(f32(info.g), v, info.r != 0u);
    let outside = any(p < vec2f(0.0)) || any(p >= atlas);
    return select(value, 255.0, outside);
}

// Coverage of "value below cut", with a screen pixel of antialiasing.
fn ink(v: f32, w: f32, cut: f32) -> f32 {
    return clamp((cut - v) / max(w, 1e-3) + 0.5, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let map = u.camera.xy + (frag.xy - u.view.xy) / u.camera.z;

    let pt = atlas_of(u.geo[0], map);
    let pl = atlas_of(u.geo[1], map);
    let pw = atlas_of(u.geo[2], map);
    let pr = atlas_of(u.geo[3], map);
    let text = select(255.0, sample_layer(t_text, i_text, u.geo[0], pt, dpdx(pt), dpdy(pt)), u.show.x > 0.5);
    let lines = select(255.0, sample_layer(t_lines, i_lines, u.geo[1], pl, dpdx(pl), dpdy(pl)), u.show.y > 0.5);
    let water = select(255.0, sample_layer(t_water, i_water, u.geo[2], pw, dpdx(pw), dpdy(pw)), u.show.z > 0.5);
    let relief = select(255.0, sample_layer(t_relief, i_relief, u.geo[3], pr, dpdx(pr), dpdy(pr)), u.show.w > 0.5);
    let wt = fwidth(text);
    let wl = fwidth(lines);
    let ww = fwidth(water);
    let wr = fwidth(relief);

    let paper = vec3f(0.827, 0.753, 0.627);
    let ink_c = vec3f(0.243, 0.216, 0.188);
    let wet = vec3f(0.612, 0.592, 0.522);

    let water_fill = ink(water, ww, 124.0);
    let coast = 1.0 - clamp(abs(water - 124.0) / max(ww * 1.2, 1e-3), 0.0, 1.0);
    var a = max(ink(text, wt, 68.0), ink(lines, wl, 64.0));
    a = max(a, coast * 0.55 * u.show.z);
    a = max(a, ink(relief, wr, 58.0) * 0.4);
    var c = mix(paper, wet, water_fill * 0.92);
    c = mix(c, ink_c, a);

    // Device pixels to the nearest tile seam, for hairlines.
    let seam = abs(fract(map / u.seam.x + 0.5) - 0.5) * u.seam.x * u.camera.z;
    let on_seam = 1.0 - clamp(min(seam.x, seam.y) - 0.5, 0.0, 1.0);
    let off_map = any(map < vec2f(0.0)) || any(map > u.view.zw);
    let around = mix(u.backdrop.rgb, u.grid_color.rgb, on_seam);
    let shown = mix(c, u.seam_color.rgb, on_seam * u.camera.w * 0.8);
    return vec4f(select(shown, around, off_map), 1.0);
}
