//! Raw events in, one [`InputState`] per frame out.

use super::bindings::{Binding, Bindings, LONG_PRESS_S};
use super::raw::{DeviceKind, MouseAxis, RawInput};
use super::{Action, Context, InputState};

/// Mouse look: one unit of `LookX`/`LookY` per this many pixels of motion between samples
/// (50 px in one frame is full deflection), before sensitivity.
pub const MOUSE_LOOK_PER_PIXEL: f32 = 0.02;
/// An analog input driving a button action (left stick up → MenuUp) counts only past this,
/// so a resting stick does not creep the free-fly camera.
const ANALOG_BUTTON_DEADZONE: f32 = 0.25;
/// A stick moved this far while a chord's hold is down uses the chord, so releasing the hold
/// does not also count as a tap of its own action (Select = map unless it scrubbed time).
const CHORD_USE: f32 = 0.3;
/// A ramp this close to its target snaps onto it.
const RAMP_SNAP: f32 = 1e-4;
/// A pad axis counts as "moved" (for remap capture and [`InputMapper::last_device`]) when
/// it crosses this from nearer the centre.
const AXIS_MOVED: f32 = 0.5;

/// Turns raw platform events into a per-frame [`InputState`] through the user's
/// [`Bindings`]. Feed events as they arrive, then call [`InputMapper::sample`] once per frame.
///
/// Raw state is latest-wins: a press and release between two samples leaves the input up,
/// but still fires `just_pressed` once. Mouse motion and wheel accumulate between samples
/// and reset after each. Allocates nothing after construction.
#[derive(Clone, Debug)]
pub struct InputMapper {
    bindings: Bindings,
    /// Keys and buttons held now.
    held: [bool; RawInput::COUNT],
    /// Analog value now (pad axes), or accumulated since the last sample (mouse, wheel).
    value: [f32; RawInput::COUNT],
    /// Pressed at least once since the last sample.
    pressed_edge: [bool; RawInput::COUNT],
    /// Used as some binding's `hold`, or by a `long` binding; such an input fires its own
    /// actions on release, and only if no chord (or long press) was used while it was down.
    modifier: [bool; RawInput::COUNT],
    /// Per context ([`Context`]): some action of that context makes the input a modifier,
    /// so that context's other actions on it act on release. Another context's (the
    /// menu's, on a d-pad direction a driving action holds long) still act on press.
    deferred: [[bool; RawInput::COUNT]; 3],
    chord_used: [bool; RawInput::COUNT],
    /// Some `long` binding reads this input.
    long: [bool; RawInput::COUNT],
    /// Seconds each input has been held, counted at samples.
    held_s: [f32; RawInput::COUNT],
    /// Released since the last sample after [`LONG_PRESS_S`] held without using a chord:
    /// its `long` bindings fire (and its other actions do not).
    long_edge: [bool; RawInput::COUNT],
    /// Released since the last sample without using a chord.
    tap: [bool; RawInput::COUNT],
    /// Keyboard-ramped value of each analog action, per device.
    ramp: [[f32; Action::ANALOG.len()]; DeviceKind::COUNT],
    was_pressed: [bool; Action::COUNT],
    state: InputState,
    /// Remap capture: armed by [`InputMapper::arm_capture`], filled by the next press.
    capture_armed: bool,
    captured: Option<(RawInput, f32)>,
    last_device: DeviceKind,
}

impl Default for InputMapper {
    fn default() -> InputMapper {
        InputMapper::new(Bindings::default())
    }
}

impl InputMapper {
    pub fn new(bindings: Bindings) -> InputMapper {
        let mut m = InputMapper {
            bindings,
            held: [false; RawInput::COUNT],
            value: [0.0; RawInput::COUNT],
            pressed_edge: [false; RawInput::COUNT],
            modifier: [false; RawInput::COUNT],
            deferred: [[false; RawInput::COUNT]; 3],
            chord_used: [false; RawInput::COUNT],
            long: [false; RawInput::COUNT],
            held_s: [0.0; RawInput::COUNT],
            long_edge: [false; RawInput::COUNT],
            tap: [false; RawInput::COUNT],
            ramp: [[0.0; Action::ANALOG.len()]; DeviceKind::COUNT],
            was_pressed: [false; Action::COUNT],
            state: InputState::default(),
            capture_armed: false,
            captured: None,
            last_device: DeviceKind::KeyboardMouse,
        };
        m.index_modifiers();
        m
    }

