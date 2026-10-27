//! In-game UI: the pause and settings menus (gamepad first), the remap capture flow, the
//! driving HUD with its minimap, the full-screen map, and the performance overlay, plus
//! loading and saving the settings.
//!
//! The UI owns the user's [`Settings`] and [`Bindings`]; the platform's [`InputMapper`]
//! gets a copy of the bindings whenever they change. While a menu is open the simulation
//! is paused and only the Menu actions are read. The full map (MapToggle) is not a pause:
//! the simulation runs on, the car gets no driving input, and the menu directions pan the
//! map while MapZoom and the mouse wheel zoom it; MapToggle, Back or Pause close it. Every
//! frame the UI draws itself into a [`HudBuilder`] that [`crate::App::scene_view`] hands to
//! the renderer.
//!
//! The UI also owns the car radio ([`CarRadio`]): its settings live with the others, the
//! station keys act while driving, and its toast and dash text are part of the HUD.

mod draw;
mod menu;
mod names;
mod pages;
pub mod roster;

pub use menu::{Effect, Item, MenuInput, MenuNav, Nav, Repeat, Unit, Widget};
pub use pages::{ItemId, Page};

use crate::camera::CameraMode;
use crate::daylight::{Clock, CLOCK_SPEEDS, DAY_SECONDS};
use crate::input::{
    Action, AxisOptions, Binding, Bindings, Conflict, DeviceKind, InputMapper, InputState, PadAxis,
    PadButton, RawInput, Sign,
};
use crate::map::view::{
    FullMap, MapGeom, MINIMAP_RANGES_M, ZOOM_OCTAVES_PER_NOTCH, ZOOM_OCTAVES_PER_S,
};
use crate::map::MapPoi;
use crate::radio::{self, CarRadio};
use crate::settings::{FrameRate, GpuInfo, Handling, Quality, Settings, SettingsStore};
use crate::App;
use render::{Hud, HudBuilder, HudLayout, RenderQuality, RenderStats, Sky};
use skywave_core::{Steps, MAX_STEPS_PER_FRAME, SIM_DT};

/// Seconds a remap capture waits for an input before giving up.
pub const CAPTURE_TIMEOUT: f32 = 8.0;
/// Seconds a status message stays up.
const TOAST_SECONDS: f32 = 2.5;
/// Seconds the new car's name stays up after a switch, and "Car reset" after a reset.
pub const VEHICLE_TOAST_SECONDS: f32 = 2.0;
/// Miles per hour in one meter per second.
const MPH_PER_MPS: f32 = 2.236_936;
/// Frame times kept for the overlay's graph.
pub const FRAME_HISTORY: usize = 120;

/// A remap waiting for its input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureState {
    pub device: DeviceKind,
    pub action: Action,
    pub sign: Sign,
    /// Seconds left before it gives up.
    pub remaining: f32,
}

/// A remap that hit an input another action uses, waiting on the Conflict page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingConflict {
    pub conflict: Conflict,
    /// The row (direction) the user was remapping.
    pub sign: Sign,
}

/// A short status message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Toast {
    Bound(Binding, Action, Sign),
    Kept(Binding, Action),
    Cancelled,
    WrongDevice(DeviceKind),
    Southpaw,
    SwapTriggers,
    Reset(DeviceKind),
    /// Switched to the built-in car with this index (its name).
    Vehicle(usize),
    /// The car was put back on the road.
    CarReset,
    /// A stream was added to the stations.
    StreamAdded,
    /// The text given for a stream was not an `https://` URL (or there are too many).
    StreamRejected,
}

/// What the minimap and the full map show, set once when the place loads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapHud {
    /// Where the uploaded map image lies; `None` draws no map at all.
    pub geom: Option<MapGeom>,
    pub pois: Vec<MapPoi>,
    pub full: FullMap,
}

pub struct Ui {
    settings: Settings,
    bindings: Bindings,
    /// The mapper needs the new bindings.
    bindings_changed: bool,
    /// The Camera page picked another camera.
    camera_changed: bool,
    /// The Vehicle page picked the built-in car with this index, in this paint; the app
    /// switches to it.
    vehicle_pick: Option<(usize, usize)>,
    /// The pause menu's "Reset car" was chosen; the app resets the car.
    reset_pick: bool,
    /// The car being driven, an index into `vehicles::BUILTIN`, and its paint.
    vehicle: usize,
    vehicle_paint: usize,
    /// The car and paint the Vehicle page shows (on the turntable).
    browse: [usize; 2],
    /// Where the Vehicle page left room for the turntable, virtual HUD units `[x, y, w,
    /// h]`, as last drawn (`None`: no room, or another page).
    stage: Option<[f32; 4]>,
    /// Something changed since the last save.
    dirty: bool,
    store: Option<Box<dyn SettingsStore>>,
    menu: Option<MenuNav>,
    /// The top page's items, rebuilt before every use.
    items: Vec<Item>,
    repeat: Repeat,
    capture: Option<CaptureState>,
    conflict: Option<PendingConflict>,
    toast: Option<(Toast, f32)>,
    can_quit: bool,
    quit: bool,
    last_device: DeviceKind,
    /// Speed shown on the HUD; `None` shows "-- mph" (no car: the free-fly camera).
    speed_mph: Option<f32>,
    render_stats: RenderStats,
    /// Recent frame times, ms, a ring with `frame_head` the next slot.
    frame_ms: [f32; FRAME_HISTORY],
    frame_head: usize,
    last_steps: u32,
    hud: HudBuilder,
    map: MapHud,
    /// A UI scale the scene asked for, kept over whatever the store loads.
    forced_ui_scale: Option<f32>,
    radio: CarRadio,
    /// The platform can ask the user for a line of text (the "Add stream URL" item).
    text_entry: bool,
    /// The preset [`Quality::auto`] picked for this device (Medium until the platform
    /// says what the GPU is).
    auto_quality: Quality,
    /// A preset the command line forced this session (not saved; a choice on the Video
    /// page replaces it).
    forced_quality: Option<Quality>,
    /// The frame-rate cap [`FrameRate::auto`] picked for this device (the panel's until the
    /// platform says what the GPU is).
    auto_frame_rate: FrameRate,
    /// The window has the focus (the platform says; true until it does).
    focused: bool,
    /// CPU time the platform spent recording and submitting the last frame, ms (0:
    /// not measured).
    cpu_submit_ms: f32,
    /// Live NPC cars and the traffic sim's time per step (ms), for the overlay.
    pub(crate) traffic: (usize, f32),
    /// Live pedestrians and the pedestrian sim's time per step (ms), for the overlay.
    pub(crate) peds: (usize, f32),
    /// Adapter and backend, for the overlay.
    gpu_label: String,
}

