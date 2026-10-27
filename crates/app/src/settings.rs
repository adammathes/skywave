//! The settings document: video, world and input options in one versioned JSON file,
//! loaded and saved through the platform's [`SettingsStore`].
//!
//! ```json
//! { "version": 1,
//!   "drive": { "handling": "arcade", "vehicle": "corsair_gtx" },
//!   "camera": { "mode": "chase-mid", "cockpit_fov_deg": 62.0, "seat_height_cm": 0.0,
//!               "seat_forward_cm": 0.0, "dashboard": true, "shake": true },
//!   "video": { "ui_scale": 1.0, "resolution_scale": 1.0, "bloom": true, "debug_overlay": false,
//!              "quality": "auto" },
//!   "hud": { "minimap": true, "minimap_rotate": true, "minimap_range_m": 150.0 },
//!   "world": { "clock_speed": 10.0, "traffic": "normal" },
//!   "audio": { "master": 0.8, "engine": 0.8, "world": 0.8, "mute": false },
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

use crate::camera::{CameraMode, TUNING};
use crate::daylight::CLOCK_SPEEDS;
use crate::input::Bindings;
use crate::map::view::{DEFAULT_MINIMAP_RANGE, MINIMAP_RANGES_M};
use crate::npc::{density_from_name, density_name};
use crate::vehicles::BUILTIN;

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

/// The graphics preset: what the renderer may spend (`docs/PERFORMANCE.md` has the table
/// and the reasoning). [`Settings::quality`] `None` is "auto": [`Quality::auto`] picks
/// for the device each run until the player chooses one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Integrated GPUs that struggle: two 1024² cascades, 75% resolution, no coronas.
    Low,
    /// The Steam Deck at 1280×800 and 90 Hz: two 1536² cascades, full resolution.
    Deck,
    /// The default: three 1536² cascades, the far one drawn every other frame, all five
    /// bloom levels (the fifth is a few hundred pixels and carries the wide night halos).
    Medium,
    /// Desktop GPUs: three 2048² cascades every frame, every caster.
    High,
    /// Big desktop GPUs: three 3072² cascades where the device allows.
    Ultra,
}

impl Quality {
    /// In the order the Video page offers them.
    pub const ALL: [Quality; 5] = [
        Quality::Low,
        Quality::Deck,
        Quality::Medium,
        Quality::High,
        Quality::Ultra,
    ];

    /// Stable name (settings JSON, `--quality`).
    pub fn name(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Deck => "deck",
            Quality::Medium => "medium",
            Quality::High => "high",
            Quality::Ultra => "ultra",
        }
    }

    pub fn from_name(name: &str) -> Option<Quality> {
        Quality::ALL.into_iter().find(|q| q.name() == name)
    }

    /// The resolution scale choosing this preset sets (the slider can change it after).
    pub fn resolution_scale(self) -> f32 {
        match self {
            Quality::Low => 0.75,
            _ => 1.0,
        }
    }

    /// The renderer's knobs for this preset, at `resolution_scale`.
    pub fn render(self, resolution_scale: f32) -> render::RenderQuality {
        let (shadow_size, cascades, bloom_levels, coronas, far_cascade_interval, min_caster) =
            match self {
                Quality::Low => (1024, 2, 4, false, 2, 3.0),
                Quality::Deck => (1536, 2, 4, true, 2, 2.0),
                Quality::Medium => (1536, 3, 5, true, 2, 2.0),
                Quality::High => (2048, 3, 5, true, 1, 0.0),
                Quality::Ultra => (3072, 3, 5, true, 1, 0.0),
            };
        render::RenderQuality {
            shadow_size,
            cascades,
            resolution_scale,
            bloom_levels,
            coronas,
            far_cascade_interval,
            min_caster_texels: min_caster,
        }
    }

    /// The preset for a device nobody chose one for: [`Quality::Deck`] on what looks like
    /// a Steam Deck (its APU's names, an AMD integrated GPU, or a 1280×800 surface on
    /// Linux), [`Quality::Medium`] everywhere else.
    pub fn auto(gpu: &GpuInfo) -> Quality {
        let name = gpu.name.to_ascii_lowercase();
        let deck_apu = ["van gogh", "vangogh", "sephiroth", "custom gpu 0405"]
            .iter()
            .any(|n| name.contains(n));
        let amd = name.contains("amd") || name.contains("radv") || name.contains("radeon");
        let deck_screen = gpu.linux && gpu.surface == (1280, 800);
        if deck_apu || (amd && gpu.integrated) || deck_screen {
            Quality::Deck
        } else {
            Quality::Medium
        }
    }
}

