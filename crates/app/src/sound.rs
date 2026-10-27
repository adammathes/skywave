//! What the car and the place sound like this frame: [`Sound`] turns the sim state, the
//! collider's surfaces, the place's zones, the daylight and the settings into the plain
//! `audio::synth::AudioParams` the platform's mixer reads (`docs/AUDIO_RADIO.md`).
//!
//! Each sim step notes the collider's [`world::Impact`] (new curb kicks and wall hits are
//! counted, so none is lost between frames); each frame [`Sound::params`] samples the rest.

use audio::synth::{AudioParams, EngineParams, EngineProfile, Surface, WheelParams};
use place::{Place, ZoneId, Zones};
use vehicle::sim::{GRAVITY, WHEEL_RADIUS_M};

use crate::daylight::Daylight;
use crate::drive::{Drive, DriveGround};
use crate::settings::Settings;

/// A wall contact this fast or faster (approach speed, m/s) counts as a hit.
const HIT_MIN_MPS: f32 = 0.5;
/// Ambience looks at the zones this far round the car, m.
const AMBIENCE_RADIUS_M: f32 = 60.0;
/// Birds sing with the sun between these elevations, degrees (none at night).
const BIRDS_SUN_DEG: [f32; 2] = [-2.0, 8.0];

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

/// The sound state the app keeps between steps and frames.
#[derive(Clone, Debug)]
pub struct Sound {
    profile: EngineProfile,
    zones: ZoneGrid,
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
    /// For the car `drive` in `place`.
    pub fn new(place: &Place, drive: &Drive) -> Sound {
        Sound {
            profile: EngineProfile::from_name(&drive.spec.feel.engine_sound),
            zones: ZoneGrid::of(place),
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
        let (ocean, birds) = self.ambience([s.pos[0], s.pos[2]]);
        let [lo, hi] = BIRDS_SUN_DEG;
        let t = ((daylight.sun_elevation_deg - lo) / (hi - lo)).clamp(0.0, 1.0);
        let day = t * t * (3.0 - 2.0 * t);
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
            curb_kicks: self.curb_kicks,
            curb_speed_mps: self.curb_speed,
            hits: self.hits,
            hit_speed_mps: self.hit_speed,
            curb_side: self.curb_side,
            hit_side: self.hit_side,
            horn: drive.horn,
            ocean,
            birds: birds * day,
        }
    }

    /// Shares of the ground round `p` that are beach or water, and residential or park.
    /// The point itself counts twice.
    fn ambience(&self, p: [f32; 2]) -> (f32, f32) {
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
        let mut ocean = 0.0;
        let mut birds = 0.0;
        let mut total = 0.0;
        let ring = RING.iter().map(|d| {
            let q = [
                p[0] + d[0] * AMBIENCE_RADIUS_M,
                p[1] + d[1] * AMBIENCE_RADIUS_M,
            ];
            (q, 1.0)
        });
        for (q, weight) in [(p, 2.0)].into_iter().chain(ring) {
            let Some(zone) = self.zones.at(q) else {
                return (0.0, 0.0);
            };
            total += weight;
            match zone {
                ZoneId::Beach | ZoneId::Water => ocean += weight,
                ZoneId::Residential | ZoneId::Suburb | ZoneId::Park => birds += weight,
                _ => {}
            }
        }
        (ocean / total, birds / total)
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
        // Deep in the residential cell: birds only.
        let (o, b) = s.ambience([-150.0, -150.0]);
        assert_eq!((o, b), (0.0, 1.0));
        // In the beach/water row: all ocean.
        let (o, b) = s.ambience([-100.0, 150.0]);
        assert_eq!((o, b), (1.0, 0.0));
        // Without zones, nothing.
        let none = Sound {
            zones: ZoneGrid::default(),
            ..sound()
        };
        assert_eq!(none.ambience([0.0, 0.0]), (0.0, 0.0));
    }
}