    pub fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    /// Switches to new bindings (after a remap or a settings load). Held inputs stay held.
    pub fn set_bindings(&mut self, bindings: Bindings) {
        self.bindings = bindings;
        self.index_modifiers();
    }

    fn index_modifiers(&mut self) {
        self.modifier = [false; RawInput::COUNT];
        self.deferred = [[false; RawInput::COUNT]; 3];
        self.long = [false; RawInput::COUNT];
        for device in DeviceKind::ALL {
            for action in Action::ALL {
                let ctx = context_slot(action.context());
                for b in self.bindings.get(device, action) {
                    if let Some(i) = b.hold.and_then(RawInput::slot) {
                        self.modifier[i] = true;
                        self.deferred[ctx][i] = true;
                    }
                    if let Some(i) = b.raw.slot().filter(|_| b.long && !b.raw.is_analog()) {
                        self.modifier[i] = true;
                        self.deferred[ctx][i] = true;
                        self.long[i] = true;
                    }
                }
            }
        }
    }

    fn is_held(&self, raw: RawInput) -> bool {
        raw.slot().is_some_and(|i| self.held[i])
    }

    /// A key or button went down. Repeats while held are ignored. For analog inputs use
    /// [`InputMapper::axis`].
    pub fn press(&mut self, raw: RawInput) {
        let Some(i) = raw.slot() else { return };
        if raw.is_analog() || self.held[i] {
            return;
        }
        self.held[i] = true;
        self.held_s[i] = 0.0;
        self.pressed_edge[i] = true;
        self.chord_used[i] = false;
        self.note_chord_use(raw);
        self.note_moved(raw, 1.0);
    }

    /// A key or button went up.
    pub fn release(&mut self, raw: RawInput) {
        let Some(i) = raw.slot() else { return };
        if !self.held[i] {
            return;
        }
        self.held[i] = false;
        if self.modifier[i] && !self.chord_used[i] {
            if self.long[i] && self.held_s[i] >= LONG_PRESS_S {
                self.long_edge[i] = true;
            } else {
                self.tap[i] = true;
            }
        }
    }

    /// An analog input moved: pad axes report their absolute value (sticks `[-1, 1]`,
    /// triggers `[0, 1]`); mouse axes (pixels) and the wheel (notches) report deltas, which
    /// add up until the next sample. A key or button given here is pressed above 0.5.
    pub fn axis(&mut self, raw: RawInput, value: f32) {
        let Some(i) = raw.slot() else { return };
        if !value.is_finite() {
            return;
        }
        if !raw.is_analog() {
            if value > 0.5 {
                self.press(raw);
            } else {
                self.release(raw);
            }
            return;
        }
        if raw.is_relative() {
            self.value[i] += value;
        } else {
            if value.abs() > AXIS_MOVED && self.value[i].abs() <= AXIS_MOVED {
                self.note_moved(raw, value);
            }
            self.value[i] = value;
        }
        if value.abs() > CHORD_USE || (raw.is_relative() && value != 0.0) {
            self.note_chord_use(raw);
        }
    }

    /// Relative mouse motion in pixels, +x right, +y down.
    pub fn mouse_delta(&mut self, dx: f32, dy: f32) {
        self.axis(RawInput::MouseAxis(MouseAxis::X), dx);
        self.axis(RawInput::MouseAxis(MouseAxis::Y), dy);
    }

    /// Marks the holds of every chord on `raw` that is down now as used.
    fn note_chord_use(&mut self, raw: RawInput) {
        let device = raw.device();
        for action in Action::ALL {
            for b in self.bindings.get(device, action) {
                if let Some(h) = b.hold.filter(|_| b.raw == raw).and_then(RawInput::slot) {
                    if self.held[h] {
                        self.chord_used[h] = true;
                    }
                }
            }
        }
    }

    /// A key or button went down, or a pad axis moved past [`AXIS_MOVED`].
    fn note_moved(&mut self, raw: RawInput, value: f32) {
        self.last_device = raw.device();
        if self.capture_armed && self.captured.is_none() {
            self.captured = Some((raw, value));
        }
    }

    /// The device of the latest key or button press or pad stick/trigger movement (mouse
    /// motion does not count), for showing the right button glyphs.
    pub fn last_device(&self) -> DeviceKind {
        self.last_device
    }

    /// Starts a remap capture: the next key, mouse button or pad button pressed, or pad axis
    /// pushed past halfway, from now on is kept for [`InputMapper::take_capture`]. Inputs
    /// already held do not count until pressed again. Actions are still mapped as usual.
    pub fn arm_capture(&mut self) {
        self.capture_armed = true;
        self.captured = None;
    }

