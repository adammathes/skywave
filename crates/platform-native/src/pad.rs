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
//! - `LeftZ`/`RightZ` (generic axes of unmapped devices) and `C`/`Z` are ignored;
//!   `DPadX`/`DPadY` hat axes become d-pad buttons in gilrs's default filters.
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

/// How gilrs names a pad's buttons.
fn mapping(pad: &gilrs::Gamepad) -> &'static str {
    match pad.mapping_source() {
        MappingSource::SdlMappings => "SDL mapping",
        MappingSource::Driver => "driver mapping",
        MappingSource::None => "no mapping",
    }
}

/// The gamepad backend and which pad is active.
pub struct Pads {
    gilrs: Option<Gilrs>,
    active: Option<GamepadId>,
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
        let active = gilrs
            .as_ref()
            .and_then(|g| g.gamepads().next().map(|(id, _)| id));
        Pads {
            gilrs,
            active,
            verbose,
            #[cfg(target_os = "macos")]
            mac: crate::pad_mac::GameControllers::new(verbose),
        }
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
                        self.active = Some(id);
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
                        self.active = g.gamepads().map(|(other, _)| other).find(|&x| x != id);
                        if let Some(next) = self.active {
                            self.note(format_args!("gamepad {next} is now active"));
                            if focused {
                                self.resync(input);
                            }
                        }
                    }
                }
                _ if !focused || overridden || Some(id) != self.active => {}
                EventType::ButtonPressed(b, _)
                | EventType::ButtonReleased(b, _)
                | EventType::ButtonChanged(b, _, _) => {
                    // The state gilrs just updated carries both the value and its own
                    // pressed threshold, whichever of the two events this was.
                    let (value, pressed) = g
                        .gamepad(id)
                        .button_data(b)
                        .map_or((0.0, false), |d| (d.value(), d.is_pressed()));
                    apply(input, PadEvent::Button(b, value, pressed));
                }
                EventType::AxisChanged(a, value, _) => apply(input, PadEvent::Axis(a, value)),
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
            let (value, pressed) = pad
                .button_data(b)
                .map_or((0.0, false), |d| (d.value(), d.is_pressed()));
            apply(input, PadEvent::Button(b, value, pressed));
        }
        for (a, _) in AXES {
            let value = pad.axis_data(a).map_or(0.0, |d| d.value());
            apply(input, PadEvent::Axis(a, value));
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
