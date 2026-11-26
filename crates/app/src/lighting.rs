//! The game's time of day and how [`Daylight`] becomes the renderer's [`Sun`] and [`Sky`],
//! and the lights that follow it: the street lamps' photocell and pools, the glow of the
//! car's lamps and instruments, and the light inside the cabin.
//!
//! **Light lists.** The renderer shades each instance with the lights its
//! [`render::LightSet`] names, up to [`LIGHTS_PER_DRAW`] of the frame's [`MAX_LIGHTS`].
//! [`FrameLights`] fills both once a frame: the car's own lights first, then every static
//! light of the place (street lamps, neon, storefront fill) while the photocell has them on
//! (the nearest to the camera when there are more than fit, the farthest of those fading
//! out), then for each instance the lights that light its bounds most, each fading out as
//! the first left out closes on it, so none pops off when they swap. Fixed arrays and
//! buffers sized at load: nothing allocates per frame, and the cost is one box-to-sphere
//! test per light and instance near it: the frame's lights are binned into a coarse grid
//! once, and each instance scores only the lights in the cells it overlaps.
//!
//! Units: `Daylight` gives illuminance in lux and luminance in cd/m²; the renderer takes
//! kilolux and kcd/m². Directions agree (world X east, Y up, Z south).
//!
//! Night brightness is set in post-exposure terms (the tonemapper's input, where 0.18 is mid
//! grey and the bloom threshold is 1): whatever the exposure, a lit window, the instrument
//! backlight or the cabin's ambient lands at a chosen display value instead of following
//! its physical luminance up into a blown-out blob.

use glam::Vec3;
use render::{
    exposure_from_ev100, Corona, LightSet, MeshInstance, PointLight, Sky, Sun, LIGHTS_PER_DRAW,
    MAX_LIGHTS,
};

use crate::daylight::{Clock, Daylight, DEFAULT_LATITUDE_DEG};
use crate::drive::CarGlow;
use crate::input::{Action, InputState};

/// Late September: near the equinox the sun sets almost due west at about 18:00, so...
pub const DEFAULT_DAY_OF_YEAR: u16 = 265;
/// ...17:40 is golden hour, the first slice's showpiece.
pub const DEFAULT_TIME_S: f64 = 17.0 * 3600.0 + 40.0 * 60.0;
/// `TimeScrub` at full deflection moves the clock this many game seconds per real second.
pub const SCRUB_SECONDS_PER_SECOND: f64 = 3600.0;
/// Luminance of the full moon's disc, kcd/m² (the renderer works out the phase).
const MOON_DISC_KCD: f32 = 2.5;
/// kcd/m² of an emissive with rgb 255 and scale 1 (`Sky::emissive_boost`, the renderer's
/// default).
pub const EMISSIVE_BOOST: f32 = 0.5;
/// Highest post-exposure value any emissive reaches (`Sky::emissive_cap`): five times the
/// bloom threshold, so neon, signals and lamp heads bloom into a halo at night without
/// swallowing the frame. By day every emissive is far under it.
pub const EMISSIVE_CAP: f32 = 5.0;
/// Post-exposure value of a scale-1 white emissive on the car's exterior lamps at most, so
/// tail lamps (scale 1) glow a saturated red at ~1.5 and brake lamps (3.5) near the cap at
/// night (brighter, AgX rolls a red lamp toward salmon).
const CAR_LAMP_EXPOSED: f32 = 1.5;
/// ...and at least, by day: as built a lens's glow is a tenth of its own sunlit color at
/// noon and brake lamps did not read in the sun. At this a brake lamp lands at ~2.5, red and
/// clearly brighter than a tail lamp, which stays under the bloom threshold.
const CAR_LAMP_DAY_EXPOSED: f32 = 0.7;
/// The same for the instruments: the radio display (scale 1.5) at ~0.9, just under the
/// bloom threshold, the needles (3, white with red tips) at ~1.8 with a faint halo, the dial
/// faces (0.7) as a backlight in the car's display color.
const INSTRUMENT_EXPOSED: f32 = 0.6;
/// Cabin ambient (`Sky::cabin_light`) after exposure: by day the light through the glass,
/// neutral; by night the dome and instrument backlight, warm and dim. A white surface in the
/// cabin would read at this value; the dark 80s trims read a tenth of it. At night it is a
/// quarter of the day's, so the backlit dials and the radio are what glows.
const CABIN_DAY_EXPOSED: f32 = 1.6;
const CABIN_NIGHT_EXPOSED: f32 = 0.4;
const CABIN_DAY_RGB: [f32; 3] = [0.96, 1.0, 1.04];
const CABIN_NIGHT_RGB: [f32; 3] = [1.23, 0.96, 0.69];
/// Moonless night sky luminance, kcd/m² (`Sky::night_luminance`; the renderer's default is
/// 2e-5). About 0.025 after the night exposure: deep blue behind the stars and the
/// roofline, not black.
const NIGHT_SKY_KCD: f32 = 2.4e-4;

/// A sodium cobra head: intensity straight down, kcd (≈ 35 lux on the road 7.5 m below),
/// reach, m, and color (linear, luminance 1, the town palette's sodium)...
const STREETLAMP_KCD: f32 = 2.0;
pub const STREETLAMP_RADIUS_M: f32 = 22.0;
const STREETLAMP_RGB: [f32; 3] = [1.98, 0.79, 0.16];
/// ...shining down in a cone, full to this angle from straight down and cut off by that one,
/// degrees: a pool on the road and the shop fronts beside it, dark upper floors (an omni
/// light lit every facade within reach as brightly as the road)...
const STREETLAMP_INNER_DEG: f32 = 45.0;
const STREETLAMP_OUTER_DEG: f32 = 72.0;
/// ...with a warm halo round the head at night.
const STREETLAMP_CORONA: Corona = Corona {
    radius: 1.6,
    strength: 1.0,
};
/// A neon sign's spill onto the sidewalk and the facade round it, kcd at its brightest
/// channel (the sign's hue), and its halo.
const NEON_KCD: f32 = 0.6;
const NEON_CORONA: Corona = Corona {
    radius: 2.0,
    strength: 0.5,
};
/// A lit storefront's fill on the sidewalk in front of it, kcd at its brightest channel.
const STOREFRONT_KCD: f32 = 0.4;
/// A set piece's flood and tube lights (a tunnel's ceiling lamps, a garage's tubes).
const FLOOD_KCD: f32 = 0.9;
/// The dusk wave lasts about this long, game seconds: every street lamp and sign has
/// switched by then (each warms up a little after)...
pub const WAVE_S: f32 = 20.0;
/// ...the front crossing the place east to west in this much of it...
const FRONT_S: f32 = 14.0;
/// ...a block (this square on the grid, m) a moment of its own within this...
const BLOCK_M: f32 = 90.0;
const BLOCK_S: f32 = 4.0;
/// ...and each lamp within this, by where it stands along a triangle wave this long, m.
const LAMP_S: f32 = 1.0;
const LAMP_PERIOD_M: f32 = 23.0;
/// Signs and landmarks follow the front over this much of the wave, the rest their own.
const SIGN_FRONT_S: f32 = 10.0;
/// A switched lamp strikes, blinking at a third (`STRIKE_HZ` times a second, a quarter of
/// them out), for this long, then warms to full by `WARM_S`, game seconds.
const STRIKE_S: f32 = 1.2;
const STRIKE_HZ: f32 = 6.0;
const STRIKE_LEVEL: f32 = 0.3;
const WARM_S: f32 = 3.0;
const WAVE_SALT: u64 = 0x4455_534b;
const SIGN_SALT: u64 = 0x5349_474e;
/// When more static lights are on than the frame holds, the farthest kept ones fade out
/// over this share of the distance to the first one left out, so none pops as the camera
/// moves.
const STATIC_FADE: f32 = 0.15;

