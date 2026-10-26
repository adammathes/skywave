//! Skywave's game loop logic, independent of any window or browser.
//!
//! The platform crate feeds raw input events to an [`InputMapper`], samples it once per
//! frame into an [`InputState`], calls
//! [`App::update`] with the real frame time, and draws [`App::scene_view`]. The
//! simulation itself only ever advances in fixed `SIM_DT` steps.

pub mod daylight;
mod input;
pub mod vehicles;
mod worldmesh;

pub use input::{
    pad_table, Action, AxisOptions, Binding, Bindings, BindingsError, Conflict, Context,
    DeviceKind, InputMapper, InputState, Key, MouseAxis, PadAxis, PadButton, RawInput, Sign,
    BINDINGS_VERSION, MOUSE_BUTTONS, MOUSE_LOOK_PER_PIXEL, MOUSE_RIGHT,
};

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};
use place::Place;
use render::{Camera, Mesh, MeshId, MeshInstance, SceneView, Sky, Sun, Vertex};
use skywave_core::{hash64, FixedStep, Seed, Steps, SIM_DT};
use world::{ChunkMesh, Glow, Palette, WorldMesh};

/// Free-fly speed, m/s.
const FLY_SPEED: f32 = 24.0;
/// Turn rate at full look input, rad/s.
const TURN_RATE: f32 = 1.6;
/// Pitch limit, rad.
const MAX_PITCH: f32 = 1.45;
/// Lowest camera height, m.
const MIN_HEIGHT: f32 = 1.2;

/// Instance color multiplier for glowing surfaces (neon, lamps, signals) until the
/// renderer has an emissive channel: bright enough to read as lit in daylight.
const GLOW_BOOST: f32 = 1.8;
/// Share of a lit window's glow mixed into its glass color (out of 256), so lit windows
/// read as warmer panes in daylight.
const WINDOW_TINT_256: u32 = 24;

/// The free-fly camera, the entire simulation state for now.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FlyCamera {
    pos: Vec3,
    /// Radians, counter-clockwise seen from above; 0 looks down -Z.
    yaw: f32,
    /// Radians, positive looks up.
    pitch: f32,
}

impl FlyCamera {
    fn step(&mut self, input: &InputState, dt: f32) {
        self.yaw -= input.axis(Action::FlyLookX) * TURN_RATE * dt;
        // FlyLookY is screen space (+ looks down); pitch is + up.
        self.pitch = (self.pitch - input.axis(Action::FlyLookY) * TURN_RATE * dt)
            .clamp(-MAX_PITCH, MAX_PITCH);
        let (s, c) = self.yaw.sin_cos();
        let forward = Vec3::new(-s, 0.0, -c);
        let right = Vec3::new(c, 0.0, -s);
        let axis = |pos: Action, neg: Action| input.axis(pos) - input.axis(neg);
        let wish = forward * axis(Action::FlyForward, Action::FlyBack)
            + right * axis(Action::FlyRight, Action::FlyLeft)
            + Vec3::Y * axis(Action::FlyUp, Action::FlyDown);
        let wish = if wish.length_squared() > 1.0 {
            wish.normalize()
        } else {
            wish
        };
        self.pos += wish * (FLY_SPEED * dt);
        self.pos.y = self.pos.y.max(MIN_HEIGHT);
    }

    fn lerp(&self, next: &FlyCamera, t: f32) -> FlyCamera {
        FlyCamera {
            pos: self.pos.lerp(next.pos, t),
            yaw: self.yaw + (next.yaw - self.yaw) * t,
            pitch: self.pitch + (next.pitch - self.pitch) * t,
        }
    }

    fn view(&self) -> Mat4 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let dir = Vec3::new(-sy * cp, sp, -cy * cp);
        view::look_to_mat4(self.pos, dir, Vec3::Y)
    }
}

pub struct App {
    clock: FixedStep,
    ticks: u64,
    prev: FlyCamera,
    curr: FlyCamera,
    meshes: Vec<Mesh>,
    instances: Vec<MeshInstance>,
}

