//! Scenarios: the worlds the World page offers (`docs/SCENARIOS.md`), each a kind of place,
//! an era with its cars, a radio preset and a light, read from `assets/scenarios/*.ron`
//! (format below), in the fixed order of [`BUILTIN`]; and the era filter that decides which
//! cars a world shows and how often ([`Lineup`]).
//!
//! ```text
//! Scenario(
//!   id: "socal_beach_1984", title: "Pacific Shore", year: 1984, kind: "Beach town",
//!   region: "california_coast",          // assets/regions/<id>.ron
//!   grammar: "california_coast_80s.ron", // assets/grammar/<file>
//!   radio: "1984_socal",                 // assets/radio/<id>.ron
//!   palette: "socal_1985",               // a world::Palette
//!   default_time: (17, 40),              // the clock a world starts at, HH, MM
//!   default_vehicle: "corsair_gtx",      // the car it starts in unless one was picked
//!   years: (1979, 1987),                 // the era: cars of these years are everyday cars
//!   heroes: [],                          // cars rare by age but kept on the road
//!   stub: false,                         // optional: a world still being built ("coming")
//! )
//! ```
//!
//! The era filter: a car newer than `years.1` never appears in the world, on the road,
//! parked or on the Vehicle page. A car of `years` weighs 1; an older one
//! `0.5^((years.1 - year) / 8)`, at least 0.05 for a hero. Weights are integers in
//! thousandths ([`era_weight`]), computed without floating point, so every platform picks
//! the same cars; a pick between cars of equal weight is the same pick as before weights
//! existed ([`reduced`]).

use std::sync::OnceLock;

use serde::Deserialize;

use crate::vehicles;

/// One world (see the module docs for the fields).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub title: String,
    pub year: u16,
    /// What kind of place, for the World page ("Beach town").
    pub kind: String,
    pub region: String,
    pub grammar: String,
    pub radio: String,
    pub palette: String,
    pub default_time: (u8, u8),
    pub default_vehicle: String,
    pub years: (u16, u16),
    pub heroes: Vec<String>,
    /// Still being built: the World page marks it "coming" (it can be picked).
    #[serde(default)]
    pub stub: bool,
}

/// The scenarios compiled into the game, as `(id, RON text)`, in the order the World page
/// lists them. The first is the world a scene is in unless it names another.
pub const BUILTIN: [(&str, &str); 2] = [
    (
        "socal_beach_1984",
        include_str!("../../../assets/scenarios/socal_beach_1984.ron"),
    ),
    (
        "la_basin_2001",
        include_str!("../../../assets/scenarios/la_basin_2001.ron"),
    ),
];

impl Scenario {
    /// Checks the fields against each other and against what the game has built in.
    pub fn validate(&self) -> Result<(), String> {
        let err = |what: &str| Err(format!("scenario `{}`: {what}", self.id));
        if self.id.is_empty() || self.title.is_empty() || self.kind.is_empty() {
            return err("empty id, title or kind");
        }
        let (first, last) = self.years;
        if first > last || !(vehicle::YEARS.0..=vehicle::YEARS.1).contains(&first) {
            return err("bad years");
        }
        if !(first..=last).contains(&self.year) {
            return err("its year is outside its years");
        }
        if self.default_time.0 > 23 || self.default_time.1 > 59 {
            return err("bad default_time");
        }
        if crate::places::region_text(&self.region).is_none() {
            return err("unknown region");
        }
        if crate::places::grammar_text(&self.grammar).is_none() {
            return err("unknown grammar");
        }
        if crate::places::palette(&self.palette).is_none() {
            return err("unknown palette");
        }
        if !crate::radio::PRESETS
            .iter()
            .any(|(id, _)| *id == self.radio)
        {
            return err("unknown radio preset");
        }
        let year_of = |id: &str| vehicles::builtin_spec(id).ok().map(|s| s.year);
        match year_of(&self.default_vehicle) {
            Some(y) if y <= last => {}
            _ => return err("default_vehicle is not a built-in car of its years"),
        }
        if self.heroes.iter().any(|h| year_of(h).is_none()) {
            return err("a hero is not a built-in car");
        }
        Ok(())
    }

