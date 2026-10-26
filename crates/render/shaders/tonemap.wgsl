// HDR -> display: exposure, ACES fitted curve, sRGB encode when the target format does not
// do it in hardware (see `display` in common.wgsl).

@group(1) @binding(0) var hdr: texture_2d<f32>;

@vertex
fn vs_tonemap(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(fullscreen_corner(index), 0.0, 1.0);
}

@fragment
fn fs_tonemap(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return display(textureLoad(hdr, vec2<i32>(frag.xy), 0).rgb);
}
