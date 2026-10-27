//! Input: raw platform events → [`Bindings`] → [`InputMapper`] → one [`InputState`] per
//! frame. See `docs/CONTROLS_INPUT.md`.
//!
//! The platform calls [`InputMapper::press`], [`InputMapper::release`],
//! [`InputMapper::axis`] and [`InputMapper::mouse_delta`] as events arrive, then
//! [`InputMapper::sample`] once at the start of each frame. Latest state wins; nothing is
//! queued except just-pressed edges.

mod bindings;
mod json;
mod mapper;
pub mod pad_table;
mod raw;

pub use bindings::{AxisOptions, Binding, Bindings, Conflict, Sign};
pub use json::{BindingsError, BINDINGS_VERSION};
pub use mapper::{InputMapper, MOUSE_LOOK_PER_PIXEL};
pub use raw::{
    DeviceKind, Key, MouseAxis, PadAxis, PadButton, RawInput, MOUSE_BUTTONS, MOUSE_RIGHT,
};

/// Every game action. Order is the slot order in [`InputState`].
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// `[0, 1]`.
    Throttle,
    /// Brake / reverse, `[0, 1]`.
    Brake,
    /// Left (-) / right (+), `[-1, 1]`.
    Steer,
    Handbrake,
    CameraCycle,
    /// Look left (-) / right (+), `[-1, 1]`.
    LookX,
    /// Look up (-) / down (+), `[-1, 1]`: screen space, like the raw mouse and stick.
    /// `invert` flips it.
    LookY,
    LookBack,
    /// Next station; on the deck, the next track.
    RadioNext,
    /// Previous station; on the deck, the previous track (or the start of this one).
    RadioPrev,
    /// The radio's power; on the deck, play/pause.
    RadioOff,
    /// Switch between the radio and the deck (tape, CD or MP3 player by era).
    DeckSource,
    /// The deck's next mode: Normal, Shuffle, Continuous.
    DeckMode,
    /// The deck's next / previous album (tape or disc).
    AlbumNext,
    AlbumPrev,
    Headlights,
    /// High beams on/off with the headlights on; with them off, headlights on with high beams.
    HighBeams,
    Horn,
    /// Swap to the next built-in vehicle, in place.
    VehicleNext,
    /// Put the car back on the nearest lane, at rest (a long press on the pad).
    Reset,
    MapToggle,
    /// Full map zoom out (-) / in (+), `[-1, 1]`.
    MapZoom,
    /// Time of day backwards (-) / forwards (+), `[-1, 1]`.
    TimeScrub,
    Pause,
    Screenshot,
    DebugOverlay,
    /// Show or hide the controls card (a long press on the pad).
    Controls,
    MenuUp,
    MenuDown,
    MenuLeft,
    MenuRight,
    MenuConfirm,
    MenuBack,
    /// Free-fly camera (until the car exists), `[0, 1]` each.
    FlyForward,
    FlyBack,
    FlyLeft,
    FlyRight,
    FlyUp,
    FlyDown,
    /// Free-fly turn left (-) / right (+), `[-1, 1]`.
    FlyLookX,
    /// Free-fly look up (-) / down (+), `[-1, 1]`, same convention as [`Action::LookY`].
    FlyLookY,
}

/// The mode an action belongs to. A raw input may drive one action per context on each
/// device (Space is both Handbrake and FlyUp); within a context, a second binding of the
/// same input is a [`Conflict`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Drive,
    Menu,
    Fly,
}

impl Action {
    pub const COUNT: usize = 41;
    pub const ALL: [Action; Action::COUNT] = [
        Action::Throttle,
        Action::Brake,
        Action::Steer,
        Action::Handbrake,
        Action::CameraCycle,
        Action::LookX,
        Action::LookY,
        Action::LookBack,
        Action::RadioNext,
        Action::RadioPrev,
        Action::RadioOff,
        Action::DeckSource,
        Action::DeckMode,
        Action::AlbumNext,
        Action::AlbumPrev,
        Action::Headlights,
        Action::HighBeams,
        Action::Horn,
        Action::VehicleNext,
        Action::Reset,
        Action::MapToggle,
        Action::MapZoom,
        Action::TimeScrub,
        Action::Pause,
        Action::Screenshot,
        Action::DebugOverlay,
        Action::Controls,
        Action::MenuUp,
        Action::MenuDown,
        Action::MenuLeft,
        Action::MenuRight,
        Action::MenuConfirm,
        Action::MenuBack,
        Action::FlyForward,
        Action::FlyBack,
        Action::FlyLeft,
        Action::FlyRight,
        Action::FlyUp,
        Action::FlyDown,
        Action::FlyLookX,
        Action::FlyLookY,
    ];

