//! Native gamepads through gilrs (evdev + udev on Linux, so the Steam Deck's built-in
//! controller and Steam's virtual pad are ordinary gamepads; IOKit on macOS;
//! Windows.Gaming.Input on Windows). gilrs applies its bundled SDL controller database and
//! `SDL_GAMECONTROLLERCONFIG` (which Steam sets), so buttons arrive already named by
//! position, and are fed to the mapper through the shared standard-mapping table.
//!
//! gilrs conventions, and what changes here:
//! - Sticks are `[-1, 1]`, +X right and +Y **up** on every platform (gilrs flips the
//!   platforms whose raw Y points down). Ours are +Y down like the W3C standard mapping,
//!   so stick Y is negated.
//! - The triggers are buttons (`LeftTrigger2`/`RightTrigger2`) whose `ButtonChanged`
//!   value is `[0, 1]`; gilrs also reports them pressed past its own threshold. Each
//!   trigger feeds both the L2/R2 button and the L2/R2 axis. `LeftTrigger`/`RightTrigger`
//!   are the bumpers (L1/R1), `Mode` is the guide button, `LeftThumb`/`RightThumb` are the
//!   stick clicks.
//! - `LeftZ`/`RightZ` (generic axes of unmapped devices) and `C`/`Z` are ignored, except
//!   as below; `DPadX`/`DPadY` hat axes become d-pad buttons in gilrs's default filters.
//! - gilrs matches SDL mappings by exact GUID, firmware version included, so a pad whose
//!   firmware is newer than the database falls back to raw evdev codes on Linux. There we
//!   follow SDL's own fallback for unmapped Linux pads ([`Quirks`]): `BTN_X` (which evdev
//!   also calls `BTN_NORTH`) is the left face button except on Sony and Nintendo pads, and
//!   `ABS_Z`/`ABS_RZ` are the analog triggers when the right stick is on `ABS_RX`/`ABS_RY`.
//!
//! The first connected pad is "the" pad; others are ignored until it disconnects. On macOS a
//! controller Apple's Game Controller framework knows takes precedence (`pad_mac`).

use std::fmt::Write as _;

use app::{pad_table, InputMapper};
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs, MappingSource};

/// gilrs's button at each W3C standard-mapping index (see `app::pad_table::BUTTONS`).
pub(crate) const BUTTONS: [Button; 17] = [
    Button::South,
    Button::East,
    Button::West,
    Button::North,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::LeftTrigger2,
    Button::RightTrigger2,
    Button::Select,
    Button::Start,
    Button::LeftThumb,
    Button::RightThumb,
    Button::DPadUp,
    Button::DPadDown,
    Button::DPadLeft,
    Button::DPadRight,
    Button::Mode,
];

/// gilrs's axis at each standard-mapping axis index, and the factor that turns its value
/// into the standard convention (+Y down).
pub(crate) const AXES: [(Axis, f32); 4] = [
    (Axis::LeftStickX, 1.0),
    (Axis::LeftStickY, -1.0),
    (Axis::RightStickX, 1.0),
    (Axis::RightStickY, -1.0),
];

fn button_index(b: Button) -> Option<usize> {
    BUTTONS.iter().position(|&x| x == b)
}

fn axis_index(a: Axis) -> Option<(usize, f32)> {
    AXES.iter()
        .position(|&(x, _)| x == a)
        .map(|i| (i, AXES[i].1))
}

/// How the active pad's readings are corrected before they reach the mapper: only for
/// Linux pads gilrs found no SDL mapping for (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quirks {
    /// `North` and `West` are swapped: evdev's `BTN_X` is the left face button.
    pub swap_xy: bool,
    /// `LeftZ`/`RightZ` are the analog triggers, `-1` released to `1` fully pulled.
    pub z_triggers: bool,
}

/// USB vendors whose Linux drivers name the face buttons by position.
const POSITIONAL_VENDORS: [u16; 2] = [0x054c, 0x057e];

/// The trigger value past which a `z_triggers` trigger counts as pressed (gilrs's own
/// axis-to-button threshold).
const TRIGGER_PRESSED: f32 = 0.75;