/// The game clock and where on Earth it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeOfDay {
    pub clock: Clock,
    pub latitude_deg: f32,
}

impl Default for TimeOfDay {
    /// 17:40 in late September at 34°N, running at the default 10×.
    fn default() -> TimeOfDay {
        TimeOfDay {
            clock: Clock::new(DEFAULT_DAY_OF_YEAR, DEFAULT_TIME_S),
            latitude_deg: DEFAULT_LATITUDE_DEG,
        }
    }
}

impl TimeOfDay {
    /// One fixed step of `dt` seconds: the clock runs at its speed and `TimeScrub` moves it.
    pub fn step(&mut self, input: &InputState, dt: f64) {
        self.clock.advance(dt);
        let scrub = f64::from(input.axis(Action::TimeScrub));
        if scrub != 0.0 {
            self.clock.scrub(scrub * SCRUB_SECONDS_PER_SECOND * dt);
        }
    }

    pub fn daylight(&self) -> Daylight {
        Daylight::compute(
            self.latitude_deg,
            self.clock.day_of_year,
            self.clock.seconds_of_day as f32,
        )
    }
}

/// The sun as the renderer's directional light: `Daylight`'s tint × lux, in kilolux.
pub fn sun(d: &Daylight) -> Sun {
    Sun {
        direction: d.sun_dir,
        illuminance: d.sun_color.map(|c| c * d.sun_intensity * 1e-3),
    }
}

/// The renderer's sky for `d`: turbidity, moon, stars, the night sky, fog, exposure (EV100
/// to the renderer's kcd/m² scale), the lit-window scale, the emissive cap and the cabin
/// light; the rest stays at the defaults.
pub fn sky(d: &Daylight) -> Sky {
    Sky {
        turbidity: d.turbidity,
        moon_direction: d.moon_dir,
        moon_luminance: if d.moon_intensity > 0.0 {
            MOON_DISC_KCD
        } else {
            0.0
        },
        star_brightness: d.star_brightness,
        night_luminance: NIGHT_SKY_KCD,
        fog_density: d.fog_density,
        exposure: exposure_from_ev100(d.ev100),
        emissive_boost: EMISSIVE_BOOST,
        window_emissive_scale: d.window_emissive_scale,
        emissive_cap: EMISSIVE_CAP,
        cabin_light: cabin_light(d),
        ..Sky::default()
    }
}

/// How dark it is for the lights inside the car: 0 by day, 1 from −8° (the lit-window ramp).
fn night(d: &Daylight) -> f32 {
    d.window_emissive_scale
}

/// Ambient radiance inside the cabin, kcd/m²: a fixed post-exposure brightness, neutral by
/// day and warm and dimmer by night, so the dash reads at any hour.
pub fn cabin_light(d: &Daylight) -> [f32; 3] {
    let n = night(d);
    let exposed = CABIN_DAY_EXPOSED + (CABIN_NIGHT_EXPOSED - CABIN_DAY_EXPOSED) * n;
    let k = exposed / exposure_from_ev100(d.ev100);
    core::array::from_fn(|c| (CABIN_DAY_RGB[c] + (CABIN_NIGHT_RGB[c] - CABIN_DAY_RGB[c]) * n) * k)
}

/// Emissive multiplier that keeps a scale-1 white emissive at or under `peak` after
/// exposure: 1 (as authored) by day, dimmed at night.
fn glow_cap(d: &Daylight, peak: f32) -> f32 {
    (peak / (exposure_from_ev100(d.ev100) * EMISSIVE_BOOST)).min(1.0)
}

/// How brightly the car's lamps and instruments glow at `d`: the lamps as built, dimmed at
/// night to [`CAR_LAMP_EXPOSED`] and raised by day to [`CAR_LAMP_DAY_EXPOSED`]; the
/// instruments as built by day and dimmed at night.
pub fn car_glow(d: &Daylight) -> CarGlow {
    let day = CAR_LAMP_DAY_EXPOSED / (exposure_from_ev100(d.ev100) * EMISSIVE_BOOST);
    CarGlow {
        lamps: glow_cap(d, CAR_LAMP_EXPOSED).max(day),
        instruments: glow_cap(d, INSTRUMENT_EXPOSED),
        speedo: 1.0,
    }
}

/// When the lamps switch: the photocell's moment and how long ago it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LampClock {
    /// The photocell: on from civil dusk to civil dawn.
    pub on: bool,
    /// Game seconds since it last switched.
    pub since: f32,
}

impl LampClock {
    pub fn of(d: &Daylight) -> LampClock {
        LampClock {
            on: d.streetlights_on,
            since: d.lamps_switched_s,
        }
    }

    /// Whether any light may be lit: the photocell is on, or the dawn wave is still going.
    pub fn any(&self) -> bool {
        self.on || self.since < WAVE_S
    }

    /// The level every light has once the wave has passed, if it has.
    fn settled(&self) -> Option<f32> {
        if self.on && self.since >= WAVE_S + WARM_S {
            Some(1.0)
        } else if !self.on && self.since >= WAVE_S {
            Some(0.0)
        } else {
            None
        }
    }

    /// How brightly a light that switches `delay` seconds after the photocell shines, 0..1.
    /// Switching on at dusk it strikes, flickering at a third for [`STRIKE_S`] in a pattern
    /// seeded by `seed`, then warms up to full by [`WARM_S`], as sodium does; at dawn it
    /// simply goes out.
    pub fn level(&self, delay: f32, seed: u32) -> f32 {
        if let Some(l) = self.settled() {
            return l;
        }
        let t = self.since - delay;
        if !self.on {
            return if t < 0.0 { 1.0 } else { 0.0 };
        }
        if t < 0.0 {
            0.0
        } else if t < STRIKE_S {
            let blink = skywave_core::hash64(u64::from(seed), &[(t * STRIKE_HZ) as u64]);
            if blink & 3 == 0 {
                0.05
            } else {
                STRIKE_LEVEL
            }
        } else {
            let w = ((t - STRIKE_S) / (WARM_S - STRIKE_S)).min(1.0);
            STRIKE_LEVEL + (1.0 - STRIKE_LEVEL) * w * w * (3.0 - 2.0 * w)
        }
    }

    /// [`LampClock::level`] of the street lamp at `pos` on `wave`.
    pub fn street(&self, wave: &DuskWave, pos: [f32; 3]) -> f32 {
        match self.settled() {
            Some(l) => l,
            None => {
                let (delay, seed) = wave.street(pos);
                self.level(delay, seed)
            }
        }
    }
}

/// The dusk wave over the place: street lamps switch block by block from east to west,
/// following the dark across the grid, signs and landmarks each a moment of their own in
/// the same [`WAVE_S`]. Pure functions of a light's position, so a lamp keeps its moment
/// whether it is an instanced lens, a chunk's light or both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuskWave {
    /// The easternmost and westernmost x the wave crosses, m.
    east: f32,
    west: f32,
}

impl DuskWave {
    /// The wave across lights at x positions `xs` (a wave of zero width when there are none).
    pub fn across(xs: impl IntoIterator<Item = f32>) -> DuskWave {
        let (mut east, mut west) = (f32::NEG_INFINITY, f32::INFINITY);
        for x in xs {
            east = east.max(x);
            west = west.min(x);
        }
        if west > east {
            (east, west) = (0.0, 0.0);
        }
        DuskWave { east, west }
    }

    /// Where the front is at `pos`, 0 at the east edge to 1 at the west.
    fn front(&self, pos: [f32; 3]) -> f32 {
        ((self.east - pos[0]) / (self.east - self.west).max(1.0)).clamp(0.0, 1.0)
    }

