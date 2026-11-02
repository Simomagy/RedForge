// The lighting, as the C# RedForge's Light.hlsli (after common.wgsl): the
// sun's light with its cascaded shadows, and the ambient light from the sky's
// environment maps, its second channel, the fill light and emission.

@group(0) @binding(1) var t_shadow: texture_depth_2d_array;
@group(0) @binding(2) var s_shadow: sampler_comparison;

// The sky's environment maps: irradiance, prefiltered by roughness (5 mips),
// and the BRDF's scale and bias.
@group(3) @binding(0) var t_env_irradiance: texture_cube<f32>;
@group(3) @binding(1) var t_env_prefiltered: texture_cube<f32>;
@group(3) @binding(2) var t_env_brdf: texture_2d<f32>;
@group(3) @binding(3) var s_env: sampler;

const ENV_MIPS: f32 = 4.0;

// A world direction as the environment cubes are looked up.
fn env_dir(d: vec3<f32>) -> vec3<f32> {
    return d;
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (1.0 - f0) * pow(saturate(1.0 - cos_theta), 5.0);
}

fn fresnel_schlick_roughness(cos_theta: f32, f0: vec3<f32>, roughness: f32) -> vec3<f32> {
    return f0 + (max(vec3<f32>(1.0 - roughness), f0) - f0) * pow(saturate(1.0 - cos_theta), 5.0);
}

fn distribution_ggx(n: vec3<f32>, h: vec3<f32>, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let nh = max(dot(n, h), 0.0);
    let d = nh * nh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn geometry_schlick_ggx(nv: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = r * r / 8.0;
    return nv / (nv * (1.0 - k) + k);
}

fn geometry_smith(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, roughness: f32) -> f32 {
    return geometry_schlick_ggx(max(dot(n, v), 0.0), roughness) * geometry_schlick_ggx(max(dot(n, l), 0.0), roughness);
}

// Light of colour `c` arriving from `l` (towards the light), Cook-Torrance as
// the C#'s `DirectionalLight`.
fn direct_light(s: Surface, v: vec3<f32>, l: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    let roughness = max(1.0 - s.smoothness, 0.02);
    let n = s.normal;
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metal);
    let h = normalize(v + l);
    let ndf = distribution_ggx(n, h, roughness);
    let g = geometry_smith(n, v, l, roughness);
    let f = fresnel_schlick(max(dot(h, v), 0.0), f0);
    let kd = (1.0 - f) * (1.0 - s.metal);
    let nl = max(dot(n, l), 0.0);
    let specular = min(ndf * g * f / (4.0 * max(dot(n, v), 0.0) * nl + 0.0001), vec3<f32>(10.0));
    return kd * s.albedo / PI * c * soft_nl(s, l) + specular * c * nl + translucent_light(s, l, c);
}

// N·L for the diffuse light: softer surfaces (skin, leaves) take some light
// past where they turn away from it, as light goes under them.
fn soft_nl(s: Surface, l: vec3<f32>) -> f32 {
    let w = s.translucency * 0.5;
    return saturate((dot(s.normal, l) + w) / (1.0 + w));
}

// Light from behind coming through (leaves, grass).
fn translucent_light(s: Surface, l: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    return s.albedo / PI * s.translucency * max(dot(-s.normal, l), 0.0) * c;
}

// Light on a surface as its diffuse and specular parts, for blended draws
// (whose reflections shouldn't fade with their opacity).
struct LightParts {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
};

fn direct_parts(s: Surface, v: vec3<f32>, l: vec3<f32>, c: vec3<f32>) -> LightParts {
    let roughness = max(1.0 - s.smoothness, 0.02);
    let n = s.normal;
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metal);
    let h = normalize(v + l);
    let f = fresnel_schlick(max(dot(h, v), 0.0), f0);
    let kd = (1.0 - f) * (1.0 - s.metal);
    let nl = max(dot(n, l), 0.0);
    let ndf = distribution_ggx(n, h, roughness);
    let g = geometry_smith(n, v, l, roughness);
    let specular = min(ndf * g * f / (4.0 * max(dot(n, v), 0.0) * nl + 0.0001), vec3<f32>(10.0));
    return LightParts(kd * s.albedo / PI * c * soft_nl(s, l) + translucent_light(s, l, c), specular * c * nl);
}

