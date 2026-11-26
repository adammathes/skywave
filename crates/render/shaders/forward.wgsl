// Forward pass: Lambert-lit, atlas-textured meshes with cascaded sun
// shadows, hemisphere sky ambient (plus cabin light for a car's interior), the point and
// spot lights of each instance's light set (up to 8 of the frame's 256), the emissive
// channel (soft-capped after exposure) and aerial-perspective fog, then the analytic sky
// behind them.
//
// Each fragment stage has two entry points: `fs_*` writes HDR radiance (Rgba16Float target,
// tonemapped later) and `fs_*_ldr` tonemaps in place for targets without float rendering.

@group(1) @binding(0) var shadow_atlas: texture_depth_2d;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;
@group(1) @binding(2) var atlas: texture_2d<f32>;
@group(1) @binding(3) var atlas_sampler: sampler;

// ---- Sky ------------------------------------------------------------------------------

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> SkyOut {
    let p = fullscreen_corner(index);
    var out: SkyOut;
    // At the far plane: drawn after the meshes, it passes the depth test only where none is.
    out.clip = vec4<f32>(p, 1.0, 1.0);
    out.ndc = p;
    return out;
}

fn hash_u32(v: u32) -> u32 {
    var h = v * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

fn hash_float(v: u32) -> f32 {
    return f32(hash_u32(v) >> 8u) / 16777216.0;
}

// A hash-based star field fixed in world space: one candidate star per 1/150 rad cell of a
// 3D grid around the camera, about 1 in 128 cells lit.
fn stars(dir: vec3<f32>) -> vec3<f32> {
    let grid = 150.0;
    let cell = floor(dir * grid);
    let ci = vec3<i32>(cell);
    let h = hash_u32(u32(ci.x) ^ hash_u32(u32(ci.y) ^ hash_u32(u32(ci.z))));
    if (h & 127u) != 0u {
        return vec3<f32>(0.0);
    }
    let jitter = vec3<f32>(hash_float(h ^ 1u), hash_float(h ^ 2u), hash_float(h ^ 3u)) - 0.5;
    let star = normalize(cell + 0.5 + jitter * 0.5);
    let d = length(dir - star);
    let magnitude = pow(hash_float(h ^ 4u), 6.0);
    let tint = mix(vec3<f32>(1.0, 0.82, 0.62), vec3<f32>(0.75, 0.85, 1.0), hash_float(h ^ 5u));
    // Peak radiance of the brightest stars is ~3 cd/m² over a pixel-sized gaussian.
    let core = exp(-(d * d) / (2.0 * 0.0007 * 0.0007));
    return tint * (0.003 * (0.04 + magnitude) * core);
}

// Local disc coordinates for a body at `center` (unit) with angular radius `cos_r`:
// xy in [-1, 1] on the disc, z = 1 inside; frame is (right, up) around the body.
fn disc_coords(dir: vec3<f32>, center: vec3<f32>, cos_r: f32) -> vec3<f32> {
    let sin_r = sqrt(max(1.0 - cos_r * cos_r, 1e-12));
    var right = cross(center, vec3<f32>(0.0, 1.0, 0.0));
    if dot(right, right) < 1e-8 {
        right = vec3<f32>(1.0, 0.0, 0.0);
    }
    right = normalize(right);
    let up = cross(right, center);
    let q = vec2<f32>(dot(dir, right), dot(dir, up)) / sin_r;
    return vec3<f32>(q, select(0.0, 1.0, dot(dir, center) > 0.0));
}

// ---- Fog bank (see `render::FogBank`) ---------------------------------------------------

// Half widths of the bank's soft edges: across the front and at the top (m).
const BANK_FRONT_EDGE: f32 = 200.0;
const BANK_TOP_EDGE: f32 = 15.0;
// Farthest a sky ray is followed through the bank (m).
const BANK_SKY_REACH: f32 = 6000.0;
const BANK_SAMPLES: i32 = 8;
// Luminance the city's lamps give the bank by night (kcd/m²).
const BANK_CITY_GLOW: f32 = 0.0008;

// How much of the bank is at `p`: 1 inside, 0 outside.
fn bank_at(p: vec3<f32>) -> f32 {
    let b = frame.fog_bank;
    let across = clamp((b.x - p.x) / (2.0 * BANK_FRONT_EDGE) + 0.5, 0.0, 1.0);
    let below = clamp((b.y - p.y) / (2.0 * BANK_TOP_EDGE) + 0.5, 0.0, 1.0);
    return across * below;
}

// The bank's color: the sky's light from above and the sun's on it, greyer than either.
fn bank_color() -> vec3<f32> {
    let sun = frame.sun_light.rgb * clamp(frame.sun_dir.y * 6.0, 0.0, 1.0);
    let c = 1.1 * frame.sky_up.rgb + 0.6 * sun;
    let grey = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    // By night it glows with the city's lamps under it (kcd/m², as the window scale rises).
    let city = vec3<f32>(1.0, 0.72, 0.48) * (BANK_CITY_GLOW * frame.emissive.y);
    return mix(vec3<f32>(grey), c, 0.25) + city;
}

// `c` seen from `eye` at `at` through the bank: the ray is clipped to the bank's box (west
// of the front's outer edge, below the top's), then its cover is the mean of a few samples.
fn fog_bank(c: vec3<f32>, eye: vec3<f32>, at: vec3<f32>) -> vec3<f32> {
    let b = frame.fog_bank;
    if b.z <= 0.0 {
        return c;
    }
    let d = at - eye;
    var t0 = 0.0;
    var t1 = 1.0;
    let xmax = b.x + BANK_FRONT_EDGE;
    let ymax = b.y + BANK_TOP_EDGE;
    if abs(d.x) > 1e-4 {
        let t = (xmax - eye.x) / d.x;
        if d.x > 0.0 { t1 = min(t1, t); } else { t0 = max(t0, t); }
    } else if eye.x > xmax {
        return c;
    }
    if abs(d.y) > 1e-4 {
        let t = (ymax - eye.y) / d.y;
        if d.y > 0.0 { t1 = min(t1, t); } else { t0 = max(t0, t); }
    } else if eye.y > ymax {
        return c;
    }
    if t1 <= t0 {
        return c;
    }
    var cover = 0.0;
    for (var i = 0; i < BANK_SAMPLES; i++) {
        let t = mix(t0, t1, (f32(i) + 0.5) / f32(BANK_SAMPLES));
        cover += bank_at(eye + d * t);
    }
    let depth = b.z * length(d) * (t1 - t0) * cover / f32(BANK_SAMPLES);
    return mix(c, bank_color(), 1.0 - exp(-depth));
}

fn sky_full(dir: vec3<f32>) -> vec3<f32> {
    var c = sky_radiance(dir);
    let above = smoothstep(-0.002, 0.004, dir.y);

    // Stars, dimmed toward the horizon by extinction.
    c += stars(dir) * (frame.moon_light.z * smoothstep(0.0, 0.25, dir.y));

    // Moon: a sphere lit from the sun's direction, so its phase follows from geometry,
    // with a few darker maria and faint earthshine on the unlit part.
    let moon = disc_coords(dir, frame.moon_dir.xyz, frame.moon_dir.w);
    let mr2 = dot(moon.xy, moon.xy);
    if moon.z > 0.0 && mr2 < 1.0 && frame.moon_light.x > 0.0 {
        var right = cross(frame.moon_dir.xyz, vec3<f32>(0.0, 1.0, 0.0));
        if dot(right, right) < 1e-8 {
            right = vec3<f32>(1.0, 0.0, 0.0);
        }
        right = normalize(right);
        let up = cross(right, frame.moon_dir.xyz);
        let n = right * moon.x + up * moon.y - frame.moon_dir.xyz * sqrt(1.0 - mr2);
        let lit = max(dot(n, frame.sun_dir.xyz), 0.0);
        var albedo = 1.0;
        albedo -= 0.28 * smoothstep(0.35, 0.15, length(moon.xy - vec2<f32>(-0.25, 0.25)));
        albedo -= 0.22 * smoothstep(0.30, 0.10, length(moon.xy - vec2<f32>(0.20, 0.35)));
        albedo -= 0.20 * smoothstep(0.25, 0.08, length(moon.xy - vec2<f32>(0.10, -0.15)));
        let edge = 1.0 - smoothstep(0.92, 1.0, sqrt(mr2));
        let l = frame.moon_light.x * albedo * (lit + frame.moon_light.y) * edge;
        // The sky scatters in front of the moon, so its light only adds.
        c += vec3<f32>(1.0, 0.96, 0.9) * (l * above);
    }

    // Sun disc with limb darkening and a soft edge.
    let sun = disc_coords(dir, frame.sun_dir.xyz, frame.sun_disc.w);
    let sr2 = dot(sun.xy, sun.xy);
    if sun.z > 0.0 && sr2 < 1.0 {
        let mu = sqrt(1.0 - sr2);
        let limb = 1.0 - 0.6 * (1.0 - mu);
        let edge = 1.0 - smoothstep(0.8, 1.0, sqrt(sr2));
        c += frame.sun_disc.rgb * (limb * edge * above);
    }
    return min(c, vec3<f32>(MAX_RADIANCE));
}

// The sky as the look draws it.
fn look_sky_full(dir: vec3<f32>) -> vec3<f32> {
    let look = look_id();
    if look == LOOK_STANDARD {
        return sky_full(dir);
    }
    return look_sky(look, sky_full(dir), dir);
}

// The sky toward `dir`, through the bank out to `BANK_SKY_REACH`.
fn banked_sky(dir: vec3<f32>) -> vec3<f32> {
    let eye = frame.camera_pos.xyz;
    return fog_bank(look_sky_full(dir), eye, eye + dir * BANK_SKY_REACH);
}

fn sky_dir(ndc: vec2<f32>) -> vec3<f32> {
    let far = frame.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    return normalize(far.xyz / far.w - frame.camera_pos.xyz);
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    return vec4<f32>(banked_sky(sky_dir(in.ndc)), 1.0);
}

@fragment
fn fs_sky_ldr(in: SkyOut) -> @location(0) vec4<f32> {
    return display(banked_sky(sky_dir(in.ndc)), in.clip.xy);
}

// ---- Meshes ---------------------------------------------------------------------------

struct MeshOut {
    // Invariant, as in `vs_depth`: the pre-pass and this pass write the same depth.
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    // rgb: emissive radiance (kcd/m²), never tinted by the instance or the atlas.
    @location(4) emissive: vec3<f32>,
    // Share of the frame's cabin light.
    @location(5) cabin: f32,
    // x: lights in the instance's set (+16: no sun shadow), y: how far each is dimmed, 4
    // bits a light (15 = off), z, w: their indices, a byte each.
    @location(6) @interpolate(flat) lights: vec4<u32>,
};

// Depth pre-pass: depth only, with `vs_mesh`'s position math, so the forward pass's
// less-or-equal test passes only for the nearest surface and each pixel shades once.
@vertex
fn vs_depth(v: VertexIn, i: InstanceIn) -> @invariant @builtin(position) vec4<f32> {
    return frame.rel_view_proj * (instance_model(i) * vec4<f32>(v.pos, 1.0));
}

// Writes nothing (its target's write mask is empty); the pass needs a fragment stage.
@fragment
fn fs_depth() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}

