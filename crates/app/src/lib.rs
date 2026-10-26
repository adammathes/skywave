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
mod lighttiles;
pub mod map;
mod places;
mod propinst;
pub mod scene;
pub mod selftest;
mod settings;
pub mod sound;
pub mod stream;
pub mod ui;
pub mod vehicles;
mod worldmesh;

pub use settings::{Handling, MemoryStore, Settings, SettingsStore, SETTINGS_VERSION};

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
use lighting::{Bounds, FrameLights, TimeOfDay};
use place::Place;
use render::{Camera, Mesh, MeshId, MeshInstance, PointLight, SceneView, Vertex};
use render::{ChunkPoolConfig, ChunkStore, MAX_CHUNK_PARTS};
use skywave_core::{hash64, FixedStep, Seed, Steps, SIM_DT};
use stream::{ChunkSource, PartRole, PlaceSource, StreamBudget, StreamStats, Streamer};
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
    /// The scene chose the camera (`cam=`): the saved camera setting does not replace it.
    camera_from_scene: bool,
    /// The player cycled the camera this frame; the setting remembers it.
    camera_cycled: bool,
    time: TimeOfDay,
    meshes: Vec<Mesh>,
    car_meshes: CarMeshes,
    /// The world's instances, then this frame's car instances (refilled by `scene_view`).
    instances: Vec<MeshInstance>,
    world_instances: usize,
    /// What each world instance is (street lamp lenses are switched with the photocell).
    roles: Vec<PartRole>,
    /// What each world instance covers, for light culling (`None`: lit by no lamp).
    bounds: Vec<Option<Bounds>>,
    /// The streamed town, or `None` when it was all built at load.
    stream: Option<Box<Streaming>>,
    /// This frame's lights, and the place's static ones they are picked from.
    lights: FrameLights,
    /// Interpolation alpha of the last frame, held while paused.
    alpha: f32,
    /// Menus, HUD, overlay and the user's settings and bindings.
    ui: ui::Ui,
    /// The place rasterized for the minimap and the full map.
    map: map::MapImage,
    /// Where the place puts the car: its `PlayerStart` ground point and heading.
    start: ([f32; 3], f32),
    /// A terrain place's trees and parked cars, drawn as instances near the camera.
    props: Option<propinst::PropInstances>,
    /// The sound's view of the game, and what it made of the last frame.
    sound: sound::Sound,
    audio: audio::synth::AudioParams,
}

impl App {
    /// The default car at the start of the strip.
    pub fn new(seed: Seed) -> App {
        App::new_with(seed, DEFAULT_VEHICLE).expect("the default car is built in")
    }

    /// The built-in car `vehicle_id` (see [`vehicles::BUILTIN`]) at the start of the strip,
    /// the whole town built at load.
    pub fn new_with(seed: Seed, vehicle_id: &str) -> Result<App, String> {
        App::new_in(seed, vehicle_id, PlaceKey::TestTown)
    }

    /// The built-in car `vehicle_id` at the `PlayerStart` of the place `key` names, the
    /// whole place built at load.
    pub fn new_in(seed: Seed, vehicle_id: &str, key: PlaceKey) -> Result<App, String> {
        App::build(seed, vehicle_id, key, None)
    }

    /// [`App::new_with`], the town streamed within `budget` round the camera instead
    /// (nothing is built until [`App::frame`] runs).
    pub fn new_streamed(seed: Seed, vehicle_id: &str, budget: StreamBudget) -> Result<App, String> {
        App::build(seed, vehicle_id, PlaceKey::TestTown, Some(budget))
    }

