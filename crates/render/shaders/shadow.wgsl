// Depth-only pass from the sun, drawn once per cascade into its viewport of the 3×1 atlas.

struct Cascade {
    view_proj: mat4x4<f32>,
};

@group(1) @binding(0) var<uniform> cascade: Cascade;

@vertex
fn vs_shadow(v: VertexIn, i: InstanceIn) -> @builtin(position) vec4<f32> {
    return cascade.view_proj * (instance_model(i) * vec4<f32>(v.pos, 1.0));
}
