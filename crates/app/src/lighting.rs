//! The game's time of day and how [`Daylight`] becomes the renderer's [`Sun`] and [`Sky`],
//! and the lights that follow it: the street lamps' photocell and pools, the glow of the
//! car's lamps and instruments, and the light inside the cabin.
//!
//! Units: `Daylight` gives illuminance in lux and luminance in cd/m²; the renderer takes
//! kilolux and kcd/m². Directions agree (world X east, Y up, Z south).
//!
//! Night brightness is set in post-exposure terms (the tonemapper's input, where 0.18 is mid
//! grey and the bloom threshold is 1): whatever the exposure, a lit window, the instrument
//! backlight or the cabin's ambient lands at a chosen display value instead of following
//! its physical luminance up into a blown-out blob.

use glam::Vec3;
use render::{exposure_from_ev100, PointLight, Sky, Sun};

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
/// bloom threshold, the needles (4, orange) at ~2 with a faint halo, the dial faces (0.15)
/// as a dim green backlight.
const INSTRUMENT_EXPOSED: f32 = 0.6;
/// Cabin ambient (`Sky::cabin_light`) after exposure: by day the light through the glass,
/// neutral; by night the dome and instrument backlight, warm and dim. A white surface in the
/// cabin would read at this value; the dark 80s trims read a tenth of it.
const CABIN_DAY_EXPOSED: f32 = 1.6;
const CABIN_NIGHT_EXPOSED: f32 = 0.9;
const CABIN_DAY_RGB: [f32; 3] = [0.96, 1.0, 1.04];
const CABIN_NIGHT_RGB: [f32; 3] = [1.23, 0.96, 0.69];
/// Moonless night sky luminance, kcd/m² (`Sky::night_luminance`; the renderer's default is
/// 2e-5). About 0.025 after the night exposure: deep blue behind the stars and the
/// roofline, not black.
const NIGHT_SKY_KCD: f32 = 2.4e-4;

/// Street lamps lit as point lights at once: the renderer takes 8 and the headlights use 2.
pub const STREETLAMP_LIGHTS: usize = 6;
/// A sodium cobra head: intensity straight down, kcd (≈ 35 lux on the road 7.5 m below),
/// reach, m, and color (linear, luminance 1, the town palette's sodium)...
const STREETLAMP_KCD: f32 = 2.0;
const STREETLAMP_RADIUS_M: f32 = 22.0;
const STREETLAMP_RGB: [f32; 3] = [1.98, 0.79, 0.16];
/// ...shining down in a cone, full to this angle from straight down and cut off by that one,
/// degrees: a pool on the road and the shop fronts beside it, dark upper floors (an omni
/// light lit every facade within reach as brightly as the road).
const STREETLAMP_INNER_DEG: f32 = 45.0;
const STREETLAMP_OUTER_DEG: f32 = 72.0;
/// The nearest lamps to a point this far ahead of the camera get the lights, m...
pub const STREETLAMP_FOCUS_AHEAD_M: f32 = 12.0;
/// ...each fading in over this share of the distance to the first lamp left out, so a lamp
/// joining or leaving the set never pops.
const STREETLAMP_FADE: f32 = 0.3;

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

/// Fills `out` with the street lamps nearest `focus` as point lights (from `heads`, the
/// lamp positions) and returns how many. Each fades in from nothing at the distance of the
/// first lamp left out, so the set changes smoothly as the car moves. Allocates nothing.
pub fn streetlamp_lights(
    heads: &[Vec3],
    focus: Vec3,
    out: &mut [PointLight; STREETLAMP_LIGHTS],
) -> usize {
    // The STREETLAMP_LIGHTS + 1 nearest by squared distance, insertion-sorted; ties keep
    // the earlier lamp, so the choice is deterministic.
    let mut best = [(f32::INFINITY, 0usize); STREETLAMP_LIGHTS + 1];
    for (i, h) in heads.iter().enumerate() {
        let d2 = (*h - focus).length_squared();
        if d2 < best[STREETLAMP_LIGHTS].0 {
            let mut k = STREETLAMP_LIGHTS;
            while k > 0 && best[k - 1].0 > d2 {
                best[k] = best[k - 1];
                k -= 1;
            }
            best[k] = (d2, i);
        }
    }
    let cut = best[STREETLAMP_LIGHTS].0.sqrt();
    let mut n = 0;
    for &(d2, i) in &best[..STREETLAMP_LIGHTS] {
        if !d2.is_finite() {
            break;
        }
        let w = if cut.is_finite() {
            ((cut - d2.sqrt()) / (STREETLAMP_FADE * cut)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        out[n] = PointLight {
            direction: [0.0, -1.0, 0.0],
            cone_cos: [
                STREETLAMP_OUTER_DEG.to_radians().cos(),
                STREETLAMP_INNER_DEG.to_radians().cos(),
            ],
            ..PointLight::omni(
                heads[i].to_array(),
                STREETLAMP_RGB.map(|c| c * STREETLAMP_KCD * w),
                STREETLAMP_RADIUS_M,
            )
        };
        n += 1;
    }
    n
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
    fn the_cabin_reads_at_the_same_brightness_day_and_night() {
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

    #[test]
    fn the_nearest_lamps_light_and_fade_in_smoothly() {
        let heads: Vec<Vec3> = (0..40)
            .map(|i| Vec3::new(i as f32 * 25.0, 7.5, 0.0))
            .collect();
        let mut out = [PointLight::omni([0.0; 3], [0.0; 3], 1.0); STREETLAMP_LIGHTS];
        let n = streetlamp_lights(&heads, Vec3::new(301.0, 0.0, 0.0), &mut out);
        assert_eq!(n, STREETLAMP_LIGHTS);
        // Nearest first: 300, 325, 275, 350, 250, 375 (and 225 is the first left out).
        let xs: Vec<f32> = out.iter().map(|l| l.position[0]).collect();
        assert_eq!(xs, [300.0, 325.0, 275.0, 350.0, 250.0, 375.0]);
        assert!((out[0].color[0] - STREETLAMP_RGB[0] * STREETLAMP_KCD).abs() < 1e-5);
        assert!(out[5].color[0] < out[0].color[0], "the farthest is fading");
        // Moving along, every lamp's intensity changes continuously, even as the set
        // changes: no step bigger than the motion explains.
        let mut prev = [0.0f32; 40];
        for step in 0..=2000 {
            let x = 300.0 + step as f32 * 0.05;
            let n = streetlamp_lights(&heads, Vec3::new(x, 0.0, 0.0), &mut out);
            let mut now = [0.0f32; 40];
            for l in &out[..n] {
                now[(l.position[0] / 25.0) as usize] = l.color[0];
            }
            if step > 0 {
                for k in 0..40 {
                    assert!((now[k] - prev[k]).abs() < 0.05, "lamp {k} at x {x}");
                }
            }
            prev = now;
        }
        // Fewer lamps than lights: all of them, full strength.
        let n = streetlamp_lights(&heads[..3], Vec3::ZERO, &mut out);
        assert_eq!(n, 3);
        assert!(out[..3].iter().all(|l| l.color[0] == out[0].color[0]));
        assert_eq!(streetlamp_lights(&[], Vec3::ZERO, &mut out), 0);
    }
}
