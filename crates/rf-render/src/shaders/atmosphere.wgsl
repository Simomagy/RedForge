// The atmosphere, as the C#'s Atmosphere.hlsli and AtmospherePS: single
// scattering of up to 4 lights in a planet's Rayleigh and Mie layers, marched
// along the view ray in planet space (metres, the planet's centre at the
// origin). Light directions point towards the lights.

struct Atmo {
    // Clip space to camera-relative world space (the view without its
    // translation).
    inv_view_proj: mat4x4<f32>,
    // The camera in planet space.
    camera: vec4<f32>,
    // x: inner (ground) radius, y: outer radius, z: brightness, w: opacity.
    rad: vec4<f32>,
    // The scattering colour (inverse wavelengths).
    colour: vec4<f32>,
    // x: Rayleigh, y: Mie density falloff.
    k: vec4<f32>,
    // x: Mie g.
    g: vec4<f32>,
    // x: Rayleigh, y: Mie scattering of the lights.
    ke_star: vec4<f32>,
    // x: lights, y: march steps.
    counts: vec4<u32>,
    // The env map's stand-in ground: rgb albedo, w 1 to draw it.
    ground: vec4<f32>,
    light_dirs: array<vec4<f32>, 4>,
    light_colours: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> atmo: Atmo;
@group(0) @binding(1) var t_lit: texture_2d<f32>;
@group(0) @binding(2) var t_depth: texture_depth_2d;

fn near_intersection(pos: vec3<f32>, ray: vec3<f32>, dist2: f32, rad2: f32) -> f32 {
    let b = 2.0 * dot(pos, ray);
    let det = max(0.0, b * b - 4.0 * (dist2 - rad2));
    return 0.5 * (-b - sqrt(det));
}

fn far_intersection(pos: vec3<f32>, ray: vec3<f32>, dist2: f32, rad2: f32) -> f32 {
    let b = 2.0 * dot(pos, ray);
    let det = max(0.0, b * b - 4.0 * (dist2 - rad2));
    return 0.5 * (-b + sqrt(det));
}

struct Span {
    hit: bool,
    start: f32,
    end: f32,
};

// Where a ray is inside a sphere around the origin (Math.hlsli's
// RaySphereIntersect): from 0 when it starts inside.
fn ray_sphere(s: vec3<f32>, d: vec3<f32>, r: f32) -> Span {
    let r2 = r * r;
    let s2 = dot(s, s);
    if s2 <= r2 {
        return Span(true, 0.0, far_intersection(s, d, s2, r2));
    }
    let ts = near_intersection(s, d, s2, r2);
    let te = far_intersection(s, d, s2, r2);
    return Span(te > ts && ts > 0.0, ts, te);
}

fn density(p: vec3<f32>, k0: f32) -> f32 {
    let h = (length(p) - atmo.rad.x) / (atmo.rad.y - atmo.rad.x);
    // min(exp(-k0 h), 1000), without overflowing.
    return exp(min(-k0 * h, 6.9077553));
}

// Three samples on a parabola. The factor isn't Simpson's 1/6, but the C#'s
// constants are tuned to it.
fn optical_depth(start: vec3<f32>, dir: vec3<f32>, len: f32, k0: f32) -> f32 {
    let y0 = density(start, k0);
    let y1 = density(start + dir * (len * 0.5), k0);
    let y2 = density(start + dir * len, k0);
    return (2.0 * len / 3.0) * (y0 + 4.0 * y1 + y2);
}

fn k4pi(k0: f32) -> f32 {
    return (1.0 / k0) * 4.0 * 0.031415;
}

// A modified Henyey-Greenstein; with g < 0 it peaks looking at the light.
fn mie_phase(c: f32, c2: f32, g: f32, g2: f32) -> f32 {
    let n = max(1.0 + g2 - 2.0 * g * c, 1e-6);
    return 1.5 * ((1.0 - g2) / (2.0 + g2)) * (1.0 + c2) / pow(n, 1.5);
}

fn rayleigh_phase(c2: f32) -> f32 {
    return 0.75 + 0.75 * c2;
}

struct Scatter {
    // The light scattered towards the camera; a: the atmosphere's opacity.
    light: vec4<f32>,
    // 1 - the transmittance along the ray.
    atten: vec3<f32>,
};

// The C#'s Atmosphere(): `start` is where the ray enters the atmosphere,
// `len` how far it goes through it.
fn scatter(start: vec3<f32>, dir: vec3<f32>, len: f32) -> Scatter {
    let irad = atmo.rad.x;
    let orad = atmo.rad.y;
    let steps = min(atmo.counts.y, 64u);
    let step_len = len / f32(steps);
    let orad2 = orad * orad;
    let ascale = atmo.rad.w / (orad - irad);
    let inv_wavelength = atmo.colour.rgb;
    let kr0 = atmo.k.x;
    let km0 = atmo.k.y;
    let kr4pi = k4pi(kr0);
    let km4pi = k4pi(km0);
    let gm = atmo.g.x;
    let gm2 = gm * gm;
    let intensity = atmo.rad.z / orad;
    let lights = min(atmo.counts.x, 4u);

    var rsums: array<vec3<f32>, 4>;
    var msums: array<vec3<f32>, 4>;
    for (var s = 0u; s < steps; s++) {
        let dist = (f32(s) + 0.5) * step_len;
        let p = start + dir * dist;
        let cam_r = optical_depth(start, dir, dist, kr0);
        let cam_m = optical_depth(start, dir, dist, km0);
        let r_density = density(p, kr0) * step_len;
        let m_density = density(p, km0) * step_len;
        for (var l = 0u; l < lights; l++) {
            let ldir = atmo.light_dirs[l].xyz;
            let light_len = far_intersection(p, ldir, dot(p, p), orad2);
            let r_depth = optical_depth(p, ldir, light_len, kr0);
            let m_depth = optical_depth(p, ldir, light_len, km0);
            let fr = (r_depth + cam_r) * kr4pi * ascale;
            let fm = (m_depth + cam_m) * km4pi * ascale;
            let attenuation = exp(inv_wavelength * -fr - fm);
            rsums[l] += attenuation * r_density;
            msums[l] += attenuation * m_density;
        }
    }

    var light = vec3<f32>(0.0);
    for (var l = 0u; l < lights; l++) {
        let colour = atmo.light_colours[l].rgb;
        let c0 = rsums[l] * atmo.ke_star.x * inv_wavelength * colour;
        let c1 = msums[l] * atmo.ke_star.y * colour;
        let c = dot(atmo.light_dirs[l].xyz, -dir);
        let c2 = c * c;
        light += abs(rayleigh_phase(c2) * c0 + mie_phase(c, c2, gm, gm2) * c1) * intensity;
    }

    // The atmosphere's thickness along the ray, for its opacity.
    let am = optical_depth(start, dir, len, km0);
    let ar = optical_depth(start, dir, len, kr0);
    let alpha = saturate(1.0 - exp((-am - ar) * ascale));
    let atten = max(1.0 - exp(inv_wavelength * -(ar * kr4pi * ascale) - am * km4pi * ascale), vec3<f32>(0.0));
    return Scatter(vec4<f32>(light, alpha), atten);
}

// AtmospherePS: `colour` seen along `dir` through the atmosphere, from the
// camera; it's `dist` metres away, 0 for nothing drawn there.
fn composite(dir: vec3<f32>, dist: f32, colour: vec3<f32>) -> vec3<f32> {
    let irad = atmo.rad.x;
    let orad = atmo.rad.y;
    let cam = atmo.camera.xyz;
    let outer = ray_sphere(cam, dir, orad);
    // Nothing on the ray is in the atmosphere.
    if !outer.hit || (dist > 0.0 && dist < outer.start) {
        return colour;
    }
    let inner = ray_sphere(cam, dir, irad);
    // The longest chord that misses the ground (the C#'s 2 orad iorng tan(acos(iorng))).
    let chord_max = 2.0 * sqrt(orad * orad - irad * irad);
    var len = min(outer.end - outer.start, chord_max);
    if inner.hit {
        // The middle of the ray, in the planet, is cut out.
        len = min(len, (inner.start - outer.start) + (outer.end - inner.end));
    }
    if dist > 0.0 {
        len = min(len, abs(dist - outer.start));
    }
    if len <= 0.0 {
        return colour;
    }
    let s = scatter(cam + dir * outer.start, dir, len);
    let atmos = s.light.rgb * s.light.a;
    // Fades to black deep under the ground.
    let height = length(cam);
    let as_max = irad * 0.995;
    let as_min = irad * 0.9;
    let blend = select(1.0, saturate((height - as_min) / (as_max - as_min)), height < as_max);
    let inscatter = min(max(atmos, vec3<f32>(0.0)), vec3<f32>(1000.0)) * blend;
    return max(colour * saturate(1.0 - s.atten), vec3<f32>(0.0)) + inscatter;
}

// The atmosphere over the lit scene.
@fragment
fn fs_scene(i: FaceOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(i.clip.xy);
    let lit = textureLoad(t_lit, px, 0).rgb;
    let depth = textureLoad(t_depth, px, 0);
    let ndc = vec2<f32>(i.uv.x * 2.0 - 1.0, 1.0 - i.uv.y * 2.0);
    // Reversed Z: depth 1 is the near plane.
    let near = atmo.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let dir = normalize(near.xyz / near.w);
    var dist = 0.0;
    if depth > 0.0 {
        let p = atmo.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
        dist = length(p.xyz / p.w);
    }
    return vec4<f32>(composite(dir, dist, lit), 1.0);
}

// A light's colour after the atmosphere above a point, as
// Sky::attenuated_light (which scales the depth twice the shader's).
fn transmittance(p: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let irad = atmo.rad.x;
    let orad = atmo.rad.y;
    let len = far_intersection(p, l, dot(p, p), orad * orad);
    let scale = 2.0 / (orad - irad);
    let rd = optical_depth(p, l, len, atmo.k.x) * scale * k4pi(atmo.k.x);
    let md = optical_depth(p, l, len, atmo.k.y) * scale * k4pi(atmo.k.y);
    return exp(atmo.colour.rgb * -rd - md);
}

// The env map's ground: a plain diffuse surface lit by the sky's lights.
fn ground(p: vec3<f32>) -> vec3<f32> {
    let n = normalize(p);
    var light = vec3<f32>(0.0);
    for (var l = 0u; l < min(atmo.counts.x, 4u); l++) {
        let ldir = atmo.light_dirs[l].xyz;
        let lit = dot(n, ldir);
        if lit > 0.0 {
            light += atmo.light_colours[l].rgb * transmittance(p, ldir) * lit;
        }
    }
    return atmo.ground.rgb / PI * light;
}

// A face of the env cube: the sky, and below the horizon the ground.
@fragment
fn fs_env(i: FaceOut) -> @location(0) vec4<f32> {
    let dir = normalize(cube_dir(i.index, i.uv));
    var colour = vec3<f32>(0.0);
    var dist = 0.0;
    if atmo.ground.w > 0.5 {
        let inner = ray_sphere(atmo.camera.xyz, dir, atmo.rad.x);
        if inner.hit && inner.start > 0.0 {
            dist = inner.start;
            colour = ground(atmo.camera.xyz + dir * dist);
        }
    }
    return vec4<f32>(composite(dir, dist, colour), 1.0);
}
