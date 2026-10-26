//! Raw, unmapped inputs as the platform reports them: physical keys, mouse, and gamepad
//! buttons and axes by the W3C "standard" gamepad mapping.

use serde::{Deserialize, Serialize};

/// A physical key, named after the W3C UI Events `KeyboardEvent.code` value (which winit's
/// `KeyCode` also follows), so each platform maps its keys 1:1. The variant name *is* the
/// code string: [`Key::code`], [`Key::from_code`] and the JSON form all agree.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Escape,
    Enter,
    Space,
    Tab,
    Backspace,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    BracketLeft,
    BracketRight,
    Backslash,
    Comma,
    Period,
    Slash,
    Semicolon,
    Quote,
    Minus,
    Equal,
    Backquote,
}

/// `(code, key)` for every key, in discriminant order. The single table both platforms use:
/// the web passes `KeyboardEvent.code` straight in, native formats winit's `KeyCode`.
const KEY_CODES: [(&str, Key); Key::COUNT] = [
    ("KeyA", Key::KeyA),
    ("KeyB", Key::KeyB),
    ("KeyC", Key::KeyC),
    ("KeyD", Key::KeyD),
    ("KeyE", Key::KeyE),
    ("KeyF", Key::KeyF),
    ("KeyG", Key::KeyG),
    ("KeyH", Key::KeyH),
    ("KeyI", Key::KeyI),
    ("KeyJ", Key::KeyJ),
    ("KeyK", Key::KeyK),
    ("KeyL", Key::KeyL),
    ("KeyM", Key::KeyM),
    ("KeyN", Key::KeyN),
    ("KeyO", Key::KeyO),
    ("KeyP", Key::KeyP),
    ("KeyQ", Key::KeyQ),
    ("KeyR", Key::KeyR),
    ("KeyS", Key::KeyS),
    ("KeyT", Key::KeyT),
    ("KeyU", Key::KeyU),
    ("KeyV", Key::KeyV),
    ("KeyW", Key::KeyW),
    ("KeyX", Key::KeyX),
    ("KeyY", Key::KeyY),
    ("KeyZ", Key::KeyZ),
    ("Digit0", Key::Digit0),
    ("Digit1", Key::Digit1),
    ("Digit2", Key::Digit2),
    ("Digit3", Key::Digit3),
    ("Digit4", Key::Digit4),
    ("Digit5", Key::Digit5),
    ("Digit6", Key::Digit6),
    ("Digit7", Key::Digit7),
    ("Digit8", Key::Digit8),
    ("Digit9", Key::Digit9),
    ("F1", Key::F1),
    ("F2", Key::F2),
    ("F3", Key::F3),
    ("F4", Key::F4),
    ("F5", Key::F5),
    ("F6", Key::F6),
    ("F7", Key::F7),
    ("F8", Key::F8),
    ("F9", Key::F9),
    ("F10", Key::F10),
    ("F11", Key::F11),
    ("F12", Key::F12),
    ("Escape", Key::Escape),
    ("Enter", Key::Enter),
    ("Space", Key::Space),
    ("Tab", Key::Tab),
    ("Backspace", Key::Backspace),
    ("ShiftLeft", Key::ShiftLeft),
    ("ShiftRight", Key::ShiftRight),
    ("ControlLeft", Key::ControlLeft),
    ("ControlRight", Key::ControlRight),
    ("AltLeft", Key::AltLeft),
    ("AltRight", Key::AltRight),
    ("ArrowUp", Key::ArrowUp),
    ("ArrowDown", Key::ArrowDown),
    ("ArrowLeft", Key::ArrowLeft),
    ("ArrowRight", Key::ArrowRight),
    ("BracketLeft", Key::BracketLeft),
    ("BracketRight", Key::BracketRight),
    ("Backslash", Key::Backslash),
    ("Comma", Key::Comma),
    ("Period", Key::Period),
    ("Slash", Key::Slash),
    ("Semicolon", Key::Semicolon),
    ("Quote", Key::Quote),
    ("Minus", Key::Minus),
    ("Equal", Key::Equal),
    ("Backquote", Key::Backquote),
];

impl Key {
    pub const COUNT: usize = 74;

    /// Every key, in discriminant order.
    pub fn all() -> impl Iterator<Item = Key> {
        KEY_CODES.iter().map(|&(_, k)| k)
    }

    /// The DOM `KeyboardEvent.code` string for this key.
    pub fn code(self) -> &'static str {
        KEY_CODES[self as usize].0
    }

    /// Looks up a DOM `KeyboardEvent.code` string (or a winit `KeyCode` debug name).
    pub fn from_code(code: &str) -> Option<Key> {
        KEY_CODES.iter().find(|(c, _)| *c == code).map(|&(_, k)| k)
    }
}

/// Mouse motion axes. Screen space: +X is right, +Y is down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseAxis {
    X,
    Y,
}

/// Gamepad buttons, named by position in the W3C standard mapping (button index in
/// parentheses): South (0, A/Cross), East (1), West (2), North (3, Y/Triangle), L1 (4),
/// R1 (5), L2 (6), R2 (7), Select (8, Back/View/Share), Start (9, Options), L3 (10),
/// R3 (11), DpadUp (12), DpadDown (13), DpadLeft (14), DpadRight (15), Guide (16).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadButton {
    South,
    East,
    West,
    North,
    L1,
    R1,
    L2,
    R2,
    Select,
    Start,
    L3,
    R3,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    Guide,
}

impl PadButton {
    pub const COUNT: usize = 17;
}

/// Gamepad axes. Sticks are `[-1, 1]` with +X right and +Y **down** (the standard mapping:
/// pushing a stick away from you is negative). Triggers are `[0, 1]`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
    L2,
    R2,
}

