// The corona pass's depth read on WebGPU and native (`CoronaDepth::Loaded`): the scene's
// depth as a `texture_depth_2d` read texel by texel with `textureLoad`, no sampler. Binding
// a depth texture as an unfilterable float texture and sampling it is legal WebGPU too, but
// this is the form every implementation maps straight onto its own depth texture read.
// Prefixed to corona.wgsl.
@group(1) @binding(0) var scene_depth: texture_depth_2d;

// 1 when the scene depth at pixel `pix` is not in front of `limit` (NDC depth), 0 when
// it is or when `pix` is off the screen.
fn depth_clear(pix: vec2<f32>, limit: f32) -> f32 {
    let size = vec2<f32>(textureDimensions(scene_depth));
    if any(pix < vec2<f32>(0.0)) || any(pix >= size) {
        return 0.0;
    }
    let d = textureLoad(scene_depth, vec2<i32>(floor(pix)), 0);
    return select(0.0, 1.0, d >= limit);
}