    /// Seconds of day of [`Scenario::default_time`].
    pub fn default_time_s(&self) -> f64 {
        f64::from(u32::from(self.default_time.0) * 3600 + u32::from(self.default_time.1) * 60)
    }
}

/// Parses a RON `Scenario` and validates it.
pub fn load(text: &str) -> Result<Scenario, String> {
    let s: Scenario = ron::from_str(text).map_err(|e| format!("scenario: {e}"))?;
    s.validate()?;
    Ok(s)
}

/// Every built-in scenario, in [`BUILTIN`] order, parsed once. They are built in and their
/// tests parse them, so a failure here is a build that should not have shipped.
pub fn all() -> &'static [Scenario] {
    static ALL: OnceLock<Vec<Scenario>> = OnceLock::new();
    ALL.get_or_init(|| {
        BUILTIN
            .iter()
            .map(|(_, text)| load(text).expect("the built-in scenarios"))
            .collect()
    })
}

/// Index in [`BUILTIN`] of the scenario `id`.
pub fn index_of(id: &str) -> Option<usize> {
    BUILTIN.iter().position(|(s, _)| *s == id)
}

/// The scenario at `index` (the last one past the end).
pub fn get(index: usize) -> &'static Scenario {
    let all = all();
    &all[index.min(all.len() - 1)]
}

/// Weight of a car within its scenario's years.
pub const WEIGHT_ONE: u32 = 1000;
/// Least weight of a hero car (0.05).
pub const HERO_FLOOR: u32 = 50;
/// `0.5^(k / 8)` in thousandths, `k` from 0 to 7.
const EIGHTHS: [u32; 8] = [1000, 917, 841, 771, 707, 648, 595, 545];

/// The pick weight of a car of model `year` in a world of `years`, in thousandths: 0 when
/// it is newer than the world, [`WEIGHT_ONE`] within its years, else halving every eight
/// years older than its last year, at least [`HERO_FLOOR`] for a `hero` and 1 for any.
pub fn era_weight(year: u16, years: (u16, u16), hero: bool) -> u32 {
    if year > years.1 {
        return 0;
    }
    if year >= years.0 {
        return WEIGHT_ONE;
    }
    let age = u32::from(years.1 - year);
    let w = EIGHTHS[(age % 8) as usize]
        .checked_shr(age / 8)
        .unwrap_or(0);
    let floor = if hero { HERO_FLOOR } else { 1 };
    w.max(floor)
}

