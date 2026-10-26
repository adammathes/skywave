//! The W3C "standard" gamepad mapping as one table, shared by every platform: the browser
//! reports buttons and axes by index in this layout, and native backends (gilrs) name the
//! same physical controls, so both shells turn their readings into [`RawInput`]s here.
//!
//! Standard mapping: buttons 0–16 are [`BUTTONS`] in order (6 and 7 are the analog
//! triggers), axes 0–3 are [`AXES`] (left X, left Y, right X, right Y; +X right, +Y down).

use super::mapper::InputMapper;
use super::raw::{PadAxis, PadButton, RawInput};

/// Standard-mapping button `i` is `BUTTONS[i]`. This is also every [`PadButton`], in
/// discriminant order.
pub const BUTTONS: [PadButton; PadButton::COUNT] = [
    PadButton::South,
    PadButton::East,
    PadButton::West,
    PadButton::North,
    PadButton::L1,
    PadButton::R1,
    PadButton::L2,
    PadButton::R2,
    PadButton::Select,
    PadButton::Start,
    PadButton::L3,
    PadButton::R3,
    PadButton::DpadUp,
    PadButton::DpadDown,
    PadButton::DpadLeft,
    PadButton::DpadRight,
    PadButton::Guide,
];

/// Standard-mapping axis `i` is `AXES[i]`. The triggers are buttons in the standard
/// mapping, not axes; see [`Source`].
pub const AXES: [PadAxis; 4] = [
    PadAxis::LeftX,
    PadAxis::LeftY,
    PadAxis::RightX,
    PadAxis::RightY,
];

/// Every [`PadAxis`], in discriminant order.
pub const ALL_AXES: [PadAxis; PadAxis::COUNT] = [
    PadAxis::LeftX,
    PadAxis::LeftY,
    PadAxis::RightX,
    PadAxis::RightY,
    PadAxis::L2,
    PadAxis::R2,
];

/// Where a [`PadAxis`] comes from in the standard mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `axes[i]`, `[-1, 1]`.
    Axis(usize),
    /// `buttons[i].value`, `[0, 1]` (the analog triggers).
    Button(usize),
}

/// The button at standard-mapping index `index`, if there is one.
pub fn button(index: usize) -> Option<PadButton> {
    BUTTONS.get(index).copied()
}

/// The standard-mapping index of `b`.
pub fn button_index(b: PadButton) -> usize {
    b as usize
}

/// The stick axis at standard-mapping axis index `index`, if there is one.
pub fn axis(index: usize) -> Option<PadAxis> {
    AXES.get(index).copied()
}

/// Where `a` is read from in the standard mapping.
pub fn axis_source(a: PadAxis) -> Source {
    match a {
        PadAxis::LeftX => Source::Axis(0),
        PadAxis::LeftY => Source::Axis(1),
        PadAxis::RightX => Source::Axis(2),
        PadAxis::RightY => Source::Axis(3),
        PadAxis::L2 => Source::Button(6),
        PadAxis::R2 => Source::Button(7),
    }
}

/// The analog axis a button also drives: L2 and R2 are both a button and a `[0, 1]` axis.
pub fn trigger_axis(b: PadButton) -> Option<PadAxis> {
    match b {
        PadButton::L2 => Some(PadAxis::L2),
        PadButton::R2 => Some(PadAxis::R2),
        _ => None,
    }
}

/// The raw inputs one button reading means, for [`InputMapper::axis`]: the button itself
/// (1 held, 0 up) and, for a trigger, its analog value as well. `pressed` is the
/// platform's own threshold, so the button and the axis may disagree near the middle.
pub fn button_inputs(b: PadButton, value: f32, pressed: bool) -> [Option<(RawInput, f32)>; 2] {
    let held = if pressed { 1.0 } else { 0.0 };
    let analog = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    [
        Some((RawInput::PadButton(b), held)),
        trigger_axis(b).map(|a| (RawInput::PadAxis(a), analog)),
    ]
}

/// The raw input one stick reading means: `value` in the standard convention (+Y down),
/// clamped to `[-1, 1]`; non-finite values read as centred.
pub fn axis_input(a: PadAxis, value: f32) -> (RawInput, f32) {
    let v = if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (RawInput::PadAxis(a), v)
}

/// Feeds one reading of standard-mapping button `index` (unknown indices are ignored).
pub fn feed_button(input: &mut InputMapper, index: usize, value: f32, pressed: bool) {
    if let Some(b) = button(index) {
        for (raw, v) in button_inputs(b, value, pressed).into_iter().flatten() {
            input.axis(raw, v);
        }
    }
}

/// Feeds one reading of standard-mapping axis `index` (unknown indices are ignored).
pub fn feed_axis(input: &mut InputMapper, index: usize, value: f32) {
    if let Some(a) = axis(index) {
        let (raw, v) = axis_input(a, value);
        input.axis(raw, v);
    }
}

