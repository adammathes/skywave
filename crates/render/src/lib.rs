//! Skywave's renderer.
//!
//! Three passes per frame, all within WebGL2 limits (uniform buffers only, no compute, no
//! storage buffers, no texture arrays):
//!
//! 1. **Shadow**: depth-only renders of the shadow-casting instances from the sun into
//!    three cascades (0–40 m, 40–160 m, 160–600 m by default; two at low quality) side by
//!    side in one 3×1 depth atlas, each an orthographic projection fitted to a slice of the
//!    camera frustum and snapped to texels. A cascade is kept from an earlier frame while
//!    its map covers the view, its casters have not changed and the sun has hardly moved;
//!    the farthest also between its frames ([`RenderQuality::far_cascade_interval`]).
//! 2. **Forward**: optionally the meshes' depth first ([`RenderQuality::depth_prepass`]),
//!    then the meshes: vertex color × atlas texel, Lambert sun with 3×3 PCF
//!    cascaded shadows, hemisphere sky ambient, the point and spot lights of each
//!    instance's [`LightSet`] (up to [`LIGHTS_PER_DRAW`] of the frame's [`MAX_LIGHTS`]), the
//!    vertex emissive channel and aerial-perspective fog; then the analytic sky (Preetham,
//!    sun and moon discs, stars) at the far plane, where no mesh is; into an `Rgba16Float`
//!    HDR target at [`RenderQuality::resolution_scale`] of the swapchain.
//! 3. **Coronas**: an additive soft sprite over each light with a [`Corona`] and over the
//!    sun and moon discs, faded by how much of it the scene's depth hides (see
//!    `shaders/corona.wgsl`), in one instanced draw. Skipped when there are none.
//! 4. **Bloom**: a soft-threshold, Karis-averaged 13-tap downsample into a chain of up to
//!    five levels ([`RenderQuality::bloom_levels`]) starting at half the HDR target's size,
//!    `Rgba16Float`, then 9-tap tent upsamples added back up it
//!    (Jimenez 2014). Skipped when [`Sky::bloom_strength`] is 0.
//! 5. **Tonemap**: a bilinear upscale when the HDR target is scaled, exposure, bloom, AgX,
//!    a light vignette and an 8-bit dither from the HDR target to the swapchain.
//! 6. **HUD**: text, rectangles and images (the map uploaded once with
//!    [`Renderer::upload_hud_image`], discs, the player arrow) from [`SceneView::hud`] as
//!    alpha-blended quads straight onto the swapchain, in one instanced draw: the HUD bind
//!    group holds the font atlas and the map, and each quad picks one (see [`HudLayout`]
//!    for the layout rules and [`font`] for the bitmap face). Skipped when there is
//!    nothing to draw.
//!
//! Where the device cannot render to `Rgba16Float` (WebGL2 without
//! `EXT_color_buffer_float`) the forward pass tonemaps in its fragment shaders (AgX,
//! vignette and dither included) and writes the swapchain directly; there is no bloom and no
//! tonemap pass; coronas are added to the tonemapped image. The HUD pass is the same in both
//! modes.
//!
//! **Lights.** The frame's lights live in the per-frame uniform block, 32 bytes each
//! ([`MAX_LIGHTS`] × 32 = 8 KiB of the block's 16 KiB WebGL2 limit), and every instance
//! names the ones that reach it in its [`LightSet`], packed into its instance attributes;
//! the caller does the culling (it knows its chunks), the shader loops over the set.
//!
//! **Culling.** Every instance is tested on the CPU against the camera frustum for the
//! forward pass and against each cascade's box for the shadow pass (see [`cull`]), using
//! the bounds of its mesh (computed in [`Renderer::add_mesh`]) or chunk part (computed when
//! it is written); the survivors of each pass are packed into one instance buffer per
//! frame. Nothing is allocated per frame. [`RenderStats`] counts submitted and drawn.
//!
//! The renderer knows nothing about the game; it draws a [`SceneView`] of plain data.
//! Radiance is linear sRGB in kcd/m² (1 = 1000 cd/m²) throughout; see [`Sun`] and [`Sky`].

mod atlas;
pub mod cull;
pub mod font;
mod frame;
mod hud;
mod light;
mod mesh;
mod pipelines;
mod pool;
mod sky;
mod timing;

pub use atlas::{
    tile_uv, uv_unorm, TextureAtlas, Tile, ATLAS_MIP_LEVELS, ATLAS_SIZE, TILES_PER_ROW, TILE_INSET,
    TILE_SIZE,
};
pub use hud::{
    clamp_ui_scale, Hud, HudBuilder, HudImage, HudImageTarget, HudItem, HudLayout, HudMask,
    HudTexture, TextRange, HUD_HEIGHT, HUD_SAFE_MARGIN, HUD_WIDTH, MAX_HUD_QUADS, UI_SCALE_MAX,
    UI_SCALE_MIN,
};
pub use light::{
    camera_depth_range, fit_cascades, slice_corners, sun_view_proj, CascadeFit, CASCADES,
};
pub use mesh::{
    emissive, emissive_scale, emissive_window, Mesh, Vertex, EMISSIVE_MAX_SCALE,
    EMISSIVE_WINDOW_BIT,
};
pub use pool::{
    ChunkPool, ChunkPoolConfig, ChunkStore, ChunkWrite, PoolError, PoolStats, SlotId,
    MAX_CHUNK_PARTS, MAX_CHUNK_VERTICES,
};
pub use sky::{exposure_from_ev100, MOON_ANGULAR_RADIUS, SUN_ANGULAR_RADIUS};
pub use timing::{GpuCounts, GPU_PARTS, GPU_PART_NAMES, GPU_PASSES, GPU_PASS_NAMES};

use bytemuck::{Pod, Zeroable};
use cull::{Aabb, Frustum};
use glam::{Mat3, Mat4, Vec3, Vec4, Vec4Swizzles};
use hud::HudQuad;
use pipelines::{
    create_instance_buffer, create_shadow_atlas, create_targets, hud_group, upload_map_texture,
};
use sky::SkyModel;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use timing::{DrawWeight, GpuTimer};
use wgpu::util::DeviceExt;

