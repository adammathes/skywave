//! Skywave's game loop logic, independent of any window or browser.
//!
//! The platform crate feeds raw input events to an [`InputMapper`], calls [`App::frame`]
//! once per frame with the real frame time (it samples the mapper, runs the menus and
//! [`ui`], and calls [`App::update`] unless a menu has the game paused), and draws
//! [`App::scene_view`]. The simulation itself only ever advances in fixed `SIM_DT` steps:
//! the car ([`drive`]), then its cameras ([`camera`]), then the clock ([`lighting`]).
//!
//! **Switching cars** ([`App::switch_vehicle`], the pause menu's Vehicle page, the
//! `VehicleNext` key): a car's meshes are built and registered the first time it is driven
//! and kept, so switching back reuses its ids. Meshes registered after load wait in
//! [`App::take_new_meshes`], which the platform drains into the renderer every frame
//! before drawing; the ids continue after [`App::meshes`]. A session registers at most
//! one set of meshes per built-in car, plus the paint-dependent ones (bodies and cabin
//! walls) for each further paint it drives or shows on the Vehicle page's turntable
//! ([`preview`]), which draws from the same ids.

pub mod camera;
pub mod car;
pub mod daylight;
pub mod drive;
mod input;
pub mod lighting;
mod lighttiles;
pub mod map;
pub mod npc;
pub mod pacing;
mod peds;
mod places;
pub mod preview;
mod propinst;
pub mod radio;
pub mod scene;
pub mod selftest;
mod settings;
pub mod sound;
pub mod stream;
pub mod ui;
pub mod vehicles;
mod worldmesh;

pub use settings::{
    FrameRate, GpuInfo, Handling, MemoryStore, Quality, Settings, SettingsStore, SETTINGS_VERSION,
};

pub use camera::CameraMode;
pub use drive::{HudState, DEFAULT_VEHICLE};
pub use scene::{PlaceKey, SceneSetup};

pub use input::{
    pad_table, Action, AxisOptions, Binding, Bindings, BindingsError, Conflict, Context,
    DeviceKind, InputMapper, InputState, Key, MouseAxis, PadAxis, PadButton, RawInput, Sign,
    BINDINGS_VERSION, MOUSE_BUTTONS, MOUSE_LOOK_PER_PIXEL, MOUSE_RIGHT,
};

use camera::CameraRig;
use car::{CarLayout, CarMeshes, CarModel};
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
    /// The place's lanes, where a reset puts the car.
    lanes: world::lanes::LaneLines,
    camera: CameraRig,
    /// The scene chose the camera (`cam=`): the saved camera setting does not replace it.
    camera_from_scene: bool,
    /// The player cycled the camera this frame; the setting remembers it.
    camera_cycled: bool,
    time: TimeOfDay,
    meshes: Vec<Mesh>,
    car_meshes: CarMeshes,
    /// Every car and paint registered so far (the one driven among them), for switching
    /// back and for the turntable.
    cars: Vec<RegisteredCar>,
    /// The Vehicle page's turntable this frame: the car and its stage (virtual HUD units).
    preview: Option<(RegisteredCar, [f32; 4])>,
    /// The turntable's disc and its turn, radians.
    turntable: MeshId,
    spin: f32,
    /// The instances the turntable added in the last [`App::scene_view`].
    preview_drawn: core::ops::Range<usize>,
    /// Meshes registered since the platform last called [`App::take_new_meshes`].
    pending_meshes: Vec<Mesh>,
    /// Mesh ids handed out: `meshes`, then every pending batch.
    mesh_count: u32,
    /// The scene chose the car (`car=`): the saved vehicle setting does not replace it.
    car_from_scene: bool,
    /// The scene chose a paint but no car (`paint=` alone): it paints the saved car too.
    paint_from_scene: Option<usize>,
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
    /// The place's NPC traffic.
    npcs: npc::Npcs,
    /// The scene's traffic density over the setting (`traffic=`; off unless a scene
    /// says otherwise, so tests and the self-test drive alone).
    traffic_scene: Option<traffic::Density>,
    /// The sound's view of the game, and what it made of the last frame.
    sound: sound::Sound,
    audio: audio::synth::AudioParams,
    /// Width over height of the last frame drawn (for placing the dash's radio text).
    aspect: f32,
    /// The target's size in pixels, if the platform said ([`App::set_target_size`]).
    target: Option<(u32, u32)>,
    /// Seconds the deep-water toast has left, and the car's water resets it has shown.
    water_toast: f32,
    water_resets_seen: u32,
    /// The ui's HUD with the deep-water toast over it, on the frames that show it.
    overlay: render::HudBuilder,
}

