//! The settings document: video, world and input options in one versioned JSON file,
//! loaded and saved through the platform's [`SettingsStore`].
//!
//! ```json
//! { "version": 1,
//!   "drive": { "handling": "arcade" },
//!   "camera": { "mode": "chase-mid", "cockpit_fov_deg": 62.0, "seat_height_cm": 0.0,
//!               "seat_forward_cm": 0.0, "dashboard": true, "shake": true },
//!   "video": { "ui_scale": 1.0, "resolution_scale": 1.0, "bloom": true, "debug_overlay": false },
//!   "hud": { "minimap": true, "minimap_rotate": true, "minimap_range_m": 150.0 },
//!   "world": { "clock_speed": 10.0 },
//!   "input": { …the bindings document, see `Bindings::to_json`… } }
//! ```
//!
//! Loading is forgiving like the bindings format: missing sections or fields take their
//! defaults, unknown ones are ignored and values are clamped into range.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::camera::{CameraMode, TUNING};
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

/// How much the car helps: the [`vehicle::Assists`] preset the player drives with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handling {
    /// [`vehicle::Assists::ARCADE`]: tight and forgiving (the default).
    Arcade,
    /// [`vehicle::Assists::LIGHT`]: the arcade-sim feel.
    Light,
    /// [`vehicle::Assists::OFF`]: the "driver" feel.
    Off,
}

impl Handling {
    /// In the order the Drive page offers them.
    pub const ALL: [Handling; 3] = [Handling::Arcade, Handling::Light, Handling::Off];

    pub fn assists(self) -> vehicle::Assists {
        match self {
            Handling::Arcade => vehicle::Assists::ARCADE,
            Handling::Light => vehicle::Assists::LIGHT,
            Handling::Off => vehicle::Assists::OFF,
        }
    }

    /// Stable name (settings JSON).
    pub fn name(self) -> &'static str {
        match self {
            Handling::Arcade => "arcade",
            Handling::Light => "light",
            Handling::Off => "off",
        }
    }

    pub fn from_name(name: &str) -> Option<Handling> {
        Handling::ALL.into_iter().find(|h| h.name() == name)
    }
}

/// Cockpit field of view, degrees: range and slider step.
pub const COCKPIT_FOV_MIN: f32 = 50.0;
pub const COCKPIT_FOV_MAX: f32 = 80.0;
pub const COCKPIT_FOV_STEP: f32 = 1.0;
/// Seat adjustment of the cockpit eye, cm: height and fore-aft ranges and the slider step.
pub const SEAT_HEIGHT_CM: [f32; 2] = [-5.0, 10.0];
pub const SEAT_FORWARD_CM: [f32; 2] = [-10.0, 10.0];
pub const SEAT_STEP_CM: f32 = 1.0;

/// Every user setting except the input bindings, which live beside it in the same file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// The assists preset.
    pub handling: Handling,
    /// The selected camera: changed on the Camera page and by cycling while driving, and
    /// the one the game starts in (a scene's `cam=` overrides it). Never `Orbit`.
    pub camera: CameraMode,
    /// Vertical field of view of the cockpit camera, degrees; the other cameras keep theirs.
    pub cockpit_fov_deg: f32,
    /// The driver's eye moved up and forward from the car's own seat, cm.
    pub seat_height_cm: f32,
    pub seat_forward_cm: f32,
    /// The cockpit draws the dashboard, wheel and gauges; off leaves the windshield frame
    /// and the hood ("hood only").
    pub dashboard: bool,
    /// Head sway in the cockpit from the car's g-forces and springs.
    pub camera_shake: bool,
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
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            handling: Handling::Arcade,
            camera: CameraMode::ChaseMid,
            cockpit_fov_deg: TUNING.cockpit_fov_deg,
            seat_height_cm: 0.0,
            seat_forward_cm: 0.0,
            dashboard: true,
            camera_shake: true,
            ui_scale: 1.0,
            resolution_scale: 1.0,
            bloom: true,
            debug_overlay: false,
            clock_speed: 10.0,
            minimap: true,
            minimap_rotate: true,
            minimap_range_m: MINIMAP_RANGES_M[DEFAULT_MINIMAP_RANGE],
        }
    }
}

pub const UI_SCALE_STEP: f32 = 0.25;
pub const RESOLUTION_SCALE_MIN: f32 = 0.5;
pub const RESOLUTION_SCALE_STEP: f32 = 0.05;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct DriveSection {
    handling: String,
}