impl App {
    pub fn new(seed: Seed) -> App {
        // The test town's layout is fixed; the session seed drives its detailing.
        let mut place = Place::test_grid();
        place.header.seed = seed.0;
        let chunks = world::build_chunk_meshes(&place, &Palette::socal_1985());
        let meshes = render_meshes(&chunks);
        let instances = (0..meshes.len())
            .map(|i| MeshInstance {
                mesh: MeshId(i as u32),
                transform: Mat4::IDENTITY.to_cols_array_2d(),
                color: if i < chunks.len() {
                    [1.0; 4]
                } else {
                    [GLOW_BOOST, GLOW_BOOST, GLOW_BOOST, 1.0]
                },
            })
            .collect();
        // Eye height above the arterial, in the westbound lane at the east end of the
        // strip, looking west down it with the afternoon sun behind.
        let camera = FlyCamera {
            pos: Vec3::new(92.0, 3.0, -3.6),
            yaw: std::f32::consts::FRAC_PI_2 - 0.06,
            pitch: -0.05,
        };
        App {
            clock: FixedStep::new(),
            ticks: 0,
            prev: camera,
            curr: camera,
            meshes,
            instances,
        }
    }

    /// Meshes the scene refers to: upload them in order, so `meshes()[i]` gets `MeshId(i)`.
    pub fn meshes(&self) -> &[Mesh] {
        &self.meshes
    }

    /// Simulation steps run so far.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Advances real time by `dt_real` seconds, running as many fixed steps as are due.
    /// Pass the returned `alpha` to [`App::scene_view`].
    pub fn update(&mut self, input: &InputState, dt_real: f64) -> Steps {
        let steps = self.clock.advance(dt_real);
        for _ in 0..steps.count {
            self.prev = self.curr;
            self.curr.step(input, SIM_DT as f32);
            self.ticks += 1;
        }
        steps
    }

    /// Fingerprint of the simulation state, for determinism checks.
    pub fn state_hash(&self) -> u64 {
        let c = &self.curr;
        let bits = |v: f32| u64::from(v.to_bits());
        hash64(
            self.ticks,
            &[
                bits(c.pos.x),
                bits(c.pos.y),
                bits(c.pos.z),
                bits(c.yaw),
                bits(c.pitch),
            ],
        )
    }

    /// The scene interpolated `alpha` of the way from the previous to the current step.
    pub fn scene_view(&self, alpha: f32, aspect: f32) -> SceneView<'_> {
        let cam = self.prev.lerp(&self.curr, alpha.clamp(0.0, 1.0));
        let proj = directx::perspective(62f32.to_radians(), aspect.max(1e-3), 0.25, 3000.0);
        SceneView {
            camera: Camera {
                view: cam.view().to_cols_array_2d(),
                proj: proj.to_cols_array_2d(),
            },
            sun: Sun {
                direction: Vec3::new(0.70, 0.55, 0.45).normalize().to_array(),
                illuminance: [80.0, 74.0, 63.0],
            },
            sky: Sky::default(),
            instances: &self.instances,
            lights: &[],
        }
    }
}

/// Converts world meshes (patterned by [`worldmesh::atlas_mesh`]) to the renderer's vertex
/// format: every chunk mesh as is (lit windows slightly tinted), followed by one mesh
/// holding all neon, lamp and signal triangles colored by their glow, drawn with a boosted
/// instance color.
fn render_meshes(chunks: &[ChunkMesh]) -> Vec<Mesh> {
    let mut out = Vec::with_capacity(chunks.len() + 1);
    let mut glow = Mesh::default();
    for c in chunks {
        let mesh = worldmesh::atlas_mesh(&c.mesh, c.kind);
        out.push(convert(&mesh, false));
        let g = convert(&mesh, true);
        let base = glow.vertices.len() as u32;
        glow.vertices.extend_from_slice(&g.vertices);
        glow.indices.extend(g.indices.iter().map(|i| i + base));
    }
    if !glow.indices.is_empty() {
        out.push(glow);
    }
    out
}