    /// The input captured since [`InputMapper::arm_capture`], with its value (1 for keys and
    /// buttons, the signed position for an axis); taking it ends the capture.
    pub fn take_capture(&mut self) -> Option<(RawInput, f32)> {
        let got = self.captured.take();
        if got.is_some() {
            self.capture_armed = false;
        }
        got
    }

    /// Ends a capture without taking anything.
    pub fn cancel_capture(&mut self) {
        self.capture_armed = false;
        self.captured = None;
    }

    pub fn is_capturing(&self) -> bool {
        self.capture_armed
    }

    /// Lets go of everything at once, e.g. when the window loses focus.
    pub fn release_all(&mut self) {
        self.held = [false; RawInput::COUNT];
        self.value = [0.0; RawInput::COUNT];
        self.pressed_edge = [false; RawInput::COUNT];
        self.tap = [false; RawInput::COUNT];
        self.long_edge = [false; RawInput::COUNT];
        self.ramp = [[0.0; Action::ANALOG.len()]; DeviceKind::COUNT];
    }

    /// The state from the last [`InputMapper::sample`].
    pub fn state(&self) -> &InputState {
        &self.state
    }

    /// Maps the current raw state into this frame's [`InputState`], advancing keyboard
    /// ramps by `dt` seconds. Call once per frame, before the simulation steps.
    pub fn sample(&mut self, dt: f32) -> &InputState {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };

        // How long each input has been held, for long presses (which act on release).
        for i in 0..RawInput::COUNT {
            if self.held[i] {
                self.held_s[i] += dt;
            }
        }

        // Inputs taken over by a chord whose hold is down, and holds whose chord is in use
        // (the stick may already be deflected when the hold goes down).
        let mut captured = [false; RawInput::COUNT];
        for device in DeviceKind::ALL {
            for action in Action::ALL {
                for b in self.bindings.get(device, action) {
                    let (Some(h), Some(i)) = (b.hold.and_then(RawInput::slot), b.raw.slot()) else {
                        continue;
                    };
                    if self.held[h] {
                        captured[i] = true;
                        if self.held[i] || self.value[i].abs() > CHORD_USE {
                            self.chord_used[h] = true;
                        }
                    }
                }
            }
        }

        for action in Action::ALL {
            let mut value = 0.0;
            let mut edge = false;
            for device in DeviceKind::ALL {
                let mut digital = 0.0f32;
                let mut analog = 0.0f32;
                for b in self.bindings.get(device, action) {
                    let ctx = context_slot(action.context());
                    let Some((v, e)) = self.read_binding(b, ctx, &captured) else {
                        continue;
                    };
                    edge |= e;
                    let v = if action.is_bipolar() { v } else { v.abs() };
                    match (b.raw.is_analog(), action.is_analog()) {
                        (false, true) => digital += v,
                        (true, true) => analog += v,
                        // Buttons take their strongest input.
                        (false, false) => value = f32::max(value, v),
                        (true, false) if v > ANALOG_BUTTON_DEADZONE => value = f32::max(value, v),
                        (true, false) => {}
                    }
                }
                if let Some(slot) = action.analog_slot() {
                    let target = action.clamp(digital);
                    let ramped = match action.ramp() {
                        Some((up, down)) => {
                            let r = &mut self.ramp[device as usize][slot];
                            *r = ramp_toward(*r, target, up, down, dt);
                            *r
                        }
                        None => target,
                    };
                    value += self
                        .bindings
                        .axis_options(device, action)
                        .apply(ramped + analog);
                }
            }
            self.state.set(action, value);
            let pressed = self.state.pressed(action);
            self.state.set_just_pressed(
                action,
                edge || (pressed && !self.was_pressed[action as usize]),
            );
            self.was_pressed[action as usize] = pressed;
        }

