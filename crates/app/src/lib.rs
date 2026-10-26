//! Skywave's game loop logic, independent of any window or browser.
//!
//! The platform crate feeds raw input events to an [`InputMapper`], calls [`App::frame`]
//! once per frame with the real frame time (it samples the mapper, runs the menus and
//! [`ui`], and calls [`App::update`] unless a menu has the game paused), and draws
//! [`App::scene_view`]. The simulation itself only ever advances in fixed `SIM_DT` steps:
//! the car ([`drive`]), then its cameras ([`camera`]), then the clock ([`lighting`]).

pub mod camera;
pub mod car;
pub mod daylight;
pub mod drive;
mod input;
pub mod lighting;
mod settings;
pub mod ui;
pub mod vehicles;
mod worldmesh;

pub use settings::{MemoryStore, Settings, SettingsStore, SETTINGS_VERSION};

pub use camera::CameraMode;
pub use drive::{HudState, DEFAULT_VEHICLE};

pub use input::{
    pad_table, Action, AxisOptions, Binding, Bindings, BindingsError, Conflict, Context,
    DeviceKind, InputMapper, InputState, Key, MouseAxis, PadAxis, PadButton, RawInput, Sign,
    BINDINGS_VERSION, MOUSE_BUTTONS, MOUSE_LOOK_PER_PIXEL, MOUSE_RIGHT,
};

use camera::CameraRig;
use car::{CarMeshes, CarModel};
use drive::{Drive, DriveGround};
use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};
use lighting::TimeOfDay;
use place::Place;
use render::{Camera, Mesh, MeshId, MeshInstance, PointLight, SceneView, Vertex};
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

/// The free-fly debug camera.
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
    /// The free-fly camera drives the view instead of the car's cameras.
    fly: bool,
    prev: FlyCamera,
    curr: FlyCamera,
    drive: Drive,
    ground: DriveGround,
    camera: CameraRig,
    time: TimeOfDay,
    meshes: Vec<Mesh>,
    car_meshes: CarMeshes,
    /// The world's instances, then this frame's car instances (refilled by `scene_view`).
    instances: Vec<MeshInstance>,
    world_instances: usize,
    lights: [PointLight; 2],
    /// Interpolation alpha of the last frame, held while paused.
    alpha: f32,
    /// Menus, HUD, overlay and the user's settings and bindings.
    ui: ui::Ui,
}

impl App {
    /// The default car at the start of the strip.
    pub fn new(seed: Seed) -> App {
        App::new_with(seed, DEFAULT_VEHICLE).expect("the default car is built in")
    }

    /// The built-in car `vehicle_id` (see [`vehicles::BUILTIN`]) at the start of the strip.
    pub fn new_with(seed: Seed, vehicle_id: &str) -> Result<App, String> {
        let spec = vehicles::builtin_spec(vehicle_id)?;
        let shape = vehicles::builtin_shape(&spec)?;
        let model = CarModel::build(&spec, &shape)?;
        let drive = Drive::with_layout(
            spec,
            model.layout,
            drive::START_GROUND,
            drive::START_HEADING_RAD,
        );
        // The test town's layout is fixed; the session seed drives its detailing.
        let mut place = Place::test_grid();
        place.header.seed = seed.0;
        let chunks = world::build_chunk_meshes(&place, &Palette::socal_1985());
        let mut meshes = render_meshes(&chunks);
        let mut instances: Vec<MeshInstance> = (0..meshes.len())
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
        let world_instances = instances.len();
        let car_meshes = model.register(|m| {
            meshes.push(m.clone());
            MeshId(meshes.len() as u32 - 1)
        });
        instances.reserve_exact(drive::MAX_CAR_INSTANCES);
        let camera = CameraRig::new(CameraMode::ChaseMid, &drive);
        let fly = fly_from(&camera.pose(&drive, 1.0));
        Ok(App {
            clock: FixedStep::new(),
            ticks: 0,
            fly: false,
            prev: fly,
            curr: fly,
            ground: drive::ground_for(&place),
            drive,
            camera,
            time: TimeOfDay::default(),
            meshes,
            car_meshes,
            instances,
            world_instances,
            lights: [PointLight {
                position: [0.0; 3],
                color: [0.0; 3],
                radius: 1.0,
            }; 2],
            alpha: 0.0,
            ui: ui::Ui::new(),
        })
    }

