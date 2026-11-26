//! Districts: the place's zone map split at load into connected runs of one zone, each big
//! enough one named ("Torvel Beach", "Downtown Calen") from the place's seed. Driving into
//! a new one shows its name for a few seconds ([`DistrictCard`]). Pure and deterministic:
//! a row-major flood fill in a fixed order, names from [`names::write_stem`].

use place::{Place, ZoneId, Zones};
use skywave_core::hash64;

use super::names;

/// No district: a cell of an unnamed zone (highway, water) or of a run too small to name.
const NONE: u16 = u16::MAX;
/// A run of one zone needs this much ground to be named, m² (a 200 m block).
const MIN_AREA_M2: f32 = 40_000.0;
/// The car must stay in a new district this long before its card shows, s, so a corner
/// clipped or a line straddled does not flash names.
pub const DWELL_S: f32 = 1.5;
/// The card's fade in, hold and fade out, s.
pub const FADE_IN_S: f32 = 0.6;
pub const HOLD_S: f32 = 2.8;
pub const FADE_OUT_S: f32 = 1.2;

/// How a zone's district is named around its stem; `None` for zones that are not
/// districts.
fn pattern(zone: ZoneId) -> Option<(&'static str, &'static str)> {
    Some(match zone {
        ZoneId::Downtown => ("Downtown ", ""),
        ZoneId::Strip => ("", " Strip"),
        ZoneId::Residential => ("", " Village"),
        ZoneId::Suburb => ("", " Estates"),
        ZoneId::Industrial => ("", " Flats"),
        ZoneId::Commercial => ("", " Square"),
        ZoneId::Park => ("", " Park"),
        ZoneId::Beach => ("", " Beach"),
        ZoneId::Farmland => ("", " Valley"),
        ZoneId::Hills => ("", " Heights"),
        ZoneId::Highway | ZoneId::Water => return None,
    })
}

/// The place's districts: a district index per zone cell and the districts' names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Districts {
    cells: Vec<u16>,
    size: [usize; 2],
    origin: [f32; 2],
    cell_m: f32,
    pub names: Vec<String>,
}

impl Districts {
    pub fn of(place: &Place) -> Districts {
        let header = &place.header;
        let res = place.terrain.res_per_chunk.max(1);
        let size = Zones::cells_per_side(header, res);
        let zones = &place.zones.cells;
        let n = size[0] * size[1];
        if zones.len() != n || n == 0 {
            return Districts::default();
        }
        let cell_m = header.chunk_size_m / f32::from(res);
        let min_cells = (MIN_AREA_M2 / (cell_m * cell_m)).ceil() as usize;
        let mut cells = vec![NONE; n];
        let mut seen = vec![false; n];
        let mut names = Vec::new();
        let mut run = Vec::new();
        let mut stack = Vec::new();
        for start in 0..n {
            if seen[start] {
                continue;
            }
            let zone = zones[start];
            seen[start] = true;
            let Some((pre, post)) = pattern(zone) else {
                continue;
            };
            // Flood the run of `zone` from `start` (4-connected).
            run.clear();
            stack.push(start);
            while let Some(i) = stack.pop() {
                run.push(i);
                let (x, z) = (i % size[0], i / size[0]);
                let mut visit = |j: usize| {
                    if !seen[j] && zones[j] == zone {
                        seen[j] = true;
                        stack.push(j);
                    }
                };
                if x > 0 {
                    visit(i - 1);
                }
                if x + 1 < size[0] {
                    visit(i + 1);
                }
                if z > 0 {
                    visit(i - size[0]);
                }
                if z + 1 < size[1] {
                    visit(i + size[0]);
                }
            }
            if run.len() < min_cells || names.len() >= usize::from(NONE) {
                continue;
            }
            let id = names.len() as u16;
            for &i in &run {
                cells[i] = id;
            }
            let mut name = String::from(pre);
            // Writing to a String cannot fail.
            let _ = names::write_stem(&mut name, hash64(header.seed, &[start as u64]));
            name.push_str(post);
            names.push(name);
        }
        Districts {
            cells,
            size,
            origin: header.origin_m(),
            cell_m,
            names,
        }
    }

