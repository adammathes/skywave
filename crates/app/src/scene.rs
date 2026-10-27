//! A fully specified starting scene: which town seed, which car where, which camera, what
//! time and whether the headlights are on. Fixed screenshot scenes
//! (`tools/screenshot/scenes.json`) are written as these; the web build reads them from the
//! URL query (`?t=17:40&cam=cockpit`), the desktop build from `--scene key=value` flags.
//!
//! Keys (every one optional):
//! - `seed=<u64>`: seed of the place (default 1): the test town's detail, or the whole
//!   procedural place.
//! - `place=<name>`: `town` (the hand-laid test town, the default) or `coast` (the
//!   procedural place of the scenario's region template for the seed).
//! - `scenario=<id>`: the world (`crate::scenario`, `assets/scenarios/<id>.ron`) and its
//!   procedural place (`place=coast`; a later `place=town` puts its cars and light in the
//!   test town). An id that is not built in is the first scenario, and a toast says so.
//!   Default: the first scenario, or the world last picked on the World page when the
//!   scene names neither a place nor a scenario ([`SceneSetup::with_saved_world`]).
//! - `car=<id>`: built-in vehicle id (default: the vehicle setting, the car last picked,
//!   else [`DEFAULT_VEHICLE`]).
//! - `paint=<n>`: the car's palette entry, counting from 0 (default: 0 with `car=`, else the
//!   paint saved with the vehicle setting). With `car=`, past the end of its palette is an
//!   error; without, it paints the car the game starts in (the saved one, else the default),
//!   its first paint when that car has fewer. Not saved.
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
//! - `traffic=off|low|normal|busy`: NPC traffic and pedestrian density for this session,
//!   over the saved setting (default: the setting, but `off` in a `still=1` scene, so the
//!   screenshot goldens draw no traffic unless they ask). Traffic runs a while before the
//!   first frame (`app::npc::WARMUP_S`, people longer), so even a still scene has cars on
//!   the streets and people on the walks.

use crate::camera::CameraMode;
use crate::drive::DEFAULT_VEHICLE;

/// Which place a scene is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaceKey {
    /// The hand-laid test town (`place::Place::test_grid`).
    #[default]
    TestTown,
    /// The procedural place of the scene's seed and its scenario's region template.
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
    /// The scene named the place (`place=`) or the world (`scenario=`).
    pub place_given: bool,
    /// The world: an index into [`crate::scenario::BUILTIN`].
    pub scenario: usize,
    /// `scenario=` named a world that is not built in (the scene is in the first).
    pub unknown_scenario: bool,
    pub car: String,
    /// The scene named the car (`car=`): the saved vehicle setting does not replace it.
    pub car_given: bool,
    /// The car's palette entry, if the scene gave one.
    pub paint: Option<usize>,
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
    /// Traffic density over the setting, or `None` for the setting.
    pub traffic: Option<traffic::Density>,
}