impl Default for Ui {
    fn default() -> Self {
        Ui::new()
    }
}

impl Ui {
    pub fn new() -> Ui {
        Ui {
            settings: Settings::default(),
            bindings: Bindings::default(),
            bindings_changed: false,
            camera_changed: false,
            vehicle_pick: None,
            reset_pick: false,
            vehicle: 0,
            vehicle_paint: 0,
            browse: [0, 0],
            stage: None,
            dirty: false,
            store: None,
            menu: None,
            items: Vec::with_capacity(64),
            repeat: Repeat::default(),
            capture: None,
            conflict: None,
            toast: None,
            can_quit: false,
            quit: false,
            last_device: DeviceKind::KeyboardMouse,
            speed_mph: None,
            render_stats: RenderStats::default(),
            frame_ms: [0.0; FRAME_HISTORY],
            frame_head: 0,
            last_steps: 0,
            hud: HudBuilder::new(),
            map: MapHud::default(),
            forced_ui_scale: None,
            radio: CarRadio::new(),
            text_entry: false,
            auto_quality: Quality::Medium,
            forced_quality: None,
            auto_frame_rate: FrameRate::Panel,
            focused: true,
            cpu_submit_ms: 0.0,
            traffic: (0, 0.0),
            peds: (0, 0.0),
            gpu_label: String::new(),
        }
    }

    /// The graphics preset in effect: forced, else saved, else picked for the device.
    pub fn quality(&self) -> Quality {
        self.forced_quality
            .or(self.settings.quality)
            .unwrap_or(self.auto_quality)
    }

    /// The frame-rate cap in effect: saved, else picked for the device.
    pub fn frame_rate(&self) -> FrameRate {
        self.settings.frame_rate.unwrap_or(self.auto_frame_rate)
    }

    /// What the renderer may spend this frame: the preset at the resolution scale setting
    /// (a forced preset brings its own scale).
    pub fn render_quality(&self) -> RenderQuality {
        match self.forced_quality {
            Some(q) => q.render(q.resolution_scale()),
            None => self.quality().render(self.settings.resolution_scale),
        }
    }

    /// Uses UI scale `s` this session, also over settings loaded later (not saved unless
    /// another change is).
    pub fn force_ui_scale(&mut self, s: f32) {
        self.forced_ui_scale = Some(s);
        self.settings.ui_scale = s;
        self.settings = std::mem::take(&mut self.settings).sanitized();
    }

    /// The map the HUD draws: its image's place in the world and the labelled POIs.
    pub fn set_map(&mut self, geom: MapGeom, pois: Vec<MapPoi>) {
        self.map = MapHud {
            geom: Some(geom),
            pois,
            full: FullMap::default(),
        };
    }

    pub fn map(&self) -> &MapHud {
        &self.map
    }

    /// The full-screen map is up.
    pub fn full_map_open(&self) -> bool {
        self.map.full.open
    }

    /// Opens the full map on the whole place (nothing happens without a map). Returns
    /// whether it is open.
    pub fn open_full_map(&mut self) -> bool {
        if let Some(geom) = &self.map.geom {
            self.map.full.open(geom);
        }
        self.map.full.open
    }

    /// The area the full map may use: the safe area at this UI scale, less a line for
    /// the controls at the bottom.
    fn full_map_rect(&self) -> [f32; 4] {
        let [x, y, w, h] = HudLayout::safe_rect(self.settings.ui_scale);
        [x, y, w, h - draw::FULL_MAP_FOOTER]
    }

    /// One frame of full-map input: menu directions pan, MapZoom and the wheel zoom.
    fn update_full_map(&mut self, s: &InputState, dt: f32) {
        let Some(geom) = self.map.geom else { return };
        let rect = self.full_map_rect();
        let dir = [
            s.axis(Action::MenuRight) - s.axis(Action::MenuLeft),
            s.axis(Action::MenuDown) - s.axis(Action::MenuUp),
        ];
        let octaves =
            s.axis(Action::MapZoom) * ZOOM_OCTAVES_PER_S * dt + s.wheel() * ZOOM_OCTAVES_PER_NOTCH;
        let full = &mut self.map.full;
        full.zoom_by(octaves, &geom, rect);
        full.pan(dir, dt, &geom, rect);
    }

    /// The settings, to change in tests as the menu would.
    #[cfg(test)]
    pub(crate) fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    pub fn menu(&self) -> Option<&MenuNav> {
        self.menu.as_ref()
    }

    pub fn is_open(&self) -> bool {
        self.menu.is_some()
    }

    pub fn capture(&self) -> Option<&CaptureState> {
        self.capture.as_ref()
    }

    pub fn conflict(&self) -> Option<&PendingConflict> {
        self.conflict.as_ref()
    }

    pub fn toast(&self) -> Option<Toast> {
        self.toast.map(|(t, _)| t)
    }

