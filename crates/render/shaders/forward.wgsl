// Forward pass: the analytic sky, then Lambert-lit, atlas-textured meshes with cascaded sun
// shadows, hemisphere sky ambient (plus cabin light for a car's interior), up to 8 point
// and spot lights, the emissive channel (soft-capped after exposure) and aerial-perspective
// fog.
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
    out.clip = vec4<f32>(p, 0.0, 1.0);
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

fn sky_dir(ndc: vec2<f32>) -> vec3<f32> {
    let far = frame.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    return normalize(far.xyz / far.w - frame.camera_pos.xyz);
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    return vec4<f32>(sky_full(sky_dir(in.ndc)), 1.0);
}

@fragment
fn fs_sky_ldr(in: SkyOut) -> @location(0) vec4<f32> {
    return display(sky_full(sky_dir(in.ndc)), in.clip.xy);
}

// ---- Meshes ---------------------------------------------------------------------------

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    // rgb: emissive radiance (kcd/m²), never tinted by the instance or the atlas.
    @location(4) emissive: vec3<f32>,
    // Share of the frame's cabin light.
    @location(5) cabin: f32,
};

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
    let world = instance_model(i) * vec4<f32>(v.pos, 1.0);
    let normal_matrix = mat3x3<f32>(i.n0.xyz, i.n1.xyz, i.n2.xyz);
    var out: MeshOut;
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normal_matrix * v.normal;
    out.color = v.color * i.color;
    out.uv = v.uv;
    // Alpha: 7-bit intensity (0..127 -> 0..8x) plus the window flag in bit 7.
    let a = u32(round(v.emissive.a * 255.0));
    let window = select(1.0, frame.emissive.y, (a & 128u) != 0u);
    let scale = f32(a & 127u) * (8.0 / 127.0);
    out.emissive = cap_emissive(v.emissive.rgb * (scale * window * frame.emissive.x * i.params.x));
    out.cabin = i.params.y;
    return out;
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

// Smooth inverse-square point lights (Karis 2013 window): illuminance in klux, reaching 0
// at each light's radius. The 0.1 m² floor stands for the bulb's size. A spot light also
// fades from full intensity to nothing across its cone (smoothstep in the cosine off its
// axis, optionally flattened vertically) and may soften the surface cosine (wrap); an omni
// light's cone covers every direction.
fn point_lights(world_pos: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    let count = min(u32(frame.emissive.z), MAX_POINT_LIGHTS);
    for (var i = 0u; i < count; i++) {
        let light = frame.lights[i];
        let to_light = light.pos_radius.xyz - world_pos;
        let d2 = dot(to_light, to_light);
        let ratio = d2 * light.color.w;
        if ratio < 1.0 {
            let window = (1.0 - ratio * ratio) * (1.0 - ratio * ratio);
            let l = to_light * inverseSqrt(max(d2, 1e-8));
            let wrap = light.cone.z;
            let ndl = max((dot(n, l) + wrap) / (1.0 + wrap), 0.0);
            let off = spot_cos(-l, light.spot.xyz, light.cone.y);
            let c = clamp((off - light.spot.w) * light.cone.x, 0.0, 1.0);
            let cone = c * c * (3.0 - 2.0 * c);
            sum += light.color.rgb * (ndl * window * cone / (d2 + 0.1));
        }
    }
    return sum;
}

// 3×3 PCF with hardware 2×2 comparison filtering (an effective 4×4 texel kernel) in one
// cascade of the 3×1 shadow atlas. 1 = lit.
fn cascade_visibility(index: i32, world_pos: vec3<f32>, n: vec3<f32>, ndl: f32) -> f32 {
    let texel = frame.cascade_texel[index];
    // Normal offset (bigger at grazing light) plus a push toward the sun, both in texels of
    // this cascade, so the bias is the same on screen in every cascade.
    let offset = n * (texel * frame.params.w * clamp(1.0 - ndl, 0.25, 1.0))
        + frame.sun_dir.xyz * (texel * frame.params.z);
    let p = frame.cascade_view_proj[index] * vec4<f32>(world_pos + offset, 1.0);
    let ndc = p.xyz / p.w;
    let res = frame.cascade_texel.w;
    // Keep the 4×4 kernel inside this cascade's third of the atlas.
    let margin = 2.0 / res;
    let local = clamp(vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5), vec2<f32>(margin), vec2<f32>(1.0 - margin));
    let uv = vec2<f32>((local.x + f32(index)) / 3.0, local.y);
    let step = vec2<f32>(1.0 / (3.0 * res), 1.0 / res);
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
// `cascade_far.w` of each cascade; shadows fade out at the end of the last one.
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
        if index < 2 {
            next = cascade_visibility(index + 1, world_pos, n, ndl);
        }
        vis = mix(vis, next, t);
    }
    return vis;
}

fn shade(in: MeshOut) -> vec3<f32> {
    // Sampled first, in uniform control flow, so implicit derivatives are valid.
    let texel = textureSample(atlas, atlas_sampler, in.uv);
    let albedo = in.color.rgb * texel.rgb;
    let n = normalize(in.normal);
    let ndl = max(dot(n, frame.sun_dir.xyz), 0.0);
    var vis = 0.0;
    if ndl > 0.0 {
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
    let lamps = point_lights(in.world_pos, n) * (1.0 / 3.14159265);
    var c = albedo * (frame.sun_light.rgb * (ndl * vis) + ambient + lamps);
    // Self-lit color, untouched by texture and instance tint; fog still dims it.
    c += in.emissive;

    let to_frag = in.world_pos - frame.camera_pos.xyz;
    let dist = length(to_frag);
    let fog = 1.0 - exp(-dist * frame.camera_pos.w);
    c = mix(c, sky_radiance(to_frag / max(dist, 1e-4)), fog);
    return min(c, vec3<f32>(MAX_RADIANCE));
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in), 1.0);
}

@fragment
fn fs_mesh_ldr(in: MeshOut) -> @location(0) vec4<f32> {
    return display(shade(in), in.clip.xy);
}
