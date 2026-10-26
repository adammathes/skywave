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
mod places;
pub mod scene;
pub mod selftest;
mod settings;
pub mod ui;
pub mod vehicles;
mod worldmesh;

pub use settings::{MemoryStore, Settings, SettingsStore, SETTINGS_VERSION};

pub use camera::CameraMode;
pub use drive::{HudState, DEFAULT_VEHICLE};
pub use scene::{PlaceKey, SceneSetup};

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
use render::{Camera, Mesh, MeshId, MeshInstance, PointLight, SceneView, Vertex, MAX_POINT_LIGHTS};
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
/// Emissive scales of the town's glow classes (see `render::emissive`; 1 = 0.5 kcd/m² for
/// white). Lit windows 0.125 (≈ 40 cd/m², a warm pane; was 1.0, which put every window at
/// the emissive cap at night), street lamp heads and neon 2.
const WINDOW_GLOW: f32 = 0.125;
const LAMP_GLOW: f32 = 2.0;
const NEON_GLOW: f32 = 2.0;
/// Street lamp lens vertices this close (m) belong to the same lamp head.
const LAMP_CLUSTER_M: f32 = 1.0;
/// The light sits this far above the center of its lens (m): inside the head, so the head
/// itself is not lit, only what is below it.
const LAMP_LIGHT_ABOVE_LENS_M: f32 = 0.05;

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
    /// The simulation does not run (a still scene for screenshots).
    still: bool,
    /// The scene fixed the time of day: the clock stays stopped whatever the clock-speed
    /// setting says.
    clock_frozen: bool,
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
    /// Instance of the street lamp heads' mesh, switched with the photocell.
    lamp_instance: Option<usize>,
    /// Where the street lamps' lights hang, world space.
    lamp_heads: Vec<Vec3>,
    /// This frame's lights: the headlights, then the nearest street lamps.
    lights: [PointLight; MAX_POINT_LIGHTS],
    /// Interpolation alpha of the last frame, held while paused.
    alpha: f32,
    /// Menus, HUD, overlay and the user's settings and bindings.
    ui: ui::Ui,
    /// Where the place puts the car: its `PlayerStart` ground point and heading.
    start: ([f32; 3], f32),
}

impl App {
    /// The default car at the start of the strip.
    pub fn new(seed: Seed) -> App {
        App::new_with(seed, DEFAULT_VEHICLE).expect("the default car is built in")
    }

    /// The built-in car `vehicle_id` (see [`vehicles::BUILTIN`]) at the start of the strip.
    pub fn new_with(seed: Seed, vehicle_id: &str) -> Result<App, String> {
        App::new_in(seed, vehicle_id, PlaceKey::TestTown)
    }

    /// The built-in car `vehicle_id` at the `PlayerStart` of the place `key` names.
    pub fn new_in(seed: Seed, vehicle_id: &str, key: PlaceKey) -> Result<App, String> {
        let spec = vehicles::builtin_spec(vehicle_id)?;
        let shape = vehicles::builtin_shape(&spec)?;
        let model = CarModel::build(&spec, &shape)?;
        let place: Place = places::build(key, seed)?;
        let start = place.player_start().ok_or("the place has no PlayerStart")?;
        let start = (start.pos, start.heading);
        let drive = Drive::with_layout(spec, model.layout, start.0, start.1);
        let chunks = world::build_chunk_meshes(&place, &Palette::socal_1985());
        let town = render_meshes(&chunks, places::merge(key));
        let mut meshes = town.meshes;
        let mut instances: Vec<MeshInstance> = (0..meshes.len())
            .map(|i| MeshInstance {
                color: if Some(i) == town.glow {
                    [GLOW_BOOST, GLOW_BOOST, GLOW_BOOST, 1.0]
                } else {
                    [1.0; 4]
                },
                ..MeshInstance::new(MeshId(i as u32), Mat4::IDENTITY.to_cols_array_2d())
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
            still: false,
            clock_frozen: false,
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
            lamp_instance: town.lamps,
            lamp_heads: town.lamp_heads,
            lights: [PointLight::omni([0.0; 3], [0.0; 3], 1.0); MAX_POINT_LIGHTS],
            alpha: 0.0,
            ui: ui::Ui::new(),
            start,
        })
    }

