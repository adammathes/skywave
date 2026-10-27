//! What the menus call actions and raw inputs. Pad buttons are named by position in the
//! common four-face layout (A bottom, B right, X left, Y top), as the Steam Deck and most
//! pads print them; nothing is brand-specific.

use core::fmt;

use crate::input::{
    Action, Binding, DeviceKind, Key, MouseAxis, PadAxis, PadButton, RawInput, Sign,
};

/// The menu name of an action, or of one direction of a bipolar action (`sign` is
/// `Negative` or `Positive` for those, `Full` otherwise).
pub fn action_label(action: Action, sign: Sign) -> &'static str {
    use Sign::{Negative as N, Positive as P};
    match (action, sign) {
        (Action::Throttle, _) => "Throttle",
        (Action::Brake, _) => "Brake / reverse",
        (Action::Steer, N) => "Steer left",
        (Action::Steer, P) => "Steer right",
        (Action::Steer, _) => "Steer",
        (Action::Handbrake, _) => "Handbrake",
        (Action::CameraCycle, _) => "Camera",
        (Action::LookX, N) => "Look left",
        (Action::LookX, P) => "Look right",
        (Action::LookX, _) => "Look left / right",
        (Action::LookY, N) => "Look up",
        (Action::LookY, P) => "Look down",
        (Action::LookY, _) => "Look up / down",
        (Action::LookBack, _) => "Look back",
        (Action::RadioNext, _) => "Next station / track",
        (Action::RadioPrev, _) => "Previous station / track",
        (Action::RadioOff, _) => "Radio on/off, play/pause",
        (Action::DeckSource, _) => "Radio / deck",
        (Action::DeckMode, _) => "Deck mode",
        (Action::AlbumNext, _) => "Next album",
        (Action::AlbumPrev, _) => "Previous album",
        (Action::Headlights, _) => "Headlights",
        (Action::HighBeams, _) => "High beams",
        (Action::Horn, _) => "Horn",
        (Action::VehicleNext, _) => "Next vehicle",
        (Action::Reset, _) => "Reset car",
        (Action::MapToggle, _) => "Map",
        (Action::MapZoom, N) => "Map zoom out",
        (Action::MapZoom, P) => "Map zoom in",
        (Action::MapZoom, _) => "Map zoom",
        (Action::TimeScrub, N) => "Time backward",
        (Action::TimeScrub, P) => "Time forward",
        (Action::TimeScrub, _) => "Time of day scrub",
        (Action::Pause, _) => "Pause menu",
        (Action::Screenshot, _) => "Screenshot",
        (Action::DebugOverlay, _) => "Performance overlay",
        (Action::Controls, _) => "Controls card",
        (Action::MenuUp, _) => "Menu up",
        (Action::MenuDown, _) => "Menu down",
        (Action::MenuLeft, _) => "Menu left",
        (Action::MenuRight, _) => "Menu right",
        (Action::MenuConfirm, _) => "Menu select",
        (Action::MenuBack, _) => "Menu back",
        (Action::FlyForward, _) => "Fly forward",
        (Action::FlyBack, _) => "Fly back",
        (Action::FlyLeft, _) => "Fly left",
        (Action::FlyRight, _) => "Fly right",
        (Action::FlyUp, _) => "Fly up",
        (Action::FlyDown, _) => "Fly down",
        (Action::FlyLookX, N) => "Fly turn left",
        (Action::FlyLookX, P) => "Fly turn right",
        (Action::FlyLookX, _) => "Fly turn",
        (Action::FlyLookY, N) => "Fly look up",
        (Action::FlyLookY, P) => "Fly look down",
        (Action::FlyLookY, _) => "Fly look up / down",
    }
}

pub fn device_label(device: DeviceKind) -> &'static str {
    match device {
        DeviceKind::KeyboardMouse => "Keyboard & mouse",
        DeviceKind::Gamepad => "Gamepad",
    }
}

/// The device in running text ("on the gamepad").
pub fn device_noun(device: DeviceKind) -> &'static str {
    match device {
        DeviceKind::KeyboardMouse => "keyboard & mouse",
        DeviceKind::Gamepad => "gamepad",
    }
}

fn key_name(k: Key) -> &'static str {
    match k {
        Key::Escape => "Esc",
        Key::Enter => "Enter",
        Key::Space => "Space",
        Key::Tab => "Tab",
        Key::Backspace => "Backspace",
        Key::ShiftLeft => "L Shift",
        Key::ShiftRight => "R Shift",
        Key::ControlLeft => "L Ctrl",
        Key::ControlRight => "R Ctrl",
        Key::AltLeft => "L Alt",
        Key::AltRight => "R Alt",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::BracketLeft => "[",
        Key::BracketRight => "]",
        Key::Backslash => "\\",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Slash => "/",
        Key::Semicolon => ";",
        Key::Quote => "'",
        Key::Minus => "-",
        Key::Equal => "=",
        Key::Backquote => "`",
        other => {
            // "KeyW" -> "W", "Digit4" -> "4", "F12" stays.
            let code = other.code();
            code.strip_prefix("Key")
                .or_else(|| code.strip_prefix("Digit"))
                .unwrap_or(code)
        }
    }
}