// Soft cap on emissive radiance in post-exposure terms: unchanged well under the cap,
// approaching it far over (a fourth-power knee), hue kept.
fn cap_emissive(e: vec3<f32>) -> vec3<f32> {
    let cap = frame.emissive.w;
    let m = max(e.r, max(e.g, e.b)) * frame.params.x / cap;
    if m <= 0.0 {
        return e;
    }
    return e * inverseSqrt(sqrt(1.0 + m * m * m * m));
}

@vertex
fn vs_mesh(v: VertexIn, i: InstanceIn) -> MeshOut {
    // Camera relative (the instance's translation is), back in the world for lighting.
    let rel = instance_model(i) * vec4<f32>(v.pos, 1.0);
    let normal_matrix = mat3x3<f32>(i.n0.xyz, i.n1.xyz, i.n2.xyz);
    var out: MeshOut;
    out.clip = frame.rel_view_proj * rel;
    out.world_pos = rel.xyz + frame.camera_pos.xyz;
    out.normal = normal_matrix * v.normal;
    out.color = v.color * i.color;
    out.uv = v.uv;
    // Alpha: 7-bit intensity (0..127 -> 0..8x) plus the window flag in bit 7.
    let a = u32(round(v.emissive.a * 255.0));
    let window = select(1.0, frame.emissive.y, (a & 128u) != 0u);
    let scale = f32(a & 127u) * (8.0 / 127.0);
    let glow = unpack2x16float(i.params.x).x;
    out.emissive = cap_emissive(v.emissive.rgb * (scale * window * frame.emissive.x * glow));
    out.cabin = f32((i.params.x >> 16u) & 255u) * (1.0 / 255.0);
    let flags = (i.params.x >> 24u) & 31u;
    out.lights = vec4<u32>(min(flags & 15u, LIGHTS_PER_DRAW) | (flags & 16u), i.params.y, i.params.z, i.params.w);
    return out;
}

