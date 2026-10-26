// The corona pass's depth read on GL (`CoronaDepth::Sampled`): the scene's depth as a plain
// float texture through a nearest, non-comparison sampler. On GL a float depth texture is
// incomplete (reads 0) unless sampled with nearest filtering, and a `textureLoad` there
// reads a texture no sampler configured. Prefixed to corona.wgsl.
@group(1) @binding(0) var scene_depth: texture_2d<f32>;
@group(1) @binding(1) var depth_sampler: sampler;

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
