// Shared by the sky's passes: a triangle covering the target, and the
// direction a cube map texel stands for.

const PI: f32 = 3.14159265359;

struct FaceOut {
    @builtin(position) clip: vec4<f32>,
    // 0..1 across the target, v down.
    @location(0) uv: vec2<f32>,
    // The draw's instance: the cube face (mip * 6 + face for the prefilter).
    @location(1) @interpolate(flat) index: u32,
};

@vertex
fn vs_face(@builtin(vertex_index) i: u32, @builtin(instance_index) instance: u32) -> FaceOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: FaceOut;
    o.clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    o.index = instance;
    return o;
}

// The direction through a point of a cube face (wgpu's layers +X, -X, +Y,
// -Y, +Z, -Z, with v down each face), so world directions sample the cube
// directly.
fn cube_dir(face: u32, uv: vec2<f32>) -> vec3<f32> {
    let a = uv.x * 2.0 - 1.0;
    let b = uv.y * 2.0 - 1.0;
    switch face {
        case 0u: { return vec3<f32>(1.0, -b, -a); }
        case 1u: { return vec3<f32>(-1.0, -b, a); }
        case 2u: { return vec3<f32>(a, 1.0, b); }
        case 3u: { return vec3<f32>(a, -1.0, -b); }
        case 4u: { return vec3<f32>(a, -b, 1.0); }
        default: { return vec3<f32>(-a, -b, -1.0); }
    }
}