/// Edge of one shadow cascade in texels, when the device allows a texture 3× as wide.
pub const SHADOW_CASCADE_SIZE: u32 = 2048;
/// Default far view depth of each cascade, meters.
pub const DEFAULT_CASCADE_SPLITS: [f32; CASCADES] = [40.0, 160.0, 600.0];
/// Far view depths of the two cascades drawn at [`RenderQuality::cascades`] 2, meters.
pub const TWO_CASCADE_SPLITS: [f32; 2] = [50.0, 400.0];
/// A farthest cascade kept over frames is fitted this much wider than its slice's sphere,
/// so the camera can move and turn a little before the kept map stops covering the slice.
const FAR_CASCADE_PAD: f32 = 0.08;
/// Passes instances are culled for: the forward pass, then each cascade.
const PASSES: usize = 1 + CASCADES;
/// Fraction of each cascade over which it cross-fades into the next.
pub const CASCADE_BLEND: f32 = 0.1;
/// A cascade is kept over frames only while the sun has moved too little to move the
/// shadow of a caster this tall (meters) by half a texel.
const TALL_CASTER: f32 = 50.0;
/// Casters this far beyond a cascade toward the sun still cast into it, meters.
const CASTER_MARGIN: f32 = 400.0;
/// Every cascade's fit while the sun is down: nothing is culled or drawn into it.
const NO_FIT: CascadeFit = CascadeFit {
    view_proj: Mat4::ZERO,
    center: Vec3::ZERO,
    radius: 0.0,
    texel: 0.0,
    depth_range: [0.0; 2],
};
/// Shadow-receiver offsets, in texels of the receiving cascade.
const SHADOW_NORMAL_OFFSET: f32 = 1.5;
const SHADOW_SUN_OFFSET: f32 = 1.0;
/// Frames after which `render` must not create GPU resources (debug-asserted).
pub const WARMUP_FRAMES: u64 = 4;
/// Lights per frame; [`SceneView::lights`] beyond this are ignored. 32 bytes each in the
/// per-frame uniform block.
pub const MAX_LIGHTS: usize = 256;
/// Shadow-map comparisons per PCF lookup (3×3, each a hardware 2×2 compare); a fragment
/// in a cascade blend band makes two lookups.
pub const PCF_TAPS: u32 = 9;
/// Lights one instance is shaded by at most ([`LightSet`]).
pub const LIGHTS_PER_DRAW: usize = 8;
/// Coronas per frame: one per light at most, plus the sun and the moon.
pub const MAX_CORONAS: usize = MAX_LIGHTS + 2;
// A light set travels in two words of instance attributes, a byte per light index.
const _: () = assert!(LIGHTS_PER_DRAW <= 8 && MAX_LIGHTS <= 256);
/// Corona brightness after exposure, as the brightest channel at the sprite's center, that
/// a light reaches as it gets much brighter than the exposure (a soft knee: `x / (1 + x)`
/// of this), before [`Sky::corona_strength`]...
const CORONA_PEAK: f32 = 2.0;
/// ...where `x` is the light's luminous intensity (kcd, luminance-weighted) × exposure ×
/// this: a sodium lamp (2 kcd) at night (exposure ≈ 100) sits well up the knee, at dusk
/// about a third of the way, and a headlamp at noon nowhere.
const CORONA_GAIN: f32 = 0.02;
/// The same for the sun (its illuminance in klux) and the moon (its disc luminance in
/// kcd/m² × the lit fraction), with their sprite radii in radians.
const SUN_CORONA_GAIN: f32 = 0.1;
const SUN_CORONA_STRENGTH: f32 = 0.5;
const SUN_CORONA_RADIUS: f32 = 0.07;
const MOON_CORONA_GAIN: f32 = 0.02;
const MOON_CORONA_STRENGTH: f32 = 0.35;
const MOON_CORONA_RADIUS: f32 = 0.035;
/// Coronas dimmer than this after exposure are not drawn.
const CORONA_MIN_PEAK: f32 = 2e-3;
/// Shortest and longest corona radius on screen, in pixels at 720 lines (scaled with the
/// target's height).
const CORONA_MIN_PX: f32 = 2.5;
const CORONA_MAX_PX: f32 = 140.0;
/// Levels in the bloom chain, the first at half the target resolution.
pub const BLOOM_LEVELS: usize = 5;
/// Vignette strength: the corners lose this fraction of their light.
pub const VIGNETTE: f32 = 0.15;
/// Highest post-exposure channel value a pixel feeds into bloom (brighter pixels are scaled
/// down to it, keeping their hue), so the sun disc or a lamp cannot flood the chain.
const BLOOM_CLAMP: f32 = 64.0;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const FONT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// The HUD map: sRGB-encoded texels read as plain unorm, like the HUD's colors.
const MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Widest map [`Renderer::upload_hud_image`] takes: WebGL2's guaranteed texture size.
pub const MAX_MAP_SIZE: u32 = 2048;
/// Stride of the per-cascade uniform slots; the largest offset alignment WebGPU allows.
const CASCADE_UNIFORM_STRIDE: u64 = 256;

/// FNV-1a over 32-bit words: the cascades' caster hashes.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(hash: u64, word: u32) -> u64 {
    (hash ^ u64::from(word)).wrapping_mul(0x0000_0100_0000_01b3)
}

/// A cascade as last drawn ([`Renderer::kept_map`]).
#[derive(Clone, Copy, Debug)]
struct KeptCascade {
    fit: CascadeFit,
    /// The sun direction it was drawn for.
    sun: Vec3,
    /// Its casters ([`Renderer::cull`]).
    hash: u64,
}

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
    /// Multiplier for the mesh's vertex emissive, applied before [`Sky::emissive_cap`]:
    /// 1 as authored, 0 dark (lamps that switch, instruments on a dimmer).
    pub glow: f32,
    /// Share, `0..=1`, of [`Sky::cabin_light`] this instance receives as extra ambient: 1
    /// for a car's interior seen from the driver's seat, 0 for everything else.
    pub cabin: f32,
    /// The [`SceneView::lights`] that shade it (the sun always does).
    pub lights: LightSet,
    /// Drawn into the shadow cascades. Off for what glows rather than blocks the sun (neon
    /// and lamp lens meshes).
    pub casts_shadow: bool,
    /// Darkened where the sun's shadow cascades say something blocks it. Off for what is
    /// not in the world's light (the vehicle select's turntable model, just in front of the
    /// camera).
    pub receives_shadow: bool,
    /// Drawn after the other instances in the forward pass: a backdrop that nearer
    /// geometry mostly hides (the far terrain), so the depth test rejects those pixels
    /// before they are shaded.
    pub late: bool,
    /// Window glass: drawn after everything else (the sky too) without writing depth,
    /// blended over what is behind it by its color's alpha, more opaque at grazing angles.
    /// Never in the depth pre-pass.
    pub glass: bool,
}

impl MeshInstance {
    /// `mesh` at `transform`, untinted, glowing as authored, outside any cabin, lit by the
    /// sun and sky only.
    pub fn new(mesh: MeshId, transform: [[f32; 4]; 4]) -> MeshInstance {
        MeshInstance {
            mesh,
            transform,
            color: [1.0; 4],
            glow: 1.0,
            cabin: 0.0,
            lights: LightSet::EMPTY,
            casts_shadow: true,
            receives_shadow: true,
            late: false,
            glass: false,
        }
    }
}

/// Up to [`LIGHTS_PER_DRAW`] indices into [`SceneView::lights`]: the lights an instance is
/// shaded by, each with a weight ([`LightSet::with_weight`], 1 unless set). Indices past the
/// end of the frame's lights are ignored. A caller choosing among more lights than fit
/// fades a light out as others overtake it instead of swapping them in one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightSet {
    len: u8,
    indices: [u8; LIGHTS_PER_DRAW],
    /// How far each light is dimmed, 0 (full) to [`DIM_STEPS`] (off).
    dim: [u8; LIGHTS_PER_DRAW],
}

/// Steps of a light's weight in a set: four bits each in the instance's packed params.
const DIM_STEPS: u8 = 15;

impl LightSet {
    pub const EMPTY: LightSet = LightSet {
        len: 0,
        indices: [0; LIGHTS_PER_DRAW],
        dim: [0; LIGHTS_PER_DRAW],
    };

    /// The set with its `k`th light scaled by `weight` (0..=1, in fifteenths).
    pub fn with_weight(mut self, k: usize, weight: f32) -> LightSet {
        if k < self.len() {
            self.dim[k] = ((1.0 - weight.clamp(0.0, 1.0)) * f32::from(DIM_STEPS)).round() as u8;
        }
        self
    }

    /// The weight of the `k`th light: 1 unless [`LightSet::with_weight`] dimmed it.
    pub fn weight(&self, k: usize) -> f32 {
        1.0 - f32::from(self.dim.get(k).copied().unwrap_or(0)) / f32::from(DIM_STEPS)
    }

    /// Adds light `index`; false (and unchanged) when the set is full or `index` is not
    /// below [`MAX_LIGHTS`].
    pub fn push(&mut self, index: usize) -> bool {
        if usize::from(self.len) >= LIGHTS_PER_DRAW || index >= MAX_LIGHTS {
            return false;
        }
        self.indices[usize::from(self.len)] = index as u8;
        self.len += 1;
        true
    }

