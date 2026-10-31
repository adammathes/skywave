use super::*;
use glam::Vec4;

/// Every entry point must parse, validate and translate to GLSL ES 3.00 (WebGL2).
#[test]
fn shaders_translate_to_webgl2_glsl() {
    use naga::back::glsl;
    let mut entry_points = Vec::new();
    for (name, source) in shaders::ALL {
        if name == "corona_load" {
            // Never used on GL: GLSL has no `textureLoad` from a depth texture (naga
            // refuses it), hence `CoronaDepth::Sampled` there.
            continue;
        }
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        // `unpack2x16float` (the packed lights) needs f16-in-f32 support, which every GLES
        // 3 device reports (wgpu-hal's GL adapter sets `SHADER_F16_IN_F32` always; it is
        // `unpackHalf2x16` in GLSL ES 3.00) and WGSL has in core.
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32,
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        assert!(!module.entry_points.is_empty());
        for ep in &module.entry_points {
            entry_points.push(ep.name.clone());
            let options = glsl::Options {
                version: glsl::Version::Embedded {
                    version: 300,
                    is_webgl: true,
                },
                ..Default::default()
            };
            let pipeline = glsl::PipelineOptions {
                shader_stage: ep.stage,
                entry_point: ep.name.clone(),
                multiview: None,
            };
            let mut out = String::new();
            glsl::Writer::new(
                &mut out,
                &module,
                &info,
                &options,
                &pipeline,
                naga::proc::BoundsCheckPolicies::default(),
            )
            .and_then(|mut w| w.write())
            .unwrap_or_else(|e| panic!("{name}::{}: {e}", ep.name));
            assert!(out.starts_with("#version 300 es"), "{name}::{}", ep.name);
        }
    }
    for ep in [
        "vs_shadow",
        "vs_shadow_clear",
        "vs_sky",
        "fs_sky",
        "fs_sky_ldr",
        "vs_mesh",
        "fs_mesh",
        "fs_mesh_ldr",
        "vs_depth",
        "fs_depth",
        "vs_bloom",
        "fs_bloom_prefilter",
        "fs_bloom_down",
        "fs_bloom_up",
        "vs_tonemap",
        "fs_tonemap",
        "vs_hud",
        "fs_hud",
        "vs_corona",
        "fs_corona",
        "fs_corona_ldr",
    ] {
        assert!(entry_points.iter().any(|e| e == ep), "missing {ep}");
    }
}

/// Safari's WGSL compiler rejects a `let` whose type is a sampler or a texture
/// ("'sampler' cannot be used as the type of a 'let'"), which naga and the other
/// browsers accept. The handle types have to be used by name.
#[test]
fn shaders_never_bind_a_sampler_or_texture_to_a_let() {
    let handles: Vec<String> = shaders::ALL
        .iter()
        .flat_map(|&(_, source)| {
            source.lines().filter_map(|l| {
                let (_, decl) = l.trim().split_once("var ")?;
                let is_handle = decl.contains(": sampler") || decl.contains(": texture_");
                is_handle.then(|| decl.split(':').next().unwrap().trim().to_string())
            })
        })
        .collect();
    assert!(!handles.is_empty());
    for (name, source) in shaders::ALL {
        for (i, line) in source.lines().enumerate() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("let ") {
                if let Some((_, value)) = rest.split_once('=') {
                    let value = value.trim().trim_end_matches(';');
                    assert!(
                        !handles.iter().any(|h| h == value),
                        "{name}:{}: `{line}` binds a handle to a let; use `{value}` directly",
                        i + 1
                    );
                }
            }
        }
    }
}

/// Parses and validates every shader module the way the GLSL test does.
fn validated_shaders() -> Vec<(&'static str, naga::Module, naga::valid::ModuleInfo)> {
    shaders::ALL
        .iter()
        .map(|&(name, source)| {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32,
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            (name, module, info)
        })
        .collect()
}

/// Every entry point must translate to Metal Shading Language. Safari compiles WGSL to
/// Metal itself (not through naga), but naga's MSL writer trips over much the same
/// things (depth reads, packing builtins, interpolation, reserved names), so this is the
/// cheapest stand-in for a Safari we cannot run here; it also covers native Metal.
#[test]
fn shaders_translate_to_metal_msl() {
    use naga::back::msl;
    let mut written = 0;
    for (name, module, info) in validated_shaders() {
        for ep in &module.entry_points {
            let options = msl::Options {
                // Metal 2.1: macOS 10.14 / iOS 12, below anything with WebGPU.
                lang_version: (2, 1),
                ..Default::default()
            };
            let pipeline = msl::PipelineOptions {
                entry_point: Some((ep.stage, ep.name.clone())),
                ..Default::default()
            };
            let (out, _) = msl::write_string(&module, &info, &options, &pipeline)
                .unwrap_or_else(|e| panic!("{name}::{}: {e}", ep.name));
            assert!(
                out.contains("#include <metal_stdlib>"),
                "{name}::{}",
                ep.name
            );
            assert!(out.contains(&ep.name), "{name}::{}", ep.name);
            written += 1;
        }
    }
    // The 21 the GLSL test names, and the `textureLoad` corona's 3.
    assert_eq!(written, 26);
}

/// Every entry point must translate to SPIR-V (native Vulkan; a cheap extra check).
#[test]
fn shaders_translate_to_spirv() {
    use naga::back::spv;
    for (name, module, info) in validated_shaders() {
        for ep in &module.entry_points {
            let pipeline = spv::PipelineOptions {
                shader_stage: ep.stage,
                entry_point: ep.name.clone(),
            };
            let words = spv::write_vec(&module, &info, &spv::Options::default(), Some(&pipeline))
                .unwrap_or_else(|e| panic!("{name}::{}: {e}", ep.name));
            assert_eq!(words.first(), Some(&0x0723_0203), "{name}::{}", ep.name);
        }
    }
}

/// The shaders stick to core WGSL, which every browser takes as is: no `enable` or
/// `requires` directives (so no `f16`), no `@builtin(sample_index)`, no `override`s,
/// no depth texture sampled without a comparison, nothing beyond the WebGL2 subset.
#[test]
fn shaders_use_only_core_wgsl() {
    for (name, source) in shaders::ALL {
        for word in [
            "enable ",
            "requires ",
            "diagnostic",
            "f16",
            "sample_index",
            "sample_mask",
            "override ",
            "var<storage",
            "var<workgroup",
            "texture_storage",
            "texture_2d_array",
            "texture_external",
            "@compute",
            "subgroup",
        ] {
            let hit = source
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .any(|l| l.contains(word));
            assert!(!hit, "{name} uses `{word}`");
        }
        // Depth textures are read with a comparison sampler or `textureLoad` only.
        if source.contains("texture_depth_2d") {
            let depth_vars: Vec<&str> = source
                .lines()
                .filter(|l| l.contains(": texture_depth_2d"))
                .filter_map(|l| l.split("var ").nth(1)?.split(':').next())
                .collect();
            for var in depth_vars {
                for call in ["textureSample(", "textureSampleLevel(", "textureGather("] {
                    assert!(
                        !source.contains(&format!("{call}{var},")),
                        "{name} samples depth `{var}` without a comparison"
                    );
                }
            }
        }
    }
}

#[test]
fn hdr_needs_a_renderable_blendable_filterable_float_format() {
    use wgpu::{TextureFormatFeatureFlags as F, TextureUsages as U};
    let features = |usages, flags| wgpu::TextureFormatFeatures {
        allowed_usages: usages,
        flags,
    };
    let all = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC;
    assert!(hdr_format_supported(features(
        all,
        F::BLENDABLE | F::FILTERABLE
    )));
    // WebGL2 without EXT_color_buffer_float, EXT_float_blend or float filtering.
    assert!(!hdr_format_supported(features(
        U::TEXTURE_BINDING,
        F::BLENDABLE | F::FILTERABLE
    )));
    assert!(!hdr_format_supported(features(all, F::FILTERABLE)));
    assert!(!hdr_format_supported(features(all, F::BLENDABLE)));
    // WebGPU guarantees all of it for Rgba16Float.
    assert!(hdr_format_supported(
        HDR_TEXTURE_FORMAT.guaranteed_format_features(wgpu::Features::empty())
    ));
}

#[test]
fn uniform_and_instance_layouts() {
    // std140-compatible: only mat4 and vec4 members, 16-byte multiple, under WebGL2's
    // 16 KiB uniform block limit.
    assert_eq!(
        size_of::<FrameUniform>(),
        6 * 64 + 29 * 16 + MAX_LIGHTS * 32
    );
    assert_eq!(size_of::<PackedLight>(), 32);
    assert_eq!(size_of::<CoronaRaw>(), 32);
    assert_eq!(size_of::<FrameUniform>() % 16, 0);
    assert!(
        size_of::<FrameUniform>()
            <= wgpu::Limits::downlevel_webgl2_defaults().max_uniform_buffer_binding_size as usize
    );
    assert_eq!(size_of::<CascadeUniform>() as u64, CASCADE_UNIFORM_STRIDE);
    assert_eq!(size_of::<InstanceRaw>(), 144);
    assert_eq!(INSTANCE_ATTRIBUTES[8].format, wgpu::VertexFormat::Uint32x4);
    assert_eq!(size_of::<Vertex>(), 36);
    assert_eq!(VERTEX_ATTRIBUTES[2].offset, 24);
    assert_eq!(VERTEX_ATTRIBUTES[3].offset, 28);
    assert_eq!(VERTEX_ATTRIBUTES[4].offset, 32);
    let attributes = VERTEX_ATTRIBUTES.len() + INSTANCE_ATTRIBUTES.len();
    assert!(attributes <= wgpu::Limits::downlevel_webgl2_defaults().max_vertex_attributes as usize);
    // Every member of `Frame` in common.wgsl is accounted for, in order.
    let wgsl = include_str!("../shaders/common.wgsl");
    let body = &wgsl[wgsl.find("struct Frame {").unwrap()..];
    let body = &body[..body.find("};").unwrap()];
    let members: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//") && l.contains(':'))
        .map(|l| l.split(':').next().unwrap())
        .collect();
    assert_eq!(
        members,
        [
            "view_proj",
            "inv_view_proj",
            "rel_view_proj",
            "cascade_view_proj",
            "camera_pos",
            "camera_fwd",
            "cascade_far",
            "cascade_texel",
            "sun_dir",
            "sun_light",
            "sun_disc",
            "moon_dir",
            "moon_light",
            "perez_a",
            "perez_b",
            "perez_c",
            "perez_d",
            "perez_e",
            "zenith",
            "sun_sky",
            "sky_params",
            "night_sky",
            "sky_up",
            "sky_ground",
            "sky_horizon",
            "params",
            "emissive",
            "post",
            "screen",
            "cabin_light",
            "corona",
            "render_size",
            "shadow_params",
            "lights",
        ]
    );
    // The light array length and the set size agree with the shaders.
    assert!(wgsl.contains(&format!("MAX_LIGHTS: u32 = {MAX_LIGHTS}u;")));
    assert!(wgsl.contains(&format!("LIGHTS_PER_DRAW: u32 = {LIGHTS_PER_DRAW}u;")));
    assert!(body.contains(&format!("array<PackedLight, {MAX_LIGHTS}>")));
}

