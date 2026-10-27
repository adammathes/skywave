//! A fully specified starting scene: which town seed, which car where, which camera, what
//! time and whether the headlights are on. Fixed screenshot scenes
//! (`tools/screenshot/scenes.json`) are written as these; the web build reads them from the
//! URL query (`?t=17:40&cam=cockpit`), the desktop build from `--scene key=value` flags.
//!
//! Keys (every one optional):
//! - `seed=<u64>`: seed of the place (default 1): the test town's detail, or the whole
//!   procedural place.
//! - `place=<name>`: `town` (the hand-laid test town, the default) or `coast` (the
//!   procedural `california_coast` place for the seed).
//! - `car=<id>`: built-in vehicle id (default: the vehicle setting, the car last picked,
//!   else [`DEFAULT_VEHICLE`]).
//! - `paint=<n>`: the car's palette entry, counting from 0 (default 0); past the end of
//!   its palette is an error. Without `car=`, a saved vehicle setting replaces both.
//! - `cam=<name>`: camera, a [`CameraMode::name`] or `chase` (default: the camera setting,
//!   which starts at `chase-mid` and remembers the last one picked).
//! - `t=HH:MM`: time of day; when given the clock is frozen there (default 17:40, running).
//! - `pos=x,z`: put the car on the ground at this point, m (default the place's
//!   `PlayerStart`: the start of the strip).
//! - `yaw=<deg>`: the car's compass heading, degrees clockwise from north (-z): 0 north,
//!   90 east, 180 south, 270 west (default the `PlayerStart`'s: east in the test town).
//! - `lights=1`: headlights on.
//! - `fly=1`: the free-fly debug camera.
//! - `still=1`: the simulation does not run, so every frame is the same picture of the
//!   scene at rest (for screenshot goldens; input is ignored).
//! - `map=1`: start with the full-screen map open.
//! - `ui=<scale>`: the HUD's UI scale for this session (1 to 2), over the saved setting.
//! - `stream=0`: build the whole place at load instead of streaming it round the camera
//!   (`stream=1`, the default; see `app::stream`). The screenshot goldens use it.

use crate::camera::CameraMode;
use crate::drive::DEFAULT_VEHICLE;

/// Which place a scene is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaceKey {
    /// The hand-laid test town (`place::Place::test_grid`).
    #[default]
    TestTown,
    /// The procedural `california_coast` place of the scene's seed.
    Coast,
}

impl PlaceKey {
    pub fn from_name(name: &str) -> Option<PlaceKey> {
        match name {
            "town" => Some(PlaceKey::TestTown),
            "coast" => Some(PlaceKey::Coast),
            _ => None,
        }
    }
}

/// A starting scene; `Default` is the normal start of the game.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSetup {
    pub seed: u64,
    pub place: PlaceKey,
    pub car: String,
    /// The scene named the car (`car=`): the saved vehicle setting does not replace it.
    pub car_given: bool,
    /// The car's palette entry.
    pub paint: usize,
    /// The camera, or `None` for the player's camera setting.
    pub camera: Option<CameraMode>,
    /// Seconds of day the clock is frozen at, or `None` for the default running clock.
    pub time_s: Option<f64>,
    /// Ground point `[x, z]` the car starts on, m; `None` for the place's `PlayerStart`.
    pub pos: Option<[f32; 2]>,
    /// Heading in the sim's convention: radians, 0 faces -z, positive turns left; `None`
    /// for the `PlayerStart`'s.
    pub heading_rad: Option<f32>,
    pub headlights: bool,
    pub fly: bool,
    /// Draw the scene without running the simulation.
    pub still: bool,
    /// Open the full map at the start.
    pub map: bool,
    /// UI scale for this session, overriding the setting.
    pub ui_scale: Option<f32>,
    /// Stream the place round the camera (else build it all at load).
    pub stream: bool,
}

impl Default for SceneSetup {
    fn default() -> SceneSetup {
        SceneSetup {
            seed: 1,
            place: PlaceKey::TestTown,
            car: DEFAULT_VEHICLE.to_string(),
            car_given: false,
            paint: 0,
            camera: None,
            time_s: None,
            pos: None,
            heading_rad: None,
            headlights: false,
            fly: false,
            still: false,
            map: false,
            ui_scale: None,
            stream: true,
        }
    }
}