/// What the platform knows about the GPU: for the automatic preset and the overlay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuInfo {
    /// The adapter's name as the driver reports it (`wgpu::AdapterInfo::name`).
    pub name: String,
    /// Backend and driver, for the overlay ("Vulkan, radv Mesa 24.1").
    pub backend: String,
    /// The adapter says it is an integrated GPU.
    pub integrated: bool,
    /// Running on Linux.
    pub linux: bool,
    /// The window's size in pixels when the game started.
    pub surface: (u32, u32),
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
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The assists preset.
    pub handling: Handling,
    /// The car picked on the Vehicle page or with `VehicleNext`: a [`BUILTIN`] id, or
    /// `None` until one is picked. The game starts in it unless the scene names a car
    /// (`car=`).
    pub vehicle: Option<&'static str>,
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
    /// Render resolution as a fraction of the window, 0.5–1: the scene draws at this
    /// size and is upscaled, the HUD stays sharp.
    pub resolution_scale: f32,
    pub bloom: bool,
    /// The graphics preset; `None` picks one for the device ([`Quality::auto`]).
    pub quality: Option<Quality>,
    /// The frame-stats overlay (F3).
    pub debug_overlay: bool,
    /// Game seconds per real second; one of [`CLOCK_SPEEDS`].
    pub clock_speed: f32,
    /// NPC traffic density (a scene's `traffic=` overrides it for the session).
    pub traffic: traffic::Density,
    /// The minimap at the bottom left.
    pub minimap: bool,
    /// The minimap turns with the car (its forward up); off is north-up.
    pub minimap_rotate: bool,
    /// Meters across the minimap; one of [`MINIMAP_RANGES_M`].
    pub minimap_range_m: f32,
    /// Sound volumes, `[0, 1]`: everything, the engine, the world (tires, wind, ambience).
    pub master_volume: f32,
    pub engine_volume: f32,
    pub world_volume: f32,
    pub mute: bool,
    /// Radio volume, power, last station, stations and presets.
    pub radio: RadioSettings,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            handling: Handling::Arcade,
            vehicle: None,
            camera: CameraMode::ChaseMid,
            cockpit_fov_deg: TUNING.cockpit_fov_deg,
            seat_height_cm: 0.0,
            seat_forward_cm: 0.0,
            dashboard: true,
            camera_shake: true,
            ui_scale: 1.0,
            resolution_scale: 1.0,
            bloom: true,
            quality: None,
            debug_overlay: false,
            clock_speed: 10.0,
            traffic: traffic::Density::Normal,
            minimap: true,
            minimap_rotate: true,
            minimap_range_m: MINIMAP_RANGES_M[DEFAULT_MINIMAP_RANGE],
            master_volume: 0.8,
            engine_volume: 0.8,
            world_volume: 0.8,
            mute: false,
            radio: RadioSettings::default(),
        }
    }
}

pub const UI_SCALE_STEP: f32 = 0.25;
pub const RESOLUTION_SCALE_MIN: f32 = 0.5;
pub const RESOLUTION_SCALE_STEP: f32 = 0.05;
pub const VOLUME_STEP: f32 = 0.05;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct DriveSection {
    handling: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    vehicle: Option<String>,
}

impl Default for DriveSection {
    fn default() -> DriveSection {
        DriveSection {
            handling: Settings::default().handling.name().to_owned(),
            vehicle: None,
        }
    }
}

/// The [`BUILTIN`] id equal to `id`, if there is one.
pub fn builtin_vehicle(id: &str) -> Option<&'static str> {
    BUILTIN.iter().map(|(v, _)| *v).find(|v| *v == id)
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
    /// A [`Quality`] name, or "auto".
    quality: String,
}

impl Default for Video {
    fn default() -> Video {
        let s = Settings::default();
        Video {
            ui_scale: s.ui_scale,
            resolution_scale: s.resolution_scale,
            bloom: s.bloom,
            debug_overlay: s.debug_overlay,
            quality: quality_name(s.quality).to_owned(),
        }
    }
}

/// The settings name of a preset choice.
fn quality_name(q: Option<Quality>) -> &'static str {
    q.map_or("auto", Quality::name)
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
    /// A traffic density name (`crate::npc::density_name`).
    traffic: String,
}