fn convert(m: &WorldMesh, glow_only: bool) -> Mesh {
    let glows = |e: [u8; 4]| e[3] != Glow::None as u8 && e[3] != Glow::Window as u8;
    let vertex = |v: &world::WorldVertex| {
        let (c, e) = (v.color, v.emissive);
        let color = if glow_only {
            [e[0], e[1], e[2], 255]
        } else if e[3] == Glow::Window as u8 {
            let t = WINDOW_TINT_256;
            let mix = |a: u8, b: u8| ((u32::from(a) * (256 - t) + u32::from(b) * t) >> 8) as u8;
            [mix(c[0], e[0]), mix(c[1], e[1]), mix(c[2], e[2]), 255]
        } else {
            [c[0], c[1], c[2], 255]
        };
        // Emissive: the renderer encodes window-flagged glow (scaled by time of day) and plain
        // glow with an intensity; the town's Glow classes map onto that.
        let emissive = match e[3] {
            x if x == Glow::Window as u8 => render::emissive_window([e[0], e[1], e[2]], 1.0),
            x if x == Glow::None as u8 => [0, 0, 0, 0],
            _ => render::emissive([e[0], e[1], e[2]], 2.0),
        };
        Vertex {
            pos: v.pos,
            normal: v.normal,
            uv: render::uv_unorm(v.uv),
            color,
            emissive,
        }
    };
    if !glow_only {
        return Mesh {
            vertices: m.vertices.iter().map(vertex).collect(),
            indices: m.indices.clone(),
        };
    }
    let mut out = Mesh::default();
    for tri in m.indices.chunks_exact(3) {
        if tri.iter().all(|&i| glows(m.vertices[i as usize].emissive)) {
            let base = out.vertices.len() as u32;
            for &i in tri {
                out.vertices.push(vertex(&m.vertices[i as usize]));
            }
            out.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed script of (frame dt, held actions) with jittery frame times.
    fn run_script(app: &mut App) {
        let mut input = InputState::default();
        for frame in 0..600u32 {
            input.clear();
            if frame < 240 {
                input.set(Action::FlyForward, 1.0);
            }
            if (120..360).contains(&frame) {
                input.set(Action::FlyLookX, 0.5);
                input.set(Action::FlyUp, 1.0);
            }
            if frame >= 300 {
                input.set(Action::FlyRight, 1.0);
                input.set(Action::FlyLookY, 0.25);
            }
            let dt = 1.0 / 60.0 + f64::from(frame % 7) * 0.0013 - 0.003;
            app.update(&input, dt);
        }
    }

    #[test]
    fn update_is_deterministic_for_scripted_input() {
        let mut a = App::new(Seed(1));
        let mut b = App::new(Seed(1));
        run_script(&mut a);
        run_script(&mut b);
        assert_eq!(a.ticks(), b.ticks());
        assert!(a.ticks() > 1100, "{}", a.ticks());
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.curr, b.curr);
        assert_ne!(a.curr, App::new(Seed(1)).curr);
    }

    #[test]
    fn forward_flies_along_minus_z_at_fly_speed() {
        let mut app = App::new(Seed(0));
        app.curr.yaw = 0.0;
        app.prev = app.curr;
        let start = app.curr.pos;
        let mut input = InputState::default();
        input.set(Action::FlyForward, 1.0);
        for _ in 0..120 {
            app.update(&input, SIM_DT);
        }
        let moved = app.curr.pos - start;
        assert_eq!(app.ticks(), 120);
        assert!((moved.z + FLY_SPEED).abs() < 1e-2, "{moved}");
        assert!(moved.x.abs() < 1e-4 && moved.y.abs() < 1e-4);
    }

    #[test]
    fn camera_stays_above_ground_and_pitch_is_limited() {
        let mut app = App::new(Seed(0));
        let mut input = InputState::default();
        input.set(Action::FlyDown, 1.0);
        input.set(Action::FlyLookY, 1.0);
        for _ in 0..600 {
            app.update(&input, 1.0 / 30.0);
        }
        assert_eq!(app.curr.pos.y, MIN_HEIGHT);
        assert_eq!(app.curr.pitch, -MAX_PITCH);
    }

    #[test]
    fn scene_view_interpolates_and_is_well_formed() {
        let mut app = App::new(Seed(5));
        let mut input = InputState::default();
        input.set(Action::FlyForward, 1.0);
        let steps = app.update(&input, SIM_DT * 1.5);
        assert_eq!(steps.count, 1);
        let at = |alpha| Mat4::from_cols_array_2d(&app.scene_view(alpha, 1.5).camera.view);
        assert_eq!(at(0.0), app.prev.view());
        assert_eq!(at(1.0), app.curr.view());
        assert_ne!(at(0.5), at(0.0));

        let view = app.scene_view(steps.alpha, 16.0 / 9.0);
        assert_eq!(view.instances.len(), app.meshes().len());
        for (i, inst) in view.instances.iter().enumerate() {
            assert_eq!(inst.mesh, MeshId(i as u32));
            assert!(Mat4::from_cols_array_2d(&inst.transform).is_finite());
        }
        // Some meshes carry glowing signs and lamps, drawn boosted.
        assert!(view.instances.iter().any(|i| i.color[0] == GLOW_BOOST));
        let tris: usize = app.meshes().iter().map(|m| m.indices.len() / 3).sum();
        assert!(tris > 100_000, "{tris}");
        for m in app.meshes() {
            assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        }
    }

    #[test]
    fn seed_changes_detail_only() {
        let a = App::new(Seed(1));
        let b = App::new(Seed(2));
        assert_ne!(a.meshes, b.meshes);
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(App::new(Seed(1)).meshes, a.meshes);
    }
}
