//! Which raw inputs drive which actions, and the per-device analog options.

use serde::{Deserialize, Serialize};

use super::raw::{DeviceKind, Key, MouseAxis, PadAxis, PadButton, RawInput, MOUSE_RIGHT};
use super::Action;

/// How a binding reads its raw input.
///
/// For keys and buttons (held = 1): `Positive` and `Full` push the action toward +1,
/// `Negative` toward -1, so A (`Negative`) and D (`Positive`) form the Steer axis.
///
/// For analog inputs: `Full` reads the whole range as is; `Positive` reads only the
/// positive half, `Negative` only the negative half (keeping its sign). Actions whose
/// range is `[0, 1]` (Throttle, Brake, every button) use the magnitude, so "left stick up"
/// is `PadAxis(LeftY)` + `Negative` on MenuUp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sign {
    Positive,
    Negative,
    #[default]
    Full,
}

/// One raw input bound to an action, optionally only while another input is held (a chord,
/// like "hold the right mouse button to look"). While a chord's `hold` is down, unchorded
/// bindings of the same `raw` on that device are ignored, so Select + right stick scrubs
/// time instead of also looking around.
///
/// A `long` binding (a button only) fires when its input is let go after [`LONG_PRESS_S`] or
/// more held with no chord used; a shorter press fires the input's other actions instead,
/// also on release (as a chord's hold's do). So a tap of pad Select opens the map, holding it
/// resets the car, and holding it to scrub time does neither. Any action bound to that
/// button acts on release, not press (shown as "(hold)" on the Bindings page).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub raw: RawInput,
    #[serde(default)]
    pub sign: Sign,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<RawInput>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub long: bool,
}

/// How long a `long` binding's input must be held, s.
pub const LONG_PRESS_S: f32 = 1.0;

fn is_false(b: &bool) -> bool {
    !*b
}

impl Binding {
    pub fn new(raw: RawInput, sign: Sign) -> Binding {
        Binding {
            raw,
            sign,
            hold: None,
            long: false,
        }
    }

    /// This binding, firing only on release after [`LONG_PRESS_S`] held.
    pub fn held_long(self) -> Binding {
        Binding { long: true, ..self }
    }

    /// This binding, active only while `hold` is held.
    pub fn while_held(self, hold: RawInput) -> Binding {
        Binding {
            hold: Some(hold),
            ..self
        }
    }

    /// True when both bindings read the same physical input: same raw input, chord and
    /// press length, and for analog inputs, overlapping halves (`Positive` and `Negative`
    /// of one stick axis are different inputs).
    pub fn overlaps(&self, other: &Binding) -> bool {
        if self.raw != other.raw || self.hold != other.hold || self.long != other.long {
            return false;
        }
        !self.raw.is_analog()
            || !matches!(
                (self.sign, other.sign),
                (Sign::Positive, Sign::Negative) | (Sign::Negative, Sign::Positive)
            )
    }

    /// The signed contribution of this binding for a raw value (held keys are 1.0).
    pub fn read(&self, value: f32) -> f32 {
        match (self.raw.is_analog(), self.sign) {
            (false, Sign::Negative) => -value,
            (false, _) | (true, Sign::Full) => value,
            (true, Sign::Positive) => value.max(0.0),
            (true, Sign::Negative) => value.min(0.0),
        }
    }
}

/// Per-device options for one analog action, applied in this order:
/// deadzone → curve → sensitivity → invert.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AxisOptions {
    /// Flip the sign. Y invert on look is the canonical case.
    pub invert: bool,
    /// Gain, 0.25–4.
    pub sensitivity: f32,
    /// Magnitudes up to this read as zero; the rest is rescaled to start from zero. 0–0.3.
    pub deadzone: f32,
    /// Response exponent, 1.0 (linear) to 2.5.
    pub curve: f32,
}

impl Default for AxisOptions {
    fn default() -> AxisOptions {
        AxisOptions::IDENTITY
    }
}

