//! What the car and the place sound like this frame: [`Sound`] turns the sim state, the
//! collider's surfaces, the place's zones, the daylight and the settings into the plain
//! `audio::synth::AudioParams` the platform's mixer reads (`docs/AUDIO_RADIO.md`).
//!
//! Each sim step notes the collider's [`world::Impact`] (new curb kicks and wall hits are
//! counted, so none is lost between frames); each frame [`Sound::params`] samples the rest.

use audio::synth::{
    AmbienceParams, AudioParams, EngineParams, EngineProfile, Surface, WheelParams,
};
use place::{Place, PropKind, RoadClass, ZoneId, Zones};
use vehicle::sim::{GRAVITY, WHEEL_RADIUS_M};

use crate::daylight::Daylight;
use crate::drive::{Drive, DriveGround};
use crate::settings::Settings;

/// A wall contact this fast or faster (approach speed, m/s) counts as a hit.
const HIT_MIN_MPS: f32 = 0.5;
/// Ambience looks at the zones this far round the car, m.
const AMBIENCE_RADIUS_M: f32 = 60.0;
/// ...and, for the foghorn, the open water this far round, m.
const FOGHORN_RADIUS_M: f32 = 350.0;
/// Birds sing with the sun between these elevations, degrees (none at night); crickets
/// start as it sets below the lower one and are in full voice by the second.
const BIRDS_SUN_DEG: [f32; 2] = [-2.0, 8.0];
const CRICKETS_SUN_DEG: [f32; 2] = [-1.0, -7.0];
/// A freeway is heard in full this near, m, fading as a line of traffic does (half as
/// loud four times as far), and not at all past [`FREEWAY_GONE_M`].
const FREEWAY_NEAR_M: f32 = 40.0;
const FREEWAY_GONE_M: f32 = 900.0;
/// The carhop speaker is heard in full this near a hot dog or burger stand, m, and not
/// at all this far.
const CARHOP_M: [f32; 2] = [12.0, 110.0];
/// The landmark kits whose lots have a carhop speaker.
const CARHOP_KITS: [&str; 2] = ["hot_dog_stand", "burger_stand"];
/// The place sounds a world can name in its scenario's `sounds`
/// (`audio::synth::signature`), in [`Signatures`] order.
pub const SOUNDS: [&str; 6] = [
    "foghorn",
    "el_train",
    "cable_car",
    "gulls",
    "helicopter",
    "ice_cream",
];
/// The city's traffic at night, against the day's.
const CITY_NIGHT: f32 = 0.55;

/// The place's zone grid, kept for the ambience (the place itself is not kept).
#[derive(Clone, Debug, Default)]
struct ZoneGrid {
    cells: Vec<ZoneId>,
    size: [usize; 2],
    origin: [f32; 2],
    cell_m: f32,
}

impl ZoneGrid {
    fn of(place: &Place) -> ZoneGrid {
        let header = &place.header;
        let res = place.terrain.res_per_chunk.max(1);
        let size = Zones::cells_per_side(header, res);
        if place.zones.cells.len() != size[0] * size[1] || size[0] == 0 || size[1] == 0 {
            return ZoneGrid::default();
        }
        ZoneGrid {
            cells: place.zones.cells.clone(),
            size,
            origin: header.origin_m(),
            cell_m: header.chunk_size_m / f32::from(res),
        }
    }

    /// The zone at `[x, z]`, clamped to the grid; `None` without one.
    fn at(&self, p: [f32; 2]) -> Option<ZoneId> {
        if self.cells.is_empty() {
            return None;
        }
        let cell =
            |v: f32, o: f32, n: usize| (((v - o) / self.cell_m).max(0.0) as usize).min(n - 1);
        let x = cell(p[0], self.origin[0], self.size[0]);
        let z = cell(p[1], self.origin[1], self.size[1]);
        Some(self.cells[z * self.size[0] + x])
    }
}