    /// The set from `indices` (the first [`LIGHTS_PER_DRAW`] of them below [`MAX_LIGHTS`]).
    pub fn from_indices(indices: &[usize]) -> LightSet {
        let mut set = LightSet::EMPTY;
        for &i in indices {
            set.push(i);
        }
        set
    }

    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn indices(&self) -> &[u8] {
        &self.indices[..self.len()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// World-to-view, column-major, right-handed (camera looks down -Z).
    pub view: [[f32; 4]; 4],
    /// Perspective view-to-clip with depth in `[0, 1]`
    /// (e.g. `glam::camera::rh::proj::directx::perspective`).
    pub proj: [[f32; 4]; 4],
}

/// The sun as a directional light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    /// Unit vector from the scene toward the sun (world space, +Y up). It may point below
    /// the horizon; the sun then stops lighting and the sky goes to twilight and night.
    pub direction: [f32; 3],
    /// Illuminance on a surface facing the sun, linear RGB, in kilolux: clear noon ≈ 100,
    /// golden hour ≈ 10–30 and orange. A white Lambertian surface facing the sun has
    /// luminance `illuminance / π` kcd/m².
    pub illuminance: [f32; 3],
}

/// Parameters of the analytic sky. The sky's own colors come from the Preetham model for
/// the sun direction and `turbidity`; they also light the scene as hemisphere ambient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    /// Atmospheric turbidity (dimensionless): 2 very clear, 3 clear, 6 light haze, 10 hazy.
    /// Clamped to `[1.9, 10]`.
    pub turbidity: f32,
    /// Unit vector toward the moon (world space).
    pub moon_direction: [f32; 3],
    /// Luminance of a fully lit moon disc, kcd/m² (≈ 2.5 in reality); 0 hides the moon.
    /// The phase follows from the sun and moon directions.
    pub moon_luminance: f32,
    /// Star field strength, 0 (none) to 1 (dark rural sky). Fixed in world space.
    pub star_brightness: f32,
    /// Luminance of the moonless night sky, kcd/m² (≈ 2e-5 for a small town).
    pub night_luminance: f32,
    /// Linear albedo of the ground: the view below the horizon and ambient from below.
    pub ground_albedo: [f32; 3],
    /// Aerial-perspective fog density, 1/m (0 disables it): about 3.9 / visibility, so
    /// 0.0002 matches a clear turbidity-3 sky and 0.001 is a hazy marine layer.
    pub fog_density: f32,
    /// Multiplier from luminance in kcd/m² to tonemapper input (≈ 0.05 on a sunny day); see
    /// [`exposure_from_ev100`].
    pub exposure: f32,
    /// Luminance, kcd/m², of a vertex emissive with rgb 255 and scale 1 (see
    /// [`emissive`]). At the default 0.5 a neon tube (scale 4–8) is 2–4 kcd/m² like a real
    /// one: faint in sunshine, glowing at dusk, blown out with a halo at night; lit windows
    /// want scale 0.1–0.3 (50–150 cd/m²).
    pub emissive_boost: f32,
    /// Multiplier, 0..1, for emissives flagged as windows ([`emissive_window`]): 0 by day,
    /// 1 at night.
    pub window_emissive_scale: f32,
    /// Post-exposure luminance (tonemapper input, where 0.18 is mid grey and AgX's white
    /// is ≈ 16) above which light blooms, with a soft knee half as wide below it. At the
    /// default 1.0 the noon sky away from the sun stays under it.
    pub bloom_threshold: f32,
    /// Weight of each of the five blurred copies of the over-threshold light added back
    /// (so up to 5× this in all, spread from a few pixels to a sixth of the screen); 0
    /// skips bloom entirely.
    pub bloom_strength: f32,
    /// Most an emissive may be after exposure (tonemapper input, the scale of
    /// `bloom_threshold`), so a sign that reads at dusk does not blow out into a blob at
    /// night. A soft knee: values well under it pass unchanged, values far over it land on
    /// it, hue kept. Daylight exposure leaves every emissive far under it.
    pub emissive_cap: f32,
    /// Ambient radiance, linear RGB kcd/m², added for instances by their
    /// [`MeshInstance::cabin`]: the light inside a car (through the windows by day, the dome
    /// and instrument backlight by night) that the sky hemisphere misses because the roof
    /// and the cabin's own shadow hide it. The shader weights it by the surface's facing: all
    /// of it on surfaces facing up, a tenth on those facing down.
    pub cabin_light: [f32; 3],
    /// Brightness of every corona ([`Corona`], and the sun's and moon's), 0 (none) to 1.
    /// A corona's own brightness follows its light against the exposure: a warm halo round
    /// a street lamp at night, nothing by day.
    pub corona_strength: f32,
}

impl Default for Sky {
    /// A clear day with no moon or stars, exposed for sunshine (EV 14).
    fn default() -> Sky {
        Sky {
            turbidity: 3.0,
            moon_direction: [0.0, -1.0, 0.0],
            moon_luminance: 0.0,
            star_brightness: 0.0,
            night_luminance: 2e-5,
            ground_albedo: [0.12, 0.11, 0.10],
            fog_density: 0.0003,
            exposure: exposure_from_ev100(14.0),
            emissive_boost: 0.5,
            window_emissive_scale: 0.0,
            bloom_threshold: 1.0,
            bloom_strength: 0.08,
            emissive_cap: 8.0,
            cabin_light: [0.0; 3],
            corona_strength: 0.6,
        }
    }
}

/// A light with a smooth inverse-square falloff that reaches zero at `radius`: a street
/// lamp head, a neon sign's spill, or (with a `direction`) a headlamp's cone. It lights every
/// mesh in the forward pass (no shadows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    /// World position, meters.
    pub position: [f32; 3],
    /// Luminous intensity, linear RGB, in kilocandela: the illuminance in klux it gives a
    /// surface facing it 1 m away (on the cone's axis for a spot). A sodium street lamp is
    /// ≈ 2–3 kcd (≈ 40 lux on the road 8 m below it); a halogen headlamp's beam ≈ 10–40.
    pub color: [f32; 3],
    /// Distance at which the light has faded to nothing, meters (≈ 25 for a street lamp).
    pub radius: f32,
    /// Spot axis (unit, world space), or zero for a light that shines every way.
    pub direction: [f32; 3],
    /// Spot cone as cosines of the angle off the axis: full intensity inside `[1]`, fading
    /// to nothing at `[0]` (so `[0] < [1]`). Ignored without a `direction`.
    pub cone_cos: [f32; 2],
    /// Spot only: angles above and below the axis (against world up) count this many times
    /// before the cone is applied, so the beam is this much narrower vertically than
    /// across: a headlamp's flat beam. 1 is a round cone; ignored for a vertical axis.
    pub cone_squash: f32,
    /// Spot only: how far the surface's cosine is flattened, `(n·l + wrap) / (1 + wrap)`
    /// (0 is Lambert). A headlamp throws most of its light toward the horizon, so the road
    /// far ahead, met at a grazing angle, still reads; this stands in for that distribution.
    pub wrap: f32,
    /// The glow sprite drawn over the light, if any. A light with radius 0 lights nothing
    /// but still draws its corona (a tail lamp).
    pub corona: Corona,
}

impl PointLight {
    /// A light shining every way, without a corona.
    pub fn omni(position: [f32; 3], color: [f32; 3], radius: f32) -> PointLight {
        PointLight {
            position,
            color,
            radius,
            direction: [0.0; 3],
            cone_cos: [-1.0, -1.0],
            cone_squash: 1.0,
            wrap: 0.0,
            corona: Corona::NONE,
        }
    }
}

/// A soft additive glow over a light, drawn after the forward pass (the era's night-driving corona).
/// Its color is the light's; its brightness follows the light's intensity against the
/// exposure, up to a cap, times `strength` and [`Sky::corona_strength`]. Hidden by whatever
/// the scene draws in front of the light, and faded as the light turns away from the view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corona {
    /// Radius in the world, m: the sprite shrinks with distance down to a few pixels and
    /// then dims instead. 0 draws none.
    pub radius: f32,
    /// Brightness multiplier, 0 to 1.
    pub strength: f32,
}

impl Corona {
    pub const NONE: Corona = Corona {
        radius: 0.0,
        strength: 0.0,
    };
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
    pub instances: &'a [MeshInstance],
    /// Lights for this frame; only the first [`MAX_LIGHTS`] are used. Each lights the
    /// instances whose [`LightSet`] names it, and draws its [`Corona`].
    pub lights: &'a [PointLight],
    /// Text and rectangles drawn over the frame ([`Hud::EMPTY`] for none).
    pub hud: Hud<'a>,
    /// How much work the frame may cost; a change is applied before the frame (rebuilding
    /// the shadow atlas or the scaled targets when their size changes).
    pub quality: RenderQuality,
}

/// The quality knobs that trade GPU time for fidelity. [`RenderQuality::default`] is the
/// full pipeline (three 2048² cascades, native resolution, five bloom levels, coronas).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderQuality {
    /// Edge of one shadow cascade in texels (at most a third of the device's widest
    /// texture).
    pub shadow_size: u32,
    /// Shadow cascades drawn: 3 ([`RendererOptions::cascade_splits`]) or 2
    /// ([`TWO_CASCADE_SPLITS`]).
    pub cascades: u32,
    /// Size of the scene's color and depth targets as a fraction of the swapchain,
    /// 0.5–1; the tonemap pass upscales bilinearly and the HUD stays at full size. Applies
    /// to the HDR path only (the LDR fallback draws straight into the swapchain).
    pub resolution_scale: f32,
    /// Levels of the bloom chain, 1–[`BLOOM_LEVELS`].
    pub bloom_levels: u32,
    /// Coronas over lights, the sun and the moon.
    pub coronas: bool,
    /// The farthest cascade is drawn every this many frames (1: every frame) and kept in
    /// between, while its map still covers the slice and the sun has not moved.
    pub far_cascade_interval: u32,
    /// Casters smaller than this many texels of a cascade (their bounds' diagonal) are
    /// left out of it; 0 keeps all.
    pub min_caster_texels: f32,
    /// The forward pass draws the meshes' depth first, then shades each pixel once (about
    /// half the fragments on the coast, for every mesh drawn twice: worth it where the GPU
    /// is short of fragment time, not where draws cost the CPU most).
    pub depth_prepass: bool,
}