impl AxisOptions {
    pub const IDENTITY: AxisOptions = AxisOptions {
        invert: false,
        sensitivity: 1.0,
        deadzone: 0.0,
        curve: 1.0,
    };
    pub const SENSITIVITY_MIN: f32 = 0.25;
    pub const SENSITIVITY_MAX: f32 = 4.0;
    pub const DEADZONE_MAX: f32 = 0.3;
    pub const CURVE_MIN: f32 = 1.0;
    pub const CURVE_MAX: f32 = 2.5;

    /// Clamps every option into its documented range; non-finite values become the identity.
    pub fn sanitized(self) -> AxisOptions {
        let fix =
            |v: f32, lo: f32, hi: f32, id: f32| if v.is_finite() { v.clamp(lo, hi) } else { id };
        AxisOptions {
            invert: self.invert,
            sensitivity: fix(
                self.sensitivity,
                Self::SENSITIVITY_MIN,
                Self::SENSITIVITY_MAX,
                1.0,
            ),
            deadzone: fix(self.deadzone, 0.0, Self::DEADZONE_MAX, 0.0),
            curve: fix(self.curve, Self::CURVE_MIN, Self::CURVE_MAX, 1.0),
        }
    }

    /// Deadzone → curve → sensitivity → invert. Does not clamp.
    pub fn apply(self, x: f32) -> f32 {
        let mag = x.abs();
        let mag = if mag <= self.deadzone {
            0.0
        } else {
            (mag - self.deadzone) / (1.0 - self.deadzone)
        };
        let mag = if self.curve == 1.0 {
            mag
        } else {
            mag.powf(self.curve)
        };
        let v = (mag * self.sensitivity).copysign(x);
        if self.invert {
            -v
        } else {
            v
        }
    }
}

/// A remap that would bind one input to two actions in the same context on one device.
/// Nothing was changed; pass it to [`Bindings::resolve_conflict`] with the user's choice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conflict {
    pub device: DeviceKind,
    pub binding: Binding,
    /// The action the input is bound to now.
    pub existing: Action,
    /// The action the capture asked for.
    pub wanted: Action,
}

#[derive(Clone, Debug, PartialEq)]
struct DeviceBindings {
    actions: [Vec<Binding>; Action::COUNT],
    axes: [AxisOptions; Action::ANALOG.len()],
}

impl DeviceBindings {
    fn defaults(device: DeviceKind) -> DeviceBindings {
        DeviceBindings {
            actions: core::array::from_fn(|i| default_bindings(device, Action::ALL[i])),
            axes: core::array::from_fn(|i| default_axis(device, Action::ANALOG[i])),
        }
    }
}

/// The user's input settings: for each device kind, the bindings of every action and the
/// [`AxisOptions`] of every analog action. Stored as JSON (see `to_json`/`from_json`).
#[derive(Clone, Debug, PartialEq)]
pub struct Bindings {
    devices: [DeviceBindings; DeviceKind::COUNT],
}

impl Default for Bindings {
    fn default() -> Bindings {
        Bindings {
            devices: DeviceKind::ALL.map(DeviceBindings::defaults),
        }
    }
}

impl Bindings {
    pub fn get(&self, device: DeviceKind, action: Action) -> &[Binding] {
        &self.devices[device as usize].actions[action as usize]
    }

    /// Replaces an action's bindings on one device. Bindings for another device's raw
    /// inputs are dropped. Does not check for conflicts; remaps go through [`Self::capture`].
    pub fn set(&mut self, device: DeviceKind, action: Action, mut list: Vec<Binding>) {
        list.retain(|b| b.raw.device() == device);
        self.devices[device as usize].actions[action as usize] = list;
    }

    /// True when any action reads `raw` (directly or as a chord's hold).
    pub fn is_bound(&self, raw: RawInput) -> bool {
        let device = &self.devices[raw.device() as usize];
        device
            .actions
            .iter()
            .flatten()
            .any(|b| b.raw == raw || b.hold == Some(raw))
    }

    /// The options of an analog action (identity for buttons).
    pub fn axis_options(&self, device: DeviceKind, action: Action) -> AxisOptions {
        match action.analog_slot() {
            Some(i) => self.devices[device as usize].axes[i],
            None => AxisOptions::IDENTITY,
        }
    }

