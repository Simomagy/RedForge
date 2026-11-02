// RDR2's rivers and lakes ("water_river", "water_riverwithfoam",
// "water_riverwithpollenlayer"...), after common.wgsl and lighting.wgsl. The
// surface ripples with the global "waterbump2" carried along the river's flow
// map; under it the lit scene shows through, bent by the ripples and fading
// into the water's colour (its fog map) the deeper it is; the sky and the sun
// reflect off it, and foam and pollen float on it.

struct Water {
    // x: ripple bumpiness, y: ripple scale (per metre), z: foam scale (per
    // metre), w: flags (1 foam, 2 pollen, 4 waterfall)
    ripples: vec4<f32>,
    // A fog colour instead of the fog map's (when not black), and the opacity.
    fog_override: vec4<f32>,
    pollen: array<vec4<f32>, 3>,
    // Where the pollen placement map lies: x, y, width, height.
    pollen_area: vec4<f32>,
};

@group(1) @binding(0) var<uniform> water: Water;
@group(1) @binding(1) var t_flow: texture_2d<f32>;
@group(1) @binding(2) var t_fog: texture_2d<f32>;
@group(1) @binding(3) var t_foam: texture_2d<f32>;
@group(1) @binding(4) var t_pollen: texture_2d<f32>;
@group(1) @binding(5) var t_pollen_placement: texture_2d<f32>;
@group(1) @binding(6) var s_water: sampler;

// The scene under the water: its lit colour and depth (copies), and the
// ripples' normal maps (two, in red-green and blue-alpha).
@group(2) @binding(0) var t_under: texture_2d<f32>;
@group(2) @binding(1) var t_under_depth: texture_depth_2d;
@group(2) @binding(2) var t_bump: texture_2d<f32>;
@group(2) @binding(3) var s_clamp: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(3) uv0: vec2<f32>,
    @location(7) m0: vec4<f32>,
    @location(8) m1: vec4<f32>,
    @location(9) m2: vec4<f32>,
    @location(10) m3: vec4<f32>,
    @location(14) id: u32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv0: vec2<f32>,
};

@vertex
fn vs_water(v: VertexIn) -> VertexOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.position, 1.0);
    var o: VertexOut;
    o.clip = frame.view_proj * world;
    o.world = world.xyz;
    o.normal = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz) * v.normal;
    o.uv0 = v.uv0;
    return o;
}

// The world point drawn at a pixel, from its depth.
fn under_at(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec2<f32>(pixel.x * frame.viewport.z * 2.0 - 1.0, 1.0 - pixel.y * frame.viewport.w * 2.0);
    let p = frame.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    return p.xyz / p.w;
}

// The ripples' slope at a point of the ripple maps: the two layers, the
// second finer.
fn ripple(uv: vec2<f32>) -> vec2<f32> {
    let b = textureSample(t_bump, s_water, uv) * 2.0 - 1.0;
    let b2 = textureSample(t_bump, s_water, uv * 2.7 + vec2<f32>(0.31, 0.17)) * 2.0 - 1.0;
    return b.xy + b2.zw * 0.5;
}

// How long a flow-map cycle lasts, in seconds: the ripples move along the
// flow for that long, then fade into a second set half a cycle apart.
const FLOW_CYCLE: f32 = 2.0;
// How fast a full flow vector carries the ripples, in metres a second.
const FLOW_SPEED: f32 = 1.5;