    /// Selects a car camera.
    pub fn set_camera_mode(&mut self, mode: CameraMode) {
        self.camera.set_mode(mode);
    }

    /// Switches the free-fly debug camera on or off. It starts where the view is now.
    pub fn set_fly(&mut self, on: bool) {
        if on && !self.fly {
            self.curr = fly_from(&self.camera.pose(&self.drive, 1.0));
            self.prev = self.curr;
        }
        self.fly = on;
    }

    /// What the HUD shows this frame.
    pub fn hud_state(&self) -> HudState {
        let s = &self.drive.state;
        HudState {
            speed_mps: s.speed_mps,
            rpm: s.engine_rpm,
            gear: s.gear,
            time_of_day: self.time.clock.seconds_of_day as f32,
            camera_mode: self.camera.mode(),
            headlights: self.drive.headlights,
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
        // Presses act once per frame, however many steps it holds.
        if input.just_pressed(Action::CameraCycle) {
            if input.pressed(Action::DebugOverlay) {
                self.set_fly(!self.fly);
            } else if !self.fly {
                self.camera.cycle();
            }
        }
        self.drive.frame(input);
        let none = InputState::default();
        let dt = SIM_DT as f32;
        for _ in 0..steps.count {
            self.prev = self.curr;
            let car_input = if self.fly {
                self.curr.step(input, dt);
                vehicle::VehicleInput::default()
            } else {
                drive::vehicle_input(input)
            };
            self.drive.step(car_input, &self.ground);
            let cam_input = if self.fly { &none } else { input };
            self.camera.step(&self.drive, cam_input, dt, &self.ground);
            self.time.step(input, SIM_DT);
            self.ticks += 1;
        }
        steps
    }

    /// Fingerprint of the simulation state, for determinism checks.
    pub fn state_hash(&self) -> u64 {
        let c = &self.curr;
        let bits = |v: f32| u64::from(v.to_bits());
        let clock = &self.time.clock;
        hash64(
            self.ticks,
            &[
                bits(c.pos.x),
                bits(c.pos.y),
                bits(c.pos.z),
                bits(c.yaw),
                bits(c.pitch),
                self.drive.state.hash(),
                clock.seconds_of_day.to_bits(),
                u64::from(clock.day_of_year),
                u64::from(self.drive.headlights),
            ],
        )
    }

    /// The scene interpolated `alpha` of the way from the previous to the current step.
    /// Refills the car's instances in place (no allocation after the first frame).
    pub fn scene_view(&mut self, alpha: f32, aspect: f32) -> SceneView<'_> {
        let alpha = alpha.clamp(0.0, 1.0);
        let aspect = aspect.max(1e-3);
        let (view, proj) = if self.fly {
            let cam = self.prev.lerp(&self.curr, alpha);
            let proj = directx::perspective(FLY_FOV_DEG.to_radians(), aspect, 0.25, 3000.0);
            (cam.view(), proj)
        } else {
            let pose = self.camera.pose(&self.drive, alpha);
            let proj = directx::perspective(pose.fov_y_rad, aspect, pose.near_m, pose.far_m);
            (pose.view(), proj)
        };
        self.instances.truncate(self.world_instances);
        let cabin = !self.fly && self.camera.mode().first_person();
        self.drive
            .push_instances(alpha, &self.car_meshes, cabin, &mut self.instances);
        let (lights, n) = self.drive.lights(alpha);
        self.lights = lights;
        let daylight = self.time.daylight();
        SceneView {
            camera: Camera {
                view: view.to_cols_array_2d(),
                proj: proj.to_cols_array_2d(),
            },
            sun: lighting::sun(&daylight),
            sky: self.ui.video_sky(lighting::sky(&daylight)),
            instances: &self.instances,
            lights: &self.lights[..n],
            hud: self.ui.hud(),
        }
    }
}

