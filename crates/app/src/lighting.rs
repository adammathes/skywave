//! The game's time of day and how [`Daylight`] becomes the renderer's [`Sun`] and [`Sky`],
//! and the lights that follow it: the street lamps' photocell and pools, the glow of the
//! car's lamps and instruments, and the light inside the cabin.
//!
//! **Light lists.** The renderer shades each instance with the lights its
//! [`render::LightSet`] names, up to [`LIGHTS_PER_DRAW`] of the frame's [`MAX_LIGHTS`].
//! [`FrameLights`] fills both once a frame: the car's own lights first, then every static
//! light of the place (street lamps, neon, storefront fill) while the photocell has them on
//! (the nearest to the camera when there are more than fit, the farthest of those fading
//! out), then for each instance the nearest lights whose reach touches its bounds. Fixed
//! arrays and buffers sized at load: nothing allocates per frame, and the cost is one
//! box-to-sphere test per light and instance.
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

/// How brightly the car's lamps and instruments glow at `d`.
pub fn car_glow(d: &Daylight) -> CarGlow {
    CarGlow {
        lamps: glow_cap(d, CAR_LAMP_EXPOSED),
        instruments: glow_cap(d, INSTRUMENT_EXPOSED),
    }
}

/// Street lamp heads' emissive multiplier: on or off with the photocell (civil dusk to
/// civil dawn), not ramped with the windows.
pub fn streetlamp_glow(d: &Daylight) -> f32 {
    if d.streetlights_on {
        1.0
    } else {
        0.0
    }
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
        let axis = Vec3::from(light.direction);
        if axis != Vec3::ZERO && light.cone_cos[0] >= 0.0 {
            let half = (self.max - self.min) * 0.5;
            let ahead = (self.center() - p).dot(axis) + half.dot(axis.abs());
            if ahead < 0.0 {
                return false;
            }
        }
        true
    }
}

/// The nearest [`LIGHTS_PER_DRAW`] of `lights` that reach `bounds` (by distance to the
/// box, then to its center, then order), as a light set. Allocates nothing.
pub fn light_set(lights: &[PointLight], bounds: &Bounds) -> LightSet {
    // Insertion into a short sorted array: most boxes are reached by a handful of lights.
    let mut best = [(f32::INFINITY, f32::INFINITY, 0usize); LIGHTS_PER_DRAW];
    let mut n = 0;
    let center = bounds.center();
    for (i, l) in lights.iter().enumerate().take(MAX_LIGHTS) {
        if !bounds.reached_by(l) {
            continue;
        }
        let p = Vec3::from(l.position);
        let key = (bounds.distance_squared(p), center.distance_squared(p), i);
        let worse = |a: &(f32, f32, usize), b: &(f32, f32, usize)| (a.0, a.1) > (b.0, b.1);
        if n == LIGHTS_PER_DRAW && !worse(&best[n - 1], &key) {
            continue;
        }
        let mut k = n.min(LIGHTS_PER_DRAW - 1);
        while k > 0 && worse(&best[k - 1], &key) {
            best[k] = best[k - 1];
            k -= 1;
        }
        best[k] = key;
        n = (n + 1).min(LIGHTS_PER_DRAW);
    }
    let mut set = LightSet::EMPTY;
    for &(_, _, i) in &best[..n] {
        set.push(i);
    }
    set
}

/// The frame's lights and the light set of every instance (see the module docs).
pub struct FrameLights {
    /// The place's static lights: street lamps, neon, storefront fill.
    statics: Vec<PointLight>,
    /// Which statics shine (a streamed chunk's lights only while it is drawn).
    active: Vec<bool>,
    /// Scratch for picking the nearest statics: squared distance to the camera and index.
    order: Vec<(f32, u32)>,
    lights: [PointLight; MAX_LIGHTS],
    len: usize,
}