/// The half-float writer agrees with IEEE 754 binary16, rounding to nearest even.
#[test]
fn f16_bits_round_like_ieee() {
    for (x, bits) in [
        (0.0f32, 0x0000u16),
        (-0.0, 0x8000),
        (1.0, 0x3c00),
        (-2.0, 0xc000),
        (0.5, 0x3800),
        (65504.0, 0x7bff),
        (1e6, 0x7bff),
        (f32::INFINITY, 0x7bff),
        (2f32.powi(-14), 0x0400),
        (2f32.powi(-24), 0x0001),
        (2f32.powi(-26), 0x0000),
        // 1 + 2^-11 is halfway between 1 and the next half: ties go to the even 1.
        (1.0 + 2f32.powi(-11), 0x3c00),
        (1.0 + 3.0 * 2f32.powi(-11), 0x3c02),
        (0.1, 0x2e66),
        (-0.309017, 0xb4f2),
    ] {
        assert_eq!(f16_bits(x), bits, "{x}");
    }
    assert_eq!(f16_bits(f32::NAN) & 0x7c00, 0x7c00);
    for i in 0..2000 {
        let x = (i as f32 - 1000.0) * 0.0173;
        assert!(
            (f16_value(f16_bits(x)) - x).abs() <= x.abs() / 1024.0 + 1e-7,
            "{x}"
        );
    }
}

/// Inverse of `f16_bits` for finite values (what `unpack2x16float` does).
fn f16_value(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = i32::from((h >> 10) & 0x1f);
    let m = f32::from(h & 0x3ff);
    sign * if e == 0 {
        m * 2f32.powi(-24)
    } else {
        (1.0 + m / 1024.0) * 2f32.powi(e - 15)
    }
}

/// `oct_decode` in shaders/forward.wgsl.
fn oct_decode(e: [f32; 2]) -> Vec3 {
    let mut v = Vec3::new(e[0], e[1], 1.0 - e[0].abs() - e[1].abs());
    if v.z < 0.0 {
        let s = |a: f32| if a >= 0.0 { 1.0 } else { -1.0 };
        v = Vec3::new((1.0 - v.y.abs()) * s(v.x), (1.0 - v.x.abs()) * s(v.y), v.z);
    }
    v.normalize()
}

fn snorm_value(bits: u32) -> f32 {
    (f32::from(bits as u16 as i16) / 32767.0).max(-1.0)
}

/// A light as the shader sees it after `unpack_light`.
struct Unpacked {
    pos: Vec3,
    radius: f32,
    color: Vec3,
    axis: Vec3,
    cone_end: f32,
    cone_scale: f32,
    squash: f32,
    wrap: f32,
}

fn unpack(p: &PackedLight) -> Unpacked {
    let lo = |w: u32| f16_value(w as u16);
    let hi = |w: u32| f16_value((w >> 16) as u16);
    let [x, y, z, w] = p.bits;
    Unpacked {
        pos: Vec4::from(p.pos_radius).xyz(),
        radius: p.pos_radius[3],
        color: Vec3::new(lo(x), hi(x), lo(y)),
        axis: oct_decode([snorm_value(z), snorm_value(z >> 16)]),
        cone_end: hi(y),
        cone_scale: lo(w),
        squash: 1.0 + ((w >> 16) & 255) as f32 / 16.0,
        wrap: (w >> 24) as f32 / 255.0,
    }
}

#[test]
fn octahedral_axes_round_trip() {
    for i in 0..400 {
        // A spiral over the sphere, both poles and the seams included.
        let t = i as f32 / 399.0;
        let y = 1.0 - 2.0 * t;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let a = i as f32 * 2.399_963;
        for v in [
            Vec3::new(r * a.cos(), y, r * a.sin()),
            Vec3::new(y, r * a.cos(), r * a.sin()),
            Vec3::new(r * a.cos(), r * a.sin(), y),
        ] {
            let e = oct_encode(v).map(|c| snorm_value(u32::from(snorm16(c))));
            let back = oct_decode(e);
            assert!(back.dot(v) > 0.999_999, "{v} -> {back}");
        }
    }
    assert_eq!(oct_encode(Vec3::ZERO), [0.0, 0.0]);
}

/// Instances carry their light set in the last attribute: count in the high half of
/// `y`, indices a byte each in `z` and `w`; indices past the frame's lights drop out.
#[test]
fn instance_params_pack_glow_cabin_and_light_set() {
    let mut inst = MeshInstance {
        glow: 0.75,
        cabin: 0.5,
        lights: LightSet::from_indices(&[3, 255, 7, 0, 200, 1, 2, 9, 11]),
        ..MeshInstance::new(MeshId(0), Mat4::IDENTITY.to_cols_array_2d())
    };
    assert_eq!(inst.lights.len(), LIGHTS_PER_DRAW, "the ninth is left out");
    let p = instance_params(&inst, MAX_LIGHTS);
    assert_eq!(
        p[0],
        0x3a00 | 128 << 16 | 8 << 24,
        "0.75 as a half float, cabin, count"
    );
    assert_eq!(p[1], 0, "nothing dimmed");
    assert_eq!(p[2], 3 | 255 << 8 | 7 << 16);
    assert_eq!(p[3], 200 | 1 << 8 | 2 << 16 | 9 << 24);
    // With 10 lights in the frame, 255 and 200 are not there.
    let p = instance_params(&inst, 10);
    assert_eq!(p[0] >> 24, 6);
    assert_eq!(p[2], 3 | 7 << 8 | 1 << 24);
    assert_eq!(p[3], 2 | 9 << 8);
    inst.lights = LightSet::EMPTY;
    inst.glow = -1.0;
    inst.cabin = 2.0;
    assert_eq!(instance_params(&inst, 4), [255 << 16, 0, 0, 0]);
    // No sun shadow: the bit past the light count, which the shader reads as 16 in the
    // count's word.
    inst.receives_shadow = false;
    inst.lights = LightSet::from_indices(&[1, 2]);
    let p = instance_params(&inst, 4);
    assert_eq!(p[0], 255 << 16 | 2 << 24 | NO_SUN_SHADOW);
    let wgsl = include_str!("../shaders/forward.wgsl");
    assert!(wgsl.contains("(i.params.x >> 24u) & 31u"));
    assert!(wgsl.contains("(in.lights.x & 16u) == 0u"));
    // Dimmed lights: four bits each in y, packed with the lights kept.
    inst.lights = LightSet::from_indices(&[1, 9, 2])
        .with_weight(0, 0.5)
        .with_weight(2, 0.0)
        .with_weight(3, 0.0);
    assert_eq!(inst.lights.weight(0), 1.0 - 8.0 / 15.0);
    assert_eq!((inst.lights.weight(1), inst.lights.weight(2)), (1.0, 0.0));
    assert_eq!(inst.lights.weight(3), 1.0, "past the set");
    assert_eq!(instance_params(&inst, 4)[1], 8 | 15 << 4);
    assert_eq!(LightSet::EMPTY.with_weight(0, 0.0), LightSet::EMPTY);
    assert!(wgsl.contains("(light_set.y >> (4u * k)) & 15u"));
    let mut set = LightSet::EMPTY;
    assert!(set.is_empty() && !set.push(MAX_LIGHTS) && set.push(MAX_LIGHTS - 1));
    assert_eq!(set.indices(), &[255]);
}

#[test]
fn normal_matrix_handles_non_uniform_scale() {
    let m = Mat4::from_scale(Vec3::new(4.0, 1.0, 1.0)) * Mat4::from_rotation_z(0.5);
    let raw = InstanceRaw::new(
        &MeshInstance::new(MeshId(0), m.to_cols_array_2d()),
        0,
        Vec3::ZERO,
    );
    let n = Mat3::from_cols(
        Vec4::from(raw.normal[0]).xyz(),
        Vec4::from(raw.normal[1]).xyz(),
        Vec4::from(raw.normal[2]).xyz(),
    );
    // A transformed normal stays perpendicular to a transformed tangent.
    let (normal, tangent) = (Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -1.0, 0.0));
    let t = m.transform_vector3(tangent);
    assert!((n * normal).dot(t).abs() < 1e-5);
}

fn noop_device(limits: wgpu::Limits) -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: wgpu::BackendOptions {
            noop: wgpu::NoopBackendOptions {
                enable: true,
                limits: Some(limits.clone()),
                ..Default::default()
            },
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&Default::default())).expect("noop adapter");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: limits,
        ..Default::default()
    }))
    .expect("noop device")
}

fn test_scene<'a>(instances: &'a [MeshInstance]) -> SceneView<'a> {
    test_scene_lit(instances, &[])
}

fn test_scene_lit<'a>(instances: &'a [MeshInstance], lights: &'a [PointLight]) -> SceneView<'a> {
    SceneView {
        camera: Camera {
            view: glam::camera::rh::view::look_at_mat4(
                Vec3::new(0.0, 10.0, 30.0),
                Vec3::ZERO,
                Vec3::Y,
            )
            .to_cols_array_2d(),
            proj: glam::camera::rh::proj::directx::perspective(1.0, 4.0 / 3.0, 0.1, 1000.0)
                .to_cols_array_2d(),
        },
        sun: Sun {
            direction: [0.3, 0.8, 0.5],
            illuminance: [95.0, 92.0, 86.0],
        },
        sky: Sky {
            moon_direction: [-0.3, 0.5, -0.8],
            moon_luminance: 2.5,
            star_brightness: 1.0,
            ..Sky::default()
        },
        instances,
        lights,
        hud: Hud::EMPTY,
        quality: RenderQuality::default(),
    }
}

/// A street lamp every 10 m, more than the uniform array holds.
fn lamps(count: usize) -> Vec<PointLight> {
    (0..count)
        .map(|i| PointLight::omni([i as f32 * 10.0, 7.5, 0.0], [2.5, 1.6, 0.6], 25.0))
        .collect()
}

fn render_frames(
    device: &wgpu::Device,
    r: &mut Renderer,
    format: wgpu::TextureFormat,
    scene: &SceneView,
    sizes: &[(u32, u32)],
) {
    for &size in sizes {
        r.resize(size);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[format.add_srgb_suffix()],
        });
        r.render(&target.create_view(&Default::default()), scene);
    }
}