/// Seconds the deep-water toast stays up.
const WATER_TOAST_S: f32 = 2.5;
/// What it says.
const WATER_TOAST: &str = "Deep water: resetting";
/// Its color (the ui's accent) and its drop shadow.
const TOAST_RGBA: [u8; 4] = [255, 178, 62, 255];
const TOAST_SHADOW: [u8; 4] = [0, 0, 0, 170];

/// A car in one paint whose meshes the renderer has (or will have, once drained).
#[derive(Clone, Copy, Debug)]
struct RegisteredCar {
    id: &'static str,
    paint: usize,
    meshes: CarMeshes,
    layout: CarLayout,
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
        App::build(seed, vehicle_id, 0, key, None)
    }

    /// [`App::new_with`], the town streamed within `budget` round the camera instead
    /// (nothing is built until [`App::frame`] runs).
    pub fn new_streamed(seed: Seed, vehicle_id: &str, budget: StreamBudget) -> Result<App, String> {
        App::build(seed, vehicle_id, 0, PlaceKey::TestTown, Some(budget))
    }

    /// The car in palette entry `paint` on the place `key` names, built at load or (with
    /// `stream`) streamed.
    fn build(
        seed: Seed,
        vehicle_id: &str,
        paint: usize,
        key: PlaceKey,
        stream: Option<StreamBudget>,
    ) -> Result<App, String> {
        let spec = vehicles::builtin_spec(vehicle_id)?;
        let vehicle_index = ui::roster::index_of(vehicle_id).ok_or("not a built-in car")?;
        let shape = vehicles::builtin_shape(&spec)?;
        if paint >= shape.palette.len() {
            return Err(format!("`{vehicle_id}` has no paint {paint}"));
        }
        let model = CarModel::build_painted(&spec, &shape, paint)?;
        let place: Place = places::build(key, seed)?;
        let start = place.player_start().ok_or("the place has no PlayerStart")?;
        let start = (start.pos, start.heading);
        let mut drive = Drive::with_layout(spec, model.layout, start.0, start.1);
        let map = map::MapImage::rasterize(&place);
        let mut ui = ui::Ui::new();
        drive.auto_headlights = ui.settings().headlights_auto;
        ui.vehicle_switched(vehicle_index, paint, false, false);
        ui.set_map(map::view::MapGeom::of(&map), map::labels(&place));
        let ground = drive::ground_for(&place);
        let lanes = world::lanes::LaneLines::of(&place);
        let chunk_size = place.header.chunk_size_m;
        let mut props = propinst::PropInstances::new(&place, &Palette::socal_1985(), |p| {
            [
                world::math::floor(p[0] / chunk_size) as i32,
                world::math::floor(p[1] / chunk_size) as i32,
            ]
        })?;
        let sound = sound::Sound::new(&place, &drive);
        let npcs = npc::Npcs::new(&place, seed);
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
        meshes.push(preview::turntable_mesh());
        let turntable = MeshId(meshes.len() as u32 - 1);
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
        let extra = drive::MAX_CAR_INSTANCES
            + preview::MAX_INSTANCES
            + prop_capacity(&props)
            + npcs.capacity();
        instances.reserve_exact(town.world_capacity - world_instances + extra);
        let camera = CameraRig::new(CameraMode::ChaseMid, &drive);
        let fly = fly_from(&camera.pose(&drive, 1.0));
        let cars = vec![RegisteredCar {
            id: vehicles::BUILTIN[vehicle_index].0,
            paint,
            meshes: car_meshes,
            layout: model.layout,
        }];
        let mesh_count = meshes.len() as u32;
        Ok(App {
            clock: FixedStep::new(),
            ticks: 0,
            fly: false,
            still: false,
            clock_frozen: false,
            prev: fly,
            curr: fly,
            ground,
            lanes,
            drive,
            camera,
            camera_from_scene: false,
            camera_cycled: false,
            time: TimeOfDay::default(),
            meshes,
            car_meshes,
            cars,
            preview: None,
            turntable,
            spin: preview::STILL_SPIN_RAD,
            preview_drawn: 0..0,
            pending_meshes: Vec::new(),
            mesh_count,
            car_from_scene: false,
            paint_from_scene: None,
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
            npcs,
            traffic_scene: Some(traffic::Density::Off),
            sound,
            audio: audio::synth::AudioParams::default(),
            aspect: 16.0 / 9.0,
            target: None,
            water_toast: 0.0,
            water_resets_seen: 0,
            overlay: render::HudBuilder::new(),
        })
    }

    /// The scene `setup` describes: its seed and car, the car on the ground at its point
    /// and heading, its camera, its headlights, and its clock (frozen when it gives a time).
    /// A streamed scene ([`SceneSetup::stream`]) streams within [`StreamBudget::WEB`];
    /// [`App::set_stream_budget`] changes that.
    pub fn from_setup(setup: &SceneSetup) -> Result<App, String> {
        let stream = setup.stream.then_some(StreamBudget::WEB);
        // A paint for a named car must be one of its own; without a car it paints whichever
        // car the game starts in (see `set_settings_store`).
        let paint = match (setup.car_given, setup.paint) {
            (true, paint) => paint.unwrap_or(0),
            (false, paint) => {
                ui::roster::index_of(&setup.car).map_or(0, |i| paint_of(i, paint.unwrap_or(0)))
            }
        };
        let mut app = App::build(Seed(setup.seed), &setup.car, paint, setup.place, stream)?;
        app.paint_from_scene = setup.paint.filter(|_| !setup.car_given);
        let pos = setup.pos.unwrap_or([app.start.0[0], app.start.0[2]]);
        let heading = setup.heading_rad.unwrap_or(app.start.1);
        let (y, _) = app.ground.level_at(pos);
        let ground = [pos[0], y, pos[1]];
        app.drive = Drive::with_layout(app.drive.spec.clone(), app.drive.layout, ground, heading);
        app.drive.headlights = setup.headlights;
        app.camera_from_scene = setup.camera.is_some();
        app.car_from_scene = setup.car_given;
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
        app.drive.auto_headlights = app.ui.settings().headlights_auto;
        // A scene that switches them on keeps them on until the next dusk or dawn.
        if setup.headlights {
            app.drive.auto_dark = Some(app.traffic_clock().dark(0.0));
        }
        if setup.map {
            app.ui.open_full_map();
        }
        if let Some(s) = setup.ui_scale {
            app.ui.force_ui_scale(s);
        }
        // A scene's density applies now; the setting's once the settings are loaded.
        app.traffic_scene = setup.traffic_density();
        if let Some(t) = setup.phase {
            app.npcs.set_signal_tick(t);
        }
        if let Some(d) = app.traffic_scene {
            app.apply_traffic(d);
            app.finish_warm_up();
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

    /// Meshes registered since load or the last call (a car driven for the first time),
    /// in id order: upload them after [`App::meshes`] and every earlier batch, so ids keep
    /// counting up from `meshes().len()`. Call once a frame, after [`App::frame`] and
    /// before [`App::scene_view`]; it is empty (and costs nothing) almost always.
    pub fn take_new_meshes(&mut self) -> std::vec::Drain<'_, Mesh> {
        self.pending_meshes.drain(..)
    }

    /// Mesh ids handed out so far: [`App::meshes`] and every batch of
    /// [`App::take_new_meshes`], taken or not.
    pub fn mesh_count(&self) -> usize {
        self.mesh_count as usize
    }

    /// Swaps in the built-in car `vehicle_id` (see [`vehicles::BUILTIN`]) in its first
    /// paint where the car is now ([`Drive::switch_car`]), keeping the camera and the
    /// headlights, and remembers it in the settings; its name shows on the HUD. Its meshes
    /// are built and queued for [`App::take_new_meshes`] the first time; later switches
    /// reuse them.
    pub fn switch_vehicle(&mut self, vehicle_id: &str) -> Result<(), String> {
        let index = ui::roster::index_of(vehicle_id).ok_or(format!("no vehicle `{vehicle_id}`"))?;
        self.switch_vehicle_at(index, 0, true, true)
    }

    /// `VehicleNext`: the next car in [`vehicles::BUILTIN`], wrapping after the last, in
    /// its first paint.
    pub fn cycle_vehicle(&mut self) {
        let next = (self.ui.vehicle() + 1) % vehicles::BUILTIN.len();
        self.switch_to(next, 0, true);
    }

    /// `Reset`: the car back on the nearest lane, at rest ([`Drive::reset`]), the camera
    /// behind it, and a "Car reset" toast. Nothing while the free-fly camera flies.
    pub fn reset_car(&mut self) {
        if self.fly {
            return;
        }
        self.drive.reset(&self.lanes, &self.ground);
        self.settle_camera();
        self.ui.car_reset();
    }

    /// The same camera, settled behind (or in) the car where it now is.
    fn settle_camera(&mut self) {
        let mut rig = CameraRig::new(self.camera.selected(), &self.drive);
        rig.tuning = self.camera.tuning;
        rig.prefs = self.camera.prefs;
        self.camera = rig;
    }

    /// [`App::switch_vehicle_at`]; a car that fails to build (never a built-in one, whose
    /// specs and shapes are tested) leaves the current one.
    fn switch_to(&mut self, index: usize, paint: usize, toast: bool) {
        let switched = self.switch_vehicle_at(index, paint, toast, true);
        debug_assert!(switched.is_ok(), "{switched:?}");
    }

    /// Switches to `vehicles::BUILTIN[index]` in palette entry `paint` (its first when it
    /// has no such entry), toasting its name or not and remembering it in the settings or
    /// not. The car already driven stays, and only the paint, the toast and the setting
    /// happen.
    fn switch_vehicle_at(
        &mut self,
        index: usize,
        paint: usize,
        toast: bool,
        remember: bool,
    ) -> Result<(), String> {
        let paint = paint_of(index, paint);
        let car = self.registered(index, paint)?;
        if self.drive.spec.id != car.id {
            let spec = vehicles::builtin_spec(car.id)?;
            self.drive.switch_car(spec, car.layout, &self.ground);
            self.sound.set_car(&self.drive);
            self.settle_camera();
        }
        self.car_meshes = car.meshes;
        self.ui.vehicle_switched(index, paint, remember, toast);
        Ok(())
    }

    /// `vehicles::BUILTIN[index]` in palette entry `paint`, its meshes registered (queued
    /// for [`App::take_new_meshes`]) the first time it is asked for: the whole car, or for a
    /// further paint of a car already registered only its bodies and cabin walls.
    fn registered(&mut self, index: usize, paint: usize) -> Result<RegisteredCar, String> {
        let &(id, _) = vehicles::BUILTIN.get(index).ok_or("no such vehicle")?;
        if let Some(car) = self.cars.iter().find(|c| c.id == id && c.paint == paint) {
            return Ok(*car);
        }
        let spec = vehicles::builtin_spec(id)?;
        let model = CarModel::build_painted(&spec, &vehicles::builtin_shape(&spec)?, paint)?;
        let (pending, mut next) = (&mut self.pending_meshes, self.mesh_count);
        let mut add = |m: &Mesh| {
            pending.push(m.clone());
            next += 1;
            MeshId(next - 1)
        };
        let meshes = match self.cars.iter().find(|c| c.id == id) {
            Some(base) => model.register_paint(&base.meshes, &mut add),
            None => model.register(&mut add),
        };
        self.mesh_count = next;
        let car = RegisteredCar {
            id,
            paint,
            meshes,
            layout: model.layout,
        };
        self.cars.push(car);
        Ok(car)
    }

    /// The car and paint the Vehicle page shows, registered (see [`App::registered`]) and
    /// put on the turntable for [`App::scene_view`]; the turntable turns `dt` seconds (not
    /// in a still scene, which shows [`preview::STILL_SPIN_RAD`]). Nothing when the page is
    /// not up.
    fn update_preview(&mut self, dt: f32) {
        self.preview = match self.ui.preview() {
            Some((index, paint, stage)) => {
                let car = self.registered(index, paint_of(index, paint));
                debug_assert!(car.is_ok(), "{car:?}");
                car.ok().map(|car| (car, stage))
            }
            None => None,
        };
        if self.preview.is_some() && !self.still {
            self.spin =
                (self.spin + dt * preview::SPIN_RAD_PER_S).rem_euclid(core::f32::consts::TAU);
        }
    }

    /// Instances of the turntable in the last [`App::scene_view`] (empty when the Vehicle
    /// page is not up).
    pub fn preview_instances(&self) -> core::ops::Range<usize> {
        self.preview_drawn.clone()
    }

    /// Simulation steps run so far.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Advances real time by `dt_real` seconds, running as many fixed steps as are due.
    /// Pass the returned `alpha` to [`App::scene_view`].
    pub fn update(&mut self, input: &InputState, dt_real: f64) -> Steps {
        self.advance(input, dt_real, &|| 0.0)
    }

    /// [`App::update`], timing the traffic sim by `now` (seconds, any origin) for the
    /// overlay.
    fn advance(&mut self, input: &InputState, dt_real: f64, now: &dyn Fn() -> f64) -> Steps {
        // Traffic switched on mid-game warms up a batch a frame, not all in one.
        let inputs = self.traffic_inputs();
        self.npcs.warm_up(&inputs, npc::WARMUP_BATCH);
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
        // Auto headlights switch when the NPC drivers' first lamps do.
        let dark = self.traffic_clock().dark(0.0);
        self.drive.frame(input, dark);
        let none = InputState::default();
        let dt = SIM_DT as f32;
        let (mut traffic_s, mut peds_s) = (0.0, 0.0);
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
            if self.npcs.active() {
                let t = now();
                let inp = npc::StepInputs {
                    player: self.traffic_player(),
                    clock: self.traffic_clock(),
                    ..inputs
                };
                self.npcs.step(&inp);
                traffic_s += now() - t;
            } else {
                self.npcs.step(&inputs);
            }
            if self.npcs.crowd.active() {
                let t = now();
                // Signed: a reversing car's danger zone is behind it (the cars' is not).
                let player = traffic::Player {
                    speed: self.drive.state.speed_mps,
                    ..self.traffic_player()
                };
                let inp = npc::StepInputs { player, ..inputs };
                self.npcs.step_peds(&inp);
                peds_s += now() - t;
            }
            let cam_input = if self.fly { &none } else { input };
            self.camera.step(&self.drive, cam_input, dt, &self.ground);
            self.time.step(input, SIM_DT);
            self.ticks += 1;
        }
        if self.drive.water_resets != self.water_resets_seen {
            self.water_resets_seen = self.drive.water_resets;
            self.water_toast = WATER_TOAST_S;
        }
        self.water_toast = (self.water_toast - dt_real as f32).max(0.0);
        self.npcs.note_time(traffic_s * 1000.0, steps.count);
        self.npcs.crowd.note_time(peds_s * 1000.0, steps.count);
        steps
    }

    /// The traffic's view of the game now: the player's car, the camera, the clock.
    fn traffic_inputs(&self) -> npc::StepInputs {
        let (eye, forward, fov) = if self.fly {
            let c = &self.curr;
            let (s, co) = c.yaw.sin_cos();
            (c.pos, Vec3::new(-s, 0.0, -co), FLY_FOV_DEG.to_radians())
        } else {
            let pose = self.camera.pose(&self.drive, 1.0);
            (pose.eye, pose.forward(), pose.fov_y_rad)
        };
        npc::StepInputs {
            player: self.traffic_player(),
            view: npc::view(eye, forward, fov, self.aspect),
            clock: self.traffic_clock(),
        }
    }

    fn traffic_player(&self) -> traffic::Player {
        let s = &self.drive.state;
        npc::player(&self.drive.pose(1.0), s.speed_mps, self.drive.spec.dims_m)
    }

    fn traffic_clock(&self) -> traffic::Clock {
        let c = &self.time.clock;
        npc::clock(c.seconds_of_day, self.time.latitude_deg, c.day_of_year)
    }

    /// Live NPC cars.
    pub fn traffic_count(&self) -> usize {
        self.npcs.count()
    }

    /// Live pedestrians.
    pub fn pedestrian_count(&self) -> usize {
        self.npcs.crowd.count()
    }

    /// The traffic density from the scene or the setting; the NPC cars' meshes are built
    /// and queued for [`App::take_new_meshes`] the first time it is on.
    fn apply_traffic(&mut self, density: traffic::Density) {
        if density != traffic::Density::Off && !self.npcs.has_meshes() {
            let (pending, mut next) = (&mut self.pending_meshes, self.mesh_count);
            let built = self.npcs.build_meshes(self.props.as_ref(), |m| {
                pending.push(m);
                next += 1;
                MeshId(next - 1)
            });
            debug_assert!(built.is_ok(), "{built:?}");
            self.mesh_count = next;
        }
        self.npcs.set_density(density);
    }

    /// Runs the traffic's whole warm-up now: while the scene loads, before its first frame.
    fn finish_warm_up(&mut self) {
        let inputs = self.traffic_inputs();
        self.npcs.warm_up_unseen(&inputs);
    }

    /// Whether the deep-water toast is up.
    pub fn water_toast(&self) -> bool {
        self.water_toast > 0.0
    }

    /// Fingerprint of the simulation state, for determinism checks.
    pub fn state_hash(&self) -> u64 {
        let c = &self.curr;
        let bits = |v: f32| u64::from(v.to_bits());
        let clock = &self.time.clock;
        let h = hash64(
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
        );
        // The traffic and pedestrians only while they run, so a scene without them hashes
        // as before.
        if self.npcs.active() || self.npcs.crowd.active() {
            hash64(h, &[self.npcs.hash()])
        } else {
            h
        }
    }

    /// A stage in virtual HUD units (`[x, y, w, h]`) in normalized device coordinates
    /// (`[x0, y0, x1, y1]`, y up) on the target, or on one of `aspect` when the platform has
    /// not said its size.
    fn stage_ndc(&self, stage: [f32; 4], aspect: f32) -> [f32; 4] {
        let target = self
            .target
            .unwrap_or(((aspect * 1080.0).round().max(1.0) as u32, 1080));
        let layout = render::HudLayout::new(target, self.ui.settings().ui_scale);
        let [x, y, w, h] = stage;
        let [px0, py0] = layout.to_pixels(x, y);
        let [px1, py1] = layout.to_pixels(x + w, y + h);
        let (tw, th) = (target.0 as f32, target.1 as f32);
        [
            px0 / tw * 2.0 - 1.0,
            1.0 - py1 / th * 2.0,
            px1 / tw * 2.0 - 1.0,
            1.0 - py0 / th * 2.0,
        ]
    }

    /// The scene interpolated `alpha` of the way from the previous to the current step.
    /// Refills the car's instances in place (no allocation after the first frame).
    pub fn scene_view(&mut self, alpha: f32, aspect: f32) -> SceneView<'_> {
        let alpha = alpha.clamp(0.0, 1.0);
        let aspect = aspect.max(1e-3);
        self.aspect = aspect;
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
        let first_person = !self.fly && self.camera.mode().first_person();
        let cabin = if !first_person {
            drive::Cabin::Outside
        } else if self.ui.settings().dashboard {
            drive::Cabin::Full
        } else {
            drive::Cabin::Frame
        };
        // Under the Vehicle page the turntable shows the car (and stands where the cabin
        // would be round a first-person eye): no car of our own.
        if self.preview.is_none() {
            self.drive.push_instances(
                alpha,
                &self.car_meshes,
                cabin,
                lighting::car_glow(&daylight),
                &mut self.instances,
            );
        }
        // The car's lamps, the nearest NPC cars' low beams, then every static light the
        // photocell has on; each instance gets the nearest that reach it, the car's
        // instances those round the car.
        let (car_lights, n) = self.drive.lights(alpha);
        let n = if self.preview.is_some() { 0 } else { n };
        let eye = view.inverse().w_axis.truncate();
        let mut dynamic = [PointLight::omni([0.0; 3], [0.0; 3], 0.0); DYNAMIC_LIGHTS];
        dynamic[..n].copy_from_slice(&car_lights[..n]);
        let mut npc_lights = [dynamic[0]; npc::NPC_LIGHTS];
        let m = self.npcs.lights(alpha, eye, &mut npc_lights);
        dynamic[n..n + m].copy_from_slice(&npc_lights[..m]);
        self.lights
            .gather(eye, daylight.streetlights_on, &dynamic[..n + m]);
        self.lights.set_second(n..n + m);
        let (world, car) = self.instances.split_at_mut(self.world_instances);
        self.lights.assign(&self.bounds, world);
        let reach = 0.5 * self.drive.layout.dims.iter().fold(0.0f32, |a, &b| a.max(b));
        let around = Bounds::cube(self.drive.pose(alpha).pos, reach);
        let set = self.lights.set_for(&around);
        for inst in car {
            inst.lights = set;
        }
        if let Some(props) = &self.props {
            let stream = self.stream.as_deref();
            let live = |c: [i32; 2]| stream.is_none_or(|s| s.streamer.is_live(c));
            let lights = &self.lights;
            let rocking = &self.drive.rocking;
            props.push(eye, live, lights, lamp_glow, rocking, &mut self.instances);
        }
        let npc_glow = lighting::car_glow(&daylight).lamps;
        self.npcs
            .push(alpha, eye, &self.lights, npc_glow, &mut self.instances);
        // Whole people only: none near the vehicle preview (below), none cut in half there.
        let clear = if self.preview.is_some() {
            preview::CLEAR_M + peds::REACH_M
        } else {
            0.0
        };
        self.npcs
            .crowd
            .push(alpha, eye, clear, &self.lights, &mut self.instances);
        if self.preview.is_some() {
            // Nothing that moves or stands by the eye cuts into the turntable: drop the
            // traffic, people and props near it (in place, keeping their order).
            let mut keep = self.world_instances;
            for k in self.world_instances..self.instances.len() {
                let at = Vec3::from_slice(&self.instances[k].transform[3][..3]);
                if at.distance_squared(eye) > preview::CLEAR_M * preview::CLEAR_M {
                    self.instances.swap(keep, k);
                    keep += 1;
                }
            }
            self.instances.truncate(keep);
        }
        let start = self.instances.len();
        if let Some((car, stage)) = &self.preview {
            let stage = self.stage_ndc(*stage, aspect);
            if let Some(table) = preview::table_to_world(view, proj, stage, &car.layout, self.spin)
            {
                let model = preview::Model {
                    car: &car.meshes,
                    layout: &car.layout,
                    turntable: self.turntable,
                };
                let glow = lighting::car_glow(&daylight);
                let fill = daylight.window_emissive_scale;
                preview::push(table, &model, glow, fill, &mut self.instances);
            }
        }
        self.preview_drawn = start..self.instances.len();
        SceneView {
            camera: Camera {
                view: view.to_cols_array_2d(),
                proj: proj.to_cols_array_2d(),
            },
            sun: lighting::sun(&daylight),
            sky: self.ui.video_sky(lighting::sky(&daylight)),
            instances: &self.instances,
            lights: self.lights.lights(),
            hud: if self.water_toast > 0.0 {
                let hud = self.ui.hud();
                let o = &mut self.overlay;
                o.begin(hud.ui_scale);
                o.extend(&hud);
                // Centered a third of the way down.
                let [w, h] = o.size();
                let scale = 2.0;
                let x = 0.5 * (w - render::font::text_width(WATER_TOAST) as f32 * scale);
                let y = h / 3.0;
                o.text(x + scale, y + scale, scale, TOAST_SHADOW, WATER_TOAST);
                o.text(x, y, scale, TOAST_RGBA, WATER_TOAST);
                o.view()
            } else {
                self.ui.hud()
            },
            quality: self.ui.render_quality(),
        }
    }
}

