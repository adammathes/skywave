// HDR forward pass: sky gradient fill, then Lambert-lit meshes with one sun shadow map.

@group(1) @binding(0) var shadow_map: texture_depth_2d;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> SkyOut {
    let p = fullscreen_corner(index);
    var out: SkyOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let far = frame.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - frame.camera_pos.xyz);
    return vec4<f32>(sky_radiance(dir, true), 1.0);
}

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_mesh(v: VertexIn, i: InstanceIn) -> MeshOut {
    let world = instance_model(i) * vec4<f32>(v.pos, 1.0);
    let normal_matrix = mat3x3<f32>(i.n0.xyz, i.n1.xyz, i.n2.xyz);
    var out: MeshOut;
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normal_matrix * v.normal;
    out.color = v.color * i.color;
    return out;
}

// 1 = fully lit, 0 = in shadow. Outside the shadow map counts as lit.
fn sun_visibility(world_pos: vec3<f32>, n: vec3<f32>) -> f32 {
    let p = frame.light_view_proj * vec4<f32>(world_pos + n * frame.params.w, 1.0);
    let ndc = p.xyz / p.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5);
    let inside = all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0)) && ndc.z < 1.0;
    let lit = textureSampleCompareLevel(shadow_map, shadow_sampler, uv, ndc.z - frame.params.z);
    return select(1.0, lit, inside);
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let l = frame.sun_dir.xyz;
    let ndl = max(dot(n, l), 0.0);
    let vis = sun_visibility(in.world_pos, n);
    // Hemisphere ambient from the same colors the sky is drawn with; the upper hemisphere
    // averages zenith and horizon.
    let sky_light = (frame.sky_zenith.rgb + frame.sky_horizon.rgb) * 0.5;
    let ambient = mix(frame.sky_ground.rgb, sky_light, n.y * 0.5 + 0.5);
    var c = in.color.rgb * (frame.sun_color.rgb * (ndl * vis) + ambient);

    let to_frag = in.world_pos - frame.camera_pos.xyz;
    let dist = length(to_frag);
    let haze = 1.0 - exp(-dist * frame.camera_pos.w);
    c = mix(c, sky_radiance(to_frag / max(dist, 1e-4), false), haze);
    return vec4<f32>(c, 1.0);
}