/// An unchanged scene is drawn once; a HUD change redraws only the tonemap and the HUD
/// (in HDR mode; without an HDR target to keep, the whole frame); any change to what
/// the world passes draw, or to the targets and chunks under them, draws the world.
#[test]
fn plans_redraw_only_what_changed() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    for mode in [HdrMode::Float16, HdrMode::Ldr] {
        let options = RendererOptions {
            hdr: mode,
            chunk_pool: ChunkPoolConfig {
                slots: 1,
                pages: 1,
                page_vertices: 64,
                page_indices: 64,
            },
            ..Default::default()
        };
        let mut r = Renderer::with_options(&device, &queue, format, (64, 48), options);
        let block = r.add_mesh(&Mesh::unit_block([255; 4]));
        let other = r.add_mesh(&Mesh::unit_block([255, 0, 0, 255]));
        let mut instances = [MeshInstance::new(block, Mat4::IDENTITY.to_cols_array_2d())];
        let lights = lamps(3);
        let mut hud = HudBuilder::new();
        hud.begin(1.0);
        hud.text(10.0, 10.0, 1.0, [255; 4], "12:00");
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 64,
                height: 48,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let draw = |r: &mut Renderer, scene: &SceneView| {
            let acquire = || Some(((), target.create_view(&Default::default())));
            r.draw(scene, acquire).0
        };
        let ui = if mode == HdrMode::Float16 {
            FrameWork::Ui
        } else {
            FrameWork::World
        };
        let scene = SceneView {
            hud: hud.view(),
            ..test_scene_lit(&instances, &lights)
        };
        assert_eq!(draw(&mut r, &scene), FrameWork::World, "the first frame");
        for _ in 0..3 {
            assert_eq!(draw(&mut r, &scene), FrameWork::Skip);
        }
        assert_eq!((r.stats().frames, r.stats().ui_frames), (1, 0));
        hud.begin(1.0);
        hud.text(10.0, 10.0, 1.0, [255; 4], "12:01");
        let scene = SceneView {
            hud: hud.view(),
            ..test_scene_lit(&instances, &lights)
        };
        assert_eq!(draw(&mut r, &scene), ui, "{mode:?}: the clock ticked");
        assert_eq!(draw(&mut r, &scene), FrameWork::Skip);
        let mut moved = scene;
        moved.sun.direction[0] += 1e-4;
        assert_eq!(draw(&mut r, &moved), FrameWork::World, "the sun moved");
        assert_eq!(draw(&mut r, &moved), FrameWork::Skip);
        let mut dimmer = lights.clone();
        dimmer[2].color[0] *= 0.5;
        let lit = SceneView {
            lights: &dimmer,
            ..moved
        };
        assert_eq!(draw(&mut r, &lit), FrameWork::World, "a light changed");
        instances[0].transform[3][0] += 0.01;
        let scene = SceneView {
            hud: hud.view(),
            ..test_scene_lit(&instances, &dimmer)
        };
        assert_eq!(draw(&mut r, &scene), FrameWork::World, "the block moved");
        assert_eq!(draw(&mut r, &scene), FrameWork::Skip);
        // A car's lamp state is another mesh, its lamps' glow a multiplier, an NPC's
        // brake lamps one more instance: each redraws the world.
        let mut swapped = instances;
        swapped[0].mesh = other;
        let at = |x: &[MeshInstance], r: &mut Renderer| {
            draw(
                r,
                &SceneView {
                    hud: hud.view(),
                    ..test_scene_lit(x, &dimmer)
                },
            )
        };
        assert_eq!(at(&swapped, &mut r), FrameWork::World, "a lamp state");
        assert_eq!(at(&swapped, &mut r), FrameWork::Skip);
        swapped[0].glow = 2.0;
        assert_eq!(at(&swapped, &mut r), FrameWork::World, "a lamp glow");
        let overlay = [swapped[0], MeshInstance::new(block, swapped[0].transform)];
        assert_eq!(at(&overlay, &mut r), FrameWork::World, "an overlay");
        assert_eq!(at(&overlay, &mut r), FrameWork::Skip);
        r.resize((32, 24));
        assert_eq!(r.plan(&scene), FrameWork::World, "resized");
        r.resize((64, 48));
        assert_eq!(draw(&mut r, &scene), FrameWork::World);
        let slot = r.alloc_chunk_slot().unwrap();
        r.write_chunk(slot, &Mesh::unit_block([9; 4]), &[[0, 36]])
            .unwrap();
        assert_eq!(
            draw(&mut r, &scene),
            FrameWork::World,
            "a chunk streamed in"
        );
        r.upload_hud_image(HudImageTarget::Map, 1, 1, &[1, 2, 3, 4]);
        assert_eq!(draw(&mut r, &scene), ui, "a new map");
        let quality = RenderQuality {
            resolution_scale: 0.5,
            ..scene.quality
        };
        let scaled = SceneView { quality, ..scene };
        assert_eq!(draw(&mut r, &scaled), FrameWork::World, "a quality change");
        assert_eq!(draw(&mut r, &scaled), FrameWork::Skip);
        let skipped = r.draw(&scaled, || -> Option<((), wgpu::TextureView)> {
            panic!("nothing to draw: no target asked for")
        });
        assert_eq!(skipped, (FrameWork::Skip, None));
    }
}

/// Idle frames and kept cascades agree: a world frame that held the far cascade over
/// without checking its casters keeps the next frames drawing the world until one
/// checks it (so a moved caster's shadow is never left stale on screen), the first
/// world frame after kept or skipped ones checks every cascade, and Ui and Skip frames
/// leave the kept cascades as they were.
#[test]
fn idle_frames_never_leave_a_held_cascade_stale() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let quality = RenderQuality {
        shadow_size: 512,
        cascades: 2,
        far_cascade_interval: 3,
        ..RenderQuality::default()
    };
    let options = RendererOptions {
        quality,
        ..Default::default()
    };
    let mut r = Renderer::with_options(&device, &queue, format, (64, 48), options);
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    // A block by the camera (near cascade) and one 150 m out (far cascade).
    let place = |far_x: f32| {
        let big = Mat4::from_scale(Vec3::splat(6.0));
        let far = Mat4::from_translation(Vec3::new(far_x, 0.0, -120.0)) * big;
        [
            MeshInstance::new(block, big.to_cols_array_2d()),
            MeshInstance::new(block, far.to_cols_array_2d()),
        ]
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 48,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let draw = |r: &mut Renderer, scene: &SceneView| {
        let acquire = || Some(((), target.create_view(&Default::default())));
        let work = r.draw(scene, acquire).0;
        (work, r.stats().shadow_instances[1])
    };
    let scene_at = |instances| SceneView {
        quality,
        ..test_scene(instances)
    };
    let a = place(0.0);
    assert_eq!(draw(&mut r, &scene_at(&a)).0, FrameWork::World);
    // Driving: the far block moves; the far cascade is held between its frames...
    let b = place(1.0);
    assert_eq!(draw(&mut r, &scene_at(&b)), (FrameWork::World, 0), "held");
    // ...and the still frames after go on drawing the world until it is checked.
    assert_eq!(draw(&mut r, &scene_at(&b)), (FrameWork::World, 0), "held");
    let (work, far) = draw(&mut r, &scene_at(&b));
    assert_eq!(work, FrameWork::World);
    assert!(far > 0, "its frame: the moved block is drawn into it");
    assert_eq!(draw(&mut r, &scene_at(&b)).0, FrameWork::Skip, "settled");
    // A Ui frame in between changes nothing the cascades keep.
    let mut hud = HudBuilder::new();
    hud.begin(1.0);
    hud.text(4.0, 4.0, 1.0, [255; 4], "paused");
    let menu = SceneView {
        hud: hud.view(),
        ..scene_at(&b)
    };
    assert_eq!(draw(&mut r, &menu).0, FrameWork::Ui);
    let mut brighter = menu;
    brighter.sky.exposure *= 1.01;
    assert_eq!(draw(&mut r, &brighter).0, FrameWork::World);
    assert_eq!(r.stats().cascades_drawn, 0, "both cascades still kept");
    assert_eq!(draw(&mut r, &brighter).0, FrameWork::Skip);
    // Resuming: the first world frame after skipped ones checks the far cascade, even
    // off its interval.
    assert!(!r.stats().frames.is_multiple_of(3));
    let c = place(2.0);
    let resumed = SceneView {
        instances: &c,
        ..brighter
    };
    let (work, far) = draw(&mut r, &resumed);
    assert_eq!(work, FrameWork::World);
    assert!(
        far > 0,
        "the moved block's shadow is drawn on the first world frame"
    );
}

/// How still is still is measured from the camera, not the place's origin: at the far
/// corner of a 3 km place a block 3 m ahead creeping 4 mm a frame is drawn every frame,
/// while a car's billionth-of-a-radian spring jitter is not.
#[test]
fn stillness_is_judged_from_the_camera() {
    let corner = Vec3::new(3000.0, 1.0, -3000.0);
    let camera = Camera {
        view: glam::camera::rh::view::look_at_mat4(corner, corner + Vec3::X, Vec3::Y)
            .to_cols_array_2d(),
        proj: glam::camera::rh::proj::directx::perspective(1.0, 1.6, 0.1, 1000.0)
            .to_cols_array_2d(),
    };
    let at = |x: f32, tilt: f32| {
        let m = Mat4::from_translation(corner + Vec3::new(3.0 + x, 0.0, 0.0))
            * Mat4::from_rotation_z(tilt);
        [MeshInstance::new(MeshId(0), m.to_cols_array_2d())]
    };
    let mut kept = KeptWorld {
        head: None,
        instances: Vec::new(),
        lights: Vec::new(),
    };
    let first = at(0.0, 0.0);
    let scene = SceneView {
        camera,
        ..test_scene(&first)
    };
    kept.keep(&scene, false);
    for step in 1..5 {
        let crept = at(0.004 * step as f32, 0.0);
        let moved = SceneView {
            instances: &crept,
            ..scene
        };
        assert!(!kept.same(&moved, false), "4 mm × {step} at 3 m");
        kept.keep(&moved, false);
    }
    let jitter = at(0.016, 1e-9);
    let still = SceneView {
        instances: &jitter,
        ..scene
    };
    assert!(kept.same(&still, false), "spring jitter");
    let nan = at(f32::NAN, 0.0);
    let broken = SceneView {
        instances: &nan,
        ..scene
    };
    assert!(!kept.same(&broken, false), "NaN is never still");
}

/// Builds the whole renderer on wgpu's no-op backend with WebGL2 limits, in both HDR
/// modes, and records frames, so wgpu's validation checks every pipeline, binding and
/// pass without a GPU.
#[test]
fn renders_frames_on_noop_device_with_webgl2_limits_in_both_hdr_modes() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    for format in [
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8Unorm,
    ] {
        for (mode, corona_depth) in [
            (HdrMode::Float16, CoronaDepth::Sampled),
            (HdrMode::Float16, CoronaDepth::Loaded),
            (HdrMode::Ldr, CoronaDepth::Sampled),
            (HdrMode::Ldr, CoronaDepth::Loaded),
        ] {
            let options = RendererOptions {
                hdr: mode,
                corona_depth,
                // Every instance in every pass, so the counts below are the runs.
                culling: false,
                ..Default::default()
            };
            let mut r = Renderer::with_options(&device, &queue, format, (64, 48), options);
            assert_eq!(r.is_hdr(), mode == HdrMode::Float16);
            // The no-op backend answers the corona pipeline's error scope at once.
            assert_eq!(r.corona_probe(), Probe::Passed);
            assert_eq!(r.corona_depth(), corona_depth);
            // WebGL2's 2048 texture limit leaves a third of that per cascade.
            assert_eq!(r.shadow_cascade_size(), 2048 / 3);
            let block = r.add_mesh(&Mesh::unit_block([255; 4]));
            let ground = r.add_mesh(&Mesh::ground_quad(10.0, 10.0, [90, 120, 60, 255]));
            let brick = r.add_mesh(&Mesh::cuboid_tiled(
                [-1.0, 0.0, -1.0],
                [1.0, 3.0, 1.0],
                [255; 4],
                Tile::BrickRed,
                0.6,
            ));
            let sign = r.add_mesh(
                &Mesh::cuboid([-1.0, 0.0, -0.1], [1.0, 0.6, 0.1], [20, 20, 20, 255])
                    .with_emissive(emissive([255, 40, 160], 6.0)),
            );
            assert_eq!((block, ground, brick), (MeshId(0), MeshId(1), MeshId(2)));

            // More instances than the initial capacity forces one buffer reallocation.
            let mut instances = vec![MeshInstance::new(ground, Mat4::IDENTITY.to_cols_array_2d())];
            for i in 0..300 {
                instances.push(MeshInstance {
                    color: [0.8, 0.7, 0.6, 1.0],
                    // Some switched off, some in a cabin.
                    glow: (i % 3) as f32 * 0.5,
                    cabin: (i % 2) as f32,
                    ..MeshInstance::new(
                        match i % 50 {
                            0 => brick,
                            25 => sign,
                            _ => block,
                        },
                        Mat4::from_translation(Vec3::new(i as f32, 0.0, 0.0)).to_cols_array_2d(),
                    )
                });
            }
            // More lights than a frame holds; every tenth draws a corona, one lights
            // nothing (a tail lamp).
            let mut lights = lamps(MAX_LIGHTS + 3);
            lights[1].direction = [0.0, -0.2, -1.0];
            lights[1].cone_cos = [0.9, 0.98];
            lights[1].cone_squash = 4.0;
            lights[1].wrap = 0.25;
            lights[5].radius = 0.0;
            for l in lights.iter_mut().step_by(10) {
                l.corona = Corona {
                    radius: 1.0,
                    strength: 1.0,
                };
            }
            // Each instance is lit by up to 8 lights round it.
            for (i, inst) in instances.iter_mut().enumerate() {
                let near: Vec<usize> = (0..MAX_LIGHTS).filter(|k| k.abs_diff(i / 10) < 4).collect();
                inst.lights = LightSet::from_indices(&near);
            }
            let mut scene = test_scene_lit(&instances, &lights);
            scene.sky.cabin_light = [0.01, 0.008, 0.006];
            // Night: the lamps' coronas show.
            scene.sky.exposure = exposure_from_ev100(3.0);
            // Ground, then runs of block with a brick or sign every 25 instances, drawn
            // in 3 cascades + forward.
            let runs = 1 + 1 + 12 + 11;
            // Bloom: five levels down, four back up; sky and tonemap are one draw each.
            let bloom = if r.is_hdr() {
                2 * BLOOM_LEVELS as u32 - 1
            } else {
                0
            };
            // Sky, coronas and (HDR) tonemap.
            let passes = if r.is_hdr() { 3 } else { 2 };
            // Odd sizes and a size smaller than the bloom chain is deep.
            let sizes = [(64, 48), (33, 17), (5, 3)];
            render_frames(&device, &mut r, format, &scene, &sizes[..1]);
            let first = r.stats();
            assert_eq!(first.shadow_draw_calls, 3 * runs);
            assert_eq!(first.bloom_draw_calls, bloom);
            assert_eq!(first.draw_calls, 4 * runs + passes + bloom);
            render_frames(&device, &mut r, format, &scene, &sizes[1..]);
            let stats = r.stats();
            assert_eq!(stats.frames, 3);
            assert_eq!(stats.resources_created, 1);
            assert_eq!(stats.lights_submitted, MAX_LIGHTS as u32);
            // Instances 0..=300 reach lights 0..=33.
            assert_eq!(stats.lights_used, 34);
            // Lights 0, 10, ..., 250, the sun and the moon.
            assert_eq!(stats.coronas, 26 + 2);
            // Nothing moved since the first frame: every cascade is kept.
            assert_eq!((stats.shadow_draw_calls, stats.cascades_drawn), (0, 0));
            assert_eq!(stats.draw_calls, runs + passes + bloom);

            // The depth pre-pass draws every mesh run again, depth only, before them.
            let mut prepass = scene;
            prepass.quality.depth_prepass = true;
            render_frames(&device, &mut r, format, &prepass, &[(64, 48)]);
            assert_eq!(r.stats().draw_calls, 2 * runs + passes + bloom);

            // Bloom off, no lights, no coronas: those passes are skipped.
            scene.sky.bloom_strength = 0.0;
            scene.sky.corona_strength = 0.0;
            scene.lights = &[];
            render_frames(&device, &mut r, format, &scene, &[(64, 48)]);
            let stats = r.stats();
            assert_eq!(stats.bloom_draw_calls, 0);
            assert_eq!((stats.lights_submitted, stats.lights_used), (0, 0));
            assert_eq!(stats.coronas, 0);
            assert_eq!(stats.draw_calls, runs + passes - 1);
            assert_eq!(stats.resources_created, 1);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
    }
}

