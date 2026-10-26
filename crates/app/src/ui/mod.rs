//! In-game UI: the pause and settings menus (gamepad first), the remap capture flow, the
//! driving HUD and the performance overlay, plus loading and saving the settings.
//!
//! The UI owns the user's [`Settings`] and [`Bindings`]; the platform's [`InputMapper`]
//! gets a copy of the bindings whenever they change. While a menu is open the simulation
//! is paused and only the Menu actions are read. Every frame the UI draws itself into a
//! [`HudBuilder`] that [`crate::App::scene_view`] hands to the renderer.

mod draw;
mod menu;
mod names;
mod pages;

pub use menu::{Effect, Item, MenuInput, MenuNav, Nav, Repeat, Unit, Widget};
pub use pages::{ItemId, Page};

use crate::daylight::{Clock, CLOCK_SPEEDS, DAY_SECONDS};
use crate::input::{
    Action, AxisOptions, Binding, Bindings, Conflict, DeviceKind, InputMapper, InputState, PadAxis,
    PadButton, RawInput, Sign,
};
use crate::settings::{Settings, SettingsStore};
use crate::App;
use render::{Hud, HudBuilder, RenderStats, Sky};
use skywave_core::Steps;

/// Seconds a remap capture waits for an input before giving up.
pub const CAPTURE_TIMEOUT: f32 = 8.0;
/// Seconds a status message stays up.
const TOAST_SECONDS: f32 = 2.5;
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
}

pub struct Ui {
    settings: Settings,
    bindings: Bindings,
    /// The mapper needs the new bindings.
    bindings_changed: bool,
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
        }
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
        self.store = Some(store);
        self.dirty = false;
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
                Some(menu) => menu.apply(nav, &self.items),
                None => Effect::None,
            };
            self.apply(effect, clock, mapper);
            self.rebuild(clock);
        }
    }

    fn apply(&mut self, effect: Effect, clock: &mut Clock, mapper: &mut InputMapper) {
        match effect {
            Effect::None | Effect::Moved | Effect::Opened(_) => {}
            Effect::Toggle(id, on) => self.set_toggle(id, on),
            Effect::Slide(id, v) => self.set_slider(id, v, clock),
            Effect::Choose(ItemId::ClockSpeed, i) => {
                self.settings.clock_speed = CLOCK_SPEEDS[i.min(CLOCK_SPEEDS.len() - 1)];
                self.dirty = true;
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
            ItemId::TimeOfDay => {
                clock.seconds_of_day = (f64::from(v) * 3600.0).rem_euclid(DAY_SECONDS);
            }
            _ => {}
        }
        self.settings = self.settings.sanitized();
    }

    fn activate(&mut self, id: ItemId) {
        match id {
            ItemId::Resume => self.close(),
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
            _ => {}
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
/// that was pushed; the analog triggers bind as axes for analog actions.
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
    /// the simulation unless a menu has it paused, and lays out the HUD for
    /// [`App::scene_view`]. This is the entry point for platforms; [`App::update`] is the
    /// bare simulation step.
    pub fn frame(&mut self, input: &mut InputMapper, dt_real: f64) -> Steps {
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
        self.apply_clock_speed();
        let steps = if self.ui.is_open() {
            Steps {
                count: 0,
                alpha: self.alpha,
            }
        } else if was_open {
            // The press that closed the menu (Space on Resume) is not also a drive input.
            self.update(&InputState::default(), dt)
        } else {
            self.update(&state, dt)
        };
        self.alpha = steps.alpha;
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
        let ctx = draw::DrawCtx {
            daylight: &daylight,
            clock: &self.time.clock,
            camera: camera.to_array(),
            ticks: self.ticks,
        };
        self.ui.draw(&ctx);
        steps
    }

    /// Loads settings and bindings from `store` (the bindings reach the mapper on the next
    /// [`App::frame`]) and saves there from now on.
    pub fn set_settings_store(&mut self, store: Box<dyn SettingsStore>) {
        self.ui.set_store(store);
        self.apply_clock_speed();
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

    /// Opens the settings menu (over the pause menu), e.g. at startup for screenshots.
    pub fn open_settings(&mut self) {
        self.ui.open(Page::Settings, &InputState::default());
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