impl SceneSetup {
    /// Parses a URL query string (`?t=22:30&lights=1`). Keys this module does not know are
    /// left to the platform and skipped; a known key with a bad value is an error.
    pub fn from_query(query: &str) -> Result<SceneSetup, String> {
        let mut setup = SceneSetup::default();
        for pair in query.trim_start_matches('?').split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            if SceneSetup::KEYS.contains(&key) {
                setup.set(key, value)?;
            }
        }
        Ok(setup)
    }

    /// The keys [`SceneSetup::set`] takes.
    pub const KEYS: [&'static str; 14] = [
        "seed", "place", "car", "paint", "cam", "t", "pos", "yaw", "lights", "fly", "still", "map",
        "ui", "stream",
    ];

    /// Sets one key (see the module docs).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let bad = || format!("bad scene value {key}={value}");
        match key {
            "seed" => self.seed = value.parse().map_err(|_| bad())?,
            "place" => self.place = PlaceKey::from_name(value).ok_or_else(bad)?,
            "car" => {
                self.car = value.to_string();
                self.car_given = true;
            }
            "paint" => self.paint = value.parse().map_err(|_| bad())?,
            "cam" => self.camera = Some(CameraMode::from_name(value).ok_or_else(bad)?),
            "t" => self.time_s = Some(parse_clock(value).ok_or_else(bad)?),
            "pos" => {
                let (x, z) = value.split_once(',').ok_or_else(bad)?;
                let x: f32 = x.trim().parse().map_err(|_| bad())?;
                let z: f32 = z.trim().parse().map_err(|_| bad())?;
                if !(x.is_finite() && z.is_finite()) {
                    return Err(bad());
                }
                self.pos = Some([x, z]);
            }
            "yaw" => {
                let deg: f32 = value.parse().map_err(|_| bad())?;
                if !deg.is_finite() {
                    return Err(bad());
                }
                self.heading_rad = Some(-deg.to_radians());
            }
            "lights" => self.headlights = parse_switch(value).ok_or_else(bad)?,
            "fly" => self.fly = parse_switch(value).ok_or_else(bad)?,
            "still" => self.still = parse_switch(value).ok_or_else(bad)?,
            "map" => self.map = parse_switch(value).ok_or_else(bad)?,
            "stream" => self.stream = parse_switch(value).ok_or_else(bad)?,
            "ui" => {
                let s: f32 = value.parse().map_err(|_| bad())?;
                if !(render::UI_SCALE_MIN..=render::UI_SCALE_MAX).contains(&s) {
                    return Err(bad());
                }
                self.ui_scale = Some(s);
            }
            _ => return Err(format!("unknown scene key `{key}`")),
        }
        Ok(())
    }
}

/// `HH:MM` (00:00 to 23:59) as seconds of day.
fn parse_clock(s: &str) -> Option<f64> {
    let (h, m) = s.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60 && s.len() == 5).then(|| f64::from(h * 3600 + m * 60))
}

fn parse_switch(s: &str) -> Option<bool> {
    match s {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_query_is_the_normal_start() {
        assert_eq!(SceneSetup::from_query(""), Ok(SceneSetup::default()));
        assert_eq!(SceneSetup::from_query("?"), Ok(SceneSetup::default()));
        assert_eq!(
            SceneSetup::from_query("?selftest=1&x"),
            Ok(SceneSetup::default())
        );
    }

    #[test]
    fn parses_every_key() {
        let s = SceneSetup::from_query(
            "?seed=7&place=coast&car=palisade_wagon&paint=2&cam=cockpit&t=22:30&pos=-200.5,97&yaw=180&lights=1&fly=0&still=1&map=1&ui=1.5&stream=0",
        )
        .unwrap();
        assert!(s.map && !s.stream);
        assert!(SceneSetup::default().stream, "streamed by default");
        assert_eq!(s.ui_scale, Some(1.5));
        assert_eq!(s.seed, 7);
        assert_eq!(s.place, PlaceKey::Coast);
        assert_eq!(s.car, "palisade_wagon");
        assert!(s.car_given && !SceneSetup::default().car_given);
        assert_eq!((s.paint, SceneSetup::default().paint), (2, 0));
        assert_eq!(s.camera, Some(CameraMode::Cockpit));
        assert_eq!(s.time_s, Some(22.0 * 3600.0 + 30.0 * 60.0));
        assert_eq!(s.pos, Some([-200.5, 97.0]));
        assert!((s.heading_rad.unwrap() + core::f32::consts::PI).abs() < 1e-6);
        assert!(s.headlights && !s.fly && s.still);
        // 90 is east, the test town's start heading.
        let mut east = SceneSetup::default();
        east.set("yaw", "90").unwrap();
        let start = crate::drive::START_HEADING_RAD;
        assert!((east.heading_rad.unwrap() - start).abs() < 1e-6);
        assert_eq!(SceneSetup::default().place, PlaceKey::TestTown);
    }

    #[test]
    fn rejects_bad_values() {
        for q in [
            "seed=x",
            "paint=-1",
            "paint=red",
            "cam=drone",
            "t=24:00",
            "t=7:30",
            "t=12:60",
            "t=noon",
            "pos=1",
            "pos=1,x",
            "pos=nan,0",
            "yaw=inf",
            "lights=2",
            "map=yes",
            "ui=3",
            "ui=big",
            "stream=2",
            "place=moon",
        ] {
            assert!(SceneSetup::from_query(q).is_err(), "{q}");
        }
        assert!(SceneSetup::default().set("zoom", "2").is_err());
        assert_eq!(parse_clock("00:00"), Some(0.0));
        assert_eq!(parse_clock("23:59"), Some(86_340.0));
    }
}