impl Default for RenderQuality {
    fn default() -> RenderQuality {
        RenderQuality {
            shadow_size: SHADOW_CASCADE_SIZE,
            cascades: CASCADES as u32,
            resolution_scale: 1.0,
            bloom_levels: BLOOM_LEVELS as u32,
            coronas: true,
            far_cascade_interval: 1,
            min_caster_texels: 0.0,
            depth_prepass: false,
        }
    }
}

impl RenderQuality {
    /// Every knob inside its range; `max_shadow_size` is the device's limit.
    pub fn sanitized(self, max_shadow_size: u32) -> RenderQuality {
        let scale = if self.resolution_scale.is_finite() {
            self.resolution_scale.clamp(0.5, 1.0)
        } else {
            1.0
        };
        RenderQuality {
            shadow_size: self.shadow_size.clamp(256, max_shadow_size.max(256)),
            cascades: self.cascades.clamp(2, CASCADES as u32),
            resolution_scale: scale,
            bloom_levels: self.bloom_levels.clamp(1, BLOOM_LEVELS as u32),
            coronas: self.coronas,
            far_cascade_interval: self.far_cascade_interval.clamp(1, 4),
            min_caster_texels: self.min_caster_texels.max(0.0),
            depth_prepass: self.depth_prepass,
        }
    }

    /// The scene's target size for a swapchain of `surface` pixels.
    pub fn render_size(&self, surface: (u32, u32)) -> (u32, u32) {
        let s = |v: u32| ((v as f32 * self.resolution_scale).round() as u32).clamp(1, v.max(1));
        (s(surface.0), s(surface.1))
    }
}

impl SceneView<'_> {
    /// A small fixed scene for start-up checks: the camera 30 m back from the origin and
    /// looking at it, a low sun straight ahead (so the sky, the sun's corona and, in float
    /// mode, bloom and the tonemap all draw), plus the caller's `instances` and `lights`,
    /// at the default [`RenderQuality`] (replace `quality` to check another).
    pub fn probe<'a>(instances: &'a [MeshInstance], lights: &'a [PointLight]) -> SceneView<'a> {
        SceneView {
            camera: Camera {
                view: glam::camera::rh::view::look_at_mat4(
                    Vec3::new(0.0, 4.0, 30.0),
                    Vec3::ZERO,
                    Vec3::Y,
                )
                .to_cols_array_2d(),
                proj: glam::camera::rh::proj::directx::perspective(1.0, 16.0 / 9.0, 0.1, 1000.0)
                    .to_cols_array_2d(),
            },
            sun: Sun {
                direction: Vec3::new(0.0, 0.15, -1.0).normalize().to_array(),
                illuminance: [30.0, 22.0, 12.0],
            },
            sky: Sky::default(),
            instances,
            lights,
            hud: Hud::EMPTY,
            quality: RenderQuality::default(),
        }
    }
}

/// How the renderer stores the lit image before tonemapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HdrMode {
    /// `Float16` when the device can render to `Rgba16Float`, else `Ldr`.
    #[default]
    Auto,
    /// `Rgba16Float` target and a separate tonemap pass.
    Float16,
    /// Tonemap in the forward pass, straight into the swapchain.
    Ldr,
}

/// How the corona pass reads the scene's depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CoronaDepth {
    /// `Sampled` on GL (WebGL2), `Loaded` on every other backend.
    #[default]
    Auto,
    /// The depth texture bound as an unfilterable float texture and read through a nearest
    /// sampler: what GL needs (a float depth texture is incomplete there unless sampled
    /// with nearest filtering).
    Sampled,
    /// The depth texture bound as a `texture_depth_2d` and read with `textureLoad`, no
    /// sampler: the plainest depth read on WebGPU, Metal, Vulkan and D3D12.
    Loaded,
}

/// Where an optional feature's start-up check stands. The browser's WebGPU answers error
/// scopes a moment later, so a check can be pending for the first frames; the feature is
/// left out of frames until it passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe<'a> {
    Pending,
    Passed,
    /// Failed with the device's error message; the feature stays off.
    Failed(&'a str),
}

/// Construction options for [`Renderer::with_options`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RendererOptions {
    pub hdr: HdrMode,
    pub corona_depth: CoronaDepth,
    /// Far view depth of each shadow cascade, meters, increasing.
    pub cascade_splits: [f32; CASCADES],
    /// The pooled chunk geometry, allocated here once ([`ChunkStore`]).
    pub chunk_pool: ChunkPoolConfig,
    /// The quality the renderer starts with (each [`SceneView::quality`] replaces it).
    pub quality: RenderQuality,
    /// Frustum culling of instances (off draws every instance in every pass; for
    /// comparisons).
    pub culling: bool,
}

impl Default for RendererOptions {
    fn default() -> RendererOptions {
        RendererOptions {
            hdr: HdrMode::Auto,
            corona_depth: CoronaDepth::Auto,
            cascade_splits: DEFAULT_CASCADE_SPLITS,
            chunk_pool: ChunkPoolConfig::NONE,
            quality: RenderQuality::default(),
            culling: true,
        }
    }
}

/// What a frame has to draw ([`Renderer::plan`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameWork {
    /// Something the world passes draw changed: every pass.
    #[default]
    World,
    /// Only the HUD changed: the kept HDR frame is tonemapped again with the HUD over it.
    Ui,
    /// Nothing changed: the frame on screen is still right; nothing to draw or present.
    Skip,
}

/// The world inputs the last world frame was drawn from, to tell whether the next would
/// draw the same ([`Renderer::plan`]).
struct KeptWorld {
    /// Camera, sun, sky, quality and whether coronas were drawn; `None` before the first
    /// world frame and after anything that changes what the same scene draws (targets
    /// rebuilt, a mesh added, a chunk written or freed).
    head: Option<(Camera, Sun, Sky, RenderQuality, bool)>,
    instances: Vec<MeshInstance>,
    /// The first [`MAX_LIGHTS`] (the rest are never drawn).
    lights: Vec<PointLight>,
}

impl KeptWorld {
    /// The same world as `scene` would draw: everything equal, but orientations only
    /// [`turned_little`] and positions only [`moved_little`] as seen from the camera (a car
    /// at rest jitters by a billionth of a radian on its springs forever).
    fn same(&self, scene: &SceneView, coronas: bool) -> bool {
        let lights = &scene.lights[..scene.lights.len().min(MAX_LIGHTS)];
        let Some((camera, sun, sky, quality, kept_coronas)) = &self.head else {
            return false;
        };
        let (view, new_view) = (&camera.view, &scene.camera.view);
        let eye = Mat4::from_cols_array_2d(view).inverse().w_axis.truncate();
        let new_eye = Mat4::from_cols_array_2d(new_view)
            .inverse()
            .w_axis
            .truncate();
        let instance = |a: &MeshInstance, b: &MeshInstance| {
            let (ta, tb) = (&a.transform, &b.transform);
            turned_little(ta[..3].as_flattened(), tb[..3].as_flattened())
                && moved_little(Vec3::from_slice(&ta[3]), Vec3::from_slice(&tb[3]), eye)
                && MeshInstance {
                    transform: b.transform,
                    ..*a
                } == *b
        };
        let light = |a: &PointLight, b: &PointLight| {
            let (pa, pb) = (Vec3::from(a.position), Vec3::from(b.position));
            moved_little(pa, pb, eye)
                && turned_little(&a.direction, &b.direction)
                && PointLight {
                    position: b.position,
                    direction: b.direction,
                    ..*a
                } == *b
        };
        turned_little(view[..3].as_flattened(), new_view[..3].as_flattened())
            && moved_little(eye, new_eye, eye)
            && camera.proj == scene.camera.proj
            && (*sun, *sky, *quality, *kept_coronas)
                == (scene.sun, scene.sky, scene.quality, coronas)
            && self.instances.len() == scene.instances.len()
            && self
                .instances
                .iter()
                .zip(scene.instances)
                .all(|(a, b)| instance(a, b))
            && self.lights.len() == lights.len()
            && self.lights.iter().zip(lights).all(|(a, b)| light(a, b))
    }