/// The shares of the ground round a point the ambience is made from, `[0, 1]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Shares {
    ocean: f32,
    /// Yards and parks: birds by day, crickets by night.
    yards: f32,
    city: f32,
    trees: f32,
    industry: f32,
    /// Open water further out, for the foghorn.
    open_water: f32,
}

/// Which of [`SOUNDS`] the world has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signatures {
    pub foghorn: bool,
    pub el_train: bool,
    pub cable_car: bool,
    pub gulls: bool,
    pub helicopter: bool,
    pub ice_cream: bool,
}

impl Signatures {
    /// From a scenario's `sounds` (names not in [`SOUNDS`] are left out).
    pub fn of(names: &[String]) -> Signatures {
        let has = |s: &str| names.iter().any(|n| n == s);
        Signatures {
            foghorn: has(SOUNDS[0]),
            el_train: has(SOUNDS[1]),
            cable_car: has(SOUNDS[2]),
            gulls: has(SOUNDS[3]),
            helicopter: has(SOUNDS[4]),
            ice_cream: has(SOUNDS[5]),
        }
    }
}

/// The sound state the app keeps between steps and frames.
#[derive(Clone, Debug)]
pub struct Sound {
    profile: EngineProfile,
    zones: ZoneGrid,
    /// The highways' segments, `[ax, az, bx, bz]`.
    freeways: Vec<[f32; 4]>,
    /// The world's own sounds ([`crate::scenario::Scenario::sounds`]).
    signatures: Signatures,
    /// The hot dog and burger stands, `[x, z]`.
    stands: Vec<[f32; 2]>,
    /// Half the car's width, m: how far out a bump counts as fully on one side.
    half_width: f32,
    curb_kicks: u32,
    curb_speed: f32,
    curb_side: f32,
    hits: u32,
    hit_speed: f32,
    hit_side: f32,
    in_contact: bool,
    kicking: bool,
}

impl Sound {
    /// For the car `drive` in `place`, a world with `signatures` whose landmarks are
    /// `kits`.
    pub fn new(
        place: &Place,
        drive: &Drive,
        signatures: Signatures,
        kits: &[world::landmarks::Kit],
    ) -> Sound {
        let stands = place
            .props
            .items
            .iter()
            .filter_map(|p| match p.kind {
                PropKind::Landmark { kit, .. } => kits
                    .get(usize::from(kit))
                    .filter(|k| CARHOP_KITS.contains(&k.id.as_str()))
                    .map(|_| [p.pos[0], p.pos[2]]),
                _ => None,
            })
            .collect();
        let nodes = &place.roads.nodes;
        let freeways = place
            .roads
            .edges
            .iter()
            .filter(|e| e.class == RoadClass::Highway)
            .filter_map(|e| {
                let (a, b) = (nodes.get(e.a as usize)?, nodes.get(e.b as usize)?);
                Some([a.pos[0], a.pos[1], b.pos[0], b.pos[1]])
            })
            .collect();
        Sound {
            profile: EngineProfile::from_name(&drive.spec.feel.engine_sound),
            zones: ZoneGrid::of(place),
            freeways,
            signatures,
            stands,
            half_width: 0.5 * drive.spec.dims_m[1],
            curb_kicks: 0,
            curb_speed: 0.0,
            curb_side: 0.0,
            hits: 0,
            hit_speed: 0.0,
            hit_side: 0.0,
            in_contact: false,
            kicking: false,
        }
    }

    /// The player switched to the car `drive` drives: its engine's voice and its width
    /// from now on.
    pub fn set_car(&mut self, drive: &Drive) {
        self.profile = EngineProfile::from_name(&drive.spec.feel.engine_sound);
        self.half_width = 0.5 * drive.spec.dims_m[1];
    }