/// The start-up probe scene draws on every path, a failed corona check leaves coronas
/// out instead of breaking frames, and the stats name the backend.
#[test]
fn probe_scene_and_a_failed_corona_check() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let format = wgpu::TextureFormat::Bgra8Unorm;
    for mode in [HdrMode::Float16, HdrMode::Ldr] {
        let options = RendererOptions {
            hdr: mode,
            ..Default::default()
        };
        let mut r = Renderer::with_options(&device, &queue, format, (32, 18), options);
        assert_eq!(r.stats().backend, "noop");
        // Not GL, so the depth is read with `textureLoad`.
        assert_eq!(r.corona_depth(), CoronaDepth::Loaded);
        let block = r.add_mesh(&Mesh::unit_block([255; 4]));
        let instances = [MeshInstance::new(block, Mat4::IDENTITY.to_cols_array_2d())];
        let mut lamp = PointLight::omni([0.0, 3.0, 0.0], [2.5, 1.6, 0.6], 25.0);
        lamp.corona = Corona {
            radius: 1.0,
            strength: 1.0,
        };
        let lights = [lamp];
        let mut scene = SceneView::probe(&instances, &lights);
        render_frames(&device, &mut r, format, &scene, &[(32, 18)]);
        let stats = r.stats();
        assert_eq!((stats.frames, stats.instances), (1, 1));
        assert!(stats.coronas >= 1, "{stats:?}");

        r.corona_probe = ProbeState::Failed("rejected".to_string());
        assert_eq!(r.corona_probe(), Probe::Failed("rejected"));
        scene.sky.exposure = exposure_from_ev100(3.0);
        render_frames(&device, &mut r, format, &scene, &[(32, 18)]);
        assert_eq!(r.stats().coronas, 0);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    assert_eq!(backend_label(wgpu::Backend::BrowserWebGpu), "WebGPU");
    assert_eq!(backend_label(wgpu::Backend::Gl), "GL");
}

/// Text and rectangles go over the frame in one instanced draw in both HDR modes, on
/// any target aspect; quads past the fixed buffer are dropped and counted.
#[test]
fn hud_draws_text_and_rects_in_one_call() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let mut hud = HudBuilder::new();
    hud.begin(1.0);
    hud.rect(-100.0, -100.0, 2000.0, 1000.0, [0, 0, 0, 120]);
    hud.rect(100.0, 100.0, 400.0, 32.0, [255, 176, 64, 255]);
    hud.text(110.0, 107.0, 2.0, [20, 16, 10, 255], "Invert look Y");
    hud.text_fmt(40.0, 40.0, 1.0, [255; 4], format_args!("{:.2} ms", 16.67));
    let glyphs = "InvertlookY".len() + "16.67ms".len();
    let mut flood = HudBuilder::new();
    flood.begin(2.0);
    flood.text(0.0, 0.0, 1.0, [255; 4], &"#".repeat(MAX_HUD_QUADS + 100));
    for format in [
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8Unorm,
    ] {
        for mode in [HdrMode::Float16, HdrMode::Ldr] {
            let options = RendererOptions {
                hdr: mode,
                ..Default::default()
            };
            let mut r = Renderer::with_options(&device, &queue, format, (64, 36), options);
            let block = r.add_mesh(&Mesh::unit_block([255; 4]));
            let instances = [MeshInstance::new(block, Mat4::IDENTITY.to_cols_array_2d())];
            let mut scene = test_scene(&instances);
            // Twice: from the second frame on the still scene keeps its cascades.
            render_frames(&device, &mut r, format, &scene, &[(128, 72), (128, 72)]);
            let base = r.stats().draw_calls;
            assert_eq!(r.stats().hud_quads, 0);

            scene.hud = hud.view();
            // 16:9, 16:10 (letterboxed) and a target smaller than one glyph.
            for size in [(128, 72), (128, 80), (7, 5)] {
                render_frames(&device, &mut r, format, &scene, &[size]);
                let stats = r.stats();
                assert_eq!(stats.draw_calls, base + 1, "{size:?}");
                // Rects that round to nothing on the tiny target are skipped.
                let min = if size.0 < 10 { glyphs } else { 2 + glyphs };
                assert!((min..=2 + glyphs).contains(&(stats.hud_quads as usize)));
                assert_eq!(stats.hud_quads_dropped, 0);
                assert_eq!(stats.instances, 1);
            }

            scene.hud = flood.view();
            render_frames(&device, &mut r, format, &scene, &[(128, 72)]);
            let stats = r.stats();
            assert_eq!(stats.hud_quads as usize, MAX_HUD_QUADS);
            assert_eq!(stats.hud_quads_dropped, 100);
            assert_eq!(stats.draw_calls, base + 1);
            assert_eq!(stats.resources_created, 0);
            assert_eq!(r.hud_staging.capacity(), MAX_HUD_QUADS);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
    }
}

/// A Vulkan device (lavapipe in a container, or a real GPU), or `None` without one.
fn vulkan_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// A `[0, 0, 0, 160]` HUD panel over an encoded 0.5 background blends in linear light on
/// an sRGB target and on a plain one (the web's WebGPU canvas) alike. Skipped without a
/// Vulkan device, except on CI.
#[test]
fn hud_blends_the_same_on_srgb_and_plain_targets() {
    let Some((device, queue)) = vulkan_device() else {
        assert!(std::env::var_os("CI").is_none(), "no Vulkan device on CI");
        eprintln!("skipped: no Vulkan device (install mesa-vulkan-drivers)");
        return;
    };
    let size = (16, 16);
    let mut hud = HudBuilder::new();
    hud.begin(1.0);
    hud.rect(-100.0, -100.0, 10_000.0, 10_000.0, [0, 0, 0, 160]);
    let mut scene = test_scene(&[]);
    scene.hud = hud.view();
    let bg = 128u8;
    // Linear light: the background's light times the panel's transmittance, encoded.
    let expected = atlas::srgb8(atlas::linear(bg) * (1.0 - 160.0 / 255.0));
    for format in [
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Rgba8Unorm,
    ] {
        let mut r = Renderer::new(&device, &queue, format, size);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[format.add_srgb_suffix()],
        });
        let view = texture.create_view(&Default::default());
        // A whole frame sets the uniforms; then the background is replaced and the HUD
        // drawn over it alone.
        r.render(&view, &scene);
        queue.write_texture(
            texture.as_image_copy(),
            &vec![bg; (4 * size.0 * size.1) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * size.0),
                rows_per_image: None,
            },
            texture.size(),
        );
        r.build_hud(&scene);
        let row = (4 * size.0).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * size.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        assert_eq!(r.draw_hud(&mut encoder, &view, None), 1);
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buffer.get_mapped_range(..).unwrap();
        let px = &data[(8 * row + 8 * 4) as usize..][..3];
        for &c in px {
            assert!(
                c.abs_diff(expected) <= 1,
                "{format:?}: {px:?}, want {expected}"
            );
        }
    }
}

#[test]
fn hud_quad_layout_matches_the_shader() {
    assert_eq!(size_of::<HudQuad>(), 52);
    assert_eq!(HUD_ATTRIBUTES[1].offset, 16);
    assert_eq!(HUD_ATTRIBUTES[2].offset, 32);
    assert_eq!(HUD_ATTRIBUTES[3].offset, 36);
    let wgsl = include_str!("../shaders/hud.wgsl");
    for (i, name) in ["rect", "uv", "color", "params"].iter().enumerate() {
        assert!(wgsl.contains(&format!("@location({i}) {name}: vec4<f32>")));
    }
    // The quad kinds and masks agree with the shader's.
    for (name, value) in [
        ("QUAD_FONT", hud::QUAD_FONT),
        ("QUAD_MAP", hud::QUAD_MAP),
        ("QUAD_SOLID", hud::QUAD_SOLID),
        ("QUAD_ARROW", hud::QUAD_ARROW),
        ("MASK_CIRCLE", hud::MASK_CIRCLE),
        ("MASK_ROUNDED", hud::MASK_ROUNDED),
    ] {
        assert!(
            wgsl.contains(&format!("const {name}: u32 = {value}u;")),
            "{name}"
        );
    }
    // Three textures and two samplers: far under WebGL2's 16 per stage.
    assert!(wgsl.contains("@group(1) @binding(3) var map_sampler: sampler;"));
}