    fn keep(&mut self, scene: &SceneView, coronas: bool) {
        let lights = &scene.lights[..scene.lights.len().min(MAX_LIGHTS)];
        self.head = Some((scene.camera, scene.sun, scene.sky, scene.quality, coronas));
        self.instances.clear();
        self.instances.extend_from_slice(scene.instances);
        self.lights.clear();
        self.lights.extend_from_slice(lights);
    }
}

/// How far an orientation may turn and still draw the same frame: two millionths of each
/// element of a rotation (and scale) matrix or a direction (of 1 + the element, for
/// scaled ones), a few millionths of a radian.
const STILL_TURN: f32 = 2e-6;

/// How far a point may move and still draw the same frame: a tenth of a millimetre, plus
/// a millionth of its distance from the eye (a millionth of a radian as seen, wherever in
/// the place it is).
const STILL_MOVE_M: f32 = 1e-4;
const STILL_MOVE_PER_M: f32 = 1e-6;

/// Every element of `a` within [`STILL_TURN`] of `b`'s (and neither NaN).
fn turned_little(a: &[f32], b: &[f32]) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| (x - y).abs() <= STILL_TURN * (1.0 + x.abs()))
}

/// `b` within [`STILL_MOVE_M`] and [`STILL_MOVE_PER_M`] of `a` seen from `eye`, on every
/// axis (and neither NaN).
fn moved_little(a: Vec3, b: Vec3, eye: Vec3) -> bool {
    let limit = STILL_MOVE_M + STILL_MOVE_PER_M * a.distance(eye);
    let d = (a - b).abs();
    d.x <= limit && d.y <= limit && d.z <= limit
}

/// Counters for the last rendered frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// The graphics backend ([`backend_label`]).
    pub backend: &'static str,
    /// Frames rendered so far (the world drawn).
    pub frames: u64,
    /// Frames that kept the last world frame and drew only the tonemap and the HUD over it
    /// ([`FrameWork::Ui`]).
    pub ui_frames: u64,
    /// Frames [`Renderer::draw`] drew nothing for ([`FrameWork::Skip`]).
    pub skipped_frames: u64,
    /// What the last frame drew; after a Ui or Skip frame the pass counters below are the
    /// last world frame's.
    pub last_work: FrameWork,
    /// Draw calls in the last frame, all passes.
    pub draw_calls: u32,
    /// Of which in the shadow pass.
    pub shadow_draw_calls: u32,
    /// Of which in the bloom passes (0 with bloom off or in LDR mode).
    pub bloom_draw_calls: u32,
    /// Mesh vertex and index buffer binds (a pair counts once) in the last frame's shadow
    /// and forward passes: one per pass sweep for the chunk pool, plus one per run of a
    /// standalone mesh, however many pool draws there are.
    pub buffer_binds: u32,
    /// Lights in the last frame's scene (at most [`MAX_LIGHTS`]).
    pub lights_submitted: u32,
    /// Instances drawn in the forward pass with a full [`LightSet`] ([`LIGHTS_PER_DRAW`]
    /// lights, each evaluated for every fragment), and the lights in all their sets.
    pub full_light_sets: u32,
    pub light_slots: u32,
    /// Of which in at least one instance's [`LightSet`].
    pub lights_used: u32,
    /// Corona sprites submitted (before the depth test hides any).
    pub coronas: u32,
    /// Mesh instances in the last frame's scene (submitted).
    pub instances: u32,
    /// Of which drawn in the forward pass (inside the camera frustum).
    pub instances_drawn: u32,
    /// Instances drawn into each shadow cascade (0 for a cascade not drawn this frame).
    pub shadow_instances: [u32; CASCADES],
    /// Triangles drawn in the forward pass (meshes only) and in all cascades together.
    pub triangles: u64,
    pub shadow_triangles: u64,
    /// Edge of one shadow cascade (texels), cascades in use, and of those the ones drawn
    /// this frame (the others kept from an earlier frame).
    pub shadow_size: u32,
    pub cascades: u32,
    pub cascades_drawn: u32,
    /// Swapchain size and the scene's (scaled) target size, pixels.
    pub surface_size: (u32, u32),
    pub render_size: (u32, u32),
    /// Bloom levels drawn (0 with bloom off or in LDR mode).
    pub bloom_levels: u32,
    /// GPU time of each pass ([`GPU_PASS_NAMES`]), microseconds, a frame or two old; all 0
    /// unless [`Renderer::enable_gpu_timing`] succeeded.
    pub gpu_us: [u32; GPU_PASSES],
    /// GPU time of each part of a pass ([`GPU_PART_NAMES`]: each cascade, the forward
    /// pass's meshes, late meshes and sky), microseconds, a frame or two old; all 0 unless
    /// [`RenderStats::gpu_parts`].
    pub gpu_part_us: [u32; GPU_PARTS],
    /// Medians of [`RenderStats::gpu_us`] and [`RenderStats::gpu_part_us`] over the last
    /// 64 frames read back.
    pub gpu_median_us: [u32; GPU_PASSES],
    pub gpu_median_part_us: [u32; GPU_PARTS],
    /// Fragments shaded and depth texels written, from occlusion queries, with GPU timing.
    pub gpu_counts: GpuCounts,
    /// The frame the GPU readings are from (0: none yet); a new value means a new reading.
    pub gpu_frame: u64,
    /// GPU timestamps are being read.
    pub gpu_timing: bool,
    /// The device writes timestamps inside passes, so the parts are timed too.
    pub gpu_parts: bool,
    /// Times the size-dependent targets or the shadow atlas were rebuilt for a quality
    /// change.
    pub rebuilds: u32,
    /// HUD quads (glyphs and rectangles) drawn in the last frame.
    pub hud_quads: u32,
    /// HUD quads beyond [`MAX_HUD_QUADS`] left out of the last frame.
    pub hud_quads_dropped: u32,
    /// GPU resources `render` has created over the renderer's life (instance buffer growth).
    pub resources_created: u32,
    /// The chunk pool's slots and bytes.
    pub chunk_pool: PoolStats,
}

/// GPU layout of the per-frame uniform block; mirrors `Frame` in shaders/common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    /// `view_proj` with the camera at the origin: what meshes are drawn with, their
    /// instances' translations being relative to the camera ([`camera_relative`]).
    rel_view_proj: [[f32; 4]; 4],
    cascade_view_proj: [[[f32; 4]; 4]; CASCADES],
    camera_pos: [f32; 4],
    camera_fwd: [f32; 4],
    cascade_far: [f32; 4],
    cascade_texel: [f32; 4],
    sun_dir: [f32; 4],
    sun_light: [f32; 4],
    sun_disc: [f32; 4],
    moon_dir: [f32; 4],
    moon_light: [f32; 4],
    perez: [[f32; 4]; 5],
    zenith: [f32; 4],
    sun_sky: [f32; 4],
    sky_params: [f32; 4],
    night_sky: [f32; 4],
    sky_up: [f32; 4],
    sky_ground: [f32; 4],
    sky_horizon: [f32; 4],
    params: [f32; 4],
    emissive: [f32; 4],
    post: [f32; 4],
    screen: [f32; 4],
    cabin_light: [f32; 4],
    corona: [f32; 4],
    render_size: [f32; 4],
    shadow_params: [f32; 4],
    lights: [PackedLight; MAX_LIGHTS],
}

/// GPU layout of one light in 32 bytes; mirrors `PackedLight` in shaders/common.wgsl and
/// `unpack_light` in shaders/forward.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PackedLight {
    /// xyz position (m), w radius (m; 0 lights nothing).
    pos_radius: [f32; 4],
    /// x: f16 intensity r | g << 16 (kcd); y: f16 intensity b | f16 cosine where the cone
    /// ends << 16 (below -1 for an omni light); z: spot axis, octahedral, snorm16 x | y <<
    /// 16; w: f16 1 / (cosine of full intensity − cosine where the cone ends) | vertical
    /// squash as `(squash − 1) × 16` (u8) << 16 | wrap × 255 (u8) << 24.
    bits: [u32; 4],
}

