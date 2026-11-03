// Outlines, as the C#'s OutlineShader: what's outlined is drawn into a mask
// (with no depth test, so it shows through what's in front), which is widened
// across, then down, into a band drawn over the finished image. Inside the
// mask nothing is drawn (the C#'s fill alpha is 0).

struct Outline {
    colour: vec4<f32>,
};

@group(0) @binding(0) var<uniform> outline: Outline;
@group(1) @binding(0) var t_mask: texture_2d<f32>;
// The mask widened across (only read going down).
@group(1) @binding(1) var t_wide: texture_2d<f32>;

// How many pixels each side the band reaches.
const WIDTH: i32 = 3;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn texel(t: texture_2d<f32>, p: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(p, vec2<i32>(0), size - 1), 0).r;
}

// The mask widened along `step`, about pixel `p`.
fn widened(t: texture_2d<f32>, p: vec2<i32>, step: vec2<i32>) -> f32 {
    var total = 0.0;
    for (var i = -WIDTH; i <= WIDTH; i++) {
        total += texel(t, p + step * i);
    }
    return saturate(total / f32(WIDTH));
}

@fragment
fn fs_across(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(widened(t_mask, vec2<i32>(pos.xy), vec2<i32>(1, 0)));
}

@fragment
fn fs_down(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let inside = saturate(
        (texel(t_mask, p) + texel(t_mask, p + vec2<i32>(1, 1)) + texel(t_mask, p + vec2<i32>(1, -1))
            + texel(t_mask, p + vec2<i32>(-1, 1)) + texel(t_mask, p + vec2<i32>(-1, -1))) * 0.2,
    );
    if inside >= 1.0 {
        discard;
    }
    let f = widened(t_wide, p, vec2<i32>(0, 1));
    return vec4<f32>(outline.colour.rgb, outline.colour.a * (1.0 - inside) * f);
}