/// Releases every pad button and centres every pad axis, leaving keyboard and mouse
/// alone: for when the active gamepad goes away or another one takes over.
pub fn release_gamepad(input: &mut InputMapper) {
    for a in ALL_AXES {
        input.axis(RawInput::PadAxis(a), 0.0);
    }
    for b in BUTTONS {
        input.release(RawInput::PadButton(b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::raw::Key;
    use crate::input::Action;

    #[test]
    fn every_button_has_its_standard_index_and_back() {
        for (i, &b) in BUTTONS.iter().enumerate() {
            assert_eq!(b as usize, i, "{b:?}");
            assert_eq!(button(i), Some(b));
            assert_eq!(button_index(b), i);
        }
        assert_eq!(button(PadButton::COUNT), None);
        // The indices the W3C spec names, spelled out.
        let spec = [
            (0, PadButton::South),
            (3, PadButton::North),
            (6, PadButton::L2),
            (7, PadButton::R2),
            (8, PadButton::Select),
            (9, PadButton::Start),
            (12, PadButton::DpadUp),
            (15, PadButton::DpadRight),
            (16, PadButton::Guide),
        ];
        for (i, b) in spec {
            assert_eq!(button(i), Some(b));
        }
    }

    #[test]
    fn every_axis_has_a_standard_source_and_back() {
        for (i, &a) in ALL_AXES.iter().enumerate() {
            assert_eq!(a as usize, i, "{a:?}");
            match axis_source(a) {
                Source::Axis(j) => assert_eq!(axis(j), Some(a)),
                Source::Button(j) => {
                    let b = button(j).expect("trigger button");
                    assert_eq!(trigger_axis(b), Some(a));
                }
            }
        }
        for (i, &a) in AXES.iter().enumerate() {
            assert_eq!(axis_source(a), Source::Axis(i));
        }
        assert_eq!(axis(4), None);
        assert_eq!(axis_source(PadAxis::L2), Source::Button(6));
        assert_eq!(axis_source(PadAxis::R2), Source::Button(7));
    }

    #[test]
    fn readings_become_raw_inputs() {
        // (index, value, pressed) → raw inputs.
        let b = |i| button(i).unwrap();
        type Case = ((usize, f32, bool), [Option<(RawInput, f32)>; 2]);
        let cases: [Case; 5] = [
            (
                (0, 1.0, true),
                [Some((RawInput::PadButton(PadButton::South), 1.0)), None],
            ),
            (
                (0, 0.0, false),
                [Some((RawInput::PadButton(PadButton::South), 0.0)), None],
            ),
            (
                (7, 0.4, false),
                [
                    Some((RawInput::PadButton(PadButton::R2), 0.0)),
                    Some((RawInput::PadAxis(PadAxis::R2), 0.4)),
                ],
            ),
            (
                (6, 1.5, true),
                [
                    Some((RawInput::PadButton(PadButton::L2), 1.0)),
                    Some((RawInput::PadAxis(PadAxis::L2), 1.0)),
                ],
            ),
            (
                (6, f32::NAN, false),
                [
                    Some((RawInput::PadButton(PadButton::L2), 0.0)),
                    Some((RawInput::PadAxis(PadAxis::L2), 0.0)),
                ],
            ),
        ];
        for ((i, v, p), want) in cases {
            assert_eq!(button_inputs(b(i), v, p), want, "{i} {v} {p}");
        }
        assert_eq!(
            axis_input(PadAxis::LeftY, -0.5),
            (RawInput::PadAxis(PadAxis::LeftY), -0.5)
        );
        assert_eq!(
            axis_input(PadAxis::RightX, 2.0),
            (RawInput::PadAxis(PadAxis::RightX), 1.0)
        );
        assert_eq!(
            axis_input(PadAxis::RightX, f32::INFINITY),
            (RawInput::PadAxis(PadAxis::RightX), 0.0)
        );
    }

    #[test]
    fn feeding_a_standard_pad_drives_actions_and_release_clears_it() {
        let mut m = InputMapper::default();
        m.press(RawInput::Key(Key::KeyH));
        feed_button(&mut m, 7, 0.6, false); // right trigger, below the press threshold
        feed_button(&mut m, 0, 1.0, true); // A / Cross
        feed_axis(&mut m, 0, 0.54); // left stick right
        feed_axis(&mut m, 3, -1.0); // right stick up
        feed_button(&mut m, 99, 1.0, true); // ignored
        feed_axis(&mut m, 9, 1.0); // ignored
        let s = m.sample(0.01);
        assert!((s.axis(Action::Throttle) - 0.6).abs() < 1e-6);
        assert!(s.pressed(Action::Handbrake));
        assert!(s.axis(Action::Steer) > 0.3);
        assert_eq!(s.axis(Action::LookY), -1.0);

        release_gamepad(&mut m);
        let s = m.sample(0.01);
        for a in [
            Action::Throttle,
            Action::Handbrake,
            Action::Steer,
            Action::LookY,
        ] {
            assert_eq!(s.axis(a), 0.0, "{a:?}");
        }
        // The keyboard is untouched.
        assert!(s.pressed(Action::Horn));
    }
}