impl PackedLight {
    fn new(light: &PointLight) -> PackedLight {
        let [x, y, z] = light.position;
        let [cr, cg, cb] = light.color.map(|c| c.max(0.0));
        let axis = Vec3::from(light.direction).normalize_or_zero();
        // An omni light's "cone" ends below -1, so every direction is inside it.
        let (outer, inner, squash, wrap) = if axis == Vec3::ZERO {
            (-2.0, -1.0, 1.0, 0.0)
        } else {
            let o = light.cone_cos[0].clamp(-1.0, 1.0);
            let i = light.cone_cos[1].clamp(o + 1e-4, 1.0 + 1e-4);
            // A vertical axis has no "above" to squash.
            let squash = if axis.y.abs() > 0.999 {
                1.0
            } else {
                light.cone_squash.max(1.0)
            };
            (o, i, squash, light.wrap.clamp(0.0, 1.0))
        };
        let half2 = |a: f32, b: f32| u32::from(f16_bits(a)) | u32::from(f16_bits(b)) << 16;
        let [ox, oy] = oct_encode(axis).map(|v| u32::from(snorm16(v)));
        let squash = ((squash - 1.0) * 16.0).round().clamp(0.0, 255.0) as u32;
        let wrap = (wrap * 255.0).round() as u32;
        PackedLight {
            pos_radius: [x, y, z, light.radius.max(0.0)],
            bits: [
                half2(cr, cg),
                half2(cb, outer),
                ox | oy << 16,
                u32::from(f16_bits(1.0 / (inner - outer))) | squash << 16 | wrap << 24,
            ],
        }
    }
}

/// IEEE 754 half-precision bits of `x`, rounded to nearest even; magnitudes past the largest
/// half (65504) saturate to it, NaN stays NaN. The shader reads them with `unpack2x16float`.
fn f16_bits(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | if man != 0 { 0x7e00 } else { 0x7bff };
    }
    // Round `m >> shift` to nearest, ties to even.
    let round = |m: u32, shift: u32| -> u32 {
        let q = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        q + u32::from(rem > half || (rem == half && q & 1 == 1))
    };
    let e = exp - 127 + 15;
    let h = if e >= 1 {
        // Normal; a carry out of the mantissa bumps the exponent, as it should.
        ((e as u32) << 10) + round(man, 13)
    } else if exp == 0 || 126 - exp > 24 {
        0
    } else {
        // Subnormal half: the value in units of 2^-24.
        round(man | 0x80_0000, (126 - exp) as u32)
    };
    sign | h.min(0x7bff) as u16
}

/// `x` (clamped to `[-1, 1]`) as a snorm16, as `unpack2x16snorm` reads it.
fn snorm16(x: f32) -> u16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16 as u16
}

/// Octahedral encoding (Cigolle et al. 2014) of unit `v` (zero maps to `[0, 0]`); the
/// shader's `oct_decode` inverts it.
fn oct_encode(v: Vec3) -> [f32; 2] {
    let l1 = v.x.abs() + v.y.abs() + v.z.abs();
    if l1 == 0.0 {
        return [0.0; 2];
    }
    let n = v / l1;
    if n.z >= 0.0 {
        return [n.x, n.y];
    }
    let sign = |a: f32| if a >= 0.0 { 1.0 } else { -1.0 };
    [(1.0 - n.y.abs()) * sign(n.x), (1.0 - n.x.abs()) * sign(n.y)]
}

/// GPU layout of one corona sprite (vertex buffer, step mode instance); mirrors `CoronaIn`
/// in shaders/corona.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CoronaRaw {
    /// xyz world position (m), w radius (m); or xyz unit direction, w −angular radius.
    pos_size: [f32; 4],
    /// rgb radiance at the center (kcd/m²), w unused.
    color: [f32; 4],
}

/// One cascade's matrix for the shadow pass, padded to its uniform slot.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CascadeUniform {
    view_proj: [[f32; 4]; 4],
    _pad: [[f32; 4]; 12],
}

/// GPU layout of one instance (vertex buffer, step mode instance).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    /// Inverse-transpose of the model's upper 3x3, columns padded to vec4.
    normal: [[f32; 4]; 3],
    color: [f32; 4],
    /// x glow (emissive multiplier) as f16 bits | the cabin-light share as unorm8 << 16 |
    /// the light set's length << 24 (4 bits) | [`NO_SUN_SHADOW`] when it receives no
    /// shadow, y how far each light of the set is dimmed (4 bits a light, the first lowest),
    /// zw the set's light indices, a byte each, low byte first.
    params: [u32; 4],
}

impl InstanceRaw {
    /// `inst` for a frame with `lights` lights (indices at or past it are left out) seen
    /// from `eye`, its translation relative to it ([`camera_relative`]).
    fn new(inst: &MeshInstance, lights: usize, eye: Vec3) -> InstanceRaw {
        let model = Mat4::from_cols_array_2d(&inst.transform);
        let n = Mat3::from_mat4(model).inverse().transpose();
        InstanceRaw {
            model: camera_relative(inst.transform, eye),
            normal: [
                n.x_axis.extend(0.0).to_array(),
                n.y_axis.extend(0.0).to_array(),
                n.z_axis.extend(0.0).to_array(),
            ],
            color: inst.color,
            params: instance_params(inst, lights),
        }
    }
}

/// `transform` with its translation taken relative to `eye`. Meshes are drawn camera
/// relative: in world coordinates a vertex a kilometre or two from the origin is rounded
/// to a tenth of a millimetre or more twice on the GPU (into the world, then back toward
/// the camera), and a thin layer half a millimetre off its backing (a radio display, a
/// mirror's glass, a sign's plate) lost to it in patches that changed every frame while
/// the camera moved. Relative to the eye the numbers are small and nothing near cancels.
fn camera_relative(mut transform: [[f32; 4]; 4], eye: Vec3) -> [[f32; 4]; 4] {
    for (t, e) in transform[3].iter_mut().zip(eye.to_array()) {
        *t -= e;
    }
    transform
}

/// `view` without its translation: the same view from a camera at the origin.
fn view_rotation(view: Mat4) -> Mat4 {
    Mat4::from_cols(view.x_axis, view.y_axis, view.z_axis, Vec4::W)
}

/// Bit of `params.x` (just past the light count) that skips the sun's shadow test
/// ([`MeshInstance::receives_shadow`] off); `forward.wgsl` reads it.
const NO_SUN_SHADOW: u32 = 1 << 28;

/// The packed `params` of [`InstanceRaw`].
fn instance_params(inst: &MeshInstance, lights: usize) -> [u32; 4] {
    let mut words = [0u32; 2];
    let mut dims = 0u32;
    let mut n = 0;
    for (k, &i) in inst.lights.indices().iter().enumerate() {
        if usize::from(i) < lights {
            words[n / 4] |= u32::from(i) << (8 * (n % 4));
            dims |= u32::from(inst.lights.dim[k]) << (4 * n);
            n += 1;
        }
    }
    let cabin = (inst.cabin.clamp(0.0, 1.0) * 255.0).round() as u32;
    let shadow = if inst.receives_shadow {
        0
    } else {
        NO_SUN_SHADOW
    };
    [
        u32::from(f16_bits(inst.glow.max(0.0))) | cabin << 16 | (n as u32) << 24 | shadow,
        dims,
        words[0],
        words[1],
    ]
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x3, 2 => Unorm16x2, 3 => Unorm8x4, 4 => Unorm8x4,
];
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
    5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4,
    9 => Float32x4, 10 => Float32x4, 11 => Float32x4, 12 => Float32x4,
    13 => Uint32x4,
];
const CORONA_ATTRIBUTES: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4,
];
const HUD_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Unorm8x4, 3 => Float32x4,
];

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
    /// Bloom and tonemap entry points in one module (the tonemap reuses bloom's tent).
    pub const POST: &str = concat!(
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/bloom.wgsl"),
        include_str!("../shaders/tonemap.wgsl")
    );
    pub const HUD: &str = concat!(
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/hud.wgsl")
    );
    /// Coronas reading the depth through a nearest sampler ([`CoronaDepth::Sampled`], GL).
    ///
    /// [`CoronaDepth::Sampled`]: crate::CoronaDepth::Sampled
    pub const CORONA: &str = concat!(
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/corona_sample.wgsl"),
        include_str!("../shaders/corona.wgsl")
    );
    /// Coronas reading a `texture_depth_2d` with `textureLoad` ([`CoronaDepth::Loaded`]).
    ///
    /// [`CoronaDepth::Loaded`]: crate::CoronaDepth::Loaded
    pub const CORONA_LOAD: &str = concat!(
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/corona_load.wgsl"),
        include_str!("../shaders/corona.wgsl")
    );
    pub const ALL: [(&str, &str); 6] = [
        ("shadow", SHADOW),
        ("forward", FORWARD),
        ("post", POST),
        ("hud", HUD),
        ("corona", CORONA),
        ("corona_load", CORONA_LOAD),
    ];
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    /// Model-space bounds, for culling.
    bounds: Aabb,
}

