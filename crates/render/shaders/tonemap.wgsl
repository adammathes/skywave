// HDR -> display: upscale from a scaled target, exposure, bloom, AgX, vignette and dither, sRGB encode when the target
// format does not do it in hardware (see `display_exposed` in common.wgsl). Prepended
// with bloom.wgsl for its sampler and tent filter.

@group(1) @binding(2) var hdr: texture_2d<f32>;

@vertex
fn vs_tonemap(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(fullscreen_corner(index), 0.0, 1.0);
}

@fragment
fn fs_tonemap(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    // The low-res look reads one sample per big pixel, at its center.
    var p = frag.xy;
    if look_id() == LOOK_LOW_RES {
        let b = max(floor(frame.screen.y / LOW_RES_LINES), 1.0);
        p = (floor(p / b) + 0.5) * b;
    }
    var c: vec3<f32>;
    if all(frame.render_size.xy == frame.screen.xy) {
        c = textureLoad(hdr, vec2<i32>(p), 0).rgb;
    } else {
        // A scaled target: bilinear upscale to the swapchain.
        c = textureSampleLevel(hdr, bloom_sampler, p * frame.screen.zw, 0.0).rgb;
    }
    c *= frame.params.x;
    // Level 0 of the bloom chain holds the sum of all five levels, wide and narrow.
    c += tent9(bloom_src, p * frame.screen.zw) * frame.post.x;
    return display_exposed(c, p);
}
