// HDR -> display: exposure, ACES fitted curve (Stephen Hill's RRT+ODT fit), sRGB encode
// when the target format does not do it in hardware.

@group(1) @binding(0) var hdr: texture_2d<f32>;

@vertex
fn vs_tonemap(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(fullscreen_corner(index), 0.0, 1.0);
}

fn aces_fitted(color: vec3<f32>) -> vec3<f32> {
    // sRGB -> AP1 (with the RRT's saturation tweak), column-major.
    let aces_in = mat3x3<f32>(
        vec3<f32>(0.59719, 0.07600, 0.02840),
        vec3<f32>(0.35458, 0.90834, 0.13383),
        vec3<f32>(0.04823, 0.01566, 0.83777),
    );
    let aces_out = mat3x3<f32>(
        vec3<f32>(1.60475, -0.10208, -0.00327),
        vec3<f32>(-0.53108, 1.10813, -0.07276),
        vec3<f32>(-0.07367, -0.00605, 1.07602),
    );
    let v = aces_in * color;
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return clamp(aces_out * (a / b), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_tonemap(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let hdr_color = textureLoad(hdr, vec2<i32>(frag.xy), 0).rgb;
    var c = aces_fitted(hdr_color * frame.params.x);
    if frame.params.y > 0.5 {
        c = linear_to_srgb(c);
    }
    return vec4<f32>(c, 1.0);
}
