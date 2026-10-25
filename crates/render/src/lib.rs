//! Skywave's renderer.
//!
//! Three passes per frame, all within WebGL2 limits (uniform buffers only, no compute, no
//! storage buffers, no texture arrays):
//!
//! 1. **Shadow**: depth-only render of every instance from the sun into one 2048² map,
//!    with an orthographic projection fitted around the camera.
//! 2. **Forward**: a sky-gradient fill, then Lambert-lit, hard-shadowed meshes with distance
//!    haze into an `Rgba16Float` HDR target.
//! 3. **Tonemap**: exposure and an ACES fitted curve from the HDR target to the swapchain.
//!
//! The renderer knows nothing about the game; it draws a [`SceneView`] of plain data.

mod light;
mod mesh;

pub use light::sun_view_proj;
pub use mesh::{Mesh, Vertex};

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3, Vec4Swizzles};
use wgpu::util::DeviceExt;

/// Edge length of the sun shadow map in texels.
pub const SHADOW_MAP_SIZE: u32 = 2048;
/// Radius of the sphere around the camera that receives sun shadows, in meters.
pub const SHADOW_RADIUS: f32 = 160.0;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Handle of a mesh uploaded with [`Renderer::add_mesh`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MeshId(pub u32);

/// One drawn copy of a mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshInstance {
    pub mesh: MeshId,
    /// Model-to-world matrix, column-major.
    pub transform: [[f32; 4]; 4],
    /// Linear RGBA multiplier for the mesh's vertex colors.
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// World-to-view, column-major, right-handed (camera looks down -Z).
    pub view: [[f32; 4]; 4],
    /// View-to-clip with depth in `[0, 1]` (e.g. `glam::camera::rh::proj::directx::perspective`).
    pub proj: [[f32; 4]; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    /// Unit vector from the scene toward the sun.
    pub direction: [f32; 3],
    /// Linear HDR irradiance.
    pub color: [f32; 3],
}

/// Sky gradient colors (linear HDR). They also light the scene as hemisphere ambient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    /// Below the horizon, and the ambient bounce from below.
    pub ground: [f32; 3],
    /// Distance haze density per meter; 0 disables it.
    pub haze: f32,
}

/// Everything the renderer needs for one frame.
///
/// Consecutive instances with the same mesh are drawn with one instanced draw call, so
/// group instances by mesh for fewer draws.
#[derive(Clone, Copy, Debug)]
pub struct SceneView<'a> {
    pub camera: Camera,
    pub sun: Sun,
    pub sky: Sky,
    pub exposure: f32,
    pub instances: &'a [MeshInstance],
}

/// GPU layout of the per-frame uniform block; mirrors `Frame` in shaders/common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    light_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    sky_zenith: [f32; 4],
    sky_horizon: [f32; 4],
    sky_ground: [f32; 4],
    params: [f32; 4],
}

/// GPU layout of one instance (vertex buffer, step mode instance).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    /// Inverse-transpose of the model's upper 3x3, columns padded to vec4.
    normal: [[f32; 4]; 3],
    color: [f32; 4],
}

impl InstanceRaw {
    fn new(inst: &MeshInstance) -> InstanceRaw {
        let model = Mat4::from_cols_array_2d(&inst.transform);
        let n = Mat3::from_mat4(model).inverse().transpose();
        InstanceRaw {
            model: inst.transform,
            normal: [
                n.x_axis.extend(0.0).to_array(),
                n.y_axis.extend(0.0).to_array(),
                n.z_axis.extend(0.0).to_array(),
            ],
            color: inst.color,
        }
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4];
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
    3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
    7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4,
];

fn mesh_buffer_layouts() -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
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

/// Shader sources, each prefixed with the shared declarations.
pub mod shaders {
    pub const SHADOW: &str = concat!(
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/shadow.wgsl")
    );
    pub const FORWARD: &str = concat!(
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/forward.wgsl")
    );
    pub const TONEMAP: &str = concat!(
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/tonemap.wgsl")
    );
    pub const ALL: [(&str, &str); 3] = [
        ("shadow", SHADOW),
        ("forward", FORWARD),
        ("tonemap", TONEMAP),
    ];
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// Size-dependent render targets.
struct Targets {
    hdr: wgpu::TextureView,
    depth: wgpu::TextureView,
    tonemap_group: wgpu::BindGroup,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_format: wgpu::TextureFormat,
    size: (u32, u32),

    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    shadow_view: wgpu::TextureView,
    shadow_group: wgpu::BindGroup,
    tonemap_layout: wgpu::BindGroupLayout,
    targets: Targets,

    shadow_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    tonemap_pipeline: wgpu::RenderPipeline,