/// Size-dependent render targets. `hdr` and `post` exist only in float mode.
struct Targets {
    /// Size of `hdr` and `depth`: the swapchain's, scaled in float mode.
    size: (u32, u32),
    hdr: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    /// Reads `depth` for the corona pass's occlusion test.
    corona_group: wgpu::BindGroup,
    post: Option<PostTargets>,
}

/// The bloom chain and the bind groups that read it.
struct PostTargets {
    /// Level `k` is `size >> (k + 1)` (at least 1 px).
    bloom: [wgpu::TextureView; BLOOM_LEVELS],
    /// Samples the HDR target: the prefilter's source.
    hdr_group: wgpu::BindGroup,
    /// `level_groups[k]` samples bloom level `k`.
    level_groups: [wgpu::BindGroup; BLOOM_LEVELS],
    tonemap_group: wgpu::BindGroup,
}

/// Size-independent state of the bloom and tonemap passes (float mode only).
struct Post {
    bloom_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_format: wgpu::TextureFormat,
    size: (u32, u32),
    hdr: bool,
    cascade_splits: [f32; CASCADES],
    cascade_size: u32,
    /// Largest cascade edge the device allows (a third of its widest texture).
    max_cascade_size: u32,
    quality: RenderQuality,
    culling: bool,
    /// The shadow atlas holds every cascade drawn as last fitted (false after a rebuild).
    shadow_valid: bool,
    /// The last world frame held a cascade over without checking its casters (the far
    /// one between its frames): until one checks it, every frame draws the world.
    shadows_held: bool,
    /// Each cascade as last drawn: its fit, the sun direction and its casters' hash.
    kept: [Option<KeptCascade>; CASCADES],

    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    cascade_buffer: wgpu::Buffer,
    cascade_groups: [wgpu::BindGroup; CASCADES],
    shadow_view: wgpu::TextureView,
    forward_group: wgpu::BindGroup,
    /// What the forward bind group is rebuilt from when the shadow atlas is.
    forward_layout: wgpu::BindGroupLayout,
    shadow_sampler: wgpu::Sampler,
    atlas_view: wgpu::TextureView,
    atlas_sampler: wgpu::Sampler,
    post: Option<Post>,
    targets: Targets,

    shadow_pipeline: wgpu::RenderPipeline,
    /// Clears one cascade's viewport (frames that keep another cascade).
    shadow_clear_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    /// [`MeshInstance::glass`]: blended, no depth writes.
    glass_pipeline: wgpu::RenderPipeline,
    /// The depth pre-pass, and the mesh pipeline that follows it (less-or-equal).
    depth_pipeline: wgpu::RenderPipeline,
    mesh_after_depth_pipeline: wgpu::RenderPipeline,
    hud_pipeline: wgpu::RenderPipeline,
    hud_layout: wgpu::BindGroupLayout,
    hud_group: wgpu::BindGroup,
    font_view: wgpu::TextureView,
    font_sampler: wgpu::Sampler,
    map_sampler: wgpu::Sampler,
    /// The map and the stills the HUD group holds, kept to rebuild it when one changes.
    map_view: wgpu::TextureView,
    stills_view: wgpu::TextureView,
    /// Texels of the uploaded map (1×1 transparent until [`Renderer::upload_hud_image`]).
    map_size: [u32; 2],
    /// Texels of the uploaded stills (1×1 transparent until [`Renderer::upload_hud_image`]).
    stills_size: [u32; 2],
    /// Room for [`MAX_HUD_QUADS`] quads, allocated once.
    hud_buffer: wgpu::Buffer,
    hud_staging: Vec<HudQuad>,
    /// The HUD quads on screen, and whether they still are (false after a new map).
    hud_drawn: Vec<HudQuad>,
    hud_valid: bool,
    /// What the last world frame drew from.
    kept_world: KeptWorld,
    corona_pipeline: wgpu::RenderPipeline,
    /// The corona pipeline's start-up check: coronas are drawn once it has passed.
    corona_probe: ProbeState,
    /// `Sampled` or `Loaded`, as resolved from the options.
    corona_depth: CoronaDepth,
    corona_layout: wgpu::BindGroupLayout,
    depth_sampler: wgpu::Sampler,
    /// Room for [`MAX_CORONAS`] sprites, allocated once.
    corona_buffer: wgpu::Buffer,
    corona_staging: Vec<CoronaRaw>,

    meshes: Vec<GpuMesh>,
    chunk_pool: ChunkPool,
    chunk_vertices: wgpu::Buffer,
    chunk_indices: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    /// Every pass's instances packed one pass after another ([`PASSES`] × capacity).
    instance_staging: Vec<InstanceRaw>,
    /// Per scene instance: bit `p` set when pass `p` draws it (culling scratch).
    vis: Vec<u8>,
    /// Per scene instance: its packed form, when any pass draws it.
    raws: Vec<InstanceRaw>,
    /// Scene instance indices in `instance_staging` order.
    order: Vec<u32>,
    timer: Option<GpuTimer>,
    stats: RenderStats,
}

