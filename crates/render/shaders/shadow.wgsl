// Depth-only pass from the sun, drawn once per cascade into its viewport of the 3×1 atlas.

struct Cascade {
    view_proj: mat4x4<f32>,
};

@group(1) @binding(0) var<uniform> cascade: Cascade;

@vertex
fn vs_shadow(v: VertexIn, i: InstanceIn) -> @builtin(position) vec4<f32> {
    return cascade.view_proj * (instance_model(i) * vec4<f32>(v.pos, 1.0));
}

// Clears one cascade's viewport to the far plane: a fullscreen triangle at depth 1 drawn
// with depth compare Always, for frames that keep another cascade from an earlier frame.
@vertex
fn vs_shadow_clear(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(fullscreen_corner(index), 1.0, 1.0);
}