    /// Analog actions: these sum their inputs, run the per-device [`AxisOptions`] pipeline
    /// and may ramp. Every other action is a button whose value is its strongest input.
    pub const ANALOG: [Action; 9] = [
        Action::Throttle,
        Action::Brake,
        Action::Steer,
        Action::LookX,
        Action::LookY,
        Action::TimeScrub,
        Action::MapZoom,
        Action::FlyLookX,
        Action::FlyLookY,
    ];

    /// Index into [`Action::ANALOG`], for analog actions.
    pub fn analog_slot(self) -> Option<usize> {
        Action::ANALOG.iter().position(|&a| a == self)
    }

    pub fn is_analog(self) -> bool {
        self.analog_slot().is_some()
    }

    /// True when the action's range is `[-1, 1]`; otherwise it is `[0, 1]`.
    pub fn is_bipolar(self) -> bool {
        matches!(
            self,
            Action::Steer
                | Action::LookX
                | Action::LookY
                | Action::TimeScrub
                | Action::MapZoom
                | Action::FlyLookX
                | Action::FlyLookY
        )
    }

    pub fn context(self) -> Context {
        match self {
            Action::MenuUp
            | Action::MenuDown
            | Action::MenuLeft
            | Action::MenuRight
            | Action::MenuConfirm
            | Action::MenuBack
            | Action::MapZoom => Context::Menu,
            Action::FlyForward
            | Action::FlyBack
            | Action::FlyLeft
            | Action::FlyRight
            | Action::FlyUp
            | Action::FlyDown
            | Action::FlyLookX
            | Action::FlyLookY => Context::Fly,
            _ => Context::Drive,
        }
    }

    /// Keyboard (digital input) ramp as `(seconds from 0 to full, seconds back to 0)`, for
    /// the actions that have one. Steering ramps out in 120 ms (was 150 ms until the first
    /// play found corrections slow to land).
    pub fn ramp(self) -> Option<(f32, f32)> {
        match self {
            Action::Steer => Some((0.120, 0.300)),
            Action::Throttle | Action::Brake => Some((0.100, 0.100)),
            _ => None,
        }
    }

    /// The stable name used in settings JSON.
    pub fn name(self) -> &'static str {
        match self {
            Action::Throttle => "Throttle",
            Action::Brake => "Brake",
            Action::Steer => "Steer",
            Action::Handbrake => "Handbrake",
            Action::CameraCycle => "CameraCycle",
            Action::LookX => "LookX",
            Action::LookY => "LookY",
            Action::LookBack => "LookBack",
            Action::RadioNext => "RadioNext",
            Action::RadioPrev => "RadioPrev",
            Action::RadioOff => "RadioOff",
            Action::DeckSource => "DeckSource",
            Action::DeckMode => "DeckMode",
            Action::AlbumNext => "AlbumNext",
            Action::AlbumPrev => "AlbumPrev",
            Action::Headlights => "Headlights",
            Action::HighBeams => "HighBeams",
            Action::Horn => "Horn",
            Action::VehicleNext => "VehicleNext",
            Action::Reset => "Reset",
            Action::MapToggle => "MapToggle",
            Action::MapZoom => "MapZoom",
            Action::TimeScrub => "TimeScrub",
            Action::Pause => "Pause",
            Action::Screenshot => "Screenshot",
            Action::DebugOverlay => "DebugOverlay",
            Action::Controls => "Controls",
            Action::MenuUp => "MenuUp",
            Action::MenuDown => "MenuDown",
            Action::MenuLeft => "MenuLeft",
            Action::MenuRight => "MenuRight",
            Action::MenuConfirm => "MenuConfirm",
            Action::MenuBack => "MenuBack",
            Action::FlyForward => "FlyForward",
            Action::FlyBack => "FlyBack",
            Action::FlyLeft => "FlyLeft",
            Action::FlyRight => "FlyRight",
            Action::FlyUp => "FlyUp",
            Action::FlyDown => "FlyDown",
            Action::FlyLookX => "FlyLookX",
            Action::FlyLookY => "FlyLookY",
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.name() == name)
    }

    /// Clamps a value into this action's range; non-finite values become 0.
    pub fn clamp(self, value: f32) -> f32 {
        let v = if value.is_finite() { value } else { 0.0 };
        let lo = if self.is_bipolar() { -1.0 } else { 0.0 };
        v.clamp(lo, 1.0)
    }
}