impl Quirks {
    fn of(pad: &gilrs::Gamepad) -> Quirks {
        if !cfg!(target_os = "linux") || pad.mapping_source() != MappingSource::Driver {
            return Quirks::default();
        }
        let has = |a| pad.axis_code(a).is_some();
        Quirks {
            swap_xy: !pad
                .vendor_id()
                .is_some_and(|v| POSITIONAL_VENDORS.contains(&v)),
            z_triggers: has(Axis::LeftZ)
                && has(Axis::RightZ)
                && has(Axis::RightStickX)
                && has(Axis::RightStickY),
        }
    }

    /// `ev` as the standard mapping means it.
    pub fn fix(self, ev: PadEvent) -> PadEvent {
        match ev {
            PadEvent::Button(Button::North, v, p) if self.swap_xy => {
                PadEvent::Button(Button::West, v, p)
            }
            PadEvent::Button(Button::West, v, p) if self.swap_xy => {
                PadEvent::Button(Button::North, v, p)
            }
            PadEvent::Axis(a @ (Axis::LeftZ | Axis::RightZ), v) if self.z_triggers => {
                let b = if a == Axis::LeftZ {
                    Button::LeftTrigger2
                } else {
                    Button::RightTrigger2
                };
                let v = ((v + 1.0) * 0.5).clamp(0.0, 1.0);
                PadEvent::Button(b, v, v >= TRIGGER_PRESSED)
            }
            ev => ev,
        }
    }
}

/// One gamepad reading, apart from gilrs's `EventType` (whose `Code` cannot be built
/// outside gilrs) so the conversion can be tested without hardware.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadEvent {
    /// A button's value `[0, 1]` now, and whether gilrs counts it as pressed.
    Button(Button, f32, bool),
    /// An axis value in gilrs's convention (`[-1, 1]`, +Y up).
    Axis(Axis, f32),
}

/// Feeds one reading to the mapper. Unknown controls are ignored.
pub fn apply(input: &mut InputMapper, ev: PadEvent) {
    match ev {
        PadEvent::Button(b, value, pressed) => {
            if let Some(i) = button_index(b) {
                pad_table::feed_button(input, i, value, pressed);
            }
        }
        PadEvent::Axis(a, value) => {
            if let Some((i, sign)) = axis_index(a) {
                pad_table::feed_axis(input, i, value * sign);
            }
        }
    }
}

/// The backend named in the startup line, so a field report says what read the pads.
#[cfg(target_os = "linux")]
const BACKEND: &str = "gilrs evdev";
#[cfg(target_os = "macos")]
const BACKEND: &str = "Game Controller framework, then gilrs IOKit HID";
#[cfg(target_os = "windows")]
const BACKEND: &str = "gilrs Windows.Gaming.Input";
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const BACKEND: &str = "gilrs";

/// A hot-plug message: to stderr when `verbose` (`--stats`), else the info log.
pub(crate) fn note(verbose: bool, msg: std::fmt::Arguments) {
    if verbose {
        eprintln!("{msg}");
    } else {
        log::info!("{msg}");
    }
}

/// How gilrs names a pad's buttons. Without an SDL mapping the GUID is included, so a
/// field report says what to add to `SDL_GAMECONTROLLERCONFIG`.
fn mapping(pad: &gilrs::Gamepad) -> String {
    match pad.mapping_source() {
        MappingSource::SdlMappings => "SDL mapping".to_string(),
        source => {
            let kind = if source == MappingSource::Driver {
                "driver"
            } else {
                "no"
            };
            let mut s = format!("{kind} mapping, GUID ");
            for b in pad.uuid() {
                let _ = write!(s, "{b:02x}");
            }
            s
        }
    }
}

/// A button's value and pressed state from gilrs's cached state. Looked up by code, not by
/// [`gilrs::Gamepad::button_data`], which misses d-pad buttons gilrs made from a hat.
fn button_state(pad: &gilrs::Gamepad, code: Option<gilrs::ev::Code>) -> (f32, bool) {
    code.and_then(|c| pad.state().button_data(c))
        .map_or((0.0, false), |d| (d.value(), d.is_pressed()))
}

/// The gamepad backend and which pad is active.
pub struct Pads {
    gilrs: Option<Gilrs>,
    active: Option<GamepadId>,
    /// Corrections for the active pad's readings.
    quirks: Quirks,
    /// Print hot-plug messages to stderr (`--stats`) instead of the info log.
    verbose: bool,
    /// The controller the Game Controller framework knows, which overrides gilrs.
    #[cfg(target_os = "macos")]
    mac: crate::pad_mac::GameControllers,
}