impl Default for SceneSetup {
    fn default() -> SceneSetup {
        SceneSetup {
            seed: 1,
            place: PlaceKey::TestTown,
            place_given: false,
            scenario: 0,
            unknown_scenario: false,
            car: DEFAULT_VEHICLE.to_string(),
            car_given: false,
            paint: None,
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
            traffic: None,
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

    /// The traffic density the scene asks for: its `traffic=`, else off in a still scene,
    /// else `None` (the setting).
    pub fn traffic_density(&self) -> Option<traffic::Density> {
        self.traffic.or(self.still.then_some(traffic::Density::Off))
    }

    /// The keys [`SceneSetup::set`] takes.
    pub const KEYS: [&'static str; 16] = [
        "seed", "place", "scenario", "car", "paint", "cam", "t", "pos", "yaw", "lights", "fly",
        "still", "map", "ui", "stream", "traffic",
    ];

    /// World `scenario` and its procedural place; its own car unless the scene named one.
    fn set_world(&mut self, scenario: usize) {
        self.scenario = scenario;
        self.place = PlaceKey::Coast;
        if !self.car_given {
            self.car = crate::scenario::get(scenario).default_vehicle.clone();
        }
    }

    /// The scene of the same seed, camera, light and session options in world `scenario`:
    /// its procedural place, its start, its default time and the car the settings pick
    /// for it (what picking a card on the World page loads).
    pub fn for_world(&self, scenario: usize) -> SceneSetup {
        let mut setup = SceneSetup {
            place_given: true,
            unknown_scenario: false,
            car_given: false,
            paint: None,
            time_s: None,
            pos: None,
            heading_rad: None,
            headlights: false,
            fly: false,
            map: false,
            ..self.clone()
        };
        setup.set_world(scenario);
        setup
    }

    /// Unless the scene names a place or a world, the world last picked on the World page
    /// (`settings_json`, the saved settings document), if any.
    pub fn with_saved_world(mut self, settings_json: Option<&str>) -> SceneSetup {
        if self.place_given {
            return self;
        }
        let saved = settings_json.and_then(|t| crate::settings::Settings::from_json(t).0.scenario);
        if let Some(i) = saved.and_then(crate::scenario::index_of) {
            self.set_world(i);
        }
        self
    }

    /// Sets one key (see the module docs).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let bad = || format!("bad scene value {key}={value}");
        match key {
            "seed" => self.seed = value.parse().map_err(|_| bad())?,
            "place" => {
                self.place = PlaceKey::from_name(value).ok_or_else(bad)?;
                self.place_given = true;
            }
            "scenario" => {
                let index = crate::scenario::index_of(value);
                self.set_world(index.unwrap_or(0));
                self.unknown_scenario = index.is_none();
                self.place_given = true;
            }
            "car" => {
                self.car = crate::vehicles::current_id(value).to_string();
                self.car_given = true;
            }
            "paint" => self.paint = Some(value.parse().map_err(|_| bad())?),
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
            "traffic" => self.traffic = Some(crate::npc::density_from_name(value).ok_or_else(bad)?),
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
            "?seed=7&place=coast&car=homestead_wagon&paint=2&cam=cockpit&t=22:30&pos=-200.5,97&yaw=180&lights=1&fly=0&still=1&map=1&ui=1.5&stream=0&traffic=busy",
        )
        .unwrap();
        assert!(s.map && !s.stream);
        assert_eq!(s.traffic, Some(traffic::Density::Busy));
        assert_eq!(s.traffic_density(), Some(traffic::Density::Busy));
        assert_eq!(SceneSetup::default().traffic_density(), None, "the setting");
        let still = SceneSetup::from_query("still=1").unwrap();
        assert_eq!(still.traffic_density(), Some(traffic::Density::Off));
        assert!(SceneSetup::default().stream, "streamed by default");
        assert_eq!(s.ui_scale, Some(1.5));
        assert_eq!(s.seed, 7);
        assert_eq!(s.place, PlaceKey::Coast);
        assert_eq!(s.car, "homestead_wagon");
        assert!(s.car_given && !SceneSetup::default().car_given);
        assert_eq!((s.paint, SceneSetup::default().paint), (Some(2), None));
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
        assert!(s.place_given && !SceneSetup::default().place_given);
        assert_eq!(s.scenario, 0);
        // An old URL's renamed car is the new one.
        let s = SceneSetup::from_query("?car=palisade_wagon").unwrap();
        assert_eq!(s.car, "homestead_wagon");
    }

    #[test]
    fn scenario_names_the_world_and_its_place() {
        let s = SceneSetup::from_query("?scenario=la_basin_2001&seed=3").unwrap();
        assert_eq!(
            (s.scenario, s.place, s.place_given),
            (1, PlaceKey::Coast, true)
        );
        assert!(!s.unknown_scenario);
        let s = SceneSetup::from_query("?scenario=atlantis").unwrap();
        assert_eq!(
            (s.scenario, s.place, s.unknown_scenario),
            (0, PlaceKey::Coast, true)
        );
        let s = SceneSetup::from_query("?scenario=la_basin_2001&place=town").unwrap();
        assert_eq!((s.scenario, s.place), (1, PlaceKey::TestTown));
        // The world picked last, unless the scene names one or a place.
        let picked = crate::settings::Settings {
            scenario: Some("la_basin_2001"),
            ..Default::default()
        };
        let json = picked.to_json(&crate::input::Bindings::default());
        let s = SceneSetup::default().with_saved_world(Some(&json));
        assert_eq!((s.scenario, s.place), (1, PlaceKey::Coast));
        let s = SceneSetup::from_query("place=town").unwrap();
        assert_eq!(s.clone().with_saved_world(Some(&json)), s);
        assert_eq!(
            SceneSetup::default().with_saved_world(None),
            SceneSetup::default()
        );
        assert_eq!(
            SceneSetup::default().with_saved_world(Some("{}")),
            SceneSetup::default()
        );
        // A world's scene keeps the session's options and drops the spot and the car.
        let base =
            SceneSetup::from_query("seed=9&car=pipit_hatch&t=12:00&pos=1,2&cam=cockpit&ui=2")
                .unwrap();
        let w = base.for_world(1);
        assert_eq!((w.seed, w.scenario, w.place), (9, 1, PlaceKey::Coast));
        assert_eq!((w.camera, w.ui_scale), (base.camera, base.ui_scale));
        assert!(!w.car_given && w.time_s.is_none() && w.pos.is_none());
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
            "traffic=jam",
            "place=moon",
        ] {
            assert!(SceneSetup::from_query(q).is_err(), "{q}");
        }
        assert!(SceneSetup::default().set("zoom", "2").is_err());
        assert_eq!(parse_clock("00:00"), Some(0.0));
        assert_eq!(parse_clock("23:59"), Some(86_340.0));
    }
}