    /// Sets an analog action's options, clamped into range. Ignored for buttons.
    pub fn set_axis_options(&mut self, device: DeviceKind, action: Action, options: AxisOptions) {
        if let Some(i) = action.analog_slot() {
            self.devices[device as usize].axes[i] = options.sanitized();
        }
    }

    /// Restores one action's bindings and options on one device.
    pub fn reset_action(&mut self, device: DeviceKind, action: Action) {
        self.set(device, action, default_bindings(device, action));
        self.set_axis_options(device, action, default_axis(device, action));
    }

    /// Restores every binding and option of one device.
    pub fn reset_device(&mut self, device: DeviceKind) {
        self.devices[device as usize] = DeviceBindings::defaults(device);
    }

    /// The first other action in `action`'s context, on the binding's device, that already
    /// reads the same input.
    fn find_conflict(&self, action: Action, binding: &Binding) -> Option<Action> {
        let device = binding.raw.device();
        Action::ALL.iter().copied().find(|&other| {
            other != action
                && other.context() == action.context()
                && self.get(device, other).iter().any(|b| b.overlaps(binding))
        })
    }

    /// Binds `binding` to `action` on the device its raw input belongs to (the remap UI's
    /// "press the new input"). An overlapping binding already on `action` is replaced. If
    /// another action in the same context reads that input, nothing changes and the
    /// [`Conflict`] is returned for the user to resolve.
    pub fn capture(&mut self, action: Action, binding: Binding) -> Result<(), Conflict> {
        let device = binding.raw.device();
        if let Some(existing) = self.find_conflict(action, &binding) {
            return Err(Conflict {
                device,
                binding,
                existing,
                wanted: action,
            });
        }
        let list = &mut self.devices[device as usize].actions[action as usize];
        list.retain(|b| !b.overlaps(&binding));
        list.push(binding);
        Ok(())
    }

    /// Settles a [`Conflict`]: `keep == conflict.wanted` moves the input to the new action
    /// (unbinding it from every other action in that context); `keep == conflict.existing`
    /// (or any other action) leaves the bindings unchanged. Returns whether anything changed.
    pub fn resolve_conflict(&mut self, conflict: &Conflict, keep: Action) -> bool {
        if keep != conflict.wanted {
            return false;
        }
        let context = conflict.wanted.context();
        for action in Action::ALL {
            if action != conflict.wanted && action.context() == context {
                self.devices[conflict.device as usize].actions[action as usize]
                    .retain(|b| !b.overlaps(&conflict.binding));
            }
        }
        self.capture(conflict.wanted, conflict.binding).is_ok()
    }

    /// Preset: throttle and brake move from the triggers to the right stick (up = throttle,
    /// down = brake). Right-stick-Y look is unbound on the pad, since the stick is taken;
    /// [`Self::reset_device`] undoes the preset.
    pub fn swap_triggers_sticks(&mut self) {
        let pad = DeviceKind::Gamepad;
        let stick = RawInput::PadAxis(PadAxis::RightY);
        for (action, sign) in [
            (Action::Throttle, Sign::Negative),
            (Action::Brake, Sign::Positive),
        ] {
            let mut list: Vec<Binding> = self
                .get(pad, action)
                .iter()
                .filter(|b| !matches!(b.raw, RawInput::PadAxis(PadAxis::L2 | PadAxis::R2)))
                .copied()
                .collect();
            list.push(Binding::new(stick, sign));
            self.set(pad, action, list);
        }
        for action in Action::ALL {
            if action.context() == Action::Throttle.context()
                && !matches!(action, Action::Throttle | Action::Brake)
            {
                self.devices[pad as usize].actions[action as usize]
                    .retain(|b| b.raw != stick || b.hold.is_some());
            }
        }
    }