    meshes: Vec<GpuMesh>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    instance_staging: Vec<InstanceRaw>,
}

impl Renderer {
    /// Creates all pipelines and targets. `size` is the swapchain size in pixels.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> Renderer {
        let device = device.clone();
        let queue = queue.clone();
        let size = (size.0.max(1), size.1.max(1));

        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniforms"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<FrameUniform>() as u64),
                },
                count: None,
            }],
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });

        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sun shadow map"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_SIZE,
                height: SHADOW_MAP_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow compare"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let shadow_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow"),
            layout: &shadow_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
            ],
        });
        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let module = |label: &str, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let shadow_module = module("shadow", shaders::SHADOW);
        let forward_module = module("forward", shaders::FORWARD);
        let tonemap_module = module("tonemap", shaders::TONEMAP);

        let layout = |label: &str, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let shadow_pl = layout("shadow", &[Some(&frame_layout)]);
        let forward_pl = layout("forward", &[Some(&frame_layout), Some(&shadow_layout)]);
        let tonemap_pl = layout("tonemap", &[Some(&frame_layout), Some(&tonemap_layout)]);

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
        let hdr_target = [Some(wgpu::ColorTargetState::from(HDR_FORMAT))];

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
                wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
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
            depth_stencil: depth(
                DEPTH_FORMAT,
                false,
                wgpu::CompareFunction::Always,
                Default::default(),
            ),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &forward_module,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &hdr_target,
            }),
            multiview_mask: None,
            cache: None,
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
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
                true,
                wgpu::CompareFunction::Less,
                Default::default(),
            ),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &forward_module,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &hdr_target,
            }),
            multiview_mask: None,
            cache: None,
        });
        let tonemap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tonemap"),
            layout: Some(&tonemap_pl),
            vertex: wgpu::VertexState {
                module: &tonemap_module,
                entry_point: Some("vs_tonemap"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &tonemap_module,
                entry_point: Some("fs_tonemap"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState::from(surface_format))],
            }),
            multiview_mask: None,
            cache: None,
        });

        let instance_capacity = 256;
        let instance_buffer = create_instance_buffer(&device, instance_capacity);
        let targets = create_targets(&device, &tonemap_layout, size);

        Renderer {
            device,
            queue,
            surface_format,
            size,
            frame_buffer,
            frame_group,
            shadow_view,
            shadow_group,
            tonemap_layout,
            targets,
            shadow_pipeline,
            sky_pipeline,
            mesh_pipeline,
            tonemap_pipeline,
            meshes: Vec::new(),
            instance_buffer,
            instance_capacity,
            instance_staging: Vec::with_capacity(instance_capacity),
        }
    }

    /// Recreates size-dependent targets. Call when the swapchain is reconfigured.
    pub fn resize(&mut self, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if size != self.size {
            self.size = size;
            self.targets = create_targets(&self.device, &self.tonemap_layout, size);
        }
    }

    /// Uploads a mesh. Ids are assigned in upload order starting at 0.
    pub fn add_mesh(&mut self, mesh: &Mesh) -> MeshId {
        let vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let indices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        self.meshes.push(GpuMesh {
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
        });
        MeshId(self.meshes.len() as u32 - 1)
    }

    /// Renders `scene` into `view`, which must have the surface format given to `new` and
    /// the size given to `new`/`resize`.
    pub fn render(&mut self, view: &wgpu::TextureView, scene: &SceneView) {
        let uniform = self.frame_uniform(scene);
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));

        self.instance_staging.clear();
        self.instance_staging
            .extend(scene.instances.iter().map(InstanceRaw::new));
        if self.instance_staging.len() > self.instance_capacity {
            self.instance_capacity = self.instance_staging.len().next_power_of_two();
            self.instance_buffer = create_instance_buffer(&self.device, self.instance_capacity);
        }
        if !self.instance_staging.is_empty() {
            self.queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.instance_staging),
            );
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(0, &self.frame_group, &[]);
            self.draw_instances(&mut pass, scene.instances);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forward"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.hdr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &self.shadow_group, &[]);
            pass.set_pipeline(&self.sky_pipeline);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.mesh_pipeline);
            self.draw_instances(&mut pass, scene.instances);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.tonemap_pipeline);
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &self.targets.tonemap_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Issues one instanced draw per run of consecutive instances sharing a mesh.
    fn draw_instances(&self, pass: &mut wgpu::RenderPass<'_>, instances: &[MeshInstance]) {
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        let mut start = 0;
        while start < instances.len() {
            let id = instances[start].mesh;
            let mut end = start + 1;
            while end < instances.len() && instances[end].mesh == id {
                end += 1;
            }
            if let Some(mesh) = self.meshes.get(id.0 as usize) {
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, start as u32..end as u32);
            }
            start = end;
        }
    }

    fn frame_uniform(&self, scene: &SceneView) -> FrameUniform {
        let view = Mat4::from_cols_array_2d(&scene.camera.view);
        let proj = Mat4::from_cols_array_2d(&scene.camera.proj);
        let view_proj = proj * view;
        let inv_view = view.inverse();
        let eye = inv_view.w_axis.xyz();
        let forward = -inv_view.z_axis.xyz().normalize_or(Vec3::NEG_Z);
        // Shadow the region in front of the camera, where most visible pixels are.
        let center = eye + forward * (SHADOW_RADIUS * 0.6);
        let to_sun = Vec3::from(scene.sun.direction).normalize_or(Vec3::Y);
        let light = sun_view_proj(to_sun, center, SHADOW_RADIUS, 100.0, SHADOW_MAP_SIZE);
        let texel_m = 2.0 * SHADOW_RADIUS / SHADOW_MAP_SIZE as f32;
        let rgb = |c: [f32; 3]| [c[0], c[1], c[2], 0.0];
        FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            light_view_proj: light.to_cols_array_2d(),
            camera_pos: eye.extend(scene.sky.haze).to_array(),
            sun_dir: to_sun.extend(0.0).to_array(),
            sun_color: rgb(scene.sun.color),
            sky_zenith: rgb(scene.sky.zenith),
            sky_horizon: rgb(scene.sky.horizon),
            sky_ground: rgb(scene.sky.ground),
            params: [
                scene.exposure,
                if self.surface_format.is_srgb() {
                    0.0
                } else {
                    1.0
                },
                0.0005,
                texel_m * 1.5,
            ],
        }
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: (capacity * size_of::<InstanceRaw>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_targets(
    device: &wgpu::Device,
    tonemap_layout: &wgpu::BindGroupLayout,
    size: (u32, u32),
) -> Targets {
    let target = |label, format, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
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
    let hdr = target(
        "hdr color",
        HDR_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let depth = target(
        "depth",
        DEPTH_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let tonemap_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tonemap"),
        layout: tonemap_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&hdr),
        }],
    });
    Targets {
        hdr,
        depth,
        tonemap_group,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec4;

    /// Every entry point must parse, validate and translate to GLSL ES 3.00 (WebGL2).
    #[test]
    fn shaders_translate_to_webgl2_glsl() {
        use naga::back::glsl;
        for (name, source) in shaders::ALL {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            assert!(!module.entry_points.is_empty());
            for ep in &module.entry_points {
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
    }

    #[test]
    fn uniform_and_instance_layouts() {
        // std140-compatible: only mat4 and vec4 members, 16-byte multiple, under WebGL2's
        // 16 KiB uniform block limit.
        assert_eq!(size_of::<FrameUniform>(), 3 * 64 + 7 * 16);
        assert_eq!(size_of::<FrameUniform>() % 16, 0);
        assert_eq!(size_of::<InstanceRaw>(), 128);
        assert_eq!(size_of::<Vertex>(), 28);
        let attributes = VERTEX_ATTRIBUTES.len() + INSTANCE_ATTRIBUTES.len();
        assert!(
            attributes <= wgpu::Limits::downlevel_webgl2_defaults().max_vertex_attributes as usize
        );
    }

    #[test]
    fn normal_matrix_handles_non_uniform_scale() {
        let m = Mat4::from_scale(Vec3::new(4.0, 1.0, 1.0)) * Mat4::from_rotation_z(0.5);
        let raw = InstanceRaw::new(&MeshInstance {
            mesh: MeshId(0),
            transform: m.to_cols_array_2d(),
            color: [1.0; 4],
        });
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

    /// Builds the whole renderer on wgpu's no-op backend with WebGL2 limits and records a
    /// frame, so wgpu's validation checks every pipeline, binding and pass without a GPU.
    #[test]
    fn renders_a_frame_on_noop_device_with_webgl2_limits() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::NOOP,
            backend_options: wgpu::BackendOptions {
                noop: wgpu::NoopBackendOptions {
                    enable: true,
                    limits: Some(wgpu::Limits::downlevel_webgl2_defaults()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("noop adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
            ..Default::default()
        }))
        .expect("noop device");

        for format in [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
        ] {
            let mut r = Renderer::new(&device, &queue, format, (64, 48));
            let block = r.add_mesh(&Mesh::unit_block([255; 4]));
            let ground = r.add_mesh(&Mesh::ground_quad(10.0, 10.0, [90, 120, 60, 255]));
            assert_eq!((block, ground), (MeshId(0), MeshId(1)));

            // More instances than the initial capacity forces a buffer reallocation.
            let mut instances = vec![MeshInstance {
                mesh: ground,
                transform: Mat4::IDENTITY.to_cols_array_2d(),
                color: [1.0; 4],
            }];
            for i in 0..300 {
                instances.push(MeshInstance {
                    mesh: block,
                    transform: Mat4::from_translation(Vec3::new(i as f32, 0.0, 0.0))
                        .to_cols_array_2d(),
                    color: [0.8, 0.7, 0.6, 1.0],
                });
            }
            let scene = SceneView {
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
                    color: [3.0, 2.8, 2.5],
                },
                sky: Sky {
                    zenith: [0.2, 0.4, 0.9],
                    horizon: [0.7, 0.8, 0.9],
                    ground: [0.3, 0.3, 0.3],
                    haze: 0.001,
                },
                exposure: 1.0,
                instances: &instances,
            };
            for size in [(64, 48), (32, 32)] {
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
                    view_formats: &[],
                });
                r.render(&target.create_view(&Default::default()), &scene);
            }
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
    }
}