/// `paint` when `vehicles::BUILTIN[index]` has that palette entry, else its first.
fn paint_of(index: usize, paint: usize) -> usize {
    let paints = ui::roster::roster()
        .get(index)
        .map_or(1, |c| c.paints.len());
    if paint < paints {
        paint
    } else {
        0
    }
}

/// Instances one pedestrian draws: a part each.
pub const FIGURE_PARTS: usize = traffic::figure::PARTS;

/// Most dynamic lights a frame has: the player's car's, then the traffic's.
const DYNAMIC_LIGHTS: usize = drive::CAR_LIGHTS + npc::NPC_LIGHTS;

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
    let chunks = world::build_chunk_meshes(place, &Palette::socal_1985(), places::grammar());
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

/// The instance drawing a world mesh or chunk part of role `role`. Neon, signals and lamp
/// lenses glow rather than block the sun: they cast no shadow.
fn world_instance(mesh: MeshId, role: PartRole) -> MeshInstance {
    MeshInstance {
        color: if role == PartRole::Glow {
            [GLOW_BOOST, GLOW_BOOST, GLOW_BOOST, 1.0]
        } else {
            [1.0; 4]
        },
        casts_shadow: role == PartRole::Tile,
        ..MeshInstance::new(mesh, Mat4::IDENTITY.to_cols_array_2d())
    }
}

