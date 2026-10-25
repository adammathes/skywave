//! Per-frame input, already mapped from keys/buttons to game actions by the platform.

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Up,
    Down,
    /// Turn right (+) / left (-), in `[-1, 1]` of the full turn rate.
    LookX,
    /// Look up (+) / down (-), in `[-1, 1]` of the full turn rate.
    LookY,
}

impl Action {
    pub const COUNT: usize = 8;
    pub const ALL: [Action; Action::COUNT] = [
        Action::Forward,
        Action::Back,
        Action::Left,
        Action::Right,
        Action::Up,
        Action::Down,
        Action::LookX,
        Action::LookY,
    ];
}

/// The value of every action this frame. Buttons are `0.0` or `1.0`; look axes are in
/// `[-1, 1]`. Values outside that range are clamped on write.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputState {
    values: [f32; Action::COUNT],
}

impl InputState {
    pub fn set(&mut self, action: Action, value: f32) {
        let v = if value.is_finite() { value } else { 0.0 };
        self.values[action as usize] = v.clamp(-1.0, 1.0);
    }

    /// Sets a button action from a key state.
    pub fn set_pressed(&mut self, action: Action, down: bool) {
        self.set(action, if down { 1.0 } else { 0.0 });
    }

    /// Sets an axis from a pair of opposing keys, e.g. the left and right arrows.
    pub fn set_from_keys(&mut self, action: Action, negative: bool, positive: bool) {
        self.set(
            action,
            f32::from(u8::from(positive)) - f32::from(u8::from(negative)),
        );
    }

    pub fn axis(&self, action: Action) -> f32 {
        self.values[action as usize]
    }

    pub fn pressed(&self, action: Action) -> bool {
        self.values[action as usize] > 0.5
    }

    pub fn clear(&mut self) {
        self.values = [0.0; Action::COUNT];
    }
}

/// The physical keys the game listens to, named after the W3C UI Events `code` values
/// (which winit's `KeyCode` also follows), so each platform maps its keys 1:1.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    KeyW,
    KeyA,
    KeyS,
    KeyD,
    KeyE,
    KeyQ,
    Space,
    ShiftLeft,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
}

impl Key {
    pub const COUNT: usize = 12;

    /// Looks up a DOM `KeyboardEvent.code` string.
    pub fn from_code(code: &str) -> Option<Key> {
        Some(match code {
            "KeyW" => Key::KeyW,
            "KeyA" => Key::KeyA,
            "KeyS" => Key::KeyS,
            "KeyD" => Key::KeyD,
            "KeyE" => Key::KeyE,
            "KeyQ" => Key::KeyQ,
            "Space" => Key::Space,
            "ShiftLeft" => Key::ShiftLeft,
            "ArrowUp" => Key::ArrowUp,
            "ArrowDown" => Key::ArrowDown,
            "ArrowLeft" => Key::ArrowLeft,
            "ArrowRight" => Key::ArrowRight,
            _ => return None,
        })
    }
}

/// Which game keys are held, and the default key bindings that turn them into actions:
/// WASD to move, E/Space up, Q/Shift down, arrow keys to look.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keyboard {
    held: [bool; Key::COUNT],
}

impl Keyboard {
    pub fn set(&mut self, key: Key, down: bool) {
        self.held[key as usize] = down;
    }

    pub fn is_held(&self, key: Key) -> bool {
        self.held[key as usize]
    }

    /// Releases every key, e.g. when the window loses focus.
    pub fn release_all(&mut self) {
        self.held = [false; Key::COUNT];
    }

    pub fn input(&self) -> InputState {
        let k = |key| self.is_held(key);
        let mut s = InputState::default();
        s.set_pressed(Action::Forward, k(Key::KeyW));
        s.set_pressed(Action::Back, k(Key::KeyS));
        s.set_pressed(Action::Left, k(Key::KeyA));
        s.set_pressed(Action::Right, k(Key::KeyD));
        s.set_pressed(Action::Up, k(Key::KeyE) || k(Key::Space));
        s.set_pressed(Action::Down, k(Key::KeyQ) || k(Key::ShiftLeft));
        s.set_from_keys(Action::LookX, k(Key::ArrowLeft), k(Key::ArrowRight));
        s.set_from_keys(Action::LookY, k(Key::ArrowDown), k(Key::ArrowUp));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_index_their_slot() {
        for (i, a) in Action::ALL.iter().enumerate() {
            assert_eq!(*a as usize, i);
        }
    }

    #[test]
    fn set_clamps_and_keys_combine() {
        let mut s = InputState::default();
        s.set(Action::LookX, 3.0);
        assert_eq!(s.axis(Action::LookX), 1.0);
        s.set(Action::LookY, f32::NAN);
        assert_eq!(s.axis(Action::LookY), 0.0);
        s.set_from_keys(Action::LookX, true, false);
        assert_eq!(s.axis(Action::LookX), -1.0);
        s.set_from_keys(Action::LookX, true, true);
        assert_eq!(s.axis(Action::LookX), 0.0);
        s.set_pressed(Action::Forward, true);
        assert!(s.pressed(Action::Forward));
        assert!(!s.pressed(Action::Back));
        s.clear();
        assert_eq!(s, InputState::default());
    }

    #[test]
    fn keyboard_bindings() {
        let mut kb = Keyboard::default();
        for code in ["KeyW", "ArrowLeft", "Space"] {
            kb.set(Key::from_code(code).unwrap(), true);
        }
        assert_eq!(Key::from_code("KeyZ"), None);
        let s = kb.input();
        assert!(s.pressed(Action::Forward) && s.pressed(Action::Up));
        assert!(!s.pressed(Action::Back) && !s.pressed(Action::Down));
        assert_eq!(s.axis(Action::LookX), -1.0);
        assert_eq!(s.axis(Action::LookY), 0.0);
        kb.release_all();
        assert_eq!(kb.input(), InputState::default());
    }

    #[test]
    fn key_codes_round_trip() {
        let all = [
            "KeyW",
            "KeyA",
            "KeyS",
            "KeyD",
            "KeyE",
            "KeyQ",
            "Space",
            "ShiftLeft",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
        ];
        assert_eq!(all.len(), Key::COUNT);
        for (i, code) in all.iter().enumerate() {
            assert_eq!(Key::from_code(code).map(|k| k as usize), Some(i));
        }
    }
}