impl Pads {
    /// Opens the backend. Failure only disables gamepads; it is logged, never fatal.
    pub fn new(verbose: bool) -> Pads {
        let gilrs = match Gilrs::new() {
            Ok(g) => Some(g),
            Err(gilrs::Error::NotImplemented(dummy)) => {
                log::warn!("gamepads are not supported on this platform");
                Some(dummy)
            }
            Err(e) => {
                log::warn!("gamepads disabled: {e}");
                None
            }
        };
        let mut pads = Pads {
            gilrs,
            active: None,
            quirks: Quirks::default(),
            verbose,
            #[cfg(target_os = "macos")]
            mac: crate::pad_mac::GameControllers::new(verbose),
        };
        let first = pads
            .gilrs
            .as_ref()
            .and_then(|g| g.gamepads().next().map(|(id, _)| id));
        pads.activate(first);
        pads
    }

    /// Makes `id` the active pad, with its quirks.
    fn activate(&mut self, id: Option<GamepadId>) {
        self.active = id;
        self.quirks = match (&self.gilrs, id) {
            (Some(g), Some(id)) => Quirks::of(&g.gamepad(id)),
            _ => Quirks::default(),
        };
    }

    /// Feeds one reading of the active pad.
    fn apply(&self, input: &mut InputMapper, ev: PadEvent) {
        apply(input, self.quirks.fix(ev));
    }

    /// One line naming the backend and the detected pads, e.g. `gamepads (gilrs evdev): 0
    /// "Xbox Controller" (SDL mapping, active)`. Pads found later are announced as they
    /// connect (on macOS that is all of them: both backends find pads as the app starts).
    pub fn summary(&self) -> String {
        let mut line = format!("gamepads ({BACKEND}):");
        #[cfg(target_os = "macos")]
        {
            let _ = write!(line, " game controller {};", self.mac.summary());
        }
        let Some(g) = &self.gilrs else {
            line.push_str(" unavailable");
            return line;
        };
        let mut any = false;
        for (id, pad) in g.gamepads() {
            let mapping = mapping(&pad);
            let active = if Some(id) == self.active {
                ", active"
            } else {
                ""
            };
            let sep = if any { ";" } else { "" };
            let _ = write!(line, "{sep} {id} {:?} ({mapping}{active})", pad.name());
            any = true;
        }
        if !any {
            line.push_str(" none");
        }
        line
    }

    fn note(&self, msg: std::fmt::Arguments) {
        note(self.verbose, msg);
    }

    /// Whether gilrs's pad is ignored: a framework controller drives instead (macOS).
    #[cfg(target_os = "macos")]
    fn overridden(&self) -> bool {
        self.mac.driving()
    }
    #[cfg(not(target_os = "macos"))]
    fn overridden(&self) -> bool {
        false
    }

    /// Drains pending gamepad events into `input`. Hot-plug is always handled; button and
    /// axis events reach the mapper only while `focused` (gilrs still tracks the state, and
    /// [`Pads::resync`] catches up when focus returns). Allocates nothing unless a pad
    /// connects or disconnects.
    pub fn poll(&mut self, input: &mut InputMapper, focused: bool) {
        #[cfg(target_os = "macos")]
        {
            let r = self.mac.poll(input, focused);
            // Back to gilrs's pad, if any, when the framework's controller goes away.
            if r.changed && !r.driving && focused {
                self.resync(input);
            }
        }
        let overridden = self.overridden();
        loop {
            let Some(ev) = self.gilrs.as_mut().and_then(Gilrs::next_event) else {
                return;
            };
            let Some(g) = &self.gilrs else { return };
            let id = ev.id;
            match ev.event {
                EventType::Connected => {
                    let pad = g.gamepad(id);
                    let name = format!("{:?} ({})", pad.name(), mapping(&pad));
                    // Some backends also announce the pads found at startup, one of
                    // which may already be active.
                    let adopt = self.active.is_none_or(|a| a == id);
                    if adopt {
                        self.activate(Some(id));
                    }
                    let role = if adopt { "active" } else { "ignored" };
                    self.note(format_args!("gamepad connected: {id} {name} ({role})"));
                    if adopt && focused {
                        self.resync(input);
                    }
                }
                EventType::Disconnected => {
                    self.note(format_args!("gamepad disconnected: {id}"));
                    if self.active == Some(id) {
                        if !overridden {
                            pad_table::release_gamepad(input);
                        }
                        let next = g.gamepads().map(|(other, _)| other).find(|&x| x != id);
                        self.activate(next);
                        if let Some(next) = next {
                            self.note(format_args!("gamepad {next} is now active"));
                            if focused {
                                self.resync(input);
                            }
                        }
                    }
                }
                _ if !focused || overridden || Some(id) != self.active => {}
                EventType::ButtonPressed(b, code)
                | EventType::ButtonReleased(b, code)
                | EventType::ButtonChanged(b, _, code) => {
                    // The state gilrs just updated carries both the value and its own
                    // pressed threshold, whichever of the two events this was.
                    let (value, pressed) = button_state(&g.gamepad(id), Some(code));
                    self.apply(input, PadEvent::Button(b, value, pressed));
                }
                EventType::AxisChanged(a, value, _) => {
                    self.apply(input, PadEvent::Axis(a, value));
                }
                _ => {}
            }
        }
    }