impl Renderer {
    /// The corona pass's start-up check ([`Probe`]); coronas are drawn once it passed.
    pub fn corona_probe(&mut self) -> Probe<'_> {
        self.corona_probe.poll()
    }

    /// How the corona pass reads depth on this device: `Sampled` or `Loaded`.
    pub fn corona_depth(&self) -> CoronaDepth {
        self.corona_depth
    }

    /// Starts reading GPU time per pass into [`RenderStats::gpu_us`] (and per part with
    /// [`wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES`]) and fragment counts into
    /// [`RenderStats::gpu_counts`], when the device was created with
    /// [`wgpu::Features::TIMESTAMP_QUERY`]; false (and nothing changes) otherwise. For
    /// stats runs: it allocates a little every frame.
    pub fn enable_gpu_timing(&mut self) -> bool {
        if self.timer.is_none() {
            self.timer = GpuTimer::new(&self.device, &self.queue);
        }
        self.stats.gpu_timing = self.timer.is_some();
        self.stats.gpu_parts = self.timer.as_ref().is_some_and(|t| t.inside);
        self.timer.is_some()
    }

    /// The quality in use (as sanitized for this device).
    pub fn quality(&self) -> RenderQuality {
        self.quality
    }

    /// Switches to `quality` (also done by [`Renderer::render`] from
    /// [`SceneView::quality`]). Rebuilds the shadow atlas when the cascade size changes and
    /// the scene's targets when the resolution scale does; otherwise free.
    pub fn set_quality(&mut self, quality: RenderQuality) {
        let q = quality.sanitized(self.max_cascade_size);
        if q == self.quality {
            return;
        }
        let old = std::mem::replace(&mut self.quality, q);
        if q.shadow_size != old.shadow_size {
            self.cascade_size = q.shadow_size;
            let (view, group) = create_shadow_atlas(
                &self.device,
                self.cascade_size,
                &self.forward_layout,
                &self.shadow_sampler,
                &self.atlas_view,
                &self.atlas_sampler,
            );
            self.shadow_view = view;
            self.forward_group = group;
            self.stats.rebuilds += 1;
        }
        if (q.shadow_size, q.cascades, q.far_cascade_interval)
            != (old.shadow_size, old.cascades, old.far_cascade_interval)
        {
            self.shadow_valid = false;
            self.kept = [None; CASCADES];
        }
        if q.resolution_scale != old.resolution_scale && self.hdr {
            self.rebuild_targets();
            self.stats.rebuilds += 1;
        }
    }

    /// Turns instance culling on or off (off draws every instance in every pass).
    pub fn set_culling(&mut self, on: bool) {
        self.culling = on;
        self.kept_world.head = None;
    }

    /// Size of the scene's color and depth targets, pixels.
    pub fn render_size(&self) -> (u32, u32) {
        self.targets.size
    }

    fn rebuild_targets(&mut self) {
        self.kept_world.head = None;
        let size = if self.hdr {
            self.quality.render_size(self.size)
        } else {
            self.size
        };
        self.targets = create_targets(
            &self.device,
            self.post.as_ref(),
            &self.corona_layout,
            &self.depth_sampler,
            size,
        );
    }

    /// True when lighting goes through an `Rgba16Float` target and a tonemap pass; false
    /// when the forward pass tonemaps straight into the swapchain.
    pub fn is_hdr(&self) -> bool {
        self.hdr
    }

    /// Edge of one shadow cascade in texels (smaller than [`SHADOW_CASCADE_SIZE`] only on
    /// devices whose widest texture is under 3× that).
    pub fn shadow_cascade_size(&self) -> u32 {
        self.cascade_size
    }

    /// Far view depth of each shadow cascade, meters.
    pub fn cascade_splits(&self) -> [f32; CASCADES] {
        self.cascade_splits
    }

    /// Changes the cascade far depths (meters, increasing; each clamped to the camera's far
    /// plane every frame).
    pub fn set_cascade_splits(&mut self, splits: [f32; CASCADES]) {
        self.cascade_splits = splits;
        self.kept_world.head = None;
    }

    /// Counters for the last frame.
    pub fn stats(&self) -> RenderStats {
        RenderStats {
            chunk_pool: self.chunk_pool.stats(),
            ..self.stats
        }
    }

    /// Grows the instance buffer now so frames drawing up to `count` instances never
    /// allocate. Call during loading.
    pub fn reserve_instances(&mut self, count: usize) {
        if count > self.instance_capacity {
            self.instance_capacity = count.next_power_of_two();
            let packed = PASSES * self.instance_capacity;
            self.instance_buffer = create_instance_buffer(&self.device, packed);
            self.instance_staging
                .reserve(packed - self.instance_staging.len());
            self.order.reserve(packed - self.order.len());
            self.vis.reserve(self.instance_capacity - self.vis.len());
            self.raws.reserve(self.instance_capacity - self.raws.len());
            let kept = &mut self.kept_world.instances;
            kept.reserve(self.instance_capacity - kept.len());
        }
    }

    /// Recreates size-dependent targets. Call when the swapchain is reconfigured.
    pub fn resize(&mut self, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if size != self.size {
            self.size = size;
            self.rebuild_targets();
        }
    }

    /// Uploads the image `target`'s quads show: `width × height` RGBA8 texels, sRGB-encoded,
    /// row-major from the top-left, with straight alpha; for [`HudImageTarget::Stills`] the
    /// pictures side by side in one image. Mipmaps are built here. Call it once per target
    /// while loading (it creates a texture); a later call replaces the image. Sizes are
    /// clamped to 1..=[`MAX_MAP_SIZE`]; data of the wrong length uploads a transparent image.
    pub fn upload_hud_image(
        &mut self,
        target: HudImageTarget,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) {
        let (w, h) = (width.clamp(1, MAX_MAP_SIZE), height.clamp(1, MAX_MAP_SIZE));
        let empty = [0; 4];
        let (w, h, rgba) = if (w, h) == (width, height) && rgba.len() == (w * h * 4) as usize {
            (w, h, rgba)
        } else {
            (1, 1, &empty[..])
        };
        let view = upload_map_texture(&self.device, &self.queue, w, h, rgba);
        match target {
            HudImageTarget::Map => (self.map_view, self.map_size) = (view, [w, h]),
            HudImageTarget::Stills => (self.stills_view, self.stills_size) = (view, [w, h]),
        }
        self.rebuild_hud_group();
    }

    /// Size in texels of the stills [`Renderer::upload_hud_image`] last uploaded (1×1 before).
    pub fn stills_size(&self) -> [u32; 2] {
        self.stills_size
    }

    fn rebuild_hud_group(&mut self) {
        self.hud_group = hud_group(
            &self.device,
            &self.hud_layout,
            [&self.font_view, &self.map_view, &self.stills_view],
            [&self.font_sampler, &self.map_sampler],
        );
        // The HUD's pixels change with its textures.
        self.hud_valid = false;
    }

    /// Size in texels of the map [`Renderer::upload_hud_image`] last uploaded (1×1 before).
    pub fn map_size(&self) -> [u32; 2] {
        self.map_size
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
        self.kept_world.head = None;
        self.meshes.push(GpuMesh {
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
            bounds: Aabb::of_points(mesh.vertices.iter().map(|v| Vec3::from(v.pos))),
        });
        MeshId(self.meshes.len() as u32 - 1)
    }

    /// The chunk pool's counters.
    pub fn chunk_pool_stats(&self) -> PoolStats {
        self.chunk_pool.stats()
    }
}

/// The name of a graphics backend as the platform layers and overlays show it.
pub fn backend_label(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        wgpu::Backend::Gl if cfg!(target_arch = "wasm32") => "WebGL2",
        wgpu::Backend::Gl => "GL",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Dx12 => "DX12",
        wgpu::Backend::Noop => "noop",
    }
}

/// Whether an adapter reporting `features` for [`HDR_TEXTURE_FORMAT`] can hold the lit
/// image: the float path renders to it, blends into it (coronas, bloom) and filters it
/// (bloom). Where this is false, pass [`HdrMode::Ldr`].
pub fn hdr_format_supported(features: wgpu::TextureFormatFeatures) -> bool {
    features
        .allowed_usages
        .contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING)
        && features.flags.contains(
            wgpu::TextureFormatFeatureFlags::BLENDABLE
                | wgpu::TextureFormatFeatureFlags::FILTERABLE,
        )
}

/// The texture format of the lit image in float mode, for [`hdr_format_supported`].
pub const HDR_TEXTURE_FORMAT: wgpu::TextureFormat = HDR_FORMAT;

/// The pending answer of an error scope pushed round an optional pipeline's creation.
#[cfg(target_arch = "wasm32")]
type ScopeFuture = std::pin::Pin<Box<dyn Future<Output = Option<wgpu::Error>>>>;
#[cfg(not(target_arch = "wasm32"))]
type ScopeFuture = std::pin::Pin<Box<dyn Future<Output = Option<wgpu::Error>> + Send>>;

/// The start-up check of an optional pipeline: the error scope pushed round its creation,
/// polled (with a no-op waker, no allocation) until it answers. The GL, native and no-op
/// backends answer at once; the browser's WebGPU a frame or two later.
enum ProbeState {
    Pending(ScopeFuture),
    Passed,
    Failed(String),
}

impl ProbeState {
    fn new(scope: wgpu::ErrorScopeGuard) -> ProbeState {
        let mut state = ProbeState::Pending(Box::pin(scope.pop()));
        state.poll();
        state
    }

    fn poll(&mut self) -> Probe<'_> {
        if let ProbeState::Pending(future) = self {
            match future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                Poll::Ready(None) => *self = ProbeState::Passed,
                Poll::Ready(Some(error)) => *self = ProbeState::Failed(error.to_string()),
                Poll::Pending => {}
            }
        }
        match self {
            ProbeState::Pending(_) => Probe::Pending,
            ProbeState::Passed => Probe::Passed,
            ProbeState::Failed(error) => Probe::Failed(error),
        }
    }
}

/// Chunk slots live in the buffers made at load: writing one is a queue copy, never a new
/// buffer.
impl ChunkStore for Renderer {
    fn alloc_chunk_slot(&mut self) -> Result<SlotId, PoolError> {
        self.chunk_pool.alloc()
    }

    fn write_chunk(
        &mut self,
        slot: SlotId,
        mesh: &Mesh,
        parts: &[[u32; 2]],
    ) -> Result<(), PoolError> {
        let w = self.chunk_pool.stage_write(slot, mesh, parts)?;
        self.kept_world.head = None;
        if !mesh.vertices.is_empty() {
            self.queue.write_buffer(
                &self.chunk_vertices,
                w.vertex_offset,
                bytemuck::cast_slice(&mesh.vertices),
            );
        }
        if !w.indices.is_empty() {
            self.queue.write_buffer(
                &self.chunk_indices,
                w.index_offset,
                bytemuck::cast_slice(w.indices),
            );
        }
        Ok(())
    }

    fn free_chunk_slot(&mut self, slot: SlotId) -> Result<(), PoolError> {
        self.kept_world.head = None;
        self.chunk_pool.free(slot)
    }
}

#[cfg(test)]
mod tests;