/// Map images draw in the same single HUD draw as text (the shader switches textures
/// per quad), before and after a map is uploaded, and uploading allocates nothing per
/// frame afterwards.
#[test]
fn hud_draws_map_images_in_the_same_call() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let mut r = Renderer::new(&device, &queue, format, (128, 72));
    assert_eq!(r.map_size(), [1, 1]);
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let instances = [MeshInstance::new(block, Mat4::IDENTITY.to_cols_array_2d())];
    let mut scene = test_scene(&instances);
    // Twice: from the second frame on the still scene keeps its cascades.
    render_frames(&device, &mut r, format, &scene, &[(128, 72), (128, 72)]);
    let base = r.stats().draw_calls;

    let mut hud = HudBuilder::new();
    hud.begin(1.0);
    hud.image(HudImage {
        x: 20.0,
        y: 400.0,
        w: 200.0,
        h: 200.0,
        uv: [0.4, 0.4, 0.6, 0.6],
        rotation: 1.0,
        texture: HudTexture::Map,
        mask: HudMask::Circle,
        color: [255; 4],
    });
    hud.image(HudImage::arrow(120.0, 500.0, 24.0, 0.0, [255; 4]));
    hud.image(HudImage::disc(120.0, 400.0, 4.0, [255; 4]));
    hud.text(116.0, 380.0, 2.0, [255; 4], "N");
    scene.hud = hud.view();
    render_frames(&device, &mut r, format, &scene, &[(128, 72)]);
    assert_eq!(r.stats().draw_calls, base + 1);
    assert_eq!(r.stats().hud_quads, 4);

    // A 64×32 map with its mip chain; a wrong-sized buffer falls back to 1×1.
    let rgba: Vec<u8> = (0..64 * 32 * 4).map(|i| (i % 251) as u8).collect();
    r.upload_hud_image(HudImageTarget::Map, 64, 32, &rgba);
    assert_eq!(r.map_size(), [64, 32]);
    let sizes = [(128, 72); WARMUP_FRAMES as usize + 2];
    render_frames(&device, &mut r, format, &scene, &sizes);
    let stats = r.stats();
    assert_eq!(stats.draw_calls, base + 1);
    assert_eq!(stats.hud_quads, 4);
    assert_eq!(stats.resources_created, 0);
    r.upload_hud_image(HudImageTarget::Map, 64, 32, &rgba[..10]);
    assert_eq!(r.map_size(), [1, 1]);
    r.upload_hud_image(HudImageTarget::Map, MAX_MAP_SIZE + 1, 1, &[]);
    assert_eq!(r.map_size(), [1, 1]);
    // Stills likewise, drawn in the same call.
    assert_eq!(r.stills_size(), [1, 1]);
    r.upload_hud_image(HudImageTarget::Stills, 32, 16, &rgba[..32 * 16 * 4]);
    assert_eq!(r.stills_size(), [32, 16]);
    let still = HudImage {
        x: 300.0,
        y: 100.0,
        w: 160.0,
        h: 100.0,
        uv: [0.5, 0.0, 1.0, 1.0],
        texture: HudTexture::Still,
        ..HudImage::default()
    };
    let mut stills = HudBuilder::new();
    stills.begin(1.0);
    stills.image(still);
    stills.image(HudImage {
        color: [255; 4],
        ..still
    });
    stills.text(300.0, 80.0, 2.0, [255; 4], "A");
    let mut with_stills = test_scene(&instances);
    with_stills.hud = stills.view();
    render_frames(&device, &mut r, format, &with_stills, &[(128, 72)]);
    assert_eq!(r.stats().draw_calls, base + 1);
    assert_eq!(r.stats().hud_quads, 2, "the transparent still is skipped");
    r.upload_hud_image(HudImageTarget::Stills, 32, 16, &[]);
    assert_eq!(r.stills_size(), [1, 1]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// The noop device renders Rgba16Float, so detection picks the float path; with full
/// texture limits the cascades get their full size.
#[test]
fn auto_mode_detects_float_targets_and_full_cascades() {
    let limits = wgpu::Limits {
        max_texture_dimension_2d: 8192,
        ..wgpu::Limits::downlevel_webgl2_defaults()
    };
    let (device, queue) = noop_device(limits);
    let r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, (8, 8));
    assert!(r.is_hdr());
    assert_eq!(r.shadow_cascade_size(), SHADOW_CASCADE_SIZE);
    assert_eq!(r.cascade_splits(), DEFAULT_CASCADE_SPLITS);
}

/// Chunk parts draw their slot's index runs from the pool's buffers (one draw per run
/// of instances, like meshes); freed or unwritten parts draw nothing; writing, freeing
/// and refusing never creates a buffer.
#[test]
fn chunk_slots_draw_their_parts_from_the_pool() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let config = ChunkPoolConfig {
        slots: 2,
        pages: 3,
        page_vertices: 64,
        page_indices: 96,
    };
    let options = RendererOptions {
        hdr: HdrMode::Ldr,
        chunk_pool: config,
        ..Default::default()
    };
    let mut r = Renderer::with_options(&device, &queue, format, (32, 24), options);
    assert_eq!(r.stats().chunk_pool.bytes, 3 * 64 * 36 + 3 * 96 * 4);
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let id = MeshInstance::new(block, Mat4::IDENTITY.to_cols_array_2d());
    let draws = |r: &mut Renderer, instances: &[MeshInstance]| {
        render_frames(&device, r, format, &test_scene(instances), &[(32, 24)]);
        let s = r.stats();
        (s.draw_calls - s.shadow_draw_calls, s.shadow_draw_calls)
    };
    let (forward0, shadow0) = draws(&mut r, &[id]);

    // Two cuboids in one slot (two parts), one in the other.
    let a = r.alloc_chunk_slot().unwrap();
    let b = r.alloc_chunk_slot().unwrap();
    assert_eq!(r.alloc_chunk_slot(), Err(PoolError::Full));
    let mut two = Mesh::cuboid([0.0; 3], [1.0; 3], [200; 4]);
    let far = Mesh::cuboid([3.0, 0.0, 0.0], [4.0, 1.0, 1.0], [90; 4]);
    let base = two.vertices.len() as u32;
    two.vertices.extend_from_slice(&far.vertices);
    two.indices.extend(far.indices.iter().map(|i| i + base));
    r.write_chunk(a, &two, &[[0, 36], [36, 36]]).unwrap();
    r.write_chunk(b, &far, &[[0, 36]]).unwrap();
    let part = |slot, k| MeshInstance::new(MeshId::chunk_part(slot, k), id.transform);
    // Too big for the pool's three pages: refused, nothing created.
    let big = Mesh::cuboid([0.0; 3], [1.0; 3], [1; 4]);
    let mut huge = Mesh::default();
    for _ in 0..10 {
        append_mesh(&mut huge, &big);
    }
    assert!(matches!(
        r.write_chunk(b, &huge, &[]),
        Err(PoolError::TooBig { .. })
    ));
    let instances = [
        id,
        part(a, 0),
        part(a, 0),
        part(a, 1),
        part(b, 0),
        // Never written: skipped.
        part(b, 5),
    ];
    let (forward, shadow) = draws(&mut r, &instances);
    assert_eq!(forward, forward0 + 3, "one draw per run of a part");
    assert!(shadow > shadow0);
    // The pool's buffers are bound once per sweep, not once per draw: a frame with one
    // pool run binds as often as one with three.
    let binds = r.stats().buffer_binds;
    draws(&mut r, &[id, part(a, 0)]);
    assert_eq!(r.stats().buffer_binds, binds);
    let stats = r.stats().chunk_pool;
    assert_eq!((stats.used, stats.peak, stats.writes), (2, 2, 2));
    assert_eq!(stats.pages_used, 2);
    // Freed, b's part draws nothing; its slot comes back.
    r.free_chunk_slot(b).unwrap();
    assert_eq!(r.free_chunk_slot(b), Err(PoolError::BadSlot(b)));
    let (forward, _) = draws(&mut r, &instances);
    assert_eq!(forward, forward0 + 2);
    assert_eq!(r.alloc_chunk_slot(), Ok(b));
    assert_eq!(r.stats().resources_created, 0);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

fn append_mesh(all: &mut Mesh, part: &Mesh) {
    let base = all.vertices.len() as u32;
    all.vertices.extend_from_slice(&part.vertices);
    all.indices.extend(part.indices.iter().map(|i| i + base));
}

/// After warmup, frames reuse every buffer: no GPU resources are created and the
/// staging vector never reallocates.
#[test]
fn steady_frames_allocate_nothing() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let mut r = Renderer::new(&device, &queue, format, (16, 16));
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let instances: Vec<MeshInstance> = (0..1000)
        .map(|i| {
            MeshInstance::new(
                block,
                Mat4::from_translation(Vec3::new(i as f32, 0.0, 0.0)).to_cols_array_2d(),
            )
        })
        .collect();
    r.reserve_instances(instances.len());
    let mut hud = HudBuilder::new();
    hud.begin(1.0);
    hud.rect(0.0, 0.0, 100.0, 20.0, [0, 0, 0, 128]);
    hud.text(4.0, 4.0, 2.0, [255; 4], "frame 16.7 ms");
    let mut scene = test_scene(&instances);
    scene.hud = hud.view();
    let capacity = r.instance_staging.capacity();
    let sizes = [(16, 16); WARMUP_FRAMES as usize + 4];
    render_frames(&device, &mut r, format, &scene, &sizes);
    // Lights, coronas and bloom on or off change nothing either.
    let mut lights = lamps(MAX_LIGHTS);
    for l in &mut lights {
        l.corona = Corona {
            radius: 1.0,
            strength: 1.0,
        };
    }
    let corona_capacity = r.corona_staging.capacity();
    let mut lit = test_scene_lit(&instances, &lights);
    lit.sky.exposure = exposure_from_ev100(3.0);
    render_frames(&device, &mut r, format, &lit, &[(16, 16)]);
    lit.sky.bloom_strength = 0.0;
    render_frames(&device, &mut r, format, &lit, &[(16, 16)]);
    assert_eq!(r.stats().resources_created, 0);
    assert_eq!(r.instance_staging.capacity(), capacity);
    assert_eq!(
        r.stats().coronas as usize,
        MAX_LIGHTS + 2,
        "and the sun and moon"
    );
    assert_eq!(r.corona_staging.capacity(), corona_capacity);
    // Fewer instances later is fine too.
    render_frames(
        &device,
        &mut r,
        format,
        &test_scene(&instances[..10]),
        &[(16, 16)],
    );
    assert_eq!(r.stats().resources_created, 0);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// The web flicker on thin layers: a driver's-eye view of a radio display half a
/// millimetre in front of its face, 2.5 km from the origin, the car moving a little each
/// frame. With the shader's float32 math in world coordinates the display lost to the
/// face on some frames (the black wedges flickering over it); camera relative, never.
#[test]
fn thin_layers_far_from_the_origin_never_swap() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, (8, 8));
    let (mut world_swaps, mut rel_swaps) = (0, 0);
    for k in 0..400 {
        let t = k as f32;
        let car = Vec3::new(
            2400.0 + 0.37 * t,
            30.0 + 0.01 * (t * 0.3).sin(),
            2450.0 + 0.11 * t,
        );
        let yaw = 0.3 + 0.002 * t;
        let rot = glam::Quat::from_rotation_y(yaw);
        let eye = car + rot * Vec3::new(0.35, 1.1, -0.3);
        let ahead = eye + rot * Vec3::new(0.0, -0.4, 1.0);
        let mut scene = test_scene(&[]);
        scene.camera = Camera {
            view: glam::camera::rh::view::look_at_mat4(eye, ahead, Vec3::Y).to_cols_array_2d(),
            proj: glam::camera::rh::proj::directx::perspective(1.0, 16.0 / 9.0, 0.1, 3000.0)
                .to_cols_array_2d(),
        };
        let (u, _) = r.frame_uniforms(&scene);
        let model = Mat4::from_rotation_translation(rot, car);
        let inst = MeshInstance::new(MeshId(0), model.to_cols_array_2d());
        let raw = InstanceRaw::new(&inst, 0, Vec3::from_slice(&u.camera_pos[..3]));
        // The face and the display in front of it toward the driver, car local.
        let face = Vec3::new(0.3 + 0.0001 * t, 1.0, 0.9);
        let display = face - Vec3::Z * 0.0005;
        let depth = |m: [[f32; 4]; 4], vp: [[f32; 4]; 4], p: Vec3| {
            let c = Mat4::from_cols_array_2d(&vp) * (Mat4::from_cols_array_2d(&m) * p.extend(1.0));
            c.z / c.w
        };
        world_swaps += usize::from(
            depth(inst.transform, u.view_proj, display) >= depth(inst.transform, u.view_proj, face),
        );
        rel_swaps += usize::from(
            depth(raw.model, u.rel_view_proj, display) >= depth(raw.model, u.rel_view_proj, face),
        );
    }
    assert!(
        world_swaps > 0,
        "the world-space math this replaces did swap"
    );
    assert_eq!(rel_swaps, 0);
}

