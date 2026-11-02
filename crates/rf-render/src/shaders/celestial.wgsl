// What's behind the atmosphere where nothing is drawn: the sun's disc, the
// moon opposite it and the stars, which turn with the sun about the celestial
// pole. Uses atmosphere.wgsl's `atmo` (light 0 the sun, 1 the moon) and
// `ray_sphere`.

@group(1) @binding(0) var t_moon: texture_2d<f32>;
// r: the Milky Way, all around the sky; a: the stars.
@group(1) @binding(1) var t_stars: texture_2d<f32>;
@group(1) @binding(2) var s_sky: sampler;

// Angular radii, in radians: the sun's 0.27°, the moon's 0.6° (RDR2 draws it
// larger than it is).
const SUN_RADIUS: f32 = 0.0047;
const MOON_RADIUS: f32 = 0.0105;
// Radiances: the sun's disc per unit of its colour (by day about 10× the
// Mie glow around it, whose bloom still outweighs the disc's), the moon's per
// unit of its light, the brightest stars' (the moonlit night sky is about
// 1e-4).
const SUN_DISC: f32 = 30.0;
const MOON_DISC: f32 = 30.0;
const STARS: f32 = 0.004;
const STAR_COLOUR: vec3<f32> = vec3<f32>(0.9, 0.95, 1.0);
const MILKY_WAY: f32 = 0.0015;
const MILKY_WAY_COLOUR: vec3<f32> = vec3<f32>(0.8, 0.85, 1.0);
// The celestial pole, 35° above the northern horizon, about the map's
// latitude.
const POLE: vec3<f32> = vec3<f32>(0.0, 0.819152, 0.573576);
// The galactic plane's tilt to the celestial equator, cos and sin of 62.9°.
const GALAXY_TILT: vec2<f32> = vec2<f32>(0.455842, 0.890061);

// Unit vectors across `forward`, the second as near `up` as it can be.
fn basis(forward: vec3<f32>, up: vec3<f32>) -> mat2x3<f32> {
    var right = cross(forward, up);
    let len = length(right);
    right = select(vec3<f32>(1.0, 0.0, 0.0), right / len, len > 1e-4);
    return mat2x3<f32>(right, cross(right, forward));
}

// The stars along a direction in the celestial frame (z the pole).
fn stars(c: vec3<f32>, pixel: f32) -> vec3<f32> {
    // The Milky Way: an equirectangular strip around the galactic equator.
    let g = vec3<f32>(c.x, c.y * GALAXY_TILT.x + c.z * GALAXY_TILT.y, c.z * GALAXY_TILT.x - c.y * GALAXY_TILT.y);
    let lon = atan2(g.y, g.x);
    let lat = asin(clamp(g.z, -1.0, 1.0));
    let band = vec2<f32>(lon / (2.0 * PI) + 0.5, 0.5 - lat / PI);
    let milky = textureSampleLevel(t_stars, s_sky, band, 0.0).r;
    // The stars on a cube's faces, so they keep their size near the poles.
    let a = abs(c);
    var face: vec2<f32>;
    if a.x >= a.y && a.x >= a.z {
        face = c.yz / c.x;
    } else if a.y >= a.z {
        face = c.xz / c.y;
    } else {
        face = c.xy / c.z;
    }
    let size = f32(textureDimensions(t_stars).x);
    let lod = log2(max(pixel * size * 0.5, 1.0));
    let dots = textureSampleLevel(t_stars, s_sky, face * 0.5 + 0.5, lod).a;
    return STAR_COLOUR * (STARS * dots * dots) + MILKY_WAY_COLOUR * (MILKY_WAY * milky * milky);
}

// The light from beyond the atmosphere along `dir` (from the camera), for a
// pixel `pixel` radians across; nothing below the horizon.
fn celestial(dir: vec3<f32>, pixel: f32) -> vec3<f32> {
    let below = ray_sphere(atmo.camera.xyz, dir, atmo.rad.x);
    if below.hit && below.start > 0.0 {
        return vec3<f32>(0.0);
    }
    let sun = atmo.light_dirs[0].xyz;

    // The stars, out as dusk turns to night (the sun 1° to 11° down).
    let night = 1.0 - smoothstep(-0.2, -0.02, sun.z);
    var light = vec3<f32>(0.0);
    if night > 0.0 {
        let frame = basis(POLE, sun);
        let c = vec3<f32>(dot(dir, frame[1]), dot(dir, frame[0]), dot(dir, POLE));
        light = stars(c, pixel) * night;
    }

    // The moon, in front of the stars, upright towards the pole; it's only
    // up when the sun isn't.
    let moon = atmo.light_dirs[1].xyz;
    let across = basis(moon, POLE);
    let p = vec2<f32>(dot(dir, across[0]), dot(dir, across[1])) / MOON_RADIUS;
    if dot(dir, moon) > 0.0 && max(abs(p.x), abs(p.y)) < 1.0 {
        let lod = log2(max(pixel * f32(textureDimensions(t_moon).x) / (2.0 * MOON_RADIUS), 1.0));
        let m = textureSampleLevel(t_moon, s_sky, p * vec2<f32>(0.5, -0.5) + 0.5, lod);
        let day = smoothstep(0.0, 0.15, sun.z);
        // (Its colours are stored gamma-encoded.)
        light = mix(light, m.rgb * m.rgb * atmo.light_colours[1].rgb * (MOON_DISC * (1.0 - day)), m.a);
    }

    // The sun's disc, a little darker at its limb, soft over a pixel.
    let s = dot(dir, sun);
    if s > 0.0 {
        let r = length(cross(dir, sun)) / SUN_RADIUS;
        let edge = max(pixel / SUN_RADIUS, 0.05);
        let disc = 1.0 - smoothstep(1.0 - edge, 1.0, r);
        let limb = 0.6 + 0.4 * sqrt(max(1.0 - r * r, 0.0));
        light += atmo.light_colours[0].rgb * (SUN_DISC * disc * limb);
    }
    return light;
}