/// The value of every action this frame: buttons in `[0, 1]` (pressed above 0.5), analog
/// actions in their range. Values outside the range are clamped on write.
#[derive(Clone, Debug, PartialEq)]
pub struct InputState {
    values: [f32; Action::COUNT],
    just_pressed: [bool; Action::COUNT],
    /// Mouse wheel notches this frame, + away from the user.
    wheel: f32,
}

impl Default for InputState {
    fn default() -> InputState {
        InputState {
            values: [0.0; Action::COUNT],
            just_pressed: [false; Action::COUNT],
            wheel: 0.0,
        }
    }
}

impl InputState {
    pub fn set(&mut self, action: Action, value: f32) {
        self.values[action as usize] = action.clamp(value);
    }

    /// Marks a press edge this frame (see [`InputState::just_pressed`]).
    pub fn set_just_pressed(&mut self, action: Action, edge: bool) {
        self.just_pressed[action as usize] = edge;
    }

    pub fn axis(&self, action: Action) -> f32 {
        self.values[action as usize]
    }

    /// Held this frame: the value is more than halfway from rest.
    pub fn pressed(&self, action: Action) -> bool {
        self.values[action as usize].abs() > 0.5
    }

    /// True on the one frame a press began, even if it was released again before the
    /// frame was sampled.
    pub fn just_pressed(&self, action: Action) -> bool {
        self.just_pressed[action as usize]
    }

    /// Mouse wheel notches since the last frame, + away from the user (zooms in). Not an
    /// action: one notch is one step, whatever the frame time.
    pub fn wheel(&self) -> f32 {
        self.wheel
    }

    pub fn set_wheel(&mut self, notches: f32) {
        self.wheel = if notches.is_finite() { notches } else { 0.0 };
    }

    pub fn clear(&mut self) {
        *self = InputState::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_index_their_slot_and_names_round_trip() {
        for (i, a) in Action::ALL.iter().enumerate() {
            assert_eq!(*a as usize, i);
            assert_eq!(Action::from_name(a.name()), Some(*a));
            assert_eq!(format!("{a:?}"), a.name());
        }
        assert_eq!(Action::from_name("Jump"), None);
        for (i, a) in Action::ANALOG.iter().enumerate() {
            assert_eq!(a.analog_slot(), Some(i));
        }
        assert!(!Action::Horn.is_analog() && !Action::Horn.is_bipolar());
        assert!(Action::Throttle.is_analog() && !Action::Throttle.is_bipolar());
        assert_eq!(Action::MenuBack.context(), Context::Menu);
        assert_eq!(Action::MapZoom.context(), Context::Menu);
        assert!(Action::MapZoom.is_analog() && Action::MapZoom.is_bipolar());
        assert_eq!(Action::FlyLookY.context(), Context::Fly);
        assert_eq!(Action::Pause.context(), Context::Drive);
    }

    #[test]
    fn set_clamps_to_the_action_range() {
        let mut s = InputState::default();
        s.set(Action::LookX, 3.0);
        assert_eq!(s.axis(Action::LookX), 1.0);
        s.set(Action::LookX, -3.0);
        assert_eq!(s.axis(Action::LookX), -1.0);
        s.set(Action::Throttle, -0.5);
        assert_eq!(s.axis(Action::Throttle), 0.0);
        s.set(Action::LookY, f32::NAN);
        assert_eq!(s.axis(Action::LookY), 0.0);
        s.set(Action::FlyForward, 1.0);
        s.set_just_pressed(Action::Horn, true);
        assert!(s.pressed(Action::FlyForward) && !s.pressed(Action::FlyBack));
        assert!(s.just_pressed(Action::Horn) && !s.pressed(Action::Horn));
        s.clear();
        assert_eq!(s, InputState::default());
    }
}