#[test]
fn frame_uniforms_carry_cascades_and_sky() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, (8, 8));
    r.set_cascade_splits([30.0, 100.0, 400.0]);
    let scene = test_scene(&[]);
    let (u, c) = r.frame_uniforms(&scene);
    assert_eq!(&u.cascade_far[..3], &[30.0, 100.0, 400.0]);
    // The shadow pass draws camera-relative instances: its matrices take them back to
    // the world first; the forward pass samples with the world ones.
    let eye = Vec3::from_slice(&u.camera_pos[..3]);
    for (m, cascade) in u.cascade_view_proj.iter().zip(&c) {
        let rel = Mat4::from_cols_array_2d(m) * Mat4::from_translation(eye);
        assert_eq!(rel.to_cols_array_2d(), cascade.view_proj);
    }
    // Meshes: the same clip position from a camera-relative vertex as from the world.
    let world = Vec3::new(3.0, 1.0, -7.0);
    let a = Mat4::from_cols_array_2d(&u.view_proj) * world.extend(1.0);
    let b = Mat4::from_cols_array_2d(&u.rel_view_proj) * (world - eye).extend(1.0);
    assert!((a - b).abs().max_element() < 1e-4, "{a} {b}");
    assert!(u.cascade_texel[0] < u.cascade_texel[1]);
    // Sky ambient is blue-ish and dimmer than the sun on a white surface.
    assert!(u.sky_up[2] > u.sky_up[0]);
    assert!(u.sky_up[1] < u.sun_light[1]);
    assert_eq!(u.params[0], scene.sky.exposure);
    assert_eq!(
        u.params[1], 1.0,
        "Rgba8Unorm needs sRGB encoding in the shader"
    );
    assert_eq!(u.emissive, [0.5, 0.0, 0.0, 8.0]);
    assert_eq!(u.post, [0.08, 1.0, VIGNETTE, BLOOM_CLAMP]);
    assert_eq!(u.screen, [8.0, 8.0, 0.125, 0.125]);
    assert_eq!(u.cabin_light, [0.0; 4]);
    assert!(u
        .lights
        .iter()
        .all(|l| l.pos_radius == [0.0; 4] && l.bits == [0; 4]));
    // Corona sizes: the focal length of a 1 rad vertical field on 8 lines.
    assert!((u.corona[0] - 4.0 / (0.5f32).tan()).abs() < 1e-4);
    assert!(u.corona[1] < u.corona[2]);
}

#[test]
fn frame_uniforms_carry_lights_emissive_and_post() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(&device, &queue, format, (8, 8));
    r.resize((640, 360));
    let mut lights = lamps(MAX_LIGHTS + 2);
    // The first is a spot looking down -z, full inside 10°, dark beyond 20°.
    lights[0].direction = [0.0, 0.0, -2.0];
    lights[0].cone_cos = [20f32.to_radians().cos(), 10f32.to_radians().cos()];
    let mut scene = test_scene_lit(&[], &lights);
    scene.sky.window_emissive_scale = 1.5;
    scene.sky.emissive_boost = 0.25;
    scene.sky.emissive_cap = 3.0;
    scene.sky.bloom_threshold = 2.0;
    scene.sky.cabin_light = [0.02, -1.0, 0.01];
    let (u, _) = r.frame_uniforms(&scene);
    assert_eq!(u.emissive, [0.25, 1.0, MAX_LIGHTS as f32, 3.0]);
    assert_eq!(u.post[..2], [0.08, 2.0]);
    assert_eq!(u.screen[..2], [640.0, 360.0]);
    assert_eq!(u.cabin_light, [0.02, 0.0, 0.01, 0.0], "never negative");
    assert_eq!(u.params[1], 0.0, "the sRGB target encodes in hardware");
    for (k, l) in u.lights.iter().enumerate() {
        let l = unpack(l);
        assert_eq!(l.pos, Vec3::new(k as f32 * 10.0, 7.5, 0.0));
        assert_eq!(l.radius, 25.0);
        // Half floats: within a part in a thousand.
        let c = Vec3::new(2.5, 1.6, 0.6);
        assert!(
            (l.color - c).abs().max_element() < 1e-3 * 2.5,
            "{}",
            l.color
        );
    }
    // The spot's axis is normalized and its cone mapped onto the cosine ramp the shader
    // smoothsteps; omni lights pass every direction.
    let cone = |l: &Unpacked, dir: Vec3| {
        let c = ((dir.normalize().dot(l.axis) - l.cone_end) * l.cone_scale).clamp(0.0, 1.0);
        c * c * (3.0 - 2.0 * c)
    };
    let spot = unpack(&u.lights[0]);
    assert!((spot.axis - Vec3::NEG_Z).length() < 1e-4, "{}", spot.axis);
    let off = |deg: f32| Vec3::new(deg.to_radians().sin(), 0.0, -deg.to_radians().cos());
    assert!((cone(&spot, off(5.0)) - 1.0).abs() < 1e-6);
    assert!((cone(&spot, off(10.0)) - 1.0).abs() < 2e-2);
    assert!((0.1..0.9).contains(&cone(&spot, off(15.0))));
    assert_eq!(cone(&spot, off(25.0)), 0.0);
    assert_eq!(cone(&spot, Vec3::Z), 0.0, "nothing behind it");
    let omni = unpack(&u.lights[1]);
    for dir in [Vec3::X, -Vec3::Y, Vec3::Z, Vec3::new(1.0, 2.0, -3.0)] {
        assert_eq!(cone(&omni, dir), 1.0);
    }
    assert_eq!(
        (spot.squash, spot.wrap),
        (1.0, 0.0),
        "round and Lambert by default"
    );
    assert_eq!((omni.squash, omni.wrap), (1.0, 0.0));

    // A flattened, wrapped beam keeps its squash and wrap; a vertical one cannot squash.
    let mut beams = lights.clone();
    beams[2].direction = [0.0, -0.1, -1.0];
    beams[2].cone_cos = [0.8, 0.9];
    beams[2].cone_squash = 4.0;
    beams[2].wrap = 0.25;
    beams[3].direction = [0.0, -1.0, 0.0];
    beams[3].cone_cos = [0.3, 0.7];
    beams[3].cone_squash = 3.0;
    let (b, _) = r.frame_uniforms(&test_scene_lit(&[], &beams));
    let beam = unpack(&b.lights[2]);
    assert_eq!(beam.squash, 4.0);
    assert!((beam.wrap - 0.25).abs() < 0.5 / 255.0);
    assert!((beam.axis - Vec3::new(0.0, -0.1, -1.0).normalize()).length() < 1e-4);
    assert!((beam.cone_end - 0.8).abs() < 1e-3 && (beam.cone_scale - 10.0).abs() < 0.02);
    let down = unpack(&b.lights[3]);
    assert_eq!(down.squash, 1.0);
    assert!((down.axis - Vec3::NEG_Y).length() < 1e-4);

    // Without float targets there is no bloom.
    let ldr = Renderer::with_options(
        &device,
        &queue,
        format,
        (8, 8),
        RendererOptions {
            hdr: HdrMode::Ldr,
            ..Default::default()
        },
    );
    assert_eq!(ldr.frame_uniforms(&scene).0.post[0], 0.0);
}

/// A lamp's corona glows at night, fades through dusk and is gone by day; the sun has a
/// faint one by day and the moon one at night, each only above the horizon.
#[test]
fn coronas_follow_their_lights_against_the_exposure() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, (8, 8));
    let mut lamp = lamps(1)[0];
    lamp.corona = Corona {
        radius: 1.5,
        strength: 1.0,
    };
    let lights = [lamp];
    let peak = |r: &mut Renderer, ev: f32, sun_y: f32, moon: f32| {
        let mut scene = test_scene_lit(&[], &lights);
        scene.sky.exposure = exposure_from_ev100(ev);
        scene.sun.direction = [0.0, sun_y, -1.0];
        scene.sky.moon_luminance = moon;
        r.corona_staging.clear();
        r.push_coronas(&scene);
        r.corona_staging
            .iter()
            .map(|c| {
                (
                    c.pos_size[3],
                    Vec4::from(c.color).max_element() * scene.sky.exposure,
                )
            })
            .collect::<Vec<_>>()
    };
    let strength = Sky::default().corona_strength;
    // Night (EV 3), sun well down, no moon: the lamp only, near the cap, sodium hued.
    let night = peak(&mut r, 3.0, -0.5, 0.0);
    assert_eq!(night.len(), 1);
    assert_eq!(night[0].0, 1.5);
    assert!(
        (0.7..1.0).contains(&(night[0].1 / (CORONA_PEAK * strength))),
        "{night:?}"
    );
    let rgb = Vec4::from(r.corona_staging[0].color);
    assert!(rgb.x > rgb.y && rgb.y > rgb.z, "warm: {rgb}");
    // Dusk (EV 6): dimmer. Noon (EV 15): nothing from the lamp, a faint sun.
    let dusk = peak(&mut r, 6.0, -0.1, 0.0);
    assert!(
        dusk[0].1 < 0.8 * night[0].1 && dusk[0].1 > 0.2 * night[0].1,
        "{dusk:?}"
    );
    let noon = peak(&mut r, 15.0, 1.0, 0.0);
    assert_eq!(noon.len(), 1, "{noon:?}");
    assert!(noon[0].0 < 0.0, "at infinity");
    assert!(noon[0].1 < 0.5 * CORONA_PEAK * strength, "{noon:?}");
    // A full moon at night (sun opposite it) adds its own; strength 0 hides them all.
    let moonlit = peak(&mut r, 3.0, -0.5, 2.5);
    assert_eq!(moonlit.len(), 2);
    let moon = moonlit[1];
    assert!(moon.0 == -MOON_CORONA_RADIUS && moon.1 > 0.05, "{moon:?}");
    let mut scene = test_scene_lit(&[], &lights);
    scene.sky.corona_strength = 0.0;
    r.corona_staging.clear();
    r.push_coronas(&scene);
    assert!(r.corona_staging.is_empty());
}

