//! Time of day: where the sun and moon are, and what the light, sky and city lights do about it.
//!
//! Pure CPU math, no rendering. [`Daylight::compute`] maps `(latitude, day of year, local solar
//! time)` to everything the renderer needs; [`Clock`] is the game clock that feeds it.
//!
//! Units, chosen so the output maps onto a physically based sky later:
//! - directions are unit vectors in world space: X east, Y up, Z south;
//! - `sun_intensity` and `moon_intensity` are illuminance on a surface facing the body, lux;
//! - sky and fog colors are linear-RGB luminance, cd/m²;
//! - `exposure = 1 / (1.2 · 2^ev100)` turns luminance into display values (≈ 0..1 for the sky).
//!
//! Sky colors are authored as display values at a few sun elevations, then divided by that
//! elevation's exposure, so `sky * exposure` is exactly the authored look. Golden hour and blue
//! hour are the showpieces; tune the `SKY_*` and `EV_*` tables there first.
//!
//! Approximations (all deterministic, all documented where used):
//! - `seconds_of_day` is *apparent* local solar time: the sun transits at 12:00 every day. The
//!   equation of time (±16 min against a wall clock) is deliberately left out.
//! - Solar declination is Spencer's (1971) Fourier fit, held constant over each day.
//! - Sunrise and sunset are when the sun's center is at −0.833° (refraction plus semi-diameter).
//! - The sun's tint is Rayleigh-like per-channel extinction over the Kasten–Young air mass.
//! - The moon sits on the ecliptic (its 5° inclination is ignored), `phase · 360°` east of the
//!   sun; the phase is a fixed synodic cycle that starts with a new moon on 1 January 00:00.

/// The first slice's latitude (Southern California), degrees north.
pub const DEFAULT_LATITUDE_DEG: f32 = 34.0;
/// Seconds in a day.
pub const DAY_SECONDS: f64 = 86_400.0;
/// Days in the game year (no leap years).
pub const DAYS_PER_YEAR: u16 = 365;
/// Clock speeds the UI offers: frozen, real time, the default 10×, and 60×.
pub const CLOCK_SPEEDS: [f32; 4] = [0.0, 1.0, 10.0, 60.0];

/// Sun-center elevation at sunrise and sunset, degrees.
const SUNRISE_ELEVATION_DEG: f64 = -0.833;
/// Streetlights switch on when the sun sinks below this (civil dusk)...
const STREETLIGHTS_ON_DEG: f32 = -6.0;
/// ...and off once it climbs back above this, so they never flicker at the threshold.
const STREETLIGHTS_OFF_DEG: f32 = -5.0;
/// Earth's axial tilt, degrees.
const OBLIQUITY_DEG: f64 = 23.44;
/// Mean new moon to new moon, days.
const SYNODIC_MONTH_DAYS: f64 = 29.530_588;
/// Direct sunlight above the atmosphere, lux (tuned so a clear noon gives ~95 klx).
const SUN_LUX: f32 = 110_000.0;
/// Full moon overhead, lux. About 8× the real value so moonlit streets are readable.
const MOON_LUX_FULL: f32 = 2.0;
/// Per-air-mass extinction of R, G, B at turbidity 3: tints the sun.
const EXTINCTION_RGB: [f32; 3] = [0.02, 0.05, 0.11];
/// Per-air-mass grey (aerosol) extinction at turbidity 3: dims the sun without tinting it.
const EXTINCTION_GREY: f32 = 0.08;
/// What the sun's tint fades to as it sinks from the horizon to civil dusk.
const SUN_DARK_RED: [f32; 3] = [0.40, 0.05, 0.0];
/// Exponential fog coefficient per unit turbidity, 1/m.
const FOG_PER_TURBIDITY: f32 = 0.0003;
/// The morning haze over the basin: it settles through the small hours (from this many
/// seconds after midnight)...
const HAZE_SETTLES_S: f64 = 2.0 * 3600.0;
/// ...is thickest from an hour before sunrise, starts to burn off half an hour after it and
/// is gone three and a half hours after it, by mid-morning (seconds from sunrise).
const HAZE_THICK_S: f64 = -3600.0;
const HAZE_BURNS_S: f64 = 1800.0;
const HAZE_GONE_S: f64 = 3.5 * 3600.0;
/// At its thickest it adds this much to the weather's haze (turbidity +2.4)...
const HAZE_WEATHER: f32 = 0.6;
/// ...and this much fog, 1/m: about a kilometre's visibility, a low grey basin.
const HAZE_FOG: f32 = 0.0028;

/// Exposure keys: sun elevation (degrees, ascending) → EV100.
const EV_ELEV: [f32; 11] = [
    -18.0, -12.0, -8.0, -6.0, -3.0, 0.0, 3.0, 8.0, 20.0, 45.0, 90.0,
];
const EV_VALUE: [f32; 11] = [3.0, 3.5, 5.0, 6.5, 8.5, 10.5, 12.0, 13.3, 14.3, 14.8, 15.0];

