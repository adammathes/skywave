// Shared declarations, prepended to every shader module by the renderer.
// Layout must match `FrameUniform` in src/lib.rs. Radiance is linear sRGB in kcd/m².

// Must match `MAX_LIGHTS` and `LIGHTS_PER_DRAW` in src/lib.rs.
const MAX_LIGHTS: u32 = 256u;
const LIGHTS_PER_DRAW: u32 = 8u;

// One point or spot light in 32 bytes (`PackedLight` in src/lib.rs; see `unpack_light`).
struct PackedLight {
    // xyz: position (m), w: radius (m) beyond which it adds nothing.
    pos_radius: vec4<f32>,
    // x: f16 intensity r, g (kcd); y: f16 intensity b, cosine off the axis where the cone
    // ends (below -1 for an omni light); z: spot axis, octahedral snorm16 x2; w: low half
    // f16 1 / (cosine of full intensity - cone end), bits 16..23 vertical squash
    // (1 + n / 16), bits 24..31 wrap of the surface cosine (n / 255).
    bits: vec4<u32>,
};

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // `view_proj` from a camera at the origin: meshes' instances are camera relative.
    rel_view_proj: mat4x4<f32>,
    // World-to-clip of each shadow cascade's sun projection.
    cascade_view_proj: array<mat4x4<f32>, 3>,
    // xyz: camera position (m), w: fog density (1/m).
    camera_pos: vec4<f32>,
    // xyz: unit view direction, w: smog floor height (m).
    camera_fwd: vec4<f32>,
    // xyz: far view depth of each cascade (m), w: blend band as a fraction of a cascade.
    cascade_far: vec4<f32>,
    // xyz: world size of one shadow texel per cascade (m), w: cascade edge in texels.
    cascade_texel: vec4<f32>,
    // xyz: unit vector toward the sun, w: smog scale height (m).
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
    // xyz: zenith (Y, x, y) / F(0, theta_s), w: smog tint green.
    zenith: vec4<f32>,
    // xyz: toward the sun clamped to the horizon (Perez gamma), w: smog tint red.
    sun_sky: vec4<f32>,
    // x: twilight luminance fade, y: twilight blue shift, z: horizon haze, w: smog density
    // at its floor (1/m, 0 for none).
    sky_params: vec4<f32>,
    // rgb: night sky floor radiance, w: smog tint blue.
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
    // x: emissive boost (kcd/m² per unit of emissive), y: window emissive scale 0..1,
    // z: number of lights in `lights`, w: emissive cap (post-exposure).
    emissive: vec4<f32>,
    // x: bloom strength (0 = no bloom), y: bloom threshold (post-exposure), z: vignette
    // strength, w: highest post-exposure value fed to bloom.
    post: vec4<f32>,
    // xy: swapchain size (px), zw: its reciprocal.
    screen: vec4<f32>,
    // rgb: cabin ambient radiance, scaled per instance by its cabin share.
    cabin_light: vec4<f32>,
    // x: focal length (px, vertical), y: shortest corona radius (px), z: longest corona
    // radius (px), w: unused.
    corona: vec4<f32>,
    // xy: size of the scene's color and depth targets (px; the swapchain scaled by the
    // resolution scale), zw: its reciprocal.
    render_size: vec4<f32>,
    // x: shadow cascades drawn (2 or 3), y: 1 when the sun lights (else no cascade is
    // drawn), zw: unused.
    shadow_params: vec4<f32>,
    // x: the look (`render::Look` as an index; see `look_id`), yzw: unused.
    look: vec4<f32>,
    lights: array<PackedLight, 256>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    // rgb: linear emissive color, a: intensity in the low 7 bits, window flag in bit 7.
    @location(4) emissive: vec4<f32>,
};