    pub fn hud(&self) -> Hud<'_> {
        self.hud.view()
    }

    /// `sky` with the video settings applied (bloom off zeroes its strength).
    pub fn video_sky(&self, mut sky: Sky) -> Sky {
        if !self.settings.bloom {
            sky.bloom_strength = 0.0;
        }
        sky
    }

    /// Takes the "bindings changed" flag: true once after every change.
    fn take_bindings_changed(&mut self) -> bool {
        std::mem::take(&mut self.bindings_changed)
    }

    /// Takes the "camera picked on the Camera page" flag: true once after every pick.
    fn take_camera_changed(&mut self) -> bool {
        std::mem::take(&mut self.camera_changed)
    }

    /// Takes the car (and paint) the Vehicle page picked, once.
    fn take_vehicle_pick(&mut self) -> Option<(usize, usize)> {
        self.vehicle_pick.take()
    }

    /// Takes the pause menu's "Reset car", once.
    fn take_reset_pick(&mut self) -> bool {
        std::mem::take(&mut self.reset_pick)
    }

    /// The app reset the car: say so for [`VEHICLE_TOAST_SECONDS`].
    pub(crate) fn car_reset(&mut self) {
        self.toast = Some((Toast::CarReset, VEHICLE_TOAST_SECONDS));
    }

    /// The car being driven: an index into [`crate::vehicles::BUILTIN`].
    pub fn vehicle(&self) -> usize {
        self.vehicle
    }

    /// The paint the car being driven wears: a palette entry.
    pub fn vehicle_paint(&self) -> usize {
        self.vehicle_paint
    }

    /// The Vehicle page opens on the car being driven, in its paint, the car chooser
    /// focused.
    fn focus_current_vehicle(&mut self) {
        if let Some(menu) = &mut self.menu {
            if menu.page() == Page::Vehicle {
                self.browse = [self.vehicle, self.vehicle_paint];
                menu.set_focus(0);
            }
        }
    }

    /// The car and paint on the turntable and where it goes on screen (virtual HUD units
    /// `[x, y, w, h]`), while the Vehicle page is up and has room for it.
    pub fn preview(&self) -> Option<(usize, usize, [f32; 4])> {
        let page = self.menu.as_ref()?.page();
        let stage = self.stage?;
        (page == Page::Vehicle && self.capture.is_none()).then_some((
            self.browse[0],
            self.browse[1],
            stage,
        ))
    }

    /// The app now drives the built-in car `index` in palette entry `paint`. With
    /// `remember`, they become the vehicle setting (saved now, so the next session starts
    /// in it); with `toast`, its name shows for [`VEHICLE_TOAST_SECONDS`].
    pub(crate) fn vehicle_switched(
        &mut self,
        index: usize,
        paint: usize,
        remember: bool,
        toast: bool,
    ) {
        self.vehicle = index;
        self.vehicle_paint = paint;
        let id = crate::vehicles::BUILTIN.get(index).map(|(id, _)| *id);
        if remember && id.is_some() && (self.settings.vehicle != id || self.settings.paint != paint)
        {
            self.settings.vehicle = id;
            self.settings.paint = paint;
            self.dirty = true;
            self.save_if_dirty();
        }
        if toast {
            self.toast = Some((Toast::Vehicle(index), VEHICLE_TOAST_SECONDS));
        }
    }

    /// The player cycled to camera `mode` while driving: remember it (and save now, so the
    /// next session starts there).
    fn remember_camera(&mut self, mode: CameraMode) {
        if mode != self.settings.camera {
            self.settings.camera = mode;
            self.settings = std::mem::take(&mut self.settings).sanitized();
            self.dirty = true;
            self.save_if_dirty();
        }
    }

    /// Reads the settings document from `store` and keeps the store for saving.
    pub fn set_store(&mut self, store: Box<dyn SettingsStore>) {
        if let Some(text) = store.load() {
            let (settings, bindings) = Settings::from_json(&text);
            self.settings = settings;
            if let Some(b) = bindings {
                self.bindings = b;
            }
            self.bindings_changed = true;
        }
        if let Some(s) = self.forced_ui_scale {
            self.settings.ui_scale = s;
            self.settings = std::mem::take(&mut self.settings).sanitized();
        }
        self.store = Some(store);
        self.dirty = false;
        self.radio.set_gain(self.radio_gain());
        self.radio.apply(&self.settings.radio);
    }

    /// Writes the settings document if anything changed since the last save.
    pub fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(store) = &mut self.store {
            store.save(&self.settings.to_json(&self.bindings));
        }
        self.dirty = false;
    }

    /// Opens the menu on `page`, over the pause page so Back leads there.
    pub fn open(&mut self, page: Page, input: &InputState) {
        let mut nav = MenuNav::new(Page::Pause);
        if page != Page::Pause {
            nav.push(page);
        }
        self.menu = Some(nav);
        self.repeat.reset(&MenuInput::from_state(input));
    }

    pub fn close(&mut self) {
        self.menu = None;
        self.conflict = None;
        self.save_if_dirty();
    }

    fn page_ctx<'a>(&'a self, clock: &'a Clock) -> pages::PageCtx<'a> {
        pages::PageCtx {
            settings: &self.settings,
            bindings: &self.bindings,
            clock,
            can_quit: self.can_quit,
            radio: &self.radio,
            text_entry: self.text_entry,
            quality: self.quality(),
            frame_rate: self.frame_rate(),
            browse: self.browse,
        }
    }

    /// Rebuilds the top page's items (into the reused buffer).
    fn rebuild(&mut self, clock: &Clock) {
        let Some(page) = self.menu.as_ref().map(MenuNav::page) else {
            self.items.clear();
            return;
        };
        let mut items = std::mem::take(&mut self.items);
        pages::build(page, &self.page_ctx(clock), &mut items);
        self.items = items;
    }

    /// The top page's items as of the last rebuild.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Handles one frame of input: the overlay toggle, opening and closing the menu, menu
    /// navigation and remap capture.
    pub fn handle(&mut self, s: &InputState, mapper: &mut InputMapper, clock: &mut Clock, dt: f32) {
        self.last_device = mapper.last_device();
        if let Some((_, t)) = &mut self.toast {
            *t -= dt;
            if *t <= 0.0 {
                self.toast = None;
            }
        }
        if s.just_pressed(Action::DebugOverlay) {
            self.settings.debug_overlay = !self.settings.debug_overlay;
            self.dirty = true;
            self.save_if_dirty();
        }
        if self.capture.is_some() {
            self.update_capture(mapper, dt);
            // The capturing press may also be a menu action: swallow this frame's menu input.
            self.repeat.reset(&MenuInput::from_state(s));
            self.rebuild(clock);
            return;
        }
        let Some(menu) = &self.menu else {
            if self.map.full.open {
                // Back and Pause close the map rather than open the menu over it.
                if s.just_pressed(Action::MapToggle)
                    || s.just_pressed(Action::MenuBack)
                    || s.just_pressed(Action::Pause)
                {
                    self.map.full.close();
                } else {
                    self.update_full_map(s, dt);
                }
                return;
            }
            if s.just_pressed(Action::MapToggle) && self.open_full_map() {
                return;
            }
            if s.just_pressed(Action::Pause) {
                self.open(Page::Pause, s);
                self.rebuild(clock);
            }
            return;
        };
        let input = MenuInput::from_state(s);
        // Start closes the menu from anywhere; Esc is both Pause and Back, and goes back.
        if s.just_pressed(Action::Pause) && !input.back && menu.page() != Page::Conflict {
            self.close();
            return;
        }
        self.rebuild(clock);
        if let Some(nav) = self.repeat.update(&input, dt) {
            let effect = match &mut self.menu {
                // Select drives the car shown from any row of the Vehicle page.
                Some(menu) if menu.page() == Page::Vehicle && nav == Nav::Confirm => {
                    Effect::Activate(ItemId::Vehicle(self.browse[0]))
                }
                Some(menu) => menu.apply(nav, &self.items),
                None => Effect::None,
            };
            self.apply(effect, clock, mapper);
            self.rebuild(clock);
        }
    }

    fn apply(&mut self, effect: Effect, clock: &mut Clock, mapper: &mut InputMapper) {
        match effect {
            Effect::Opened(Page::Vehicle) => self.focus_current_vehicle(),
            Effect::None | Effect::Moved | Effect::Opened(_) => {}
            Effect::Toggle(id, on) => self.set_toggle(id, on),
            Effect::Slide(id, v) => self.set_slider(id, v, clock),
            Effect::Choose(ItemId::VehicleCar, i) => {
                // Another car: the paint it is driven in, else its first.
                let paint = if i == self.vehicle {
                    self.vehicle_paint
                } else {
                    0
                };
                self.browse = [i, paint];
            }
            Effect::Choose(ItemId::Paint, k) => self.browse[1] = k,
            Effect::Choose(ItemId::ClockSpeed, i) => {
                self.settings.clock_speed = CLOCK_SPEEDS[i.min(CLOCK_SPEEDS.len() - 1)];
                self.dirty = true;
            }
            Effect::Choose(ItemId::MinimapZoom, i) => {
                self.settings.minimap_range_m = MINIMAP_RANGES_M[i.min(MINIMAP_RANGES_M.len() - 1)];
                self.dirty = true;
            }
            Effect::Choose(ItemId::Traffic, i) => {
                self.settings.traffic =
                    crate::npc::DENSITIES[i.min(crate::npc::DENSITIES.len() - 1)];
                self.dirty = true;
            }
            Effect::Choose(ItemId::Handling, i) => {
                self.settings.handling = Handling::ALL[i.min(Handling::ALL.len() - 1)];
                self.dirty = true;
            }
            Effect::Choose(ItemId::CameraView, i) => {
                self.settings.camera = CameraMode::CYCLE[i.min(CameraMode::CYCLE.len() - 1)];
                self.camera_changed = true;
                self.dirty = true;
            }
            Effect::Choose(ItemId::Dashboard, i) => {
                self.settings.dashboard = i == 0;
                self.dirty = true;
            }
            Effect::Choose(ItemId::RadioPreset, i) => {
                radio::choose_preset(&mut self.settings.radio, i);
                self.radio_settings_changed();
            }
            Effect::Choose(ItemId::Quality, i) => {
                let q = Quality::ALL[i.min(Quality::ALL.len() - 1)];
                self.settings.quality = Some(q);
                self.settings.resolution_scale = q.resolution_scale();
                if let Some(f) = q.frame_rate() {
                    self.settings.frame_rate = Some(f);
                }
                self.forced_quality = None;
                self.dirty = true;
            }
            Effect::Choose(ItemId::FrameRate, i) => {
                self.settings.frame_rate = Some(FrameRate::ALL[i.min(FrameRate::ALL.len() - 1)]);
                self.dirty = true;
            }
            Effect::Choose(ItemId::BackgroundRadio, i) => {
                self.settings.mute_radio_unfocused = i == 1;
                self.radio_settings_changed();
            }
            Effect::Choose(..) => {}
            Effect::Activate(id) => self.activate(id),
            Effect::Capture {
                device,
                action,
                sign,
            } => {
                self.capture = Some(CaptureState {
                    device,
                    action,
                    sign,
                    remaining: CAPTURE_TIMEOUT,
                });
                mapper.arm_capture();
            }
            Effect::Closed(page) => {
                if page == Page::Conflict {
                    if let Some(p) = self.conflict.take() {
                        self.show(Toast::Kept(p.conflict.binding, p.conflict.existing));
                    }
                }
                self.save_if_dirty();
            }
            Effect::Exit => self.close(),
        }
    }

    fn show(&mut self, t: Toast) {
        self.toast = Some((t, TOAST_SECONDS));
    }

    fn edit_axes(&mut self, device: DeviceKind, actions: &[Action], f: impl Fn(&mut AxisOptions)) {
        for &a in actions {
            let mut o = self.bindings.axis_options(device, a);
            f(&mut o);
            self.bindings.set_axis_options(device, a, o);
        }
        self.bindings_changed = true;
        self.dirty = true;
    }

    fn set_toggle(&mut self, id: ItemId, on: bool) {
        match id {
            ItemId::InvertLookY(d) => self.edit_axes(d, &pages::LOOK_Y, |o| o.invert = on),
            ItemId::InvertLookX(d) => self.edit_axes(d, &pages::LOOK_X, |o| o.invert = on),
            ItemId::Invert(d, a) => self.edit_axes(d, &[a], |o| o.invert = on),
            ItemId::Bloom => {
                self.settings.bloom = on;
                self.dirty = true;
            }
            ItemId::DebugOverlay => {
                self.settings.debug_overlay = on;
                self.dirty = true;
            }
            ItemId::Minimap => {
                self.settings.minimap = on;
                self.dirty = true;
            }
            ItemId::MinimapRotate => {
                self.settings.minimap_rotate = on;
                self.dirty = true;
            }
            ItemId::CameraShake => {
                self.settings.camera_shake = on;
                self.dirty = true;
            }
            ItemId::Mute => {
                self.settings.mute = on;
                self.radio_settings_changed();
            }
            ItemId::Station(i) => {
                if let Some(row) = self.radio.rows().get(i) {
                    let id = row.id.clone();
                    self.settings.radio.set_enabled(&id, on);
                    self.radio_settings_changed();
                }
            }
            _ => {}
        }
    }

    fn set_slider(&mut self, id: ItemId, v: f32, clock: &mut Clock) {
        let look = [
            pages::LOOK_X[0],
            pages::LOOK_X[1],
            pages::LOOK_Y[0],
            pages::LOOK_Y[1],
        ];
        match id {
            ItemId::LookSensitivity(d) => self.edit_axes(d, &look, |o| o.sensitivity = v),
            ItemId::Sensitivity(d, a) => self.edit_axes(d, &[a], |o| o.sensitivity = v),
            ItemId::Deadzone(d, a) => self.edit_axes(d, &[a], |o| o.deadzone = v),
            ItemId::Curve(d, a) => self.edit_axes(d, &[a], |o| o.curve = v),
            ItemId::UiScale => {
                self.settings.ui_scale = v;
                self.dirty = true;
            }
            ItemId::ResolutionScale => {
                self.settings.resolution_scale = v;
                self.dirty = true;
            }
            ItemId::CockpitFov => {
                self.settings.cockpit_fov_deg = v;
                self.dirty = true;
            }
            ItemId::SeatHeight => {
                self.settings.seat_height_cm = v;
                self.dirty = true;
            }
            ItemId::SeatForward => {
                self.settings.seat_forward_cm = v;
                self.dirty = true;
            }
            ItemId::MasterVolume => {
                self.settings.master_volume = v;
                self.radio_settings_changed();
            }
            ItemId::EngineVolume => {
                self.settings.engine_volume = v;
                self.dirty = true;
            }
            ItemId::WorldVolume => {
                self.settings.world_volume = v;
                self.dirty = true;
            }
            ItemId::TimeOfDay => {
                clock.seconds_of_day = (f64::from(v) * 3600.0).rem_euclid(DAY_SECONDS);
            }
            ItemId::RadioVolume => {
                self.settings.radio.volume = v;
                self.radio_settings_changed();
            }
            _ => {}
        }
        self.settings = std::mem::take(&mut self.settings).sanitized();
    }

    fn activate(&mut self, id: ItemId) {
        match id {
            ItemId::Resume => self.close(),
            ItemId::ResetCar => {
                // Back to the road, where the car now is.
                self.reset_pick = true;
                self.close();
            }
            ItemId::Vehicle(i) => {
                // Back to the road to see it.
                let paint = if i == self.browse[0] {
                    self.browse[1]
                } else {
                    0
                };
                self.vehicle_pick = Some((i, paint));
                self.close();
            }
            ItemId::Quit => {
                self.close();
                self.quit = true;
            }
            ItemId::Southpaw => {
                self.bindings.southpaw();
                self.bindings_edited();
                self.show(Toast::Southpaw);
            }
            ItemId::SwapTriggers => {
                self.bindings.swap_triggers_sticks();
                self.bindings_edited();
                self.show(Toast::SwapTriggers);
            }
            ItemId::ResetDevice(d) => {
                self.bindings.reset_device(d);
                self.bindings_edited();
                self.show(Toast::Reset(d));
            }
            ItemId::ResetAction(d, a) => {
                self.bindings.reset_action(d, a);
                self.bindings_edited();
            }
            ItemId::KeepExisting | ItemId::UseNew => self.settle_conflict(id == ItemId::UseNew),
            ItemId::AddStream => self.radio.request_url(),
            ItemId::ClearStreams => {
                self.settings.radio.clear_custom();
                self.radio_settings_changed();
            }
            _ => {}
        }
    }

    /// The radio follows a change to its settings, which are saved with the next save.
    fn radio_settings_changed(&mut self) {
        self.settings.radio = std::mem::take(&mut self.settings.radio).sanitized();
        self.radio.set_gain(self.radio_gain());
        self.radio.apply(&self.settings.radio);
        self.dirty = true;
    }

    /// One frame of the radio (never in a still scene): it starts from the settings on
    /// the first, takes the station keys when `keys` (driving, no menu or map up), runs its
    /// tuning phase, and saves the power and station when they change.
    fn radio_frame(&mut self, s: &InputState, dt: f32, keys: bool) {
        self.radio.set_gain(self.radio_gain());
        self.radio.start(&self.settings.radio);
        if keys {
            self.radio.press(s);
        }
        self.radio.tick(dt);
        if self.radio.store(&mut self.settings.radio) {
            self.dirty = true;
            self.save_if_dirty();
        }
    }

    pub fn radio(&self) -> &CarRadio {
        &self.radio
    }

    /// The master volume scales the radio too, and mute silences it (and so does the
    /// window going to the background, when the setting says so).
    fn radio_gain(&self) -> f32 {
        if self.settings.mute || (self.settings.mute_radio_unfocused && !self.focused) {
            0.0
        } else {
            self.settings.master_volume
        }
    }

    fn bindings_edited(&mut self) {
        self.bindings_changed = true;
        self.dirty = true;
    }

    fn update_capture(&mut self, mapper: &mut InputMapper, dt: f32) {
        let Some(cap) = self.capture else { return };
        let Some((raw, value)) = mapper.take_capture() else {
            let remaining = cap.remaining - dt;
            if remaining <= 0.0 {
                mapper.cancel_capture();
                self.capture = None;
                self.show(Toast::Cancelled);
            } else {
                self.capture = Some(CaptureState { remaining, ..cap });
            }
            return;
        };
        // The back input cancels (so it cannot be captured from here; the settings file
        // can still bind it).
        let is_back = self
            .bindings
            .get(raw.device(), Action::MenuBack)
            .iter()
            .any(|b| b.raw == raw && b.hold.is_none());
        if is_back {
            self.capture = None;
            self.show(Toast::Cancelled);
            return;
        }
        if raw.device() != cap.device {
            // An input from the other device: say so and keep waiting.
            self.show(Toast::WrongDevice(cap.device));
            mapper.arm_capture();
            return;
        }
        self.capture = None;
        let binding = capture_binding(cap.action, cap.sign, raw, value);
        match self.bindings.capture(cap.action, binding) {
            Ok(()) => self.bound(cap.device, cap.action, cap.sign, binding),
            Err(conflict) => {
                self.conflict = Some(PendingConflict {
                    conflict,
                    sign: cap.sign,
                });
                if let Some(menu) = &mut self.menu {
                    menu.push(Page::Conflict);
                }
            }
        }
    }

    /// `binding` now drives `action`: it replaces the other inputs of that row (direction)
    /// on the device.
    fn bound(&mut self, device: DeviceKind, action: Action, sign: Sign, binding: Binding) {
        let slot = if binding.raw.is_analog() && binding.sign == Sign::Full {
            Sign::Full
        } else {
            sign
        };
        let kept: Vec<Binding> = self
            .bindings
            .get(device, action)
            .iter()
            .copied()
            .filter(|b| *b == binding || !covers(b, slot))
            .collect();
        self.bindings.set(device, action, kept);
        self.bindings_edited();
        self.show(Toast::Bound(binding, action, sign));
    }

    fn settle_conflict(&mut self, use_new: bool) {
        let Some(p) = self.conflict.take() else {
            return;
        };
        let c = p.conflict;
        if use_new {
            if self.bindings.resolve_conflict(&c, c.wanted) {
                self.bound(c.device, c.wanted, p.sign, c.binding);
            }
        } else {
            self.bindings.resolve_conflict(&c, c.existing);
            self.show(Toast::Kept(c.binding, c.existing));
        }
        if let Some(menu) = &mut self.menu {
            if menu.page() == Page::Conflict {
                menu.pop();
            }
        }
        self.save_if_dirty();
    }

    /// Records one frame for the overlay.
    fn note_frame(&mut self, dt: f32, steps: u32) {
        self.frame_ms[self.frame_head] = dt * 1000.0;
        self.frame_head = (self.frame_head + 1) % FRAME_HISTORY;
        self.last_steps = steps;
    }
}