struct Light {
    pos: vec3<f32>,
    // 1 / radius².
    inv_r2: f32,
    color: vec3<f32>,
    // Spot axis (unit) and the cosine off it where the cone ends (below -1: omni).
    axis: vec3<f32>,
    cone_end: f32,
    // 1 / (cosine of full intensity - cone_end).
    cone_scale: f32,
    squash: f32,
    wrap: f32,
};

// Octahedral unit vector (Cigolle et al. 2014) from two snorm values.
fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var v = vec3<f32>(e, 1.0 - abs(e.x) - abs(e.y));
    if v.z < 0.0 {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), v.xy >= vec2<f32>(0.0));
        v = vec3<f32>((1.0 - abs(v.yx)) * s, v.z);
    }
    return normalize(v);
}

// Mirrors `PackedLight::new` in src/lib.rs.
fn unpack_light(p: PackedLight) -> Light {
    var l: Light;
    l.pos = p.pos_radius.xyz;
    l.inv_r2 = 1.0 / max(p.pos_radius.w * p.pos_radius.w, 1e-6);
    let rg = unpack2x16float(p.bits.x);
    let b_end = unpack2x16float(p.bits.y);
    l.color = vec3<f32>(rg, b_end.x);
    l.cone_end = b_end.y;
    l.axis = oct_decode(unpack2x16snorm(p.bits.z));
    l.cone_scale = unpack2x16float(p.bits.w & 0xffffu).x;
    l.squash = 1.0 + f32((p.bits.w >> 16u) & 255u) * (1.0 / 16.0);
    l.wrap = f32(p.bits.w >> 24u) * (1.0 / 255.0);
    return l;
}