impl FrameLights {
    pub fn new(statics: Vec<PointLight>) -> FrameLights {
        FrameLights {
            order: Vec::with_capacity(statics.len()),
            active: vec![true; statics.len()],
            statics,
            lights: [PointLight::omni([0.0; 3], [0.0; 3], 0.0); MAX_LIGHTS],
            len: 0,
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

    /// Capacity of the scratch buffer, which must not grow after load.
    pub fn order_capacity(&self) -> usize {
        self.order.capacity()
    }

    /// This frame's lights, as last gathered.
    pub fn lights(&self) -> &[PointLight] {
        &self.lights[..self.len]
    }

    /// This frame's lights: `dynamic` (the car's), then the static ones when `statics_on`
    /// (the street lamps' photocell), all of them when they fit, else those nearest `eye`,
    /// the farthest fading out toward the first left out.
    pub fn gather(&mut self, eye: Vec3, statics_on: bool, dynamic: &[PointLight]) {
        let n = dynamic.len().min(MAX_LIGHTS);
        self.lights[..n].copy_from_slice(&dynamic[..n]);
        self.len = n;
        if !statics_on {
            return;
        }
        let room = MAX_LIGHTS - n;
        let active = &self.active;
        let on = || {
            self.statics
                .iter()
                .zip(active)
                .filter(|(_, &a)| a)
                .map(|(l, _)| l)
        };
        if on().count() <= room {
            for l in on() {
                self.lights[self.len] = *l;
                self.len += 1;
            }
            return;
        }
        self.order.clear();
        self.order.extend(
            self.statics
                .iter()
                .enumerate()
                .filter(|&(i, _)| active[i])
                .map(|(i, l)| (Vec3::from(l.position).distance_squared(eye), i as u32)),
        );
        // Ties keep the earlier light, so the choice is deterministic.
        let by_distance = |a: &(f32, u32), b: &(f32, u32)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1));
        let (kept, cut, _) = self.order.select_nth_unstable_by(room, by_distance);
        let cut = cut.0.sqrt();
        for &(d2, i) in kept.iter() {
            let w = ((cut - d2.sqrt()) / (STATIC_FADE * cut)).clamp(0.0, 1.0);
            let mut l = self.statics[i as usize];
            l.color = l.color.map(|c| c * w);
            l.corona.strength *= w;
            self.lights[self.len] = l;
            self.len += 1;
        }
    }

    /// Sets each instance's light set from its bounds (`None`: lit by the sun and sky only).
    /// `bounds` and `instances` pair up; extra instances are left alone.
    pub fn assign(&self, bounds: &[Option<Bounds>], instances: &mut [MeshInstance]) {
        let lights = self.lights();
        for (inst, b) in instances.iter_mut().zip(bounds) {
            inst.lights = b.map_or(LightSet::EMPTY, |b| light_set(lights, &b));
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
    fn lamps_and_instruments_glow_as_built_by_day_and_dimmed_at_night() {
        let day = car_glow(&at(12.0));
        assert_eq!((day.lamps, day.instruments), (1.0, 1.0));
        let night_d = at(22.5);
        let night = car_glow(&night_d);
        let e = exposure_from_ev100(night_d.ev100) * EMISSIVE_BOOST;
        assert!((night.instruments * e - INSTRUMENT_EXPOSED).abs() < 1e-4);
        assert!((night.lamps * e - CAR_LAMP_EXPOSED).abs() < 1e-3);
        assert!(night.instruments < night.lamps && night.lamps < 1.0);
    }

    #[test]
    fn street_lamps_switch_with_the_photocell() {
        assert_eq!(streetlamp_glow(&at(12.0)), 0.0);
        assert_eq!(streetlamp_glow(&at(17.0 + 40.0 / 60.0)), 0.0);
        // Civil dusk is about 18:30 in late September; the dusk scene at 18:35 has them on.
        assert_eq!(streetlamp_glow(&at(18.0 + 35.0 / 60.0)), 1.0);
        assert_eq!(streetlamp_glow(&at(22.5)), 1.0);
        // The windows are already most of the way up when the lamps come on.
        assert!(at(18.0 + 35.0 / 60.0).window_emissive_scale > 0.9);
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
        frame.gather(Vec3::ZERO, false, &[head, head]);
        assert_eq!(frame.lights().len(), 2, "photocell off: the car's only");
        frame.gather(Vec3::ZERO, true, &[head, head]);
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
            frame.gather(eye, true, &dynamic);
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
    fn instances_get_the_nearest_lights_that_reach_them() {
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
                let key = |i: &usize| {
                    let p = Vec3::from(lights[*i].position);
                    (b.distance_squared(p), b.center().distance_squared(p))
                };
                reach.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap());
                reach.truncate(LIGHTS_PER_DRAW);
                let set = light_set(&lights, &b);
                let got: Vec<usize> = set.indices().iter().map(|&i| usize::from(i)).collect();
                assert_eq!(got, reach, "box at {x}, {z}");
            }
        }
        // Instances get their sets; those without bounds none.
        let mut frame = FrameLights::new(lamps_along_x(30, 10.0));
        frame.gather(Vec3::ZERO, true, &[]);
        let mut instances =
            [MeshInstance::new(render::MeshId(0), glam::Mat4::IDENTITY.to_cols_array_2d()); 3];
        instances[1].lights = LightSet::from_indices(&[1]);
        frame.assign(&[Some(small), None], &mut instances);
        assert_eq!(instances[0].lights.len(), 4);
        assert!(instances[1].lights.is_empty());
        assert_eq!(instances[2].lights, LightSet::EMPTY);
    }
}