        if let Some(i) = RawInput::MouseWheel.slot() {
            self.state.set_wheel(self.value[i]);
        }
        self.pressed_edge = [false; RawInput::COUNT];
        self.tap = [false; RawInput::COUNT];
        self.long_edge = [false; RawInput::COUNT];
        for raw in [
            RawInput::MouseAxis(MouseAxis::X),
            RawInput::MouseAxis(MouseAxis::Y),
            RawInput::MouseWheel,
        ] {
            if let Some(i) = raw.slot() {
                self.value[i] = 0.0;
            }
        }
        &self.state
    }

    /// The signed value and press edge of one binding now, or `None` while it is inactive
    /// (its chord's hold is up, or a chord has taken its input over).
    fn read_binding(
        &self,
        b: &Binding,
        ctx: usize,
        captured: &[bool; RawInput::COUNT],
    ) -> Option<(f32, bool)> {
        let i = b.raw.slot()?;
        match b.hold {
            Some(h) if !self.is_held(h) => return None,
            None if captured[i] => return None,
            _ => {}
        }
        if b.raw.is_analog() {
            let scale = match b.raw {
                RawInput::MouseAxis(_) => MOUSE_LOOK_PER_PIXEL,
                _ => 1.0,
            };
            return Some((b.read(self.value[i] * scale), false));
        }
        let (down, edge) = if b.long {
            (self.long_edge[i], self.long_edge[i])
        } else if self.deferred[ctx][i] && b.hold.is_none() {
            (self.tap[i], self.tap[i])
        } else {
            (self.held[i], self.pressed_edge[i])
        };
        Some((b.read(if down { 1.0 } else { 0.0 }), edge))
    }
}

fn context_slot(c: Context) -> usize {
    match c {
        Context::Drive => 0,
        Context::Menu => 1,
        Context::Fly => 2,
    }
}