struct InstanceIn {
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(9) n0: vec4<f32>,
    @location(10) n1: vec4<f32>,
    @location(11) n2: vec4<f32>,
    @location(12) color: vec4<f32>,
    // x: emissive multiplier (glow) as f16 bits (bits 0-15), cabin-light share as unorm8
    // (bits 16-23), the number of lights in its set (bits 24-27) and the no-sun-shadow flag
    // (bit 28); y: how far each light of the set is dimmed, 4 bits a light, the first
    // lowest; zw: indices of those lights in `frame.lights`, one byte each, low byte first.
    @location(13) params: vec4<u32>,
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

// AgX (Troy Sobotka) as fitted by Benjamin Wrensch: inset to the AgX working space, log2
// encode over [-12.47, 4.03] EV around 0.18, a 6th-order fit of the default sigmoid, outset
// back to sRGB primaries. It rolls bright saturated light (neon, sodium, the sun) toward
// white instead of clipping it to a flat primary. Returns linear display light.
fn agx(color: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        vec3<f32>(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3<f32>(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3<f32>(0.0792237451477643, 0.0791661274605434, 0.879142973793104),
    );
    let outset = mat3x3<f32>(
        vec3<f32>(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3<f32>(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3<f32>(-0.0990297440797205, -0.0989611768448433, 1.15107367264116),
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = inset * max(color, vec3<f32>(1e-10));
    v = (clamp(log2(v), vec3<f32>(min_ev), vec3<f32>(max_ev)) - min_ev) / (max_ev - min_ev);
    let x2 = v * v;
    let x4 = x2 * x2;
    v = 15.5 * x4 * x2 - 40.14 * x4 * v + 31.96 * x4 - 6.868 * x2 * v + 0.4298 * x2
        + 0.1191 * v - 0.00232;
    // Look: base AgX lifts shadows and reads flat next to the period palette, so add
    // contrast (power 1.25) and saturation (1.2), between base and Wrensch's "punchy".
    let luma = dot(v, LUMA);
    v = pow(max(v, vec3<f32>(0.0)), vec3<f32>(1.25));
    v = max(luma + 1.2 * (v - luma), vec3<f32>(0.0));
    // The outset has negative entries, so a saturated colour can land just below zero
    // here, and pow() of a negative base is undefined (NaN on some GPUs).
    let display = max(outset * v, vec3<f32>(0.0));
    return clamp(pow(display, vec3<f32>(2.2)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Interleaved gradient noise (Jimenez 2014) in [0, 1): a fixed per-pixel pattern, so the
// dither never crawls like film grain.
fn gradient_noise(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(floor(pixel), vec2<f32>(0.06711056, 0.00583715))));
}

// ---- Looks -------------------------------------------------------------------------
// Mirrors `render::Look`: the index of each in `Look::ALL`.
const LOOK_STANDARD: u32 = 0u;
const LOOK_SMOG: u32 = 1u;
const LOOK_DUSK: u32 = 2u;
const LOOK_AMBER: u32 = 3u;
const LOOK_POSTER: u32 = 4u;
const LOOK_TOON: u32 = 5u;
const LOOK_VIDEO: u32 = 6u;
const LOOK_LOW_RES: u32 = 7u;
const LOOK_INK: u32 = 8u;
// Lines of the low-res look's picture.
const LOW_RES_LINES: f32 = 180.0;

fn look_id() -> u32 {
    return u32(frame.look.x + 0.5);
}

// Ordered dither threshold in (0, 1) for pixel `p`: the 8×8 Bayer matrix, from the bits
// of x ^ y and y interleaved and reversed.
fn bayer8(p: vec2<f32>) -> f32 {
    let y = u32(p.y) & 7u;
    let v = (u32(p.x) & 7u) ^ y;
    let b = ((v & 1u) << 5u) | ((y & 1u) << 4u) | ((v & 2u) << 2u) | ((y & 2u) << 1u)
        | ((v & 4u) >> 1u) | ((y & 4u) >> 2u);
    return (f32(b) + 0.5) / 64.0;
}

fn saturate_by(c: vec3<f32>, k: f32) -> vec3<f32> {
    let l = dot(c, LUMA);
    return max(vec3<f32>(l) + (c - vec3<f32>(l)) * k, vec3<f32>(0.0));
}

// The look's grade of a display value (sRGB-encoded, 0..1) at pixel `frag`, before the
// dither. The low-res look's `frag` is already snapped to its big pixels.
fn grade(e: vec3<f32>, frag: vec2<f32>, look: u32) -> vec3<f32> {
    var c = e;
    switch look {
        case LOOK_SMOG: {
            // Faded print: warm, a touch more color, lifted blacks, rolled-off whites, grain.
            c = saturate_by(c * vec3<f32>(1.06, 1.0, 0.86), 1.15);
            c = 0.06 + 0.9 * c;
            c += (gradient_noise(frag * 1.7 + 13.0) - 0.5) * 0.05;
        }
        case LOOK_DUSK: {
            // A few flat steps.
            c = (floor(c * 7.0) + 0.5) / 7.0;
        }
        case LOOK_AMBER: {
            // One hue: brightness picks a point on a dark-brown to hot-orange ramp.
            let l = clamp(dot(c, LUMA) * 1.25, 0.0, 1.0);
            c = mix(vec3<f32>(0.16, 0.06, 0.02), vec3<f32>(1.0, 0.64, 0.25), l);
            c = mix(c, vec3<f32>(1.0, 0.9, 0.7), smoothstep(0.85, 1.0, l));
        }
        case LOOK_POSTER: {
            c = saturate_by(c, 1.2);
        }
        case LOOK_TOON: {
            c = saturate_by(c, 1.45);
        }
        case LOOK_VIDEO: {
            // Split tone: cyan-green shadows, sodium highlights; lifted blacks; noise.
            let l = dot(c, LUMA);
            let tone = mix(vec3<f32>(0.6, 1.08, 1.0), vec3<f32>(1.25, 0.92, 0.58), smoothstep(0.08, 0.55, l));
            c = 0.045 + 0.95 * c * tone;
            c += (gradient_noise(frag * 0.93 + 7.0) - 0.5) * vec3<f32>(0.05, 0.06, 0.08);
        }
        case LOOK_LOW_RES: {
            // Seven levels a channel, ordered-dithered over the big pixels.
            let b = max(floor(frame.screen.y / LOW_RES_LINES), 1.0);
            let t = (bayer8(floor(frag / b)) - 0.5) * 0.7;
            c = floor(c * 6.0 + 0.5 + t) / 6.0;
        }
        case LOOK_INK: {
            // Paper where the dither threshold is under the brightness, ink elsewhere, in
            // dots two pixels wide.
            let l = smoothstep(0.06, 0.8, dot(c, LUMA));
            let paper = vec3<f32>(0.91, 0.886, 0.79);
            let ink = vec3<f32>(0.17, 0.165, 0.23);
            c = select(ink, paper, l > bayer8(floor(frag * 0.5)));
        }
        default: {}
    }
    return c;
}

// Post-exposure value -> display value at pixel `frag` (framebuffer coordinates):
// AgX, vignette, the look's grade, then ±half an 8-bit step of dither in sRGB-encoded
// space (where the quantization happens), decoded again when the target encodes sRGB in
// hardware.
fn display_exposed(exposed: vec3<f32>, frag: vec2<f32>) -> vec4<f32> {
    var c = agx(exposed);
    let d = frag * frame.screen.zw * 2.0 - 1.0;
    c *= 1.0 - frame.post.z * (0.5 * dot(d, d));
    let look = look_id();
    var e = linear_to_srgb(c);
    if look != LOOK_STANDARD {
        e = grade(e, frag, look);
    }
    if look != LOOK_INK && look != LOOK_LOW_RES {
        e += (gradient_noise(frag) - 0.5) / 255.0;
    }
    e = clamp(e, vec3<f32>(0.0), vec3<f32>(1.0));
    if frame.params.y > 0.5 {
        return vec4<f32>(e, 1.0);
    }
    return vec4<f32>(srgb_to_linear(e), 1.0);
}

// HDR radiance -> display value (see `display_exposed`).
fn display(radiance: vec3<f32>, frag: vec2<f32>) -> vec4<f32> {
    return display_exposed(radiance * frame.params.x, frag);
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

// Optical depth of the smog layer along a ray `dist` m long from height `y0` to `y1`: its
// density falls by e every scale height above its floor and holds below it.
fn smog_depth(y0: f32, y1: f32, dist: f32) -> f32 {
    let rho = frame.sky_params.w;
    if rho <= 0.0 {
        return 0.0;
    }
    let h = frame.sun_dir.w;
    let a = max(y0 - frame.camera_fwd.w, 0.0);
    let b = max(y1 - frame.camera_fwd.w, 0.0);
    let dy = b - a;
    var mean = exp(-0.5 * (a + b) / h);
    if abs(dy) > 0.5 {
        mean = h * (exp(-a / h) - exp(-b / h)) / dy;
    }
    return rho * dist * mean;
}

// The smog's in-scatter: the horizon sky's radiance, tinted.
fn smog_color() -> vec3<f32> {
    let tint = vec3<f32>(frame.sun_sky.w, frame.zenith.w, frame.night_sky.w);
    return frame.sky_horizon.rgb * tint;
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
