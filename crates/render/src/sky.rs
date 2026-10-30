//! The analytic sky: Preetham, Shirley & Smits (1999), "A Practical Analytic Model for
//! Daylight", with a twilight fade, a night sky floor and a horizon haze term.
//!
//! The CPU evaluates the turbidity-dependent Perez coefficients and zenith values once per
//! frame and uploads them; the shader evaluates the Perez function per pixel. The same
//! radiance function is mirrored here to derive hemisphere ambient light and the haze color,
//! and to test the model. All radiance is luminance-scaled linear sRGB in kcd/m².

use crate::{Sky, Sun};
use glam::Vec3;

/// Angular radius of the drawn sun disc, radians (about twice the real 0.27° so it reads
/// at game resolutions; bloom will carry it further).
pub const SUN_ANGULAR_RADIUS: f32 = 0.5 * core::f32::consts::PI / 180.0;
/// Angular radius of the drawn moon disc, radians.
pub const MOON_ANGULAR_RADIUS: f32 = 0.5 * core::f32::consts::PI / 180.0;
/// Highest radiance written for the sun disc, kcd/m² (stays well inside f16).
const SUN_DISC_MAX: f32 = 30_000.0;

/// Perez coefficients A..E as (slope, offset) in turbidity, for luminance Y and
/// chromaticities x and y (Preetham et al., appendix A.2).
const PEREZ: [[[f32; 2]; 5]; 3] = [
    [
        [0.1787, -1.4630],
        [-0.3554, 0.4275],
        [-0.0227, 5.3251],
        [0.1206, -2.5771],
        [-0.0670, 0.3703],
    ],
    [
        [-0.0193, -0.2592],
        [-0.0665, 0.0008],
        [-0.0004, 0.2125],
        [-0.0641, -0.8989],
        [-0.0033, 0.0452],
    ],
    [
        [-0.0167, -0.2608],
        [-0.0950, 0.0092],
        [-0.0079, 0.2102],
        [-0.0441, -1.6537],
        [-0.0109, 0.0529],
    ],
];

/// Zenith chromaticity polynomials: rows multiply T², T, 1; columns θs³, θs², θs, 1.
const ZENITH_X: [[f32; 4]; 3] = [
    [0.00166, -0.00375, 0.00209, 0.0],
    [-0.02903, 0.06377, -0.03202, 0.00394],
    [0.11693, -0.21196, 0.06052, 0.25886],
];
const ZENITH_Y: [[f32; 4]; 3] = [
    [0.00275, -0.00610, 0.00317, 0.0],
    [-0.04214, 0.08970, -0.04153, 0.00516],
    [0.15346, -0.26756, 0.06670, 0.26688],
];

/// Deep blue the upper sky shifts toward after sunset, luminance 1.
pub(crate) const TWILIGHT_BLUE: [f32; 3] = [0.447, 0.894, 3.687];
/// Night sky floor color, luminance 1.
pub(crate) const NIGHT_BLUE: [f32; 3] = [0.69, 0.99, 1.98];

const LUMA: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

