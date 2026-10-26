//! The game's time of day and how [`Daylight`] becomes the renderer's [`Sun`] and [`Sky`].
//!
//! Units: `Daylight` gives illuminance in lux and luminance in cd/m²; the renderer takes
//! kilolux and kcd/m². Directions agree (world X east, Y up, Z south).

use render::{exposure_from_ev100, Sky, Sun};

use crate::daylight::{Clock, Daylight, DEFAULT_LATITUDE_DEG};
use crate::input::{Action, InputState};

/// Late September: near the equinox the sun sets almost due west at about 18:00, so...
pub const DEFAULT_DAY_OF_YEAR: u16 = 265;
/// ...17:40 is golden hour, the first slice's showpiece.
pub const DEFAULT_TIME_S: f64 = 17.0 * 3600.0 + 40.0 * 60.0;
/// `TimeScrub` at full deflection moves the clock this many game seconds per real second.
pub const SCRUB_SECONDS_PER_SECOND: f64 = 3600.0;
/// Luminance of the full moon's disc, kcd/m² (the renderer works out the phase).
const MOON_DISC_KCD: f32 = 2.5;

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

/// The renderer's sky for `d`: turbidity, moon, stars, fog, exposure (EV100 to the
/// renderer's kcd/m² scale) and the lit-window scale; the rest stays at the defaults.
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
        fog_density: d.fog_density,
        exposure: exposure_from_ev100(d.ev100),
        window_emissive_scale: d.window_emissive_scale,
        ..Sky::default()
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
}
