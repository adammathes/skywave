// Coronas: a soft additive glow sprite over each bright light (street lamp heads, car
// lamps, neon, the sun and moon discs), drawn after the forward pass and before bloom, so
// bloom spreads them a little further. One instanced draw of camera-facing quads.
//
// A sprite keeps its world size (a lamp's halo shrinks with distance) down to a shortest
// screen radius, below which it dims instead of shrinking. Visibility comes from the
// scene's depth: five samples (the center and four offsets around it), each one counting
// when nothing in front of the light was drawn there, fade the sprite by the share seen,
// so a lamp passing behind a pole or a palm fades instead of popping. The vertex shader
// does the test once per corner; the fragment shader only draws the profile.

// The scene's depth, read through a nearest, non-comparison sampler: on GL a float depth
// texture is incomplete (reads 0) unless sampled with nearest filtering.
@group(1) @binding(0) var scene_depth: texture_2d<f32>;
@group(1) @binding(1) var depth_sampler: sampler;

struct CoronaIn {
    // xyz: world position (m), w: radius (m); or, for a body at infinity (the sun, the
    // moon), xyz the unit direction toward it and w minus its angular radius (rad).
    @location(0) pos_size: vec4<f32>,
    // rgb: radiance at the center (kcd/m²), w: unused.
    @location(1) color: vec4<f32>,
};

struct CoronaOut {
    @builtin(position) clip: vec4<f32>,
    // Offset from the center, in sprite radii.
    @location(0) local: vec2<f32>,
    // Radiance at the center times every fade.
    @location(1) @interpolate(flat) color: vec3<f32>,
};

// A light hidden behind something at least this far in front of it (m) is occluded: a
// lamp's own head and lens sit closer than this to its light.
const CORONA_DEPTH_MARGIN: f32 = 0.4;

// 1 when the scene depth at pixel `pix` is not in front of `limit` (NDC depth), 0 when
// it is or when `pix` is off the screen.
fn depth_clear(pix: vec2<f32>, limit: f32) -> f32 {
    let size = vec2<f32>(textureDimensions(scene_depth));
    if any(pix < vec2<f32>(0.0)) || any(pix >= size) {
        return 0.0;
    }
    let d = textureSampleLevel(scene_depth, depth_sampler, (floor(pix) + 0.5) / size, 0.0).r;
    return select(0.0, 1.0, d >= limit);
}

@vertex
fn vs_corona(@builtin(vertex_index) index: u32, c: CoronaIn) -> CoronaOut {
    var out: CoronaOut;
    // Degenerate and off screen unless the light turns out to be visible.
    out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    out.local = vec2<f32>(0.0);
    out.color = vec3<f32>(0.0);

    var p: vec4<f32>;
    var toward: vec3<f32>;
    var radius_px: f32;
    var limit: f32;
    var fade = 1.0;
    if c.pos_size.w < 0.0 {
        // At infinity: hidden by anything drawn at all (the sky leaves the far plane).
        toward = c.pos_size.xyz;
        p = frame.view_proj * vec4<f32>(toward, 0.0);
        radius_px = -c.pos_size.w * frame.corona.x;
        limit = 1.0;
    } else {
        let rel = c.pos_size.xyz - frame.camera_pos.xyz;
        let dist = max(length(rel), 1e-3);
        toward = rel / dist;
        p = frame.view_proj * vec4<f32>(c.pos_size.xyz, 1.0);
        radius_px = c.pos_size.w * frame.corona.x / max(p.w, 1e-3);
        let margin = min(max(CORONA_DEPTH_MARGIN, 0.02 * dist), 0.5 * dist);
        let q = frame.view_proj * vec4<f32>(c.pos_size.xyz - toward * margin, 1.0);
        limit = q.z / max(q.w, 1e-6);
        // Aerial perspective dims it like the lamp itself.
        fade = exp(-dist * frame.camera_pos.w);
    }
    if p.w <= 1e-4 {
        return out;
    }
    // Fades out as the light swings toward the side of and behind the camera.
    fade *= smoothstep(0.0, 0.3, dot(toward, frame.camera_fwd.xyz));
    let r = clamp(radius_px, frame.corona.y, frame.corona.z);
    // Held at the shortest radius, a distant halo dims instead of shrinking.
    fade *= sqrt(clamp(radius_px / r, 0.0, 1.0));

    let ndc = p.xy / p.w;
    let pix = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * frame.render_size.xy;
    let spread = clamp(0.2 * r, 1.0, 6.0);
    let seen = depth_clear(pix, limit)
        + depth_clear(pix + vec2<f32>(spread, 0.0), limit)
        + depth_clear(pix - vec2<f32>(spread, 0.0), limit)
        + depth_clear(pix + vec2<f32>(0.0, spread), limit)
        + depth_clear(pix - vec2<f32>(0.0, spread), limit);
    fade *= seen * 0.2;
    if fade <= 1e-4 {
        return out;
    }
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u)) * 2.0 - 1.0;
    out.clip = vec4<f32>(ndc + corner * (2.0 * r) * frame.render_size.zw, 0.0, 1.0);
    out.local = corner;
    out.color = c.color.rgb * fade;
    return out;
}

// Brightness across the sprite, 1 at the center and 0 at the rim: a small hot core over
// a wide, soft skirt, so it reads as a halo round the light rather than a disc.
fn corona_profile(local: vec2<f32>) -> f32 {
    let r2 = dot(local, local);
    if r2 >= 1.0 {
        return 0.0;
    }
    let rim = (1.0 - r2) * (1.0 - r2);
    let r = sqrt(r2);
    let skirt = (1.0 - r) * (1.0 - r);
    return rim * (0.35 * exp(-r2 * 30.0) + 0.65 * skirt * skirt);
}

@fragment
fn fs_corona(in: CoronaOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color * corona_profile(in.local), 0.0);
}

// Without a float target the frame is already display-encoded: add a soft-clipped
// display value instead (an approximation; the halo has no bloom to sit in either).
@fragment
fn fs_corona_ldr(in: CoronaOut) -> @location(0) vec4<f32> {
    let x = in.color * (corona_profile(in.local) * frame.params.x);
    var c = vec3<f32>(1.0) - exp(-x);
    if frame.params.y > 0.5 {
        c = linear_to_srgb(c);
    }
    return vec4<f32>(c, 0.0);
}
