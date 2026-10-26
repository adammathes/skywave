//! Skywave's renderer.
//!
//! Three passes per frame, all within WebGL2 limits (uniform buffers only, no compute, no
//! storage buffers, no texture arrays):
//!
//! 1. **Shadow**: depth-only renders of every instance from the sun into three cascades
//!    (0–40 m, 40–160 m, 160–600 m by default) side by side in one 3×1 depth atlas, each an
//!    orthographic projection fitted to a slice of the camera frustum and snapped to texels.
//! 2. **Forward**: the analytic sky (Preetham, sun and moon discs, stars), then meshes:
//!    vertex color × atlas texel, Lambert sun with 3×3 PCF cascaded shadows, hemisphere sky
//!    ambient, the point and spot lights of each instance's [`LightSet`] (up to
//!    [`LIGHTS_PER_DRAW`] of the frame's [`MAX_LIGHTS`]), the vertex emissive channel and
//!    aerial-perspective fog, into an `Rgba16Float` HDR target.
//! 3. **Coronas**: an additive soft sprite over each light with a [`Corona`] and over the
//!    sun and moon discs, faded by how much of it the scene's depth hides (see
//!    `shaders/corona.wgsl`), in one instanced draw. Skipped when there are none.
//! 4. **Bloom**: a soft-threshold, Karis-averaged 13-tap downsample into a five-level
//!    half-resolution `Rgba16Float` chain, then 9-tap tent upsamples added back up it
//!    (Jimenez 2014). Skipped when [`Sky::bloom_strength`] is 0.
//! 5. **Tonemap**: exposure, bloom, AgX, a light vignette and an 8-bit dither from the HDR
//!    target to the swapchain.
//! 6. **HUD**: text and rectangles from [`SceneView::hud`] as alpha-blended quads straight
//!    onto the swapchain, in one instanced draw (see [`HudLayout`] for the layout rules
//!    and [`font`] for the bitmap face). Skipped when there is nothing to draw.
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
//! The renderer knows nothing about the game; it draws a [`SceneView`] of plain data.
//! Radiance is linear sRGB in kcd/m² (1 = 1000 cd/m²) throughout; see [`Sun`] and [`Sky`].

mod atlas;
pub mod font;
mod hud;
mod light;
mod mesh;
mod sky;

pub use atlas::{
    tile_uv, uv_unorm, TextureAtlas, Tile, ATLAS_MIP_LEVELS, ATLAS_SIZE, TILES_PER_ROW, TILE_INSET,
    TILE_SIZE,
};
pub use hud::{
    clamp_ui_scale, Hud, HudBuilder, HudItem, HudLayout, TextRange, HUD_HEIGHT, HUD_SAFE_MARGIN,
    HUD_WIDTH, MAX_HUD_QUADS, UI_SCALE_MAX, UI_SCALE_MIN,
};
pub use light::{
    camera_depth_range, fit_cascades, slice_corners, sun_view_proj, CascadeFit, CASCADES,
};
pub use mesh::{
    emissive, emissive_scale, emissive_window, Mesh, Vertex, EMISSIVE_MAX_SCALE,
    EMISSIVE_WINDOW_BIT,
};
pub use sky::{exposure_from_ev100, MOON_ANGULAR_RADIUS, SUN_ANGULAR_RADIUS};

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3, Vec4Swizzles};
use hud::HudQuad;
use sky::SkyModel;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use wgpu::util::DeviceExt;

/// Edge of one shadow cascade in texels, when the device allows a texture 3× as wide.
pub const SHADOW_CASCADE_SIZE: u32 = 2048;
/// Default far view depth of each cascade, meters.
pub const DEFAULT_CASCADE_SPLITS: [f32; CASCADES] = [40.0, 160.0, 600.0];
/// Fraction of each cascade over which it cross-fades into the next.
pub const CASCADE_BLEND: f32 = 0.1;
/// Casters this far beyond a cascade toward the sun still cast into it, meters.
const CASTER_MARGIN: f32 = 400.0;
/// Shadow-receiver offsets, in texels of the receiving cascade.
const SHADOW_NORMAL_OFFSET: f32 = 1.5;
const SHADOW_SUN_OFFSET: f32 = 1.0;
/// Frames after which `render` must not create GPU resources (debug-asserted).
pub const WARMUP_FRAMES: u64 = 4;
/// Lights per frame; [`SceneView::lights`] beyond this are ignored. 32 bytes each in the
/// per-frame uniform block.
pub const MAX_LIGHTS: usize = 256;
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
/// Stride of the per-cascade uniform slots; the largest offset alignment WebGPU allows.
const CASCADE_UNIFORM_STRIDE: u64 = 256;

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
        }
    }
}