// Cosine of the angle off a spot's `axis` toward unit `d`, with angles above and below the
// axis (against world up) stretched `squash` times: a beam flatter than it is wide.
fn spot_cos(d: vec3<f32>, axis: vec3<f32>, squash: f32) -> f32 {
    let f = dot(d, axis);
    if squash <= 1.0 {
        return f;
    }
    let right = normalize(cross(axis, vec3<f32>(0.0, 1.0, 0.0)));
    let up = cross(right, axis);
    let u = dot(d, up) * squash;
    let r = dot(d, right);
    return f * inverseSqrt(max(f * f + u * u + r * r, 1e-8));
}

// Smooth inverse-square point lights (Karis 2013 window) of the instance's light set
// (`light_set`: count and how far each is dimmed, then the indices a byte each):
// illuminance in klux, reaching 0 at each light's radius. The 0.1 m² floor stands for the
// bulb's size. A spot light also fades from full intensity to nothing across its cone
// (smoothstep in the cosine off its axis, optionally flattened vertically) and may soften
// the surface cosine (wrap); an omni light's cone covers every direction.
fn point_lights(world_pos: vec3<f32>, n: vec3<f32>, light_set: vec4<u32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var k = 0u; k < (light_set.x & 15u); k++) {
        let word = select(light_set.w, light_set.z, k < 4u);
        let index = min((word >> ((k & 3u) * 8u)) & 255u, MAX_LIGHTS - 1u);
        let packed = frame.lights[index];
        let to_light = packed.pos_radius.xyz - world_pos;
        let d2 = dot(to_light, to_light);
        let r2 = packed.pos_radius.w * packed.pos_radius.w;
        if d2 < r2 {
            let light = unpack_light(packed);
            let ratio = d2 * light.inv_r2;
            let window = (1.0 - ratio * ratio) * (1.0 - ratio * ratio);
            let l = to_light * inverseSqrt(max(d2, 1e-8));
            let ndl = max((dot(n, l) + light.wrap) / (1.0 + light.wrap), 0.0);
            let off = spot_cos(-l, light.axis, light.squash);
            let c = clamp((off - light.cone_end) * light.cone_scale, 0.0, 1.0);
            let cone = c * c * (3.0 - 2.0 * c);
            let weight = 1.0 - f32((light_set.y >> (4u * k)) & 15u) * (1.0 / 15.0);
            sum += light.color * (weight * ndl * window * cone / (d2 + 0.1));
        }
    }
    return sum;
}