/// Vertical field of view of the free-fly camera, degrees.
const FLY_FOV_DEG: f32 = 62.0;

/// A free-fly camera where `pose` is, looking the same way.
fn fly_from(pose: &camera::CameraPose) -> FlyCamera {
    let f = pose.forward();
    FlyCamera {
        pos: pose.eye,
        yaw: (-f.x).atan2(-f.z),
        pitch: f.y.clamp(-1.0, 1.0).asin().clamp(-MAX_PITCH, MAX_PITCH),
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
        // Street lamps share the window flag, so they are dark by day and lit from dusk.
        let emissive = match e[3] {
            x if x == Glow::Window as u8 => render::emissive_window([e[0], e[1], e[2]], 1.0),
            x if x == Glow::Lamp as u8 => render::emissive_window([e[0], e[1], e[2]], 2.0),
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

    /// Jittery frame times around 60 Hz.
    fn frame_dt(frame: u32) -> f64 {
        1.0 / 60.0 + f64::from(frame % 7) * 0.0013 - 0.003
    }

    /// The scripted drive: 5 s full throttle, 2 s steering right, 2 s braking. Returns the
    /// peak speed.
    fn run_script(app: &mut App) -> f32 {
        let mut input = InputState::default();
        let mut t = 0.0;
        let mut peak = 0.0f32;
        let mut frame = 0;
        while t < 9.0 {
            input.clear();
            if t < 5.0 {
                input.set(Action::Throttle, 1.0);
            } else if t < 7.0 {
                input.set(Action::Steer, 0.6);
            } else {
                input.set(Action::Brake, 1.0);
            }
            let dt = frame_dt(frame);
            app.update(&input, dt);
            t += dt;
            frame += 1;
            peak = peak.max(app.drive.state.speed_mps);
            let s = &app.drive.state;
            assert!(
                s.pos
                    .iter()
                    .chain(&s.rot)
                    .chain(&s.vel)
                    .all(|v| v.is_finite()),
                "NaN at {t}"
            );
            let alpha = (frame % 4) as f32 / 4.0;
            let view = app.scene_view(alpha, 1.5);
            for i in view.instances {
                assert!(Mat4::from_cols_array_2d(&i.transform).is_finite());
            }
        }
        peak
    }

    #[test]
    fn scripted_drive_moves_the_car_deterministically() {
        let mut a = App::new(Seed(1));
        let mut b = App::new(Seed(1));
        let start = Vec3::from(a.drive.state.pos);
        let peak = run_script(&mut a);
        run_script(&mut b);
        assert_eq!(a.ticks(), b.ticks());
        assert!(a.ticks() >= 1080, "{}", a.ticks());
        assert_eq!(a.drive.state, b.drive.state);
        assert_eq!(a.state_hash(), b.state_hash());

        let moved = (Vec3::from(a.drive.state.pos) - start).length();
        assert!(moved > 50.0, "{moved}");
        let end = a.drive.state.speed_mps;
        assert!(peak > 15.0, "{peak}");
        assert!(end < peak - 5.0, "{end} vs {peak}");
        // Pinned: a change here is a change to the sim, the input mapping, the step order or
        // the chase camera (the hashed debug fly camera starts at its pose). Update it
        // deliberately.
        assert_eq!(
            a.state_hash(),
            0x0c4a_c704_5986_cac0,
            "{:#x}",
            a.state_hash()
        );
    }

    #[test]
    fn the_wagon_drives_too() {
        let mut app = App::new_with(Seed(1), "palisade_wagon").unwrap();
        assert_eq!(app.drive.spec.id, "palisade_wagon");
        let start = Vec3::from(app.drive.state.pos);
        run_script(&mut app);
        assert!((Vec3::from(app.drive.state.pos) - start).length() > 30.0);
        assert!(App::new_with(Seed(1), "hovercar").is_err());
    }

    fn press(input: &mut InputState, action: Action) {
        input.set(action, 1.0);
        input.set_just_pressed(action, true);
    }

    #[test]
    fn camera_cycle_and_the_debug_fly_toggle() {
        let mut app = App::new(Seed(0));
        assert_eq!(app.hud_state().camera_mode, CameraMode::ChaseMid);
        let mut input = InputState::default();
        press(&mut input, Action::CameraCycle);
        app.update(&input, SIM_DT);
        assert_eq!(app.hud_state().camera_mode, CameraMode::ChaseFar);
        app.update(&input, SIM_DT);
        assert_eq!(app.hud_state().camera_mode, CameraMode::Cockpit);
        assert!(!app.fly);

        input.set(Action::DebugOverlay, 1.0);
        app.update(&input, SIM_DT);
        assert!(app.fly);
        assert_eq!(
            app.hud_state().camera_mode,
            CameraMode::Cockpit,
            "not cycled"
        );
        app.update(&input, SIM_DT);
        assert!(!app.fly);
    }

    #[test]
    fn hud_state_reads_the_car_clock_and_switches() {
        let mut app = App::new(Seed(0));
        let hud = app.hud_state();
        assert_eq!(hud.gear, 1);
        assert_eq!(hud.speed_mps, 0.0);
        assert!(hud.rpm > 500.0);
        assert!((hud.time_of_day - (17.0 * 3600.0 + 40.0 * 60.0)).abs() < 1.0);
        assert!(!hud.headlights);
        let mut input = InputState::default();
        press(&mut input, Action::Headlights);
        input.set(Action::Throttle, 1.0);
        for _ in 0..60 {
            app.update(&input, 1.0 / 60.0);
            input.set_just_pressed(Action::Headlights, false);
        }
        let hud = app.hud_state();
        assert!(hud.headlights);
        assert!(hud.speed_mps > 2.0, "{}", hud.speed_mps);
        assert!(hud.time_of_day > app.hud_state().time_of_day - 1.0);
        // Headlights on: two pools of light ahead and the lit lamps drawn.
        let lamps = app.car_meshes.lods[0].lights[1];
        let view = app.scene_view(0.5, 1.5);
        assert_eq!(view.lights.len(), 2);
        assert!(view.instances.iter().any(|i| lamps.contains(&i.mesh)));
    }

    #[test]
    fn fly_camera_flies_along_minus_z_at_fly_speed() {
        let mut app = App::new(Seed(0));
        app.set_fly(true);
        app.curr.yaw = 0.0;
        app.prev = app.curr;
        let start = app.curr.pos;
        let car = app.drive.state;
        let mut input = InputState::default();
        input.set(Action::FlyForward, 1.0);
        input.set(Action::Throttle, 1.0);
        for _ in 0..120 {
            app.update(&input, SIM_DT);
        }
        let moved = app.curr.pos - start;
        assert_eq!(app.ticks(), 120);
        assert!((moved.z + FLY_SPEED).abs() < 1e-2, "{moved}");
        assert!(moved.x.abs() < 1e-4 && moved.y.abs() < 1e-4);
        // The car ignores the throttle while flying.
        let rolled = Vec3::from(app.drive.state.pos) - Vec3::from(car.pos);
        assert!(rolled.length() < 0.05, "{rolled}");
    }

    #[test]
    fn fly_camera_stays_above_ground_and_pitch_is_limited() {
        let mut app = App::new(Seed(0));
        app.set_fly(true);
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
        input.set(Action::Throttle, 1.0);
        for _ in 0..120 {
            app.update(&input, SIM_DT);
        }
        let steps = app.update(&input, SIM_DT * 1.5);
        assert_eq!(steps.count, 1);
        let mut at = |alpha| Mat4::from_cols_array_2d(&app.scene_view(alpha, 1.5).camera.view);
        let (v0, v5, v1) = (at(0.0), at(0.5), at(1.0));
        assert_eq!(v0, app.camera.pose(&app.drive, 0.0).view());
        assert_eq!(v1, app.camera.pose(&app.drive, 1.0).view());
        assert_ne!(v5, v0);

        let world = app.world_instances;
        let meshes = app.meshes().len();
        let ids = app.car_meshes;
        let view = app.scene_view(steps.alpha, 16.0 / 9.0);
        // The world, then the car: body, glass, unlit lamps and four wheels (no cabin in
        // the chase view).
        assert_eq!(MeshId(world as u32), ids.lods[0].body[0]);
        assert_eq!(ids.dial_ticks, MeshId(meshes as u32 - 1));
        assert_eq!(view.instances.len(), world + 7);
        for (i, inst) in view.instances[..world].iter().enumerate() {
            assert_eq!(inst.mesh, MeshId(i as u32));
        }
        let lod = ids.lods[0];
        assert_eq!(view.instances[world].mesh, lod.body[0]);
        assert_eq!(view.instances[world + 1].mesh, lod.glass);
        assert_eq!(view.instances[world + 2].mesh, lod.lights[0][0]);
        assert!(view.instances[world + 3..]
            .iter()
            .all(|i| i.mesh == lod.wheel));
        for inst in view.instances {
            assert!(Mat4::from_cols_array_2d(&inst.transform).is_finite());
        }
        assert!(view.lights.is_empty(), "headlights off");
        // Golden hour: low warm sun in the west.
        assert!(view.sun.direction[0] < -0.9 && view.sun.direction[1] > 0.0);
        assert!(view.sun.illuminance[0] > view.sun.illuminance[2]);
        // Some meshes carry glowing signs and lamps, drawn boosted.
        assert!(view.instances.iter().any(|i| i.color[0] == GLOW_BOOST));
        let tris: usize = app.meshes().iter().map(|m| m.indices.len() / 3).sum();
        assert!(tris > 100_000, "{tris}");
        for m in app.meshes() {
            assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        }
        // Refilling does not grow the list.
        let cap = app.instances.capacity();
        for _ in 0..3 {
            app.scene_view(0.3, 1.0);
        }
        assert_eq!(app.instances.capacity(), cap);
    }

    #[test]
    fn the_car_starts_on_the_road_in_the_chase_view() {
        let mut app = App::new(Seed(1));
        let view = app.scene_view(1.0, 16.0 / 9.0);
        let v = Mat4::from_cols_array_2d(&view.camera.view);
        let p = Mat4::from_cols_array_2d(&view.camera.proj);
        let car = Vec3::from(app.drive.state.pos);
        let clip = p * v * car.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(
            clip.w > 0.0 && ndc.x.abs() < 0.2 && ndc.y.abs() < 0.6,
            "{ndc}"
        );
    }

    #[test]
    fn the_cockpit_draws_the_cabin_without_growing_the_list() {
        let mut app = App::new(Seed(1));
        let cap = app.instances.capacity();
        app.set_camera_mode(CameraMode::Cockpit);
        let world = app.world_instances;
        let ids = app.car_meshes;
        let view = app.scene_view(1.0, 16.0 / 9.0);
        let car: Vec<MeshId> = view.instances[world..].iter().map(|i| i.mesh).collect();
        assert!(car.contains(&ids.lods[0].body[0]), "the hood shows");
        assert!(car.contains(&ids.lods[0].interior));
        assert!(car.contains(&ids.lods[0].steering_wheel));
        assert!(car.contains(&ids.lods[0].shell_inside));
        assert_eq!(car.len(), 11 + app.drive.layout.dial_count);
        assert_eq!(app.instances.capacity(), cap);
        // Back in the chase view the cabin is not drawn.
        app.set_camera_mode(CameraMode::ChaseNear);
        let view = app.scene_view(1.0, 16.0 / 9.0);
        assert!(view
            .instances
            .iter()
            .all(|i| i.mesh != ids.lods[0].interior));
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