/// Sky keys: sun elevation (degrees, ascending) → display-referred zenith and horizon colors
/// at turbidity 3.
const SKY_ELEV: [f32; 9] = [-18.0, -12.0, -6.0, -3.0, 0.0, 4.0, 10.0, 25.0, 90.0];
const SKY_ZENITH: [[f32; 3]; 9] = [
    [0.003, 0.004, 0.010], // night
    [0.006, 0.010, 0.030], // nautical twilight
    [0.020, 0.040, 0.120], // blue hour
    [0.040, 0.080, 0.220],
    [0.080, 0.150, 0.380], // sunset
    [0.090, 0.190, 0.500], // golden hour
    [0.090, 0.210, 0.620],
    [0.090, 0.220, 0.700], // day
    [0.070, 0.200, 0.720],
];
const SKY_HORIZON: [[f32; 3]; 9] = [
    [0.020, 0.015, 0.012], // night: faint warm city glow
    [0.025, 0.022, 0.030],
    [0.120, 0.090, 0.140], // blue hour, a little violet
    [0.400, 0.220, 0.180], // afterglow
    [0.850, 0.450, 0.220], // sunset
    [0.850, 0.620, 0.420], // golden hour
    [0.620, 0.640, 0.700],
    [0.500, 0.620, 0.800], // day
    [0.480, 0.620, 0.820],
];

/// Where in the day we are, from the sun's elevation and whether it is rising or setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Sun below −12° (past nautical twilight).
    Night,
    /// Morning twilight, −12° to −2°.
    Dawn,
    /// Rising sun, −2° to 6°.
    Sunrise,
    /// Sun above 6°.
    Day,
    /// Setting sun, 6° to 1°.
    GoldenHour,
    /// Sun on the horizon, 1° to −2°.
    Sunset,
    /// Evening twilight, −2° to −12°, blue hour included.
    Dusk,
}

/// Atmospheric conditions. Only haze for now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weather {
    /// 0 = crystal clear (turbidity 2) … 1 = thick haze (turbidity 6).
    pub haze: f32,
}

impl Default for Weather {
    /// A typical coastal day, turbidity 3.
    fn default() -> Weather {
        Weather { haze: 0.25 }
    }
}

impl Weather {
    /// Linke-style turbidity, 2..=6.
    pub fn turbidity(&self) -> f32 {
        2.0 + 4.0 * self.haze.clamp(0.0, 1.0)
    }
}

/// Everything the renderer needs to light one moment of the day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Daylight {
    /// Unit vector towards the sun (X east, Y up, Z south).
    pub sun_dir: [f32; 3],
    /// Degrees above the horizon; negative below.
    pub sun_elevation_deg: f32,
    /// Degrees clockwise from north, `0..360` (east 90, south 180).
    pub sun_azimuth_deg: f32,
    /// Linear-RGB chromaticity of direct sunlight, max channel 1 (0 below −6°).
    pub sun_color: [f32; 3],
    /// Direct sunlight, lux; 0 once the disc has set.
    pub sun_intensity: f32,
    /// Unit vector towards the moon.
    pub moon_dir: [f32; 3],
    /// 0 new, 0.25 first quarter, 0.5 full, 0.75 last quarter; `0..1`.
    pub moon_phase: f32,
    /// Moonlight, lux (artistically boosted); 0 when the moon is down or new.
    pub moon_intensity: f32,
    /// Sky luminance straight up, linear RGB cd/m².
    pub sky_zenith_color: [f32; 3],
    /// Sky luminance at the horizon, linear RGB cd/m².
    pub sky_horizon_color: [f32; 3],
    /// 2..=6, from the weather.
    pub turbidity: f32,
    /// Exponential fog coefficient, 1/m.
    pub fog_density: f32,
    /// Fog in-scatter luminance, linear RGB cd/m².
    pub fog_color: [f32; 3],
    /// Exposure value at ISO 100: ~15 at noon, ~10.5 at sunset, 3 at night.
    pub ev100: f32,
    /// Multiplier from luminance to display: `1 / (1.2 · 2^ev100)`.
    pub exposure: f32,
    /// Civil dusk to civil dawn, with hysteresis (see [`streetlights_next`]).
    pub streetlights_on: bool,
    /// Game seconds since the photocell last switched (on at civil dusk, off at dawn): the
    /// clock the street lamps' wave and the signs' delays run on, `0..86400`.
    pub lamps_switched_s: f32,
    /// The morning haze, 0..1: settles overnight, thickest at dawn, burnt off by
    /// mid-morning. Already in `turbidity` and `fog_density`.
    pub haze: f32,
    /// Lit-window emissive scale: 0 by day, ramping over blue hour to 1 by −8°.
    pub window_emissive_scale: f32,
    /// 0 by day, 1 at astronomical night (−18°).
    pub star_brightness: f32,
    pub phase: Phase,
}