impl Default for DriveSection {
    fn default() -> DriveSection {
        DriveSection {
            handling: Settings::default().handling.name().to_owned(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct CameraSection {
    mode: String,
    cockpit_fov_deg: f32,
    seat_height_cm: f32,
    seat_forward_cm: f32,
    dashboard: bool,
    shake: bool,
}

impl Default for CameraSection {
    fn default() -> CameraSection {
        CameraSection::of(&Settings::default())
    }
}

impl CameraSection {
    fn of(s: &Settings) -> CameraSection {
        CameraSection {
            mode: s.camera.name().to_owned(),
            cockpit_fov_deg: s.cockpit_fov_deg,
            seat_height_cm: s.seat_height_cm,
            seat_forward_cm: s.seat_forward_cm,
            dashboard: s.dashboard,
            shake: s.camera_shake,
        }
    }
}

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
        let camera = match self.camera {
            CameraMode::Orbit => d.camera,
            m => m,
        };
        Settings {
            camera,
            cockpit_fov_deg: fix(
                self.cockpit_fov_deg,
                COCKPIT_FOV_MIN,
                COCKPIT_FOV_MAX,
                d.cockpit_fov_deg,
            ),
            seat_height_cm: fix(
                self.seat_height_cm,
                SEAT_HEIGHT_CM[0],
                SEAT_HEIGHT_CM[1],
                0.0,
            ),
            seat_forward_cm: fix(
                self.seat_forward_cm,
                SEAT_FORWARD_CM[0],
                SEAT_FORWARD_CM[1],
                0.0,
            ),
            ui_scale: fix(
                self.ui_scale,
                render::UI_SCALE_MIN,
                render::UI_SCALE_MAX,
                d.ui_scale,
            ),
            resolution_scale: fix(self.resolution_scale, RESOLUTION_SCALE_MIN, 1.0, 1.0),
            clock_speed: speed,
            minimap_range_m: range,
            ..self
        }
    }

    /// The whole settings document, bindings included.
    pub fn to_json(&self, bindings: &Bindings) -> String {
        let input: Value = serde_json::from_str(&bindings.to_json()).unwrap_or(Value::Null);
        let doc = serde_json::json!({
            "version": SETTINGS_VERSION,
            "drive": DriveSection { handling: self.handling.name().to_owned() },
            "camera": CameraSection::of(self),
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
        let drive: DriveSection =
            serde_json::from_value(section(&mut root, "drive")).unwrap_or_default();
        let camera: CameraSection =
            serde_json::from_value(section(&mut root, "camera")).unwrap_or_default();
        let video: Video = serde_json::from_value(section(&mut root, "video")).unwrap_or_default();
        let hud: HudSection = serde_json::from_value(section(&mut root, "hud")).unwrap_or_default();
        let world: World = serde_json::from_value(section(&mut root, "world")).unwrap_or_default();
        let bindings = match section(&mut root, "input") {
            v @ Value::Object(_) => Bindings::from_json(&v.to_string()).ok(),
            _ => None,
        };
        let d = Settings::default();
        let settings = Settings {
            handling: Handling::from_name(&drive.handling).unwrap_or(d.handling),
            camera: CameraMode::from_name(&camera.mode).unwrap_or(d.camera),
            cockpit_fov_deg: camera.cockpit_fov_deg,
            seat_height_cm: camera.seat_height_cm,
            seat_forward_cm: camera.seat_forward_cm,
            dashboard: camera.dashboard,
            camera_shake: camera.shake,
            ui_scale: video.ui_scale,
            resolution_scale: video.resolution_scale,
            bloom: video.bloom,
            debug_overlay: video.debug_overlay,
            clock_speed: world.clock_speed,
            minimap: hud.minimap,
            minimap_rotate: hud.minimap_rotate,
            minimap_range_m: hud.minimap_range_m,
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
            handling: Handling::Off,
            camera: CameraMode::Cockpit,
            cockpit_fov_deg: 71.0,
            seat_height_cm: 4.0,
            seat_forward_cm: -3.0,
            dashboard: false,
            camera_shake: false,
            ui_scale: 1.75,
            resolution_scale: 0.75,
            bloom: false,
            debug_overlay: true,
            clock_speed: 60.0,
            minimap: false,
            minimap_rotate: false,
            minimap_range_m: 250.0,
        };
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
                 "hud": { "minimap_rotate": false, "minimap_range_m": 120 },
                 "drive": { "handling": "light" },
                 "camera": { "mode": "orbit", "cockpit_fov_deg": 120, "seat_height_cm": -9,
                             "seat_forward_cm": 4 } }"#,
        );
        assert_eq!(
            s,
            Settings {
                handling: Handling::Light,
                cockpit_fov_deg: COCKPIT_FOV_MAX,
                seat_height_cm: SEAT_HEIGHT_CM[0],
                seat_forward_cm: 4.0,
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
        let (s, _) = Settings::from_json(
            r#"{ "drive": { "handling": "rails" }, "camera": { "mode": "drone" } }"#,
        );
        assert_eq!(s, Settings::default());
        let (_, b) = Settings::from_json(r#"{ "input": {} }"#);
        assert_eq!(b, Some(Bindings::default()));
    }

    #[test]
    fn defaults_are_arcade_handling_and_the_mid_chase_camera() {
        let s = Settings::default();
        assert_eq!(s.handling.assists(), vehicle::Assists::default());
        assert_eq!(s.camera, CameraMode::ChaseMid);
        assert_eq!(s.cockpit_fov_deg, 62.0);
        assert!(s.dashboard && s.camera_shake);
        for h in Handling::ALL {
            assert_eq!(Handling::from_name(h.name()), Some(h));
        }
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