    /// Preset: swap the left and right sticks (and their clicks) for every action, so steer
    /// is on the right stick and look on the left. Applying it twice restores the original.
    pub fn southpaw(&mut self) {
        fn swap(raw: RawInput) -> RawInput {
            match raw {
                RawInput::PadAxis(a) => RawInput::PadAxis(match a {
                    PadAxis::LeftX => PadAxis::RightX,
                    PadAxis::LeftY => PadAxis::RightY,
                    PadAxis::RightX => PadAxis::LeftX,
                    PadAxis::RightY => PadAxis::LeftY,
                    other => other,
                }),
                RawInput::PadButton(PadButton::L3) => RawInput::PadButton(PadButton::R3),
                RawInput::PadButton(PadButton::R3) => RawInput::PadButton(PadButton::L3),
                other => other,
            }
        }
        for list in &mut self.devices[DeviceKind::Gamepad as usize].actions {
            for b in list {
                b.raw = swap(b.raw);
                b.hold = b.hold.map(swap);
            }
        }
    }
}

/// The default bindings, per `docs/CONTROLS_INPUT.md`.
pub fn default_bindings(device: DeviceKind, action: Action) -> Vec<Binding> {
    use Sign::{Full, Negative as Neg, Positive as Pos};
    let key = |k: Key, s: Sign| Binding::new(RawInput::Key(k), s);
    let btn = |b: PadButton| Binding::new(RawInput::PadButton(b), Pos);
    let axis = |a: PadAxis, s: Sign| Binding::new(RawInput::PadAxis(a), s);
    let mouse_look = |a: MouseAxis| {
        Binding::new(RawInput::MouseAxis(a), Full).while_held(RawInput::MouseButton(MOUSE_RIGHT))
    };
    match device {
        DeviceKind::KeyboardMouse => match action {
            Action::Throttle => vec![key(Key::KeyW, Pos), key(Key::ArrowUp, Pos)],
            Action::Brake => vec![key(Key::KeyS, Pos), key(Key::ArrowDown, Pos)],
            Action::Steer => vec![
                key(Key::KeyA, Neg),
                key(Key::KeyD, Pos),
                key(Key::ArrowLeft, Neg),
                key(Key::ArrowRight, Pos),
            ],
            Action::Handbrake => vec![key(Key::Space, Pos)],
            Action::CameraCycle => vec![key(Key::KeyC, Pos)],
            Action::LookX => vec![mouse_look(MouseAxis::X)],
            Action::LookY => vec![mouse_look(MouseAxis::Y)],
            Action::LookBack => vec![key(Key::KeyQ, Pos)],
            Action::RadioNext => vec![key(Key::BracketRight, Pos)],
            Action::RadioPrev => vec![key(Key::BracketLeft, Pos)],
            Action::RadioOff => vec![key(Key::Backslash, Pos)],
            Action::Headlights => vec![key(Key::KeyL, Pos)],
            Action::Horn => vec![key(Key::KeyH, Pos)],
            Action::VehicleNext => vec![key(Key::KeyV, Pos)],
            Action::Reset => vec![key(Key::KeyR, Pos)],
            Action::MapToggle => vec![key(Key::KeyM, Pos)],
            Action::MapZoom => vec![key(Key::Minus, Neg), key(Key::Equal, Pos)],
            Action::TimeScrub => vec![key(Key::Comma, Neg), key(Key::Period, Pos)],
            Action::Pause => vec![key(Key::Escape, Pos)],
            Action::Screenshot => vec![key(Key::F12, Pos)],
            Action::DebugOverlay => vec![key(Key::F3, Pos)],
            Action::MenuUp => vec![key(Key::ArrowUp, Pos)],
            Action::MenuDown => vec![key(Key::ArrowDown, Pos)],
            Action::MenuLeft => vec![key(Key::ArrowLeft, Pos)],
            Action::MenuRight => vec![key(Key::ArrowRight, Pos)],
            Action::MenuConfirm => vec![key(Key::Enter, Pos), key(Key::Space, Pos)],
            Action::MenuBack => vec![key(Key::Escape, Pos), key(Key::Backspace, Pos)],
            Action::FlyForward => vec![key(Key::KeyW, Pos)],
            Action::FlyBack => vec![key(Key::KeyS, Pos)],
            Action::FlyLeft => vec![key(Key::KeyA, Pos)],
            Action::FlyRight => vec![key(Key::KeyD, Pos)],
            Action::FlyUp => vec![key(Key::KeyE, Pos), key(Key::Space, Pos)],
            Action::FlyDown => vec![key(Key::KeyQ, Pos), key(Key::ShiftLeft, Pos)],
            Action::FlyLookX => vec![key(Key::ArrowLeft, Neg), key(Key::ArrowRight, Pos)],
            Action::FlyLookY => vec![key(Key::ArrowUp, Neg), key(Key::ArrowDown, Pos)],
        },
        DeviceKind::Gamepad => match action {
            Action::Throttle => vec![axis(PadAxis::R2, Full)],
            Action::Brake => vec![axis(PadAxis::L2, Full)],
            Action::Steer => vec![axis(PadAxis::LeftX, Full)],
            Action::Handbrake => vec![btn(PadButton::South)],
            Action::CameraCycle => vec![btn(PadButton::North)],
            Action::LookX => vec![axis(PadAxis::RightX, Full)],
            Action::LookY => vec![axis(PadAxis::RightY, Full)],
            Action::LookBack => vec![btn(PadButton::R3)],
            Action::RadioNext => vec![btn(PadButton::DpadRight)],
            Action::RadioPrev => vec![btn(PadButton::DpadLeft)],
            Action::RadioOff => vec![btn(PadButton::DpadDown)],
            Action::Headlights => vec![btn(PadButton::DpadUp)],
            Action::Horn => vec![btn(PadButton::L3)],
            // The pad picks a car on the pause menu's Vehicle page.
            Action::VehicleNext => vec![],
            // Held, so a tap (the map) never resets by accident.
            Action::Reset => vec![btn(PadButton::Select).held_long()],
            Action::MapToggle => vec![btn(PadButton::Select)],
            Action::MapZoom => vec![
                Binding::new(RawInput::PadButton(PadButton::L1), Neg),
                btn(PadButton::R1),
            ],
            Action::TimeScrub => {
                vec![axis(PadAxis::RightX, Full).while_held(RawInput::PadButton(PadButton::Select))]
            }
            Action::Pause => vec![btn(PadButton::Start)],
            Action::Screenshot | Action::DebugOverlay => vec![],
            Action::MenuUp => vec![btn(PadButton::DpadUp), axis(PadAxis::LeftY, Neg)],
            Action::MenuDown => vec![btn(PadButton::DpadDown), axis(PadAxis::LeftY, Pos)],
            Action::MenuLeft => vec![btn(PadButton::DpadLeft), axis(PadAxis::LeftX, Neg)],
            Action::MenuRight => vec![btn(PadButton::DpadRight), axis(PadAxis::LeftX, Pos)],
            Action::MenuConfirm => vec![btn(PadButton::South)],
            Action::MenuBack => vec![btn(PadButton::East)],
            Action::FlyForward => vec![axis(PadAxis::LeftY, Neg)],
            Action::FlyBack => vec![axis(PadAxis::LeftY, Pos)],
            Action::FlyLeft => vec![axis(PadAxis::LeftX, Neg)],
            Action::FlyRight => vec![axis(PadAxis::LeftX, Pos)],
            Action::FlyUp => vec![btn(PadButton::R1)],
            Action::FlyDown => vec![btn(PadButton::L1)],
            Action::FlyLookX => vec![axis(PadAxis::RightX, Full)],
            Action::FlyLookY => vec![axis(PadAxis::RightY, Full)],
        },
    }
}