/// Up to [`LIGHTS_PER_DRAW`] indices into [`SceneView::lights`]: the lights an instance is
/// shaded by. Indices past the end of the frame's lights are ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightSet {
    len: u8,
    indices: [u8; LIGHTS_PER_DRAW],
}

impl LightSet {
    pub const EMPTY: LightSet = LightSet {
        len: 0,
        indices: [0; LIGHTS_PER_DRAW],
    };

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

/// A soft additive glow over a light, drawn after the forward pass (the Vice City corona).
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

/// Construction options for [`Renderer::with_options`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RendererOptions {
    pub hdr: HdrMode,
    /// Far view depth of each shadow cascade, meters, increasing.
    pub cascade_splits: [f32; CASCADES],
}

impl Default for RendererOptions {
    fn default() -> RendererOptions {
        RendererOptions {
            hdr: HdrMode::Auto,
            cascade_splits: DEFAULT_CASCADE_SPLITS,
        }
    }
}

/// Counters for the last rendered frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Frames rendered so far.
    pub frames: u64,
    /// Draw calls in the last frame, all passes.
    pub draw_calls: u32,
    /// Of which in the shadow pass.
    pub shadow_draw_calls: u32,
    /// Of which in the bloom passes (0 with bloom off or in LDR mode).
    pub bloom_draw_calls: u32,
    /// Lights in the last frame's scene (at most [`MAX_LIGHTS`]).
    pub lights_submitted: u32,
    /// Of which in at least one instance's [`LightSet`].
    pub lights_used: u32,
    /// Corona sprites submitted (before the depth test hides any).
    pub coronas: u32,
    /// Mesh instances in the last frame's scene.
    pub instances: u32,
    /// HUD quads (glyphs and rectangles) drawn in the last frame.
    pub hud_quads: u32,
    /// HUD quads beyond [`MAX_HUD_QUADS`] left out of the last frame.
    pub hud_quads_dropped: u32,
    /// GPU resources `render` has created over the renderer's life (instance buffer growth).
    pub resources_created: u32,
}

/// GPU layout of the per-frame uniform block; mirrors `Frame` in shaders/common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
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
    /// x glow (emissive multiplier) as f32 bits, y cabin-light share as unorm16 | the light
    /// set's length << 16, zw the set's light indices, a byte each, low byte first.
    params: [u32; 4],
}

impl InstanceRaw {
    /// `inst` for a frame with `lights` lights (indices at or past it are left out).
    fn new(inst: &MeshInstance, lights: usize) -> InstanceRaw {
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
            params: instance_params(inst, lights),
        }
    }
}

