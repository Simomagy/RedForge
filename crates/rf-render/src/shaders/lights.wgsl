// The map's lights, each drawn as the inside of a box around its reach and
// added to the lit G-buffer (after common.wgsl, lighting.wgsl and
// deferred.wgsl): the C# RedForge's point, spot and capsule light passes.

struct Light {
    // xyz: position, w: range
    position: vec4<f32>,
    // rgb: colour, w: how sharply it fades at its range
    colour: vec4<f32>,
    // xyz: direction, w: kind (0 point, 1 spot, 2 capsule)
    direction: vec4<f32>,
    // x, y: the spot's inner and outer half angles, z: the capsule's half length
    shape: vec4<f32>,
};

@group(2) @binding(0) var<storage, read> lights: array<Light>;

struct LightOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) index: u32,
};

// A unit cube's 12 triangles, each wound anticlockwise seen from outside.
const CUBE: array<vec3<f32>, 36> = array<vec3<f32>, 36>(
    vec3(-1., -1., 1.), vec3(1., -1., 1.), vec3(1., 1., 1.), vec3(-1., -1., 1.), vec3(1., 1., 1.), vec3(-1., 1., 1.),
    vec3(1., -1., -1.), vec3(-1., -1., -1.), vec3(-1., 1., -1.), vec3(1., -1., -1.), vec3(-1., 1., -1.), vec3(1., 1., -1.),
    vec3(1., -1., 1.), vec3(1., -1., -1.), vec3(1., 1., -1.), vec3(1., -1., 1.), vec3(1., 1., -1.), vec3(1., 1., 1.),
    vec3(-1., -1., -1.), vec3(-1., -1., 1.), vec3(-1., 1., 1.), vec3(-1., -1., -1.), vec3(-1., 1., 1.), vec3(-1., 1., -1.),
    vec3(-1., 1., 1.), vec3(1., 1., 1.), vec3(1., 1., -1.), vec3(-1., 1., 1.), vec3(1., 1., -1.), vec3(-1., 1., -1.),
    vec3(-1., -1., -1.), vec3(1., -1., -1.), vec3(1., -1., 1.), vec3(-1., -1., -1.), vec3(1., -1., 1.), vec3(-1., -1., 1.),
);

@vertex
fn vs_light(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> LightOut {
    let l = lights[i];
    let reach = l.position.w + l.shape.z;
    let world = l.position.xyz + CUBE[v] * reach;
    return LightOut(frame.view_proj * vec4<f32>(world, 1.0), i);
}

// Inverse square, faded to 0 at the range (the C#'s `GetAttenuation`).
fn attenuation(d: f32, range: f32, power: f32) -> f32 {
    let edge = saturate(1.0 - pow(saturate(d / range), max(power, 1.0)));
    return edge / max(d * d, 0.01);
}

// The nearest point of a segment to a point, from that point.
fn to_segment(p: vec3<f32>, a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let ab = b - a;
    let t = saturate(dot(p - a, ab) / max(dot(ab, ab), 1e-8));
    return a + ab * t - p;
}

@fragment
fn fs_light(f: LightOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(f.clip.xy);
    let depth = textureLoad(t_depth, px, 0);
    if depth == 0.0 {
        discard;
    }
    let l = lights[f.index];
    let world = world_at(f.clip.xy, depth);
    let kind = u32(l.direction.w);
    var to_light = l.position.xyz - world;
    if kind == 2u {
        let half = l.direction.xyz * l.shape.z;
        to_light = to_segment(world, l.position.xyz - half, l.position.xyz + half);
    }
    let d = length(to_light);
    if d >= l.position.w {
        discard;
    }
    let ldir = to_light / max(d, 1e-5);
    var amount = attenuation(d, l.position.w, l.colour.w);
    if kind == 1u {
        let angle = acos(clamp(-dot(ldir, l.direction.xyz), -1.0, 1.0));
        if angle > l.shape.y {
            discard;
        }
        amount *= saturate(1.0 - (angle - l.shape.x) / max(l.shape.y - l.shape.x, 1e-4));
    }
    let albedo = textureLoad(g_albedo, px, 0);
    let params = textureLoad(g_params, px, 0);
    let n = normalize(textureLoad(g_normal, px, 0).xyz);
    let s = Surface(albedo.rgb, n, params.r, params.g, 1.0, params.a, params.b);
    let v = normalize(frame.camera_pos.xyz - world);
    return vec4<f32>(direct_light(s, v, ldir, l.colour.rgb * amount), 0.0);
}