@fragment
fn fs_water(i: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let flags = u32(water.ripples.w);
    let t = frame.time.x;
    var up = normalize(i.normal);
    if !front {
        up = -up;
    }

    // The ripples, along the flow (the map's v runs against the world's y).
    let flow_s = textureSample(t_flow, s_water, i.uv0);
    var flow = (flow_s.rg * 2.0 - 1.0) * vec2<f32>(1.0, -1.0);
    // Still water (no flow map, or its middle) drifts a little.
    flow += vec2<f32>(0.05, 0.03);
    var base = i.world.xy * water.ripples.y;
    if (flags & 4u) != 0u {
        // A waterfall: falls along its texcoords.
        base = i.uv0 * 8.0;
        flow = vec2<f32>(0.0, -3.0);
    }
    let p0 = fract(t / FLOW_CYCLE);
    let p1 = fract(t / FLOW_CYCLE + 0.5);
    let carried = flow * FLOW_SPEED * FLOW_CYCLE * water.ripples.y;
    let r0 = ripple(base - carried * p0);
    let r1 = ripple(base - carried * p1 + vec2<f32>(0.5, 0.25));
    let slope = mix(r0, r1, abs(p0 * 2.0 - 1.0)) * water.ripples.x * 0.35;
    let ref_t = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), up) + vec3<f32>(1e-4, 0.0, 0.0));
    let ref_b = cross(up, ref_t);
    let n = normalize(up + ref_t * slope.x + ref_b * slope.y);

    let v = normalize(frame.camera_pos.xyz - i.world);
    let px = i.clip.xy;
    let size = vec2<f32>(textureDimensions(t_under_depth));

    // What's under the surface, and how much water there is in between.
    let behind = textureLoad(t_under_depth, vec2<i32>(px), 0);
    var thickness = 200.0;
    if behind > 0.0 {
        thickness = distance(under_at(px, behind), i.world);
    }
    var uv = px / size + slope * 0.04 * saturate(thickness * 0.5);
    // Not what's in front of the water.
    let bent = textureLoad(t_under_depth, vec2<i32>(clamp(uv * size, vec2<f32>(0.0), size - 1.0)), 0);
    if bent > i.clip.z {
        uv = px / size;
    }
    let under = textureSampleLevel(t_under, s_clamp, uv, 0.0).rgb;

    // The water's own colour, lit by the sky and the sun.
    // (The fog map is a colour, stored sRGB-encoded.)
    var fog = pow(textureSample(t_fog, s_water, i.uv0).rgb, vec3<f32>(2.2));
    if any(water.fog_override.rgb > vec3<f32>(0.0)) {
        fog = water.fog_override.rgb;
    }
    let shadow = sun_shadow(i.world, up);
    let sky = textureSampleLevel(t_env_irradiance, s_env, env_dir(up), 0.0).rgb;
    let sun = frame.sun_colour.rgb * shadow * max(dot(up, normalize(frame.sun_dir.xyz)), 0.0) / PI;
    let lit_fog = fog * (sky + sun + frame.fill.rgb);
    // Murkier water (a darker fog colour) gets opaque sooner.
    let clarity = 0.5 + 6.0 * dot(fog, vec3<f32>(0.33));
    let transmittance = exp(-thickness / clarity * (1.05 - fog));
    var c = under * transmittance + lit_fog * (1.0 - transmittance);

    // Reflections: the sky's, and the sun's highlight.
    let nv = max(dot(n, v), 0.0);
    let fresnel = 0.02 + 0.98 * pow(1.0 - nv, 5.0);
    let r = reflect(-v, n);
    let reflected = textureSampleLevel(t_env_prefiltered, s_env, env_dir(r), 0.3).rgb;
    c = mix(c, reflected, fresnel);
    let l = normalize(frame.sun_dir.xyz);
    let s = Surface(vec3<f32>(0.0), n, 0.92, 0.0, 1.0, 0.0, 0.0);
    c += direct_light(s, v, l, frame.sun_colour.rgb * shadow);
    // ("fogandopacityoverride"'s opacity: waterfalls are mostly see-through.)
    c = mix(under, c, water.fog_override.w);

    // Foam on the rapids, carried by the flow.
    if (flags & 1u) != 0u {
        let fuv = i.world.xy * water.ripples.z;
        let fcarried = flow * FLOW_SPEED * FLOW_CYCLE * water.ripples.z;
        let f0 = textureSample(t_foam, s_water, fuv - fcarried * p0).r;
        let f1 = textureSample(t_foam, s_water, fuv - fcarried * p1 + vec2<f32>(0.5, 0.25)).r;
        let foam = saturate(mix(f0, f1, abs(p0 * 2.0 - 1.0)) * flow_s.a * 1.5);
        let foam_lit = vec3<f32>(0.85) * (sky + sun * PI * 0.5 + frame.fill.rgb);
        c = mix(c, foam_lit, foam);
    }

    // Pollen and weed floating where the placement map has them.
    if (flags & 2u) != 0u {
        let area = water.pollen_area;
        let puv = (i.world.xy - area.xy) / max(area.zw, vec2<f32>(1.0)) * vec2<f32>(1.0, -1.0);
        let place = textureSample(t_pollen_placement, s_water, puv);
        let masks = textureSample(t_pollen, s_water, i.world.xy * 0.25);
        let weights = place.rgb * masks.rgb;
        let colour = water.pollen[0].rgb * weights.r + water.pollen[1].rgb * weights.g + water.pollen[2].rgb * weights.b;
        let cover = saturate(weights.r + weights.g + weights.b);
        let pollen_lit = colour / max(cover, 1e-3) * (sky + sun * PI * 0.5 + frame.fill.rgb);
        c = mix(c, pollen_lit, cover);
    }

    // Debug views: the water's colour.
    if frame.options.x != 0u {
        c = fog;
    }
    return vec4<f32>(c, 1.0);
}