impl Default for World {
    fn default() -> World {
        let s = Settings::default();
        World {
            clock_speed: s.clock_speed,
            traffic: density_name(s.traffic).to_owned(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Audio {
    master: f32,
    engine: f32,
    world: f32,
    mute: bool,
}

impl Default for Audio {
    fn default() -> Audio {
        let s = Settings::default();
        Audio {
            master: s.master_volume,
            engine: s.engine_volume,
            world: s.world_volume,
            mute: s.mute,
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
            master_volume: fix(self.master_volume, 0.0, 1.0, d.master_volume),
            engine_volume: fix(self.engine_volume, 0.0, 1.0, d.engine_volume),
            world_volume: fix(self.world_volume, 0.0, 1.0, d.world_volume),
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
            "drive": DriveSection {
                handling: self.handling.name().to_owned(),
                vehicle: self.vehicle.map(str::to_owned),
            },
            "camera": CameraSection::of(self),
            "video": Video {
                ui_scale: self.ui_scale,
                resolution_scale: self.resolution_scale,
                bloom: self.bloom,
                debug_overlay: self.debug_overlay,
                quality: quality_name(self.quality).to_owned(),
            },
            "hud": HudSection {
                minimap: self.minimap,
                minimap_rotate: self.minimap_rotate,
                minimap_range_m: self.minimap_range_m,
            },
            "world": World {
                clock_speed: self.clock_speed,
                traffic: density_name(self.traffic).to_owned(),
            },
            "audio": Audio {
                master: self.master_volume,
                engine: self.engine_volume,
                world: self.world_volume,
                mute: self.mute,
            },
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
        let drive: DriveSection =
            serde_json::from_value(section(&mut root, "drive")).unwrap_or_default();
        let camera: CameraSection =
            serde_json::from_value(section(&mut root, "camera")).unwrap_or_default();
        let video: Video = serde_json::from_value(section(&mut root, "video")).unwrap_or_default();
        let hud: HudSection = serde_json::from_value(section(&mut root, "hud")).unwrap_or_default();
        let world: World = serde_json::from_value(section(&mut root, "world")).unwrap_or_default();
        let audio: Audio = serde_json::from_value(section(&mut root, "audio")).unwrap_or_default();
        let radio: RadioSettings =
            serde_json::from_value(section(&mut root, "radio")).unwrap_or_default();
        let bindings = match section(&mut root, "input") {
            v @ Value::Object(_) => Bindings::from_json(&v.to_string()).ok(),
            _ => None,
        };
        let d = Settings::default();
        let settings = Settings {
            handling: Handling::from_name(&drive.handling).unwrap_or(d.handling),
            vehicle: drive.vehicle.as_deref().and_then(builtin_vehicle),
            camera: CameraMode::from_name(&camera.mode).unwrap_or(d.camera),
            cockpit_fov_deg: camera.cockpit_fov_deg,
            seat_height_cm: camera.seat_height_cm,
            seat_forward_cm: camera.seat_forward_cm,
            dashboard: camera.dashboard,
            camera_shake: camera.shake,
            ui_scale: video.ui_scale,
            resolution_scale: video.resolution_scale,
            bloom: video.bloom,
            quality: Quality::from_name(&video.quality),
            debug_overlay: video.debug_overlay,
            clock_speed: world.clock_speed,
            traffic: density_from_name(&world.traffic).unwrap_or(d.traffic),
            minimap: hud.minimap,
            minimap_rotate: hud.minimap_rotate,
            minimap_range_m: hud.minimap_range_m,
            master_volume: audio.master,
            engine_volume: audio.engine,
            world_volume: audio.world,
            mute: audio.mute,
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
            handling: Handling::Off,
            vehicle: Some("palisade_wagon"),
            camera: CameraMode::Cockpit,
            cockpit_fov_deg: 71.0,
            seat_height_cm: 4.0,
            seat_forward_cm: -3.0,
            dashboard: false,
            camera_shake: false,
            ui_scale: 1.75,
            resolution_scale: 0.75,
            bloom: false,
            quality: Some(Quality::Low),
            debug_overlay: true,
            clock_speed: 60.0,
            traffic: traffic::Density::Busy,
            minimap: false,
            minimap_rotate: false,
            minimap_range_m: 250.0,
            master_volume: 0.5,
            engine_volume: 0.25,
            world_volume: 1.0,
            mute: true,
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
                 "hud": { "minimap_rotate": false, "minimap_range_m": 120 },
                 "drive": { "handling": "light" },
                 "camera": { "mode": "orbit", "cockpit_fov_deg": 120, "seat_height_cm": -9,
                             "seat_forward_cm": 4 },

                 "audio": { "master": 3, "engine": -1, "mute": true } }"#,
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
                master_volume: 1.0,
                engine_volume: 0.0,
                mute: true,
                ..Settings::default()
            }
        );
        assert_eq!(b, None);
        let (s, _) = Settings::from_json(r#"{ "video": { "ui_scale": "big" } }"#);
        assert_eq!(s, Settings::default());
        let (s, _) = Settings::from_json(r#"{ "world": { "traffic": "jam" } }"#);
        assert_eq!(
            s.traffic,
            traffic::Density::Normal,
            "unknown density: the default"
        );
        let (s, _) = Settings::from_json(r#"{ "world": { "traffic": "off" } }"#);
        assert_eq!(s.traffic, traffic::Density::Off);
        let (s, _) = Settings::from_json(
            r#"{ "drive": { "handling": "rails" }, "camera": { "mode": "drone" } }"#,
        );
        assert_eq!(s, Settings::default());
        // An unknown car is forgotten; a missing one is none.
        let (s, _) = Settings::from_json(r#"{ "drive": { "vehicle": "hovercar" } }"#);
        assert_eq!(s.vehicle, None);
        let (s, _) = Settings::from_json(r#"{ "drive": { "vehicle": "palisade_wagon" } }"#);
        assert_eq!(s.vehicle, Some("palisade_wagon"));
        assert!(!Settings::default()
            .to_json(&Bindings::default())
            .contains("vehicle"));
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
    fn quality_presets_round_trip_and_map_to_the_renderer() {
        let b = Bindings::default();
        for q in Quality::ALL.map(Some).into_iter().chain([None]) {
            let s = Settings {
                quality: q,
                ..Settings::default()
            };
            assert_eq!(Settings::from_json(&s.to_json(&b)).0.quality, q);
        }
        let (s, _) = Settings::from_json(r#"{ "video": { "quality": "potato" } }"#);
        assert_eq!(s.quality, None, "unknown names are auto");
        // Cheaper presets never ask for more than dearer ones.
        let r = Quality::ALL.map(|q| q.render(q.resolution_scale()));
        for w in r.windows(2) {
            assert!(w[0].shadow_size <= w[1].shadow_size);
            assert!(w[0].cascades <= w[1].cascades);
            assert!(w[0].bloom_levels <= w[1].bloom_levels);
            assert!(w[0].resolution_scale <= w[1].resolution_scale);
        }
        assert_eq!(Quality::High.render(1.0), render::RenderQuality::default());
        assert_eq!(r[0].cascades, 2);
        assert_eq!(r[0].resolution_scale, 0.75);
        assert_eq!(r[2].shadow_size, 1536);
    }

    #[test]
    fn auto_quality_picks_deck_on_the_deck() {
        let gpu = |name: &str, integrated, linux, surface| GpuInfo {
            name: name.to_owned(),
            integrated,
            linux,
            surface,
            ..GpuInfo::default()
        };
        let deck = [
            gpu(
                "AMD Custom GPU 0405 (RADV VANGOGH)",
                true,
                true,
                (1280, 800),
            ),
            gpu(
                "AMD Custom GPU 0932 (RADV SEPHIROTH)",
                true,
                true,
                (1920, 1080),
            ),
            gpu(
                "AMD Radeon Graphics (RADV GFX1103_R1)",
                true,
                true,
                (2560, 1440),
            ),
            gpu("llvmpipe (LLVM 17.0.6, 256 bits)", false, true, (1280, 800)),
        ];
        for g in &deck {
            assert_eq!(Quality::auto(g), Quality::Deck, "{g:?}");
        }
        let other = [
            gpu(
                "AMD Radeon RX 7800 XT (RADV NAVI32)",
                false,
                true,
                (2560, 1440),
            ),
            gpu("Apple M2", true, false, (2880, 1800)),
            gpu("Intel(R) Iris(R) Xe Graphics", true, false, (1280, 800)),
            gpu("Google SwiftShader", false, true, (1280, 720)),
        ];
        for g in &other {
            assert_eq!(Quality::auto(g), Quality::Medium, "{g:?}");
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
