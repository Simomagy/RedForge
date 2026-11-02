// Image-based lighting from the env cube, as the C#'s EnvMap*PS: the
// diffuse irradiance, the split-sum prefiltered radiance by roughness, and
// the BRDF lookup table (learnopengl.com's PBR/IBL).

@group(0) @binding(0) var t_env: texture_cube<f32>;
@group(0) @binding(1) var s_env: sampler;

// The prefiltered cube's mips; roughness is mip / (MIPS - 1).
const PREFILTERED_MIPS: u32 = 5u;

// The cosine-weighted hemisphere around each direction, 64 × 64 samples
// (EnvMapIrradiancePS), with π folded in: diffuse = irradiance × albedo.
@fragment
fn fs_irradiance(i: FaceOut) -> @location(0) vec4<f32> {
    let dir = normalize(cube_dir(i.index, i.uv));
    // The C# builds the frame on the face's axis; the sum over each ring
    // doesn't depend on it.
    let axis = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(dir.z) < 0.999);
    let right = normalize(cross(axis, dir));
    let up = cross(dir, right);
    let n = 64u;
    var col = vec3<f32>(0.0);
    for (var t = 0u; t < n; t++) {
        let theta = 0.5 * PI * f32(t) / f32(n);
        let st = sin(theta);
        let ct = cos(theta);
        for (var p = 0u; p < n; p++) {
            let phi = 2.0 * PI * f32(p) / f32(n);
            let v = (st * cos(phi)) * right + (st * sin(phi)) * up + ct * dir;
            col += textureSampleLevel(t_env, s_env, v, 0.0).rgb * ct * st;
        }
    }
    return vec4<f32>(col * PI / f32(n * n), 1.0);
}

fn radical_inverse(bits: u32) -> f32 {
    return f32(reverseBits(bits)) * 2.3283064365386963e-10;
}

fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), radical_inverse(i));
}

fn importance_sample_ggx(xi: vec2<f32>, n: vec3<f32>, roughness: f32) -> vec3<f32> {
    let a = roughness * roughness;
    let phi = 2.0 * PI * xi.x;
    let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_theta = sqrt(1.0 - cos_theta * cos_theta);
    let h = vec3<f32>(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
    let up = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(n.z) < 0.999);
    let tangent = normalize(cross(up, n));
    let bitangent = cross(n, tangent);
    return normalize(tangent * h.x + bitangent * h.y + n * h.z);
}

// One mip of the prefiltered cube (EnvMapSplitSumPS); the instance is
// mip * 6 + face. Mip 0 is a plain copy.
@fragment
fn fs_prefilter(i: FaceOut) -> @location(0) vec4<f32> {
    let face = i.index % 6u;
    let mip = i.index / 6u;
    let n = normalize(cube_dir(face, i.uv));
    let roughness = f32(mip) / f32(PREFILTERED_MIPS - 1u);
    let count = select(select(1024u, 512u, mip == 1u), 1u, mip == 0u);
    var c = vec3<f32>(0.0);
    var weight = 0.0;
    for (var s = 0u; s < count; s++) {
        let h = importance_sample_ggx(hammersley(s, count), n, roughness);
        let l = normalize(2.0 * dot(n, h) * h - n);
        let ndl = max(dot(n, l), 0.0);
        if ndl > 0.0 {
            c += textureSampleLevel(t_env, s_env, l, 0.0).rgb * ndl;
            weight += ndl;
        }
    }
    return vec4<f32>(c / max(weight, 1e-6), 1.0);
}

fn geometry_schlick_ggx(ndv: f32, roughness: f32) -> f32 {
    // IBL's k.
    let k = (roughness * roughness) / 2.0;
    return ndv / (ndv * (1.0 - k) + k);
}

// The split sum's scale and bias to F0 (EnvMapBRDFPS): u is N·V, v roughness.
@fragment
fn fs_brdf(i: FaceOut) -> @location(0) vec4<f32> {
    let ndv = i.uv.x;
    let roughness = i.uv.y;
    let v = vec3<f32>(sqrt(1.0 - ndv * ndv), 0.0, ndv);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let count = 1024u;
    var a = 0.0;
    var b = 0.0;
    for (var s = 0u; s < count; s++) {
        let h = importance_sample_ggx(hammersley(s, count), n, roughness);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let ndl = max(l.z, 0.0);
        let ndh = max(h.z, 0.0);
        let vdh = max(dot(v, h), 0.0);
        if ndl > 0.0 {
            let g = geometry_schlick_ggx(ndv, roughness) * geometry_schlick_ggx(ndl, roughness);
            let g_vis = (g * vdh) / (ndh * ndv);
            let fc = pow(1.0 - vdh, 5.0);
            a += (1.0 - fc) * g_vis;
            b += fc * g_vis;
        }
    }
    return vec4<f32>(a / f32(count), b / f32(count), 0.0, 1.0);
}
