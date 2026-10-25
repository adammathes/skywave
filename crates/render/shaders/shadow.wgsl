// Depth-only pass from the sun.

@vertex
fn vs_shadow(v: VertexIn, i: InstanceIn) -> @builtin(position) vec4<f32> {
    return frame.light_view_proj * (instance_model(i) * vec4<f32>(v.pos, 1.0));
}