/// The default options of an analog action. Keyboard/mouse is linear with no deadzone; the
/// gamepad sticks get the doc's steering deadzone 0.08 (steer also its 1.5 curve), so a
/// resting stick does not drift; triggers are linear.
pub fn default_axis(device: DeviceKind, action: Action) -> AxisOptions {
    match (device, action) {
        (DeviceKind::Gamepad, Action::Steer) => AxisOptions {
            deadzone: 0.08,
            curve: 1.5,
            ..AxisOptions::IDENTITY
        },
        (
            DeviceKind::Gamepad,
            Action::LookX | Action::LookY | Action::TimeScrub | Action::FlyLookX | Action::FlyLookY,
        ) => AxisOptions {
            deadzone: 0.08,
            ..AxisOptions::IDENTITY
        },
        _ => AxisOptions::IDENTITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key, s: Sign) -> Binding {
        Binding::new(RawInput::Key(k), s)
    }

    #[test]
    fn defaults_follow_the_doc_table() {
        let b = Bindings::default();
        let kb = DeviceKind::KeyboardMouse;
        let pad = DeviceKind::Gamepad;
        let raws = |d, a| b.get(d, a).iter().map(|b| b.raw).collect::<Vec<_>>();
        use RawInput::{Key as K, PadAxis as A, PadButton as B};
        let table: [(Action, Vec<RawInput>, Vec<RawInput>); 20] = [
            (
                Action::Throttle,
                vec![K(Key::KeyW), K(Key::ArrowUp)],
                vec![A(PadAxis::R2)],
            ),
            (
                Action::Brake,
                vec![K(Key::KeyS), K(Key::ArrowDown)],
                vec![A(PadAxis::L2)],
            ),
            (
                Action::Steer,
                vec![
                    K(Key::KeyA),
                    K(Key::KeyD),
                    K(Key::ArrowLeft),
                    K(Key::ArrowRight),
                ],
                vec![A(PadAxis::LeftX)],
            ),
            (
                Action::Handbrake,
                vec![K(Key::Space)],
                vec![B(PadButton::South)],
            ),
            (
                Action::CameraCycle,
                vec![K(Key::KeyC)],
                vec![B(PadButton::North)],
            ),
            (
                Action::LookX,
                vec![RawInput::MouseAxis(MouseAxis::X)],
                vec![A(PadAxis::RightX)],
            ),
            (
                Action::LookY,
                vec![RawInput::MouseAxis(MouseAxis::Y)],
                vec![A(PadAxis::RightY)],
            ),
            (Action::LookBack, vec![K(Key::KeyQ)], vec![B(PadButton::R3)]),
            (
                Action::RadioNext,
                vec![K(Key::BracketRight)],
                vec![B(PadButton::DpadRight)],
            ),
            (
                Action::RadioPrev,
                vec![K(Key::BracketLeft)],
                vec![B(PadButton::DpadLeft)],
            ),
            (
                Action::RadioOff,
                vec![K(Key::Backslash)],
                vec![B(PadButton::DpadDown)],
            ),
            (
                Action::Headlights,
                vec![K(Key::KeyL)],
                vec![B(PadButton::DpadUp)],
            ),
            (Action::Horn, vec![K(Key::KeyH)], vec![B(PadButton::L3)]),
            (Action::VehicleNext, vec![K(Key::KeyV)], vec![]),
            (
                Action::Reset,
                vec![K(Key::KeyR)],
                vec![B(PadButton::Select)],
            ),
            (
                Action::MapToggle,
                vec![K(Key::KeyM)],
                vec![B(PadButton::Select)],
            ),
            (
                Action::TimeScrub,
                vec![K(Key::Comma), K(Key::Period)],
                vec![A(PadAxis::RightX)],
            ),
            (
                Action::Pause,
                vec![K(Key::Escape)],
                vec![B(PadButton::Start)],
            ),
            (Action::Screenshot, vec![K(Key::F12)], vec![]),
            (Action::DebugOverlay, vec![K(Key::F3)], vec![]),
        ];
        for (action, keys, pads) in table {
            assert_eq!(raws(kb, action), keys, "{action:?} keyboard");
            assert_eq!(raws(pad, action), pads, "{action:?} gamepad");
        }
        // Signs that make axes out of key pairs, and the chords.
        assert_eq!(b.get(kb, Action::Steer)[0].sign, Sign::Negative);
        assert_eq!(b.get(kb, Action::Steer)[1].sign, Sign::Positive);
        assert_eq!(b.get(kb, Action::TimeScrub)[0].sign, Sign::Negative);
        assert_eq!(
            b.get(kb, Action::LookY)[0].hold,
            Some(RawInput::MouseButton(MOUSE_RIGHT))
        );
        assert_eq!(
            b.get(pad, Action::TimeScrub)[0].hold,
            Some(RawInput::PadButton(PadButton::Select))
        );
        assert_eq!(b.get(pad, Action::LookX)[0].hold, None);
        let steer = b.axis_options(pad, Action::Steer);
        assert_eq!((steer.deadzone, steer.curve), (0.08, 1.5));
        assert_eq!(b.axis_options(kb, Action::Steer), AxisOptions::IDENTITY);
        assert!(!b.axis_options(pad, Action::LookY).invert);
        // Every binding sits on its own device.
        for d in DeviceKind::ALL {
            for a in Action::ALL {
                assert!(b.get(d, a).iter().all(|x| x.raw.device() == d), "{a:?}");
            }
        }
    }

    #[test]
    fn defaults_have_no_conflicts() {
        let b = Bindings::default();
        for d in DeviceKind::ALL {
            for a in Action::ALL {
                for binding in b.get(d, a) {
                    assert_eq!(b.find_conflict(a, binding), None, "{d:?} {a:?} {binding:?}");
                }
            }
        }
    }

    #[test]
    fn axis_options_apply_in_order_and_sanitize() {
        // deadzone → curve → sensitivity → invert:
        // (0.54 - 0.08) / 0.92 = 0.5;  0.5^1.5 = 0.35355;  × 1.5 = 0.53033;  inverted.
        let o = AxisOptions {
            invert: true,
            sensitivity: 1.5,
            deadzone: 0.08,
            curve: 1.5,
        };
        let cases: [(f32, f32); 6] = [
            (0.54, -0.530_330_1),
            (-0.54, 0.530_330_1),
            (0.08, 0.0),
            (-0.05, 0.0),
            (0.0, 0.0),
            (1.0, -1.5),
        ];
        for (x, want) in cases {
            assert!((o.apply(x) - want).abs() < 1e-5, "{x} -> {}", o.apply(x));
        }
        assert_eq!(AxisOptions::IDENTITY.apply(0.3), 0.3);

        let wild = AxisOptions {
            invert: true,
            sensitivity: 9.0,
            deadzone: -1.0,
            curve: f32::NAN,
        }
        .sanitized();
        assert_eq!(
            wild,
            AxisOptions {
                invert: true,
                sensitivity: 4.0,
                deadzone: 0.0,
                curve: 1.0
            }
        );
        let mut b = Bindings::default();
        b.set_axis_options(DeviceKind::Gamepad, Action::Horn, wild);
        assert_eq!(b, Bindings::default());
    }

    #[test]
    fn binding_reads_halves_and_directions() {
        let pad = |s| Binding::new(RawInput::PadAxis(PadAxis::LeftY), s);
        let cases: [(Binding, f32, f32); 8] = [
            (key(Key::KeyA, Sign::Negative), 1.0, -1.0),
            (key(Key::KeyD, Sign::Positive), 1.0, 1.0),
            (key(Key::KeyD, Sign::Full), 1.0, 1.0),
            (pad(Sign::Full), -0.6, -0.6),
            (pad(Sign::Positive), -0.6, 0.0),
            (pad(Sign::Positive), 0.6, 0.6),
            (pad(Sign::Negative), -0.6, -0.6),
            (pad(Sign::Negative), 0.6, 0.0),
        ];
        for (b, raw, want) in cases {
            assert_eq!(b.read(raw), want, "{b:?} {raw}");
        }
        assert!(pad(Sign::Full).overlaps(&pad(Sign::Negative)));
        assert!(!pad(Sign::Positive).overlaps(&pad(Sign::Negative)));
        assert!(key(Key::KeyA, Sign::Positive).overlaps(&key(Key::KeyA, Sign::Negative)));
        let chord = pad(Sign::Full).while_held(RawInput::PadButton(PadButton::Select));
        assert!(!chord.overlaps(&pad(Sign::Full)));
    }

    #[test]
    fn capture_reports_conflicts_and_resolution_moves_the_input() {
        let kb = DeviceKind::KeyboardMouse;
        let mut b = Bindings::default();

        // A free key binds straight away; capturing it again replaces, not duplicates.
        let j = key(Key::KeyJ, Sign::Positive);
        assert_eq!(b.capture(Action::Horn, j), Ok(()));
        assert_eq!(b.capture(Action::Horn, j), Ok(()));
        assert_eq!(
            b.get(kb, Action::Horn),
            &[key(Key::KeyH, Sign::Positive), j]
        );

        // H is the horn: binding it to the headlights conflicts and changes nothing.
        let h = key(Key::KeyH, Sign::Positive);
        let before = b.clone();
        let conflict = b.capture(Action::Headlights, h).unwrap_err();
        assert_eq!(
            conflict,
            Conflict {
                device: kb,
                binding: h,
                existing: Action::Horn,
                wanted: Action::Headlights,
            }
        );
        assert_eq!(b, before);

        // Keeping the existing action is a no-op.
        assert!(!b.resolve_conflict(&conflict, Action::Horn));
        assert_eq!(b, before);
        // Keeping the new one moves H.
        assert!(b.resolve_conflict(&conflict, Action::Headlights));
        assert_eq!(b.get(kb, Action::Horn), &[j]);
        assert_eq!(
            b.get(kb, Action::Headlights),
            &[key(Key::KeyL, Sign::Positive), h]
        );

        // Other contexts never conflict: W is Throttle and FlyForward, and can be MenuUp too.
        assert_eq!(
            b.capture(Action::MenuUp, key(Key::KeyW, Sign::Positive)),
            Ok(())
        );
        // Opposite halves of one stick axis are different inputs.
        let mut b = Bindings::default();
        b.set(DeviceKind::Gamepad, Action::MenuUp, vec![]);
        let up = Binding::new(RawInput::PadAxis(PadAxis::LeftY), Sign::Negative);
        assert_eq!(b.capture(Action::MenuUp, up), Ok(()));
        // ...but the full axis overlaps both, and conflicts with MenuUp.
        let full = Binding::new(RawInput::PadAxis(PadAxis::LeftY), Sign::Full);
        assert_eq!(
            b.capture(Action::MenuLeft, full).unwrap_err().existing,
            Action::MenuUp
        );

        assert!(b.is_bound(RawInput::Key(Key::KeyW)));
        assert!(!b.is_bound(RawInput::Key(Key::KeyK)));
        assert!(Bindings::default().is_bound(RawInput::MouseButton(MOUSE_RIGHT)));
        b.reset_device(DeviceKind::Gamepad);
        b.reset_action(kb, Action::Horn);
        b.reset_action(kb, Action::Headlights);
        b.reset_action(kb, Action::MenuUp);
        assert_eq!(b, Bindings::default());
    }

    #[test]
    fn presets_rebind_the_pad() {
        let pad = DeviceKind::Gamepad;
        let mut b = Bindings::default();
        b.southpaw();
        assert_eq!(
            b.get(pad, Action::Steer)[0].raw,
            RawInput::PadAxis(PadAxis::RightX)
        );
        assert_eq!(
            b.get(pad, Action::LookY)[0].raw,
            RawInput::PadAxis(PadAxis::LeftY)
        );
        assert_eq!(
            b.get(pad, Action::Horn)[0].raw,
            RawInput::PadButton(PadButton::R3)
        );
        assert_eq!(
            b.get(pad, Action::TimeScrub)[0].raw,
            RawInput::PadAxis(PadAxis::LeftX)
        );
        // Steering keeps its curve; options belong to the action, not the stick.
        assert_eq!(b.axis_options(pad, Action::Steer).curve, 1.5);
        b.southpaw();
        assert_eq!(b, Bindings::default());

        b.swap_triggers_sticks();
        let stick = RawInput::PadAxis(PadAxis::RightY);
        assert_eq!(
            b.get(pad, Action::Throttle),
            &[Binding::new(stick, Sign::Negative)]
        );
        assert_eq!(
            b.get(pad, Action::Brake),
            &[Binding::new(stick, Sign::Positive)]
        );
        assert!(b.get(pad, Action::LookY).is_empty());
        assert_eq!(
            b.get(pad, Action::LookX)[0].raw,
            RawInput::PadAxis(PadAxis::RightX)
        );
        assert_eq!(b.get(pad, Action::FlyLookY)[0].raw, stick);
        b.reset_device(pad);
        assert_eq!(b, Bindings::default());
    }
}
