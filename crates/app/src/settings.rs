//! The settings document: video, world and input options in one versioned JSON file,
//! loaded and saved through the platform's [`SettingsStore`].
//!
//! ```json
//! { "version": 1,
//!   "video": { "ui_scale": 1.0, "resolution_scale": 1.0, "bloom": true, "debug_overlay": false },
//!   "hud": { "minimap": true, "minimap_rotate": true, "minimap_range_m": 150.0 },
//!   "world": { "clock_speed": 10.0 },
//!   "radio": { "volume": 0.8, "on": false, "last_station": null, "disabled": [],
//!              "custom": [], "presets": {} },
//!   "input": { …the bindings document, see `Bindings::to_json`… } }
//! ```
//!
//! The `radio` section is [`audio::RadioSettings`].
//!
//! Loading is forgiving like the bindings format: missing sections or fields take their
//! defaults, unknown ones are ignored and values are clamped into range.

use audio::RadioSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::daylight::CLOCK_SPEEDS;
use crate::input::Bindings;
use crate::map::view::{DEFAULT_MINIMAP_RANGE, MINIMAP_RANGES_M};

/// The settings format version written by [`Settings::to_json`].
pub const SETTINGS_VERSION: u32 = 1;

/// Where the platform keeps the settings document: `localStorage` on the web, a file in
/// the XDG config directory natively.
pub trait SettingsStore {
    /// The saved document, if there is one.
    fn load(&self) -> Option<String>;
    /// Replaces the saved document. Failures are the store's to report (log); the game
    /// carries on with what it has.
    fn save(&mut self, json: &str);
}

/// A store in memory: for tests, and for platforms with nowhere to save.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryStore {
    pub text: Option<String>,
    /// Number of saves so far.
    pub saves: u32,
}

impl SettingsStore for MemoryStore {
    fn load(&self) -> Option<String> {
        self.text.clone()
    }

    fn save(&mut self, json: &str) {
        self.text = Some(json.to_owned());
        self.saves += 1;
    }
}

/// Every user setting except the input bindings, which live beside it in the same file.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// HUD and menu scale, 1–2 (TV distance wants more).
    pub ui_scale: f32,
    /// Render resolution as a fraction of the window, 0.5–1. Stored for the renderer's
    /// upcoming scaled target; not applied yet.
    pub resolution_scale: f32,
    pub bloom: bool,
    /// The frame-stats overlay (F3).
    pub debug_overlay: bool,
    /// Game seconds per real second; one of [`CLOCK_SPEEDS`].
    pub clock_speed: f32,
    /// The minimap at the bottom left.
    pub minimap: bool,
    /// The minimap turns with the car (its forward up); off is north-up.
    pub minimap_rotate: bool,
    /// Meters across the minimap; one of [`MINIMAP_RANGES_M`].
    pub minimap_range_m: f32,
    /// Radio volume, power, last station, stations and presets.
    pub radio: RadioSettings,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            ui_scale: 1.0,
            resolution_scale: 1.0,
            bloom: true,
            debug_overlay: false,
            clock_speed: 10.0,
            minimap: true,
            minimap_rotate: true,
            minimap_range_m: MINIMAP_RANGES_M[DEFAULT_MINIMAP_RANGE],
            radio: RadioSettings::default(),
        }
    }
}

pub const UI_SCALE_STEP: f32 = 0.25;
pub const RESOLUTION_SCALE_MIN: f32 = 0.5;
pub const RESOLUTION_SCALE_STEP: f32 = 0.05;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Video {
    ui_scale: f32,
    resolution_scale: f32,
    bloom: bool,
    debug_overlay: bool,
}

