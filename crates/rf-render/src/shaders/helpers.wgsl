// Editor helpers (after common.wgsl): collisions, navmeshes, occluders and
// the like, as coloured meshes drawn into a layer of their own, with its own
// depth, over the image (the C#'s helpers view). Each instance's colour
// multiplies its vertices' (white when 0).

struct HelperIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(5) colour: vec4<f32>,
    @location(7) m0: vec4<f32>,
    @location(8) m1: vec4<f32>,
    @location(9) m2: vec4<f32>,
    @location(10) m3: vec4<f32>,
    @location(14) id: u32,
    @location(15) tint: u32,
};

struct HelperOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) colour: vec4<f32>,
    @location(3) @interpolate(flat) id: u32,
};

struct HelperTargets {
    // Premultiplied.
    @location(0) colour: vec4<f32>,
    @location(1) id: vec2<u32>,
};

@vertex
fn vs_helper(v: HelperIn) -> HelperOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.position, 1.0);
    var o: HelperOut;
    o.clip = clip_of(model, v.position);
    o.world = world.xyz;
    o.normal = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz) * v.normal;
    var tint = vec4<f32>(1.0);
    if v.tint != 0u {
        tint = unpack4x8unorm(v.tint);
    }
    o.colour = v.colour * tint;
    o.id = v.id;
    return o;
}

// Lit by the sun a little, and more where they face the eye, so that their
// sides tell apart; lines (no normal) as they are.
@fragment
fn fs_helper(in: HelperOut) -> HelperTargets {
    var shade = 1.0;
    if dot(in.normal, in.normal) > 1e-6 {
        let n = normalize(in.normal);
        let v = normalize(frame.camera_pos.xyz - in.world);
        shade = 0.45 + 0.3 * max(dot(n, normalize(frame.sun_dir.xyz)), 0.0) + 0.25 * abs(dot(n, v));
    }
    let c = in.colour;
    return HelperTargets(vec4<f32>(c.rgb * shade * c.a, c.a), vec2<u32>(in.id, bitcast<u32>(in.clip.z)));
}

// Into an outline's mask.
@fragment
fn fs_helper_mask() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0);
}

// The layer over the image, at the C#'s `Editor.HelperOverlayAlpha`.
const OVERLAY_ALPHA: f32 = 0.85;

@group(1) @binding(0) var t_helpers: texture_2d<f32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_overlay(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(t_helpers, vec2<i32>(pos.xy), 0) * OVERLAY_ALPHA;
}