    /// When the street lamp at `pos` switches (seconds after the photocell, under
    /// [`WAVE_S`]) and its flicker seed: the front, its block's moment, and its own along
    /// the block. Continuous in `pos` within a block, so a lens and its light (a few
    /// centimetres apart) switch together.
    pub fn street(&self, pos: [f32; 3]) -> (f32, u32) {
        let block = [
            (pos[0] / BLOCK_M).floor() as i32 as u32 as u64,
            (pos[2] / BLOCK_M).floor() as i32 as u32 as u64,
        ];
        let b = skywave_core::hash64(WAVE_SALT, &block);
        // A triangle wave along z: neighbours on a north-south street a moment apart.
        let along = (pos[2] / LAMP_PERIOD_M).rem_euclid(1.0);
        let own = 1.0 - (2.0 * along - 1.0).abs();
        let delay = FRONT_S * self.front(pos) + BLOCK_S * unit(b) + LAMP_S * own;
        (delay, b as u32)
    }

    /// When the sign or landmark light at `pos` switches, and its seed: in the same window,
    /// following the front loosely, each at its own moment.
    pub fn sign(&self, pos: [f32; 3]) -> (f32, u32) {
        let h = skywave_core::hash64(SIGN_SALT, &[cm(pos[0]), cm(pos[1]), cm(pos[2])]);
        let delay = SIGN_FRONT_S * self.front(pos) + (WAVE_S - SIGN_FRONT_S - 1.0) * unit(h);
        (delay, h as u32)
    }
}

/// A coordinate in whole centimetres, as hash input.
fn cm(x: f32) -> u64 {
    (x * 100.0).round() as i32 as u32 as u64
}