// 3×3 PCF with hardware 2×2 comparison filtering (an effective 4×4 texel kernel) in one
// cascade of the shadow atlas (`shadow_params.x` cascades side by side). 1 = lit.
fn cascade_visibility(index: i32, world_pos: vec3<f32>, n: vec3<f32>, ndl: f32) -> f32 {
    let texel = frame.cascade_texel[index];
    // Normal offset (bigger at grazing light) plus a push toward the sun, both in texels of
    // this cascade, so the bias is the same on screen in every cascade.
    let offset = n * (texel * frame.params.w * clamp(1.0 - ndl, 0.25, 1.0))
        + frame.sun_dir.xyz * (texel * frame.params.z);
    let p = frame.cascade_view_proj[index] * vec4<f32>(world_pos + offset, 1.0);
    let ndc = p.xyz / p.w;
    let res = frame.cascade_texel.w;
    // Keep the 4×4 kernel inside this cascade's slice of the atlas.
    let margin = 2.0 / res;
    let local = clamp(vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5), vec2<f32>(margin), vec2<f32>(1.0 - margin));
    let cascades = frame.shadow_params.x;
    let uv = vec2<f32>((local.x + f32(index)) / cascades, local.y);
    let step = vec2<f32>(1.0 / (cascades * res), 1.0 / res);
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<f32>(f32(x), f32(y)) * step;
            sum += textureSampleCompareLevel(shadow_atlas, shadow_sampler, uv + o, ndc.z);
        }
    }
    return select(sum / 9.0, 1.0, ndc.z >= 1.0);
}

// Picks the cascade by view depth and cross-fades into the next one over the last
// `cascade_far.w` of each cascade; shadows fade out at the end of the last one (with two
// cascades, `cascade_far.z` equals `cascade_far.y`, so the third is never picked).
fn sun_visibility(world_pos: vec3<f32>, n: vec3<f32>, ndl: f32) -> f32 {
    let depth = dot(world_pos - frame.camera_pos.xyz, frame.camera_fwd.xyz);
    let far = frame.cascade_far.xyz;
    if depth >= far.z {
        return 1.0;
    }
    var index = 2;
    if depth < far.y {
        index = 1;
    }
    if depth < far.x {
        index = 0;
    }
    let start = select(select(far.y, far.x, index == 1), 0.0, index == 0);
    let end = far[index];
    let band = frame.cascade_far.w * (end - start);
    let t = smoothstep(end - band, end, depth);
    var vis = cascade_visibility(index, world_pos, n, ndl);
    if t > 0.0 {
        var next = 1.0;
        if index < i32(frame.shadow_params.x) - 1 {
            next = cascade_visibility(index + 1, world_pos, n, ndl);
        }
        vis = mix(vis, next, t);
    }
    return vis;
}

// Vibrance on a linear albedo: a muted colour (pastel stucco, dry lawn, a car's paint) gets
// up to VIBRANCE more chroma, an already bold one (a red band, neon trim) little, a grey
// (asphalt, concrete) none, so the palette reads bright without the paint going neon.
const VIBRANCE: f32 = 0.8;