    /// The scene `setup` describes: its seed and car, the car on the ground at its point
    /// and heading, its camera, its headlights, and its clock (frozen when it gives a time).
    pub fn from_setup(setup: &SceneSetup) -> Result<App, String> {
        let mut app = App::new_in(Seed(setup.seed), &setup.car, setup.place)?;
        let pos = setup.pos.unwrap_or([app.start.0[0], app.start.0[2]]);
        let heading = setup.heading_rad.unwrap_or(app.start.1);
        let (y, _) = app.ground.level_at(pos);
        let ground = [pos[0], y, pos[1]];
        app.drive = Drive::with_layout(app.drive.spec.clone(), app.drive.layout, ground, heading);
        app.drive.headlights = setup.headlights;
        app.camera = CameraRig::new(setup.camera, &app.drive);
        app.curr = fly_from(&app.camera.pose(&app.drive, 1.0));
        app.prev = app.curr;
        app.set_fly(setup.fly);
        if let Some(t) = setup.time_s {
            app.time.clock = daylight::Clock::new(lighting::DEFAULT_DAY_OF_YEAR, t);
            app.time.clock.speed = 0.0;
            app.clock_frozen = true;
        }
        app.still = setup.still;
        Ok(app)
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
        if self.still {
            return Steps {
                count: 0,
                alpha: 1.0,
            };
        }
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
        let daylight = self.time.daylight();
        self.instances.truncate(self.world_instances);
        if let Some(k) = self.lamp_instance {
            self.instances[k].glow = lighting::streetlamp_glow(&daylight);
        }
        let cabin = !self.fly && self.camera.mode().first_person();
        self.drive.push_instances(
            alpha,
            &self.car_meshes,
            cabin,
            lighting::car_glow(&daylight),
            &mut self.instances,
        );
        let (head, mut n) = self.drive.lights(alpha);
        self.lights[..n].copy_from_slice(&head[..n]);
        if daylight.streetlights_on {
            // The lamps nearest a point down the view light the road where the eye is.
            let inv = view.inverse();
            let eye = inv.w_axis.truncate();
            let fwd = -inv.z_axis.truncate();
            let flat = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
            let focus = eye + flat * lighting::STREETLAMP_FOCUS_AHEAD_M;
            let mut lamps = [self.lights[0]; lighting::STREETLAMP_LIGHTS];
            let m = lighting::streetlamp_lights(&self.lamp_heads, focus, &mut lamps);
            self.lights[n..n + m].copy_from_slice(&lamps[..m]);
            n += m;
        }
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

/// The town as the renderer draws it ([`render_meshes`]).
struct TownMeshes {
    /// Draw order: the chunk meshes, then the glow mesh and the street lamp heads (each
    /// when there is any).
    meshes: Vec<Mesh>,
    /// Index of the glow mesh (drawn with a boosted instance color) and of the lamp heads'
    /// mesh (switched by the instance's glow).
    glow: Option<usize>,
    lamps: Option<usize>,
    /// Where each street lamp's light hangs, world space.
    lamp_heads: Vec<Vec3>,
}

/// Converts world meshes (patterned by [`worldmesh::atlas_mesh`]) to the renderer's vertex
/// format: every chunk mesh without its street lamp lenses (lit windows slightly tinted),
/// then one mesh holding all neon and signal triangles colored by their glow, drawn with a
/// boosted instance color, then one holding every street lamp lens, which the photocell
/// switches. The lamp heads are where the lenses cluster. With `merge` above 1, the chunk
/// meshes of each `merge × merge` block of chunks (every kind) become one mesh.
fn render_meshes(chunks: &[ChunkMesh], merge: i32) -> TownMeshes {
    let mut meshes = Vec::with_capacity(chunks.len() + 2);
    let (mut glow, mut lamps) = (Mesh::default(), Mesh::default());
    let append = |all: &mut Mesh, part: Mesh| {
        let base = all.vertices.len() as u32;
        all.vertices.extend_from_slice(&part.vertices);
        all.indices.extend(part.indices.iter().map(|i| i + base));
    };
    // Merged blocks, in first-seen order (the chunks come sorted).
    let mut blocks: Vec<([i32; 2], usize)> = Vec::new();
    for c in chunks {
        let mesh = worldmesh::atlas_mesh(&c.mesh, c.kind);
        let base = convert(&mesh, Part::Base);
        if merge > 1 {
            let key = [c.chunk[0].div_euclid(merge), c.chunk[1].div_euclid(merge)];
            match blocks.iter().find(|b| b.0 == key) {
                Some(&(_, i)) => append(&mut meshes[i], base),
                None => {
                    blocks.push((key, meshes.len()));
                    meshes.push(base);
                }
            }
        } else {
            meshes.push(base);
        }
        append(&mut glow, convert(&mesh, Part::Glow));
        append(&mut lamps, convert(&mesh, Part::Lamps));
    }
    let lamp_heads = lamp_heads(&lamps);
    let mut push = |m: Mesh| {
        (!m.indices.is_empty()).then(|| {
            meshes.push(m);
            meshes.len() - 1
        })
    };
    let glow = push(glow);
    let lamps = push(lamps);
    TownMeshes {
        meshes,
        glow,
        lamps,
        lamp_heads,
    }
}

/// Which triangles of a world mesh [`convert`] keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// Everything but street lamp lenses.
    Base,
    /// Neon and signal triangles, colored by their glow.
    Glow,
    /// Street lamp lenses.
    Lamps,
}

fn convert(m: &WorldMesh, part: Part) -> Mesh {
    let is = |v: &world::WorldVertex, g: Glow| v.emissive[3] == g as u8;
    let keep = |v: &world::WorldVertex| match part {
        Part::Base => !is(v, Glow::Lamp),
        Part::Glow => is(v, Glow::Neon) || is(v, Glow::Signal),
        Part::Lamps => is(v, Glow::Lamp),
    };
    let vertex = |v: &world::WorldVertex| {
        let (c, e) = (v.color, v.emissive);
        let color = if part == Part::Glow {
            [e[0], e[1], e[2], 255]
        } else if is(v, Glow::Window) {
            let t = WINDOW_TINT_256;
            let mix = |a: u8, b: u8| ((u32::from(a) * (256 - t) + u32::from(b) * t) >> 8) as u8;
            [mix(c[0], e[0]), mix(c[1], e[1]), mix(c[2], e[2]), 255]
        } else {
            [c[0], c[1], c[2], 255]
        };
        // Emissive: the renderer encodes window-flagged glow (scaled by time of day) and plain
        // glow with an intensity; the town's Glow classes map onto that. Street lamps are
        // plain glow in their own mesh, whose instance the photocell switches.
        let rgb = [e[0], e[1], e[2]];
        let emissive = match e[3] {
            x if x == Glow::Window as u8 => render::emissive_window(rgb, WINDOW_GLOW),
            x if x == Glow::Lamp as u8 => render::emissive(rgb, LAMP_GLOW),
            x if x == Glow::None as u8 => [0, 0, 0, 0],
            _ => render::emissive(rgb, NEON_GLOW),
        };
        Vertex {
            pos: v.pos,
            normal: v.normal,
            uv: render::uv_unorm(v.uv),
            color,
            emissive,
        }
    };
    let tris = m.indices.chunks_exact(3);
    if part == Part::Base {
        // Every vertex, without the triangles whose corners are all lamp lens.
        return Mesh {
            vertices: m.vertices.iter().map(vertex).collect(),
            indices: tris
                .filter(|t| !t.iter().all(|&i| is(&m.vertices[i as usize], Glow::Lamp)))
                .flatten()
                .copied()
                .collect(),
        };
    }
    let mut out = Mesh::default();
    for tri in tris {
        if tri.iter().all(|&i| keep(&m.vertices[i as usize])) {
            let base = out.vertices.len() as u32;
            for &i in tri {
                out.vertices.push(vertex(&m.vertices[i as usize]));
            }
            out.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    }
    out
}

/// Street lamp heads from their lenses' vertices: each cluster of vertices within
/// [`LAMP_CLUSTER_M`] of its first is one lamp, its light just above the lens's center.
/// Deterministic (first-come clusters, in mesh order).
fn lamp_heads(lenses: &Mesh) -> Vec<Vec3> {
    let mut sums: Vec<(Vec3, Vec3, u32)> = Vec::new();
    for v in &lenses.vertices {
        let p = Vec3::from(v.pos);
        match sums
            .iter_mut()
            .find(|(first, _, _)| first.distance_squared(p) < LAMP_CLUSTER_M * LAMP_CLUSTER_M)
        {
            Some((_, sum, n)) => {
                *sum += p;
                *n += 1;
            }
            None => sums.push((p, p, 1)),
        }
    }
    sums.iter()
        .map(|&(_, sum, n)| sum / n as f32 + Vec3::Y * LAMP_LIGHT_ABOVE_LENS_M)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scripted drive ([`selftest::run_drive`]), checking every frame. Returns the
    /// peak speed.
    fn run_script(app: &mut App) -> f32 {
        let mut input = InputState::default();
        let mut t = 0.0;
        let mut peak = 0.0f32;
        let mut frame = 0;
        while t < selftest::DRIVE_S {
            selftest::drive_input(t, &mut input);
            let dt = selftest::drive_frame_dt(frame);
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
            0x8814_0d4e_d4a2_232d,
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
    fn a_scene_setup_places_the_car_and_freezes_the_clock() {
        // The default setup is the normal start.
        let mut normal = App::new(Seed(1));
        let mut same = App::from_setup(&SceneSetup::default()).unwrap();
        assert_eq!(same.state_hash(), normal.state_hash());
        assert_eq!(
            same.scene_view(1.0, 1.5).camera.view,
            normal.scene_view(1.0, 1.5).camera.view
        );

        let setup = SceneSetup::from_query(
            "?seed=3&car=palisade_wagon&cam=cockpit&t=22:30&pos=-200,-197.5&yaw=0&lights=1",
        )
        .unwrap();
        let mut app = App::from_setup(&setup).unwrap();
        assert_eq!(app.drive.spec.id, "palisade_wagon");
        assert_eq!(app.hud_state().camera_mode, CameraMode::Cockpit);
        assert!(app.hud_state().headlights);
        let pose = app.drive.pose(1.0);
        assert!((pose.pos.x + 200.0).abs() < 1e-4 && (pose.pos.z + 197.5).abs() < 1e-4);
        assert!((pose.forward() - Vec3::NEG_Z).length() < 1e-5, "north");
        let world = app.world_instances;
        assert!(
            app.meshes[..world] == App::new(Seed(3)).meshes[..world],
            "seed 3"
        );
        // The clock stays put while the sim runs; the night needs no sun.
        let input = InputState::default();
        for _ in 0..60 {
            app.update(&input, 1.0 / 30.0);
        }
        assert_eq!(app.hud_state().time_of_day, 22.5 * 3600.0);
        assert!(app.scene_view(0.5, 1.5).sun.direction[1] < 0.0);
        // Night: the two headlight beams, then the nearest street lamps.
        let view = app.scene_view(0.5, 1.5);
        assert_eq!(view.lights.len(), 2 + lighting::STREETLAMP_LIGHTS);
        assert!(view.lights[..2].iter().all(|l| l.direction != [0.0; 3]));
        assert!(view.lights[2..]
            .iter()
            .all(|l| l.direction == [0.0, -1.0, 0.0]));
        // Parked on the street it stays parked.
        let moved = (app.drive.pose(1.0).pos - pose.pos).length();
        assert!(moved < 0.05, "{moved}");

        let bad = SceneSetup {
            car: "hovercar".into(),
            ..SceneSetup::default()
        };
        assert!(App::from_setup(&bad).is_err());
    }

    #[test]
    fn a_still_scene_renders_the_same_every_frame() {
        // Screenshot goldens rely on this: however many frames a capture takes, a still
        // scene draws bit for bit the same. (A parked car's sim jitters in the last bits,
        // enough to flip distant depth ties.)
        for query in [
            "t=17:40&cam=chase-mid&still=1",
            "t=22:30&cam=cockpit&lights=1&still=1",
        ] {
            let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
            fn view(app: &mut App, alpha: f32) -> ([[f32; 4]; 4], [f32; 3], usize) {
                let v = app.scene_view(alpha, 16.0 / 9.0);
                (v.camera.view, v.sun.direction, v.lights.len())
            }
            let first = view(&mut app, 1.0);
            let mut input = InputState::default();
            input.set(Action::Throttle, 1.0);
            for frame in 0..90 {
                let steps = app.update(&input, 1.0 / 15.0);
                assert_eq!(steps.count, 0);
                assert_eq!(view(&mut app, steps.alpha), first, "{query} frame {frame}");
            }
            assert_eq!(app.ticks(), 0);
        }
    }

    #[test]
    fn street_lamps_have_their_own_mesh_and_switch_at_dusk() {
        let mut app = App::new(Seed(1));
        let k = app.lamp_instance.expect("the test town has street lamps");
        // One light per lamp head, up on the poles, all inside the town.
        assert!(app.lamp_heads.len() >= 20, "{}", app.lamp_heads.len());
        for h in &app.lamp_heads {
            assert!((5.0..12.0).contains(&h.y), "{h}");
            assert!(h.x.abs() < 1000.0 && h.z.abs() < 1000.0, "{h}");
        }
        for (a, i) in app.lamp_heads.iter().zip(1..) {
            for b in &app.lamp_heads[i..] {
                assert!(a.distance(*b) > 2.0, "{a} and {b} are one lamp");
            }
        }
        // The lenses left the chunk meshes for the lamp mesh: all of it glows.
        let lamps = &app.meshes[k];
        assert!(lamps.vertices.iter().all(|v| {
            let (s, window) = render::emissive_scale(v.emissive);
            (s - LAMP_GLOW).abs() < 0.05 && !window
        }));
        // Golden hour: off and no lamp lights; 18:35: on, with lights.
        let golden = app.scene_view(1.0, 1.5);
        assert_eq!(golden.instances[k].glow, 0.0);
        assert!(golden.lights.is_empty());
        app.time.clock = daylight::Clock::new(lighting::DEFAULT_DAY_OF_YEAR, 18.6 * 3600.0);
        let dusk = app.scene_view(1.0, 1.5);
        assert_eq!(dusk.instances[k].glow, 1.0);
        assert_eq!(dusk.lights.len(), lighting::STREETLAMP_LIGHTS);
        // The chase camera's lamps are round the car, not across town.
        let car = Vec3::from(app.drive.state.pos);
        for l in app.scene_view(1.0, 1.5).lights {
            assert!(Vec3::from(l.position).distance(car) < 80.0);
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

    #[test]
    fn the_coast_place_loads_and_the_car_drives_on_it() {
        let setup = SceneSetup::from_query("?seed=1&place=coast").unwrap();
        let mut app = App::from_setup(&setup).unwrap();
        // Every chunk meshed, merged into few draws.
        let world = app.world_instances;
        assert!((40..=80).contains(&world), "{world} world meshes");
        assert!(app.meshes[..world]
            .iter()
            .all(|m| m.vertices.len() <= 1 << 20));
        // On the ground at the start, rolling forward along the road.
        let (start, heading) = app.start;
        let pose = app.drive.pose(1.0);
        assert!((pose.pos.x - start[0]).abs() < 1e-3 && (pose.pos.z - start[2]).abs() < 1e-3);
        let mut input = InputState::default();
        input.set(Action::Throttle, 0.5);
        for _ in 0..90 {
            app.update(&input, 1.0 / 30.0);
        }
        let now = app.drive.pose(1.0).pos;
        let (y, _) = app.ground.level_at([now.x, now.z]);
        let moved = Vec3::new(now.x - start[0], 0.0, now.z - start[2]);
        let ahead = Vec3::new(-heading.sin(), 0.0, -heading.cos());
        assert!(moved.dot(ahead) > 10.0, "{moved}");
        assert!((now.y - y).abs() < 1.0, "car at {now}, ground {y}");
    }
}
