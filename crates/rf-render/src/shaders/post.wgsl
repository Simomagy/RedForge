// The HDR post-processing, as the C#'s HDRScene: the adapted luminance, the
// bloom, lens flares, the exposure and the tonemap curves.
//
// Bindings are unique across the module; each pass's layout has the ones it
// uses.

struct Params {
    // x: the adaptation's blend factor, y: 1 / the samples measured,
    // z, w: the adapted luminance's min and max
    luminance: vec4<f32>,
    // x: 2^EV, y: the curve's middle grey, z: the exposed luminance where the
    // scotopic shift is gone, w: unused
    exposure: vec4<f32>,
    // rgb: the night tint (luminance 1), a: how much (0 by day)
    scotopic: vec4<f32>,
    // x: bright threshold, y: bloom amount, z: flare threshold, w: flare amount
    bloom: vec4<f32>,
    // x: tonemap, y: flares drawn, z: 1 = pass the values through (debug
    // views), w: unused
    modes: vec4<u32>,
    // rgb: the flare's colour, a: its texcoord scale
    flares: array<vec4<f32>, 8>,
};

@group(0) @binding(0) var<uniform> params: Params;
// The HDR image; the horizontally blurred bloom in the vertical pass.
@group(0) @binding(1) var t_input: texture_2d<f32>;
// The sum of the log luminances, blended over time.
@group(0) @binding(2) var<storage, read_write> adapted_rw: f32;
@group(0) @binding(3) var<storage, read> adapted: f32;
@group(0) @binding(4) var t_bloom: texture_2d<f32>;
@group(0) @binding(5) var s_linear: sampler;

// The luminance is measured on a grid of this many samples a side.
const GRID: u32 = 81u;
const GROUP: u32 = 256u;
const LUM_VECTOR = vec3<f32>(0.299, 0.587, 0.114);
const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);

var<workgroup> sums: array<f32, GROUP>;