fn vivid(a: vec3<f32>) -> vec3<f32> {
    let hi = max(a.r, max(a.g, a.b));
    let lo = min(a.r, min(a.g, a.b));
    let sat = (hi - lo) / max(hi, 1e-4);
    let l = dot(a, LUMA);
    return clamp(vec3<f32>(l) + (a - vec3<f32>(l)) * (1.0 + VIBRANCE * (1.0 - sat)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn shade(in: MeshOut) -> vec3<f32> {
    // Sampled first, in uniform control flow, so implicit derivatives are valid.
    let texel = textureSample(atlas, atlas_sampler, in.uv);
    let albedo = vivid(in.color.rgb) * texel.rgb;
    let n = normalize(in.normal);
    let ndl = max(dot(n, frame.sun_dir.xyz), 0.0);
    // 1: unshadowed (it only ever scales ndl); the no-sun-shadow flag (16 in lights.x, bit 28
    // of params.x) skips the test, as does a sun below the horizon (no cascades are drawn).
    var vis = 1.0;
    if ndl > 0.0 && frame.shadow_params.y > 0.0 && (in.lights.x & 16u) == 0u {
        vis = sun_visibility(in.world_pos, n, ndl);
    }
    // Hemisphere ambient from the sky model: mean sky radiance above, lit ground below. A car's
    // interior adds the cabin light, full on surfaces facing up (the dash, seats, wheel) and a
    // tenth on those facing down (the headliner): it comes in through the glass and off the
    // dash, not from the roof.
    let ambient = mix(frame.sky_ground.rgb, frame.sky_up.rgb, n.y * 0.5 + 0.5)
        + frame.cabin_light.rgb * (in.cabin * (0.55 + 0.45 * n.y));
    // Point-light illuminance / pi is the luminance of a white Lambertian surface, as for
    // the sun.
    let lamps = point_lights(in.world_pos, n, in.lights) * (1.0 / 3.14159265);
    let look = look_id();
    var c: vec3<f32>;
    if look == LOOK_STANDARD {
        c = albedo * (frame.sun_light.rgb * (ndl * vis) + ambient + lamps);
    } else {
        c = look_light(look, albedo, in.color.rgb, ndl * vis, ambient, lamps);
    }
    // Self-lit color, untouched by texture and instance tint; fog still dims it.
    c += in.emissive;

    let to_frag = in.world_pos - frame.camera_pos.xyz;
    let dist = length(to_frag);
    let density = frame.camera_pos.w * look_fog_scale(look);
    let fog = 1.0 - exp(-dist * density);
    // A fog thicker than any weather's (a morning haze, over 0.002/m) lies on the ground
    // too: its in-scatter fades into the ground's over a wider band below the horizon, so
    // no line marks the eye height. Thinner fog keeps the sky's own band exactly.
    var d = to_frag / max(dist, 1e-4);
    let band = 0.03 + 0.5 * clamp((density - 0.002) / 0.002, 0.0, 1.0);
    d.y = select(d.y, d.y * (0.03 / band), d.y < 0.0);
    if look == LOOK_DUSK {
        c = dusk_layers(c, dist);
    } else {
        c = mix(c, look_sky(look, sky_radiance(d), d), fog);
    }
    c = fog_bank(c, frame.camera_pos.xyz, in.world_pos);
    return min(c, vec3<f32>(MAX_RADIANCE));
}

// ---- Looks (see `render::Look`) ---------------------------------------------------------

// A look's sun, ambient and lamp light on `albedo` (`color` is the albedo without the
// atlas's pattern), given the sun's share `sun` (cosine times shadow visibility).
fn look_light(look: u32, albedo: vec3<f32>, color: vec3<f32>, sun: f32, ambient: vec3<f32>, lamps: vec3<f32>) -> vec3<f32> {
    var s = sun;
    var a = albedo;
    var sun_tint = vec3<f32>(1.0);
    var shade_tint = vec3<f32>(1.0);
    switch look {
        case LOOK_SMOG: {
            sun_tint = vec3<f32>(1.3, 1.06, 0.72);
            shade_tint = vec3<f32>(0.78, 0.98, 1.08);
        }
        case LOOK_POSTER: {
            // Lit or not, no falloff; shadow in one violet tone; a dozen paint colors and
            // no pattern.
            s = select(0.0, 1.0, sun > 0.2);
            shade_tint = vec3<f32>(0.8, 0.72, 1.2);
            a = poster_paint(color);
        }
        case LOOK_TOON: {
            s = select(select(0.0, 0.5, sun > 0.1), 1.0, sun > 0.45);
        }
        default: {}
    }
    return a * (frame.sun_light.rgb * (s * sun_tint) + ambient * shade_tint + lamps);
}

// The poster look's paint: the nearest of a dozen colors to the albedo with its color
// pushed up, compared in sRGB.
fn poster_paint(albedo: vec3<f32>) -> vec3<f32> {
    var paints = array<vec3<f32>, 12>(
        vec3<f32>(0.957, 0.937, 0.894), vec3<f32>(0.914, 0.847, 0.722),
        vec3<f32>(0.184, 0.714, 0.816), vec3<f32>(0.937, 0.353, 0.541),
        vec3<f32>(0.231, 0.561, 0.353), vec3<f32>(0.173, 0.169, 0.2),
        vec3<f32>(0.91, 0.639, 0.239), vec3<f32>(0.839, 0.271, 0.196),
        vec3<f32>(0.541, 0.561, 0.588), vec3<f32>(0.788, 0.761, 0.702),
        vec3<f32>(0.122, 0.435, 0.545), vec3<f32>(0.369, 0.294, 0.337),
    );
    let e = saturate_by(linear_to_srgb(clamp(albedo, vec3<f32>(0.0), vec3<f32>(1.0))), 1.4);
    var best = paints[0];
    var best_d = 1e9;
    for (var i = 0u; i < 12u; i++) {
        let v = e - paints[i];
        let dd = dot(v, v);
        if dd < best_d {
            best_d = dd;
            best = paints[i];
        }
    }
    return srgb_to_linear(best);
}

// How much thicker a look's fog is than the weather's.
fn look_fog_scale(look: u32) -> f32 {
    switch look {
        case LOOK_SMOG: {
            return 3.0;
        }
        case LOOK_AMBER: {
            return 14.0;
        }
        default: {
            return 1.0;
        }
    }
}

// A look's color for the sky radiance `c` toward `dir` (also the fog's color).
fn look_sky(look: u32, c: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    let l = dot(c, LUMA);
    switch look {
        case LOOK_SMOG: {
            // A brown-gold smog band low, the sky's own blue faded above it.
            let low = 1.0 - smoothstep(0.0, 0.35, dir.y);
            return mix(mix(c, vec3<f32>(l), 0.3), l * vec3<f32>(1.3, 1.0, 0.62), low);
        }
        case LOOK_AMBER: {
            return l * vec3<f32>(1.7, 0.85, 0.3);
        }
        case LOOK_DUSK: {
            // Warm near the horizon, fading to pale gold above.
            let t = smoothstep(0.0, 0.5, dir.y);
            return l * mix(vec3<f32>(1.75, 0.82, 0.45), vec3<f32>(1.35, 1.05, 0.7), t);
        }
        default: {
            return c;
        }
    }
}

// The dusk look's distance color: five stops from dark plum near to pale gold far, in
// place of the fog, lit by the horizon so it follows the time of day.
fn dusk_layers(c: vec3<f32>, dist: f32) -> vec3<f32> {
    let t = clamp(log(max(dist, 15.0) / 15.0) / log(120.0), 0.0, 1.0);
    var stops = array<vec3<f32>, 5>(
        vec3<f32>(0.165, 0.086, 0.192), vec3<f32>(0.478, 0.184, 0.271),
        vec3<f32>(0.816, 0.341, 0.243), vec3<f32>(0.945, 0.604, 0.298),
        vec3<f32>(0.984, 0.827, 0.541),
    );
    let x = t * 4.0;
    let i = u32(min(floor(x), 3.0));
    let ramp = srgb_to_linear(mix(stops[i], stops[i + 1u], x - f32(i)));
    let horizon = dot(frame.sky_horizon.rgb, LUMA);
    let w = clamp(t * 1.6, 0.0, 1.0);
    return mix(c, ramp * (horizon * 1.6), 0.3 + 0.7 * w);
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in), 1.0);
}

@fragment
fn fs_mesh_ldr(in: MeshOut) -> @location(0) vec4<f32> {
    return display(shade(in), in.clip.xy);
}

// Window glass: the instance color's alpha, rising toward opaque at grazing angles where
// glass reflects most (Schlick's curve).
fn glass_alpha(in: MeshOut) -> f32 {
    let v = normalize(frame.camera_pos.xyz - in.world_pos);
    let f = pow(1.0 - clamp(abs(dot(normalize(in.normal), v)), 0.0, 1.0), 5.0);
    return mix(in.color.a, 1.0, f);
}

@fragment
fn fs_glass(in: MeshOut) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in), glass_alpha(in));
}

@fragment
fn fs_glass_ldr(in: MeshOut) -> @location(0) vec4<f32> {
    return vec4<f32>(display(shade(in), in.clip.xy).rgb, glass_alpha(in));
}
