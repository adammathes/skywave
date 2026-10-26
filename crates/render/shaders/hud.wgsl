// HUD: textured quads over the finished frame, one instance per quad, in target pixels.
// Glyphs sample the font atlas (coverage in .r) with nearest filtering; rectangles sample
// its solid cell. Colors arrive sRGB-encoded with straight alpha.

@group(1) @binding(0) var font_atlas: texture_2d<f32>;
@group(1) @binding(1) var font_sampler: sampler;

struct HudIn {
    // Target pixels: left, top, right, bottom.
    @location(0) rect: vec4<f32>,
    // Atlas uv: left, top, right, bottom.
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
};

struct HudOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

// Triangle strip over the quad: (0,0), (1,0), (0,1), (1,1).
@vertex
fn vs_hud(@builtin(vertex_index) index: u32, q: HudIn) -> HudOut {
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let px = mix(q.rect.xy, q.rect.zw, corner);
    var out: HudOut;
    out.pos = vec4<f32>(
        px.x * frame.screen.z * 2.0 - 1.0,
        1.0 - px.y * frame.screen.w * 2.0,
        0.0,
        1.0
    );
    out.uv = mix(q.uv.xy, q.uv.zw, corner);
    out.color = q.color;
    return out;
}

@fragment
fn fs_hud(in: HudOut) -> @location(0) vec4<f32> {
    let coverage = textureSampleLevel(font_atlas, font_sampler, in.uv, 0.0).r;
    var rgb = in.color.rgb;
    // An sRGB target encodes in hardware, so hand it linear values.
    if frame.params.y < 0.5 {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb, in.color.a * coverage);
}