impl Daylight {
    /// The light at `seconds_of_day` (apparent local solar time) on `day_of_year` (1..=365),
    /// at `latitude_deg` (north positive), in default weather.
    pub fn compute(latitude_deg: f32, day_of_year: u16, seconds_of_day: f32) -> Daylight {
        Daylight::compute_with(
            latitude_deg,
            day_of_year,
            seconds_of_day,
            Weather::default(),
        )
    }

    /// [`Daylight::compute`] in the given weather.
    pub fn compute_with(
        latitude_deg: f32,
        day_of_year: u16,
        seconds_of_day: f32,
        weather: Weather,
    ) -> Daylight {
        let lat = f64::from(latitude_deg).to_radians();
        let secs = f64::from(seconds_of_day).rem_euclid(DAY_SECONDS);
        let hour_angle = hour_angle(secs);
        let sun = horizon_dir(lat, declination(day_of_year), hour_angle);
        let (sun_elev, sun_az) = elevation_azimuth(sun);

        let phase_f = moon_phase(day_of_year, secs);
        let moon = moon_dir(lat, day_of_year, hour_angle, phase_f);
        let (moon_elev, _) = elevation_azimuth(moon);
        let lit_fraction = (1.0 - (core::f64::consts::TAU * phase_f).cos()) as f32 * 0.5;

        let haze = morning_haze(latitude_deg, day_of_year, secs);
        let turbidity = Weather {
            haze: weather.haze + HAZE_WEATHER * haze,
        }
        .turbidity();
        let (sun_color, sun_intensity) = sunlight(sun_elev, turbidity);

        let ev100 = ev100(sun_elev);
        let exposure = 1.0 / (1.2 * ev100.exp2());
        let (zenith, horizon) = sky_display(sun_elev, turbidity);
        let sky_zenith_color = scale(zenith, 1.0 / exposure);
        let sky_horizon_color = scale(horizon, 1.0 / exposure);

        let morning = secs < DAY_SECONDS * 0.5;
        // Without history, assume a continuous clock: lights that came on overnight are
        // still on during the morning hysteresis band, and still off in the evening one.
        let streetlights_on = streetlights_next(morning, sun_elev);
        let switched = if streetlights_on {
            DAY_SECONDS * 0.5 + half_day(lat, day_of_year, STREETLIGHTS_ON_DEG)
        } else {
            DAY_SECONDS * 0.5 - half_day(lat, day_of_year, STREETLIGHTS_OFF_DEG)
        };
        // The photocell may read a hair ahead of the analytic moment: just switched, not a
        // day ago.
        let since = (secs - switched).rem_euclid(DAY_SECONDS);
        let since = if since > DAY_SECONDS - 60.0 {
            0.0
        } else {
            since
        };
        Daylight {
            sun_dir: to_f32(sun),
            sun_elevation_deg: sun_elev,
            sun_azimuth_deg: sun_az,
            sun_color,
            sun_intensity,
            moon_dir: to_f32(moon),
            moon_phase: phase_f as f32,
            moon_intensity: MOON_LUX_FULL * lit_fraction * smoothstep(-1.0, 3.0, moon_elev),
            sky_zenith_color,
            sky_horizon_color,
            turbidity,
            fog_density: FOG_PER_TURBIDITY * turbidity + HAZE_FOG * haze,
            fog_color: sky_horizon_color,
            ev100,
            exposure,
            streetlights_on,
            lamps_switched_s: since as f32,
            haze,
            window_emissive_scale: 1.0 - smoothstep(-8.0, 0.0, sun_elev),
            star_brightness: 1.0 - smoothstep(-18.0, -4.0, sun_elev),
            phase: phase(sun_elev, morning),
        }
    }
}

/// The streetlight photocell: given whether the lights were on, are they on at this sun
/// elevation? On below −6° (civil dusk), off again only above −5°.
pub fn streetlights_next(was_on: bool, sun_elevation_deg: f32) -> bool {
    if was_on {
        sun_elevation_deg < STREETLIGHTS_OFF_DEG
    } else {
        sun_elevation_deg < STREETLIGHTS_ON_DEG
    }
}

/// Sunrise and sunset, seconds of apparent local solar time, symmetric about noon. Under the
/// midnight sun this is `(0, 86400)`; in polar night, `(43200, 43200)`.
pub fn sun_times(latitude_deg: f32, day_of_year: u16) -> (f32, f32) {
    let lat = f64::from(latitude_deg).to_radians();
    let half_day = half_day(lat, day_of_year, SUNRISE_ELEVATION_DEG as f32);
    let noon = DAY_SECONDS * 0.5;
    ((noon - half_day) as f32, (noon + half_day) as f32)
}