/// Whether binding `b` feeds row `slot` (`Full` is the whole action): analog inputs read
/// whole are in both directions' rows; keys and half axes only in their own direction's.
fn covers(b: &Binding, slot: Sign) -> bool {
    match (slot, b.sign) {
        (Sign::Full, _) => true,
        (_, Sign::Full) if b.raw.is_analog() => true,
        (Sign::Negative, s) => s == Sign::Negative,
        (_, s) => s != Sign::Negative,
    }
}

/// The binding a captured raw input makes for one row of an action: two-way actions take a
/// whole stick axis (or a key for one direction), one-way actions the half of the stick
/// that was pushed; the analog triggers bind as axes for analog actions. Reset on a pad
/// button is a long press, as by default.
pub fn capture_binding(action: Action, sign: Sign, raw: RawInput, value: f32) -> Binding {
    let half = if value < 0.0 {
        Sign::Negative
    } else {
        Sign::Positive
    };
    match raw {
        RawInput::PadButton(b @ (PadButton::L2 | PadButton::R2)) if action.is_analog() => {
            let axis = if b == PadButton::L2 {
                PadAxis::L2
            } else {
                PadAxis::R2
            };
            Binding::new(RawInput::PadAxis(axis), Sign::Full)
        }
        RawInput::PadAxis(PadAxis::L2 | PadAxis::R2) => Binding::new(raw, Sign::Full),
        RawInput::PadAxis(_) if action.is_bipolar() => Binding::new(raw, Sign::Full),
        RawInput::PadAxis(_) => Binding::new(raw, half),
        RawInput::PadButton(_) if action == Action::Reset => {
            Binding::new(raw, Sign::Positive).held_long()
        }
        _ => Binding::new(
            raw,
            if sign == Sign::Negative {
                Sign::Negative
            } else {
                Sign::Positive
            },
        ),
    }
}

