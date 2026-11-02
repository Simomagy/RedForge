// Game geometry, lit by a sun and a sky, into an HDR target.
//
// Materials follow RDR2's "standard" shaders: a diffuse map, a normal map with
// XY in its red and green channels, and a material map ("_ma") with metalness in
// red and roughness in green.

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

struct Material {
    tint: vec4<f32>,
    // x: bumpiness, y: alpha cut-off (0 = none), z: has normal map, w: has material map
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var t_albedo: texture_2d<f32>;
@group(1) @binding(2) var t_normal: texture_2d<f32>;
@group(1) @binding(3) var t_params: texture_2d<f32>;
@group(1) @binding(4) var s_wrap: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) colour0: vec4<f32>,
    @location(6) colour1: vec4<f32>,
    // The instance's transform, by columns.
    @location(7) m0: vec4<f32>,
    @location(8) m1: vec4<f32>,
    @location(9) m2: vec4<f32>,
    @location(10) m3: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) colour0: vec4<f32>,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.position, 1.0);
    let normal_m = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz);
    var o: VertexOut;
    o.clip = frame.view_proj * world;
    o.world = world.xyz;
    o.normal = normal_m * v.normal;
    o.tangent = vec4<f32>(normal_m * v.tangent.xyz, v.tangent.w);
    o.uv0 = v.uv0;
    o.colour0 = v.colour0;
    return o;
}

const PI: f32 = 3.14159265;

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

fn shade(i: VertexOut, front: bool) -> vec4<f32> {
    let albedo_s = textureSample(t_albedo, s_wrap, i.uv0) * material.tint;
    if material.params.y > 0.0 && albedo_s.a < material.params.y {
        discard;
    }

    var n = normalize(i.normal);
    if !front {
        n = -n;
    }
    if material.params.z > 0.5 {
        let nm = textureSample(t_normal, s_wrap, i.uv0);
        var xy = (nm.xy * 2.0 - 1.0) * material.params.x;
        let z = sqrt(max(1.0 - dot(xy, xy), 0.0));
        let t = normalize(i.tangent.xyz - n * dot(n, i.tangent.xyz));
        let b = cross(n, t) * i.tangent.w;
        n = normalize(t * xy.x + b * xy.y + n * z);
    }

    var metal = 0.0;
    var roughness = 0.6;
    if material.params.w > 0.5 {
        let p = textureSample(t_params, s_wrap, i.uv0);
        metal = p.r;
        roughness = clamp(p.g, 0.04, 1.0);
    }

    switch frame.options.x {
        case 1u: { return vec4<f32>(albedo_s.rgb, albedo_s.a); }
        case 2u: { return vec4<f32>(n * 0.5 + 0.5, 1.0); }
        case 3u: { return vec4<f32>(metal, roughness, 0.0, 1.0); }
        case 4u: { return vec4<f32>(i.colour0.rgb, 1.0); }
        default: {}
    }

    let v = normalize(frame.camera_pos.xyz - i.world);
    let l = normalize(frame.sun_dir.xyz);
    let diffuse_colour = albedo_s.rgb * (1.0 - metal);
    let f0 = mix(vec3<f32>(0.04), albedo_s.rgb, metal);
    let nl = max(dot(n, l), 0.0);
    let sun = frame.sun_colour.rgb * nl * (diffuse_colour / PI + ggx_specular(n, v, l, roughness, f0));
    let sky = mix(frame.ground_colour.rgb, frame.sky_colour.rgb, n.z * 0.5 + 0.5);
    let ambient = sky * (diffuse_colour + f0 * 0.25 * (1.0 - roughness));
    return vec4<f32>(sun + ambient, albedo_s.a);
}

@fragment
fn fs_main(i: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade(i, front);
}