/// The packed `params` of [`InstanceRaw`].
fn instance_params(inst: &MeshInstance, lights: usize) -> [u32; 4] {
    let mut words = [0u32; 2];
    let mut n = 0;
    for &i in inst.lights.indices() {
        if usize::from(i) < lights {
            words[n / 4] |= u32::from(i) << (8 * (n % 4));
            n += 1;
        }
    }
    let cabin = (inst.cabin.clamp(0.0, 1.0) * 65535.0).round() as u32;
    [
        inst.glow.max(0.0).to_bits(),
        cabin | (n as u32) << 16,
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
const HUD_ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Unorm8x4,
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
    pub const CORONA: &str = concat!(
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/corona.wgsl")
    );
    pub const ALL: [(&str, &str); 5] = [
        ("shadow", SHADOW),
        ("forward", FORWARD),
        ("post", POST),
        ("hud", HUD),
        ("corona", CORONA),
    ];
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// Size-dependent render targets. `hdr` and `post` exist only in float mode.
struct Targets {
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

    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    cascade_buffer: wgpu::Buffer,
    cascade_groups: [wgpu::BindGroup; CASCADES],
    shadow_view: wgpu::TextureView,
    forward_group: wgpu::BindGroup,
    post: Option<Post>,
    targets: Targets,

    shadow_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    hud_pipeline: wgpu::RenderPipeline,
    hud_group: wgpu::BindGroup,
    /// Room for [`MAX_HUD_QUADS`] quads, allocated once.
    hud_buffer: wgpu::Buffer,
    hud_staging: Vec<HudQuad>,
    corona_pipeline: wgpu::RenderPipeline,
    corona_layout: wgpu::BindGroupLayout,
    depth_sampler: wgpu::Sampler,
    /// Room for [`MAX_CORONAS`] sprites, allocated once.
    corona_buffer: wgpu::Buffer,
    corona_staging: Vec<CoronaRaw>,

    meshes: Vec<GpuMesh>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    instance_staging: Vec<InstanceRaw>,
    stats: RenderStats,
}

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
        let cascade_size = SHADOW_CASCADE_SIZE.min(device.limits().max_texture_dimension_2d / 3);

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

        let shadow_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("sun shadow cascades"),
                size: wgpu::Extent3d {
                    width: cascade_size * CASCADES as u32,
                    height: cascade_size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SHADOW_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
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
        let forward_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forward"),
            layout: &forward_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });
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
                entry_point: Some(fs_sky),
                compilation_options: Default::default(),
                targets: &color_target,
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
                entry_point: Some(fs_mesh),
                compilation_options: Default::default(),
                targets: &color_target,
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
                        texture_entry(2, wgpu::TextureSampleType::Float { filterable: false }),
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
        let hud_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud"),
            entries: &[
                texture_entry(0, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(1, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let hud_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud"),
            layout: &hud_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&font_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&font_sampler),
                },
            ],
        });
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

        // Coronas read the scene's depth as a plain float texture in the vertex stage,
        // through a nearest sampler without comparison (GL counts a float depth texture
        // sampled any other way as incomplete and reads zeros), and add onto the lit image.
        let corona_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("corona"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
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
        let corona_module = module("corona", shaders::CORONA);
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
        let corona_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("coronas"),
            size: (MAX_CORONAS * size_of::<CoronaRaw>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let instance_capacity = 256;
        let instance_buffer = create_instance_buffer(&device, instance_capacity);
        let targets = create_targets(&device, post.as_ref(), &corona_layout, &depth_sampler, size);

        Renderer {
            device,
            queue,
            surface_format,
            size,
            hdr,
            cascade_splits: options.cascade_splits,
            cascade_size,
            frame_buffer,
            frame_group,
            cascade_buffer,
            cascade_groups,
            shadow_view,
            forward_group,
            post,
            targets,
            shadow_pipeline,
            sky_pipeline,
            mesh_pipeline,
            hud_pipeline,
            hud_group,
            hud_buffer,
            hud_staging: Vec::with_capacity(MAX_HUD_QUADS),
            corona_pipeline,
            corona_layout,
            depth_sampler,
            corona_buffer,
            corona_staging: Vec::with_capacity(MAX_CORONAS),
            meshes: Vec::new(),
            instance_buffer,
            instance_capacity,
            instance_staging: Vec::with_capacity(instance_capacity),
            stats: RenderStats::default(),
        }
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
    }

    /// Counters for the last frame.
    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    /// Grows the instance buffer now so frames drawing up to `count` instances never
    /// allocate. Call during loading.
    pub fn reserve_instances(&mut self, count: usize) {
        if count > self.instance_capacity {
            self.instance_capacity = count.next_power_of_two();
            self.instance_buffer = create_instance_buffer(&self.device, self.instance_capacity);
            self.instance_staging
                .reserve(self.instance_capacity - self.instance_staging.len());
        }
    }

    /// Recreates size-dependent targets. Call when the swapchain is reconfigured.
    pub fn resize(&mut self, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if size != self.size {
            self.size = size;
            self.targets = create_targets(
                &self.device,
                self.post.as_ref(),
                &self.corona_layout,
                &self.depth_sampler,
                size,
            );
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
    ///
    /// Allocates nothing on the heap or the GPU unless the instance count exceeds every
    /// earlier frame's (and [`Renderer::reserve_instances`]); in debug builds that is an
    /// assertion failure after [`WARMUP_FRAMES`].
    pub fn render(&mut self, view: &wgpu::TextureView, scene: &SceneView) {
        let (uniform, cascades) = self.frame_uniforms(scene);
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        self.queue
            .write_buffer(&self.cascade_buffer, 0, bytemuck::bytes_of(&cascades));

        if scene.instances.len() > self.instance_capacity {
            debug_assert!(
                self.stats.frames < WARMUP_FRAMES,
                "instance buffer grew after warmup ({} instances): call Renderer::reserve_instances while loading",
                scene.instances.len()
            );
            self.reserve_instances(scene.instances.len());
            self.stats.resources_created += 1;
        }
        let lights = scene.lights.len().min(MAX_LIGHTS);
        self.instance_staging.clear();
        self.instance_staging.extend(
            scene
                .instances
                .iter()
                .map(|inst| InstanceRaw::new(inst, lights)),
        );
        // Which lights any instance uses, for the stats.
        let mut used = [0u64; MAX_LIGHTS / 64];
        for inst in scene.instances {
            for &i in inst.lights.indices() {
                if usize::from(i) < lights {
                    used[usize::from(i) / 64] |= 1 << (i % 64);
                }
            }
        }
        self.corona_staging.clear();
        self.push_coronas(scene);
        let coronas = self.corona_staging.len() as u32;
        if coronas > 0 {
            self.queue.write_buffer(
                &self.corona_buffer,
                0,
                bytemuck::cast_slice(&self.corona_staging),
            );
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
        let mut shadow_draws = 0;
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
            let res = self.cascade_size;
            for (i, group) in self.cascade_groups.iter().enumerate() {
                let x = i as u32 * res;
                pass.set_viewport(x as f32, 0.0, res as f32, res as f32, 0.0, 1.0);
                // GL does not clip to the viewport; the scissor keeps cascades apart.
                pass.set_scissor_rect(x, 0, res, res);
                pass.set_bind_group(1, group, &[]);
                shadow_draws += self.draw_instances(&mut pass, scene.instances);
            }
        }
        let mut forward_draws = 1;
        {
            let color = self.targets.hdr.as_ref().unwrap_or(view);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forward"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
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
                        // Kept for the corona pass's occlusion test.
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &self.forward_group, &[]);
            pass.set_pipeline(&self.sky_pipeline);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.mesh_pipeline);
            forward_draws += self.draw_instances(&mut pass, scene.instances);
        }
        if coronas > 0 {
            let color = self.targets.hdr.as_ref().unwrap_or(view);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("coronas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.corona_pipeline);
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &self.targets.corona_group, &[]);
            pass.set_vertex_buffer(0, self.corona_buffer.slice(..));
            pass.draw(0..4, 0..coronas);
            forward_draws += 1;
        }
        let mut bloom_draws = 0;
        if let (Some(post), Some(targets)) = (&self.post, &self.targets.post) {
            if scene.sky.bloom_strength > 0.0 {
                // Down the chain: the prefilter reads the HDR target, each later level the
                // one before it.
                for (k, level) in targets.bloom.iter().enumerate() {
                    let (pipeline, group) = match k {
                        0 => (&post.prefilter, &targets.hdr_group),
                        _ => (&post.down, &targets.level_groups[k - 1]),
                    };
                    let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
                    fullscreen_pass(
                        &mut encoder,
                        level,
                        clear,
                        pipeline,
                        [&self.frame_group, group],
                    );
                    bloom_draws += 1;
                }
                // Back up: each level adds a tent-filtered copy of the (already summed)
                // level below it.
                for k in (0..BLOOM_LEVELS - 1).rev() {
                    let groups = [&self.frame_group, &targets.level_groups[k + 1]];
                    let level = &targets.bloom[k];
                    fullscreen_pass(&mut encoder, level, wgpu::LoadOp::Load, &post.up, groups);
                    bloom_draws += 1;
                }
            }
            let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
            let groups = [&self.frame_group, &targets.tonemap_group];
            fullscreen_pass(&mut encoder, view, clear, &post.tonemap, groups);
            forward_draws += 1;
        }
        self.hud_staging.clear();
        let hud_dropped =
            hud::build_quads(&scene.hud, self.size, &mut self.hud_staging, MAX_HUD_QUADS);
        let hud_quads = self.hud_staging.len() as u32;
        if hud_quads > 0 {
            self.queue
                .write_buffer(&self.hud_buffer, 0, bytemuck::cast_slice(&self.hud_staging));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hud"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.hud_pipeline);
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &self.hud_group, &[]);
            pass.set_vertex_buffer(0, self.hud_buffer.slice(..));
            pass.draw(0..4, 0..hud_quads);
            forward_draws += 1;
        }
        self.queue.submit([encoder.finish()]);
        self.stats.frames += 1;
        self.stats.shadow_draw_calls = shadow_draws;
        self.stats.bloom_draw_calls = bloom_draws;
        self.stats.draw_calls = shadow_draws + forward_draws + bloom_draws;
        self.stats.lights_submitted = lights as u32;
        self.stats.lights_used = used.iter().map(|w| w.count_ones()).sum();
        self.stats.coronas = coronas;
        self.stats.instances = scene.instances.len() as u32;
        self.stats.hud_quads = hud_quads;
        self.stats.hud_quads_dropped = hud_dropped;
    }

    /// Fills `corona_staging` with the frame's coronas: one per light with a [`Corona`],
    /// then the sun and the moon, each as bright as its light against the exposure (see
    /// [`CORONA_PEAK`]); the invisible ones are left out.
    fn push_coronas(&mut self, scene: &SceneView) {
        let sky = &scene.sky;
        let strength = sky.corona_strength.clamp(0.0, 1.0);
        if strength <= 0.0 {
            return;
        }
        let exposure = sky.exposure.max(1e-6);
        let luma = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        // Radiance whose brightest channel lands at the knee of `x` after exposure, in the
        // hue of `color`, or none when that is too dim to see.
        let peak = |color: Vec3, x: f32, k: f32| {
            let e = CORONA_PEAK * (x / (1.0 + x)) * k * strength;
            let max = color.max_element();
            (e >= CORONA_MIN_PEAK && max > 0.0).then(|| color * (e / (max * exposure)))
        };
        for light in &scene.lights[..scene.lights.len().min(MAX_LIGHTS)] {
            let c = light.corona;
            if c.radius <= 0.0 || c.strength <= 0.0 {
                continue;
            }
            let color = Vec3::from(light.color).max(Vec3::ZERO);
            let x = luma(color) * exposure * CORONA_GAIN;
            if let Some(rgb) = peak(color, x, c.strength.min(1.0)) {
                let [px, py, pz] = light.position;
                self.corona_staging.push(CoronaRaw {
                    pos_size: [px, py, pz, c.radius],
                    color: rgb.extend(0.0).to_array(),
                });
            }
        }
        // The sun and moon, while above the horizon.
        let sun_dir = Vec3::from(scene.sun.direction).normalize_or(Vec3::Y);
        let moon_dir = Vec3::from(sky.moon_direction).normalize_or(Vec3::NEG_Y);
        let above = |d: Vec3| smoothstep(-0.01, 0.02, d.y);
        let sun = Vec3::from(scene.sun.illuminance).max(Vec3::ZERO);
        let x = luma(sun) * exposure * SUN_CORONA_GAIN;
        if let Some(rgb) = peak(sun, x, SUN_CORONA_STRENGTH * above(sun_dir)) {
            self.corona_staging.push(CoronaRaw {
                pos_size: sun_dir.extend(-SUN_CORONA_RADIUS).to_array(),
                color: rgb.extend(0.0).to_array(),
            });
        }
        // The lit fraction of the disc follows from the angle between the sun and moon.
        let lit = 0.5 * (1.0 - sun_dir.dot(moon_dir));
        let x = sky.moon_luminance.max(0.0) * lit * exposure * MOON_CORONA_GAIN;
        let moonlight = Vec3::new(1.0, 0.96, 0.9);
        if let Some(rgb) = peak(moonlight, x, MOON_CORONA_STRENGTH * above(moon_dir)) {
            self.corona_staging.push(CoronaRaw {
                pos_size: moon_dir.extend(-MOON_CORONA_RADIUS).to_array(),
                color: rgb.extend(0.0).to_array(),
            });
        }
    }

    /// Issues one instanced draw per run of consecutive instances sharing a mesh and
    /// returns the number of draws.
    fn draw_instances(&self, pass: &mut wgpu::RenderPass<'_>, instances: &[MeshInstance]) -> u32 {
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        let mut draws = 0;
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
                draws += 1;
            }
            start = end;
        }
        draws
    }

    fn frame_uniforms(&self, scene: &SceneView) -> (FrameUniform, [CascadeUniform; CASCADES]) {
        let view = Mat4::from_cols_array_2d(&scene.camera.view);
        let proj = Mat4::from_cols_array_2d(&scene.camera.proj);
        let view_proj = proj * view;
        let inv_view = view.inverse();
        let eye = inv_view.w_axis.xyz();
        let forward = -inv_view.z_axis.xyz().normalize_or(Vec3::Z);
        let sky = SkyModel::new(&scene.sun, &scene.sky);
        let fits = fit_cascades(
            view,
            proj,
            sky.sun_dir,
            self.cascade_splits,
            CASCADE_BLEND,
            self.cascade_size,
            CASTER_MARGIN,
        );
        let cascades = fits.map(|f| CascadeUniform {
            view_proj: f.view_proj.to_cols_array_2d(),
            _pad: [[0.0; 4]; 12],
        });
        let v4 = |v: Vec3, w: f32| v.extend(w).to_array();
        let moon_dir = Vec3::from(scene.sky.moon_direction).normalize_or(Vec3::NEG_Y);
        let uniform = FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            cascade_view_proj: fits.map(|f| f.view_proj.to_cols_array_2d()),
            camera_pos: v4(eye, scene.sky.fog_density.max(0.0)),
            camera_fwd: v4(forward, 0.0),
            cascade_far: [
                fits[0].depth_range[1],
                fits[1].depth_range[1],
                fits[2].depth_range[1],
                CASCADE_BLEND,
            ],
            cascade_texel: [
                fits[0].texel,
                fits[1].texel,
                fits[2].texel,
                self.cascade_size as f32,
            ],
            sun_dir: v4(sky.sun_dir, 0.0),
            sun_light: v4(sky.sun_light, 0.0),
            sun_disc: v4(sky.sun_disc, SUN_ANGULAR_RADIUS.cos()),
            moon_dir: v4(moon_dir, MOON_ANGULAR_RADIUS.cos()),
            moon_light: [
                scene.sky.moon_luminance.max(0.0),
                0.01,
                scene.sky.star_brightness.clamp(0.0, 1.0),
                0.0,
            ],
            perez: sky.perez.map(|p| v4(p, 0.0)),
            zenith: v4(sky.zenith, 0.0),
            sun_sky: v4(sky.sun_sky, 0.0),
            sky_params: [sky.day, sky.blue, sky.haze, 0.0],
            night_sky: v4(sky.night, 0.0),
            sky_up: v4(sky.sky_up, 0.0),
            sky_ground: v4(sky.ground, 0.0),
            sky_horizon: v4(sky.horizon, 0.0),
            params: [
                scene.sky.exposure,
                if self.surface_format.is_srgb() {
                    0.0
                } else {
                    1.0
                },
                SHADOW_SUN_OFFSET,
                SHADOW_NORMAL_OFFSET,
            ],
            emissive: [
                scene.sky.emissive_boost.max(0.0),
                scene.sky.window_emissive_scale.clamp(0.0, 1.0),
                scene.lights.len().min(MAX_LIGHTS) as f32,
                scene.sky.emissive_cap.max(1e-3),
            ],
            post: [
                if self.hdr {
                    scene.sky.bloom_strength.max(0.0)
                } else {
                    0.0
                },
                scene.sky.bloom_threshold.max(0.0),
                VIGNETTE,
                BLOOM_CLAMP,
            ],
            screen: [
                self.size.0 as f32,
                self.size.1 as f32,
                1.0 / self.size.0 as f32,
                1.0 / self.size.1 as f32,
            ],
            cabin_light: v4(Vec3::from(scene.sky.cabin_light).max(Vec3::ZERO), 0.0),
            corona: [
                // Pixels per unit of view-space slope, vertically.
                0.5 * self.size.1 as f32 * proj.y_axis.y.abs(),
                CORONA_MIN_PX * self.size.1 as f32 / 720.0,
                CORONA_MAX_PX * self.size.1 as f32 / 720.0,
                0.0,
            ],
            lights: std::array::from_fn(|i| {
                scene
                    .lights
                    .get(i)
                    .map_or(PackedLight::zeroed(), PackedLight::new)
            }),
        };
        (uniform, cascades)
    }
}