    /// The car on the place `key` names, built at load or (with `stream`) streamed.
    fn build(
        seed: Seed,
        vehicle_id: &str,
        key: PlaceKey,
        stream: Option<StreamBudget>,
    ) -> Result<App, String> {
        let spec = vehicles::builtin_spec(vehicle_id)?;
        let shape = vehicles::builtin_shape(&spec)?;
        let model = CarModel::build(&spec, &shape)?;
        let place: Place = places::build(key, seed)?;
        let start = place.player_start().ok_or("the place has no PlayerStart")?;
        let start = (start.pos, start.heading);
        let drive = Drive::with_layout(spec, model.layout, start.0, start.1);
        let map = map::MapImage::rasterize(&place);
        let mut ui = ui::Ui::new();
        ui.set_map(map::view::MapGeom::of(&map), map::labels(&place));
        let ground = drive::ground_for(&place);
        let chunk_size = place.header.chunk_size_m;
        let mut props = propinst::PropInstances::new(&place, &Palette::socal_1985(), |p| {
            [
                world::math::floor(p[0] / chunk_size) as i32,
                world::math::floor(p[1] / chunk_size) as i32,
            ]
        })?;
        let sound = sound::Sound::new(&place, &drive);
        let town = match stream {
            None => resident_town(&place),
            Some(budget) => streamed_town(place, budget),
        };
        let mut meshes = town.meshes;
        let mut instances = town.instances;
        let world_instances = instances.len();
        let car_meshes = model.register(|m| {
            meshes.push(m.clone());
            MeshId(meshes.len() as u32 - 1)
        });
        if let Some(p) = &mut props {
            let ids = p
                .meshes()
                .iter()
                .map(|m| {
                    meshes.push(m.clone());
                    MeshId(meshes.len() as u32 - 1)
                })
                .collect();
            p.set_ids(ids);
        }
        let extra = drive::MAX_CAR_INSTANCES + prop_capacity(&props);
        instances.reserve_exact(town.world_capacity - world_instances + extra);
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
            ground,
            drive,
            camera,
            camera_from_scene: false,
            camera_cycled: false,
            time: TimeOfDay::default(),
            meshes,
            car_meshes,
            instances,
            world_instances,
            roles: town.roles,
            bounds: town.bounds,
            stream: town.stream,
            lights: town.lights,
            alpha: 0.0,
            ui,
            map,
            start,
            props,
            sound,
            audio: audio::synth::AudioParams::default(),
        })
    }

    /// The scene `setup` describes: its seed and car, the car on the ground at its point
    /// and heading, its camera, its headlights, and its clock (frozen when it gives a time).
    /// A streamed scene ([`SceneSetup::stream`]) streams within [`StreamBudget::WEB`];
    /// [`App::set_stream_budget`] changes that.
    pub fn from_setup(setup: &SceneSetup) -> Result<App, String> {
        let stream = setup.stream.then_some(StreamBudget::WEB);
        let mut app = App::build(Seed(setup.seed), &setup.car, setup.place, stream)?;
        let pos = setup.pos.unwrap_or([app.start.0[0], app.start.0[2]]);
        let heading = setup.heading_rad.unwrap_or(app.start.1);
        let (y, _) = app.ground.level_at(pos);
        let ground = [pos[0], y, pos[1]];
        app.drive = Drive::with_layout(app.drive.spec.clone(), app.drive.layout, ground, heading);
        app.drive.headlights = setup.headlights;
        app.camera_from_scene = setup.camera.is_some();
        app.camera = CameraRig::new(setup.camera.unwrap_or(CameraMode::ChaseMid), &app.drive);
        app.curr = fly_from(&app.camera.pose(&app.drive, 1.0));
        app.prev = app.curr;
        app.set_fly(setup.fly);
        if let Some(t) = setup.time_s {
            app.time.clock = daylight::Clock::new(lighting::DEFAULT_DAY_OF_YEAR, t);
            app.time.clock.speed = 0.0;
            app.clock_frozen = true;
        }
        app.still = setup.still;
        if setup.map {
            app.ui.open_full_map();
        }
        if let Some(s) = setup.ui_scale {
            app.ui.force_ui_scale(s);
        }
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

    /// The map image for `render::Renderer::upload_map`, made once at load.
    pub fn map_image(&self) -> &map::MapImage {
        &self.map
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
                self.camera_cycled = true;
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
            self.sound.note_step(&self.drive.impact);
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
        let lamp_glow = lighting::streetlamp_glow(&daylight);
        for (inst, role) in self.instances.iter_mut().zip(&self.roles) {
            if *role == PartRole::Lamps {
                inst.glow = lamp_glow;
            }
        }
        let cabin = if self.fly || !self.camera.mode().first_person() {
            drive::Cabin::Outside
        } else if self.ui.settings().dashboard {
            drive::Cabin::Full
        } else {
            drive::Cabin::Frame
        };
        self.drive.push_instances(
            alpha,
            &self.car_meshes,
            cabin,
            lighting::car_glow(&daylight),
            &mut self.instances,
        );
        // The car's lamps, then every static light the photocell has on; each instance gets
        // the nearest that reach it, the car's instances those round the car.
        let (car_lights, n) = self.drive.lights(alpha);
        let eye = view.inverse().w_axis.truncate();
        self.lights
            .gather(eye, daylight.streetlights_on, &car_lights[..n]);
        let (world, car) = self.instances.split_at_mut(self.world_instances);
        self.lights.assign(&self.bounds, world);
        let reach = 0.5 * self.drive.layout.dims.iter().fold(0.0f32, |a, &b| a.max(b));
        let around = Bounds::cube(self.drive.pose(alpha).pos, reach);
        let set = lighting::light_set(self.lights.lights(), &around);
        for inst in car {
            inst.lights = set;
        }
        if let Some(props) = &self.props {
            let stream = self.stream.as_deref();
            let live = |c: [i32; 2]| stream.is_none_or(|s| s.streamer.is_live(c));
            let lights = self.lights.lights();
            props.push(eye, live, lights, lamp_glow, &mut self.instances);
        }
        SceneView {
            camera: Camera {
                view: view.to_cols_array_2d(),
                proj: proj.to_cols_array_2d(),
            },
            sun: lighting::sun(&daylight),
            sky: self.ui.video_sky(lighting::sky(&daylight)),
            instances: &self.instances,
            lights: self.lights.lights(),
            hud: self.ui.hud(),
        }
    }
}