impl App {
    /// One frame: samples `input`, runs the UI (menus, remapping, overlay toggle), steps
    /// the simulation unless a menu has it paused, streams the town round the view (one
    /// streaming step per sim step, at least one a frame even when the sim does not run,
    /// each spending up to the budget as `now_seconds` measures it: the platform's clock,
    /// any origin), and lays out the HUD for [`App::scene_view`]. This is the entry point
    /// for platforms; [`App::update`] is the bare simulation step. Then call
    /// [`App::sync_chunks`].
    pub fn frame(
        &mut self,
        input: &mut InputMapper,
        dt_real: f64,
        now_seconds: &dyn Fn() -> f64,
    ) -> Steps {
        let dt = if dt_real.is_finite() {
            dt_real.max(0.0)
        } else {
            0.0
        };
        if self.ui.take_bindings_changed() {
            input.set_bindings(self.ui.bindings.clone());
        }
        let state = input.sample(dt as f32).clone();
        let was_open = self.ui.is_open();
        self.ui
            .handle(&state, input, &mut self.time.clock, dt as f32);
        if self.ui.take_bindings_changed() {
            input.set_bindings(self.ui.bindings.clone());
        }
        if self.ui.take_camera_changed() {
            self.camera.set_mode(self.ui.settings.camera);
        }
        let driving = !was_open && !self.ui.is_open() && !self.ui.full_map_open() && !self.still;
        if let Some((i, paint)) = self.ui.take_vehicle_pick() {
            self.switch_to(i, paint, true);
        } else if state.just_pressed(Action::VehicleNext) && driving {
            self.cycle_vehicle();
        }
        if self.ui.take_reset_pick() || (state.just_pressed(Action::Reset) && driving) {
            self.reset_car();
        }
        self.apply_clock_speed();
        self.apply_settings();
        let radio_keys = !was_open && !self.ui.is_open() && !self.ui.full_map_open();
        if !self.still {
            self.ui.radio_frame(&state, dt as f32, radio_keys);
        }
        let steps = if self.ui.is_open() {
            Steps {
                count: 0,
                alpha: self.alpha,
            }
        } else if was_open || self.ui.full_map_open() {
            // The press that closed the menu (Space on Resume) is not also a drive input,
            // and the full map takes the sticks and keys: the car coasts under it.
            self.advance(&InputState::default(), dt, now_seconds)
        } else {
            self.advance(&state, dt, now_seconds)
        };
        self.alpha = steps.alpha;
        if std::mem::take(&mut self.camera_cycled) {
            self.ui.remember_camera(self.camera.selected());
        }
        let stream_steps = if steps.count > 0 {
            steps.count
        } else {
            (dt / SIM_DT)
                .round()
                .clamp(1.0, f64::from(MAX_STEPS_PER_FRAME)) as u32
        };
        self.stream_steps(stream_steps, steps.alpha, now_seconds);
        self.ui.note_frame(dt as f32, steps.count);
        self.ui.speed_mph = if self.fly {
            None
        } else {
            Some(self.hud_state().speed_mps.abs() * MPH_PER_MPS)
        };
        let daylight = self.time.daylight();
        let camera = if self.fly {
            self.curr.pos
        } else {
            self.camera.pose(&self.drive, steps.alpha).eye
        };
        let (car, heading) = self.map_focus(steps.alpha);
        let radio_display = self.radio_display(steps.alpha);
        let ctx = draw::DrawCtx {
            daylight: &daylight,
            clock: &self.time.clock,
            camera: camera.to_array(),
            ticks: self.ticks,
            car,
            heading,
            radio_display,
        };
        self.ui.draw(&ctx);
        self.update_preview(dt as f32);
        self.audio = self.sound.params(
            &self.drive,
            &self.ground,
            &daylight,
            &self.ui.settings,
            self.ui.is_open(),
        );
        // The engine and the world duck under a station that is playing; the dial sweeps
        // between stations at the radio's volume.
        let radio = self.ui.radio.radio();
        if radio.is_live() {
            self.audio.radio = self.ui.settings.radio.volume;
        }
        self.audio.tunes = radio.tunes();
        self.audio.tune_volume = self.ui.settings.radio.volume;
        // The listener is the camera, moving with the car.
        let velocity = glam::Vec3::from(self.drive.state.vel);
        let (eye, forward) = if self.fly {
            let (s, c) = self.curr.yaw.sin_cos();
            (self.curr.pos, glam::Vec3::new(-s, 0.0, -c))
        } else {
            let pose = self.camera.pose(&self.drive, steps.alpha);
            (pose.eye, pose.forward())
        };
        self.audio.traffic = self.npcs.voices(steps.alpha, eye, forward, velocity);
        self.ui.traffic = (self.npcs.count(), self.npcs.step_ms());
        self.ui.peds = (self.npcs.crowd.count(), self.npcs.crowd.step_ms());
        steps
    }