/// Seconds from noon to when the sun's center is at `elev_deg` (latitude `lat` in
/// radians), clamped to `0..43200` where it never gets there.
fn half_day(lat: f64, day_of_year: u16, elev_deg: f32) -> f64 {
    let decl = declination(day_of_year);
    let cos_h = (f64::from(elev_deg).to_radians().sin() - lat.sin() * decl.sin())
        / (lat.cos() * decl.cos());
    cos_h.clamp(-1.0, 1.0).acos() / core::f64::consts::TAU * DAY_SECONDS
}

/// The morning haze at `secs` (apparent solar time), 0..1: rising from
/// [`HAZE_SETTLES_S`] to its thickest an hour before sunrise, burning off by mid-morning.
fn morning_haze(latitude_deg: f32, day_of_year: u16, secs: f64) -> f32 {
    let (rise, _) = sun_times(latitude_deg, day_of_year);
    let rise = f64::from(rise);
    let settle = smoothstep(
        HAZE_SETTLES_S as f32,
        (rise + HAZE_THICK_S).max(HAZE_SETTLES_S + 1.0) as f32,
        secs as f32,
    );
    let burn = smoothstep(
        (rise + HAZE_BURNS_S) as f32,
        (rise + HAZE_GONE_S) as f32,
        secs as f32,
    );
    settle * (1.0 - burn)
}

/// The game clock. Real time drives it at `speed`; the day of year rolls over at midnight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// Apparent local solar time, `0..86400`.
    pub seconds_of_day: f64,
    /// Game seconds per real second; one of [`CLOCK_SPEEDS`].
    pub speed: f32,
    /// 1..=365.
    pub day_of_year: u16,
}

impl Clock {
    /// A clock at the given moment, running at the default 10×.
    pub fn new(day_of_year: u16, seconds_of_day: f64) -> Clock {
        let mut c = Clock {
            seconds_of_day: 0.0,
            speed: 10.0,
            day_of_year: day_of_year.clamp(1, DAYS_PER_YEAR),
        };
        c.scrub(seconds_of_day);
        c
    }

    /// Runs the clock for `dt_real` real seconds.
    pub fn advance(&mut self, dt_real: f64) {
        self.scrub(dt_real * f64::from(self.speed));
    }

    /// Jumps by `delta_seconds` of game time (negative goes back), rolling the day over.
    pub fn scrub(&mut self, delta_seconds: f64) {
        let s = self.seconds_of_day + delta_seconds;
        let mut days = (s / DAY_SECONDS).floor();
        let mut rest = s - days * DAY_SECONDS;
        if rest >= DAY_SECONDS {
            // Rounding put a value just under a day boundary onto it.
            rest = 0.0;
            days += 1.0;
        }
        self.seconds_of_day = rest;
        let year = i64::from(DAYS_PER_YEAR);
        let day = (i64::from(self.day_of_year) - 1 + days as i64).rem_euclid(year) + 1;
        self.day_of_year = day as u16;
    }
}

/// Solar declination for the day, radians (Spencer 1971, max error ~0.2°).
fn declination(day_of_year: u16) -> f64 {
    let g = core::f64::consts::TAU * (f64::from(day_of_year.clamp(1, DAYS_PER_YEAR)) - 1.0)
        / f64::from(DAYS_PER_YEAR);
    0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos()
        + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin()
}

/// Sun hour angle, radians: 0 at solar noon, negative in the morning.
fn hour_angle(seconds_of_day: f64) -> f64 {
    (seconds_of_day / DAY_SECONDS - 0.5) * core::f64::consts::TAU
}

/// Unit vector (X east, Y up, Z south) to a body at declination `decl` and `hour_angle`, seen
/// from latitude `lat` (all radians).
fn horizon_dir(lat: f64, decl: f64, hour_angle: f64) -> [f64; 3] {
    let (sl, cl) = lat.sin_cos();
    let (sd, cd) = decl.sin_cos();
    let (sh, ch) = hour_angle.sin_cos();
    let east = -cd * sh;
    let north = cl * sd - sl * cd * ch;
    let up = sl * sd + cl * cd * ch;
    [east, up, -north]
}

/// Elevation and azimuth (clockwise from north, `0..360`) of a direction, degrees.
fn elevation_azimuth(d: [f64; 3]) -> (f32, f32) {
    let elev = d[1].clamp(-1.0, 1.0).asin().to_degrees();
    let az = d[0].atan2(-d[2]).to_degrees().rem_euclid(360.0);
    (elev as f32, az as f32)
}