/// At the default exposure the daytime sky more than 30° from the sun stays under the
/// bloom threshold, so bloom does not haze the frame at noon; only the aureole right
/// around the sun crosses it (by at most ~50%), and the disc itself blooms fully.
#[test]
fn noon_sky_does_not_bloom() {
    let sky = Sky::default();
    let luma = Vec3::new(0.2126, 0.7152, 0.0722);
    for elev in [20f32, 35.0, 60.0, 80.0] {
        let e = elev.to_radians();
        let sun = Sun {
            direction: [0.0, e.sin(), -e.cos()],
            illuminance: [100.0, 97.0, 90.0],
        };
        let model = SkyModel::new(&sun, &sky);
        let (mut aureole, mut rest) = (0f32, 0f32);
        for i in 0..=45 {
            for j in 0..180 {
                let (el, az) = ((i as f32 * 2.0).to_radians(), (j as f32 * 2.0).to_radians());
                let d = Vec3::new(el.cos() * az.sin(), el.sin(), -el.cos() * az.cos());
                let lum = (model.radiance(d) * sky.exposure).dot(luma);
                let angle = d.dot(model.sun_dir).clamp(-1.0, 1.0).acos().to_degrees();
                if angle > 30.0 {
                    rest = rest.max(lum);
                } else if angle > 2.0 {
                    aureole = aureole.max(lum);
                }
            }
        }
        assert!(rest < sky.bloom_threshold, "{elev}: {rest}");
        assert!(aureole < 1.6 * sky.bloom_threshold, "{elev}: {aureole}");
        assert!(model.sun_disc.max_element() * sky.exposure > BLOOM_CLAMP);
    }
}

