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
//!   default_vehicle: "gannet_gt",      // the car it starts in unless one was picked
//!   years: (1979, 1987),                 // the era: cars of these years are everyday cars
//!   heroes: [],                          // cars rare by age but kept on the road
//!   stub: false,                         // optional: a world still being built ("coming")
//! )
//! ```
//!
//! The era filter: a car newer than `years.1` never appears in the world, on the road,
//! parked or on the Vehicle page. A car of `years` weighs 1; an older one
//! `0.5^((years.0 - year) / 8)`, at least 0.05 for a hero. Weights are kept per roster car
//! and per body shape ([`Lineup::shapes`], what traffic and parked cars pick from). Weights are integers in
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
        "inland_basin_2001",
        include_str!("../../../assets/scenarios/inland_basin_2001.ron"),
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
/// years older than its first year, at least [`HERO_FLOOR`] for a `hero` and 1 for any.
pub fn era_weight(year: u16, years: (u16, u16), hero: bool) -> u32 {
    if year > years.1 {
        return 0;
    }
    if year >= years.0 {
        return WEIGHT_ONE;
    }
    let age = u32::from(years.0 - year);
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

/// One car of a roster a [`Lineup`] is made from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RosterCar<'a> {
    pub id: &'a str,
    pub name: &'static str,
    pub year: u16,
    /// Its body: a `VehicleSpec::shape` file name.
    pub shape: &'a str,
}

/// The cars a world shows: the roster indices it allows, in roster order, their names (for
/// the Vehicle page's chooser), every roster car's [`era_weight`] (0 for the ones it does
/// not allow), and the same weights per built-in body shape, in
/// [`vehicles::BUILTIN_SHAPES`] order (the heaviest car of that shape's; 0 for a shape no
/// allowed car has), which is what traffic and parked cars index.
#[derive(Clone, Debug, PartialEq)]
pub struct Lineup {
    pub cars: Vec<usize>,
    pub names: Vec<&'static str>,
    pub weights: Vec<u32>,
    pub shapes: Vec<u32>,
}

