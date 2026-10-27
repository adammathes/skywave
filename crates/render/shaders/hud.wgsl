// HUD: textured quads over the finished frame, one instance per quad, in target pixels.
// Glyphs sample the font atlas (coverage in .r) with nearest filtering; rectangles sample
// its solid cell. Image quads sample the map texture (rotated content, explicit mip level)
// or the stills texture, or draw a flat fill or the player arrow, optionally cut to a circle or a rounded
// rectangle; their edges are anti-aliased here. Colors (and the map's texels, stored
// unorm) arrive sRGB-encoded with straight alpha.

@group(1) @binding(0) var font_atlas: texture_2d<f32>;
@group(1) @binding(1) var font_sampler: sampler;
@group(1) @binding(2) var map_texture: texture_2d<f32>;
@group(1) @binding(3) var map_sampler: sampler;
@group(1) @binding(4) var still_texture: texture_2d<f32>;

// Quad kinds and masks; mirror `QUAD_*` and `MASK_*` in src/hud.rs.
const QUAD_FONT: u32 = 0u;
const QUAD_MAP: u32 = 1u;
const QUAD_SOLID: u32 = 2u;
const QUAD_ARROW: u32 = 3u;
const QUAD_STILL: u32 = 4u;
const MASK_CIRCLE: u32 = 1u;
const MASK_ROUNDED: u32 = 2u;

struct HudIn {
    // Target pixels: left, top, right, bottom.
    @location(0) rect: vec4<f32>,
    // Atlas or map uv: left, top, right, bottom.
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    // x: content rotation (rad, clockwise), y: map mip level, z: corner radius (px),
    // w: kind + 8 * mask.
    @location(3) params: vec4<f32>,
};

struct HudOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Pixels from the quad's center, screen-aligned (for the mask).
    @location(2) local: vec2<f32>,
    // The same point in the content's frame (rotated back), pixels from the center.
    @location(3) content: vec2<f32>,
    // xy: half the quad's size (px), z: mip level, w: flags.
    @location(4) @interpolate(flat) shape: vec4<f32>,
    @location(5) @interpolate(flat) radius: f32,
};

// Rotates `p` clockwise on screen (y down) by `a` radians.
fn rotate_cw(p: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(p.x * c - p.y * s, p.x * s + p.y * c);
}

// Triangle strip over the quad: (0,0), (1,0), (0,1), (1,1).
@vertex
fn vs_hud(@builtin(vertex_index) index: u32, q: HudIn) -> HudOut {
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let px = mix(q.rect.xy, q.rect.zw, corner);
    let half_size = max((q.rect.zw - q.rect.xy) * 0.5, vec2<f32>(1e-3));
    let local = (corner - 0.5) * half_size * 2.0;
    // Content point under this corner: the content is turned clockwise by the rotation,
    // so undo it.
    let content = rotate_cw(local, -q.params.x);
    var out: HudOut;
    out.pos = vec4<f32>(
        px.x * frame.screen.z * 2.0 - 1.0,
        1.0 - px.y * frame.screen.w * 2.0,
        0.0,
        1.0
    );
    let kind = u32(q.params.w + 0.5) & 7u;
    if kind == QUAD_MAP {
        out.uv = mix(q.uv.xy, q.uv.zw, content / (half_size * 2.0) + 0.5);
    } else {
        out.uv = mix(q.uv.xy, q.uv.zw, corner);
    }
    out.color = q.color;
    out.local = local;
    out.content = content;
    out.shape = vec4<f32>(half_size, q.params.y, q.params.w);
    out.radius = q.params.z;
    return out;
}

// Signed distance (px, negative inside) from `p` to triangle `abc`, by its edge lines.
fn triangle_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let winding = sign((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x));
    var d = -1e9;
    let corners = array<vec2<f32>, 3>(a, b, c);
    for (var i = 0; i < 3; i++) {
        let e0 = corners[i];
        let e1 = corners[(i + 1) % 3];
        let e = e1 - e0;
        let inward = winding * (e.x * (p.y - e0.y) - e.y * (p.x - e0.x)) / max(length(e), 1e-6);
        d = max(d, -inward);
    }
    return d;
}

// The player arrow: a dart pointing up, its tip at the top edge, filling the quad.
fn arrow_distance(p: vec2<f32>, half_size: vec2<f32>) -> f32 {
    let tip = vec2<f32>(0.0, -1.0) * half_size;
    let left = vec2<f32>(-0.78, 0.92) * half_size;
    let right = vec2<f32>(0.78, 0.92) * half_size;
    let notch = vec2<f32>(0.0, 0.42) * half_size;
    return min(triangle_distance(p, tip, left, notch), triangle_distance(p, tip, notch, right));
}

@fragment
fn fs_hud(in: HudOut) -> @location(0) vec4<f32> {
    let flags = u32(in.shape.w + 0.5);
    let kind = flags & 7u;
    let mask = flags >> 3u;
    var rgb = in.color.rgb;
    var alpha = in.color.a;
    // Sample both textures unconditionally (uniform control flow), then pick.
    let coverage = textureSampleLevel(font_atlas, font_sampler, in.uv, 0.0).r;
    let texel = textureSampleLevel(map_texture, map_sampler, in.uv, in.shape.z);
    let still = textureSampleLevel(still_texture, map_sampler, in.uv, in.shape.z);
    if kind == QUAD_FONT {
        alpha *= coverage;
    } else if kind == QUAD_MAP {
        let inside = all(in.uv >= vec2<f32>(0.0)) && all(in.uv <= vec2<f32>(1.0));
        rgb *= texel.rgb;
        alpha *= select(0.0, texel.a, inside);
    } else if kind == QUAD_STILL {
        let inside = all(in.uv >= vec2<f32>(0.0)) && all(in.uv <= vec2<f32>(1.0));
        rgb *= still.rgb;
        alpha *= select(0.0, still.a, inside);
    } else if kind == QUAD_ARROW {
        alpha *= clamp(0.5 - arrow_distance(in.content, in.shape.xy), 0.0, 1.0);
    }
    if mask == MASK_CIRCLE {
        // Distance to the inscribed ellipse, approximately in pixels.
        let r = min(in.shape.x, in.shape.y);
        let d = length(in.local / in.shape.xy) * r - (r - 0.5);
        alpha *= clamp(0.5 - d, 0.0, 1.0);
    } else if mask == MASK_ROUNDED {
        let r = min(in.radius, min(in.shape.x, in.shape.y));
        let q = abs(in.local) - in.shape.xy + vec2<f32>(r);
        let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
        alpha *= clamp(0.5 - d, 0.0, 1.0);
    }
    // An sRGB target encodes in hardware, so hand it linear values.
    if frame.params.y < 0.5 {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb, alpha);
}
