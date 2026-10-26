// HDR -> display: exposure, bloom, AgX, vignette and dither, sRGB encode when the target
// format does not do it in hardware (see `display_exposed` in common.wgsl). Prepended
// with bloom.wgsl for its sampler and tent filter.

@group(1) @binding(2) var hdr: texture_2d<f32>;

@vertex
fn vs_tonemap(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(fullscreen_corner(index), 0.0, 1.0);
}

@fragment
fn fs_tonemap(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    var c = textureLoad(hdr, vec2<i32>(frag.xy), 0).rgb * frame.params.x;
    // Level 0 of the bloom chain holds the sum of all five levels, wide and narrow.
    c += tent9(bloom_src, frag.xy * frame.screen.zw) * frame.post.x;
    return display_exposed(c, frag.xy);
}
