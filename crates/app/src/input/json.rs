//! `Bindings` ⇄ settings JSON.
//!
//! ```json
//! { "version": 1,
//!   "keyboard_mouse": {
//!     "bindings": { "Steer": [ { "raw": { "Key": "KeyA" }, "sign": "Negative" }, … ], … },
//!     "axes": { "LookY": { "invert": true, "sensitivity": 1.0, "deadzone": 0.0, "curve": 1.0 }, … } },
//!   "gamepad": { … } }
//! ```
//!
//! Loading is forgiving, so an old or hand-edited file never locks anyone out: a missing
//! device, action or option takes its default, unknown fields and action names are
//! ignored, a binding that does not parse is skipped (an action left with none of its listed
//! bindings takes its default), and options are clamped into range. An explicitly empty
//! list unbinds the action.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};

use super::bindings::{AxisOptions, Binding, Bindings};
use super::raw::DeviceKind;
use super::Action;

/// The settings format version written by [`Bindings::to_json`].
pub const BINDINGS_VERSION: u32 = 1;

/// Why a bindings file could not be read at all.
#[derive(Clone, Debug, PartialEq)]
pub enum BindingsError {
    /// Not valid JSON.
    Syntax(String),
    /// Valid JSON, but not an object.
    NotAnObject,
}

impl fmt::Display for BindingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindingsError::Syntax(e) => write!(f, "bindings JSON: {e}"),
            BindingsError::NotAnObject => f.write_str("bindings JSON: expected an object"),
        }
    }
}

impl std::error::Error for BindingsError {}

fn device_key(device: DeviceKind) -> &'static str {
    match device {
        DeviceKind::KeyboardMouse => "keyboard_mouse",
        DeviceKind::Gamepad => "gamepad",
    }
}

#[derive(Serialize)]
struct DeviceOut<'a> {
    bindings: BTreeMap<&'static str, &'a [Binding]>,
    axes: BTreeMap<&'static str, AxisOptions>,
}

#[derive(Serialize)]
struct FileOut<'a> {
    version: u32,
    keyboard_mouse: DeviceOut<'a>,
    gamepad: DeviceOut<'a>,
}