/// Divides `weights` by their greatest common divisor, so equal weights become ones: a pick
/// by them draws the same numbers as a pick among the same cars before weights existed.
pub fn reduced(weights: &mut [u32]) {
    let g = weights.iter().fold(0, |g, &w| gcd(g, w));
    if g > 1 {
        for w in weights {
            *w /= g;
        }
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The cars a world shows: the roster indices it allows, in roster order, their names (for
/// the Vehicle page's chooser) and every roster car's [`era_weight`] (0 for the ones it
/// does not allow).
#[derive(Clone, Debug, PartialEq)]
pub struct Lineup {
    pub cars: Vec<usize>,
    pub names: Vec<&'static str>,
    pub weights: Vec<u32>,
}

impl Lineup {
    /// The lineup of `s` from a roster of `(id, name, year)`.
    pub fn new(s: &Scenario, roster: &[(&str, &'static str, u16)]) -> Lineup {
        let weights: Vec<u32> = roster
            .iter()
            .map(|&(id, _, year)| era_weight(year, s.years, s.heroes.iter().any(|h| h == id)))
            .collect();
        let cars: Vec<usize> = (0..roster.len()).filter(|&i| weights[i] > 0).collect();
        Lineup {
            names: cars.iter().map(|&i| roster[i].1).collect(),
            cars,
            weights,
        }
    }

    /// Whether roster car `index` appears in this world.
    pub fn allows(&self, index: usize) -> bool {
        self.weights.get(index).is_some_and(|&w| w > 0)
    }

    /// Where roster car `index` is in [`Lineup::cars`].
    pub fn position(&self, index: usize) -> Option<usize> {
        self.cars.iter().position(|&c| c == index)
    }

    /// The car after roster car `index` in this world, wrapping after the last.
    pub fn next_after(&self, index: usize) -> usize {
        self.cars
            .iter()
            .copied()
            .find(|&c| c > index)
            .or(self.cars.first().copied())
            .unwrap_or(index)
    }
}

/// The lineup of built-in scenario `index` over the built-in cars
/// ([`vehicles::BUILTIN`]), made once.
pub fn lineup(index: usize) -> &'static Lineup {
    static LINEUPS: OnceLock<Vec<Lineup>> = OnceLock::new();
    let all = LINEUPS.get_or_init(|| {
        let names = crate::ui::roster::names();
        let roster: Vec<(&str, &'static str, u16)> = vehicles::BUILTIN
            .iter()
            .enumerate()
            .map(|(i, &(id, _))| {
                let year = vehicles::builtin_spec(id).map_or(u16::MAX, |s| s.year);
                (id, names[i], year)
            })
            .collect();
        all().iter().map(|s| Lineup::new(s, &roster)).collect()
    });
    &all[index.min(all.len() - 1)]
}

/// Scenario `index`'s lineup of the built-in cars with car `id` re-dated to 2001: a 2001 car
/// in the place of a built-in one, for tests of what a world never shows.
#[cfg(test)]
pub(crate) fn lineup_with_2001_car(index: usize, id: &str) -> Lineup {
    let roster: Vec<(&str, &'static str, u16)> = vehicles::BUILTIN
        .iter()
        .map(|&(car, _)| {
            let year = vehicles::builtin_spec(car).unwrap().year;
            (car, car, if car == id { 2001 } else { year })
        })
        .collect();
    Lineup::new(get(index), &roster)
}

/// Width and height of a scenario's still, texels.
pub const STILL_SIZE: (u32, u32) = (320, 200);

/// Each scenario's still for the World page (`assets/stills/<id>.png`, rendered with
/// `tools/screenshot`), in [`BUILTIN`] order.
const STILLS: [(&str, &[u8]); 2] = [
    (
        "socal_beach_1984",
        include_bytes!("../../../assets/stills/socal_beach_1984.png"),
    ),
    (
        "la_basin_2001",
        include_bytes!("../../../assets/stills/la_basin_2001.png"),
    ),
];

/// Every scenario's still side by side in [`BUILTIN`] order (still `i` spans u from
/// `i / n` to `(i + 1) / n`), decoded once, for `render::Renderer::upload_stills`. A still
/// that does not decode (never a built-in one, which its test decodes) is grey.
pub fn stills() -> &'static crate::png::Image {
    static STILLS_IMAGE: OnceLock<crate::png::Image> = OnceLock::new();
    STILLS_IMAGE.get_or_init(|| {
        let (w, h) = STILL_SIZE;
        let n = BUILTIN.len() as u32;
        let mut rgba = vec![96u8; (w * n * h * 4) as usize];
        for (k, (id, _)) in BUILTIN.iter().enumerate() {
            let still = STILLS
                .iter()
                .find(|s| s.0 == *id)
                .and_then(|s| crate::png::decode(s.1).ok())
                .filter(|img| (img.w, img.h) == STILL_SIZE);
            let Some(still) = still else { continue };
            for y in 0..h as usize {
                let row = (w * 4) as usize;
                let to = (y * n as usize + k) * row;
                rgba[to..to + row].copy_from_slice(&still.rgba[y * row..(y + 1) * row]);
            }
        }
        crate::png::Image { w: w * n, h, rgba }
    })
}

/// Stations on each built-in scenario's radio preset, counted once.
pub fn station_counts() -> &'static [usize] {
    static COUNTS: OnceLock<Vec<usize>> = OnceLock::new();
    COUNTS.get_or_init(|| all().iter().map(station_count).collect())
}

/// Stations on scenario `s`'s radio preset.
pub fn station_count(s: &Scenario) -> usize {
    crate::radio::PRESETS
        .iter()
        .find(|(id, _)| *id == s.radio)
        .and_then(|(_, text)| audio::RadioPreset::parse(text).ok())
        .map_or(0, |p| p.stations.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A roster of the built-in cars plus a test-only 2001 car at the end.
    fn with_2001_car() -> Vec<(&'static str, &'static str, u16)> {
        let mut r: Vec<_> = vehicles::BUILTIN
            .iter()
            .map(|&(id, _)| (id, id, vehicles::builtin_spec(id).unwrap().year))
            .collect();
        r.push(("test_tuner_2001", "Test Tuner", 2001));
        r
    }

    #[test]
    fn the_scenario_files_parse_with_distinct_ids() {
        let all = all();
        assert_eq!(all.len(), BUILTIN.len());
        for (k, s) in all.iter().enumerate() {
            assert_eq!(s.id, BUILTIN[k].0);
            assert_eq!(index_of(&s.id), Some(k));
            assert!(all[k + 1..]
                .iter()
                .all(|o| o.id != s.id && o.title != s.title));
        }
        let coast = &all[0];
        assert_eq!(coast.id, "socal_beach_1984");
        assert_eq!((coast.years, coast.stub), ((1979, 1987), false));
        assert_eq!(coast.default_time_s(), crate::lighting::DEFAULT_TIME_S);
        assert_eq!(coast.default_vehicle, crate::DEFAULT_VEHICLE);
        let basin = &all[1];
        assert_eq!(
            (basin.id.as_str(), basin.years, basin.stub),
            ("la_basin_2001", (1965, 2001), true)
        );
        assert_eq!(index_of("atlantis"), None);
        assert_eq!(get(99).id, basin.id);
    }

    #[test]
    fn broken_scenarios_are_refused() {
        let text = BUILTIN[0].1;
        assert!(load("Scenario(").is_err());
        assert!(
            load(&text.replace("kind:", "knd:")).is_err(),
            "unknown fields"
        );
        for (from, to) in [
            ("(1979, 1987)", "(1987, 1979)"),
            ("year: 1984", "year: 1990"),
            ("(17, 40)", "(24, 0)"),
            ("\"california_coast\"", "\"atlantis\""),
            ("\"california_coast_80s.ron\"", "\"atlantis.ron\""),
            ("\"socal_1985\"", "\"atlantis\""),
            ("\"1984_socal\"", "\"atlantis\""),
            ("\"corsair_gtx\"", "\"hovercar\""),
            ("heroes: []", "heroes: [\"hovercar\"]"),
        ] {
            let broken = text.replace(from, to);
            assert_ne!(broken, text, "{from}");
            assert!(load(&broken).is_err(), "{to}");
        }
    }

    #[test]
    fn weights_fall_off_with_age() {
        let y = (1995, 2001);
        assert_eq!(era_weight(2002, y, false), 0, "newer than the world");
        assert_eq!(era_weight(2002, y, true), 0, "even a hero");
        assert_eq!(era_weight(2001, y, false), WEIGHT_ONE);
        assert_eq!(era_weight(1995, y, false), WEIGHT_ONE);
        // 1993: eight years older than 2001, half.
        assert_eq!(era_weight(1993, y, false), 500);
        assert_eq!(era_weight(1985, y, false), 250);
        // 1984: 17 years, 0.5^(17/8) = 0.229.
        assert_eq!(era_weight(1984, y, false), 229);
        // 1969: 32 years, 1/16; a hero keeps its floor beyond.
        assert_eq!(era_weight(1969, y, false), 62);
        assert_eq!(era_weight(1969, y, true), 62);
        assert_eq!(era_weight(1950, y, false), 12);
        assert_eq!(era_weight(1950, y, true), HERO_FLOOR);
        assert_eq!(era_weight(1900, y, false), 1, "never quite gone");
        // The same in floating point, for the docs' formula.
        for year in 1900..1995u16 {
            let f = 0.5f64.powf(f64::from(2001 - year) / 8.0) * 1000.0;
            let w = era_weight(year, y, false);
            assert!(
                (f64::from(w) - f).abs() <= 1.0 || w == 1,
                "{year}: {w} vs {f}"
            );
        }
        let mut w = [WEIGHT_ONE, WEIGHT_ONE, 0, WEIGHT_ONE];
        reduced(&mut w);
        assert_eq!(w, [1, 1, 0, 1]);
        let mut w = [3000, 2000, 500];
        reduced(&mut w);
        assert_eq!(w, [6, 4, 1]);
        let mut w = [0, 0];
        reduced(&mut w);
        assert_eq!(w, [0, 0]);
    }

    #[test]
    fn the_1984_lineup_is_the_whole_80s_roster_at_weight_one() {
        let l = lineup(0);
        assert_eq!(l.cars, (0..vehicles::BUILTIN.len()).collect::<Vec<_>>());
        assert_eq!(l.names, crate::ui::roster::names());
        assert!(l.weights.iter().all(|&w| w == WEIGHT_ONE));
        // Every built-in car is a 1979-1987 car.
        for (id, _) in vehicles::BUILTIN {
            let y = vehicles::builtin_spec(id).unwrap().year;
            assert!((1979..=1987).contains(&y), "{id}: {y}");
        }
        // The 2001 world has all of them too, by its stub's years.
        assert_eq!(lineup(1).cars, l.cars);
    }

    #[test]
    fn a_2001_car_never_lists_in_1984() {
        let roster = with_2001_car();
        let last = roster.len() - 1;
        let l84 = Lineup::new(get(0), &roster);
        assert!(!l84.allows(last));
        assert_eq!(l84.position(last), None);
        assert!(!l84.names.contains(&"Test Tuner"));
        assert_eq!(l84.weights[last], 0);
        // Adding it changes nothing else in 1984.
        assert_eq!(l84.cars, lineup(0).cars);
        assert_eq!(&l84.weights[..last], &lineup(0).weights[..]);
        let mut next = 0;
        for _ in 0..2 * roster.len() {
            next = l84.next_after(next);
            assert_ne!(next, last);
        }
        let l01 = Lineup::new(get(1), &roster);
        assert!(l01.allows(last));
        assert_eq!(l01.names.last(), Some(&"Test Tuner"));
        assert_eq!(l01.next_after(last - 1), last);
        assert_eq!(l01.next_after(last), 0, "wraps");
    }

    #[test]
    fn every_scenario_has_its_still() {
        assert_eq!(STILLS.len(), BUILTIN.len());
        for (k, (id, png)) in STILLS.iter().enumerate() {
            assert_eq!(*id, BUILTIN[k].0);
            assert!(png.len() < 60 * 1024, "{id}: {} bytes", png.len());
            let img = crate::png::decode(png).unwrap();
            assert_eq!((img.w, img.h), STILL_SIZE, "{id}");
            // Packed side by side: the still's middle texel is where its slot is.
            let all = stills();
            let (w, h) = STILL_SIZE;
            let (x, y) = (w / 2, h / 2);
            let at = ((y * all.w + k as u32 * w + x) * 4) as usize;
            let own = ((y * w + x) * 4) as usize;
            assert_eq!(all.rgba[at..at + 4], img.rgba[own..own + 4], "{id}");
        }
        assert_eq!((stills().w, stills().h), (STILL_SIZE.0 * 2, STILL_SIZE.1));
    }

    #[test]
    fn the_stills_decode_as_an_image_library_reads_them() {
        // `h = h * 31 + byte` mod 2^61 over the RGBA bytes, as Pillow decodes the files.
        let expected = [2_138_132_481_716_855_108u128, 2_229_843_925_750_921_464];
        for ((id, png), want) in STILLS.iter().zip(expected) {
            let img = crate::png::decode(png).unwrap();
            let h = img
                .rgba
                .iter()
                .fold(0u128, |h, &b| (h * 31 + u128::from(b)) % (1 << 61));
            assert_eq!(h, want, "{id}");
        }
    }

    #[test]
    fn station_counts_come_from_the_preset() {
        assert_eq!(station_counts()[0], station_count(get(0)));
        assert!(station_count(get(0)) > 0);
        assert_eq!(
            station_count(get(1)),
            station_count(get(0)),
            "the stub's radio"
        );
    }
}