impl PadAxis {
    pub const COUNT: usize = 6;
}

/// Which family of hardware a raw input comes from. Bindings and axis options are stored
/// separately for each.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    KeyboardMouse,
    Gamepad,
}

impl DeviceKind {
    pub const COUNT: usize = 2;
    pub const ALL: [DeviceKind; DeviceKind::COUNT] =
        [DeviceKind::KeyboardMouse, DeviceKind::Gamepad];
}

/// Mouse buttons tracked, numbered as DOM `MouseEvent.button`: 0 left, 1 middle, 2 right,
/// 3 back, 4 forward. Higher buttons are ignored.
pub const MOUSE_BUTTONS: u8 = 8;
/// The right mouse button, which the default bindings hold for mouse look.
pub const MOUSE_RIGHT: u8 = 2;

/// One raw input, before any binding is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RawInput {
    Key(Key),
    MouseButton(u8),
    /// Relative motion; values are pixels since the last sample.
    MouseAxis(MouseAxis),
    /// Relative scroll; values are notches since the last sample, + away from the user.
    MouseWheel,
    PadButton(PadButton),
    PadAxis(PadAxis),
}

const MOUSE_BUTTON_BASE: usize = Key::COUNT;
const MOUSE_AXIS_BASE: usize = MOUSE_BUTTON_BASE + MOUSE_BUTTONS as usize;
const MOUSE_WHEEL_SLOT: usize = MOUSE_AXIS_BASE + 2;
const PAD_BUTTON_BASE: usize = MOUSE_WHEEL_SLOT + 1;
const PAD_AXIS_BASE: usize = PAD_BUTTON_BASE + PadButton::COUNT;

impl RawInput {
    /// Number of distinct raw inputs; [`RawInput::slot`] is below this.
    pub const COUNT: usize = PAD_AXIS_BASE + PadAxis::COUNT;

    /// A dense index for per-input state arrays, or `None` for an untracked mouse button.
    pub fn slot(self) -> Option<usize> {
        Some(match self {
            RawInput::Key(k) => k as usize,
            RawInput::MouseButton(b) if b < MOUSE_BUTTONS => MOUSE_BUTTON_BASE + b as usize,
            RawInput::MouseButton(_) => return None,
            RawInput::MouseAxis(MouseAxis::X) => MOUSE_AXIS_BASE,
            RawInput::MouseAxis(MouseAxis::Y) => MOUSE_AXIS_BASE + 1,
            RawInput::MouseWheel => MOUSE_WHEEL_SLOT,
            RawInput::PadButton(b) => PAD_BUTTON_BASE + b as usize,
            RawInput::PadAxis(a) => PAD_AXIS_BASE + a as usize,
        })
    }

    pub fn device(self) -> DeviceKind {
        match self {
            RawInput::Key(_)
            | RawInput::MouseButton(_)
            | RawInput::MouseAxis(_)
            | RawInput::MouseWheel => DeviceKind::KeyboardMouse,
            RawInput::PadButton(_) | RawInput::PadAxis(_) => DeviceKind::Gamepad,
        }
    }

    /// True for inputs with a continuous value; false for keys and buttons (held or not).
    pub fn is_analog(self) -> bool {
        matches!(
            self,
            RawInput::MouseAxis(_) | RawInput::MouseWheel | RawInput::PadAxis(_)
        )
    }

    /// True for relative inputs that accumulate between samples and then reset.
    pub fn is_relative(self) -> bool {
        matches!(self, RawInput::MouseAxis(_) | RawInput::MouseWheel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_table_matches_discriminants_and_names() {
        assert_eq!(Key::all().count(), Key::COUNT);
        for (i, k) in Key::all().enumerate() {
            assert_eq!(k as usize, i);
            assert_eq!(Key::from_code(k.code()), Some(k));
            // The Debug name, the DOM code and the JSON form are the same string.
            assert_eq!(format!("{k:?}"), k.code());
            assert_eq!(
                serde_json::to_string(&k).unwrap(),
                format!("\"{}\"", k.code())
            );
        }
        for code in ["", "KeyAA", "keyw", "Numpad1", "MetaLeft"] {
            assert_eq!(Key::from_code(code), None, "{code}");
        }
    }

    #[test]
    fn raw_slots_are_dense_and_unique() {
        let mut all: Vec<RawInput> = Key::all().map(RawInput::Key).collect();
        all.extend((0..MOUSE_BUTTONS).map(RawInput::MouseButton));
        all.extend([MouseAxis::X, MouseAxis::Y].map(RawInput::MouseAxis));
        all.push(RawInput::MouseWheel);
        all.extend(
            [
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
            ]
            .map(RawInput::PadButton),
        );
        all.extend(
            [
                PadAxis::LeftX,
                PadAxis::LeftY,
                PadAxis::RightX,
                PadAxis::RightY,
                PadAxis::L2,
                PadAxis::R2,
            ]
            .map(RawInput::PadAxis),
        );
        assert_eq!(all.len(), RawInput::COUNT);
        for (i, raw) in all.iter().enumerate() {
            assert_eq!(raw.slot(), Some(i), "{raw:?}");
        }
        assert_eq!(RawInput::MouseButton(MOUSE_BUTTONS).slot(), None);
        assert_eq!(RawInput::MouseWheel.device(), DeviceKind::KeyboardMouse);
        assert_eq!(RawInput::PadAxis(PadAxis::R2).device(), DeviceKind::Gamepad);
        assert!(RawInput::PadAxis(PadAxis::LeftX).is_analog());
        assert!(!RawInput::PadButton(PadButton::L2).is_analog());
        assert!(RawInput::MouseAxis(MouseAxis::Y).is_relative());
        assert!(!RawInput::PadAxis(PadAxis::RightY).is_relative());
    }
}