/// Moves a keyboard-driven value toward `target` (-1, 0 or 1): `up` seconds from rest to
/// full, `down` seconds from full back to rest. Reversing returns to rest first, then
/// ramps up from there on the next sample.
fn ramp_toward(cur: f32, target: f32, up: f32, down: f32, dt: f32) -> f32 {
    let outward = target != 0.0 && cur * target >= 0.0 && target.abs() > cur.abs();
    let step = dt / if outward { up } else { down };
    let mut next = if target > cur {
        (cur + step).min(target)
    } else {
        (cur - step).max(target)
    };
    if !outward && cur != 0.0 && next * cur < 0.0 {
        next = 0.0;
    }
    if (next - target).abs() < RAMP_SNAP {
        target
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::bindings::AxisOptions;
    use crate::input::raw::{Key, PadAxis, PadButton, MOUSE_RIGHT};

    const KB: DeviceKind = DeviceKind::KeyboardMouse;
    const PAD: DeviceKind = DeviceKind::Gamepad;

    fn key(k: Key) -> RawInput {
        RawInput::Key(k)
    }
    fn pad(a: PadAxis) -> RawInput {
        RawInput::PadAxis(a)
    }
    fn btn(b: PadButton) -> RawInput {
        RawInput::PadButton(b)
    }

    /// A mapper whose `action` options on `device` are `options`.
    fn with_options(device: DeviceKind, action: Action, options: AxisOptions) -> InputMapper {
        let mut b = Bindings::default();
        b.set_axis_options(device, action, options);
        InputMapper::new(b)
    }

    fn invert(device: DeviceKind, action: Action, on: bool) -> InputMapper {
        let mut b = Bindings::default();
        let mut o = b.axis_options(device, action);
        o.invert = on;
        b.set_axis_options(device, action, o);
        InputMapper::new(b)
    }

    #[derive(Clone, Copy, Debug)]
    enum Feed {
        Stick(PadAxis, f32),
        Mouse(f32, f32),
    }

    #[test]
    fn invert_flips_look_on_gamepad_and_mouse() {
        // (path, action, raw feed, sign of the result without invert)
        let cases: [(DeviceKind, Action, Feed, f32); 8] = [
            (PAD, Action::LookY, Feed::Stick(PadAxis::RightY, 0.8), 1.0),
            (PAD, Action::LookY, Feed::Stick(PadAxis::RightY, -0.8), -1.0),
            (PAD, Action::LookX, Feed::Stick(PadAxis::RightX, 0.8), 1.0),
            (PAD, Action::LookX, Feed::Stick(PadAxis::RightX, -0.8), -1.0),
            (KB, Action::LookY, Feed::Mouse(0.0, 12.0), 1.0),
            (KB, Action::LookY, Feed::Mouse(0.0, -12.0), -1.0),
            (KB, Action::LookX, Feed::Mouse(12.0, 0.0), 1.0),
            (KB, Action::LookX, Feed::Mouse(-12.0, 0.0), -1.0),
        ];
        for (device, action, feed, sign) in cases {
            for inverted in [false, true] {
                let mut m = invert(device, action, inverted);
                m.press(RawInput::MouseButton(MOUSE_RIGHT));
                match feed {
                    Feed::Stick(a, v) => m.axis(pad(a), v),
                    Feed::Mouse(dx, dy) => m.mouse_delta(dx, dy),
                }
                let got = m.sample(1.0 / 60.0).axis(action);
                let want = if inverted { -sign } else { sign };
                assert!(
                    got * want > 0.1,
                    "{device:?} {action:?} {feed:?} invert={inverted}: {got}"
                );
                // The other look axis is untouched.
                let other = if action == Action::LookX {
                    Action::LookY
                } else {
                    Action::LookX
                };
                assert_eq!(m.state().axis(other), 0.0);
            }
        }
    }

    #[test]
    fn stick_runs_deadzone_curve_sensitivity_invert_in_order() {
        let o = AxisOptions {
            invert: true,
            sensitivity: 1.5,
            deadzone: 0.08,
            curve: 1.5,
        };
        let mut m = with_options(PAD, Action::Steer, o);
        // (raw left stick X, Steer): (0.54-0.08)/0.92 = 0.5 → 0.5^1.5 = 0.35355 → ×1.5 → -0.53033.
        let cases: [(f32, f32); 5] = [
            (0.54, -0.530_330_1),
            (-0.54, 0.530_330_1),
            (0.05, 0.0),
            (1.0, -1.0), // -1.5 clamped
            (0.0, 0.0),
        ];
        for (raw, want) in cases {
            m.axis(pad(PadAxis::LeftX), raw);
            let got = m.sample(0.01).axis(Action::Steer);
            assert!((got - want).abs() < 1e-5, "{raw}: {got}");
        }
        // Default pad steering: deadzone 0.08, curve 1.5.
        let mut m = InputMapper::default();
        m.axis(pad(PadAxis::LeftX), 0.54);
        let got = m.sample(0.01).axis(Action::Steer);
        assert!((got - 0.353_553_4).abs() < 1e-5, "{got}");
    }

    #[test]
    fn mouse_look_needs_the_right_button_and_resets_each_sample() {
        let mut m = InputMapper::default();
        m.mouse_delta(10.0, 10.0);
        assert_eq!(m.sample(0.01).axis(Action::LookX), 0.0);
        m.press(RawInput::MouseButton(MOUSE_RIGHT));
        m.mouse_delta(10.0, 0.0);
        m.mouse_delta(5.0, -2.5);
        let s = m.sample(0.01);
        assert!((s.axis(Action::LookX) - 15.0 * MOUSE_LOOK_PER_PIXEL).abs() < 1e-6);
        assert!((s.axis(Action::LookY) + 2.5 * MOUSE_LOOK_PER_PIXEL).abs() < 1e-6);
        // No motion since the last sample: no look.
        assert_eq!(m.sample(0.01).axis(Action::LookX), 0.0);
        // A huge flick clamps.
        m.mouse_delta(5000.0, 0.0);
        assert_eq!(m.sample(0.01).axis(Action::LookX), 1.0);
    }

    /// A scripted step: events, then a sample of `dt` seconds, then expected values.
    struct Step {
        press: &'static [Key],
        release: &'static [Key],
        dt: f32,
        expect: &'static [(Action, f32)],
    }

    fn run(steps: &[Step]) {
        let mut m = InputMapper::default();
        for (n, s) in steps.iter().enumerate() {
            for &k in s.press {
                m.press(key(k));
            }
            for &k in s.release {
                m.release(key(k));
            }
            let state = m.sample(s.dt);
            for &(action, want) in s.expect {
                let got = state.axis(action);
                assert!(
                    (got - want).abs() < 1e-4,
                    "step {n}: {action:?} = {got}, want {want}"
                );
            }
        }
    }

    #[test]
    fn keyboard_steering_ramps_up_in_120_ms_and_back_in_300_ms() {
        let hold = |dt, expect| Step {
            press: &[],
            release: &[],
            dt,
            expect,
        };
        run(&[
            Step {
                press: &[Key::KeyD],
                release: &[],
                dt: 0.040,
                expect: &[(Action::Steer, 1.0 / 3.0)],
            },
            hold(0.040, &[(Action::Steer, 2.0 / 3.0)]),
            hold(0.040, &[(Action::Steer, 1.0)]),
            hold(0.100, &[(Action::Steer, 1.0)]),
            Step {
                press: &[],
                release: &[Key::KeyD],
                dt: 0.150,
                expect: &[(Action::Steer, 0.5)],
            },
            hold(0.150, &[(Action::Steer, 0.0)]),
            hold(0.100, &[(Action::Steer, 0.0)]),
        ]);
        // At 120 Hz, 15 samples (125 ms, the first past 120 ms) land exactly on 1.0; 36 more
        // (300 ms) exactly on 0.
        let mut m = InputMapper::default();
        m.press(key(Key::KeyD));
        for i in 0..15 {
            let v = m.sample(1.0 / 120.0).axis(Action::Steer);
            assert_eq!(v == 1.0, i == 14, "sample {i}: {v}");
        }
        m.release(key(Key::KeyD));
        for i in 0..36 {
            let v = m.sample(1.0 / 120.0).axis(Action::Steer);
            assert_eq!(v == 0.0, i == 35, "sample {i}: {v}");
        }
    }

    #[test]
    fn opposing_keys_cancel_and_reversal_returns_through_zero() {
        run(&[
            Step {
                press: &[Key::KeyA, Key::KeyD],
                release: &[],
                dt: 0.2,
                expect: &[(Action::Steer, 0.0)],
            },
            Step {
                press: &[],
                release: &[Key::KeyD],
                dt: 0.2,
                expect: &[(Action::Steer, -1.0)],
            },
            Step {
                // A to D: return at the 300 ms rate, stopping at zero...
                press: &[Key::KeyD],
                release: &[Key::KeyA],
                dt: 0.15,
                expect: &[(Action::Steer, -0.5)],
            },
            Step {
                press: &[],
                release: &[],
                dt: 0.3,
                expect: &[(Action::Steer, 0.0)],
            },
            Step {
                // ...then out at the 120 ms rate.
                press: &[],
                release: &[],
                dt: 0.06,
                expect: &[(Action::Steer, 0.5)],
            },
        ]);
    }

    #[test]
    fn throttle_and_brake_ramp_in_100_ms_and_buttons_are_instant() {
        run(&[
            Step {
                press: &[Key::KeyW, Key::ArrowDown, Key::Space, Key::KeyE],
                release: &[],
                dt: 0.05,
                expect: &[
                    (Action::Throttle, 0.5),
                    (Action::Brake, 0.5),
                    (Action::Handbrake, 1.0),
                    (Action::FlyUp, 1.0),
                    (Action::FlyForward, 1.0),
                    (Action::MenuDown, 1.0),
                ],
            },
            Step {
                // Two keys on one action do not add past full.
                press: &[Key::ArrowUp],
                release: &[Key::Space],
                dt: 0.05,
                expect: &[
                    (Action::Throttle, 1.0),
                    (Action::Brake, 1.0),
                    (Action::Handbrake, 0.0),
                    (Action::FlyUp, 1.0),
                    (Action::FlyLookY, 0.0),
                ],
            },
            Step {
                press: &[],
                release: &[Key::KeyW, Key::ArrowUp, Key::ArrowDown],
                dt: 0.1,
                expect: &[(Action::Throttle, 0.0), (Action::Brake, 0.0)],
            },
        ]);
    }

    #[test]
    fn free_fly_keys_match_the_old_table() {
        // (key, action, value): the same keys as before the bindings model.
        let table: [(Key, Action, f32); 12] = [
            (Key::KeyW, Action::FlyForward, 1.0),
            (Key::KeyS, Action::FlyBack, 1.0),
            (Key::KeyA, Action::FlyLeft, 1.0),
            (Key::KeyD, Action::FlyRight, 1.0),
            (Key::KeyE, Action::FlyUp, 1.0),
            (Key::Space, Action::FlyUp, 1.0),
            (Key::KeyQ, Action::FlyDown, 1.0),
            (Key::ShiftLeft, Action::FlyDown, 1.0),
            (Key::ArrowLeft, Action::FlyLookX, -1.0),
            (Key::ArrowRight, Action::FlyLookX, 1.0),
            (Key::ArrowUp, Action::FlyLookY, -1.0),
            (Key::ArrowDown, Action::FlyLookY, 1.0),
        ];
        for (k, action, want) in table {
            let mut m = InputMapper::default();
            m.press(key(k));
            let s = m.sample(0.0);
            assert_eq!(s.axis(action), want, "{k:?}");
            let others = Action::ALL
                .iter()
                .filter(|a| a.context() == action.context() && **a != action);
            for &other in others {
                assert_eq!(s.axis(other), 0.0, "{k:?} also drives {other:?}");
            }
        }
    }

    #[test]
    fn just_pressed_fires_once_per_press() {
        let mut m = InputMapper::default();
        let horn = |m: &mut InputMapper| {
            let s = m.sample(0.01);
            (s.pressed(Action::Horn), s.just_pressed(Action::Horn))
        };
        assert_eq!(horn(&mut m), (false, false));
        m.press(key(Key::KeyB));
        assert_eq!(horn(&mut m), (true, true));
        // Held, and OS key repeat while held: no new edge.
        m.press(key(Key::KeyB));
        assert_eq!(horn(&mut m), (true, false));
        assert_eq!(horn(&mut m), (true, false));
        m.release(key(Key::KeyB));
        assert_eq!(horn(&mut m), (false, false));
        // A tap entirely between two samples still fires exactly once.
        m.press(key(Key::KeyB));
        m.release(key(Key::KeyB));
        assert_eq!(horn(&mut m), (false, true));
        assert_eq!(horn(&mut m), (false, false));
        // Release and press again between samples: a second edge.
        m.press(key(Key::KeyB));
        assert_eq!(horn(&mut m), (true, true));
        m.release(key(Key::KeyB));
        m.press(key(Key::KeyB));
        assert_eq!(horn(&mut m), (true, true));

        // A stick crossing into a menu direction is an edge too, once.
        let mut m = InputMapper::default();
        let up = |m: &mut InputMapper, y| {
            m.axis(pad(PadAxis::LeftY), y);
            m.sample(0.01).just_pressed(Action::MenuUp)
        };
        assert!(!up(&mut m, -0.4));
        assert!(up(&mut m, -0.9));
        assert!(!up(&mut m, -1.0));
        assert!(!up(&mut m, 0.0));
        assert!(up(&mut m, -0.7));
    }

    #[test]
    fn select_chord_scrubs_time_and_a_tap_toggles_the_map() {
        let mut m = InputMapper::default();
        // Right stick alone looks.
        m.axis(pad(PadAxis::RightX), 0.6);
        let s = m.sample(0.01);
        assert!(s.axis(Action::LookX) > 0.5 && s.axis(Action::TimeScrub) == 0.0);
        // Hold Select: the stick scrubs time instead, and the map does not open.
        m.press(btn(PadButton::Select));
        let s = m.sample(0.01);
        assert!(
            s.axis(Action::TimeScrub) > 0.5,
            "{}",
            s.axis(Action::TimeScrub)
        );
        assert_eq!(s.axis(Action::LookX), 0.0);
        assert!(!s.pressed(Action::MapToggle) && !s.just_pressed(Action::MapToggle));
        m.release(btn(PadButton::Select));
        let s = m.sample(0.01);
        assert!(!s.just_pressed(Action::MapToggle));
        assert!(s.axis(Action::LookX) > 0.5 && s.axis(Action::TimeScrub) == 0.0);
        // Tap Select with the stick centred: map toggle, once, on release.
        m.axis(pad(PadAxis::RightX), 0.0);
        m.press(btn(PadButton::Select));
        assert!(!m.sample(0.01).just_pressed(Action::MapToggle));
        m.release(btn(PadButton::Select));
        assert!(m.sample(0.01).just_pressed(Action::MapToggle));
        assert!(!m.sample(0.01).just_pressed(Action::MapToggle));
    }

    #[test]
    fn holding_select_resets_on_release_and_a_tap_or_a_scrub_does_not() {
        let mut m = InputMapper::default();
        let select = btn(PadButton::Select);
        let reset = |m: &mut InputMapper, dt| {
            let s = m.sample(dt);
            (
                s.just_pressed(Action::Reset),
                s.just_pressed(Action::MapToggle),
            )
        };
        let hold = |m: &mut InputMapper, frames: usize| {
            for _ in 0..frames {
                assert_eq!(reset(m, 1.0 / 60.0), (false, false));
            }
        };
        // A tap, or anything short of a second: the map, no reset.
        m.press(select);
        hold(&mut m, 55);
        m.release(select);
        assert_eq!(reset(&mut m, 0.01), (false, true));
        // A second and more: nothing while held, the reset once on release, no map.
        m.press(select);
        hold(&mut m, 62);
        m.release(select);
        assert_eq!(reset(&mut m, 0.01), (true, false));
        assert_eq!(reset(&mut m, 0.01), (false, false));
        // Scrubbing time with Select held is not a long press, however late the stick
        // moves.
        m.press(select);
        hold(&mut m, 72);
        m.axis(pad(PadAxis::RightX), 0.8);
        m.sample(1.0 / 60.0);
        m.axis(pad(PadAxis::RightX), 0.0);
        m.release(select);
        assert_eq!(reset(&mut m, 0.01), (false, false));
        // The keyboard's R resets at once.
        m.press(key(Key::KeyR));
        assert!(m.sample(0.01).just_pressed(Action::Reset));
    }

    #[test]
    fn wheel_notches_and_map_zoom() {
        let mut m = InputMapper::default();
        // Notches accumulate until the next sample, then reset.
        m.axis(RawInput::MouseWheel, 1.0);
        m.axis(RawInput::MouseWheel, 2.0);
        assert_eq!(m.sample(0.01).wheel(), 3.0);
        assert_eq!(m.sample(0.01).wheel(), 0.0);
        // The shoulders zoom the full map; = and - on the keyboard.
        m.press(btn(PadButton::R1));
        assert_eq!(m.sample(0.01).axis(Action::MapZoom), 1.0);
        m.release(btn(PadButton::R1));
        m.press(btn(PadButton::L1));
        assert_eq!(m.sample(0.01).axis(Action::MapZoom), -1.0);
        m.release(btn(PadButton::L1));
        m.press(key(Key::Minus));
        assert_eq!(m.sample(0.01).axis(Action::MapZoom), -1.0);
        m.release(key(Key::Minus));
        m.press(key(Key::Equal));
        assert_eq!(m.sample(0.01).axis(Action::MapZoom), 1.0);
    }

    #[test]
    fn triggers_and_keys_combine_and_release_all_clears() {
        let mut m = InputMapper::default();
        m.axis(pad(PadAxis::R2), 0.4);
        m.axis(btn(PadButton::South), 1.0);
        let s = m.sample(0.01);
        assert!((s.axis(Action::Throttle) - 0.4).abs() < 1e-6);
        assert!(s.pressed(Action::Handbrake) && s.pressed(Action::MenuConfirm));
        m.press(key(Key::KeyW));
        assert!((m.sample(0.05).axis(Action::Throttle) - 0.9).abs() < 1e-5);
        m.release_all();
        assert_eq!(m.sample(0.01), &InputState::default());
    }

    #[test]
    fn remapped_bindings_take_effect() {
        let mut b = Bindings::default();
        let conflict = b
            .capture(
                Action::Horn,
                Binding::new(key(Key::Space), crate::input::bindings::Sign::Positive),
            )
            .unwrap_err();
        assert!(b.resolve_conflict(&conflict, Action::Horn));
        let mut m = InputMapper::default();
        m.set_bindings(b);
        m.press(key(Key::Space));
        let s = m.sample(0.01);
        assert!(s.pressed(Action::Horn) && !s.pressed(Action::Handbrake));
        // Space still flies up: a different context.
        assert!(s.pressed(Action::FlyUp));
    }

    #[test]
    fn capture_takes_the_next_fresh_press_or_axis_push() {
        let mut m = InputMapper::default();
        // Held before arming: not captured until pressed again.
        m.press(btn(PadButton::South));
        m.axis(pad(PadAxis::LeftX), 0.9);
        m.arm_capture();
        assert!(m.is_capturing());
        m.axis(pad(PadAxis::LeftX), 0.95);
        m.press(btn(PadButton::South));
        assert_eq!(m.take_capture(), None);
        // A small stick wobble is ignored; a push past halfway is taken, with its sign.
        m.axis(pad(PadAxis::RightY), -0.3);
        m.axis(pad(PadAxis::RightY), -0.8);
        m.axis(pad(PadAxis::RightX), 0.9);
        assert_eq!(m.take_capture(), Some((pad(PadAxis::RightY), -0.8)));
        assert!(!m.is_capturing());
        assert_eq!(m.take_capture(), None);
        // Keys and mouse buttons, and the last device follows them.
        assert_eq!(m.last_device(), PAD);
        m.arm_capture();
        m.press(key(Key::KeyJ));
        m.press(key(Key::KeyK));
        assert_eq!(m.take_capture(), Some((key(Key::KeyJ), 1.0)));
        assert_eq!(m.last_device(), KB);
        m.arm_capture();
        m.cancel_capture();
        m.press(RawInput::MouseButton(0));
        assert_eq!(m.take_capture(), None);
        // Mapping goes on while armed.
        m.arm_capture();
        m.press(key(Key::KeyB));
        assert!(m.sample(0.01).pressed(Action::Horn));
    }
}
