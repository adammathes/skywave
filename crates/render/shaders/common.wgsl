// Shared declarations, prepended to every shader module by the renderer.
// Layout must match `FrameUniform` in src/lib.rs. Radiance is linear sRGB in kcd/m².

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // World-to-clip of each shadow cascade's sun projection.
    cascade_view_proj: array<mat4x4<f32>, 3>,
    // xyz: camera position (m), w: fog density (1/m).
    camera_pos: vec4<f32>,
    // xyz: unit view direction, w: unused.
    camera_fwd: vec4<f32>,
    // xyz: far view depth of each cascade (m), w: blend band as a fraction of a cascade.
    cascade_far: vec4<f32>,
    // xyz: world size of one shadow texel per cascade (m), w: cascade edge in texels.
    cascade_texel: vec4<f32>,
    // xyz: unit vector toward the sun, w: unused.
    sun_dir: vec4<f32>,
    // rgb: sun illuminance / pi, i.e. luminance of a white Lambertian surface facing the sun.
    sun_light: vec4<f32>,
    // rgb: radiance at the sun disc center, w: cos(angular radius).
    sun_disc: vec4<f32>,
    // xyz: unit vector toward the moon, w: cos(angular radius).
    moon_dir: vec4<f32>,
    // x: full-disc luminance, y: earthshine fraction, z: star brightness 0..1, w: unused.
    moon_light: vec4<f32>,
    // Perez coefficients A..E, each for (Y, x, y).
    perez_a: vec4<f32>,
    perez_b: vec4<f32>,
    perez_c: vec4<f32>,
    perez_d: vec4<f32>,
    perez_e: vec4<f32>,
    // xyz: zenith (Y, x, y) / F(0, theta_s), w: unused.
    zenith: vec4<f32>,
    // xyz: toward the sun clamped to the horizon (Perez gamma), w: unused.
    sun_sky: vec4<f32>,
    // x: twilight luminance fade, y: twilight blue shift, z: horizon haze, w: unused.
    sky_params: vec4<f32>,
    // rgb: night sky floor radiance.
    night_sky: vec4<f32>,
    // rgb: cosine-weighted mean sky radiance (ambient from above).
    sky_up: vec4<f32>,
    // rgb: ground radiance (ambient from below, and the view below the horizon).
    sky_ground: vec4<f32>,
    // rgb: mean horizon radiance (haze color).
    sky_horizon: vec4<f32>,
    // x: exposure, y: 1 to encode sRGB in the shader, z: shadow offset toward the sun
    // (texels), w: shadow normal offset (texels).
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct InstanceIn {
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) m3: vec4<f32>,
    @location(8) n0: vec4<f32>,
    @location(9) n1: vec4<f32>,
    @location(10) n2: vec4<f32>,
    @location(11) color: vec4<f32>,
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

// ---- Display transform ----------------------------------------------------------------

// ACES fitted curve (Stephen Hill's RRT+ODT fit).
fn aces_fitted(color: vec3<f32>) -> vec3<f32> {
    // sRGB -> AP1 (with the RRT's saturation tweak), column-major.
    let aces_in = mat3x3<f32>(
        vec3<f32>(0.59719, 0.07600, 0.02840),
        vec3<f32>(0.35458, 0.90834, 0.13383),
        vec3<f32>(0.04823, 0.01566, 0.83777),
    );
    let aces_out = mat3x3<f32>(
        vec3<f32>(1.60475, -0.10208, -0.00327),
        vec3<f32>(-0.53108, 1.10813, -0.07276),
        vec3<f32>(-0.07367, -0.00605, 1.07602),
    );
    let v = aces_in * color;
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return clamp(aces_out * (a / b), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// HDR radiance -> display value: exposure, tonemap, sRGB encode unless the target does it.
fn display(radiance: vec3<f32>) -> vec4<f32> {
    var c = aces_fitted(radiance * frame.params.x);
    if frame.params.y > 0.5 {
        c = linear_to_srgb(c);
    }
    return vec4<f32>(c, 1.0);
}

// ---- Sky ------------------------------------------------------------------------------
// Mirrors SkyModel in src/sky.rs.

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const TWILIGHT_BLUE: vec3<f32> = vec3<f32>(0.447, 0.894, 3.687);
// Keep radiance well inside the f16 range.
const MAX_RADIANCE: f32 = 60000.0;

fn perez(cos_theta: f32, gamma: f32, cos_gamma: f32) -> vec3<f32> {
    let ct = max(cos_theta, 1e-3);
    return (1.0 + frame.perez_a.xyz * exp(frame.perez_b.xyz / ct))
        * (1.0 + frame.perez_c.xyz * exp(frame.perez_d.xyz * gamma)
            + frame.perez_e.xyz * cos_gamma * cos_gamma);
}

// CIE xyY -> linear sRGB, white-balanced from the model's equal-energy white to D65.
fn yxy_to_rgb(yxy: vec3<f32>) -> vec3<f32> {
    let big_y = yxy.x;
    let y = max(yxy.z, 1e-4);
    let xyz = vec3<f32>(yxy.y / y * big_y * 0.95047, big_y, (1.0 - yxy.y - y) / y * big_y * 1.08883);
    // XYZ -> linear sRGB (D65), column-major.
    let m = mat3x3<f32>(
        vec3<f32>(3.240454, -0.969266, 0.055643),
        vec3<f32>(-1.537138, 1.876011, -0.204026),
        vec3<f32>(-0.498531, 0.041556, 1.057225),
    );
    return max(m * xyz, vec3<f32>(0.0));
}

// Diffuse sky radiance toward unit `dir`: Preetham with twilight fade and blue shift, the
// night floor and a horizon haze band; the ground below the horizon. No sun, moon or stars.
fn sky_radiance(dir: vec3<f32>) -> vec3<f32> {
    let up = max(dir.y, 0.0);
    let d = normalize(vec3<f32>(dir.x, up, dir.z) + vec3<f32>(0.0, 1e-5, 0.0));
    let cos_g = clamp(dot(d, frame.sun_sky.xyz), -1.0, 1.0);
    var c = yxy_to_rgb(frame.zenith.xyz * perez(d.y + 0.01, acos(cos_g), cos_g)) * frame.sky_params.x;
    let w = frame.sky_params.y * smoothstep(-0.05, 0.35, d.y) * (1.0 - 0.6 * smoothstep(0.5, 1.0, cos_g));
    c = mix(c, TWILIGHT_BLUE * dot(c, LUMA), w);
    c += frame.night_sky.rgb;

    let flat_len = length(dir.xz);
    let sun_flat_len = length(frame.sun_sky.xz);
    var toward = 0.0;
    if flat_len > 1e-4 && sun_flat_len > 1e-4 {
        toward = pow(max(dot(dir.xz / flat_len, frame.sun_sky.xz / sun_flat_len), 0.0), 4.0);
    }
    let haze = frame.sky_horizon.rgb * (1.0 + 0.8 * toward);
    c = mix(c, haze, frame.sky_params.z * exp(-up * 12.0));
    c = mix(frame.sky_ground.rgb, c, smoothstep(-0.03, 0.0, dir.y));
    return min(c, vec3<f32>(MAX_RADIANCE));
}