    /// Feeds the active pad's whole current state, after it takes over or focus returns.
    pub fn resync(&self, input: &mut InputMapper) {
        if self.overridden() {
            // The framework's controller is fed whole every frame anyway.
            return;
        }
        let (Some(g), Some(id)) = (&self.gilrs, self.active) else {
            return;
        };
        let Some(pad) = g.connected_gamepad(id) else {
            return;
        };
        for b in BUTTONS {
            let code = pad.button_code(b).or_else(|| b.to_nec());
            let (value, pressed) = button_state(&pad, code);
            self.apply(input, PadEvent::Button(b, value, pressed));
        }
        for (a, _) in AXES {
            let value = pad.axis_data(a).map_or(0.0, |d| d.value());
            self.apply(input, PadEvent::Axis(a, value));
        }
        if self.quirks.z_triggers {
            // An untouched trigger has no reading yet: it is released.
            for a in [Axis::LeftZ, Axis::RightZ] {
                let value = pad.axis_data(a).map_or(-1.0, |d| d.value());
                self.apply(input, PadEvent::Axis(a, value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::{Action, Binding, Bindings, PadButton, RawInput, Sign};

    #[test]
    fn gilrs_tables_line_up_with_the_standard_mapping() {
        assert_eq!(BUTTONS.len(), pad_table::BUTTONS.len());
        for (i, &b) in BUTTONS.iter().enumerate() {
            assert_eq!(button_index(b), Some(i), "{b:?}");
        }
        for (i, &(a, _)) in AXES.iter().enumerate() {
            assert_eq!(axis_index(a).map(|(j, _)| j), Some(i), "{a:?}");
        }
        for b in [Button::C, Button::Z, Button::Unknown] {
            assert_eq!(button_index(b), None);
        }
        for a in [Axis::LeftZ, Axis::RightZ, Axis::DPadX, Axis::Unknown] {
            assert_eq!(axis_index(a), None);
        }
    }

    #[test]
    fn a_synthetic_event_stream_drives_the_mapper() {
        // Bind the R2 *button* to the horn so both trigger paths are visible.
        let mut b = Bindings::default();
        b.capture(
            Action::Horn,
            Binding::new(RawInput::PadButton(PadButton::R2), Sign::Positive),
        )
        .unwrap();
        let mut m = InputMapper::new(b);
        let mut feed = |events: &[PadEvent]| {
            for &e in events {
                apply(&mut m, e);
            }
            m.sample(0.01).clone()
        };

        // Half a trigger: the axis drives throttle, the button is not pressed yet.
        let s = feed(&[PadEvent::Button(Button::RightTrigger2, 0.5, false)]);
        assert!((s.axis(Action::Throttle) - 0.5).abs() < 1e-6);
        assert!(!s.pressed(Action::Horn));

        // Past gilrs's threshold: both.
        let s = feed(&[
            PadEvent::Button(Button::RightTrigger2, 0.9, true),
            PadEvent::Button(Button::South, 1.0, true),
            // gilrs +Y is up: pushing the left stick forward is +1 here, -1 (up) for us.
            PadEvent::Axis(Axis::LeftStickY, 1.0),
            PadEvent::Axis(Axis::RightStickX, -1.0),
            PadEvent::Axis(Axis::LeftZ, 1.0),
            PadEvent::Button(Button::C, 1.0, true),
        ]);
        assert!((s.axis(Action::Throttle) - 0.9).abs() < 1e-6);
        assert!(s.pressed(Action::Horn) && s.just_pressed(Action::Horn));
        assert!(s.pressed(Action::Handbrake));
        assert!(s.pressed(Action::MenuUp) && !s.pressed(Action::MenuDown));
        assert_eq!(s.axis(Action::FlyForward), 1.0);
        assert_eq!(s.axis(Action::LookX), -1.0);

        // Stick Y down (gilrs -1) is +LookY, screen space.
        let s = feed(&[PadEvent::Axis(Axis::RightStickY, -0.8)]);
        assert!(s.axis(Action::LookY) > 0.5);

        // Left trigger brakes; letting go of everything clears it all.
        let s = feed(&[PadEvent::Button(Button::LeftTrigger2, 1.0, true)]);
        assert_eq!(s.axis(Action::Brake), 1.0);
        let s = feed(&[
            PadEvent::Button(Button::RightTrigger2, 0.0, false),
            PadEvent::Button(Button::LeftTrigger2, 0.0, false),
            PadEvent::Button(Button::South, 0.0, false),
            PadEvent::Axis(Axis::LeftStickY, 0.0),
            PadEvent::Axis(Axis::RightStickX, 0.0),
            PadEvent::Axis(Axis::RightStickY, 0.0),
        ]);
        for a in Action::ALL {
            assert_eq!(s.axis(a), 0.0, "{a:?}");
        }

        // D-pad and the guide button reach their bindings by position.
        let s = feed(&[
            PadEvent::Button(Button::DPadRight, 1.0, true),
            PadEvent::Button(Button::Start, 1.0, true),
        ]);
        assert!(s.pressed(Action::RadioNext));
        // Start pauses on release: held, it is the controls card.
        let s = feed(&[PadEvent::Button(Button::Start, 0.0, false)]);
        assert!(s.just_pressed(Action::Pause));
    }

    #[test]
    fn an_unmapped_linux_pad_reads_like_its_sdl_mapping() {
        let none = Quirks::default();
        let ev = PadEvent::Axis(Axis::LeftZ, 1.0);
        assert_eq!(none.fix(ev), ev);
        assert_eq!(
            none.fix(PadEvent::Button(Button::North, 1.0, true)),
            PadEvent::Button(Button::North, 1.0, true)
        );

        let q = Quirks {
            swap_xy: true,
            z_triggers: true,
        };
        // evdev's BTN_X (gilrs North) is the left face button; Y is the top one.
        assert_eq!(
            q.fix(PadEvent::Button(Button::North, 1.0, true)),
            PadEvent::Button(Button::West, 1.0, true)
        );
        assert_eq!(
            q.fix(PadEvent::Button(Button::West, 0.0, false)),
            PadEvent::Button(Button::North, 0.0, false)
        );
        // ABS_Z/ABS_RZ run -1 (released) to 1 (pulled) and become the trigger buttons.
        assert_eq!(
            q.fix(PadEvent::Axis(Axis::LeftZ, -1.0)),
            PadEvent::Button(Button::LeftTrigger2, 0.0, false)
        );
        assert_eq!(
            q.fix(PadEvent::Axis(Axis::RightZ, 1.0)),
            PadEvent::Button(Button::RightTrigger2, 1.0, true)
        );
        assert_eq!(
            q.fix(PadEvent::Axis(Axis::RightZ, 0.0)),
            PadEvent::Button(Button::RightTrigger2, 0.5, false)
        );
        // Sticks pass through.
        let ev = PadEvent::Axis(Axis::LeftStickX, 0.3);
        assert_eq!(q.fix(ev), ev);

        // ...so the right trigger drives the throttle.
        let mut m = InputMapper::default();
        apply(&mut m, q.fix(PadEvent::Axis(Axis::RightZ, 1.0)));
        assert_eq!(m.sample(0.01).axis(Action::Throttle), 1.0);
    }

    #[test]
    fn a_disconnect_releases_only_the_pad() {
        let mut m = InputMapper::default();
        m.press(RawInput::Key(app::Key::KeyB));
        apply(&mut m, PadEvent::Button(Button::RightTrigger2, 1.0, true));
        apply(&mut m, PadEvent::Axis(Axis::LeftStickX, 1.0));
        assert!(m.sample(0.01).axis(Action::Throttle) > 0.9);
        pad_table::release_gamepad(&mut m);
        let s = m.sample(0.01);
        assert_eq!(s.axis(Action::Throttle), 0.0);
        assert_eq!(s.axis(Action::Steer), 0.0);
        assert!(s.pressed(Action::Horn));
    }
}