/// Converts the model's CIE xyY to linear sRGB. Preetham's colors are relative to an
/// equal-energy white, which reads pink on a D65 display, so they are white-balanced to D65
/// first (von Kries scaling in XYZ): a white haze then stays white.
fn yxy_to_rgb(yxy: Vec3) -> Vec3 {
    let (big_y, x, y) = (yxy.x, yxy.y, yxy.z.max(1e-4));
    let xyz = Vec3::new(
        x / y * big_y * 0.950_47,
        big_y,
        (1.0 - x - y) / y * big_y * 1.088_83,
    );
    Vec3::new(
        3.240_454 * xyz.x - 1.537_138 * xyz.y - 0.498_531 * xyz.z,
        -0.969_266 * xyz.x + 1.876_011 * xyz.y + 0.041_556 * xyz.z,
        0.055_643 * xyz.x - 0.204_026 * xyz.y + 1.057_225 * xyz.z,
    )
    .max(Vec3::ZERO)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Everything the sky shader needs, derived from [`Sun`] and [`Sky`] once per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SkyModel {
    /// Unit vector toward the sun as given.
    pub sun_dir: Vec3,
    /// Toward the sun, clamped to the horizon: drives the Perez gamma term.
    pub sun_sky: Vec3,
    /// Perez A..E, each for (Y, x, y).
    pub perez: [Vec3; 5],
    /// Zenith (Y, x, y) divided by F(0, θs): multiply by F(θ, γ) to get the sky value.
    pub zenith: Vec3,
    /// Twilight luminance fade, 1 with the sun up.
    pub day: f32,
    /// How far the upper sky has shifted to twilight blue, 0..1.
    pub blue: f32,
    /// Horizon haze strength, 0..1.
    pub haze: f32,
    /// Night floor radiance.
    pub night: Vec3,
    /// Mean horizon radiance (before haze): the haze color.
    pub horizon: Vec3,
    /// Cosine-weighted mean radiance of the sky hemisphere: ambient for an up normal.
    pub sky_up: Vec3,
    /// Radiance of the ground below the horizon: ambient for a down normal.
    pub ground: Vec3,
    /// Sun luminance factor E/π (kcd/m² on a white Lambertian surface facing the sun).
    pub sun_light: Vec3,
    /// Radiance at the sun disc's center.
    pub sun_disc: Vec3,
}

impl SkyModel {
    /// Whether the sun lights anything: false once its disc has sunk below the horizon.
    pub fn sun_lit(&self) -> bool {
        self.sun_light.max_element() > 0.0
    }

    pub fn new(sun: &Sun, sky: &Sky) -> SkyModel {
        let sun_dir = Vec3::from(sun.direction).normalize_or(Vec3::Y);
        let t = sky.turbidity.clamp(1.9, 10.0);
        let sun_sky = Vec3::new(sun_dir.x, sun_dir.y.max(0.0), sun_dir.z).normalize_or(Vec3::Y);
        let theta_s = sun_sky.y.clamp(0.0, 1.0).acos();

        let mut perez = [Vec3::ZERO; 5];
        for (k, p) in perez.iter_mut().enumerate() {
            *p = Vec3::new(
                PEREZ[0][k][0] * t + PEREZ[0][k][1],
                PEREZ[1][k][0] * t + PEREZ[1][k][1],
                PEREZ[2][k][0] * t + PEREZ[2][k][1],
            );
        }
        let chi = (4.0 / 9.0 - t / 120.0) * (core::f32::consts::PI - 2.0 * theta_s);
        let zenith_y = ((4.0453 * t - 4.9710) * chi.tan() - 0.2155 * t + 2.4192).max(0.0);
        let poly = |m: &[[f32; 4]; 3]| {
            let th = [theta_s.powi(3), theta_s * theta_s, theta_s, 1.0];
            let row = |r: &[f32; 4]| r.iter().zip(th).map(|(a, b)| a * b).sum::<f32>();
            t * t * row(&m[0]) + t * row(&m[1]) + row(&m[2])
        };
        let zenith = Vec3::new(zenith_y, poly(&ZENITH_X), poly(&ZENITH_Y));
        let f0 = perez_f(&perez, 1.0, theta_s, theta_s.cos());

        let elev_deg = sun_dir.y.clamp(-1.0, 1.0).asin().to_degrees();
        // Below the horizon the sky dims ~0.45 decades per degree (about 1/500 at the end
        // of civil twilight) and its upper part turns deep blue.
        let day = if elev_deg >= 0.0 {
            1.0
        } else {
            10f32.powf(0.45 * elev_deg.max(-24.0))
        };
        let blue = 0.85 * smoothstep(3.0, -5.0, elev_deg);
        let haze = ((t - 1.5) / 9.0).clamp(0.05, 0.6);
        let night = Vec3::from(NIGHT_BLUE) * sky.night_luminance.max(0.0);

        let mut model = SkyModel {
            sun_dir,
            sun_sky,
            perez,
            zenith: zenith / f0,
            day,
            blue,
            haze,
            night,
            horizon: Vec3::ZERO,
            sky_up: Vec3::ZERO,
            ground: Vec3::ZERO,
            sun_light: Vec3::ZERO,
            sun_disc: Vec3::ZERO,
        };

        // Haze color: the mean radiance just above the horizon, around the compass.
        let mut horizon = Vec3::ZERO;
        for k in 0..8 {
            let a = k as f32 * core::f32::consts::TAU / 8.0;
            horizon += model.base_radiance(Vec3::new(a.cos(), 0.02, a.sin()).normalize());
        }
        model.horizon = horizon / 8.0;

        // Hemisphere ambient: cosine-weighted mean of the sky (stratified, 4 rings × 8).
        let mut up = Vec3::ZERO;
        for ring in 0..4 {
            let sin_t = ((ring as f32 + 0.5) / 4.0).sqrt();
            let cos_t = (1.0 - sin_t * sin_t).sqrt();
            for k in 0..8 {
                let a = (k as f32 + 0.5 * ring as f32) * core::f32::consts::TAU / 8.0;
                up += model.radiance(Vec3::new(sin_t * a.cos(), cos_t, sin_t * a.sin()));
            }
        }
        model.sky_up = up / 32.0;

        // The sun stops lighting as its disc sinks below the horizon.
        let above = ((sun_dir.y + 0.01) / 0.03).clamp(0.0, 1.0);
        let illuminance = Vec3::from(sun.illuminance).max(Vec3::ZERO) * above;
        model.sun_light = illuminance / core::f32::consts::PI;
        let albedo = Vec3::from(sky.ground_albedo).max(Vec3::ZERO);
        model.ground = albedo * (model.sun_light * sun_dir.y.max(0.0) + model.sky_up);
        let solid_angle = core::f32::consts::PI * SUN_ANGULAR_RADIUS.sin().powi(2);
        let disc = Vec3::from(sun.illuminance).max(Vec3::ZERO) / solid_angle;
        model.sun_disc = disc * (SUN_DISC_MAX / disc.max_element().max(SUN_DISC_MAX));
        model
    }

    /// Preetham sky with twilight and the night floor, no haze (mirrors the shader).
    fn base_radiance(&self, dir: Vec3) -> Vec3 {
        let d = Vec3::new(dir.x, dir.y.max(0.0), dir.z).normalize_or(Vec3::Y);
        let cos_g = d.dot(self.sun_sky).clamp(-1.0, 1.0);
        let f = perez_f(&self.perez, d.y + 0.01, cos_g.acos(), cos_g);
        let mut c = yxy_to_rgb(self.zenith * f) * self.day;
        let w =
            self.blue * smoothstep(-0.05, 0.35, d.y) * (1.0 - 0.6 * smoothstep(0.5, 1.0, cos_g));
        c = c.lerp(Vec3::from(TWILIGHT_BLUE) * c.dot(LUMA), w);
        c + self.night
    }

    /// Diffuse sky radiance toward `dir` (unit, y up) in kcd/m², without the sun disc, moon
    /// or stars; below the horizon it is the ground. Mirrors `sky_radiance` in common.wgsl.
    pub fn radiance(&self, dir: Vec3) -> Vec3 {
        let mut c = self.base_radiance(dir);
        let up = dir.y.max(0.0);
        let flat = Vec3::new(dir.x, 0.0, dir.z);
        let sun_flat = Vec3::new(self.sun_sky.x, 0.0, self.sun_sky.z);
        let toward = if flat.length() > 1e-4 && sun_flat.length() > 1e-4 {
            flat.normalize().dot(sun_flat.normalize()).max(0.0).powi(4)
        } else {
            0.0
        };
        let haze = self.horizon * (1.0 + 0.8 * toward);
        c = c.lerp(haze, self.haze * (-up * 12.0).exp());
        self.ground.lerp(c, smoothstep(-0.03, 0.0, dir.y))
    }
}

/// The Perez sky function F(θ, γ) for (Y, x, y) at once.
fn perez_f(p: &[Vec3; 5], cos_theta: f32, gamma: f32, cos_gamma: f32) -> Vec3 {
    let [a, b, c, d, e] = *p;
    let ct = cos_theta.max(1e-3);
    (Vec3::ONE + a * (b / ct).exp())
        * (Vec3::ONE + c * (d * gamma).exp() + e * cos_gamma * cos_gamma)
}

/// Exposure multiplier (from luminance in kcd/m² to tonemapper input) for a camera
/// exposure value at ISO 100: `1000 / (1.2 · 2^ev100)`. With the AgX tonemap and its look
/// (which map 0.18 to about 0.15) good values run about a stop under a light meter: sunny
/// day ≈ EV 14, golden hour ≈ 11–12, blue hour ≈ 7–8, street-lit night ≈ 1–3.
pub fn exposure_from_ev100(ev100: f32) -> f32 {
    1000.0 / (1.2 * 2f32.powf(ev100))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(elev_deg: f32, turbidity: f32) -> SkyModel {
        let e = elev_deg.to_radians();
        let sun = Sun {
            direction: [0.0, e.sin(), -e.cos()],
            illuminance: [100.0, 97.0, 90.0],
        };
        let sky = Sky {
            turbidity,
            ..Sky::default()
        };
        SkyModel::new(&sun, &sky)
    }

    fn dir(elev_deg: f32, azimuth_deg: f32) -> Vec3 {
        let (e, a) = (elev_deg.to_radians(), azimuth_deg.to_radians());
        // Azimuth 0 faces the sun (-Z in `model`).
        Vec3::new(e.cos() * a.sin(), e.sin(), -e.cos() * a.cos())
    }

    #[test]
    fn noon_zenith_matches_preetham_magnitudes() {
        // Preetham's zenith formula for T = 3 and the sun at 60° elevation gives
        // (4.0453·3 − 4.9710)·tan(0.878) − 0.2155·3 + 2.4192 ≈ 10.4 kcd/m².
        let m = model(60.0, 3.0);
        let z = m.radiance(Vec3::Y);
        let lum = z.dot(LUMA);
        assert!((lum - 10.37).abs() < 0.2, "{lum}");
        // A clear zenith is blue.
        assert!(z.z > z.y && z.y > z.x, "{z}");
    }

    #[test]
    fn horizon_is_brighter_and_warmer_toward_the_sun() {
        for elev in [5.0, 20.0, 45.0] {
            let m = model(elev, 3.0);
            let toward = m.radiance(dir(3.0, 0.0));
            let away = m.radiance(dir(3.0, 180.0));
            let zenith = m.radiance(Vec3::Y);
            assert!(toward.dot(LUMA) > away.dot(LUMA), "{elev}");
            assert!(
                away.dot(LUMA) > 0.6 * zenith.dot(LUMA),
                "{elev}: horizon not brighter"
            );
            // Warmer: more red relative to blue toward the sun than at the zenith.
            assert!(
                toward.x / toward.z > zenith.x / zenith.z,
                "{elev}: {toward} {zenith}"
            );
        }
    }

    #[test]
    fn hazier_sky_is_whiter() {
        let sat = |c: Vec3| (c.max_element() - c.min_element()) / c.max_element();
        let clear = model(40.0, 2.0).radiance(dir(30.0, 90.0));
        let hazy = model(40.0, 8.0).radiance(dir(30.0, 90.0));
        assert!(sat(hazy) < sat(clear), "{clear} {hazy}");
    }

    #[test]
    fn sky_fades_to_night_blue_below_the_horizon() {
        let day = model(10.0, 3.0).radiance(Vec3::Y).dot(LUMA);
        let set = model(0.0, 3.0).radiance(Vec3::Y).dot(LUMA);
        let civil = model(-6.0, 3.0).radiance(Vec3::Y);
        let night = model(-30.0, 3.0).radiance(Vec3::Y);
        assert!(
            day > set && set > 100.0 * civil.dot(LUMA),
            "{day} {set} {civil}"
        );
        assert!(civil.z > civil.x, "twilight zenith is blue: {civil}");
        let floor = Sky::default().night_luminance;
        assert!((night.dot(LUMA) / floor - 1.0).abs() < 0.05, "{night}");
        assert!(night.z > night.y && night.y > night.x);
        // Continuous through sunset.
        let a = model(0.01, 3.0).radiance(Vec3::Y).dot(LUMA);
        let b = model(-0.01, 3.0).radiance(Vec3::Y).dot(LUMA);
        assert!((a - b).abs() / a < 0.05, "{a} {b}");
    }

    #[test]
    fn ambient_follows_the_sky() {
        let m = model(45.0, 3.0);
        let zenith = m.radiance(Vec3::Y);
        let horizon = m.horizon;
        // The hemisphere mean lies between zenith and horizon brightness.
        let up = m.sky_up.dot(LUMA);
        let (lo, hi) = (
            zenith.dot(LUMA).min(horizon.dot(LUMA)),
            zenith.dot(LUMA).max(horizon.dot(LUMA)),
        );
        assert!(up > lo * 0.8 && up < hi * 1.5, "{up} {lo} {hi}");
        // Sky illuminance (π · mean radiance) is a fraction of the sun's on the ground:
        // Preetham's bright horizon puts it near 40% at T = 3; measured clear skies give
        // 20-30%.
        let e_sky = core::f32::consts::PI * up;
        let e_sun = 97.0 * 45f32.to_radians().sin();
        assert!((0.15..0.5).contains(&(e_sky / e_sun)), "{}", e_sky / e_sun);
        // Below the horizon is the lit ground.
        assert_eq!(m.radiance(-Vec3::Y), m.ground);
        assert!(m.ground.dot(LUMA) > 0.0);
    }

    #[test]
    fn sun_disc_is_bounded_and_sun_light_fades_at_the_horizon() {
        let m = model(30.0, 3.0);
        assert!(m.sun_disc.max_element() <= SUN_DISC_MAX * 1.0001);
        assert!(m.sun_disc.x > m.sun_disc.z);
        assert!((m.sun_light.x - 100.0 / core::f32::consts::PI).abs() < 1e-3);
        assert_eq!(model(-3.0, 3.0).sun_light, Vec3::ZERO);
    }

    #[test]
    fn radiance_is_finite_everywhere() {
        for elev in [-90.0, -10.0, -0.5, 0.0, 0.5, 10.0, 89.0, 90.0] {
            for t in [1.0, 2.0, 5.0, 12.0] {
                let m = model(elev, t);
                for k in 0..64 {
                    let d = dir(k as f32 * 5.6 - 90.0, k as f32 * 37.0);
                    let c = m.radiance(d);
                    assert!(
                        c.is_finite() && c.min_element() >= 0.0,
                        "{elev} {t} {d}: {c}"
                    );
                }
            }
        }
    }

    #[test]
    fn exposure_from_ev() {
        // EV 15 maps a sunlit white wall (~30 kcd/m²) to just under 1.
        let e = exposure_from_ev100(15.0);
        assert!((30.0 * e - 0.76).abs() < 0.02, "{e}");
        assert!((exposure_from_ev100(14.0) / e - 2.0).abs() < 1e-4);
    }
}