impl Bindings {
    /// Pretty-printed settings JSON with every action and option written out.
    pub fn to_json(&self) -> String {
        let device = |d| DeviceOut {
            bindings: Action::ALL
                .iter()
                .map(|&a| (a.name(), self.get(d, a)))
                .collect(),
            axes: Action::ANALOG
                .iter()
                .map(|&a| (a.name(), self.axis_options(d, a)))
                .collect(),
        };
        let file = FileOut {
            version: BINDINGS_VERSION,
            keyboard_mouse: device(DeviceKind::KeyboardMouse),
            gamepad: device(DeviceKind::Gamepad),
        };
        // Only plain structs, strings and finite floats: serialization cannot fail.
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// Reads settings JSON written by any version, filling defaults (see the module docs).
    pub fn from_json(text: &str) -> Result<Bindings, BindingsError> {
        let root: Value =
            serde_json::from_str(text).map_err(|e| BindingsError::Syntax(e.to_string()))?;
        let Value::Object(mut root) = root else {
            return Err(BindingsError::NotAnObject);
        };
        let mut out = Bindings::default();
        for device in DeviceKind::ALL {
            if let Some(Value::Object(section)) = root.remove(device_key(device)) {
                read_device(&mut out, device, section);
            }
        }
        Ok(out)
    }
}

fn read_device(out: &mut Bindings, device: DeviceKind, mut section: Map<String, Value>) {
    if let Some(Value::Object(lists)) = section.remove("bindings") {
        for (name, list) in lists {
            let (Some(action), Value::Array(items)) = (Action::from_name(&name), list) else {
                continue;
            };
            let listed = items.len();
            let parsed: Vec<Binding> = items
                .into_iter()
                .filter_map(|v| serde_json::from_value::<Binding>(v).ok())
                .filter(|b| b.raw.device() == device)
                .collect();
            if listed == 0 || !parsed.is_empty() {
                out.set(device, action, parsed);
            }
        }
    }
    if let Some(Value::Object(axes)) = section.remove("axes") {
        for (name, fields) in axes {
            let (Some(action), Value::Object(fields)) = (Action::from_name(&name), fields) else {
                continue;
            };
            let mut o = out.axis_options(device, action);
            if let Some(v) = fields.get("invert").and_then(Value::as_bool) {
                o.invert = v;
            }
            let num = |key: &str| fields.get(key).and_then(Value::as_f64).map(|v| v as f32);
            if let Some(v) = num("sensitivity") {
                o.sensitivity = v;
            }
            if let Some(v) = num("deadzone") {
                o.deadzone = v;
            }
            if let Some(v) = num("curve") {
                o.curve = v;
            }
            out.set_axis_options(device, action, o);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::bindings::Sign;
    use crate::input::raw::{Key, RawInput};

    #[test]
    fn defaults_round_trip() {
        let b = Bindings::default();
        let text = b.to_json();
        assert_eq!(Bindings::from_json(&text), Ok(b.clone()));
        // settings → bindings → settings is identical text.
        assert_eq!(Bindings::from_json(&text).unwrap().to_json(), text);
        assert!(text.contains("\"version\": 1"));
        assert!(text.contains("\"Key\": \"BracketRight\""));
    }

    #[test]
    fn changed_bindings_round_trip() {
        let mut b = Bindings::default();
        b.southpaw();
        let pad = DeviceKind::Gamepad;
        let mut o = b.axis_options(pad, Action::LookY);
        o.invert = true;
        o.sensitivity = 2.5;
        b.set_axis_options(pad, Action::LookY, o);
        b.set(DeviceKind::KeyboardMouse, Action::Horn, vec![]);
        assert_eq!(Bindings::from_json(&b.to_json()), Ok(b));
    }

    #[test]
    fn missing_entries_take_defaults_and_unknowns_are_ignored() {
        let text = r#"{
            "version": 7,
            "future_field": [1, 2, 3],
            "keyboard_mouse": {
                "bindings": {
                    "Horn": [ { "raw": { "Key": "KeyJ" }, "sign": "Positive", "rumble": 1 } ],
                    "Headlights": [ { "raw": { "Key": "NoSuchKey" } } ],
                    "RadioOff": [],
                    "Teleport": [ { "raw": { "Key": "KeyT" } } ],
                    "Steer": "not a list"
                },
                "axes": { "LookY": { "invert": true }, "Steer": { "deadzone": 5.0 } }
            },
            "gamepad": { "axes": { "LookY": { "invert": true, "curve": "steep" } } }
        }"#;
        let b = Bindings::from_json(text).unwrap();
        let d = Bindings::default();
        let kb = DeviceKind::KeyboardMouse;
        let pad = DeviceKind::Gamepad;
        assert_eq!(
            b.get(kb, Action::Horn),
            &[Binding::new(RawInput::Key(Key::KeyJ), Sign::Positive)]
        );
        assert_eq!(b.get(kb, Action::Headlights), d.get(kb, Action::Headlights));
        assert!(b.get(kb, Action::RadioOff).is_empty());
        assert_eq!(b.get(kb, Action::Steer), d.get(kb, Action::Steer));
        assert_eq!(b.get(kb, Action::Throttle), d.get(kb, Action::Throttle));
        assert!(b.axis_options(kb, Action::LookY).invert);
        assert_eq!(
            b.axis_options(kb, Action::Steer).deadzone,
            AxisOptions::DEADZONE_MAX
        );
        let look = b.axis_options(pad, Action::LookY);
        assert!(look.invert);
        assert_eq!(look.deadzone, d.axis_options(pad, Action::LookY).deadzone);
        assert_eq!(look.curve, 1.0);
        for a in Action::ALL {
            assert_eq!(b.get(pad, a), d.get(pad, a), "{a:?}");
        }

        assert_eq!(Bindings::from_json("{}"), Ok(d.clone()));
        // A pad binding in the keyboard section is dropped.
        let wrong = r#"{"keyboard_mouse":{"bindings":{"Horn":[{"raw":{"PadAxis":"L2"}}]}}}"#;
        assert_eq!(Bindings::from_json(wrong), Ok(d));
    }

    #[test]
    fn unreadable_files_are_errors() {
        assert!(matches!(
            Bindings::from_json("{ nope"),
            Err(BindingsError::Syntax(_))
        ));
        assert_eq!(Bindings::from_json("[1]"), Err(BindingsError::NotAnObject));
    }
}
