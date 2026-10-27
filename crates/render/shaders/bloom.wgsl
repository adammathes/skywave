// Bloom (Jimenez 2014, "Next Generation Post Processing in Call of Duty: Advanced
// Warfare"): a 13-tap downsample from the HDR target into a half-resolution chain of five
// levels, then 9-tap tent upsamples, each added (blend One + One) onto the next larger
// level, so level 0 ends up holding the sum of all five. Values are post-exposure.

@group(1) @binding(0) var bloom_src: texture_2d<f32>;
@group(1) @binding(1) var bloom_sampler: sampler;

struct BloomOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_bloom(@builtin(vertex_index) index: u32) -> BloomOut {
    let p = fullscreen_corner(index);
    var out: BloomOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

fn tap(uv: vec2<f32>, texel: vec2<f32>, x: f32, y: f32) -> vec3<f32> {
    return textureSampleLevel(bloom_src, bloom_sampler, uv + vec2<f32>(x, y) * texel, 0.0).rgb;
}

// The source as exposed for the tonemapper, scaled down (keeping its hue) so no channel
// exceeds the cap: a lone bright pixel (the sun disc, a lamp) cannot flood the chain.
fn exposed_tap(uv: vec2<f32>, texel: vec2<f32>, x: f32, y: f32) -> vec3<f32> {
    let c = tap(uv, texel, x, y) * frame.params.x;
    return c * min(1.0, frame.post.w / max(max(c.r, max(c.g, c.b)), 1e-6));
}

fn karis_weight(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + dot(c, LUMA));
}

// First level: the 13-tap pattern as five overlapping 2×2 box groups, each weighted by
// 1 / (1 + luma) (Karis average) against fireflies, then a soft-knee threshold.
@fragment
fn fs_bloom_prefilter(in: BloomOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(bloom_src));
    let uv = in.uv;
    let a = exposed_tap(uv, t, -2.0, -2.0);
    let b = exposed_tap(uv, t, 0.0, -2.0);
    let c = exposed_tap(uv, t, 2.0, -2.0);
    let d = exposed_tap(uv, t, -2.0, 0.0);
    let e = exposed_tap(uv, t, 0.0, 0.0);
    let f = exposed_tap(uv, t, 2.0, 0.0);
    let g = exposed_tap(uv, t, -2.0, 2.0);
    let h = exposed_tap(uv, t, 0.0, 2.0);
    let i = exposed_tap(uv, t, 2.0, 2.0);
    let j = exposed_tap(uv, t, -1.0, -1.0);
    let k = exposed_tap(uv, t, 1.0, -1.0);
    let l = exposed_tap(uv, t, -1.0, 1.0);
    let m = exposed_tap(uv, t, 1.0, 1.0);
    let g0 = (j + k + l + m) * 0.25;
    let g1 = (a + b + d + e) * 0.25;
    let g2 = (b + c + e + f) * 0.25;
    let g3 = (d + e + g + h) * 0.25;
    let g4 = (e + f + h + i) * 0.25;
    let w0 = 0.5 * karis_weight(g0);
    let w1 = 0.125 * karis_weight(g1);
    let w2 = 0.125 * karis_weight(g2);
    let w3 = 0.125 * karis_weight(g3);
    let w4 = 0.125 * karis_weight(g4);
    let color = (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / (w0 + w1 + w2 + w3 + w4);

    // Threshold on luminance with a soft knee half the threshold wide: nothing at
    // threshold - knee, linear above
    // threshold + knee, so surfaces just over the threshold barely contribute.
    let threshold = frame.post.y;
    let knee = 0.5 * threshold + 1e-4;
    let bright = dot(color, LUMA);
    var soft = clamp(bright - threshold + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee);
    let keep = max(soft, bright - threshold) / max(bright, 1e-4);
    return vec4<f32>(color * keep, 1.0);
}

// Later levels: the plain 13-tap weights (0.5 center box, 0.125 per corner box).
@fragment
fn fs_bloom_down(in: BloomOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(bloom_src));
    let uv = in.uv;
    var c = tap(uv, t, 0.0, 0.0) * 0.125;
    c += (tap(uv, t, -2.0, -2.0) + tap(uv, t, 2.0, -2.0) + tap(uv, t, -2.0, 2.0)
        + tap(uv, t, 2.0, 2.0)) * 0.03125;
    c += (tap(uv, t, 0.0, -2.0) + tap(uv, t, -2.0, 0.0) + tap(uv, t, 2.0, 0.0)
        + tap(uv, t, 0.0, 2.0)) * 0.0625;
    c += (tap(uv, t, -1.0, -1.0) + tap(uv, t, 1.0, -1.0) + tap(uv, t, -1.0, 1.0)
        + tap(uv, t, 1.0, 1.0)) * 0.125;
    return vec4<f32>(c, 1.0);
}

// 3×3 tent over the smaller level, one of its texels wide (shared with the tonemap pass).
fn tent9(src: texture_2d<f32>, uv: vec2<f32>) -> vec3<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    var c = textureSampleLevel(src, bloom_sampler, uv, 0.0).rgb * 4.0;
    c += (textureSampleLevel(src, bloom_sampler, uv + vec2<f32>(-t.x, 0.0), 0.0).rgb
        + textureSampleLevel(src, bloom_sampler, uv + vec2<f32>(t.x, 0.0), 0.0).rgb
        + textureSampleLevel(src, bloom_sampler, uv + vec2<f32>(0.0, -t.y), 0.0).rgb
        + textureSampleLevel(src, bloom_sampler, uv + vec2<f32>(0.0, t.y), 0.0).rgb) * 2.0;
    c += textureSampleLevel(src, bloom_sampler, uv - t, 0.0).rgb
        + textureSampleLevel(src, bloom_sampler, uv + t, 0.0).rgb
        + textureSampleLevel(src, bloom_sampler, uv + vec2<f32>(-t.x, t.y), 0.0).rgb
        + textureSampleLevel(src, bloom_sampler, uv + vec2<f32>(t.x, -t.y), 0.0).rgb;
    return c * (1.0 / 16.0);
}

@fragment
fn fs_bloom_up(in: BloomOut) -> @location(0) vec4<f32> {
    return vec4<f32>(tent9(bloom_src, in.uv), 1.0);
}