    /// The sound parameters [`App::frame`] made, for the platform's
    /// `audio::mixer::Mixer::set_params`.
    pub fn audio_params(&self) -> &audio::synth::AudioParams {
        &self.audio
    }

    /// Where the map centers and which way the arrow points: the car (or the free-fly
    /// camera) `alpha` of the way into the step, `[x, z]` and the sim's heading.
    fn map_focus(&self, alpha: f32) -> ([f32; 2], f32) {
        if self.fly {
            let c = self.prev.lerp(&self.curr, alpha);
            ([c.pos.x, c.pos.z], c.yaw)
        } else {
            let pose = self.drive.pose(alpha);
            let f = pose.forward();
            (
                [pose.pos.x, pose.pos.z],
                crate::map::view::heading_of([f.x, f.z]),
            )
        }
    }

    /// Where the radio's display quad is on screen this frame, in the cockpit view only:
    /// its corners in normalized device coordinates, with the aspect they were projected
    /// at and the display's glow color. `None` when it is behind the eye.
    fn radio_display(&self, alpha: f32) -> Option<draw::RadioDisplay> {
        if self.fly || self.camera.mode() != crate::CameraMode::Cockpit {
            return None;
        }
        let pose = self.camera.pose(&self.drive, alpha);
        let proj = glam::camera::rh::proj::directx::perspective(
            pose.fov_y_rad,
            self.aspect,
            pose.near_m,
            pose.far_m,
        );
        let clip_from_world = proj * pose.view();
        let body = self.drive.pose(alpha);
        let mut ndc = [[0.0; 2]; 4];
        for (out, &p) in ndc.iter_mut().zip(&self.drive.layout.radio_display) {
            let c = clip_from_world * body.point(p).extend(1.0);
            if c.w <= pose.near_m * 0.5 {
                return None;
            }
            *out = [c.x / c.w, c.y / c.w];
        }
        let [r, g, b] = self.drive.layout.radio_glow;
        Some(draw::RadioDisplay {
            ndc,
            aspect: self.aspect,
            target: self.target,
            color: [r, g, b, 255],
        })
    }