fn pad_button_name(b: PadButton) -> &'static str {
    match b {
        PadButton::South => "A",
        PadButton::East => "B",
        PadButton::West => "X",
        PadButton::North => "Y",
        PadButton::L1 => "LB",
        PadButton::R1 => "RB",
        PadButton::L2 => "LT",
        PadButton::R2 => "RT",
        PadButton::Select => "Select",
        PadButton::Start => "Start",
        PadButton::L3 => "LS click",
        PadButton::R3 => "RS click",
        PadButton::DpadUp => "D-pad up",
        PadButton::DpadDown => "D-pad down",
        PadButton::DpadLeft => "D-pad left",
        PadButton::DpadRight => "D-pad right",
        PadButton::Guide => "Guide",
    }
}

/// Writes the short name of a raw input ("W", "A", "LS up", "Mouse X").
pub fn write_raw(out: &mut impl fmt::Write, raw: RawInput, sign: Sign) -> fmt::Result {
    match raw {
        RawInput::Key(k) => out.write_str(key_name(k)),
        RawInput::MouseButton(0) => out.write_str("LMB"),
        RawInput::MouseButton(1) => out.write_str("MMB"),
        RawInput::MouseButton(2) => out.write_str("RMB"),
        RawInput::MouseButton(b) => write!(out, "Mouse {}", u32::from(b) + 1),
        RawInput::MouseAxis(MouseAxis::X) => out.write_str("Mouse X"),
        RawInput::MouseAxis(MouseAxis::Y) => out.write_str("Mouse Y"),
        RawInput::MouseWheel => out.write_str("Wheel"),
        RawInput::PadButton(b) => out.write_str(pad_button_name(b)),
        RawInput::PadAxis(a) => {
            let (stick, axis) = match a {
                PadAxis::LeftX => ("LS", 'X'),
                PadAxis::LeftY => ("LS", 'Y'),
                PadAxis::RightX => ("RS", 'X'),
                PadAxis::RightY => ("RS", 'Y'),
                PadAxis::L2 => return out.write_str("LT"),
                PadAxis::R2 => return out.write_str("RT"),
            };
            let dir = match (axis, sign) {
                ('X', Sign::Negative) => "left",
                ('X', Sign::Positive) => "right",
                ('Y', Sign::Negative) => "up",
                ('Y', Sign::Positive) => "down",
                _ => return write!(out, "{stick} {axis}"),
            };
            write!(out, "{stick} {dir}")
        }
    }
}

/// Writes one binding, chord first ("RMB + Mouse X").
pub fn write_binding(out: &mut impl fmt::Write, b: &Binding) -> fmt::Result {
    if let Some(hold) = b.hold {
        write_raw(out, hold, Sign::Full)?;
        out.write_str(" + ")?;
    }
    write_raw(out, b.raw, b.sign)?;
    if b.long {
        out.write_str(" (hold)")?;
    }
    Ok(())
}

/// Writes a list of bindings separated by commas, or "-" for none.
pub fn write_bindings<'a>(
    out: &mut impl fmt::Write,
    list: impl IntoIterator<Item = &'a Binding>,
) -> fmt::Result {
    let mut any = false;
    for b in list {
        if any {
            out.write_str(", ")?;
        }
        write_binding(out, b)?;
        any = true;
    }
    if !any {
        out.write_char('-')?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(r: RawInput, s: Sign) -> String {
        let mut out = String::new();
        write_raw(&mut out, r, s).unwrap();
        out
    }

    #[test]
    fn names_are_short_and_printable() {
        assert_eq!(raw(RawInput::Key(Key::KeyW), Sign::Positive), "W");
        assert_eq!(raw(RawInput::Key(Key::Digit4), Sign::Positive), "4");
        assert_eq!(raw(RawInput::Key(Key::F12), Sign::Positive), "F12");
        assert_eq!(raw(RawInput::Key(Key::Escape), Sign::Positive), "Esc");
        assert_eq!(
            raw(RawInput::PadButton(PadButton::South), Sign::Positive),
            "A"
        );
        assert_eq!(
            raw(RawInput::PadAxis(PadAxis::LeftY), Sign::Negative),
            "LS up"
        );
        assert_eq!(raw(RawInput::PadAxis(PadAxis::RightX), Sign::Full), "RS X");
        assert_eq!(raw(RawInput::PadAxis(PadAxis::R2), Sign::Full), "RT");
        assert_eq!(raw(RawInput::MouseButton(4), Sign::Positive), "Mouse 5");
        let chord = Binding::new(RawInput::MouseAxis(MouseAxis::X), Sign::Full)
            .while_held(RawInput::MouseButton(2));
        let mut out = String::new();
        write_bindings(&mut out, [&chord, &chord]).unwrap();
        assert_eq!(out, "RMB + Mouse X, RMB + Mouse X");
        out.clear();
        write_bindings(&mut out, []).unwrap();
        assert_eq!(out, "-");
        // Every key and action name only uses characters the font has.
        for k in Key::all() {
            assert!(raw(RawInput::Key(k), Sign::Positive)
                .bytes()
                .all(|c| (32..127).contains(&c)));
        }
        for a in Action::ALL {
            for s in [Sign::Negative, Sign::Positive, Sign::Full] {
                assert!(action_label(a, s).bytes().all(|c| (32..127).contains(&c)));
            }
        }
    }
}