/// Whether the device can render to `Rgba16Float` (on WebGL2 that needs
/// `EXT_color_buffer_float`). Probes by creating a tiny target inside an error scope; the
/// GL and native backends resolve the scope immediately. The browser's WebGPU resolves it
/// later, and always supports the format, so a pending answer counts as yes.
fn float16_renderable(device: &wgpu::Device) -> bool {
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

fn upload_atlas(
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
fn upload_font(
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

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: (capacity * size_of::<InstanceRaw>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// One fullscreen-triangle pass into `target` (no depth).
fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    pipeline: &wgpu::RenderPipeline,
    groups: [&wgpu::BindGroup; 2],
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fullscreen"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, groups[0], &[]);
    pass.set_bind_group(1, groups[1], &[]);
    pass.draw(0..3, 0..1);
}

/// Hermite step from 0 at `e0` to 1 at `e1`, as in WGSL.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Size of bloom level `k` for a `size` target: half, quarter, … of it, at least 1 px.
fn bloom_level_size(size: (u32, u32), k: usize) -> (u32, u32) {
    ((size.0 >> (k + 1)).max(1), (size.1 >> (k + 1)).max(1))
}

fn create_targets(
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
        hdr: Some(hdr),
        depth,
        corona_group,
        post: Some(post),
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
        let mut entry_points = Vec::new();
        for (name, source) in shaders::ALL {
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
            "vs_sky",
            "fs_sky",
            "fs_sky_ldr",
            "vs_mesh",
            "fs_mesh",
            "fs_mesh_ldr",
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

    #[test]
    fn uniform_and_instance_layouts() {
        // std140-compatible: only mat4 and vec4 members, 16-byte multiple, under WebGL2's
        // 16 KiB uniform block limit.
        assert_eq!(
            size_of::<FrameUniform>(),
            5 * 64 + 27 * 16 + MAX_LIGHTS * 32
        );
        assert_eq!(size_of::<PackedLight>(), 32);
        assert_eq!(size_of::<CoronaRaw>(), 32);
        assert_eq!(size_of::<FrameUniform>() % 16, 0);
        assert!(
            size_of::<FrameUniform>()
                <= wgpu::Limits::downlevel_webgl2_defaults().max_uniform_buffer_binding_size
                    as usize
        );
        assert_eq!(size_of::<CascadeUniform>() as u64, CASCADE_UNIFORM_STRIDE);
        assert_eq!(size_of::<InstanceRaw>(), 144);
        assert_eq!(INSTANCE_ATTRIBUTES[8].format, wgpu::VertexFormat::Uint32x4);
        assert_eq!(size_of::<Vertex>(), 36);
        assert_eq!(VERTEX_ATTRIBUTES[2].offset, 24);
        assert_eq!(VERTEX_ATTRIBUTES[3].offset, 28);
        assert_eq!(VERTEX_ATTRIBUTES[4].offset, 32);
        let attributes = VERTEX_ATTRIBUTES.len() + INSTANCE_ATTRIBUTES.len();
        assert!(
            attributes <= wgpu::Limits::downlevel_webgl2_defaults().max_vertex_attributes as usize
        );
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
        assert_eq!(f32::from_bits(p[0]), 0.75);
        assert_eq!(p[1] & 0xffff, 32768);
        assert_eq!(p[1] >> 16, 8);
        assert_eq!(p[2], 3 | 255 << 8 | 7 << 16);
        assert_eq!(p[3], 200 | 1 << 8 | 2 << 16 | 9 << 24);
        // With 10 lights in the frame, 255 and 200 are not there.
        let p = instance_params(&inst, 10);
        assert_eq!(p[1] >> 16, 6);
        assert_eq!(p[2], 3 | 7 << 8 | 1 << 24);
        assert_eq!(p[3], 2 | 9 << 8);
        inst.lights = LightSet::EMPTY;
        inst.glow = -1.0;
        inst.cabin = 2.0;
        assert_eq!(instance_params(&inst, 4), [0, 65535, 0, 0]);
        let mut set = LightSet::EMPTY;
        assert!(set.is_empty() && !set.push(MAX_LIGHTS) && set.push(MAX_LIGHTS - 1));
        assert_eq!(set.indices(), &[255]);
    }

    #[test]
    fn normal_matrix_handles_non_uniform_scale() {
        let m = Mat4::from_scale(Vec3::new(4.0, 1.0, 1.0)) * Mat4::from_rotation_z(0.5);
        let raw = InstanceRaw::new(&MeshInstance::new(MeshId(0), m.to_cols_array_2d()), 0);
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
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("noop adapter");
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        }))
        .expect("noop device")
    }

    fn test_scene<'a>(instances: &'a [MeshInstance]) -> SceneView<'a> {
        test_scene_lit(instances, &[])
    }

    fn test_scene_lit<'a>(
        instances: &'a [MeshInstance],
        lights: &'a [PointLight],
    ) -> SceneView<'a> {
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
                view_formats: &[],
            });
            r.render(&target.create_view(&Default::default()), scene);
        }
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
            for mode in [HdrMode::Float16, HdrMode::Ldr] {
                let options = RendererOptions {
                    hdr: mode,
                    ..Default::default()
                };
                let mut r = Renderer::with_options(&device, &queue, format, (64, 48), options);
                assert_eq!(r.is_hdr(), mode == HdrMode::Float16);
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
                let mut instances =
                    vec![MeshInstance::new(ground, Mat4::IDENTITY.to_cols_array_2d())];
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
                            Mat4::from_translation(Vec3::new(i as f32, 0.0, 0.0))
                                .to_cols_array_2d(),
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
                    let near: Vec<usize> =
                        (0..MAX_LIGHTS).filter(|k| k.abs_diff(i / 10) < 4).collect();
                    inst.lights = LightSet::from_indices(&near);
                }
                let mut scene = test_scene_lit(&instances, &lights);
                scene.sky.cabin_light = [0.01, 0.008, 0.006];
                // Night: the lamps' coronas show.
                scene.sky.exposure = exposure_from_ev100(3.0);
                // Odd sizes and a size smaller than the bloom chain is deep.
                let sizes = [(64, 48), (33, 17), (5, 3)];
                render_frames(&device, &mut r, format, &scene, &sizes);
                let stats = r.stats();
                assert_eq!(stats.frames, 3);
                assert_eq!(stats.resources_created, 1);
                assert_eq!(stats.lights_submitted, MAX_LIGHTS as u32);
                // Instances 0..=300 reach lights 0..=33.
                assert_eq!(stats.lights_used, 34);
                // Lights 0, 10, ..., 250, the sun and the moon.
                assert_eq!(stats.coronas, 26 + 2);
                // Ground, then runs of block with a brick or sign every 25 instances, drawn
                // in 3 cascades + forward.
                let runs = 1 + 1 + 12 + 11;
                assert_eq!(stats.shadow_draw_calls, 3 * runs);
                // Bloom: five levels down, four back up; sky and tonemap are one draw each.
                let bloom = if r.is_hdr() {
                    2 * BLOOM_LEVELS as u32 - 1
                } else {
                    0
                };
                assert_eq!(stats.bloom_draw_calls, bloom);
                // Sky, coronas and (HDR) tonemap.
                let passes = if r.is_hdr() { 3 } else { 2 };
                assert_eq!(stats.draw_calls, 4 * runs + passes + bloom);

                // Bloom off, no lights, no coronas: those passes are skipped.
                scene.sky.bloom_strength = 0.0;
                scene.sky.corona_strength = 0.0;
                scene.lights = &[];
                render_frames(&device, &mut r, format, &scene, &[(64, 48)]);
                let stats = r.stats();
                assert_eq!(stats.bloom_draw_calls, 0);
                assert_eq!((stats.lights_submitted, stats.lights_used), (0, 0));
                assert_eq!(stats.coronas, 0);
                assert_eq!(stats.draw_calls, 4 * runs + passes - 1);
                assert_eq!(stats.resources_created, 1);
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            }
        }
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
                render_frames(&device, &mut r, format, &scene, &[(128, 72)]);
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

    #[test]
    fn hud_quad_layout_matches_the_shader() {
        assert_eq!(size_of::<HudQuad>(), 36);
        assert_eq!(HUD_ATTRIBUTES[1].offset, 16);
        assert_eq!(HUD_ATTRIBUTES[2].offset, 32);
        let wgsl = include_str!("../shaders/hud.wgsl");
        for (i, name) in ["rect", "uv", "color"].iter().enumerate() {
            assert!(wgsl.contains(&format!("@location({i}) {name}: vec4<f32>")));
        }
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

    #[test]
    fn frame_uniforms_carry_cascades_and_sky() {
        let (device, queue) = noop_device(wgpu::Limits::downlevel_webgl2_defaults());
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, (8, 8));
        r.set_cascade_splits([30.0, 100.0, 400.0]);
        let scene = test_scene(&[]);
        let (u, c) = r.frame_uniforms(&scene);
        assert_eq!(&u.cascade_far[..3], &[30.0, 100.0, 400.0]);
        for (m, cascade) in u.cascade_view_proj.iter().zip(&c) {
            assert_eq!(*m, cascade.view_proj);
        }
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
}
