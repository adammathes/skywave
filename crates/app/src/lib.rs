//! Skywave's game loop logic, independent of any window or browser.
//!
//! The platform crate samples input every frame into an [`InputState`], calls
//! [`App::update`] with the real frame time, and draws [`App::scene_view`]. The
//! simulation itself only ever advances in fixed `SIM_DT` steps.

pub mod daylight;
mod input;

pub use input::{Action, InputState, Key, Keyboard};

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};
use place::Place;
use render::{Camera, Mesh, MeshId, MeshInstance, SceneView, Sky, Sun};
use skywave_core::{hash64, FixedStep, Rng, Seed, Steps, SIM_DT};

/// Free-fly speed, m/s.
const FLY_SPEED: f32 = 24.0;
/// Turn rate at full look input, rad/s.
const TURN_RATE: f32 = 1.6;
/// Pitch limit, rad.
const MAX_PITCH: f32 = 1.45;
/// Lowest camera height, m.
const MIN_HEIGHT: f32 = 1.2;

const MESH_GROUND: MeshId = MeshId(0);
const MESH_BLOCK: MeshId = MeshId(1);
/// Rng key for building tints.
const KEY_TINTS: u64 = 0x5449_4e54;

/// Linear-light building tints: plaster, brick, concrete, sand, sage, slate.
const BUILDING_TINTS: [[f32; 3]; 6] = [
    [0.78, 0.74, 0.66],
    [0.55, 0.26, 0.18],
    [0.60, 0.60, 0.58],
    [0.72, 0.58, 0.40],
    [0.44, 0.52, 0.42],
    [0.36, 0.40, 0.46],
];

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
        self.yaw -= input.axis(Action::LookX) * TURN_RATE * dt;
        self.pitch =
            (self.pitch + input.axis(Action::LookY) * TURN_RATE * dt).clamp(-MAX_PITCH, MAX_PITCH);
        let (s, c) = self.yaw.sin_cos();
        let forward = Vec3::new(-s, 0.0, -c);
        let right = Vec3::new(c, 0.0, -s);
        let axis = |pos: Action, neg: Action| input.axis(pos) - input.axis(neg);
        let wish = forward * axis(Action::Forward, Action::Back)
            + right * axis(Action::Right, Action::Left)
            + Vec3::Y * axis(Action::Up, Action::Down);
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
        let place = Place::test_grid();
        let camera = FlyCamera {
            pos: Vec3::new(-75.0, 24.0, 165.0),
            yaw: -0.42,
            pitch: -0.17,
        };
        App {
            clock: FixedStep::new(),
            ticks: 0,
            prev: camera,
            curr: camera,
            meshes: vec![
                Mesh::ground_quad(1500.0, 1500.0, [255; 4]),
                Mesh::unit_block([255; 4]),
            ],
            instances: build_instances(&place, seed),
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
                color: [3.0, 2.75, 2.35],
            },
            sky: Sky {
                zenith: [0.09, 0.22, 0.70],
                horizon: [0.50, 0.62, 0.80],
                ground: [0.20, 0.19, 0.17],
                haze: 0.0009,
            },
            exposure: 0.7,
            instances: &self.instances,
        }
    }
}

/// Ground, then roads and buildings as scaled unit blocks (all sharing one mesh, so they
/// draw in a single instanced call).
fn build_instances(place: &Place, seed: Seed) -> Vec<MeshInstance> {
    let mut out = vec![MeshInstance {
        mesh: MESH_GROUND,
        transform: Mat4::IDENTITY.to_cols_array_2d(),
        color: [0.16, 0.24, 0.09, 1.0],
    }];

    let nodes = &place.roads.nodes;
    for e in &place.roads.edges {
        let a = nodes[e.a as usize].pos;
        let b = nodes[e.b as usize].pos;
        let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dz * dz).sqrt();
        let width = f32::from(e.lanes) * 3.5 + 2.0;
        let transform =
            Mat4::from_translation(Vec3::new((a[0] + b[0]) * 0.5, 0.0, (a[1] + b[1]) * 0.5))
                * Mat4::from_rotation_y(dx.atan2(dz))
                * Mat4::from_scale(Vec3::new(width, 0.08, len + width));
        out.push(MeshInstance {
            mesh: MESH_BLOCK,
            transform: transform.to_cols_array_2d(),
            color: [0.07, 0.07, 0.075, 1.0],
        });
    }

    let mut rng = Rng::new(seed, &[KEY_TINTS]);
    for b in &place.buildings {
        // The skeleton draws each footprint's bounding box; the test grid only has rectangles.
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &b.footprint {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let center = Vec3::new((lo[0] + hi[0]) * 0.5, 0.0, (lo[1] + hi[1]) * 0.5);
        let size = Vec3::new(hi[0] - lo[0], b.height, hi[1] - lo[1]);
        let tint = BUILDING_TINTS[rng.range(0, BUILDING_TINTS.len() as u32) as usize];
        out.push(MeshInstance {
            mesh: MESH_BLOCK,
            transform: (Mat4::from_translation(center) * Mat4::from_scale(size)).to_cols_array_2d(),
            color: [tint[0], tint[1], tint[2], 1.0],
        });
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
                input.set(Action::Forward, 1.0);
            }
            if (120..360).contains(&frame) {
                input.set(Action::LookX, 0.5);
                input.set(Action::Up, 1.0);
            }
            if frame >= 300 {
                input.set(Action::Right, 1.0);
                input.set(Action::LookY, -0.25);
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
        input.set(Action::Forward, 1.0);
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
        input.set(Action::Down, 1.0);
        input.set(Action::LookY, -1.0);
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
        input.set(Action::Forward, 1.0);
        let steps = app.update(&input, SIM_DT * 1.5);
        assert_eq!(steps.count, 1);
        let at = |alpha| Mat4::from_cols_array_2d(&app.scene_view(alpha, 1.5).camera.view);
        assert_eq!(at(0.0), app.prev.view());
        assert_eq!(at(1.0), app.curr.view());
        assert_ne!(at(0.5), at(0.0));

        let view = app.scene_view(steps.alpha, 16.0 / 9.0);
        let place = Place::test_grid();
        assert_eq!(
            view.instances.len(),
            1 + place.roads.edges.len() + place.buildings.len()
        );
        assert_eq!(view.instances[0].mesh, MESH_GROUND);
        assert!(view.instances[1..].iter().all(|i| i.mesh == MESH_BLOCK));
        assert!(view
            .instances
            .iter()
            .all(|i| Mat4::from_cols_array_2d(&i.transform).is_finite()));
        assert!((app.meshes().len() as u32) > MESH_BLOCK.0);
    }

    #[test]
    fn seed_changes_building_tints_only() {
        let a = App::new(Seed(1));
        let b = App::new(Seed(2));
        assert_eq!(a.instances.len(), b.instances.len());
        assert_ne!(a.instances, b.instances);
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(App::new(Seed(1)).instances, a.instances);
    }
}