    /// The size of the target the HUD is drawn on, pixels (the platform calls this at
    /// start and on every resize; it sizes the dash's radio text).
    pub fn set_target_size(&mut self, width: u32, height: u32) {
        self.target = Some((width.max(1), height.max(1)));
    }

    /// The radio's commands for the platform's player since the last call (see
    /// [`audio::RadioCommand`]). Call once a frame, after [`App::frame`].
    pub fn radio_commands(&mut self) -> std::vec::Drain<'_, audio::RadioCommand> {
        self.ui.radio.commands()
    }

    /// A report from the platform's player.
    pub fn radio_event(&mut self, event: audio::PlayerEvent) {
        self.ui.radio.on_event(event);
    }

    /// The radio's state, station list and volume.
    pub fn radio(&self) -> &audio::Radio {
        self.ui.radio.radio()
    }

    /// The platform can ask the user for a stream URL: the Audio page offers "Add stream
    /// URL", and [`App::take_stream_url_request`] says when it was chosen.
    pub fn set_text_entry(&mut self, on: bool) {
        self.ui.text_entry = on;
    }

    /// True once after the user chose "Add stream URL": ask for one and pass it to
    /// [`App::add_stream_url`].
    pub fn take_stream_url_request(&mut self) -> bool {
        self.ui.radio.take_url_request()
    }

    /// Adds a stream the user typed to the stations (an `https://` URL), and says so.
    pub fn add_stream_url(&mut self, url: &str) {
        if url.trim().is_empty() {
            return;
        }
        let ui = &mut self.ui;
        match ui.settings.radio.add_custom(url) {
            Ok(_) => {
                ui.radio_settings_changed();
                ui.save_if_dirty();
                ui.show(Toast::StreamAdded);
            }
            Err(_) => ui.show(Toast::StreamRejected),
        }
    }

    /// Loads settings and bindings from `store` (the bindings reach the mapper on the next
    /// [`App::frame`]) and saves there from now on. The saved vehicle and paint replace the
    /// car the app was built with unless a scene named that one (`car=`); its meshes then
    /// wait in [`App::take_new_meshes`].
    pub fn set_settings_store(&mut self, store: Box<dyn SettingsStore>) {
        self.ui.set_store(store);
        if !self.camera_from_scene {
            self.camera.set_mode(self.ui.settings.camera);
        }
        if !self.car_from_scene {
            // The saved car, in the scene's paint (not saved) or its saved one.
            let saved = self.ui.settings.vehicle.and_then(roster::index_of);
            let paint = self.paint_from_scene.unwrap_or(self.ui.settings.paint);
            if let Some(i) =
                saved.filter(|&i| (i, paint) != (self.ui.vehicle, self.ui.vehicle_paint))
            {
                let remember = self.paint_from_scene.is_none();
                let switched = self.switch_vehicle_at(i, paint, false, remember);
                debug_assert!(switched.is_ok(), "{switched:?}");
            }
        }
        self.apply_clock_speed();
        self.apply_settings();
        // Loading: traffic the saved setting switched on warms up before the first frame.
        self.finish_warm_up();
    }

    /// The handling preset drives the car's assists; the camera settings reach the rig.
    fn apply_settings(&mut self) {
        self.apply_traffic(self.traffic_scene.unwrap_or(self.ui.settings.traffic));
        let s = &self.ui.settings;
        self.drive.assists = s.handling.assists();
        self.camera.prefs = crate::camera::CameraPrefs {
            cockpit_fov_deg: s.cockpit_fov_deg,
            seat_up_m: s.seat_height_cm * 0.01,
            seat_forward_m: s.seat_forward_cm * 0.01,
            shake: s.camera_shake,
        };
    }

    /// The clock runs at the clock-speed setting, unless the scene froze it at its time.
    fn apply_clock_speed(&mut self) {
        self.time.clock.speed = if self.clock_frozen {
            0.0
        } else {
            self.ui.settings.clock_speed
        };
    }

    /// The renderer's counters for the last frame, for the performance overlay.
    pub fn set_render_stats(&mut self, stats: RenderStats) {
        self.ui.render_stats = stats;
    }

    /// CPU time the platform spent in the last frame's `scene_view` and
    /// `Renderer::render` (recording and submitting), ms, for the overlay.
    pub fn set_cpu_submit_ms(&mut self, ms: f32) {
        self.ui.cpu_submit_ms = ms;
    }

    /// What the GPU is: picks the preset for a player who has not chosen one
    /// ([`Quality::auto`]) and labels the overlay. Call once, before the first frame.
    pub fn set_gpu_info(&mut self, gpu: &GpuInfo) {
        self.ui.auto_quality = Quality::auto(gpu);
        self.ui.auto_frame_rate = FrameRate::auto(gpu);
        self.ui.gpu_label = if gpu.backend.is_empty() {
            gpu.name.clone()
        } else {
            format!("{} ({})", gpu.name, gpu.backend)
        };
    }

    /// The window gained or lost the focus (or was hidden): the radio goes quiet in the
    /// background when the setting says so.
    pub fn set_focused(&mut self, focused: bool) {
        if self.ui.focused != focused {
            self.ui.focused = focused;
            self.ui.radio.set_gain(self.ui.radio_gain());
            self.ui.radio.apply(&self.ui.settings.radio);
        }
    }

    /// How often to draw: the frame-rate cap in effect (Settings › Video › Frame rate).
    pub fn frame_rate(&self) -> FrameRate {
        self.ui.frame_rate()
    }

    /// Uses preset `q` this session whatever is saved (`None` goes back to the setting).
    pub fn force_quality(&mut self, q: Option<Quality>) {
        self.ui.forced_quality = q;
    }

    /// The renderer's construction options for this app: its chunk pool and the
    /// quality in effect (each frame's `SceneView::quality` keeps it current).
    pub fn renderer_options(&self) -> render::RendererOptions {
        render::RendererOptions {
            chunk_pool: self.chunk_pool_config(),
            quality: self.ui.render_quality(),
            ..render::RendererOptions::default()
        }
    }

    /// Opens the settings menu (over the pause menu), e.g. at startup for screenshots.
    pub fn open_settings(&mut self) {
        self.open_menu(Page::Settings);
    }

    /// Opens the menu on `page` (over the pause menu), e.g. at startup for screenshots.
    pub fn open_menu(&mut self, page: Page) {
        self.ui.open(page, &InputState::default());
        self.ui.focus_current_vehicle();
        self.ui.rebuild(&self.time.clock);
    }

    /// Whether the pause menu offers Quit (native: yes; the web cannot close its tab).
    pub fn set_can_quit(&mut self, can_quit: bool) {
        self.ui.can_quit = can_quit;
    }

    /// The player chose Quit.
    pub fn quit_requested(&self) -> bool {
        self.ui.quit
    }

    /// A menu is open (and the simulation paused).
    pub fn menu_open(&self) -> bool {
        self.ui.is_open()
    }

    pub fn settings(&self) -> &Settings {
        self.ui.settings()
    }

    pub fn bindings(&self) -> &Bindings {
        self.ui.bindings()
    }

    pub fn ui(&self) -> &Ui {
        &self.ui
    }
}

#[cfg(test)]
mod tests;