// How much of a cascade's map at `uvz` sees the light: 3x3 samples.
fn shadow_pcf(uvz: vec3<f32>, cascade: u32) -> f32 {
    let texel = frame.shadows.params.z;
    let bias = frame.shadows.texels[cascade] / frame.shadows.depths[cascade];
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let uv = uvz.xy + vec2<f32>(f32(x), f32(y)) * texel;
            lit += textureSampleCompareLevel(t_shadow, s_shadow, uv, cascade, uvz.z - bias);
        }
    }
    return lit / 9.0;
}

fn shadow_coords(world: vec3<f32>, cascade: u32) -> vec3<f32> {
    let c = frame.shadows.cascades[cascade] * vec4<f32>(world, 1.0);
    return vec3<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5, c.z);
}

// How much of the sun reaches a point: the first cascade that has it (the
// C#'s `CSMAmount`), blended into the next one near its edge. The point moves
// a little along the normal first, against shadow acne.
fn sun_shadow(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let count = u32(frame.shadows.params.x);
    let edge = frame.shadows.params.z;
    for (var i = 0u; i < count; i++) {
        let p = world + n * frame.shadows.texels[i] * 1.5;
        let uvz = shadow_coords(p, i);
        if min(uvz.x, uvz.y) <= edge || max(uvz.x, uvz.y) >= 1.0 - edge || uvz.z >= 1.0 || uvz.z <= 0.0 {
            continue;
        }
        var lit = shadow_pcf(uvz, i);
        let band = min(min(uvz.x, uvz.y), min(1.0 - uvz.x, 1.0 - uvz.y));
        let blend = frame.shadows.params.y;
        if band < blend && i + 1u < count {
            let next = shadow_coords(world + n * frame.shadows.texels[i + 1u] * 1.5, i + 1u);
            lit = mix(shadow_pcf(next, i + 1u), lit, band / blend);
        }
        return lit;
    }
    return 1.0;
}

// The ambient light, as the C#'s environment-mapped `AmbientLight` with the
// fill light the ambient pass adds, and emission. (RDR2's materials have no
// second ambient channel, so that's left out.)
fn ambient_parts(s: Surface, v: vec3<f32>, ao: f32) -> LightParts {
    let n = s.normal;
    let roughness = 1.0 - s.smoothness;
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metal);
    let nv = max(dot(n, v), 0.0);
    let f = fresnel_schlick_roughness(nv, f0, roughness);
    let kd = (1.0 - f) * (1.0 - s.metal);
    let irradiance = textureSampleLevel(t_env_irradiance, s_env, env_dir(n), 0.0).rgb;
    let r = reflect(-v, n);
    let prefiltered = textureSampleLevel(t_env_prefiltered, s_env, env_dir(r), roughness * ENV_MIPS).rgb;
    let brdf = textureSampleLevel(t_env_brdf, s_env, vec2<f32>(nv, roughness), 0.0).rg;
    let specular = prefiltered * (f * brdf.x + brdf.y);
    let a = saturate(ao);
    let diffuse = kd * irradiance * s.albedo * a + s.albedo * s.emissive + s.albedo * frame.fill.rgb * a;
    return LightParts(diffuse, specular * a);
}

fn ambient_light(s: Surface, v: vec3<f32>, ao: f32) -> vec3<f32> {
    let p = ambient_parts(s, v, ao);
    return p.diffuse + p.specular;
}

// All the frame's light on a surface at a point, as its two parts.
fn light_parts(s: Surface, world: vec3<f32>, ao: f32) -> LightParts {
    let v = normalize(frame.camera_pos.xyz - world);
    let sun = frame.sun_colour.rgb * sun_shadow(world, s.normal);
    let a = ambient_parts(s, v, ao);
    let d = direct_parts(s, v, normalize(frame.sun_dir.xyz), sun);
    return LightParts(a.diffuse + d.diffuse, a.specular + d.specular);
}

// All the frame's light on a surface at a point.
fn light_surface(s: Surface, world: vec3<f32>, ao: f32) -> vec3<f32> {
    let v = normalize(frame.camera_pos.xyz - world);
    let sun = frame.sun_colour.rgb * sun_shadow(world, s.normal);
    return ambient_light(s, v, ao) + direct_light(s, v, normalize(frame.sun_dir.xyz), sun);
}