    /// The district at ground point `[x, z]`, if any.
    pub fn at(&self, p: [f32; 2]) -> Option<usize> {
        if self.cells.is_empty() {
            return None;
        }
        let x = ((p[0] - self.origin[0]) / self.cell_m).floor();
        let z = ((p[1] - self.origin[1]) / self.cell_m).floor();
        if x < 0.0 || z < 0.0 || x >= self.size[0] as f32 || z >= self.size[1] as f32 {
            return None;
        }
        let id = self.cells[z as usize * self.size[0] + x as usize];
        (id != NONE).then_some(usize::from(id))
    }
}

/// When to show a district's name: after the car has been in a new district for
/// [`DWELL_S`], once, until it settles in another.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DistrictCard {
    /// The district whose name was last shown.
    shown: Option<usize>,
    /// The district the car is in and how long it has been there, s.
    here: Option<usize>,
    here_s: f32,
    /// The card up: its district and its age, s.
    card: Option<(usize, f32)>,
}

impl DistrictCard {
    /// One frame of `dt` seconds with the car in district `here`.
    pub fn update(&mut self, here: Option<usize>, dt: f32) {
        if here != self.here {
            self.here = here;
            self.here_s = 0.0;
        } else {
            self.here_s += dt;
        }
        if let Some((_, age)) = &mut self.card {
            *age += dt;
            if *age >= FADE_IN_S + HOLD_S + FADE_OUT_S {
                self.card = None;
            }
        }
        if let Some(d) = self.here {
            if self.here_s >= DWELL_S && self.shown != Some(d) {
                self.shown = Some(d);
                self.card = Some((d, 0.0));
            }
        }
    }

    /// The district on the card and the card's opacity, 0..=1.
    pub fn showing(&self) -> Option<(usize, f32)> {
        let (d, age) = self.card?;
        let a = if age < FADE_IN_S {
            age / FADE_IN_S
        } else {
            1.0 - ((age - FADE_IN_S - HOLD_S) / FADE_OUT_S).clamp(0.0, 1.0)
        };
        Some((d, a.clamp(0.0, 1.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::places;
    use crate::PlaceKey;
    use skywave_core::Seed;

    #[test]
    fn coast_districts_are_few_named_and_found() {
        let place = places::build(PlaceKey::Coast, Seed(1)).expect("coast");
        let d = Districts::of(&place);
        assert!(
            (4..=60).contains(&d.names.len()),
            "{} districts: {:?}",
            d.names.len(),
            d.names
        );
        assert_eq!(d, Districts::of(&place), "deterministic");
        for name in &d.names {
            assert!((6..=24).contains(&name.len()), "{name}");
        }
        let start = place.player_start().expect("start").pos;
        let _ = d.at([start[0], start[2]]);
        assert_eq!(d.at([-1.0e6, 0.0]), None);
    }

    #[test]
    fn card_waits_fades_and_shows_once() {
        let mut c = DistrictCard::default();
        let dt = 0.1;
        // A brush with district 1 is too short to name it.
        for _ in 0..5 {
            c.update(Some(1), dt);
        }
        c.update(None, dt);
        assert_eq!(c.showing(), None);
        for _ in 0..16 {
            c.update(Some(0), dt);
        }
        let (d, a) = c.showing().expect("card up");
        assert_eq!(d, 0);
        assert!(a < 0.5, "fading in: {a}");
        for _ in 0..30 {
            c.update(Some(0), dt);
        }
        assert_eq!(c.showing().map(|s| s.1), Some(1.0));
        for _ in 0..20 {
            c.update(Some(0), dt);
        }
        assert_eq!(c.showing(), None, "gone, and not again while here");
        // Out to the highway and back: the same district does not show again.
        for _ in 0..30 {
            c.update(None, dt);
        }
        for _ in 0..30 {
            c.update(Some(0), dt);
        }
        assert_eq!(c.showing(), None);
    }
}