/// `place` to be streamed within `budget`: nothing built yet but a terrain place's far
/// terrain and horizon (drawn always, under the chunks, out to the horizon), every static
/// light known
/// (and off until its chunk is drawn), room for every part the resident chunks can have.
fn streamed_town(place: Place, budget: StreamBudget) -> Town {
    let source = PlaceSource::new(place, Palette::socal_1985());
    let streamer = Streamer::new(&source, budget);
    let statics = source.static_lights();
    let mut lights = FrameLights::new(statics.iter().map(|s| s.light).collect());
    lights.set_active(|_| false);
    let light_chunks = statics.iter().map(|s| s.chunk).collect();
    let meshes: Vec<Mesh> = [source.far_terrain(), source.horizon()]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let fixed = meshes.len();
    let capacity = fixed + streamer.capacity() * MAX_CHUNK_PARTS;
    let mut instances = Vec::with_capacity(capacity);
    let mut roles = Vec::with_capacity(capacity);
    let mut bounds = Vec::with_capacity(capacity);
    for i in 0..fixed {
        // Under the chunks everywhere: drawn after them, so they hide it before shading.
        instances.push(MeshInstance {
            late: true,
            ..world_instance(MeshId(i as u32), PartRole::Tile)
        });
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
        let extra = drive::MAX_CAR_INSTANCES + prop_capacity(&self.props) + self.npcs.capacity();
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
        let extra = drive::MAX_CAR_INSTANCES + prop_capacity(&self.props) + self.npcs.capacity();
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
            0xadde_172d_3f95_7922,
            "{:#x}",
            a.state_hash()
        );
    }

    #[test]
    fn the_wagon_drives_too() {
        let mut app = App::new_with(Seed(1), "homestead_wagon").unwrap();
        assert_eq!(app.drive.spec.id, "homestead_wagon");
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

    /// Auto headlights (the default) come on at dusk and go off at dawn across the whole
    /// day, with the NPC drivers' first lamps; the switch overrides them until the next
    /// dusk or dawn; Manual leaves them to the switch.
    #[test]
    fn auto_headlights_follow_the_daylight_clock() {
        let mut app = App::new(Seed(0));
        assert!(app.drive.auto_headlights);
        let mut input = InputState::default();
        let at = |app: &mut App, input: &InputState, h: f64| {
            app.time.clock.seconds_of_day = (h * 3600.0).rem_euclid(86_400.0);
            app.update(input, 1.0 / 60.0);
            app.traffic_clock().dark(0.0)
        };
        let mut dark_steps = 0;
        for k in 0..24 * 6 {
            let h = 12.0 + f64::from(k) / 6.0;
            let dark = at(&mut app, &input, h);
            assert_eq!(app.drive.headlights, dark, "at {h:.2} h");
            dark_steps += usize::from(dark);
        }
        assert!((24..120).contains(&dark_steps), "{dark_steps} dark steps");
        assert!(at(&mut app, &input, 12.0) == app.drive.headlights);
        assert!(!app.drive.headlights, "off by day");
        // Switched on at noon, they stay on through the afternoon, and dusk keeps them on;
        // switched off at night, they stay off until dawn switches them... off.
        press(&mut input, Action::Headlights);
        at(&mut app, &input, 12.1);
        input.set_just_pressed(Action::Headlights, false);
        input.set(Action::Headlights, 0.0);
        at(&mut app, &input, 16.0);
        assert!(app.drive.headlights, "the switch holds by day");
        assert!(at(&mut app, &input, 23.0) && app.drive.headlights);
        press(&mut input, Action::Headlights);
        at(&mut app, &input, 23.1);
        input.set_just_pressed(Action::Headlights, false);
        input.set(Action::Headlights, 0.0);
        at(&mut app, &input, 2.0);
        assert!(!app.drive.headlights, "the switch holds by night");
        at(&mut app, &input, 12.0);
        assert!(
            at(&mut app, &input, 23.0) && app.drive.headlights,
            "the next dusk"
        );
        // Manual: the clock never touches them.
        app.drive.auto_headlights = false;
        at(&mut app, &input, 12.0);
        assert!(app.drive.headlights, "manual keeps them on by day");
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
        let turntable = app.turntable;
        let view = app.scene_view(steps.alpha, 16.0 / 9.0);
        // The world, then the car: body, glass, unlit lamps and four wheels (no cabin in
        // the chase view).
        assert_eq!(MeshId(world as u32), ids.lods[0].body[0]);
        // ...then the turntable's disc.
        assert_eq!(ids.dial_ticks, MeshId(meshes as u32 - 4));
        assert_eq!(ids.telltales, [3, 2].map(|k| MeshId(meshes as u32 - k)));
        assert_eq!(turntable, MeshId(meshes as u32 - 1));
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
        // The default setup is the normal start (with traffic, which `App::new` leaves off).
        let mut normal = App::new(Seed(1));
        let mut same = App::from_setup(&SceneSetup::from_query("traffic=off").unwrap()).unwrap();
        assert_eq!(same.state_hash(), normal.state_hash());
        assert_eq!(
            same.scene_view(1.0, 1.5).camera.view,
            normal.scene_view(1.0, 1.5).camera.view
        );

        let setup = SceneSetup::from_query(
            "?seed=3&car=homestead_wagon&cam=cockpit&t=22:30&pos=-200,-197.5&yaw=0&lights=1&stream=0&traffic=off",
        )
        .unwrap();
        let mut app = App::from_setup(&setup).unwrap();
        assert_eq!(app.drive.spec.id, "homestead_wagon");
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
        // Streamed (the default): the far terrain and the horizon drawn from the start,
        // under the ring round the start as it fills, every chunk within budget.
        let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
        assert!(app.is_streamed() && app.world_instances == 2);
        let far = app.meshes()[0].indices.len() / 3;
        assert!(
            (8192..=10_000).contains(&far),
            "far terrain: {far} triangles"
        );
        let horizon = app.meshes()[1].indices.len() / 3;
        assert!(
            (1000..=8000).contains(&horizon),
            "horizon: {horizon} triangles"
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
            [app.instances[0].mesh, app.instances[1].mesh],
            [MeshId(0), MeshId(1)],
            "the far terrain and the horizon stay drawn"
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

    #[test]
    fn deep_water_resets_the_car_with_a_toast() {
        // On the sand by the pier facing the sea; full throttle until the water resets it.
        let query = "?seed=1&place=coast&pos=440,940&yaw=270&stream=1";
        let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
        let mut input = InputState::default();
        input.set(Action::Throttle, 1.0);
        let mut frames = 0;
        while !app.water_toast() {
            app.update(&input, 1.0 / 30.0);
            frames += 1;
            assert!(frames < 30 * 20, "no reset");
        }
        let has_toast = |app: &mut App| {
            let view = app.scene_view(1.0, 16.0 / 9.0);
            let hud = view.hud;
            hud.items.iter().any(|i| match *i {
                render::HudItem::Text { text, .. } => hud.str(text) == WATER_TOAST,
                _ => false,
            })
        };
        assert!(has_toast(&mut app));
        // Back at the scene's start point, and the toast goes after its time.
        let pos = app.drive.pose(1.0).pos;
        assert!((pos.x - 440.0).abs() < 1e-3 && (pos.z - 940.0).abs() < 1e-3);
        for _ in 0..(WATER_TOAST_S * 30.0) as usize + 2 {
            app.update(&InputState::default(), 1.0 / 30.0);
        }
        assert!(!app.water_toast() && !has_toast(&mut app));
    }
}