/// A renderer on the no-op device with room for full-size cascades.
fn full_renderer(options: RendererOptions) -> (wgpu::Device, Renderer) {
    let limits = wgpu::Limits {
        max_texture_dimension_2d: 8192,
        ..wgpu::Limits::downlevel_webgl2_defaults()
    };
    let (device, queue) = noop_device(limits);
    let r = Renderer::with_options(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        (64, 48),
        options,
    );
    (device, r)
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn at(mesh: MeshId, p: Vec3) -> MeshInstance {
    MeshInstance::new(mesh, Mat4::from_translation(p).to_cols_array_2d())
}

/// The test camera sits at (0, 10, 30) looking at the origin: instances out of its
/// frustum are not drawn in the forward pass, those in it are, and a caster out of
/// view between the sun and the ground still draws into the shadow cascade.
#[test]
fn culling_drops_what_no_pass_can_see() {
    let (device, mut r) = full_renderer(RendererOptions::default());
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let to_sun = Vec3::new(0.3, 0.8, 0.5).normalize();
    let instances = [
        at(block, Vec3::ZERO),
        at(block, Vec3::new(-5.0, 0.0, -20.0)),
        // Behind the camera, on the side away from the sun.
        at(block, Vec3::new(0.0, 0.0, 60.0)),
        // Far off to the right, and past the far cascade.
        at(block, Vec3::new(900.0, 0.0, 0.0)),
        // Up toward the sun from the origin, above and behind the view.
        at(block, to_sun * 45.0),
    ];
    render_frames(
        &device,
        &mut r,
        FORMAT,
        &test_scene(&instances),
        &[(64, 48)],
    );
    let s = r.stats();
    assert_eq!(s.instances, 5);
    assert_eq!(s.instances_drawn, 2, "{s:?}");
    let forward: Vec<u32> = r.order[..2].to_vec();
    assert_eq!(forward, [0, 1]);
    // The near cascade holds the visible blocks and the one between them and the sun.
    let shadow = s.shadow_instances[0] as usize;
    let c0 = &r.order[2..2 + shadow];
    assert!(
        c0.contains(&0) && c0.contains(&1) && c0.contains(&4),
        "{c0:?}"
    );
    assert!(!r.order.contains(&3), "{:?}", r.order);
    assert_eq!(s.triangles, 2 * 12);
    // Off, every instance is drawn everywhere.
    r.set_culling(false);
    render_frames(
        &device,
        &mut r,
        FORMAT,
        &test_scene(&instances),
        &[(64, 48)],
    );
    let s = r.stats();
    assert_eq!(s.instances_drawn, 5);
    assert_eq!(s.shadow_instances, [5; CASCADES]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// Every caster that can shadow a cascade's slice is kept for that cascade: boxes at
/// each slice corner and up to 300 m toward the sun from it pass the cascade's cull;
/// a box well beside the slice does not.
#[test]
fn cascade_culling_keeps_every_caster_of_the_slice() {
    use glam::camera::rh::{proj::directx, view};
    let proj = directx::perspective(62f32.to_radians(), 16.0 / 9.0, 0.25, 3000.0);
    let suns = [
        Vec3::new(0.70, 0.55, 0.45),
        Vec3::new(-0.2, 0.98, 0.1),
        Vec3::new(0.9, 0.06, -0.4),
    ];
    for (s, to_sun) in suns.iter().enumerate() {
        let to_sun = to_sun.normalize();
        for k in 0..12 {
            let yaw = k as f32 * 0.7 + s as f32;
            let dir = Vec3::new(-yaw.sin(), -0.1, -yaw.cos());
            let eye = Vec3::new(k as f32 * 13.1, 1.5, k as f32 * -7.3);
            let v = view::look_to_mat4(eye, dir, Vec3::Y);
            let fits = fit_cascades(
                v,
                proj,
                to_sun,
                DEFAULT_CASCADE_SPLITS,
                0.1,
                2048,
                CASTER_MARGIN,
            );
            for fit in &fits {
                let f = Frustum::from_matrix(fit.view_proj);
                let [n, far] = fit.depth_range;
                for p in slice_corners(v, proj, n, far) {
                    for t in [0.0, 100.0, 300.0] {
                        let c = p + to_sun * t;
                        assert!(f.intersects(c, Vec3::splat(0.5)), "sun {s} k {k} t {t}");
                    }
                }
                let side = (fit.center
                    + to_sun.cross(Vec3::Y).normalize_or(Vec3::X) * 3.0 * fit.radius)
                    - to_sun * fit.radius;
                assert!(!f.intersects(side, Vec3::splat(0.5)), "sun {s} k {k}");
            }
        }
    }
}

/// Instances that do not cast are left out of the cascades; `late` ones are drawn
/// after the rest in the forward pass; casters a fraction of a far-cascade texel wide
/// are left out of it when the quality says so.
#[test]
fn shadow_flags_late_instances_and_small_casters() {
    let (device, mut r) = full_renderer(RendererOptions {
        culling: false,
        ..Default::default()
    });
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let ground = r.add_mesh(&Mesh::ground_quad(10.0, 10.0, [90, 120, 60, 255]));
    let glow = MeshInstance {
        casts_shadow: false,
        ..at(block, Vec3::X)
    };
    let backdrop = MeshInstance {
        late: true,
        ..at(ground, Vec3::ZERO)
    };
    let tiny = MeshInstance::new(
        block,
        Mat4::from_scale_rotation_translation(Vec3::splat(0.05), glam::Quat::IDENTITY, Vec3::ZERO)
            .to_cols_array_2d(),
    );
    let instances = [backdrop, glow, at(block, Vec3::ZERO), tiny];
    let mut scene = test_scene(&instances);
    render_frames(&device, &mut r, FORMAT, &scene, &[(64, 48)]);
    let s = r.stats();
    assert_eq!(s.instances_drawn, 4);
    assert_eq!(r.order[..4], [1, 2, 3, 0], "the backdrop last");
    assert_eq!(s.shadow_instances, [3; CASCADES], "no glow");
    scene.quality.min_caster_texels = 2.0;
    render_frames(&device, &mut r, FORMAT, &scene, &[(64, 48)]);
    let s = r.stats();
    // 5 cm is over two texels of the near cascade only: the far two lose it and are
    // drawn again, the near one keeps its casters and its map.
    assert_eq!(s.shadow_instances, [0, 2, 2], "{s:?}");
    assert_eq!(r.vis[3] & 0b1110, 0b0010, "{s:?}");
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// Glass goes last in the forward pass, after the late backdrop, and the frame
/// validates with the blended pipeline drawing it.
#[test]
fn glass_is_drawn_last() {
    let (device, mut r) = full_renderer(RendererOptions {
        culling: false,
        ..Default::default()
    });
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let ground = r.add_mesh(&Mesh::ground_quad(10.0, 10.0, [90, 120, 60, 255]));
    let pane = MeshInstance {
        glass: true,
        casts_shadow: false,
        color: [1.0, 1.0, 1.0, 0.5],
        ..at(block, Vec3::Y)
    };
    let backdrop = MeshInstance {
        late: true,
        ..at(ground, Vec3::ZERO)
    };
    let instances = [pane, backdrop, at(block, Vec3::ZERO)];
    let scene = test_scene(&instances);
    render_frames(&device, &mut r, FORMAT, &scene, &[(64, 48)]);
    assert_eq!(r.stats().instances_drawn, 3);
    assert_eq!(r.order[..3], [2, 1, 0], "the backdrop, then the glass");
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// At half resolution the scene's targets are half the swapchain, the HUD is laid out
/// for the full swapchain (the same quads as at full resolution) and coronas, bloom and
/// tonemap still validate.
#[test]
fn half_resolution_scale_keeps_the_hud_at_full_size() {
    let (device, mut r) = full_renderer(RendererOptions::default());
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let instances = [at(block, Vec3::ZERO)];
    let mut lights = lamps(4);
    lights[0].corona = Corona {
        radius: 1.0,
        strength: 1.0,
    };
    let mut hud = HudBuilder::new();
    hud.begin(1.0);
    hud.rect(10.0, 10.0, 200.0, 30.0, [0, 0, 0, 160]);
    hud.text(14.0, 14.0, 2.0, [255; 4], "60 fps");
    let mut scene = test_scene_lit(&instances, &lights);
    scene.sky.exposure = exposure_from_ev100(3.0);
    scene.hud = hud.view();
    render_frames(&device, &mut r, FORMAT, &scene, &[(640, 360)]);
    let full: Vec<HudQuad> = r.hud_staging.clone();
    assert_eq!(r.render_size(), (640, 360));
    scene.quality.resolution_scale = 0.5;
    render_frames(&device, &mut r, FORMAT, &scene, &[(640, 360)]);
    let s = r.stats();
    assert_eq!((s.surface_size, s.render_size), ((640, 360), (320, 180)));
    assert_eq!(r.render_size(), (320, 180));
    assert_eq!(s.rebuilds, 1);
    assert!(s.coronas > 0 && s.bloom_levels == BLOOM_LEVELS as u32);
    assert_eq!(
        bytemuck::cast_slice::<HudQuad, u8>(&r.hud_staging),
        bytemuck::cast_slice::<HudQuad, u8>(&full),
        "the HUD does not scale"
    );
    let (u, _) = r.frame_uniforms(&scene);
    assert_eq!(u.screen[..2], [640.0, 360.0]);
    assert_eq!(u.render_size[..2], [320.0, 180.0]);
    // A resize keeps the scale.
    render_frames(&device, &mut r, FORMAT, &scene, &[(100, 50)]);
    assert_eq!(r.render_size(), (50, 25));
    assert_eq!(r.stats().rebuilds, 1, "a resize is not a quality rebuild");
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// Quality changes apply on the next frame: the cascade size rebuilds the atlas, two
/// cascades draw two, fewer bloom levels draw fewer passes, coronas can be switched
/// off, and a kept far cascade is drawn every other frame of a still camera.
#[test]
fn quality_changes_apply_on_the_next_frame() {
    let (device, mut r) = full_renderer(RendererOptions::default());
    assert_eq!(r.shadow_cascade_size(), SHADOW_CASCADE_SIZE);
    assert_eq!(
        r.shadow_atlas_size(),
        (3 * SHADOW_CASCADE_SIZE, SHADOW_CASCADE_SIZE)
    );
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let instances = [at(block, Vec3::ZERO)];
    let mut scene = test_scene(&instances);
    scene.quality = RenderQuality {
        shadow_size: 1024,
        cascades: 2,
        bloom_levels: 4,
        coronas: false,
        ..RenderQuality::default()
    };
    render_frames(&device, &mut r, FORMAT, &scene, &[(64, 48)]);
    let s = r.stats();
    assert_eq!(r.shadow_cascade_size(), 1024);
    assert_eq!(
        r.shadow_atlas_size(),
        (2 * 1024, 1024),
        "two cascades, two slices"
    );
    assert_eq!((s.shadow_size, s.cascades, s.cascades_drawn), (1024, 2, 2));
    assert_eq!(s.shadow_instances[2], 0);
    assert_eq!(s.bloom_draw_calls, 7);
    assert_eq!(s.coronas, 0, "not even the sun's");
    assert_eq!(s.rebuilds, 1);
    let (u, _) = r.frame_uniforms(&scene);
    assert_eq!(u.shadow_params[0], 2.0);
    assert_eq!(u.cascade_far[1], TWO_CASCADE_SPLITS[1]);
    assert_eq!(u.cascade_far[2], u.cascade_far[1]);

    scene.quality = RenderQuality {
        far_cascade_interval: 2,
        ..RenderQuality::default()
    };
    // A caster that moves a little every frame, under a still camera.
    let mut step = 0;
    let mut frame = |r: &mut Renderer, scene: &SceneView| {
        step += 1;
        let moving = [at(block, Vec3::new(0.01 * step as f32, 0.0, 0.0))];
        let scene = SceneView {
            instances: &moving,
            ..*scene
        };
        render_frames(&device, r, FORMAT, &scene, &[(64, 48)]);
        r.stats().cascades_drawn
    };
    let drawn: Vec<u32> = (0..6).map(|_| frame(&mut r, &scene)).collect();
    assert_eq!(r.stats().rebuilds, 2);
    // Drawn whole after the change, then the far cascade every other frame.
    assert_eq!(drawn[0], 3, "{drawn:?}");
    assert!(drawn.windows(2).all(|w| w[0] + w[1] >= 5), "{drawn:?}");
    assert_eq!(drawn.iter().filter(|&&d| d == 2).count(), 2, "{drawn:?}");
    // Once it stops (and the far cascade has caught up), every cascade is kept.
    render_frames(&device, &mut r, FORMAT, &scene, &[(64, 48); 3]);
    assert_eq!(r.stats().cascades_drawn, 0);
    // A camera that jumps away cannot keep the old map, even on a frame that would.
    let mut moved = scene;
    moved.camera.view = glam::camera::rh::view::look_at_mat4(
        Vec3::new(500.0, 10.0, 30.0),
        Vec3::new(500.0, 0.0, 0.0),
        Vec3::Y,
    )
    .to_cols_array_2d();
    render_frames(&device, &mut r, FORMAT, &moved, &[(64, 48)]);
    assert_eq!(r.stats().cascades_drawn, 3);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// With the sun below the horizon no cascade is drawn and the shader is told to skip
/// the shadow test; the first frame after dawn draws every cascade again.
#[test]
fn no_shadow_pass_at_night() {
    let (device, mut r) = full_renderer(RendererOptions::default());
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let instances = [at(block, Vec3::ZERO)];
    let day = test_scene(&instances);
    render_frames(&device, &mut r, FORMAT, &day, &[(64, 48)]);
    assert_eq!(r.stats().cascades_drawn, 3);
    assert_eq!(r.frame_uniforms(&day).0.shadow_params[1], 1.0);
    let mut night = day;
    night.sun.direction = [0.3, -0.2, 0.5];
    for _ in 0..2 {
        render_frames(&device, &mut r, FORMAT, &night, &[(64, 48)]);
        let s = r.stats();
        assert_eq!((s.shadow_draw_calls, s.cascades_drawn), (0, 0));
        assert!(s.draw_calls > 0);
    }
    assert_eq!(r.frame_uniforms(&night).0.shadow_params[1], 0.0);
    // Dawn on the same caster and camera: nothing kept from before the night.
    render_frames(&device, &mut r, FORMAT, &day, &[(64, 48)]);
    assert_eq!(r.stats().cascades_drawn, 3);
    assert!(r.stats().shadow_draw_calls > 0);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// A still camera keeps every cascade while its casters stay put and the sun moves too
/// little to shift a shadow by half a texel; a moved caster or a sun that moves more
/// draws them again.
#[test]
fn still_cascades_are_kept_until_a_caster_or_the_sun_moves() {
    let (device, mut r) = full_renderer(RendererOptions::default());
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let instances = [at(block, Vec3::ZERO)];
    let scene = test_scene(&instances);
    let drawn = |r: &mut Renderer, scene: &SceneView| {
        render_frames(&device, r, FORMAT, scene, &[(64, 48)]);
        r.stats().cascades_drawn
    };
    assert_eq!(drawn(&mut r, &scene), 3);
    assert_eq!(drawn(&mut r, &scene), 0);
    // A twentieth of a degree: the near cascade's texel (~4 cm) is the first it moves a
    // tall shadow across by half.
    let mut sunset = scene;
    let sun = Vec3::from(scene.sun.direction);
    sunset.sun.direction = (glam::Quat::from_rotation_y(0.0009) * sun).to_array();
    assert!(drawn(&mut r, &sunset) >= 1);
    assert_eq!(drawn(&mut r, &sunset), 0);
    let moved = [at(block, Vec3::new(0.5, 0.0, 0.0))];
    let scene = SceneView {
        instances: &moved,
        ..sunset
    };
    assert_eq!(drawn(&mut r, &scene), 3);
    assert_eq!(drawn(&mut r, &scene), 0);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// A chunk slot freed and written again with other geometry keeps its part ids (and
/// chunks draw untransformed), so the pool's writes go into the cascades' hashes: the
/// kept maps are drawn again.
#[test]
fn a_chunk_written_again_draws_its_cascades_again() {
    let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
    let options = RendererOptions {
        chunk_pool: ChunkPoolConfig {
            slots: 1,
            pages: 1,
            page_vertices: 64,
            page_indices: 96,
        },
        ..Default::default()
    };
    let mut r = Renderer::with_options(&device, &queue, FORMAT, (64, 48), options);
    let slot = r.alloc_chunk_slot().unwrap();
    r.write_chunk(
        slot,
        &Mesh::cuboid([0.0; 3], [1.0; 3], [200; 4]),
        &[[0, 36]],
    )
    .unwrap();
    let instances = [MeshInstance::new(
        MeshId::chunk_part(slot, 0),
        Mat4::IDENTITY.to_cols_array_2d(),
    )];
    let scene = test_scene(&instances);
    let drawn = |r: &mut Renderer| {
        render_frames(&device, r, FORMAT, &scene, &[(64, 48)]);
        r.stats().cascades_drawn
    };
    assert_eq!(drawn(&mut r), 3);
    assert_eq!(drawn(&mut r), 0);
    // A taller building where the first stood, in the same slot.
    r.free_chunk_slot(slot).unwrap();
    assert_eq!(r.alloc_chunk_slot(), Ok(slot));
    r.write_chunk(
        slot,
        &Mesh::cuboid([0.0; 3], [1.0, 5.0, 1.0], [90; 4]),
        &[[0, 36]],
    )
    .unwrap();
    assert_eq!(drawn(&mut r), 3);
    assert_eq!(drawn(&mut r), 0);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// Not a check: the CPU cost of culling and packing 16k instances (a 128×128 grid 4 m
/// apart round the camera) for the forward pass and three cascades. Run with
/// `cargo test --release -p render -- --ignored --nocapture cull_cost`.
#[test]
#[ignore = "benchmark; prints numbers"]
fn cull_cost() {
    let (device, mut r) = full_renderer(RendererOptions::default());
    let block = r.add_mesh(&Mesh::unit_block([255; 4]));
    let instances: Vec<MeshInstance> = (0..128 * 128)
        .map(|i| {
            at(
                block,
                Vec3::new(
                    (i % 128) as f32 * 4.0 - 256.0,
                    0.0,
                    (i / 128) as f32 * 4.0 - 256.0,
                ),
            )
        })
        .collect();
    r.reserve_instances(instances.len());
    let scene = test_scene(&instances);
    render_frames(&device, &mut r, FORMAT, &scene, &[(64, 48)]);
    let view = Mat4::from_cols_array_2d(&scene.camera.view);
    let proj = Mat4::from_cols_array_2d(&scene.camera.proj);
    let sky = SkyModel::new(&scene.sun, &scene.sky);
    let fits = r.fit_shadows(view, proj, sky.sun_dir);
    let runs = 50;
    let t = std::time::Instant::now();
    let mut kept = 0;
    for _ in 0..runs {
        let (ranges, _) = r.cull_and_pack(&scene, 0, proj * view, &fits, [true; CASCADES]);
        kept = ranges.iter().map(|x| x.len()).sum::<usize>();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    eprintln!(
        "cull_and_pack: {} instances, {kept} kept over 4 passes: {ms:.3} ms ({:.1} ns each)",
        instances.len(),
        ms * 1e6 / instances.len() as f64
    );
    r.set_culling(false);
    let t = std::time::Instant::now();
    for _ in 0..runs {
        r.cull_and_pack(&scene, 0, proj * view, &fits, [true; CASCADES]);
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    eprintln!("  the same packing everything into every pass (no culling): {ms:.3} ms");
}

/// CPU port of `agx` in common.wgsl, step for step (matrices are column-major there).
fn agx_cpu(color: Vec3) -> (Vec3, Vec3) {
    let inset = glam::Mat3::from_cols(
        Vec3::new(0.842_479_06, 0.042_328_242, 0.042_375_655),
        Vec3::new(0.078_433_6, 0.878_468_6, 0.078_433_6),
        Vec3::new(0.079_223_745, 0.079_166_13, 0.879_142_96),
    );
    let outset = glam::Mat3::from_cols(
        Vec3::new(1.196_879, -0.052_896_85, -0.052_971_636),
        Vec3::new(-0.098_020_88, 1.151_903_1, -0.098_043_45),
        Vec3::new(-0.099_029_74, -0.098_961_18, 1.151_073_7),
    );
    let (min_ev, max_ev) = (-12.47393_f32, 4.026069_f32);
    let mut v = inset * color.max(Vec3::splat(1e-10));
    v = (Vec3::new(v.x.log2(), v.y.log2(), v.z.log2())
        .clamp(Vec3::splat(min_ev), Vec3::splat(max_ev))
        - min_ev)
        / (max_ev - min_ev);
    let x2 = v * v;
    let x4 = x2 * x2;
    v = 15.5 * x4 * x2 - 40.14 * x4 * v + 31.96 * x4 - 6.868 * x2 * v + 0.4298 * x2 + 0.1191 * v
        - Vec3::splat(0.00232);
    let luma = v.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    v = v.max(Vec3::ZERO).powf(1.25);
    v = (luma + 1.2 * (v - luma)).max(Vec3::ZERO);
    let pre = outset * v;
    (
        pre,
        pre.max(Vec3::ZERO).powf(2.2).clamp(Vec3::ZERO, Vec3::ONE),
    )
}

/// Saturated inputs push the AgX outset below zero; the tonemap must clamp before pow()
/// (undefined for a negative base) so every output stays finite and in [0, 1].
#[test]
fn agx_never_pows_a_negative() {
    let src = include_str!("../shaders/common.wgsl");
    assert!(src.contains("let display = max(outset * v, vec3<f32>(0.0));"));
    assert!(src.contains("pow(display, vec3<f32>(2.2))"));
    let mut negative = 0;
    for hue in [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 1.0),
        Vec3::new(1.0, 0.0, 1.0),
        Vec3::new(1.0, 0.02, 0.0),
    ] {
        for ev in -12..=8 {
            let (pre, out) = agx_cpu(hue * 2f32.powi(ev));
            negative += usize::from(pre.min_element() < 0.0);
            assert!(out.is_finite(), "{hue} at {ev} EV -> {out}");
            assert!(
                out.min_element() >= 0.0 && out.max_element() <= 1.0,
                "{out}"
            );
        }
    }
    assert!(
        negative > 0,
        "no input reached a negative outset; the test proves nothing"
    );
}
