// What every model shader shares: the frame's constants and the lighting.

struct Frame {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // xyz: direction towards the sun.
    sun_dir: vec4<f32>,
    sun_colour: vec4<f32>,
    sky_colour: vec4<f32>,
    ground_colour: vec4<f32>,
    // x: debug view (0 lit, 1 albedo, 2 normals, 3 material, 4 vertex colour)
    options: vec4<u32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

// What the scene pass writes: the colour, and the drawn instance's id for
// picking (0 for nothing).
struct SceneOut {
    @location(0) colour: vec4<f32>,
    @location(1) id: u32,
};

const PI: f32 = 3.14159265;

// A tangent-space normal map value applied to a surface, as RedForge's
// `NormalMapping`: the bitangent is cross(tangent, normal) * tangent.w.
fn normal_mapping(nm: vec2<f32>, strength: f32, n: vec3<f32>, t: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let xy = nm * 2.0 - 1.0;
    let z = sqrt(abs(1.0 - dot(xy, xy)));
    let s = xy * strength;
    return normalize(t * s.x + b * s.y + n * z);
}

fn ggx_specular(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, roughness: f32, f0: vec3<f32>) -> vec3<f32> {
    let h = normalize(v + l);
    let a = max(roughness * roughness, 0.002);
    let a2 = a * a;
    let nh = max(dot(n, h), 0.0);
    let nv = max(dot(n, v), 1e-4);
    let nl = max(dot(n, l), 0.0);
    let d = a2 / (PI * pow(nh * nh * (a2 - 1.0) + 1.0, 2.0));
    let k = a * 0.5;
    let g = (nv / (nv * (1.0 - k) + k)) * (nl / (nl * (1.0 - k) + k));
    let f = f0 + (1.0 - f0) * pow(1.0 - max(dot(h, v), 0.0), 5.0);
    return d * g * f / (4.0 * nv * max(nl, 1e-4));
}

// Lights a surface (or shows one of its inputs, for the debug views).
fn lit(world: vec3<f32>, albedo: vec4<f32>, n: vec3<f32>, metal: f32, roughness: f32, colour: vec4<f32>) -> vec4<f32> {
    switch frame.options.x {
        case 1u: { return albedo; }
        case 2u: { return vec4<f32>(n * 0.5 + 0.5, 1.0); }
        case 3u: { return vec4<f32>(metal, roughness, 0.0, 1.0); }
        case 4u: { return vec4<f32>(colour.rgb, 1.0); }
        default: {}
    }

    let v = normalize(frame.camera_pos.xyz - world);
    let l = normalize(frame.sun_dir.xyz);
    let diffuse_colour = albedo.rgb * (1.0 - metal);
    let f0 = mix(vec3<f32>(0.04), albedo.rgb, metal);
    let nl = max(dot(n, l), 0.0);
    let sun = frame.sun_colour.rgb * nl * (diffuse_colour / PI + ggx_specular(n, v, l, roughness, f0));
    let sky = mix(frame.ground_colour.rgb, frame.sky_colour.rgb, n.z * 0.5 + 0.5);
    let ambient = sky * (diffuse_colour + f0 * 0.25 * (1.0 - roughness));
    return vec4<f32>(sun + ambient, albedo.a);
}