    /// After every sim step: counts a new wall hit and a new curb kick, each with how hard
    /// and on which side of the car it was, so the bump plays to scale.
    pub fn note_step(&mut self, impact: &world::Impact) {
        let side = |b: &world::Bump| (b.at[0] / self.half_width).clamp(-1.0, 1.0);
        let contact = impact.contact && impact.hit.speed >= HIT_MIN_MPS;
        if contact && !self.in_contact {
            self.hits = self.hits.wrapping_add(1);
            self.hit_speed = impact.hit.speed;
            self.hit_side = side(&impact.hit);
        }
        self.in_contact = impact.contact;
        if impact.curb_kick && !self.kicking {
            self.curb_kicks = self.curb_kicks.wrapping_add(1);
            self.curb_speed = impact.curb.speed;
            self.curb_side = side(&impact.curb);
        }
        self.kicking = impact.curb_kick;
    }

    /// This frame's sound: the car `drive` on `ground`, in `daylight`, with the user's
    /// `settings`; `paused` while a menu is open.
    pub fn params(
        &self,
        drive: &Drive,
        ground: &DriveGround,
        daylight: &Daylight,
        settings: &Settings,
        paused: bool,
    ) -> AudioParams {
        let s = &drive.state;
        let spec = &drive.spec;
        let rest_load = 0.25 * spec.mass_kg * GRAVITY;
        let mut wheels = [WheelParams::default(); 4];
        for (w, ws) in wheels.iter_mut().zip(&s.wheels) {
            let c = ws.contact;
            let (_, surface) = ground.level_from([c[0], c[2]], c[1] + 0.5);
            *w = WheelParams {
                slip: ws.slip,
                load: ws.load / rest_load,
                speed_mps: s.speed_mps.abs().max((ws.spin * WHEEL_RADIUS_M).abs()),
                grounded: ws.grounded,
                surface: match surface {
                    world::collide::Surface::Asphalt => Surface::Asphalt,
                    world::collide::Surface::Concrete => Surface::Concrete,
                    world::collide::Surface::Grass => Surface::Grass,
                },
            };
        }
        let p = [s.pos[0], s.pos[2]];
        let shares = self.shares(p);
        let ease = |[lo, hi]: [f32; 2]| {
            let t = ((daylight.sun_elevation_deg - lo) / (hi - lo)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let day = ease(BIRDS_SUN_DEG);
        let night = ease(CRICKETS_SUN_DEG);
        let w = self.signatures;
        let on = |has: bool, amount: f32| if has { amount } else { 0.0 };
        let unit = |v: f32| v.clamp(0.0, 1.0);
        let ambience = AmbienceParams {
            city: shares.city * (CITY_NIGHT + (1.0 - CITY_NIGHT) * (1.0 - night)),
            freeway: self.freeway(p),
            trees: shares.trees,
            crickets: shares.yards * night,
            industry: shares.industry,
            foghorn: on(w.foghorn, shares.open_water),
            el_train: on(
                w.el_train,
                (1.2 * shares.city + 0.5 * shares.yards).min(1.0),
            ),
            cable_car: on(w.cable_car, unit((shares.city - 0.3) / 0.5)),
            gulls: on(
                w.gulls,
                (shares.ocean + 0.5 * shares.open_water).min(1.0) * day,
            ),
            helicopter: on(w.helicopter, 0.5 + 0.5 * shares.city),
            ice_cream: on(w.ice_cream, shares.yards * day),
            carhop: self.carhop(p),
        };
        AudioParams {
            master: settings.master_volume,
            engine_volume: settings.engine_volume,
            world_volume: settings.world_volume,
            muted: settings.mute,
            paused,
            // Set by the app from the car radio (`App::frame`).
            radio: 0.0,
            engine: EngineParams {
                profile: self.profile,
                rpm: s.engine_rpm,
                idle_rpm: spec.engine.idle_rpm,
                redline_rpm: spec.engine.redline_rpm,
                // In reverse the brake pedal drives.
                throttle: if s.gear < 0 { s.brake } else { s.throttle },
                gear: s.gear,
                shift_ms: spec.shift_ms,
            },
            speed_mps: s.speed_mps,
            wheels,
            open_top: drive.layout.open_top,
            curb_kicks: self.curb_kicks,
            curb_speed_mps: self.curb_speed,
            hits: self.hits,
            hit_speed_mps: self.hit_speed,
            curb_side: self.curb_side,
            hit_side: self.hit_side,
            horn: drive.horn,
            ocean: shares.ocean,
            birds: shares.yards * day,
            ambience,
            // Set by the app from the railway (`App::frame`).
            bell: 0.0,
            // Set by the app from the traffic (`App::frame`).
            traffic: Default::default(),
            // Set by the app from the car radio (`App::frame`).
            tunes: 0,
            tune_volume: 0.0,
            deck_clicks: 0,
            deck_kind: 0,
            // Set by the app from the green wave (`App::frame`).
            waves: 0,
            wave_run: 0,
        }
    }

    /// Shares of the ground round `p` (the zones at it, counted twice, and
    /// [`AMBIENCE_RADIUS_M`] round it in eight directions; the open water
    /// [`FOGHORN_RADIUS_M`] round it); all 0 without zones.
    fn shares(&self, p: [f32; 2]) -> Shares {
        const RING: [[f32; 2]; 8] = [
            [1.0, 0.0],
            [0.707, 0.707],
            [0.0, 1.0],
            [-0.707, 0.707],
            [-1.0, 0.0],
            [-0.707, -0.707],
            [0.0, -1.0],
            [0.707, -0.707],
        ];
        let around = |d: &[f32; 2], r: f32| [p[0] + d[0] * r, p[1] + d[1] * r];
        let mut s = Shares::default();
        let mut total = 0.0;
        let ring = RING.iter().map(|d| (around(d, AMBIENCE_RADIUS_M), 1.0));
        for (q, weight) in [(p, 2.0)].into_iter().chain(ring) {
            let Some(zone) = self.zones.at(q) else {
                return Shares::default();
            };
            total += weight;
            // How busy, how leafy, and the rest, per zone.
            let (city, trees) = match zone {
                ZoneId::Downtown => (1.0, 0.0),
                ZoneId::Commercial => (0.8, 0.1),
                ZoneId::Strip => (0.7, 0.05),
                ZoneId::Highway => (0.6, 0.0),
                ZoneId::Industrial => (0.5, 0.0),
                ZoneId::Residential => (0.3, 0.6),
                ZoneId::Suburb => (0.15, 0.7),
                ZoneId::Park => (0.15, 1.0),
                ZoneId::Beach | ZoneId::Water => (0.1, 0.0),
                ZoneId::Hills => (0.0, 0.8),
                ZoneId::Farmland => (0.0, 0.4),
            };
            s.city += city * weight;
            s.trees += trees * weight;
            match zone {
                ZoneId::Beach | ZoneId::Water => s.ocean += weight,
                ZoneId::Residential | ZoneId::Suburb | ZoneId::Park => s.yards += weight,
                ZoneId::Industrial => s.industry += weight,
                _ => {}
            }
        }
        for v in [
            &mut s.ocean,
            &mut s.yards,
            &mut s.city,
            &mut s.trees,
            &mut s.industry,
        ] {
            *v /= total;
        }
        let water = RING
            .iter()
            .filter(|d| self.zones.at(around(d, FOGHORN_RADIUS_M)) == Some(ZoneId::Water))
            .count();
        s.open_water = (water as f32 / 4.0).min(1.0);
        s
    }

    /// How near the nearest hot dog or burger stand is, `[0, 1]`: 1 within
    /// `CARHOP_M[0]`, 0 past `CARHOP_M[1]`.
    fn carhop(&self, p: [f32; 2]) -> f32 {
        let d2 = self
            .stands
            .iter()
            .map(|s| (s[0] - p[0]) * (s[0] - p[0]) + (s[1] - p[1]) * (s[1] - p[1]))
            .fold(f32::INFINITY, f32::min);
        let [near, far] = CARHOP_M;
        let t = 1.0 - ((d2.sqrt() - near) / (far - near)).clamp(0.0, 1.0);
        t * t
    }

    /// How near the nearest freeway is, `[0, 1]`: 1 within [`FREEWAY_NEAR_M`], then as
    /// the square root of the distance falls off, gone by [`FREEWAY_GONE_M`].
    fn freeway(&self, p: [f32; 2]) -> f32 {
        let mut d2 = f32::INFINITY;
        for &[ax, az, bx, bz] in &self.freeways {
            let (dx, dz) = (bx - ax, bz - az);
            let len2 = dx * dx + dz * dz;
            let t = if len2 > 0.0 {
                (((p[0] - ax) * dx + (p[1] - az) * dz) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (ex, ez) = (ax + t * dx - p[0], az + t * dz - p[1]);
            d2 = d2.min(ex * ex + ez * ez);
        }
        let d = d2.sqrt();
        if d >= FREEWAY_GONE_M {
            return 0.0;
        }
        let near = (FREEWAY_NEAR_M / d.max(FREEWAY_NEAR_M)).sqrt();
        let fade = 1.0 - d / FREEWAY_GONE_M;
        near * fade.sqrt()
    }
}

/// The mixer's one-shot counters in [`AudioParams`] (wall hits, curb kicks, radio tunes,
/// deck clicks, green-wave chimes), shifted so they run on across a world switch: the
/// mixer outlives the app, and plays a sound whenever a counter changes, so the new
/// app's counters (restarting at 0) must not look like new events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Added (wrapping) to each counter this app makes.
    base: [u32; 5],
    /// What the mixer last saw from the previous app, until this app's first frame.
    carried: Option<[u32; 5]>,
}

fn counts(p: &AudioParams) -> [u32; 5] {
    [p.hits, p.curb_kicks, p.tunes, p.deck_clicks, p.waves]
}

impl Counters {
    /// For the next app, after this one whose last frame's sound was `last` (or, if it
    /// never made a frame, what it carried in).
    pub fn for_next(&self, last: &AudioParams) -> Counters {
        Counters {
            base: [0; 5],
            carried: Some(self.carried.unwrap_or(counts(last))),
        }
    }

    /// Shifts this frame's counters in `p`: on the first frame they read what the mixer
    /// last saw, and every later event moves them on from there.
    pub fn apply(&mut self, p: &mut AudioParams) {
        let now = counts(p);
        if let Some(last) = self.carried.take() {
            for i in 0..5 {
                self.base[i] = last[i].wrapping_sub(now[i]);
            }
        }
        let b = self.base;
        p.hits = p.hits.wrapping_add(b[0]);
        p.curb_kicks = p.curb_kicks.wrapping_add(b[1]);
        p.tunes = p.tunes.wrapping_add(b[2]);
        p.deck_clicks = p.deck_clicks.wrapping_add(b[3]);
        p.waves = p.waves.wrapping_add(b[4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impact(contact: bool, speed: f32, curb_kick: bool) -> world::Impact {
        let bump = |speed: f32, x: f32| world::Bump {
            speed,
            normal: [0.0, 1.0, 0.0],
            at: [x, 0.0, -2.0],
        };
        world::Impact {
            contact,
            hit: bump(speed, 0.45),
            curb_kick,
            curb: bump(if curb_kick { 15.0 } else { 0.0 }, -1.2),
            parked: None,
            parked_hit: world::Bump::default(),
        }
    }

    fn sound() -> Sound {
        Sound {
            profile: EngineProfile::Default,
            freeways: vec![[0.0, 1000.0, 100.0, 1000.0]],
            signatures: Signatures::of(&["foghorn".to_string()]),
            stands: vec![[0.0, 0.0]],
            zones: ZoneGrid {
                cells: vec![
                    ZoneId::Residential,
                    ZoneId::Strip,
                    ZoneId::Beach,
                    ZoneId::Water,
                ],
                size: [2, 2],
                origin: [-200.0, -200.0],
                cell_m: 200.0,
            },
            half_width: 0.9,
            curb_kicks: 0,
            curb_speed: 0.0,
            curb_side: 0.0,
            hits: 0,
            hit_speed: 0.0,
            hit_side: 0.0,
            in_contact: false,
            kicking: false,
        }
    }

    #[test]
    fn hits_and_kicks_count_once_each() {
        let mut s = sound();
        s.note_step(&impact(true, 4.0, false));
        s.note_step(&impact(true, 0.0, false));
        s.note_step(&impact(true, 6.0, false));
        assert_eq!((s.hits, s.hit_speed, s.hit_side), (1, 4.0, 0.5));
        s.note_step(&impact(false, 0.0, false));
        s.note_step(&impact(true, 0.2, false));
        assert_eq!(s.hits, 1, "a resting touch is not a hit");
        s.note_step(&impact(false, 0.0, true));
        s.note_step(&impact(false, 0.0, true));
        s.note_step(&impact(false, 0.0, false));
        s.note_step(&impact(false, 0.0, true));
        assert_eq!((s.curb_kicks, s.curb_speed, s.curb_side), (2, 15.0, -1.0));
    }

    #[test]
    fn ambience_follows_the_zones() {
        let s = sound();
        // Deep in the residential cell: yards and trees, a little city.
        let a = s.shares([-150.0, -150.0]);
        assert_eq!((a.ocean, a.yards), (0.0, 1.0));
        assert!(
            (a.city - 0.3).abs() < 1e-6 && (a.trees - 0.6).abs() < 1e-6,
            "{a:?}"
        );
        // In the beach/water row: all ocean, and the open water further out.
        let a = s.shares([-100.0, 150.0]);
        assert_eq!((a.ocean, a.yards), (1.0, 0.0));
        assert!(a.open_water > 0.0);
        // Without zones, nothing.
        let none = Sound {
            zones: ZoneGrid::default(),
            ..sound()
        };
        assert_eq!(none.shares([0.0, 0.0]), Shares::default());
    }

    #[test]
    fn the_carhop_speaker_plays_near_a_stand() {
        let s = sound();
        assert_eq!(s.carhop([5.0, 0.0]), 1.0);
        assert!(s.carhop([60.0, 0.0]) > 0.0 && s.carhop([60.0, 0.0]) < 0.5);
        assert_eq!(s.carhop([200.0, 0.0]), 0.0);
        let none = Sound {
            stands: Vec::new(),
            ..sound()
        };
        assert_eq!(none.carhop([0.0, 0.0]), 0.0);
    }

    #[test]
    fn the_freeway_fades_with_distance() {
        let s = sound();
        assert!(s.freeway([50.0, 1010.0]) > 0.99);
        let mid = s.freeway([50.0, 1160.0]);
        assert!(mid > 0.3 && mid < 0.6, "{mid}");
        assert!(s.freeway([50.0, 1600.0]) < mid);
        assert_eq!(s.freeway([50.0, 2000.0]), 0.0);
    }

    #[test]
    fn counters_go_on_from_the_last_app() {
        let mut last = AudioParams::default();
        (last.hits, last.curb_kicks, last.tunes) = (4, u32::MAX, 2);
        (last.deck_clicks, last.waves) = (9, 1);
        let mut c = Counters::default().for_next(&last);
        let counts = |p: &AudioParams| [p.hits, p.curb_kicks, p.tunes, p.deck_clicks, p.waves];
        // The new app's first frame (a click already counted) reads what the mixer saw...
        let mut p = AudioParams {
            deck_clicks: 1,
            ..AudioParams::default()
        };
        c.apply(&mut p);
        assert_eq!(counts(&p), counts(&last));
        // ...and its next events move on from there, wrapping.
        let mut p = AudioParams {
            curb_kicks: 1,
            deck_clicks: 2,
            ..AudioParams::default()
        };
        c.apply(&mut p);
        assert_eq!(counts(&p), [4, 0, 2, 10, 1]);
        // A switch before the new app made a frame carries the same values on.
        let mut next = Counters::default()
            .for_next(&last)
            .for_next(&AudioParams::default());
        let mut p = AudioParams::default();
        next.apply(&mut p);
        assert_eq!(counts(&p), counts(&last));
    }
}
