//! Renderer construction: pipelines, targets and the GPU resources they bind.

use super::*;

impl Renderer {
    /// Creates all pipelines and targets with default options. `size` is the swapchain
    /// size in pixels.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> Renderer {
        Renderer::with_options(
            device,
            queue,
            surface_format,
            size,
            RendererOptions::default(),
        )
    }

    /// Creates all pipelines and targets. `size` is the swapchain size in pixels.
    pub fn with_options(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        size: (u32, u32),
        options: RendererOptions,
    ) -> Renderer {
        let device = device.clone();
        let queue = queue.clone();
        let size = (size.0.max(1), size.1.max(1));
        let hdr = match options.hdr {
            HdrMode::Auto => float16_renderable(&device),
            HdrMode::Float16 => true,
            HdrMode::Ldr => false,
        };
        let color_format = if hdr { HDR_FORMAT } else { surface_format };
        // Three cascades side by side must fit the device's widest texture.
        let max_cascade_size = device.limits().max_texture_dimension_2d / 3;
        let quality = options.quality.sanitized(max_cascade_size);
        let cascade_size = quality.shadow_size;

        let uniform_entry = |binding, visibility, min_size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(min_size as u64),
            },
            count: None,
        };
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniforms"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
                size_of::<FrameUniform>(),
            )],
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });

        let cascade_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cascade uniforms"),
            size: CASCADE_UNIFORM_STRIDE * CASCADES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cascade_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cascade"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX, 64)],
        });
        let cascade_groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cascade"),
                layout: &cascade_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &cascade_buffer,
                        offset: i as u64 * CASCADE_UNIFORM_STRIDE,
                        size: wgpu::BufferSize::new(64),
                    }),
                }],
            })
        });

        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow compare"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let atlas_view = upload_atlas(&device, &queue, &TextureAtlas::generate());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            lod_max_clamp: (ATLAS_MIP_LEVELS - 1) as f32,
            // Keeps grazing surfaces readable; wgpu drops it where unsupported.
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let texture_entry = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(ty),
            count: None,
        };
        let forward_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forward"),
            entries: &[
                texture_entry(0, wgpu::TextureSampleType::Depth),
                sampler_entry(1, wgpu::SamplerBindingType::Comparison),
                texture_entry(2, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(3, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let (shadow_view, forward_group) = create_shadow_atlas(
            &device,
            cascade_size,
            &forward_layout,
            &shadow_sampler,
            &atlas_view,
            &atlas_sampler,
        );
        let module = |label: &str, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let shadow_module = module("shadow", shaders::SHADOW);
        let forward_module = module("forward", shaders::FORWARD);

        let layout = |label: &str, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let shadow_pl = layout("shadow", &[Some(&frame_layout), Some(&cascade_layout)]);
        let forward_pl = layout("forward", &[Some(&frame_layout), Some(&forward_layout)]);

        let buffers = mesh_buffer_layouts();
        let back_faces = wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        };
        let depth = |format, write, compare, bias| {
            Some(wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: wgpu::StencilState::default(),
                bias,
            })
        };
        let color_target = [Some(wgpu::ColorTargetState::from(color_format))];
        let (fs_sky, fs_mesh) = if hdr {
            ("fs_sky", "fs_mesh")
        } else {
            ("fs_sky_ldr", "fs_mesh_ldr")
        };

        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&shadow_pl),
            vertex: wgpu::VertexState {
                module: &shadow_module,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: back_faces,
            depth_stencil: depth(
                SHADOW_FORMAT,
                true,
                wgpu::CompareFunction::LessEqual,
                // Slope-scaled only: the receiver offsets in the shader handle flat bias.
                wgpu::DepthBiasState {
                    constant: 0,
                    slope_scale: 1.5,
                    clamp: 0.0,
                },
            ),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let shadow_clear_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("shadow clear"),
                layout: Some(&shadow_pl),
                vertex: wgpu::VertexState {
                    module: &shadow_module,
                    entry_point: Some("vs_shadow_clear"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: depth(
                    SHADOW_FORMAT,
                    true,
                    wgpu::CompareFunction::Always,
                    Default::default(),
                ),
                multisample: Default::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            });
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&forward_pl),
            vertex: wgpu::VertexState {
                module: &forward_module,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            // Drawn after the meshes at the far plane, so it shades only what they left.
            depth_stencil: depth(
                DEPTH_FORMAT,
                false,
                wgpu::CompareFunction::LessEqual,
                Default::default(),
            ),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &forward_module,
                entry_point: Some(fs_sky),
                compilation_options: Default::default(),
                targets: &color_target,
            }),
            multiview_mask: None,
            cache: None,
        });
        let mesh = |label, compare| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&forward_pl),
                vertex: wgpu::VertexState {
                    module: &forward_module,
                    entry_point: Some("vs_mesh"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: back_faces,
                depth_stencil: depth(DEPTH_FORMAT, true, compare, Default::default()),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &forward_module,
                    entry_point: Some(fs_mesh),
                    compilation_options: Default::default(),
                    targets: &color_target,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let mesh_pipeline = mesh("mesh", wgpu::CompareFunction::Less);
        let fs_glass = if hdr { "fs_glass" } else { "fs_glass_ldr" };
        let glass_target = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::COLOR,
        })];
        let glass_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glass"),
            layout: Some(&forward_pl),
            vertex: wgpu::VertexState {
                module: &forward_module,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: back_faces,
            depth_stencil: depth(
                DEPTH_FORMAT,
                false,
                wgpu::CompareFunction::LessEqual,
                Default::default(),
            ),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &forward_module,
                entry_point: Some(fs_glass),
                compilation_options: Default::default(),
                targets: &glass_target,
            }),
            multiview_mask: None,
            cache: None,
        });
        let mesh_after_depth_pipeline = mesh("mesh after depth", wgpu::CompareFunction::LessEqual);
        let depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth pre-pass"),
            layout: Some(&forward_pl),
            vertex: wgpu::VertexState {
                module: &forward_module,
                entry_point: Some("vs_depth"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: back_faces,
            depth_stencil: depth(
                DEPTH_FORMAT,
                true,
                wgpu::CompareFunction::Less,
                Default::default(),
            ),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &forward_module,
                entry_point: Some("fs_depth"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let post = hdr.then(|| {
            let filtered = wgpu::TextureSampleType::Float { filterable: true };
            let bloom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bloom"),
                entries: &[
                    texture_entry(0, filtered),
                    sampler_entry(1, wgpu::SamplerBindingType::Filtering),
                ],
            });
            let tonemap_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("tonemap"),
                    entries: &[
                        texture_entry(0, filtered),
                        sampler_entry(1, wgpu::SamplerBindingType::Filtering),
                        // Filterable: a scaled target is upscaled with the bloom sampler.
                        texture_entry(2, filtered),
                    ],
                });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("bloom"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let post_module = module("post", shaders::POST);
            let bloom_pl = layout("bloom", &[Some(&frame_layout), Some(&bloom_layout)]);
            let tonemap_pl = layout("tonemap", &[Some(&frame_layout), Some(&tonemap_layout)]);
            let fullscreen = |label, pl: &wgpu::PipelineLayout, vs, fs, target| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(pl),
                    vertex: wgpu::VertexState {
                        module: &post_module,
                        entry_point: Some(vs),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &post_module,
                        entry_point: Some(fs),
                        compilation_options: Default::default(),
                        targets: &[Some(target)],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
            let bloom_target = wgpu::ColorTargetState::from(HDR_FORMAT);
            let add = wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            };
            let additive = wgpu::ColorTargetState {
                blend: Some(wgpu::BlendState {
                    color: add,
                    alpha: wgpu::BlendComponent::REPLACE,
                }),
                ..bloom_target.clone()
            };
            Post {
                prefilter: fullscreen(
                    "bloom prefilter",
                    &bloom_pl,
                    "vs_bloom",
                    "fs_bloom_prefilter",
                    bloom_target.clone(),
                ),
                down: fullscreen(
                    "bloom down",
                    &bloom_pl,
                    "vs_bloom",
                    "fs_bloom_down",
                    bloom_target,
                ),
                up: fullscreen("bloom up", &bloom_pl, "vs_bloom", "fs_bloom_up", additive),
                tonemap: fullscreen(
                    "tonemap",
                    &tonemap_pl,
                    "vs_tonemap",
                    "fs_tonemap",
                    wgpu::ColorTargetState::from(surface_format),
                ),
                bloom_layout,
                tonemap_layout,
                sampler,
            }
        });

        let font_view = upload_font(&device, &queue, &font::FontAtlas::generate());
        let font_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("font"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // Trilinear, at the mip level the quad asks for (see `hud::build_quads`).
        let map_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hud map"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let hud_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud"),
            entries: &[
                texture_entry(0, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(1, wgpu::SamplerBindingType::Filtering),
                texture_entry(2, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(3, wgpu::SamplerBindingType::Filtering),
                texture_entry(4, wgpu::TextureSampleType::Float { filterable: true }),
            ],
        });
        let no_map = upload_map_texture(&device, &queue, 1, 1, &[0; 4]);
        let no_stills = upload_map_texture(&device, &queue, 1, 1, &[0; 4]);
        let hud_group = hud_group(
            &device,
            &hud_layout,
            [&font_view, &no_map, &no_stills],
            [&font_sampler, &map_sampler],
        );
        let hud_module = module("hud", shaders::HUD);
        let hud_pl = layout("hud", &[Some(&frame_layout), Some(&hud_layout)]);
        let hud_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud"),
            layout: Some(&hud_pl),
            vertex: wgpu::VertexState {
                module: &hud_module,
                entry_point: Some("vs_hud"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<HudQuad>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &HUD_ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &hud_module,
                entry_point: Some("fs_hud"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let hud_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud quads"),
            size: (MAX_HUD_QUADS * size_of::<HudQuad>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Coronas read the scene's depth in the vertex stage and add onto the lit image. On
        // GL the depth is a plain float texture read through a nearest sampler without
        // comparison (GL counts a float depth texture sampled any other way as incomplete
        // and reads zeros); elsewhere a depth texture read with `textureLoad`, which leaves
        // the sampler binding unused.
        let corona_load = match options.corona_depth {
            CoronaDepth::Auto => device.adapter_info().backend != wgpu::Backend::Gl,
            CoronaDepth::Sampled => false,
            CoronaDepth::Loaded => true,
        };
        let corona_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("corona"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: if corona_load {
                            wgpu::TextureSampleType::Depth
                        } else {
                            wgpu::TextureSampleType::Float { filterable: false }
                        },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let depth_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene depth"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // An optional pass: built in its own error scope, so a device that rejects it loses
        // the coronas rather than the frame (see `ProbeState`).
        let corona_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let corona_module = if corona_load {
            module("corona", shaders::CORONA_LOAD)
        } else {
            module("corona", shaders::CORONA)
        };
        let corona_pl = layout("corona", &[Some(&frame_layout), Some(&corona_layout)]);
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let keep = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let corona_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("corona"),
            layout: Some(&corona_pl),
            vertex: wgpu::VertexState {
                module: &corona_module,
                entry_point: Some("vs_corona"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<CoronaRaw>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &CORONA_ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &corona_module,
                entry_point: Some(if hdr { "fs_corona" } else { "fs_corona_ldr" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState {
                        color: add,
                        alpha: keep,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let corona_probe = ProbeState::new(corona_scope);
        let backend = backend_label(device.adapter_info().backend);
        let corona_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("coronas"),
            size: (MAX_CORONAS * size_of::<CoronaRaw>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let instance_capacity = 256;
        let instance_buffer = create_instance_buffer(&device, instance_capacity);
        let pool_buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                // Never empty, so a renderer without a pool still binds valid buffers.
                size: size.max(wgpu::COPY_BUFFER_ALIGNMENT),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let chunk_vertices = pool_buffer(
            "chunk vertices",
            options.chunk_pool.vertex_bytes(),
            wgpu::BufferUsages::VERTEX,
        );
        let chunk_indices = pool_buffer(
            "chunk indices",
            options.chunk_pool.index_bytes(),
            wgpu::BufferUsages::INDEX,
        );
        let render_size = if hdr { quality.render_size(size) } else { size };
        let targets = create_targets(
            &device,
            post.as_ref(),
            &corona_layout,
            &depth_sampler,
            render_size,
        );

        Renderer {
            device,
            queue,
            surface_format,
            size,
            hdr,
            cascade_splits: options.cascade_splits,
            cascade_size,
            max_cascade_size,
            quality,
            culling: options.culling,
            shadow_valid: false,
            shadows_held: false,
            kept: [None; CASCADES],
            frame_buffer,
            frame_group,
            cascade_buffer,
            cascade_groups,
            shadow_view,
            forward_group,
            forward_layout,
            shadow_sampler,
            atlas_view,
            atlas_sampler,
            post,
            targets,
            shadow_pipeline,
            shadow_clear_pipeline,
            sky_pipeline,
            mesh_pipeline,
            glass_pipeline,
            depth_pipeline,
            mesh_after_depth_pipeline,
            hud_pipeline,
            hud_layout,
            hud_group,
            font_view,
            font_sampler,
            map_sampler,
            map_view: no_map,
            stills_view: no_stills,
            map_size: [1, 1],
            stills_size: [1, 1],
            hud_buffer,
            hud_staging: Vec::with_capacity(MAX_HUD_QUADS),
            hud_drawn: Vec::with_capacity(MAX_HUD_QUADS),
            hud_valid: false,
            kept_world: KeptWorld {
                head: None,
                instances: Vec::with_capacity(instance_capacity),
                lights: Vec::with_capacity(MAX_LIGHTS),
            },
            corona_pipeline,
            corona_probe,
            corona_depth: if corona_load {
                CoronaDepth::Loaded
            } else {
                CoronaDepth::Sampled
            },
            corona_layout,
            depth_sampler,
            corona_buffer,
            corona_staging: Vec::with_capacity(MAX_CORONAS),
            meshes: Vec::new(),
            chunk_pool: ChunkPool::new(options.chunk_pool),
            chunk_vertices,
            chunk_indices,
            instance_buffer,
            instance_capacity,
            instance_staging: Vec::with_capacity(PASSES * instance_capacity),
            vis: Vec::with_capacity(instance_capacity),
            raws: Vec::with_capacity(instance_capacity),
            order: Vec::with_capacity(PASSES * instance_capacity),
            timer: None,
            stats: RenderStats {
                backend,
                ..RenderStats::default()
            },
        }
    }
}

pub(crate) fn mesh_buffer_layouts() -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
    [
        Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX_ATTRIBUTES,
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<InstanceRaw>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE_ATTRIBUTES,
        }),
    ]
}

/// Whether the device can render to `Rgba16Float` (on WebGL2 that needs
/// `EXT_color_buffer_float`). Probes by creating a tiny target inside an error scope; the
/// GL and native backends resolve the scope immediately. The browser's WebGPU resolves it
/// later, and always supports the format, so a pending answer counts as yes.
pub(crate) fn float16_renderable(device: &wgpu::Device) -> bool {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let probe = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hdr probe"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    drop(probe);
    let mut pop = std::pin::pin!(scope.pop());
    match pop.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(error) => error.is_none(),
        Poll::Pending => true,
    }
}

pub(crate) fn upload_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &TextureAtlas,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pattern atlas"),
        size: wgpu::Extent3d {
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: ATLAS_MIP_LEVELS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ATLAS_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for level in 0..ATLAS_MIP_LEVELS {
        let size = ATLAS_SIZE >> level;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            atlas.level(level),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Uploads the HUD font as a single-channel texture (no mips: it is only ever drawn at
/// whole multiples of its size).
pub(crate) fn upload_font(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &font::FontAtlas,
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: font::ATLAS_WIDTH,
        height: font::ATLAS_HEIGHT,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud font"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FONT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &atlas.pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(font::ATLAS_WIDTH),
            rows_per_image: Some(font::ATLAS_HEIGHT),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The HUD bind group: font atlas and sampler, map and sampler, stills (the map's sampler).
pub(crate) fn hud_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    views: [&wgpu::TextureView; 3],
    samplers: [&wgpu::Sampler; 2],
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hud"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(samplers[0]),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(samplers[1]),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(views[2]),
            },
        ],
    })
}

/// Uploads a map image with its whole mip chain (see [`hud::mip_chain`]).
pub(crate) fn upload_map_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::TextureView {
    let levels = hud::mip_chain(width, height, rgba);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud map"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32 + 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: MAP_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let base = [(width, height, rgba)];
    let rest = levels.iter().map(|(w, h, px)| (*w, *h, px.as_slice()));
    for (level, (w, h, px)) in base.into_iter().chain(rest).enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            px,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The shadow atlas (three `size`² cascades side by side) and the forward bind group that
/// samples it.
pub(crate) fn create_shadow_atlas(
    device: &wgpu::Device,
    size: u32,
    layout: &wgpu::BindGroupLayout,
    shadow_sampler: &wgpu::Sampler,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
) -> (wgpu::TextureView, wgpu::BindGroup) {
    let shadow_view = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("sun shadow cascades"),
            size: wgpu::Extent3d {
                width: size * CASCADES as u32,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("forward"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&shadow_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(atlas_sampler),
            },
        ],
    });
    (shadow_view, group)
}

pub(crate) fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: (capacity * size_of::<InstanceRaw>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Size of bloom level `k` for a `size` target: half, quarter, … of it, at least 1 px.
pub(crate) fn bloom_level_size(size: (u32, u32), k: usize) -> (u32, u32) {
    ((size.0 >> (k + 1)).max(1), (size.1 >> (k + 1)).max(1))
}

pub(crate) fn create_targets(
    device: &wgpu::Device,
    post: Option<&Post>,
    corona_layout: &wgpu::BindGroupLayout,
    depth_sampler: &wgpu::Sampler,
    size: (u32, u32),
) -> Targets {
    let target = |label, format, usage, (width, height): (u32, u32)| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    };
    let depth = target(
        "depth",
        DEPTH_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        size,
    );
    let corona_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("corona"),
        layout: corona_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&depth),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(depth_sampler),
            },
        ],
    });
    let Some(post) = post else {
        return Targets {
            size,
            hdr: None,
            depth,
            corona_group,
            post: None,
        };
    };
    let sampled = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    let hdr = target("hdr color", HDR_FORMAT, sampled, size);
    let bloom: [wgpu::TextureView; BLOOM_LEVELS] =
        std::array::from_fn(|k| target("bloom", HDR_FORMAT, sampled, bloom_level_size(size, k)));
    let sample_group = |view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bloom"),
            layout: &post.bloom_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&post.sampler),
                },
            ],
        })
    };
    let tonemap_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tonemap"),
        layout: &post.tonemap_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&bloom[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&post.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&hdr),
            },
        ],
    });
    let post = PostTargets {
        hdr_group: sample_group(&hdr),
        level_groups: std::array::from_fn(|k| sample_group(&bloom[k])),
        tonemap_group,
        bloom,
    };
    Targets {
        size,
        hdr: Some(hdr),
        depth,
        corona_group,
        post: Some(post),
    }
}