impl Default for Video {
    fn default() -> Video {
        let s = Settings::default();
        Video {
            ui_scale: s.ui_scale,
            resolution_scale: s.resolution_scale,
            bloom: s.bloom,
            debug_overlay: s.debug_overlay,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct HudSection {
    minimap: bool,
    minimap_rotate: bool,
    minimap_range_m: f32,
}

impl Default for HudSection {
    fn default() -> HudSection {
        let s = Settings::default();
        HudSection {
            minimap: s.minimap,
            minimap_rotate: s.minimap_rotate,
            minimap_range_m: s.minimap_range_m,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct World {
    clock_speed: f32,
}

impl Default for World {
    fn default() -> World {
        World {
            clock_speed: Settings::default().clock_speed,
        }
    }
}

impl Settings {
    /// Every value inside its range: scales clamped (non-finite ones take the default),
    /// the clock speed and minimap range snapped to the nearest offered value.
    pub fn sanitized(self) -> Settings {
        let d = Settings::default();
        let fix =
            |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        let nearest = |v: f32, offered: &[f32], def: f32| {
            if v.is_finite() {
                offered
                    .iter()
                    .copied()
                    .min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs()))
                    .unwrap_or(def)
            } else {
                def
            }
        };
        let speed = nearest(self.clock_speed, &CLOCK_SPEEDS, d.clock_speed);
        let range = nearest(self.minimap_range_m, &MINIMAP_RANGES_M, d.minimap_range_m);
        Settings {
            ui_scale: fix(
                self.ui_scale,
                render::UI_SCALE_MIN,
                render::UI_SCALE_MAX,
                d.ui_scale,
            ),
            resolution_scale: fix(self.resolution_scale, RESOLUTION_SCALE_MIN, 1.0, 1.0),
            clock_speed: speed,
            minimap_range_m: range,
            radio: self.radio.sanitized(),
            ..self
        }
    }

    /// The whole settings document, bindings included.
    pub fn to_json(&self, bindings: &Bindings) -> String {
        let input: Value = serde_json::from_str(&bindings.to_json()).unwrap_or(Value::Null);
        let doc = serde_json::json!({
            "version": SETTINGS_VERSION,
            "video": Video {
                ui_scale: self.ui_scale,
                resolution_scale: self.resolution_scale,
                bloom: self.bloom,
                debug_overlay: self.debug_overlay,
            },
            "hud": HudSection {
                minimap: self.minimap,
                minimap_rotate: self.minimap_rotate,
                minimap_range_m: self.minimap_range_m,
            },
            "world": World { clock_speed: self.clock_speed },
            "radio": &self.radio,
            "input": input,
        });
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    }

    /// Reads a settings document; anything unreadable takes its default. Returns the
    /// bindings too when the document has an `input` section.
    pub fn from_json(text: &str) -> (Settings, Option<Bindings>) {
        let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(text) else {
            return (Settings::default(), None);
        };
        let section = |root: &mut serde_json::Map<String, Value>, key: &str| {
            root.remove(key).unwrap_or(Value::Null)
        };
        let video: Video = serde_json::from_value(section(&mut root, "video")).unwrap_or_default();
        let hud: HudSection = serde_json::from_value(section(&mut root, "hud")).unwrap_or_default();
        let world: World = serde_json::from_value(section(&mut root, "world")).unwrap_or_default();
        let radio: RadioSettings =
            serde_json::from_value(section(&mut root, "radio")).unwrap_or_default();
        let bindings = match section(&mut root, "input") {
            v @ Value::Object(_) => Bindings::from_json(&v.to_string()).ok(),
            _ => None,
        };
        let settings = Settings {
            ui_scale: video.ui_scale,
            resolution_scale: video.resolution_scale,
            bloom: video.bloom,
            debug_overlay: video.debug_overlay,
            clock_speed: world.clock_speed,
            minimap: hud.minimap,
            minimap_rotate: hud.minimap_rotate,
            minimap_range_m: hud.minimap_range_m,
            radio,
        }
        .sanitized();
        (settings, bindings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Action, DeviceKind};

    #[test]
    fn settings_and_bindings_round_trip() {
        let s = Settings {
            ui_scale: 1.75,
            resolution_scale: 0.75,
            bloom: false,
            debug_overlay: true,
            clock_speed: 60.0,
            minimap: false,
            minimap_rotate: false,
            minimap_range_m: 250.0,
            radio: RadioSettings {
                volume: 0.45,
                on: true,
                last_station: Some("somafm_seventies".into()),
                disabled: vec!["radioparadise_main".into()],
                ..RadioSettings::default()
            },
        };
        let mut s = s;
        s.radio.add_custom("https://example.org/stream").unwrap();
        s.radio
            .presets
            .insert("socal_beach_1984".into(), "1977_chicago".into());
        let mut b = Bindings::default();
        let mut o = b.axis_options(DeviceKind::Gamepad, Action::LookY);
        o.invert = true;
        b.set_axis_options(DeviceKind::Gamepad, Action::LookY, o);
        let text = s.to_json(&b);
        assert!(text.contains("\"version\": 1"));
        let (s2, b2) = Settings::from_json(&text);
        assert_eq!(s2, s);
        assert_eq!(b2, Some(b));
    }

    #[test]
    fn loading_is_forgiving() {
        let (s, b) = Settings::from_json("not json");
        assert_eq!((s, b), (Settings::default(), None));
        let (s, b) = Settings::from_json("[1, 2]");
        assert_eq!((s, b), (Settings::default(), None));
        // Partial sections, wrong types, out-of-range values, unknown keys.
        let (s, b) = Settings::from_json(
            r#"{ "video": { "ui_scale": 7, "bloom": false, "extra": 1 },
                 "world": { "clock_speed": 12 }, "input": 5, "radio": {},
                 "hud": { "minimap_rotate": false, "minimap_range_m": 120 } }"#,
        );
        assert_eq!(
            s,
            Settings {
                ui_scale: 2.0,
                bloom: false,
                clock_speed: 10.0,
                minimap_rotate: false,
                minimap_range_m: 100.0,
                ..Settings::default()
            }
        );
        assert_eq!(b, None);
        let (s, _) = Settings::from_json(r#"{ "video": { "ui_scale": "big" } }"#);
        assert_eq!(s, Settings::default());
        let (_, b) = Settings::from_json(r#"{ "input": {} }"#);
        assert_eq!(b, Some(Bindings::default()));
        // A radio section with bad values keeps what it can.
        let (s, _) = Settings::from_json(
            r#"{ "radio": { "volume": 9, "on": true, "custom": [{ "id": "custom_1",
                 "name": "x", "url": "http://plain" }], "last_station": 3 } }"#,
        );
        assert_eq!(
            s.radio,
            RadioSettings::default(),
            "a wrong type drops the section"
        );
        let (s, _) = Settings::from_json(
            r#"{ "radio": { "volume": 9, "on": true, "custom": [{ "id": "custom_1",
                 "name": "x", "url": "http://plain" }] } }"#,
        );
        assert_eq!((s.radio.volume, s.radio.on), (1.0, true));
        assert!(s.radio.custom.is_empty());
    }

    #[test]
    fn memory_store_keeps_the_last_save() {
        let mut m = MemoryStore::default();
        assert_eq!(m.load(), None);
        m.save("a");
        m.save("b");
        assert_eq!((m.load().as_deref(), m.saves), (Some("b"), 2));
    }
}