impl Lineup {
    /// The lineup of `s` from `roster`.
    pub fn new(s: &Scenario, roster: &[RosterCar]) -> Lineup {
        let weights: Vec<u32> = roster
            .iter()
            .map(|c| era_weight(c.year, s.years, s.heroes.iter().any(|h| h == c.id)))
            .collect();
        let cars: Vec<usize> = (0..roster.len()).filter(|&i| weights[i] > 0).collect();
        let shapes = vehicles::BUILTIN_SHAPES
            .iter()
            .map(|(file, _)| {
                roster
                    .iter()
                    .zip(&weights)
                    .filter(|(c, _)| c.shape == *file)
                    .map(|(_, &w)| w)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        Lineup {
            names: cars.iter().map(|&i| roster[i].name).collect(),
            cars,
            weights,
            shapes,
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
        let roster = builtin_roster(|_| None);
        let cars: Vec<RosterCar> = roster.iter().map(OwnedCar::as_car).collect();
        all().iter().map(|s| Lineup::new(s, &cars)).collect()
    });
    &all[index.min(all.len() - 1)]
}

/// A built-in car as a [`RosterCar`], its strings owned.
pub(crate) struct OwnedCar {
    id: &'static str,
    name: &'static str,
    year: u16,
    shape: String,
}

impl OwnedCar {
    fn as_car(&self) -> RosterCar<'_> {
        RosterCar {
            id: self.id,
            name: self.name,
            year: self.year,
            shape: &self.shape,
        }
    }
}

/// The built-in cars in [`vehicles::BUILTIN`] order, `year_of` overriding a car's year
/// (tests date one differently).
fn builtin_roster(year_of: impl Fn(&str) -> Option<u16>) -> Vec<OwnedCar> {
    let names = crate::ui::roster::names();
    vehicles::BUILTIN
        .iter()
        .enumerate()
        .map(|(i, &(id, _))| {
            let spec = vehicles::builtin_spec(id).ok();
            let year = spec.as_ref().map_or(u16::MAX, |s| s.year);
            OwnedCar {
                id,
                name: names[i],
                year: year_of(id).unwrap_or(year),
                shape: spec.map(|s| s.shape).unwrap_or_default(),
            }
        })
        .collect()
}

/// A world of `scenario`'s with these `years`, over the built-in cars with `years_of`
/// overriding some cars' years: for tests of what a world shows.
#[cfg(test)]
pub(crate) fn lineup_with(
    scenario: usize,
    years: Option<(u16, u16)>,
    year_of: impl Fn(&str) -> Option<u16>,
) -> Lineup {
    let mut s = get(scenario).clone();
    s.years = years.unwrap_or(s.years);
    let roster = builtin_roster(year_of);
    let cars: Vec<RosterCar> = roster.iter().map(OwnedCar::as_car).collect();
    Lineup::new(&s, &cars)
}

/// Scenario `index`'s lineup of the built-in cars with car `id` re-dated to 2001: a 2001 car
/// in the place of a built-in one, for tests of what a world never shows.
#[cfg(test)]
pub(crate) fn lineup_with_2001_car(index: usize, id: &str) -> Lineup {
    lineup_with(index, None, |car| (car == id).then_some(2001))
}

/// Width and height of a scenario's still, texels.
pub const STILL_SIZE: (u32, u32) = (320, 200);
/// Stills per row of the atlas [`stills`] lays out.
pub const STILLS_PER_ROW: u32 = 4;

/// Each scenario's still for the World page (`assets/stills/<id>.still`, rendered with
/// `tools/screenshot`), in [`BUILTIN`] order: a 256-color palette (768 bytes of sRGB) and
/// then one palette index per texel, row-major from the top-left.
const STILLS: [(&str, &[u8]); 2] = [
    (
        "socal_beach_1984",
        include_bytes!("../../../assets/stills/socal_beach_1984.still"),
    ),
    (
        "inland_basin_2001",
        include_bytes!("../../../assets/stills/inland_basin_2001.still"),
    ),
];

/// An RGBA8 picture, row-major from the top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Texels of the atlas of `n` stills: rows of up to [`STILLS_PER_ROW`].
pub fn atlas_size(n: u32) -> (u32, u32) {
    let cols = n.clamp(1, STILLS_PER_ROW);
    (
        cols * STILL_SIZE.0,
        n.max(1).div_ceil(STILLS_PER_ROW) * STILL_SIZE.1,
    )
}

/// Where scenario `index`'s still is in [`stills`]: u0, v0, u1, v1.
pub fn still_uv(index: usize) -> [f32; 4] {
    let (w, h) = atlas_size(BUILTIN.len() as u32);
    let (col, row) = (index as u32 % STILLS_PER_ROW, index as u32 / STILLS_PER_ROW);
    let (x, y) = (col * STILL_SIZE.0, row * STILL_SIZE.1);
    [
        x as f32 / w as f32,
        y as f32 / h as f32,
        (x + STILL_SIZE.0) as f32 / w as f32,
        (y + STILL_SIZE.1) as f32 / h as f32,
    ]
}

/// Every scenario's still in an atlas ([`atlas_size`], [`still_uv`]), read once, for
/// `render::Renderer::upload_stills`. A still of the wrong size (never a built-in one,
/// which a test reads) is grey.
pub fn stills() -> &'static Image {
    static STILLS_IMAGE: OnceLock<Image> = OnceLock::new();
    STILLS_IMAGE.get_or_init(|| {
        let (sw, sh) = (STILL_SIZE.0 as usize, STILL_SIZE.1 as usize);
        let (w, h) = atlas_size(BUILTIN.len() as u32);
        let mut rgba = vec![96u8; (w * h * 4) as usize];
        for (k, (id, _)) in BUILTIN.iter().enumerate() {
            let Some((_, bytes)) = STILLS.iter().find(|s| s.0 == *id) else {
                continue;
            };
            if bytes.len() != 768 + sw * sh {
                continue;
            }
            let (palette, texels) = bytes.split_at(768);
            let [u, v, _, _] = still_uv(k);
            let (x0, y0) = ((u * w as f32) as usize, (v * h as f32) as usize);
            for (i, &t) in texels.iter().enumerate() {
                let at = (((y0 + i / sw) * w as usize) + x0 + i % sw) * 4;
                let c = &palette[usize::from(t) * 3..usize::from(t) * 3 + 3];
                rgba[at..at + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        Image { w, h, rgba }
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
            ("inland_basin_2001", (1965, 2001), true)
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
            ("\"gannet_gt\"", "\"hovercar\""),
            ("heroes: []", "heroes: [\"hovercar\"]"),
        ] {
            let broken = text.replace(from, to);
            assert_ne!(broken, text, "{from}");
            assert!(load(&broken).is_err(), "{to}");
        }
    }

    #[test]
    fn weights_fall_off_with_age() {
        let y = (2001, 2001);
        assert_eq!(era_weight(2002, y, false), 0, "newer than the world");
        assert_eq!(era_weight(2002, y, true), 0, "even a hero");
        assert_eq!(era_weight(2001, y, false), WEIGHT_ONE);
        // 1993: eight years older than the world, half.
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
        for year in 1900..2001u16 {
            let f = 0.5f64.powf(f64::from(2001 - year) / 8.0) * 1000.0;
            let w = era_weight(year, y, false);
            assert!(
                (f64::from(w) - f).abs() <= 1.0 || w == 1,
                "{year}: {w} vs {f}"
            );
        }
        // Measured from the world's first year: no step down at its edge.
        let y = (1995, 2001);
        assert_eq!(era_weight(1995, y, false), WEIGHT_ONE);
        assert_eq!(era_weight(1994, y, false), 917);
        assert_eq!(era_weight(1987, y, false), 500);
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
        let k = vehicles::BUILTIN
            .iter()
            .position(|b| b.0 == "halcyon_turbo")
            .unwrap();
        let l84 = lineup_with_2001_car(0, "halcyon_turbo");
        assert!(!l84.allows(k));
        assert_eq!(l84.position(k), None);
        assert!(!l84.names.contains(&crate::ui::roster::names()[k]));
        assert_eq!(l84.weights[k], 0);
        let mut next = 0;
        for _ in 0..2 * vehicles::BUILTIN.len() {
            next = l84.next_after(next);
            assert_ne!(next, k);
        }
        let l01 = lineup_with_2001_car(1, "halcyon_turbo");
        assert!(l01.allows(k));
        assert_eq!(l01.next_after(k - 1), k);
        assert_eq!(l01.next_after(vehicles::BUILTIN.len() - 1), 0, "wraps");
    }

    #[test]
    fn shape_weights_follow_each_shapes_own_car() {
        // A world cut inside the roster's years: every shape weighs what its car does,
        // whatever order the shapes and the cars are listed in.
        let l = lineup_with(0, Some((1979, 1983)), |_| None);
        for (k, (file, _)) in vehicles::BUILTIN_SHAPES.iter().enumerate() {
            let i = vehicles::BUILTIN
                .iter()
                .position(|(id, _)| vehicles::builtin_spec(id).unwrap().shape == *file)
                .unwrap();
            let year = vehicles::builtin_spec(vehicles::BUILTIN[i].0).unwrap().year;
            assert_eq!(l.shapes[k], l.weights[i], "{file}");
            assert_eq!(l.shapes[k] > 0, year <= 1983, "{file}: {year}");
        }
        assert!(l.shapes.contains(&0) && l.shapes.iter().any(|&w| w > 0));
    }

    #[test]
    fn every_scenario_has_its_still() {
        assert_eq!(STILLS.len(), BUILTIN.len());
        let all = stills();
        assert_eq!((all.w, all.h), atlas_size(BUILTIN.len() as u32));
        // `h = h * 31 + byte` mod 2^61 over each still's RGBA, as Pillow reads the
        // quantized frame the file was written from.
        let expected = [2_138_132_481_716_855_108u128, 2_229_843_925_750_921_464];
        for (k, ((id, bytes), want)) in STILLS.iter().zip(expected).enumerate() {
            assert_eq!(*id, BUILTIN[k].0);
            let [u, v, u1, v1] = still_uv(k);
            let (x0, y0) = ((u * all.w as f32) as u32, (v * all.h as f32) as u32);
            assert_eq!(
                (
                    ((u1 - u) * all.w as f32) as u32,
                    ((v1 - v) * all.h as f32) as u32
                ),
                STILL_SIZE
            );
            let mut h = 0u128;
            for y in y0..y0 + STILL_SIZE.1 {
                let row = ((y * all.w + x0) * 4) as usize;
                for &b in &all.rgba[row..row + (STILL_SIZE.0 * 4) as usize] {
                    h = (h * 31 + u128::from(b)) % (1 << 61);
                }
            }
            assert_eq!(h, want, "{id}");
            assert_eq!(bytes.len(), 768 + 320 * 200);
        }
    }

    #[test]
    fn the_stills_atlas_fits_a_dozen_worlds() {
        for n in 1..=12 {
            let (w, h) = atlas_size(n);
            assert!(
                w <= render::MAX_MAP_SIZE && h <= render::MAX_MAP_SIZE,
                "{n}: {w}x{h}"
            );
            assert!(w * h >= n * STILL_SIZE.0 * STILL_SIZE.1);
        }
        assert_eq!(atlas_size(2), (640, 200));
        assert_eq!(atlas_size(5), (1280, 400));
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
