// Shared declarations, prepended to every shader module by the renderer.
// Layout must match `FrameUniform` in src/lib.rs.

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    // xyz: camera position (m), w: haze density (1/m).
    camera_pos: vec4<f32>,
    // xyz: unit vector toward the sun, w: unused.
    sun_dir: vec4<f32>,
    // rgb: sun irradiance (linear, HDR).
    sun_color: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_ground: vec4<f32>,
    // x: exposure, y: 1 to encode sRGB in the tonemap pass, z: shadow depth bias,
    // w: shadow normal offset (m).
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

struct InstanceIn {
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    @location(7) n0: vec4<f32>,
    @location(8) n1: vec4<f32>,
    @location(9) n2: vec4<f32>,
    @location(10) color: vec4<f32>,
};

fn instance_model(i: InstanceIn) -> mat4x4<f32> {
    return mat4x4<f32>(i.m0, i.m1, i.m2, i.m3);
}

// Clip-space corner of a triangle that covers the whole viewport.
fn fullscreen_corner(index: u32) -> vec2<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec2<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0);
}

// Radiance of the sky in direction `dir` (unit). `sun_disk` adds the visible sun.
fn sky_radiance(dir: vec3<f32>, sun_disk: bool) -> vec3<f32> {
    let up = sqrt(clamp(dir.y, 0.0, 1.0));
    var c = mix(frame.sky_horizon.rgb, frame.sky_zenith.rgb, up);
    c = mix(c, frame.sky_ground.rgb, 1.0 - smoothstep(-0.06, 0.0, dir.y));
    let s = max(dot(dir, frame.sun_dir.xyz), 0.0);
    // Broad forward-scatter glow around the sun, always on.
    c += frame.sun_color.rgb * (pow(s, 8.0) * 0.06);
    if sun_disk {
        c += frame.sun_color.rgb * (smoothstep(0.99985, 0.9999, s) * 20.0);
    }
    return c;
}