/// Moon phase in `0..1`, a fixed synodic cycle from a new moon on day 1 at 00:00.
fn moon_phase(day_of_year: u16, seconds_of_day: f64) -> f64 {
    let days = f64::from(day_of_year) - 1.0 + seconds_of_day / DAY_SECONDS;
    (days / SYNODIC_MONTH_DAYS).rem_euclid(1.0)
}

/// Direction to the moon: on the ecliptic, `phase · 360°` east of the sun, so it transits
/// `phase` of a day after the sun (a full moon at midnight).
fn moon_dir(lat: f64, day_of_year: u16, sun_hour_angle: f64, phase: f64) -> [f64; 3] {
    let tau = core::f64::consts::TAU;
    let sun_longitude = tau * (f64::from(day_of_year) - 80.0) / f64::from(DAYS_PER_YEAR);
    let longitude = sun_longitude + tau * phase;
    let decl = (OBLIQUITY_DEG.to_radians().sin() * longitude.sin()).asin();
    horizon_dir(lat, decl, sun_hour_angle - tau * phase)
}

/// Kasten–Young relative air mass at an elevation in degrees (≥ 0): 1 overhead, ~38 at the
/// horizon.
fn air_mass(elev_deg: f32) -> f32 {
    let h = elev_deg.max(0.0);
    1.0 / (h.to_radians().sin() + 0.50572 * (h + 6.07995).powf(-1.6364))
}

/// Direct sunlight: chromaticity (max channel 1) and illuminance in lux.
fn sunlight(elev_deg: f32, turbidity: f32) -> ([f32; 3], f32) {
    let tau = turbidity / 3.0;
    let m = air_mass(elev_deg);
    let mut t = [0.0; 3];
    for c in 0..3 {
        // Relative to one air mass, so the sun is white overhead.
        t[c] = (-EXTINCTION_RGB[c] * tau * (m - 1.0)).exp().min(1.0);
    }
    let peak = t[0].max(t[1]).max(t[2]);
    let mut color = scale(t, 1.0 / peak);
    if elev_deg < 0.0 {
        // Below the horizon only the afterglow is left: fade to dark red, then to nothing.
        let s = ((elev_deg + 6.0) / 6.0).clamp(0.0, 1.0);
        color = scale(lerp3(SUN_DARK_RED, color, s), smoothstep(0.0, 1.0, s));
    }
    let mut abs = [0.0; 3];
    for c in 0..3 {
        abs[c] = (-EXTINCTION_RGB[c] * tau * m).exp();
    }
    let lux = SUN_LUX * (-EXTINCTION_GREY * tau * m).exp() * luminance(abs);
    (color, lux * smoothstep(-1.0, 1.0, elev_deg))
}

/// EV100 for a sun elevation: lower as the light goes, so dusk and night are exposed up.
fn ev100(elev_deg: f32) -> f32 {
    let (i, t) = locate(&EV_ELEV, elev_deg);
    EV_VALUE[i] + (EV_VALUE[i + 1] - EV_VALUE[i]) * t
}

/// Display-referred zenith and horizon colors, hazed by turbidity.
fn sky_display(elev_deg: f32, turbidity: f32) -> ([f32; 3], [f32; 3]) {
    let (i, t) = locate(&SKY_ELEV, elev_deg);
    let zenith = lerp3(SKY_ZENITH[i], SKY_ZENITH[i + 1], t);
    let horizon = lerp3(SKY_HORIZON[i], SKY_HORIZON[i + 1], t);
    // Haze greys the horizon and spreads it up the sky; clear air does the opposite.
    let w = (turbidity - 3.0) / 3.0;
    let y = luminance(horizon);
    let horizon = clamp3(lerp3(horizon, [y; 3], 0.3 * w));
    let zenith = clamp3(lerp3(zenith, horizon, 0.25 * w));
    (zenith, horizon)
}

fn phase(elev_deg: f32, morning: bool) -> Phase {
    match (morning, elev_deg) {
        (_, h) if h < -12.0 => Phase::Night,
        (_, h) if h >= 6.0 => Phase::Day,
        (true, h) if h < -2.0 => Phase::Dawn,
        (true, _) => Phase::Sunrise,
        (false, h) if h >= 1.0 => Phase::GoldenHour,
        (false, h) if h >= -2.0 => Phase::Sunset,
        (false, _) => Phase::Dusk,
    }
}