/// Room for the instanced props a frame may draw.
fn prop_capacity(props: &Option<propinst::PropInstances>) -> usize {
    props
        .as_ref()
        .map_or(0, |p| p.count().min(propinst::MAX_PROP_INSTANCES))
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

/// The streamed town: what builds its chunks, what keeps them resident, and the chunk of
/// each of `FrameLights`' statics (a static light shines while its chunk is drawn).
struct Streaming {
    source: PlaceSource,
    streamer: Streamer,
    light_chunks: Vec<[i32; 2]>,
    /// World instances drawn whatever is resident (the far terrain), before the chunks'.
    fixed: usize,
}

/// The town an [`App`] starts with.
struct Town {
    /// Static meshes (the whole town when it is built at load; when it streams, a terrain
    /// place's far terrain).
    meshes: Vec<Mesh>,
    instances: Vec<MeshInstance>,
    roles: Vec<PartRole>,
    bounds: Vec<Option<Bounds>>,
    /// World instances there may ever be.
    world_capacity: usize,
    lights: FrameLights,
    stream: Option<Box<Streaming>>,
}

/// The whole town of `place` built at load ([`render_meshes`]): static meshes, one
/// instance each.
fn resident_town(place: &Place) -> Town {
    let chunks = world::build_chunk_meshes(place, &Palette::socal_1985());
    let place_lights: Vec<PointLight> = place
        .lights
        .items
        .iter()
        .filter_map(lighting::place_light)
        .collect();
    // Until the place lists its street lamps, they hang where the lamp heads are.
    let has_lamps = place
        .lights
        .items
        .iter()
        .any(|l| l.kind == place::LightKind::Street);
    let town = render_meshes(&chunks, place.header.chunk_size_m, place_lights, !has_lamps);
    let role = |i| {
        if Some(i) == town.glow {
            PartRole::Glow
        } else if Some(i) == town.lamps {
            PartRole::Lamps
        } else {
            PartRole::Tile
        }
    };
    let roles: Vec<PartRole> = (0..town.meshes.len()).map(role).collect();
    let instances: Vec<MeshInstance> = roles
        .iter()
        .enumerate()
        .map(|(i, &r)| world_instance(MeshId(i as u32), r))
        .collect();
    Town {
        world_capacity: instances.len(),
        meshes: town.meshes,
        instances,
        roles,
        bounds: town.bounds,
        lights: FrameLights::new(town.statics),
        stream: None,
    }
}

/// The instance drawing a world mesh or chunk part of role `role`.
fn world_instance(mesh: MeshId, role: PartRole) -> MeshInstance {
    MeshInstance {
        color: if role == PartRole::Glow {
            [GLOW_BOOST, GLOW_BOOST, GLOW_BOOST, 1.0]
        } else {
            [1.0; 4]
        },
        ..MeshInstance::new(mesh, Mat4::IDENTITY.to_cols_array_2d())
    }
}

/// `place` to be streamed within `budget`: nothing built yet but a terrain place's far
/// terrain (drawn always, under the chunks, out to the horizon), every static light known
/// (and off until its chunk is drawn), room for every part the resident chunks can have.
fn streamed_town(place: Place, budget: StreamBudget) -> Town {
    let source = PlaceSource::new(place, Palette::socal_1985());
    let streamer = Streamer::new(&source, budget);
    let statics = source.static_lights();
    let mut lights = FrameLights::new(statics.iter().map(|s| s.light).collect());
    lights.set_active(|_| false);
    let light_chunks = statics.iter().map(|s| s.chunk).collect();
    let meshes: Vec<Mesh> = source.far_terrain().cloned().into_iter().collect();
    let fixed = meshes.len();
    let capacity = fixed + streamer.capacity() * MAX_CHUNK_PARTS;
    let mut instances = Vec::with_capacity(capacity);
    let mut roles = Vec::with_capacity(capacity);
    let mut bounds = Vec::with_capacity(capacity);
    for i in 0..fixed {
        instances.push(world_instance(MeshId(i as u32), PartRole::Tile));
        roles.push(PartRole::Tile);
        bounds.push(None);
    }
    Town {
        meshes,
        instances,
        roles,
        bounds,
        world_capacity: capacity,
        lights,
        stream: Some(Box::new(Streaming {
            source,
            streamer,
            light_chunks,
            fixed,
        })),
    }
}

impl App {
    /// Streams within `budget` from now on (a platform with more room than
    /// [`StreamBudget::WEB`] calls this before its first frame and before sizing the
    /// renderer's chunk pool). Does nothing when the town is built at load.
    pub fn set_stream_budget(&mut self, budget: StreamBudget) {
        let Some(s) = &mut self.stream else {
            return;
        };
        debug_assert_eq!(
            s.streamer.stats().built,
            0,
            "set the budget before streaming"
        );
        s.streamer = Streamer::new(&s.source, budget);
        // Nothing is drawn yet, so these are empty: room for every part and the car.
        let parts = s.streamer.capacity() * MAX_CHUNK_PARTS;
        let extra = drive::MAX_CAR_INSTANCES + prop_capacity(&self.props);
        self.instances.reserve_exact(parts + extra);
        self.roles.reserve_exact(parts);
        self.bounds.reserve_exact(parts);
    }

    /// The renderer's chunk pool this app needs (`RendererOptions::chunk_pool`); empty
    /// when the town is built at load.
    pub fn chunk_pool_config(&self) -> ChunkPoolConfig {
        self.stream
            .as_ref()
            .map_or(ChunkPoolConfig::NONE, |s| s.streamer.budget().pool_config())
    }

    /// Most instances a frame can draw: reserve them in the renderer at load.
    pub fn max_instances(&self) -> usize {
        let extra = drive::MAX_CAR_INSTANCES + prop_capacity(&self.props);
        self.instances.capacity().max(self.world_instances + extra)
    }

    /// Whether the town is streamed.
    pub fn is_streamed(&self) -> bool {
        self.stream.is_some()
    }

    /// Nothing left to stream: every chunk round the camera is drawn. Always true for a
    /// town built at load.
    pub fn stream_idle(&self) -> bool {
        self.stream.as_ref().is_none_or(|s| s.streamer.is_idle())
    }

    /// Streaming counters, when the town is streamed.
    pub fn stream_stats(&self) -> Option<StreamStats> {
        self.stream.as_ref().map(|s| s.streamer.stats())
    }

    /// Runs `steps` streaming steps round the current view; `now` is the platform's clock
    /// in seconds.
    fn stream_steps(&mut self, steps: u32, alpha: f32, now: &dyn Fn() -> f64) {
        let (eye, forward) = if self.fly {
            let c = self.prev.lerp(&self.curr, alpha);
            let (s, co) = c.yaw.sin_cos();
            (c.pos, Vec3::new(-s, 0.0, -co))
        } else {
            let pose = self.camera.pose(&self.drive, alpha);
            (pose.eye, pose.forward())
        };
        let Some(s) = &mut self.stream else {
            return;
        };
        for _ in 0..steps {
            s.streamer
                .step(&s.source, [eye.x, eye.z], [forward.x, forward.z], now);
        }
    }

    /// Writes the chunks built since the last call into `gpu` (the renderer's chunk pool)
    /// and frees the evicted ones', then refreshes the world's instances and which static
    /// lights shine. Call once a frame, after [`App::frame`] and before
    /// [`App::scene_view`]. Allocates nothing.
    pub fn sync_chunks(&mut self, gpu: &mut impl ChunkStore) {
        let Some(s) = &mut self.stream else {
            return;
        };
        if !s.streamer.sync(gpu) {
            return;
        }
        self.instances.truncate(s.fixed);
        self.roles.truncate(s.fixed);
        self.bounds.truncate(s.fixed);
        for (slot, parts) in s.streamer.drawn() {
            for (k, p) in parts.iter().enumerate() {
                self.instances
                    .push(world_instance(MeshId::chunk_part(slot, k), p.role));
                self.roles.push(p.role);
                self.bounds.push(p.bounds);
            }
        }
        self.world_instances = self.instances.len();
        let (streamer, chunks) = (&s.streamer, &s.light_chunks);
        self.lights.set_active(|i| streamer.is_live(chunks[i]));
    }
}

/// The town as the renderer draws it ([`render_meshes`]).
struct TownMeshes {
    /// Draw order: the light tiles of the chunks, then the glow mesh and the street lamp
    /// heads (each when there is any).
    meshes: Vec<Mesh>,
    /// What each mesh covers, for light culling; `None` for the glow and lamp meshes,
    /// which glow by themselves (and span the town).
    bounds: Vec<Option<Bounds>>,
    /// Index of the glow mesh (drawn with a boosted instance color) and of the lamp heads'
    /// mesh (switched by the instance's glow).
    glow: Option<usize>,
    lamps: Option<usize>,
    /// The town's static lights: `place_lights`, then a street lamp at each lamp head
    /// when asked to derive them.
    statics: Vec<PointLight>,
}

/// Converts world meshes (patterned by [`worldmesh::atlas_mesh`]) to the renderer's vertex
/// format: each chunk's meshes without their street lamp lenses (lit windows slightly
/// tinted) as one, cut into light tiles ([`lighttiles`]) where more static lights reach it
/// than a draw takes, then one mesh holding all neon and signal triangles colored by their
/// glow, drawn with a boosted instance color, then one holding every street lamp lens,
/// which the photocell switches. The lamp heads are where the lenses cluster; with
/// `derive_lamps` each gets a street lamp among the static lights.
fn render_meshes(
    chunks: &[ChunkMesh],
    chunk_size: f32,
    place_lights: Vec<PointLight>,
    derive_lamps: bool,
) -> TownMeshes {
    let (mut glow, mut lamps) = (Mesh::default(), Mesh::default());
    let mut bases = Vec::with_capacity(chunks.len());
    for c in chunks {
        let mesh = worldmesh::atlas_mesh(&c.mesh, c.kind);
        bases.push(convert(&mesh, Part::Base));
        lighttiles::append(&mut glow, &convert(&mesh, Part::Glow));
        lighttiles::append(&mut lamps, &convert(&mesh, Part::Lamps));
    }
    let lamp_heads = lamp_heads(&lamps);
    let mut statics = place_lights;
    if derive_lamps {
        statics.extend(lamp_heads.iter().map(|&h| lighting::streetlamp(h)));
    }
    // Meshes no static light reaches (the terrain apron) are drawn as they are; the rest go
    // into one mesh per chunk, cut into light tiles.
    let mut tiles = Vec::new();
    let mut cells = std::collections::BTreeMap::new();
    for base in bases {
        let Some(b) = lighttiles::mesh_bounds(&base).filter(|_| !base.indices.is_empty()) else {
            continue;
        };
        if statics.iter().any(|l| b.reached_by(l)) {
            lighttiles::bin_by_cells(base, chunk_size, &mut cells);
        } else {
            tiles.push(lighttiles::Tile {
                mesh: base,
                bounds: b,
            });
        }
    }
    for ([z, x], mesh) in cells {
        let origin = [x as f32 * chunk_size, z as f32 * chunk_size];
        lighttiles::split_for_lights(mesh, origin, chunk_size, &statics, &mut tiles);
    }
    let mut meshes = Vec::with_capacity(tiles.len() + 2);
    let mut bounds = Vec::with_capacity(tiles.len() + 2);
    for t in tiles {
        meshes.push(t.mesh);
        bounds.push(Some(t.bounds));
    }
    let mut push = |m: Mesh| {
        (!m.indices.is_empty()).then(|| {
            meshes.push(m);
            bounds.push(None);
            meshes.len() - 1
        })
    };
    let glow = push(glow);
    let lamps = push(lamps);
    TownMeshes {
        meshes,
        bounds,
        glow,
        lamps,
        statics,
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

    /// Seconds since the first call: the platform clock tests pass to [`App::frame`].
    pub(crate) fn wall_clock() -> f64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs_f64()
    }

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
            0x3bd9_240f_f5b3_05d0,
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
        // Headlights on: two pools of light ahead, the tail lamps' coronas and the lit
        // lamps drawn; the car is lit by its own beams.
        let lamps = app.car_meshes.lods[0].lights[1];
        let world = app.world_instances;
        let view = app.scene_view(0.5, 1.5);
        assert_eq!(view.lights.len(), 4);
        assert!(view.instances.iter().any(|i| lamps.contains(&i.mesh)));
        assert!(view.instances[world..]
            .iter()
            .all(|i| i.lights.indices() == [0, 1]));
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
            "?seed=3&car=palisade_wagon&cam=cockpit&t=22:30&pos=-200,-197.5&yaw=0&lights=1&stream=0",
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
        // Night: the two headlight beams, the tail lamps, then street lamps up to the
        // frame's limit (the town has more).
        let view = app.scene_view(0.5, 1.5);
        assert_eq!(view.lights.len(), render::MAX_LIGHTS);
        assert!(view.lights[..2].iter().all(|l| l.direction != [0.0; 3]));
        assert!(view.lights[2..4].iter().all(|l| l.radius == 0.0));
        assert!(view.lights[4..]
            .iter()
            .all(|l| l.direction == [0.0, -1.0, 0.0] && l.corona.radius > 0.0));
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
        let k = app.roles.iter().position(|r| *r == PartRole::Lamps);
        let k = k.expect("the test town has street lamps");
        // One light per lamp head, up on the poles, all inside the town.
        let heads: Vec<Vec3> = app
            .lights
            .statics()
            .iter()
            .map(|l| Vec3::from(l.position))
            .collect();
        assert!(heads.len() >= 300, "{}", heads.len());
        for h in &heads {
            assert!((5.0..12.0).contains(&h.y), "{h}");
            assert!(h.x.abs() < 1000.0 && h.z.abs() < 1000.0, "{h}");
        }
        for (a, i) in heads.iter().zip(1..) {
            for b in &heads[i..] {
                assert!(a.distance(*b) > 2.0, "{a} and {b} are one lamp");
            }
        }
        // The lenses left the chunk meshes for the lamp mesh: all of it glows.
        let lamps = &app.meshes[k];
        assert!(lamps.vertices.iter().all(|v| {
            let (s, window) = render::emissive_scale(v.emissive);
            (s - LAMP_GLOW).abs() < 0.05 && !window
        }));
        // Golden hour: off and no lamp lights; 18:35: on, as many as the frame holds, the
        // ones round the camera at full strength.
        let golden = app.scene_view(1.0, 1.5);
        assert_eq!(golden.instances[k].glow, 0.0);
        assert!(golden.lights.is_empty());
        assert!(golden.instances.iter().all(|i| i.lights.is_empty()));
        app.time.clock = daylight::Clock::new(lighting::DEFAULT_DAY_OF_YEAR, 18.6 * 3600.0);
        let dusk = app.scene_view(1.0, 1.5);
        assert_eq!(dusk.instances[k].glow, 1.0);
        assert_eq!(dusk.lights.len(), heads.len().min(render::MAX_LIGHTS));
        let eye = Mat4::from_cols_array_2d(&dusk.camera.view)
            .inverse()
            .w_axis
            .truncate();
        let lit: Vec<Vec3> = dusk.lights.iter().map(|l| Vec3::from(l.position)).collect();
        for h in heads.iter().filter(|h| h.distance(eye) < 200.0) {
            let l = dusk.lights.iter().find(|l| Vec3::from(l.position) == *h);
            let l = l.unwrap_or_else(|| panic!("lamp at {h} is off"));
            assert_eq!(
                l.color,
                lighting::streetlamp(*h).color,
                "{h} at full strength"
            );
        }
        assert!(lit.iter().all(|p| p.distance(eye) < 600.0));
    }

    /// The point under every lamp round the camera is in a tile whose light set holds that
    /// lamp, so each lamp pools on the road; few lights reaching a tile are left out of its
    /// set. The culling allocates nothing per frame.
    #[test]
    fn every_lamp_round_the_camera_pools_on_the_tiles_under_it() {
        let setup =
            SceneSetup::from_query("t=22:30&cam=cockpit&lights=1&still=1&stream=0").unwrap();
        let mut app = App::from_setup(&setup).unwrap();
        lamps_pool_under_themselves(&mut app);
        // Tiles that fit their lights, and still fewer draws than the 372 meshes the town
        // was as one per chunk and kind.
        assert!(app.world_instances < 372, "{}", app.world_instances);
    }

    /// The scene streamed: once its ring is filled, the same holds, the static lights are
    /// the same ones, and filling it allocated nothing in the app's own lists.
    #[test]
    fn a_streamed_town_fills_its_ring_and_its_lamps_pool_the_same() {
        let query = "t=22:30&cam=cockpit&lights=1&still=1";
        let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
        assert!(app.is_streamed() && app.world_instances == 0 && app.meshes().len() < 40);
        assert!(app.lights.lights().is_empty());
        let mut pool = render::ChunkPool::new(app.chunk_pool_config());
        let mut input = InputMapper::default();
        let caps = (
            app.instances.capacity(),
            app.bounds.capacity(),
            app.roles.capacity(),
        );
        assert!(app.max_instances() >= caps.0);
        let mut frames = 0;
        while !app.stream_idle() {
            app.frame(&mut input, 1.0 / 60.0, &wall_clock);
            app.sync_chunks(&mut pool);
            frames += 1;
            assert!(frames < 20_000, "{:?}", app.stream_stats());
        }
        let stats = app.stream_stats().unwrap();
        eprintln!(
            "streamed in {frames} frames: {stats:?}; {} world instances; pool {:?}",
            app.world_instances,
            pool.stats()
        );
        assert_eq!(
            (
                app.instances.capacity(),
                app.bounds.capacity(),
                app.roles.capacity()
            ),
            caps
        );
        assert_eq!(stats.upload_errors, 0);
        // The 9×9 ring round the start holds the whole town but the apron's far edges.
        assert!(stats.built >= 81 - 9, "{stats:?}");
        let resident =
            App::from_setup(&SceneSetup::from_query(&format!("{query}&stream=0")).unwrap())
                .unwrap();
        let key = |l: &PointLight| l.position.map(f32::to_bits);
        let mut a: Vec<_> = app.lights.statics().iter().map(key).collect();
        let mut b: Vec<_> = resident.lights.statics().iter().map(key).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "the same lamps, bit for bit");
        lamps_pool_under_themselves(&mut app);
        // Glow and lamp parts are drawn as in the resident town.
        assert!(app.roles.contains(&PartRole::Glow) && app.roles.contains(&PartRole::Lamps));
    }

    fn lamps_pool_under_themselves(app: &mut App) {
        let world = app.world_instances;
        let caps = (app.instances.capacity(), app.lights.order_capacity());
        let started = std::time::Instant::now();
        let frames = 20;
        for _ in 0..frames {
            app.scene_view(1.0, 16.0 / 9.0);
        }
        let per_frame = started.elapsed() / frames;
        let bounds = app.bounds.clone();
        // The culling alone: every world instance against every light.
        let mut instances = app.instances.clone();
        let started = std::time::Instant::now();
        for _ in 0..frames {
            app.lights.assign(&bounds, &mut instances[..world]);
        }
        let culling = started.elapsed() / frames;
        let view = app.scene_view(1.0, 16.0 / 9.0);
        let eye = Mat4::from_cols_array_2d(&view.camera.view)
            .inverse()
            .w_axis
            .truncate();
        let (mut pairs, mut dropped) = (0, 0);
        for (inst, b) in view.instances[..world].iter().zip(&bounds) {
            let Some(b) = b else { continue };
            for (i, l) in view.lights.iter().enumerate() {
                if b.reached_by(l) {
                    pairs += 1;
                    dropped += usize::from(!inst.lights.indices().contains(&(i as u8)));
                }
            }
        }
        eprintln!(
            "{} instances, {} lights: {pairs} light-tile pairs, {dropped} left out; scene_view {per_frame:?}, culling {culling:?}",
            view.instances.len(),
            view.lights.len()
        );
        assert!(dropped * 50 < pairs, "{dropped} of {pairs}");
        for (i, l) in view.lights.iter().enumerate().skip(4) {
            let h = Vec3::from(l.position);
            if h.distance(eye) > 250.0 {
                continue;
            }
            let ground = Vec3::new(h.x, 0.0, h.z);
            let under = view.instances[..world]
                .iter()
                .zip(&bounds)
                .filter(|(_, b)| b.is_some_and(|b| b.distance_squared(ground) == 0.0));
            let mut any = false;
            for (inst, _) in under {
                any = true;
                assert!(inst.lights.indices().contains(&(i as u8)), "lamp at {h}");
            }
            assert!(any, "nothing drawn under the lamp at {h}");
        }
        assert_eq!(
            (app.instances.capacity(), app.lights.order_capacity()),
            caps
        );
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
        // Built at load: every chunk of the 16×16-chunk place meshed, a draw for each kind
        // of mesh it has (ground, roads, paint), and no far terrain.
        let query = "?seed=1&place=coast";
        let setup = SceneSetup::from_query(&format!("{query}&stream=0")).unwrap();
        let resident = App::from_setup(&setup).unwrap();
        let world = resident.world_instances;
        eprintln!("coast at load: {world} world meshes");
        // Ground, roads, paint and buildings per chunk: at most five meshes a chunk.
        assert!((256..=1280).contains(&world), "{world} world meshes");
        assert!(resident.meshes[..world]
            .iter()
            .all(|m| m.vertices.len() <= 1 << 20));
        // Streamed (the default): the far terrain drawn from the start, under the ring round
        // the start as it fills, every chunk within budget.
        let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
        assert!(app.is_streamed() && app.world_instances == 1);
        let far = app.meshes()[0].indices.len() / 3;
        assert!(
            (8192..=10_000).contains(&far),
            "far terrain: {far} triangles"
        );
        // On the ground at the start.
        let (start, heading) = app.start;
        let pose = app.drive.pose(1.0);
        assert!((pose.pos.x - start[0]).abs() < 1e-3 && (pose.pos.z - start[2]).abs() < 1e-3);
        let mut pool = render::ChunkPool::new(app.chunk_pool_config());
        let mut mapper = InputMapper::default();
        let mut frames = 0;
        while !app.stream_idle() {
            app.frame(&mut mapper, 1.0 / 60.0, &wall_clock);
            app.sync_chunks(&mut pool);
            frames += 1;
            assert!(frames < 20_000, "{:?}", app.stream_stats());
        }
        let stats = app.stream_stats().unwrap();
        eprintln!(
            "coast streamed in {frames} frames: {stats:?}; {} world instances",
            app.world_instances
        );
        assert_eq!(stats.upload_errors, 0);
        assert!(stats.built >= 25, "{stats:?}");
        assert!(app.world_instances > 1 && app.world_instances < world);
        assert_eq!(
            app.instances[0].mesh,
            MeshId(0),
            "the far terrain stays drawn"
        );
        // Rolling forward along the road from the start, on the ground.
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