/// The top 24 bits of a hash as `0..1`.
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Whether a static light is a street lamp ([`streetlamp`]'s downward spot and halo); the
/// rest are signs, shop fills and set pieces' lights.
fn is_streetlamp(l: &PointLight) -> bool {
    l.direction == [0.0, -1.0, 0.0] && l.corona == STREETLAMP_CORONA
}

/// A street lamp hanging at `head`: a sodium spot shining down, with its corona.
pub fn streetlamp(head: Vec3) -> PointLight {
    PointLight {
        direction: [0.0, -1.0, 0.0],
        cone_cos: [
            STREETLAMP_OUTER_DEG.to_radians().cos(),
            STREETLAMP_INNER_DEG.to_radians().cos(),
        ],
        corona: STREETLAMP_CORONA,
        ..PointLight::omni(
            head.to_array(),
            STREETLAMP_RGB.map(|c| c * STREETLAMP_KCD),
            STREETLAMP_RADIUS_M,
        )
    }
}

/// The renderer's light for one of the place's [`place::Light`]s: street lamps as
/// [`streetlamp`], neon as a small colored omni light with a corona, lit storefronts as a
/// fill light; car headlights are the cars' own business.
pub fn place_light(l: &place::Light) -> Option<PointLight> {
    let hue = Vec3::from(l.color).max(Vec3::ZERO);
    let hue = hue / hue.max_element().max(1e-6);
    let omni = |kcd: f32| PointLight::omni(l.pos, (hue * kcd).to_array(), l.radius.max(0.0));
    match l.kind {
        place::LightKind::Street => Some(streetlamp(Vec3::from(l.pos))),
        place::LightKind::Neon => Some(PointLight {
            corona: NEON_CORONA,
            ..omni(NEON_KCD)
        }),
        place::LightKind::Window => Some(omni(STOREFRONT_KCD)),
        place::LightKind::Headlight => None,
        place::LightKind::Flood => Some(PointLight {
            color: (Vec3::from(l.color).max(Vec3::ZERO) * FLOOD_KCD).to_array(),
            ..PointLight::omni(l.pos, [0.0; 3], l.radius.max(0.0))
        }),
    }
}

/// An axis-aligned box, world space: what a drawn mesh covers, for light culling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    /// The box round `points`, or `None` when there are none.
    pub fn around(points: impl IntoIterator<Item = Vec3>) -> Option<Bounds> {
        points.into_iter().fold(None, |b, p| {
            Some(match b {
                None => Bounds { min: p, max: p },
                Some(b) => Bounds {
                    min: b.min.min(p),
                    max: b.max.max(p),
                },
            })
        })
    }

    /// A cube of half-size `r` round `center`.
    pub fn cube(center: Vec3, r: f32) -> Bounds {
        Bounds {
            min: center - Vec3::splat(r),
            max: center + Vec3::splat(r),
        }
    }

    /// Squared distance from `p` to the box (0 inside).
    pub fn distance_squared(&self, p: Vec3) -> f32 {
        p.clamp(self.min, self.max).distance_squared(p)
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Whether `light` can reach anything in the box: within its radius, and not wholly
    /// behind a spot whose cone is narrower than a hemisphere.
    pub fn reached_by(&self, light: &PointLight) -> bool {
        let p = Vec3::from(light.position);
        if light.radius <= 0.0 || self.distance_squared(p) >= light.radius * light.radius {
            return false;
        }
        self.ahead_of(light) >= 0.0
    }

    /// How far the box reaches in front of a spot narrower than a hemisphere (along its
    /// axis, m; negative when it is wholly behind); infinite for any other light.
    fn ahead_of(&self, light: &PointLight) -> f32 {
        let axis = Vec3::from(light.direction);
        if axis == Vec3::ZERO || light.cone_cos[0] < 0.0 {
            return f32::INFINITY;
        }
        let half = (self.max - self.min) * 0.5;
        (self.center() - Vec3::from(light.position)).dot(axis) + half.dot(axis.abs())
    }
}

/// The [`LIGHTS_PER_DRAW`] of `lights` that light `bounds` most ([`light_score`]), as a
/// light set. When more reach it than fit, each kept light fades out as the first left out
/// closes on it (fully out where they tie), so a light leaving the set as the camera, the
/// car or the frame's lights move never pops off the whole instance at once: they swap at
/// zero. Allocates nothing.
pub fn light_set(lights: &[PointLight], bounds: &Bounds) -> LightSet {
    light_set_ranked(lights, bounds, |_| false)
}

/// Share of a kept light's score within which the first left out fades it out.
const SET_FADE: f32 = 0.5;

/// How strongly `light` lights `bounds`, for ranking: its brightest channel through the
/// renderer's reach window at the box's nearest point, over one plus that distance squared
/// (and a hundredth of the distance to the center squared, which orders lights that are all
/// inside the box), ramped in over the first [`SPOT_RAMP_M`] the box reaches in front of a
/// spot. Zero where it does not reach. Continuous in the light's position, direction and
/// color, so ranks change only by crossing.
pub fn light_score(light: &PointLight, bounds: &Bounds) -> f32 {
    if !bounds.reached_by(light) {
        return 0.0;
    }
    let p = Vec3::from(light.position);
    let d2 = bounds.distance_squared(p);
    let ratio = d2 / (light.radius * light.radius);
    let window = (1.0 - ratio * ratio) * (1.0 - ratio * ratio);
    let lum = light.color[0].max(light.color[1]).max(light.color[2]);
    let ahead = (bounds.ahead_of(light) / SPOT_RAMP_M).min(1.0);
    lum * window * ahead / (1.0 + d2 + 0.01 * bounds.center().distance_squared(p))
}

/// A spot's score ramps in over this much of a box in front of it, m: a car's headlights
/// sweeping round as it turns come into a light tile's set gradually, not at full rank.
const SPOT_RAMP_M: f32 = 10.0;

/// [`light_set`], the lights `second` names (the NPC cars' low beams, lamps still warming
/// up) taking only the slots the others leave: none crowds out a lit street lamp or the
/// player's headlights. Only a light of the same rank fades the kept ones out (a lamp
/// reaching a box whose set is full with a beam in it still takes the beam's slot at once;
/// that needs eight lights there).
pub fn light_set_ranked(
    lights: &[PointLight],
    bounds: &Bounds,
    second: impl Fn(usize) -> bool,
) -> LightSet {
    light_set_among(lights, 0..lights.len().min(MAX_LIGHTS), bounds, second)
}

/// [`light_set_ranked`] over the lights `candidates` names, in ascending order; the same set
/// whenever they include every light that reaches `bounds`.
fn light_set_among(
    lights: &[PointLight],
    candidates: impl IntoIterator<Item = usize>,
    bounds: &Bounds,
    second: impl Fn(usize) -> bool,
) -> LightSet {
    // Insertion into a short sorted array (best first) holding one more than a set: most
    // boxes are reached by a handful of lights. Ties keep the earlier light.
    const KEEP: usize = LIGHTS_PER_DRAW + 1;
    let mut best = [(false, 0.0f32, 0usize); KEEP];
    let mut n = 0;
    // `a` ranks above `b`: not a beam when `b` is, else the higher score.
    type Key = (bool, f32, usize);
    let above = |a: &Key, b: &Key| if a.0 == b.0 { a.1 > b.1 } else { b.0 };
    for i in candidates {
        let key = (second(i), light_score(&lights[i], bounds), i);
        if key.1 <= 0.0 || (n == KEEP && !above(&key, &best[n - 1])) {
            continue;
        }
        let mut k = n.min(KEEP - 1);
        while k > 0 && above(&key, &best[k - 1]) {
            best[k] = best[k - 1];
            k -= 1;
        }
        best[k] = key;
        n = (n + 1).min(KEEP);
    }
    let mut set = LightSet::EMPTY;
    for &(_, _, i) in &best[..n.min(LIGHTS_PER_DRAW)] {
        set.push(i);
    }
    if n < KEEP {
        return set;
    }
    // Each kept light fades as the first left out closes on it, so lights overtaking it
    // together (the car's two headlights, lamps with near-equal scores) hand over smoothly
    // whichever slot it is in.
    let next = best[KEEP - 1];
    for (k, key) in best[..LIGHTS_PER_DRAW].iter().enumerate() {
        if key.0 == next.0 {
            set = set.with_weight(k, (key.1 - next.1) / (SET_FADE * key.1));
        }
    }
    set
}

/// Cells a side of the grid the frame's lights are binned into...
const GRID: usize = 32;
/// ...each at least this square, m (wider when the lights spread farther than the grid).
const GRID_CELL_M: f32 = 32.0;
/// A light is binned this much beyond its reach, m, so rounding never drops one.
const GRID_PAD_M: f32 = 0.05;
const MASK_WORDS: usize = MAX_LIGHTS.div_ceil(64);
/// One bit per light of the frame.
type LightMask = [u64; MASK_WORDS];

/// The frame's lights binned by reach into a grid on the ground (X, Z), so a box scores only
/// the lights in the cells it overlaps: a light that reaches a box reaches its nearest point,
/// which is in a cell both cover. Sized at load.
struct LightGrid {
    cells: Vec<LightMask>,
    origin: [f32; 2],
    cell: f32,
    /// Off when there is nothing to bin or a light's reach is not finite: every box then
    /// scores every light.
    on: bool,
}

impl LightGrid {
    fn new() -> LightGrid {
        LightGrid {
            cells: vec![[0; MASK_WORDS]; GRID * GRID],
            origin: [0.0; 2],
            cell: GRID_CELL_M,
            on: false,
        }
    }

    /// The cell holding `x` along axis `a` (0 X, 1 Z), clamped to the grid.
    fn index(&self, a: usize, x: f32) -> usize {
        (((x - self.origin[a]) / self.cell).max(0.0) as usize).min(GRID - 1)
    }

    /// The cells `lo..=hi` covers, as index ranges along X and Z.
    fn span(&self, lo: [f32; 2], hi: [f32; 2]) -> [core::ops::RangeInclusive<usize>; 2] {
        [0, 1].map(|a| self.index(a, lo[a])..=self.index(a, hi[a]))
    }

    fn bin(&mut self, lights: &[PointLight]) {
        self.on = false;
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for l in lights
            .iter()
            .filter(|l| l.radius.is_nan() || l.radius > 0.0)
        {
            let r = l.radius + GRID_PAD_M;
            let p = [l.position[0], l.position[2]];
            if !(r.is_finite() && p[0].is_finite() && p[1].is_finite()) {
                return;
            }
            for a in 0..2 {
                lo[a] = lo[a].min(p[a] - r);
                hi[a] = hi[a].max(p[a] + r);
            }
        }
        if !lo[0].is_finite() {
            return;
        }
        let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        self.origin = lo;
        self.cell = GRID_CELL_M.max(extent / GRID as f32);
        self.cells.fill([0; MASK_WORDS]);
        for (i, l) in lights.iter().enumerate().filter(|(_, l)| l.radius > 0.0) {
            let r = l.radius + GRID_PAD_M;
            let p = [l.position[0], l.position[2]];
            let [xs, zs] = self.span([p[0] - r, p[1] - r], [p[0] + r, p[1] + r]);
            for z in zs {
                for x in xs.clone() {
                    self.cells[z * GRID + x][i / 64] |= 1 << (i % 64);
                }
            }
        }
        self.on = true;
    }

    /// The lights that may reach `bounds`, or `None` when every light may.
    fn candidates(&self, bounds: &Bounds) -> Option<LightMask> {
        if !(self.on && bounds.min.is_finite() && bounds.max.is_finite()) {
            return None;
        }
        let [xs, zs] = self.span([bounds.min.x, bounds.min.z], [bounds.max.x, bounds.max.z]);
        let mut mask = [0; MASK_WORDS];
        for z in zs {
            for c in &self.cells[z * GRID..][xs.clone()] {
                for (m, w) in mask.iter_mut().zip(c) {
                    *m |= w;
                }
            }
        }
        Some(mask)
    }
}

/// The lights `mask` names, in ascending order.
fn mask_lights(mask: LightMask) -> impl Iterator<Item = usize> {
    mask.into_iter().enumerate().flat_map(|(w, mut bits)| {
        core::iter::from_fn(move || {
            let b = bits.trailing_zeros() as usize;
            bits &= bits.wrapping_sub(1);
            (b < 64).then_some(w * 64 + b)
        })
    })
}

/// The frame's lights and the light set of every instance (see the module docs).
pub struct FrameLights {
    /// The place's static lights: street lamps, neon, storefront fill.
    statics: Vec<PointLight>,
    /// Which statics shine (a streamed chunk's lights only while it is drawn).
    active: Vec<bool>,
    /// Each static's moment in the dusk wave (seconds after the photocell) and seed.
    switch: Vec<(f32, u32)>,
    /// Scratch: each static's level this frame (0 when out or inactive).
    level: Vec<f32>,
    wave: DuskWave,
    /// Scratch for picking the nearest statics: squared distance to the camera and index.
    order: Vec<(f32, u32)>,
    lights: [PointLight; MAX_LIGHTS],
    len: usize,
    /// The lights that only fill slots the others leave ([`light_set_ranked`]): these, and
    /// every one from `dim`, the statics still striking or warming up.
    second: core::ops::Range<usize>,
    dim: usize,
    grid: LightGrid,
}

impl FrameLights {
    pub fn new(statics: Vec<PointLight>) -> FrameLights {
        let wave = DuskWave::across(statics.iter().map(|l| l.position[0]));
        let switch = statics
            .iter()
            .map(|l| {
                if is_streetlamp(l) {
                    wave.street(l.position)
                } else {
                    wave.sign(l.position)
                }
            })
            .collect();
        FrameLights {
            order: Vec::with_capacity(statics.len()),
            active: vec![true; statics.len()],
            level: vec![0.0; statics.len()],
            switch,
            wave,
            statics,
            lights: [PointLight::omni([0.0; 3], [0.0; 3], 0.0); MAX_LIGHTS],
            len: 0,
            second: 0..0,
            dim: MAX_LIGHTS,
            grid: LightGrid::new(),
        }
    }

    /// The place's static lights.
    pub fn statics(&self) -> &[PointLight] {
        &self.statics
    }

    /// Switches each static light `i` on or off as `on(i)` says (all start on).
    pub fn set_active(&mut self, on: impl Fn(usize) -> bool) {
        for (i, a) in self.active.iter_mut().enumerate() {
            *a = on(i);
        }
    }

    /// The dusk wave across the place's lights, for the lamp lenses drawn apart from them.
    pub fn wave(&self) -> &DuskWave {
        &self.wave
    }

    /// Capacity of the scratch buffer, which must not grow after load.
    pub fn order_capacity(&self) -> usize {
        self.order.capacity()
    }

    /// This frame's lights, as last gathered.
    pub fn lights(&self) -> &[PointLight] {
        &self.lights[..self.len]
    }

    /// This frame's lights: `dynamic` (the car's), then the static ones `lamps` has lit
    /// (each at its moment in the dusk wave, dimmed by its level), all of them when they
    /// fit, else those nearest `eye`, the farthest fading out toward the first left out.
    pub fn gather(&mut self, eye: Vec3, lamps: LampClock, dynamic: &[PointLight]) {
        self.collect(eye, lamps, dynamic);
        self.grid.bin(&self.lights[..self.len]);
    }

    fn collect(&mut self, eye: Vec3, lamps: LampClock, dynamic: &[PointLight]) {
        self.second = 0..0;
        self.dim = MAX_LIGHTS;
        let n = dynamic.len().min(MAX_LIGHTS);
        self.lights[..n].copy_from_slice(&dynamic[..n]);
        self.len = n;
        if !lamps.any() {
            return;
        }
        let mut lit = 0;
        for (i, level) in self.level.iter_mut().enumerate() {
            *level = if self.active[i] {
                let (delay, seed) = self.switch[i];
                lamps.level(delay, seed)
            } else {
                0.0
            };
            lit += usize::from(*level > 0.0);
        }
        let dim = |l: &PointLight, w: f32| {
            let mut l = *l;
            l.color = l.color.map(|c| c * w);
            l.corona.strength *= w;
            l
        };
        let room = MAX_LIGHTS - n;
        // Full-strength lights first, then those warming up, which rank second.
        if lit <= room {
            for warming in [false, true] {
                if warming {
                    self.dim = self.len;
                }
                for (l, &w) in self.statics.iter().zip(&self.level) {
                    if w > 0.0 && (w < 1.0) == warming {
                        self.lights[self.len] = dim(l, w);
                        self.len += 1;
                    }
                }
            }
            return;
        }
        let level = &self.level;
        self.order.clear();
        self.order.extend(
            self.statics
                .iter()
                .enumerate()
                .filter(|&(i, _)| level[i] > 0.0)
                .map(|(i, l)| (Vec3::from(l.position).distance_squared(eye), i as u32)),
        );
        // Ties keep the earlier light, so the choice is deterministic.
        let by_distance = |a: &(f32, u32), b: &(f32, u32)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1));
        let (kept, cut, _) = self.order.select_nth_unstable_by(room, by_distance);
        let cut = cut.0.sqrt();
        for warming in [false, true] {
            if warming {
                self.dim = self.len;
            }
            for &(d2, i) in kept.iter() {
                let i = i as usize;
                if (level[i] < 1.0) != warming {
                    continue;
                }
                let w = ((cut - d2.sqrt()) / (STATIC_FADE * cut)).clamp(0.0, 1.0);
                self.lights[self.len] = dim(&self.statics[i], w * level[i]);
                self.len += 1;
            }
        }
    }

    /// Ranks the gathered lights in `second` below the rest in every light set (the NPC
    /// cars' low beams, among the dynamic lights). Call after [`FrameLights::gather`].
    pub fn set_second(&mut self, second: core::ops::Range<usize>) {
        self.second = second.start.min(self.len)..second.end.min(self.len);
    }

    /// The light set of an instance covering `bounds` ([`light_set_ranked`]).
    pub fn set_for(&self, bounds: &Bounds) -> LightSet {
        let (second, dim) = (self.second.clone(), self.dim);
        let second = |i| second.contains(&i) || i >= dim;
        match self.grid.candidates(bounds) {
            Some(mask) => light_set_among(self.lights(), mask_lights(mask), bounds, second),
            None => light_set_ranked(self.lights(), bounds, second),
        }
    }

    /// Sets each instance's light set from its bounds (`None`: lit by the sun and sky only).
    /// `bounds` and `instances` pair up; extra instances are left alone.
    pub fn assign(&self, bounds: &[Option<Bounds>], instances: &mut [MeshInstance]) {
        for (inst, b) in instances.iter_mut().zip(bounds) {
            inst.lights = b.map_or(LightSet::EMPTY, |b| self.set_for(&b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daylight::Phase;

    #[test]
    fn default_is_golden_hour_with_the_sun_in_the_west() {
        let d = TimeOfDay::default().daylight();
        assert_eq!(d.phase, Phase::GoldenHour, "{}", d.sun_elevation_deg);
        assert!((240.0..300.0).contains(&d.sun_azimuth_deg));
        let sun = sun(&d);
        assert!(sun.direction[0] < -0.9, "west is -x");
        // Warm and in the renderer's golden-hour range (10–30 klx).
        let [r, g, b] = sun.illuminance;
        assert!(r > g && g > b, "{:?}", sun.illuminance);
        assert!((5.0..40.0).contains(&r), "{r}");
        let sky = sky(&d);
        assert_eq!(sky.exposure, exposure_from_ev100(d.ev100));
        assert!(
            sky.exposure > Sky::default().exposure,
            "exposed up from noon"
        );
        assert_eq!(sky.window_emissive_scale, 0.0);
    }

    #[test]
    fn clock_runs_at_ten_times_and_scrubs_an_hour_a_second() {
        let mut t = TimeOfDay::default();
        let mut input = InputState::default();
        let dt = 1.0 / 120.0;
        for _ in 0..120 {
            t.step(&input, dt);
        }
        let ran = t.clock.seconds_of_day - DEFAULT_TIME_S;
        assert!((ran - 10.0).abs() < 1e-6, "{ran}");
        input.set(Action::TimeScrub, 1.0);
        for _ in 0..120 {
            t.step(&input, dt);
        }
        let scrubbed = t.clock.seconds_of_day - DEFAULT_TIME_S - ran;
        assert!((scrubbed - 3610.0).abs() < 1e-6, "{scrubbed}");
        input.set(Action::TimeScrub, -1.0);
        for _ in 0..240 {
            t.step(&input, dt);
        }
        assert!(t.clock.seconds_of_day < DEFAULT_TIME_S);
    }

    #[test]
    fn sun_is_below_the_horizon_at_eleven_at_night() {
        let t = TimeOfDay {
            clock: Clock::new(DEFAULT_DAY_OF_YEAR, 23.0 * 3600.0),
            ..TimeOfDay::default()
        };
        let d = t.daylight();
        let sun = sun(&d);
        assert_eq!(sun.direction, d.sun_dir);
        assert!(sun.direction[1] < 0.0);
        assert_eq!(sun.illuminance, [0.0; 3]);
        let sky = sky(&d);
        assert!(sky.window_emissive_scale > 0.99);
        assert!(sky.star_brightness > 0.5);
    }

    fn at(hour: f64) -> Daylight {
        TimeOfDay {
            clock: Clock::new(DEFAULT_DAY_OF_YEAR, hour * 3600.0),
            ..TimeOfDay::default()
        }
        .daylight()
    }

    #[test]
    fn the_cabin_is_lit_by_day_and_dim_at_night() {
        let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let exposed = |d: &Daylight| luma(cabin_light(d)) * exposure_from_ev100(d.ev100);
        let (noon, golden, night) = (at(12.0), at(17.0 + 40.0 / 60.0), at(22.5));
        assert!((exposed(&noon) - CABIN_DAY_EXPOSED).abs() < 0.02 * CABIN_DAY_EXPOSED);
        assert!((exposed(&golden) - CABIN_DAY_EXPOSED).abs() < 0.05 * CABIN_DAY_EXPOSED);
        assert!((exposed(&night) - CABIN_NIGHT_EXPOSED).abs() < 0.02);
        // Warm at night, neutral by day; in the sky it rides along.
        let [r, _, b] = cabin_light(&night);
        assert!(r > 1.5 * b);
        assert_eq!(sky(&night).cabin_light, cabin_light(&night));
        assert_eq!(sky(&night).emissive_cap, EMISSIVE_CAP);
    }

    #[test]
    fn lamps_glow_alike_day_and_night_and_instruments_dim_at_night() {
        let day_d = at(12.0);
        let day = car_glow(&day_d);
        assert_eq!(day.instruments, 1.0);
        let e = exposure_from_ev100(day_d.ev100) * EMISSIVE_BOOST;
        assert!((day.lamps * e - CAR_LAMP_DAY_EXPOSED).abs() < 1e-3);
        assert!(day.lamps > 10.0, "the sun needs {}", day.lamps);
        let night_d = at(22.5);
        let night = car_glow(&night_d);
        let e = exposure_from_ev100(night_d.ev100) * EMISSIVE_BOOST;
        assert!((night.instruments * e - INSTRUMENT_EXPOSED).abs() < 1e-4);
        assert!((night.lamps * e - CAR_LAMP_EXPOSED).abs() < 1e-3);
        assert!(night.instruments < night.lamps && night.lamps < 1.0);
    }

    #[test]
    fn street_lamps_switch_with_the_photocell() {
        let glow = |d: &Daylight| LampClock::of(d).level(WAVE_S - 1.0, 7);
        assert_eq!(glow(&at(12.0)), 0.0);
        assert_eq!(glow(&at(17.0 + 40.0 / 60.0)), 0.0);
        // Civil dusk is about 18:30 in late September; the dusk scene at 18:35 has them on.
        assert_eq!(glow(&at(18.0 + 35.0 / 60.0)), 1.0);
        assert_eq!(glow(&at(22.5)), 1.0);
        assert_eq!(glow(&at(5.0)), 1.0);
        assert!(!LampClock::of(&at(12.0)).any());
        // The windows are already most of the way up when the lamps come on.
        assert!(at(18.0 + 35.0 / 60.0).window_emissive_scale > 0.9);
    }

    /// The photocell long on (every light lit) and long off.
    const ON: LampClock = LampClock {
        on: true,
        since: 3600.0,
    };
    const OFF: LampClock = LampClock {
        on: false,
        since: 3600.0,
    };

    /// A grid of street lamps 30 m apart over 1.5 km by 600 m.
    fn lamp_grid() -> Vec<PointLight> {
        let mut v = Vec::new();
        for i in 0..50 {
            for j in 0..20 {
                // Off the 90 m block lines, as lamps by the roads are.
                let (x, z) = (7.0 + i as f32 * 30.0, 11.0 + j as f32 * 30.0);
                v.push(streetlamp(Vec3::new(x, 7.5, z)));
            }
        }
        v
    }

    #[test]
    fn the_lamps_come_on_in_a_wave_east_to_west_over_twenty_seconds() {
        let lamps = lamp_grid();
        let wave = DuskWave::across(lamps.iter().map(|l| l.position[0]));
        let at = |since: f32| LampClock { on: true, since };
        let lit = |since: f32| {
            lamps
                .iter()
                .filter(|l| at(since).street(&wave, l.position) > 0.0)
                .count()
        };
        // None at the photocell's moment, some partway, all by the end of the wave.
        assert_eq!(lit(0.0), 0);
        let mid = lit(0.5 * WAVE_S);
        assert!(mid > lamps.len() / 5 && mid < 4 * lamps.len() / 5, "{mid}");
        assert_eq!(lit(WAVE_S), lamps.len());
        for l in &lamps {
            let (delay, _) = wave.street(l.position);
            assert!((0.0..WAVE_S).contains(&delay));
            assert_eq!(at(WAVE_S + WARM_S).street(&wave, l.position), 1.0);
        }
        // The front starts in the east (+x) and the blocks keep it ragged, not a line.
        let mean = |range: core::ops::Range<f32>| {
            let ds: Vec<f32> = lamps
                .iter()
                .filter(|l| range.contains(&l.position[0]))
                .map(|l| wave.street(l.position).0)
                .collect();
            ds.iter().sum::<f32>() / ds.len() as f32
        };
        assert!(mean(1200.0..1500.0) + 8.0 < mean(0.0..300.0));
        let column: Vec<f32> = (0..20)
            .map(|j| wave.street([750.0, 7.5, j as f32 * 30.0]).0)
            .collect();
        let (lo, hi) = column
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &d| (a.min(d), b.max(d)));
        assert!(hi - lo > 1.5, "one street, one moment: {lo}..{hi}");
        // A lens and its light a few centimetres apart switch together.
        for l in &lamps {
            let p = l.position;
            let near = [p[0] + 0.03, p[1] - 0.2, p[2] - 0.03];
            assert!((wave.street(p).0 - wave.street(near).0).abs() < 0.05);
        }
        // Signs come on in the same window, each at its own moment.
        let signs: Vec<f32> = (0..200)
            .map(|i| wave.sign([i as f32 * 7.0, 4.0, 13.0]).0)
            .collect();
        assert!(signs.iter().all(|d| (0.0..WAVE_S).contains(d)));
        assert!(signs.windows(2).filter(|w| w[0] == w[1]).count() == 0);
    }

    #[test]
    fn a_lamp_strikes_flickering_then_warms_up_and_goes_out_at_dawn() {
        let on = |since: f32| LampClock { on: true, since };
        let delay = 5.0;
        let mut seen = Vec::new();
        let mut t = 0.0;
        while t < delay + WAVE_S {
            seen.push(on(t).level(delay, 1234));
            t += 1.0 / 12.0;
        }
        assert!(seen.iter().all(|l| (0.0..=1.0).contains(l)));
        assert_eq!(seen[(delay * 12.0) as usize - 1], 0.0);
        // It flickers while it strikes: dips after it first lit.
        let strike = &seen[(delay * 12.0) as usize..((delay + STRIKE_S) * 12.0) as usize];
        assert!(strike.iter().any(|&l| l < 0.1) && strike.iter().any(|&l| l > 0.2));
        // Then climbs steadily to full.
        let warm = &seen[((delay + STRIKE_S) * 12.0) as usize + 1..];
        assert!(warm.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(*warm.last().unwrap(), 1.0);
        // At dawn it is on until its moment, then out.
        let off = |since: f32| LampClock { on: false, since };
        assert_eq!(off(0.0).level(delay, 1), 1.0);
        assert_eq!(off(delay + 0.01).level(delay, 1), 0.0);
        assert!(off(0.0).any() && !off(WAVE_S).any());
    }

    #[test]
    fn mid_wave_the_frame_holds_only_the_lit_lights_dimmed_by_their_level() {
        let mut frame = FrameLights::new(lamp_grid());
        let n = frame.statics().len();
        let mid = LampClock {
            on: true,
            since: 0.5 * WAVE_S,
        };
        frame.gather(Vec3::new(750.0, 1.2, 300.0), mid, &[]);
        let lit = frame.lights().len();
        assert!(lit > 0 && lit <= MAX_LIGHTS.min(n));
        let full = STREETLAMP_RGB[0] * STREETLAMP_KCD;
        assert!(frame.lights().iter().all(|l| l.color[0] <= full + 1e-5));
        assert!(frame.lights().iter().any(|l| l.color[0] < 0.5 * full));
        frame.gather(Vec3::ZERO, OFF, &[]);
        assert!(frame.lights().is_empty());
        frame.gather(Vec3::ZERO, ON, &[]);
        assert_eq!(frame.lights().len(), MAX_LIGHTS.min(n));
        // Lamps still warming up take only the slots the rest leave: a headlight far off
        // in a box crowded with them keeps its slot.
        let eye = Vec3::new(750.0, 1.2, 300.0);
        let head = PointLight::omni([eye.x - 60.0, 0.7, eye.z], [10.0; 3], 80.0);
        let early = LampClock {
            on: true,
            since: 10.0,
        };
        frame.gather(eye, early, &[head]);
        let warming = frame.lights()[1..]
            .iter()
            .filter(|l| l.color[0] < 0.99 * full)
            .count();
        assert!(warming > 8, "{warming}");
        let crowded = Bounds::cube(eye, 30.0);
        assert!(frame.set_for(&crowded).indices().contains(&0));
    }

    fn lamps_along_x(count: usize, spacing: f32) -> Vec<PointLight> {
        (0..count)
            .map(|i| streetlamp(Vec3::new(i as f32 * spacing, 7.5, 0.0)))
            .collect()
    }

    #[test]
    fn a_street_lamp_is_a_sodium_spot_with_a_halo() {
        let l = streetlamp(Vec3::new(1.0, 7.5, 2.0));
        assert_eq!(l.position, [1.0, 7.5, 2.0]);
        assert_eq!(l.direction, [0.0, -1.0, 0.0]);
        assert!(l.cone_cos[0] < l.cone_cos[1]);
        assert!(l.color[0] > l.color[1] && l.color[1] > l.color[2]);
        assert!(l.corona.radius > 0.5 && l.corona.strength > 0.0);
        // The place's own lights map onto the same kinds.
        let neon = place::Light {
            pos: [0.0, 4.0, 0.0],
            color: [0.5, 0.1, 1.0],
            radius: 6.0,
            kind: place::LightKind::Neon,
        };
        let n = place_light(&neon).unwrap();
        assert_eq!(n.radius, 6.0);
        assert!((n.color[2] - NEON_KCD).abs() < 1e-6 && n.corona.radius > 0.0);
        let shop = place_light(&place::Light {
            kind: place::LightKind::Window,
            ..neon.clone()
        })
        .unwrap();
        assert_eq!(shop.corona, Corona::NONE);
        let street = place::Light {
            kind: place::LightKind::Street,
            ..neon.clone()
        };
        assert_eq!(
            place_light(&street),
            Some(streetlamp(Vec3::new(0.0, 4.0, 0.0)))
        );
        let car = place::Light {
            kind: place::LightKind::Headlight,
            ..neon
        };
        assert_eq!(place_light(&car), None);
    }

    #[test]
    fn every_static_light_is_on_when_they_fit() {
        let mut frame = FrameLights::new(lamps_along_x(100, 20.0));
        let head = PointLight::omni([0.0; 3], [1.0; 3], 10.0);
        frame.gather(Vec3::ZERO, OFF, &[head, head]);
        assert_eq!(frame.lights().len(), 2, "photocell off: the car's only");
        frame.gather(Vec3::ZERO, ON, &[head, head]);
        assert_eq!(frame.lights().len(), 102);
        assert_eq!(frame.lights()[2..], frame.statics()[..]);
    }

    #[test]
    fn past_the_frame_budget_the_nearest_statics_stay_and_fade_out_smoothly() {
        let count = 400;
        let mut frame = FrameLights::new(lamps_along_x(count, 25.0));
        let dynamic = [PointLight::omni([0.0; 3], [1.0; 3], 10.0); 4];
        let capacity = frame.order.capacity();
        let mut prev = vec![0.0f32; count];
        for step in 0..=400 {
            let eye = Vec3::new(4000.0 + step as f32 * 0.5, 1.2, 0.0);
            frame.gather(eye, ON, &dynamic);
            let lights = frame.lights();
            assert_eq!(lights.len(), MAX_LIGHTS);
            let mut now = vec![0.0f32; count];
            for l in &lights[4..] {
                now[(l.position[0] / 25.0).round() as usize] = l.color[0];
            }
            // The kept ones are the nearest: the near lamps are at full strength.
            let near = (eye.x / 25.0).round() as usize;
            assert!((now[near] - STREETLAMP_RGB[0] * STREETLAMP_KCD).abs() < 1e-5);
            if step > 0 {
                for k in 0..count {
                    assert!((now[k] - prev[k]).abs() < 0.1, "lamp {k} at x {}", eye.x);
                }
            }
            prev = now;
        }
        assert_eq!(frame.order.capacity(), capacity, "no allocation per frame");
    }

    #[test]
    fn instances_get_the_lights_that_light_them_most() {
        let mut lights = lamps_along_x(30, 10.0);
        // A headlight at x 100 pointing +x, and an unlit tail lamp corona.
        lights.push(PointLight {
            direction: [1.0, 0.0, 0.0],
            cone_cos: [0.8, 0.95],
            ..PointLight::omni([100.0, 0.7, 0.0], [10.0; 3], 60.0)
        });
        lights.push(PointLight::omni([0.0, 0.7, 0.0], [1.0; 3], 0.0));
        let small = Bounds::cube(Vec3::new(45.0, 1.0, 3.0), 1.0);
        let set = light_set(&lights, &small);
        // Lamps within 22 m of the box, nearest first (40 and 50 tie on the box distance
        // and center; the earlier wins); those 20 m off are out of reach.
        let got: Vec<u8> = set.indices().to_vec();
        assert_eq!(got, [4, 5, 3, 6]);
        // Behind the headlight nothing is lit by it; ahead it is.
        let behind = Bounds::cube(Vec3::new(90.0, 1.0, 0.0), 2.0);
        assert!(!light_set(&lights, &behind).indices().contains(&30));
        let ahead = Bounds::cube(Vec3::new(130.0, 1.0, 0.0), 2.0);
        assert!(light_set(&lights, &ahead).indices().contains(&30));
        // A big box reached by more than fit keeps the nearest 8; the zero-radius corona
        // light never lights anything.
        let big = Bounds {
            min: Vec3::new(-5.0, 0.0, -5.0),
            max: Vec3::new(300.0, 10.0, 5.0),
        };
        let set = light_set(&lights, &big);
        assert_eq!(set.len(), LIGHTS_PER_DRAW);
        assert!(!set.indices().contains(&31));
        // Brute force agrees on a grid of boxes.
        for x in 0..40 {
            for z in 0..4 {
                let b = Bounds::cube(Vec3::new(x as f32 * 8.0, 2.0, z as f32 * 9.0), 3.0);
                let mut reach: Vec<usize> = (0..lights.len())
                    .filter(|&i| b.reached_by(&lights[i]))
                    .collect();
                let score = |i: &usize| light_score(&lights[*i], &b);
                reach.sort_by(|x, y| score(y).total_cmp(&score(x)).then(x.cmp(y)));
                reach.truncate(LIGHTS_PER_DRAW);
                let set = light_set(&lights, &b);
                let got: Vec<usize> = set.indices().iter().map(|&i| usize::from(i)).collect();
                assert_eq!(got, reach, "box at {x}, {z}");
            }
        }
        // Instances get their sets; those without bounds none.
        let mut frame = FrameLights::new(lamps_along_x(30, 10.0));
        frame.gather(Vec3::ZERO, ON, &[]);
        let mut instances =
            [MeshInstance::new(render::MeshId(0), glam::Mat4::IDENTITY.to_cols_array_2d()); 3];
        instances[1].lights = LightSet::from_indices(&[1]);
        frame.assign(&[Some(small), None], &mut instances);
        assert_eq!(instances[0].lights.len(), 4);
        assert!(instances[1].lights.is_empty());
        assert_eq!(instances[2].lights, LightSet::EMPTY);
    }

    /// The grid of lights picks exactly what scoring every light picks: every piece of the
    /// coast at night with busy traffic (the NPC cars' beams ranking second), frame after
    /// frame as the car drives up the strip.
    #[test]
    fn the_light_grid_picks_what_scoring_every_light_picks() {
        use crate::{App, InputState, SceneSetup};
        let query = "place=coast&t=22:30&lights=1&stream=0&car=gannet_gt&traffic=busy";
        let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
        let mut input = InputState::default();
        input.set(Action::Throttle, 0.6);
        let (mut lit, mut full, mut frames) = (0, 0, 0);
        let (mut scoring, mut binned) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for frame in 0..240 {
            app.update(&input, 1.0 / 60.0);
            app.scene_view(1.0, 16.0 / 9.0);
            if frame % 20 != 0 {
                continue;
            }
            let l = &app.lights;
            assert!(l.grid.on && l.lights().len() > 100, "{}", l.lights().len());
            let (second, dim) = (l.second.clone(), l.dim);
            let second = |i| second.contains(&i) || i >= dim;
            let started = std::time::Instant::now();
            let brute: Vec<_> = app
                .bounds
                .iter()
                .flatten()
                .map(|b| light_set_ranked(l.lights(), b, second))
                .collect();
            scoring += started.elapsed();
            let started = std::time::Instant::now();
            let grid: Vec<_> = app.bounds.iter().flatten().map(|b| l.set_for(b)).collect();
            binned += started.elapsed();
            for ((g, s), b) in grid.iter().zip(&brute).zip(app.bounds.iter().flatten()) {
                assert_eq!(g, s, "{b:?}");
                lit += usize::from(!s.is_empty());
                full += usize::from(s.len() == LIGHTS_PER_DRAW);
            }
            frames += 1;
        }
        eprintln!(
            "{} boxes: every light {:?}, the grid {:?} a frame",
            app.bounds.iter().flatten().count(),
            scoring / frames,
            binned / frames
        );
        assert!(lit > 1000 && full > 100, "{lit} lit, {full} full");
    }

    /// The weight each of `count` lights has in `set` (0 out of it).
    fn weights(set: &LightSet, count: usize) -> Vec<f32> {
        let mut w = vec![0.0; count];
        for (k, &i) in set.indices().iter().enumerate() {
            w[usize::from(i)] = set.weight(k);
        }
        w
    }

    /// A box reached by more lights than a set holds, and lights moving past it (a car's
    /// lamps, or the camera moving the frame's lights): no light's share of the box jumps
    /// from one frame to the next. Ranked by distance alone they swapped in one frame, and
    /// a street lamp's pool switched off across a whole light tile (the web flicker).
    #[test]
    fn a_full_light_set_hands_over_without_popping() {
        // Two lights moving together, like a car's headlights, overtake lamps together.
        let mut lights = lamps_along_x(14, 10.0);
        lights.push(PointLight::omni([0.0, 6.0, 4.0], [3.0; 3], 22.0));
        lights.push(PointLight::omni([0.0, 6.0, 5.2], [3.0; 3], 22.0));
        let moving = lights.len() - 2;
        let tile = Bounds {
            min: Vec3::new(10.0, 0.0, -6.0),
            max: Vec3::new(110.0, 8.0, 6.0),
        };
        let mut prev: Option<Vec<f32>> = None;
        let (mut swaps, mut faded) = (0, 0);
        // 60 frames a second at 20 m/s.
        for step in 0..=3600 {
            lights[moving].position[0] = -40.0 + step as f32 / 3.0;
            lights[moving + 1].position[0] = -40.0 + step as f32 / 3.0;
            let set = light_set(&lights, &tile);
            assert_eq!(set.len(), LIGHTS_PER_DRAW, "more reach it than fit");
            let w = weights(&set, lights.len());
            faded += usize::from(set.weight(LIGHTS_PER_DRAW - 1) < 1.0);
            if let Some(p) = &prev {
                for (k, (a, b)) in p.iter().zip(&w).enumerate() {
                    assert!(
                        (a - b).abs() < 0.2,
                        "light {k} jumps {a} -> {b} at step {step}"
                    );
                    swaps += usize::from((*a > 0.0) != (*b > 0.0));
                }
            }
            prev = Some(w);
        }
        // They come into the set and leave it again, fading in and out: no frame moves any
        // light's share by a fifth (the swap moved two lights by all of it).
        assert!(
            swaps >= 2 && faded > 100,
            "{swaps} swaps, {faded} frames fading"
        );
    }

    /// The web flicker, measured: driving up the coast's strip at night for ten seconds,
    /// every static light's light on every lit piece of the town (its share there, the
    /// frame's fade times its weight in the piece's set, times a bound on what it gives
    /// there) never steps by 25 lux from one 60 Hz frame to the next, and by 10 lux only a
    /// few times, in the middle of a fade. Ranked by distance alone, a lamp at full strength
    /// left a full set in one frame whenever the car's headlights or a lamp fading into the
    /// frame's lights came nearer: 12 steps over 10 lux in the same drive, the worst a
    /// whole lamp (70 lux) switching off.
    #[test]
    fn driving_at_night_no_lamp_pops_on_the_town() {
        use crate::{App, InputState, SceneSetup};
        use std::collections::BTreeMap;
        let query = "place=coast&t=22:30&lights=1&stream=0&car=gannet_gt&traffic=busy";
        let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
        let key = |l: &PointLight| l.position.map(f32::to_bits);
        let statics: BTreeMap<_, _> = app.lights.statics().iter().map(|l| (key(l), *l)).collect();
        let bounds: Vec<_> = app.bounds[..app.world_instances].to_vec();
        let mut input = InputState::default();
        input.set(Action::Throttle, 0.6);
        // Per piece: each static light's share of it (frame fade times set weight).
        let mut prev: Vec<BTreeMap<[u32; 3], f32>> = Vec::new();
        let (mut worst, mut lit, mut steps) = (0.0f32, 0usize, 0usize);
        for _ in 0..600 {
            app.update(&input, 1.0 / 60.0);
            let view = app.scene_view(1.0, 16.0 / 9.0);
            let shares: Vec<BTreeMap<[u32; 3], f32>> = view.instances[..bounds.len()]
                .iter()
                .map(|inst| {
                    let set = inst.lights;
                    let mut m = BTreeMap::new();
                    for (k, &i) in set.indices().iter().enumerate() {
                        let l = &view.lights[usize::from(i)];
                        if let Some(s) = statics.get(&key(l)) {
                            m.insert(key(l), set.weight(k) * l.color[0] / s.color[0]);
                        }
                    }
                    m
                })
                .collect();
            lit += shares.iter().map(|m| m.len()).sum::<usize>();
            for ((a, b), piece) in prev.iter().zip(&shares).zip(&bounds) {
                let Some(piece) = piece else { continue };
                for k in a.keys().chain(b.keys()) {
                    let jump = (a.get(k).unwrap_or(&0.0) - b.get(k).unwrap_or(&0.0)).abs();
                    // Red (sodium's brightest) at the piece's nearest point, the lamp no
                    // nearer than its mast's height (7.5 m), klux.
                    let l = &statics[k];
                    let d2 = piece.distance_squared(Vec3::from(l.position));
                    let ratio = d2 / (l.radius * l.radius);
                    let at = l.color[0] * (1.0 - ratio * ratio).max(0.0).powi(2) / d2.max(56.0);
                    worst = worst.max(jump * at);
                    steps += usize::from(jump * at > 0.01);
                }
            }
            prev = shares;
        }
        assert!(
            app.hud_state().speed_mps > 15.0,
            "{}",
            app.hud_state().speed_mps
        );
        assert!(lit > 100_000, "the town is lit: {lit}");
        assert!(
            worst < 0.025,
            "a lamp's light on a piece of town jumped {} lux",
            worst * 1000.0
        );
        assert!(steps <= 6, "{steps} steps over 10 lux");
    }
}