/// Segment of ascending knots `xs` containing `x`, and the eased position within it. Clamps
/// outside the knots. Easing each segment keeps the curve C1 and monotone between knots.
fn locate(xs: &[f32], x: f32) -> (usize, f32) {
    let last = xs.len() - 1;
    if x <= xs[0] {
        return (0, 0.0);
    }
    if x >= xs[last] {
        return (last - 1, 1.0);
    }
    let mut i = 0;
    while x >= xs[i + 1] {
        i += 1;
    }
    (i, smoothstep(xs[i], xs[i + 1], x))
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

fn clamp3(c: [f32; 3]) -> [f32; 3] {
    [
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
    ]
}

fn to_f32(d: [f64; 3]) -> [f32; 3] {
    [d[0] as f32, d[1] as f32, d[2] as f32]
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAT: f32 = DEFAULT_LATITUDE_DEG;
    const MARCH_EQUINOX: u16 = 79;
    const JUNE_SOLSTICE: u16 = 172;
    const DECEMBER_SOLSTICE: u16 = 355;
    const HOUR: f32 = 3600.0;

    fn len(v: [f32; 3]) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn equinox_sunrise_and_sunset_near_six() {
        let (rise, set) = sun_times(LAT, MARCH_EQUINOX);
        assert!((rise - 6.0 * HOUR).abs() < 600.0, "sunrise {}", rise / HOUR);
        assert!((set - 18.0 * HOUR).abs() < 600.0, "sunset {}", set / HOUR);
    }

    #[test]
    fn solstice_day_lengths() {
        let length = |day| {
            let (rise, set) = sun_times(LAT, day);
            (set - rise) / 60.0
        };
        let june = length(JUNE_SOLSTICE);
        let december = length(DECEMBER_SOLSTICE);
        assert!(
            (june - (14.0 * 60.0 + 20.0)).abs() < 15.0,
            "june {june} min"
        );
        assert!(
            (december - (9.0 * 60.0 + 50.0)).abs() < 15.0,
            "december {december} min"
        );
    }

    #[test]
    fn sun_times_agree_with_compute() {
        for day in [MARCH_EQUINOX, JUNE_SOLSTICE, DECEMBER_SOLSTICE] {
            let (rise, set) = sun_times(LAT, day);
            let at_rise = Daylight::compute(LAT, day, rise);
            let at_set = Daylight::compute(LAT, day, set);
            assert!((at_rise.sun_elevation_deg + 0.833).abs() < 0.01);
            assert!((at_set.sun_elevation_deg + 0.833).abs() < 0.01);
            assert!(at_rise.sun_dir[0] > 0.3, "rises in the east");
            assert!(at_set.sun_dir[0] < -0.3, "sets in the west");
        }
    }

    #[test]
    fn equinox_noon_sun_is_south_at_56_degrees() {
        let d = Daylight::compute(LAT, MARCH_EQUINOX, 12.0 * HOUR);
        assert!(
            (d.sun_elevation_deg - 56.0).abs() < 1.0,
            "{}",
            d.sun_elevation_deg
        );
        assert!((d.sun_azimuth_deg - 180.0).abs() < 0.01);
        assert!(d.sun_dir[2] > 0.5 && d.sun_dir[0].abs() < 1e-6);
        assert_eq!(d.phase, Phase::Day);
        assert!(d.sun_intensity > 80_000.0, "{}", d.sun_intensity);
        assert_eq!(d.star_brightness, 0.0);
        assert_eq!(d.window_emissive_scale, 0.0);
        assert!(!d.streetlights_on);
    }

    #[test]
    fn midnight_is_dark_and_lit_up() {
        let d = Daylight::compute(LAT, MARCH_EQUINOX, 0.0);
        assert_eq!(d.phase, Phase::Night);
        assert_eq!(d.sun_intensity, 0.0);
        assert_eq!(d.sun_color, [0.0; 3]);
        assert_eq!(d.star_brightness, 1.0);
        assert_eq!(d.window_emissive_scale, 1.0);
        assert!(d.streetlights_on);
    }

    #[test]
    fn sun_is_white_overhead_and_orange_low() {
        let noon = Daylight::compute(0.0, MARCH_EQUINOX, 12.0 * HOUR);
        assert!(noon.sun_elevation_deg > 89.0);
        assert!(
            noon.sun_color.iter().all(|&c| c > 0.999),
            "{:?}",
            noon.sun_color
        );
        let (low, _) = sunlight(2.0, 3.0);
        assert!(low[0] == 1.0 && low[1] < 0.7 && low[2] < 0.4, "{low:?}");
        let (clear, clear_lux) = sunlight(2.0, Weather { haze: 0.0 }.turbidity());
        let (hazy, hazy_lux) = sunlight(2.0, Weather { haze: 1.0 }.turbidity());
        assert!(hazy[2] < clear[2] && hazy_lux < clear_lux);
    }

    #[test]
    fn turbidity_follows_weather() {
        assert_eq!(Weather { haze: 0.0 }.turbidity(), 2.0);
        assert_eq!(Weather { haze: 1.0 }.turbidity(), 6.0);
        assert_eq!(Weather { haze: 9.0 }.turbidity(), 6.0);
        assert_eq!(Daylight::compute(LAT, 1, 0.0).turbidity, 3.0);
    }

    #[test]
    fn exposure_rises_as_the_sun_sets() {
        let noon = Daylight::compute(LAT, JUNE_SOLSTICE, 12.0 * HOUR);
        let mut prev = noon;
        let mut s = 12.0 * HOUR;
        while s <= 24.0 * HOUR {
            let d = Daylight::compute(LAT, JUNE_SOLSTICE, s);
            assert!(d.sun_elevation_deg <= prev.sun_elevation_deg);
            assert!(
                d.exposure >= prev.exposure * (1.0 - 1e-6),
                "at {} h",
                s / HOUR
            );
            prev = d;
            s += 60.0;
        }
        assert!(prev.exposure > noon.exposure * 1000.0);
    }

    #[test]
    fn streetlights_turn_on_once_each_evening_and_off_each_morning() {
        let (mut ons, mut offs) = (0, 0);
        let mut was = Daylight::compute(LAT, MARCH_EQUINOX, 0.0).streetlights_on;
        for minute in 0..(3 * 24 * 60) {
            let clock = Clock::new(MARCH_EQUINOX, f64::from(minute) * 60.0);
            let d = Daylight::compute(LAT, clock.day_of_year, clock.seconds_of_day as f32);
            match (was, d.streetlights_on) {
                (false, true) => {
                    ons += 1;
                    assert!(d.sun_elevation_deg < -6.0 && d.phase == Phase::Dusk);
                }
                (true, false) => offs += 1,
                _ => {}
            }
            was = d.streetlights_on;
        }
        assert_eq!((ons, offs), (3, 3));
    }

    #[test]
    fn the_lamp_clock_restarts_at_civil_dusk_and_at_dawn() {
        let day = 265;
        // Find the evening switch to a tenth of a second, then step through it.
        let mut t = 18.0 * HOUR;
        while !Daylight::compute(LAT, day, t).streetlights_on {
            t += 0.1;
        }
        let d = Daylight::compute(LAT, day, t);
        assert!(d.lamps_switched_s < 0.5, "{}", d.lamps_switched_s);
        let later = Daylight::compute(LAT, day, t + 12.0);
        assert!((later.lamps_switched_s - 12.0 - d.lamps_switched_s).abs() < 0.01);
        assert!(Daylight::compute(LAT, day, t - 1.0).lamps_switched_s > 3600.0);
        // On all night, off from the dawn switch.
        assert!(Daylight::compute(LAT, day, 2.0 * HOUR).lamps_switched_s > 6.0 * HOUR);
        let mut t = 4.0 * HOUR;
        while Daylight::compute(LAT, day, t).streetlights_on {
            t += 0.1;
        }
        assert!(Daylight::compute(LAT, day, t).lamps_switched_s < 0.5);
    }

    #[test]
    fn morning_haze_settles_by_dawn_and_burns_off_by_mid_morning() {
        let day = 265;
        let at = |h: f32| Daylight::compute(LAT, day, h * HOUR);
        let (rise, _) = sun_times(LAT, day);
        let dawn = Daylight::compute(LAT, day, rise - 0.5 * HOUR);
        assert!(dawn.haze > 0.99, "{}", dawn.haze);
        for h in [10.0, 12.0, 17.0 + 40.0 / 60.0, 22.5, 0.5] {
            assert_eq!(at(h).haze, 0.0, "{h}");
        }
        // Thicker fog and greyer air at dawn than the same light in the evening.
        let dusk = at(19.0);
        assert!(dawn.fog_density > 3.0 * dusk.fog_density);
        assert!(dawn.turbidity > dusk.turbidity + 2.0);
        // It thins steadily through the morning.
        let mut prev = 1.0;
        for k in 0..=24 {
            let h = rise / HOUR + 0.25 * k as f32;
            let haze = at(h).haze;
            assert!(haze <= prev, "{h}");
            prev = haze;
        }
        assert_eq!(prev, 0.0);
    }

    #[test]
    fn streetlight_hysteresis_ignores_a_jittering_clock() {
        // Creep through the evening while scrubbing back and forth by ±90 s.
        let set = sun_times(LAT, MARCH_EQUINOX).1;
        let (mut latched, mut bare) = (false, false);
        let (mut latched_flips, mut bare_flips) = (0, 0);
        for i in 0..600 {
            let jitter = if i % 2 == 0 { 90.0 } else { -90.0 };
            let s = set + i as f32 * 10.0 + jitter;
            let elev = Daylight::compute(LAT, MARCH_EQUINOX, s).sun_elevation_deg;
            let next = streetlights_next(latched, elev);
            latched_flips += usize::from(next != latched);
            latched = next;
            let next_bare = elev < STREETLIGHTS_ON_DEG;
            bare_flips += usize::from(next_bare != bare);
            bare = next_bare;
        }
        assert!(latched);
        assert_eq!(latched_flips, 1);
        assert!(bare_flips > 1, "jitter should defeat a bare threshold");
    }

    #[test]
    fn phases_run_in_order_through_a_day() {
        let mut seen = Vec::new();
        for minute in 0..(24 * 60) {
            let p = Daylight::compute(LAT, JUNE_SOLSTICE, minute as f32 * 60.0).phase;
            if seen.last() != Some(&p) {
                seen.push(p);
            }
        }
        use Phase::*;
        assert_eq!(
            seen,
            [Night, Dawn, Sunrise, Day, GoldenHour, Sunset, Dusk, Night]
        );
    }

    #[test]
    fn full_moon_is_opposite_the_sun() {
        let lat = f64::from(LAT).to_radians();
        for (day, secs) in [(10, 0.0), (172, 30_000.0), (300, 70_000.0)] {
            let h = hour_angle(secs);
            let sun = to_f32(horizon_dir(lat, declination(day), h));
            let full = to_f32(moon_dir(lat, day, h, 0.5));
            let new = to_f32(moon_dir(lat, day, h, 0.0));
            assert!(dot(sun, full) < -0.98, "{}", dot(sun, full));
            assert!(dot(sun, new) > 0.98, "{}", dot(sun, new));
        }
        assert_eq!(moon_phase(1, 0.0), 0.0);
        assert!((moon_phase(16, 0.0) - 15.0 / SYNODIC_MONTH_DAYS).abs() < 1e-12);
    }

    #[test]
    fn outputs_are_unit_finite_and_in_range() {
        for lat in [-60.0, -20.0, 0.0, 34.0, 50.0, 66.0] {
            for day in (1..=DAYS_PER_YEAR).step_by(23) {
                for step in 0..96 {
                    let s = step as f32 * 900.0 + 7.0;
                    for haze in [0.0, 0.25, 1.0] {
                        let d = Daylight::compute_with(lat, day, s, Weather { haze });
                        assert!((len(d.sun_dir) - 1.0).abs() < 1e-5);
                        assert!((len(d.moon_dir) - 1.0).abs() < 1e-5);
                        assert!((-90.0..=90.0).contains(&d.sun_elevation_deg));
                        assert!((0.0..360.0).contains(&d.sun_azimuth_deg));
                        assert!((0.0..1.0).contains(&d.moon_phase));
                        assert!(d.sun_color.iter().all(|c| (0.0..=1.0).contains(c)));
                        for sky in [d.sky_zenith_color, d.sky_horizon_color, d.fog_color] {
                            for c in sky {
                                assert!(c.is_finite() && c >= 0.0);
                                assert!(c * d.exposure <= 1.0 + 1e-5);
                            }
                        }
                        for x in [d.sun_intensity, d.moon_intensity, d.exposure, d.fog_density] {
                            assert!(x.is_finite() && x >= 0.0);
                        }
                        assert!((0.0..=1.0).contains(&d.window_emissive_scale));
                        assert!((0.0..=1.0).contains(&d.star_brightness));
                        assert!((2.0..=6.0).contains(&d.turbidity));
                    }
                }
            }
        }
    }

    #[test]
    fn polar_sun_times_saturate() {
        assert_eq!(sun_times(80.0, JUNE_SOLSTICE), (0.0, 86_400.0));
        assert_eq!(sun_times(80.0, DECEMBER_SOLSTICE), (43_200.0, 43_200.0));
    }

    #[test]
    fn clock_at_10x_wraps_one_day_in_8640_real_seconds() {
        let mut clock = Clock::new(100, 6.0 * 3600.0);
        assert_eq!(clock.speed, 10.0);
        for _ in 0..(8640 * 60) {
            clock.advance(1.0 / 60.0);
        }
        assert_eq!(clock.day_of_year, 101);
        assert!((clock.seconds_of_day - 6.0 * 3600.0).abs() < 1e-3);
    }

    #[test]
    fn clock_scrub_and_speeds() {
        let mut clock = Clock::new(1, 600.0);
        clock.scrub(-1200.0);
        assert_eq!((clock.day_of_year, clock.seconds_of_day), (365, 85_800.0));
        clock.scrub(1200.0);
        assert_eq!((clock.day_of_year, clock.seconds_of_day), (1, 600.0));
        clock.scrub(-3.0 * DAY_SECONDS);
        assert_eq!(clock.day_of_year, 363);
        clock.speed = CLOCK_SPEEDS[0];
        clock.advance(1000.0);
        assert_eq!((clock.day_of_year, clock.seconds_of_day), (363, 600.0));
        clock.speed = CLOCK_SPEEDS[3];
        clock.advance(10.0);
        assert_eq!(clock.seconds_of_day, 1200.0);
    }
}