// One workgroup: the sum of the log luminances on the grid (so their average
// is the geometric mean, and a few bright lights at night don't darken
// everything else), blended into the adapted value.
@compute @workgroup_size(256)
fn cs_luminance(@builtin(local_invocation_index) i: u32) {
    let size = vec2<f32>(textureDimensions(t_input));
    var sum = 0.0;
    for (var s = i; s < GRID * GRID; s += GROUP) {
        let cell = vec2<f32>(f32(s % GRID), f32(s / GRID));
        let c = textureLoad(t_input, vec2<u32>(cell / f32(GRID) * size), 0);
        sum += log(max(dot(c.rgb, LUM_VECTOR), 1e-7));
    }
    sums[i] = sum;
    for (var n = GROUP / 2u; n > 0u; n >>= 1u) {
        workgroupBarrier();
        if i < n {
            sums[i] += sums[i + n];
        }
    }
    if i == 0u {
        let current = adapted_rw;
        let next = current + (sums[0] - current) * params.luminance.x;
        // (a NaN or infinite pixel would otherwise stick for good)
        if abs(next) < 1e30 {
            adapted_rw = next;
        }
    }
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    // A triangle covering the target.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VertexOut;
    o.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

// The bloom's 15-tap Gaussian, deviation 3.
fn bloom_weight(k: i32) -> f32 {
    let x = f32(k);
    return exp(-x * x / 18.0) / sqrt(2.0 * 3.14159265 * 9.0);
}

// The bright pass on every 8th pixel, blurred horizontally.
@fragment
fn fs_bloom_h(v: VertexOut) -> @location(0) vec4<f32> {
    let last = vec2<i32>(textureDimensions(t_input)) - 1;
    let p = vec2<i32>(v.clip.xy);
    var sum = vec3<f32>(0.0);
    for (var k = -7; k <= 7; k++) {
        let coord = clamp(vec2<i32>(p.x + k, p.y) * 8 + vec2<i32>(4, 3), vec2<i32>(0), last);
        let c = textureLoad(t_input, coord, 0).rgb;
        sum += max(c - params.bloom.x, vec3<f32>(0.0)) * bloom_weight(k);
    }
    return vec4<f32>(sum, 1.0);
}

@fragment
fn fs_bloom_v(v: VertexOut) -> @location(0) vec4<f32> {
    let last = i32(textureDimensions(t_input).y) - 1;
    let p = vec2<i32>(v.clip.xy);
    var sum = vec3<f32>(0.0);
    for (var k = -7; k <= 7; k++) {
        sum += textureLoad(t_input, vec2<i32>(p.x, clamp(p.y + k, 0, last)), 0).rgb * bloom_weight(k);
    }
    return vec4<f32>(sum, 1.0);
}

@fragment
fn fs_final(v: VertexOut) -> @location(0) vec4<f32> {
    var c = textureLoad(t_input, vec2<i32>(v.clip.xy), 0).rgb;
    if params.modes.z != 0u {
        return vec4<f32>(c, 1.0);
    }
    let uv = v.clip.xy / vec2<f32>(textureDimensions(t_input));
    c += params.bloom.y * textureSampleLevel(t_bloom, s_linear, uv, 0.0).rgb;

    // Lens flares: the bloom past a threshold, mirrored through the centre.
    let centred = uv * 2.0 - 1.0;
    for (var i = 0u; i < min(params.modes.y, 8u); i++) {
        let flare = params.flares[i];
        let fc = centred * flare.a;
        let f = saturate(1.0 - length(fc));
        let s = textureSampleLevel(t_bloom, s_linear, fc * 0.5 + 0.5, 0.0).rgb;
        c += max(s - params.bloom.z, vec3<f32>(0.0)) * flare.rgb * (f * f * params.bloom.w);
    }

    // The exposure: the average luminance lands at the curve's middle grey.
    let lum = clamp(exp(adapted * params.luminance.y), params.luminance.z, params.luminance.w);
    c *= params.exposure.y / lum * params.exposure.x;

    // Night vision (RDR2's scotopic shift): in the dark, colours fade to a
    // bluish grey.
    let l = dot(c, LUMA);
    let rod = params.scotopic.a * saturate(1.0 - l / params.exposure.z);
    c = mix(c, l * params.scotopic.rgb, rod);

    return vec4<f32>(saturate(tonemap(c, params.modes.x)), 1.0);
}

// The curves, in the order of the C#'s TonemapModeOptions.
fn tonemap(x: vec3<f32>, mode: u32) -> vec3<f32> {
    switch mode {
        case 1u: { return reinhard(x); }
        case 2u: { return reinhard_jodie(x); }
        case 3u: { return aces(x); }
        case 4u: { return aces_fitted(x); }
        case 5u: { return uncharted2(x); }
        case 6u: { return agx(x); }
        case 7u: { return lottes(x); }
        case 8u: { return saturate(x); }
        case 9u: { return rdr2(x); }
        default: { return reinhard_ext(x); }
    }
}

fn reinhard_ext(x: vec3<f32>) -> vec3<f32> {
    // (white at 1.5)
    let y = x * (1.0 + x / 1.5);
    return y / (1.0 + y);
}

fn reinhard(x: vec3<f32>) -> vec3<f32> {
    return x / (1.0 + x);
}

// Per-channel Reinhard blended with luminance Reinhard, which keeps hues.
fn reinhard_jodie(x: vec3<f32>) -> vec3<f32> {
    let l = dot(x, LUMA);
    let tc = x / (1.0 + x);
    return mix(x / (1.0 + l), tc, tc);
}

// Narkowicz's fit.
fn aces(x: vec3<f32>) -> vec3<f32> {
    return saturate((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14));
}

// Stephen Hill's fit of the RRT and ODT. The matrices are the HLSL's rows as
// columns, so `v * m` is its `mul(m, v)`.
const ACES_IN = mat3x3<f32>(
    vec3<f32>(0.59719, 0.35458, 0.04823),
    vec3<f32>(0.07600, 0.90834, 0.01566),
    vec3<f32>(0.02840, 0.13383, 0.83777),
);
const ACES_OUT = mat3x3<f32>(
    vec3<f32>(1.60475, -0.53108, -0.07367),
    vec3<f32>(-0.10208, 1.10813, -0.00605),
    vec3<f32>(-0.00327, -0.07276, 1.07602),
);

fn aces_fitted(x: vec3<f32>) -> vec3<f32> {
    let v = (x * 1.8) * ACES_IN;
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return saturate((a / b) * ACES_OUT);
}

fn hable(x: vec3<f32>, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> vec3<f32> {
    return ((x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f)) - e / f;
}

// Hable's filmic curve.
fn uncharted2(x: vec3<f32>) -> vec3<f32> {
    let white = hable(vec3<f32>(11.2), 0.15, 0.5, 0.1, 0.2, 0.02, 0.3);
    return hable(x * 2.0, 0.15, 0.5, 0.1, 0.2, 0.02, 0.3) / white;
}

// Hable's curve with the constants of RDR2's timecycles (postfx_tonemap_filmic_*).
fn rdr2(x: vec3<f32>) -> vec3<f32> {
    let white = hable(vec3<f32>(4.0), 0.1, 0.4, 0.1, 0.2, 0.01, 0.3);
    return saturate(hable(x, 0.1, 0.4, 0.1, 0.2, 0.01, 0.3) / white);
}

// AgX (Sobotka), with Wrensch's polynomial fit. The matrices are the HLSL's
// rows as columns, so `m * v` is its `mul(v, m)`.
const AGX_INSET = mat3x3<f32>(
    vec3<f32>(0.856627153315983, 0.137318972929847, 0.111898212999958),
    vec3<f32>(0.095212240538158, 0.761241990602591, 0.076799418603190),
    vec3<f32>(0.048251606145858, 0.101439036467562, 0.811302368396859),
);
const AGX_OUTSET = mat3x3<f32>(
    vec3<f32>(1.127100581814437, -0.141329763498438, -0.141329763498438),
    vec3<f32>(-0.110606643096603, 1.157823702216272, -0.110606643096603),
    vec3<f32>(-0.016493938717834, -0.016493938717834, 1.251936406595040),
);

fn agx(x: vec3<f32>) -> vec3<f32> {
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var c = log2(max(AGX_INSET * x, vec3<f32>(1e-10)));
    c = saturate((c - min_ev) / (max_ev - min_ev));
    // The sigmoid's 6th-order fit.
    let c2 = c * c;
    let c4 = c2 * c2;
    c = 15.5 * c4 * c2 - 40.14 * c4 * c + 31.96 * c4 - 6.868 * c2 * c + 0.4298 * c2 + 0.1191 * c - 0.00232;
    c = saturate(AGX_OUTSET * c);
    // The look: power 1.35, saturation 1.4.
    let luma = dot(c, LUMA);
    c = pow(c, vec3<f32>(1.35));
    c = luma + 1.4 * (c - luma);
    return saturate(c);
}

// Timothy Lottes' filmic curve: contrast, shoulder, the HDR value mapped to
// white and the middle grey in and out.
fn lottes(x: vec3<f32>) -> vec3<f32> {
    let a = 1.6;
    let d = 0.977;
    let hdr_max = 8.0;
    let mid_in = 0.18;
    let mid_out = 0.267;
    let ad = a * d;
    let mid_in_a = pow(mid_in, a);
    let mid_in_ad = pow(mid_in, ad);
    let hdr_max_a = pow(hdr_max, a);
    let hdr_max_ad = pow(hdr_max, ad);
    let b = (-mid_in_a + hdr_max_a * mid_out) / ((hdr_max_ad - mid_in_ad) * mid_out);
    let c = (hdr_max_ad * mid_in_a - hdr_max_a * mid_in_ad * mid_out) / ((hdr_max_ad - mid_in_ad) * mid_out);
    let xa = pow(max(x, vec3<f32>(0.0)), vec3<f32>(a));
    return saturate(xa / (xa * b + c));
}
