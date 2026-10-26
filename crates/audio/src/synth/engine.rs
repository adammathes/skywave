//! The engine: an oscillator bank on the firing frequency, shaped per car by an
//! [`EngineProfile`] (the spec's `feel.engine_sound`).
//!
//! One phase runs at the engine-cycle rate, `rpm / 120` (a four-stroke fires every cylinder
//! once per two revolutions), and every partial is a whole multiple of it, so nothing
//! drifts or clicks however the rpm moves:
//! - harmonics 1..=N of the firing frequency `rpm × cylinders / 120`, amplitudes per profile;
//! - a sub-oscillator at half the firing frequency;
//! - each firing scaled by its cylinder's gain (an uneven pattern gives a cross-plane V8 its
//!   burble) and a little random variation, most at idle (cycle-to-cycle combustion);
//! - exhaust and intake noise (band-passed white noise riding the firing pulse), louder
//!   with throttle;
//! - all through a low-pass that opens with throttle and rpm.
//!
//! Events: lifting off the throttle at high rpm schedules a few exhaust pops (and a closed
//! throttle on the overrun crackles now and then); at the redline under throttle the
//! limiter cuts the sound at `limiter_hz`; a change between forward gears dips it for the
//! gearbox's torque cut. Every parameter is slewed per sample.

use super::dsp::{fract, sin_turns, unit, Decay, Noise, Slew, Svf, DT};
use super::EngineParams;

/// Most harmonics a profile may use.
pub const HARMONICS: usize = 10;
/// Most cylinders a profile may have (the length of [`Voice::pattern`]).
pub const MAX_CYLINDERS: usize = 8;
/// Filter coefficients and events are updated every this many samples.
const TICK: usize = 32;
/// Overall engine level at full throttle and redline, before the bus gain.
const LEVEL: f32 = 0.55;

/// The engine sounds a car spec may name in `feel.engine_sound`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineProfile {
    /// A flat-plane V8: even firing, bright and rasping, revs freely, crackles.
    V8Flatplane = 0,
    /// A cross-plane V8 in no hurry: uneven firing, a deep burble, dark.
    V8Lazy = 1,
    /// An inline four: buzzy, thin at the bottom, eager at the top.
    I4 = 2,
    /// A V6: smooth and mid-weighted.
    V6 = 3,
    /// Anything else: a neutral six.
    #[default]
    Default = 4,
}

impl EngineProfile {
    pub const ALL: [EngineProfile; 5] = [
        EngineProfile::V8Flatplane,
        EngineProfile::V8Lazy,
        EngineProfile::I4,
        EngineProfile::V6,
        EngineProfile::Default,
    ];

    /// The profile named by a spec's `engine_sound`; unknown names get [`EngineProfile::Default`].
    pub fn from_name(name: &str) -> EngineProfile {
        EngineProfile::ALL
            .into_iter()
            .find(|p| p.name() == name)
            .unwrap_or_default()
    }

    pub fn name(self) -> &'static str {
        match self {
            EngineProfile::V8Flatplane => "v8_flatplane",
            EngineProfile::V8Lazy => "v8_lazy",
            EngineProfile::I4 => "i4",
            EngineProfile::V6 => "v6",
            EngineProfile::Default => "default",
        }
    }

    pub fn voice(self) -> &'static Voice {
        match self {
            EngineProfile::V8Flatplane => &V8_FLATPLANE,
            EngineProfile::V8Lazy => &V8_LAZY,
            EngineProfile::I4 => &I4,
            EngineProfile::V6 => &V6,
            EngineProfile::Default => &DEFAULT,
        }
    }
}

/// What a profile sounds like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voice {
    pub cylinders: u8,
    /// Amplitudes of harmonics 1.. of the firing frequency; the first `n_harmonics` are used.
    pub harmonics: [f32; HARMONICS],
    pub n_harmonics: usize,
    /// Sub-oscillator at half the firing frequency, relative to harmonic 1.
    pub sub: f32,
    /// Gain of each cylinder's firing, in firing order (the first `cylinders` are used).
    pub pattern: [f32; MAX_CYLINDERS],
    /// Random variation of each firing at idle, fading to a third of it at the redline.
    pub jitter: f32,
    /// Exhaust rasp and intake roar levels.
    pub exhaust: f32,
    pub intake: f32,
    /// Low-pass cutoff at idle, and how far full throttle opens it, Hz.
    pub lp_base_hz: f32,
    pub lp_open_hz: f32,
    /// Exhaust pop level (0: never pops).
    pub pops: f32,
    /// Rev-limiter cut rate, Hz.
    pub limiter_hz: f32,
    /// Overall level, relative.
    pub level: f32,
}

const V8_FLATPLANE: Voice = Voice {
    cylinders: 8,
    harmonics: [1.0, 0.7, 0.55, 0.5, 0.36, 0.3, 0.22, 0.18, 0.14, 0.1],
    n_harmonics: 10,
    sub: 0.18,
    pattern: [1.0, 0.96, 1.03, 0.98, 1.02, 0.97, 1.04, 1.0],
    jitter: 0.14,
    exhaust: 1.0,
    intake: 0.7,
    lp_base_hz: 900.0,
    lp_open_hz: 4500.0,
    pops: 1.0,
    limiter_hz: 16.0,
    level: 1.0,
};

const V8_LAZY: Voice = Voice {
    cylinders: 8,
    harmonics: [1.0, 0.42, 0.24, 0.15, 0.1, 0.06, 0.04, 0.0, 0.0, 0.0],
    n_harmonics: 7,
    sub: 0.6,
    pattern: [1.2, 0.78, 1.05, 0.88, 1.18, 0.82, 0.95, 1.12],
    jitter: 0.2,
    exhaust: 0.8,
    intake: 0.35,
    lp_base_hz: 480.0,
    lp_open_hz: 2200.0,
    pops: 0.6,
    limiter_hz: 12.0,
    level: 1.1,
};

const I4: Voice = Voice {
    cylinders: 4,
    harmonics: [1.0, 0.85, 0.6, 0.55, 0.45, 0.36, 0.3, 0.25, 0.2, 0.15],
    n_harmonics: 10,
    sub: 0.12,
    pattern: [1.0, 0.97, 1.02, 0.99, 1.0, 1.0, 1.0, 1.0],
    jitter: 0.12,
    exhaust: 0.8,
    intake: 1.0,
    lp_base_hz: 1100.0,
    lp_open_hz: 5000.0,
    pops: 0.5,
    limiter_hz: 18.0,
    level: 0.9,
};

const V6: Voice = Voice {
    cylinders: 6,
    harmonics: [1.0, 0.6, 0.42, 0.3, 0.22, 0.16, 0.12, 0.08, 0.0, 0.0],
    n_harmonics: 8,
    sub: 0.28,
    pattern: [1.04, 0.95, 1.02, 0.97, 1.05, 0.96, 1.0, 1.0],
    jitter: 0.13,
    exhaust: 0.85,
    intake: 0.65,
    lp_base_hz: 750.0,
    lp_open_hz: 3500.0,
    pops: 0.4,
    limiter_hz: 15.0,
    level: 1.0,
};

const DEFAULT: Voice = V6;

/// The engine sound of one car.
#[derive(Clone, Debug)]
pub struct EngineSynth {
    target: EngineParams,
    voice: Voice,
    /// 1 / the sum of the tone's partial amplitudes.
    norm: f32,
    /// The first parameters snap the slews instead of sweeping from the defaults.
    fresh: bool,
    rpm: Slew,
    throttle: Slew,
    /// Shift dip and limiter gate, multipliers.
    dip: Slew,
    gate: Slew,
    fire_gain: Slew,
    fire_target: f32,
    /// Engine-cycle phase, `[0, 1)`, and the cylinder firing now.
    phase: f32,
    cylinder: u32,
    lp: Svf,
    exhaust: Svf,
    intake: Svf,
    noise: Noise,
    /// Seconds left of the shift's torque cut.
    shift_left: f32,
    last_throttle: f32,
    pops_left: u32,
    pop_timer: f32,
    pop_env: Decay,
    pop_kick: Decay,
    pop_bp: Svf,
    pop_phase: f32,
    limiter_phase: f32,
    idle_phase: f32,
    /// Per-tick values: tone, exhaust and intake amplitudes, and whether the limiter cuts.
    tone_amp: f32,
    exhaust_amp: f32,
    intake_amp: f32,
    jitter: f32,
    limiting: bool,
    /// Samples into the current tick.
    tick: usize,
}

impl Default for EngineSynth {
    fn default() -> Self {
        EngineSynth::new()
    }
}

impl EngineSynth {
    pub fn new() -> EngineSynth {
        let target = EngineParams::default();
        let mut s = EngineSynth {
            target,
            voice: *target.profile.voice(),
            norm: 1.0,
            fresh: true,
            rpm: Slew::new(target.rpm, 0.02),
            throttle: Slew::new(0.0, 0.03),
            dip: Slew::new(1.0, 0.015),
            gate: Slew::new(1.0, 0.003),
            fire_gain: Slew::new(1.0, 0.002),
            fire_target: 1.0,
            phase: 0.0,
            cylinder: 0,
            lp: Svf::new(1000.0, 0.8),
            exhaust: Svf::new(600.0, 1.0),
            intake: Svf::new(2000.0, 1.5),
            noise: Noise::new(0x5eed_e791),
            shift_left: 0.0,
            last_throttle: 0.0,
            pops_left: 0,
            pop_timer: 0.0,
            pop_env: Decay::SILENT,
            pop_kick: Decay::SILENT,
            pop_bp: Svf::new(500.0, 1.4),
            pop_phase: 0.0,
            limiter_phase: 0.0,
            idle_phase: 0.0,
            tone_amp: 0.0,
            exhaust_amp: 0.0,
            intake_amp: 0.0,
            jitter: 0.0,
            limiting: false,
            tick: 0,
        };
        s.set_voice(target.profile);
        s
    }

    fn set_voice(&mut self, profile: EngineProfile) {
        self.voice = *profile.voice();
        let v = &self.voice;
        let sum: f32 = v.harmonics[..v.n_harmonics].iter().sum::<f32>() + v.sub;
        self.norm = if sum > 0.0 { 1.0 / sum } else { 1.0 };
    }

    /// The engine this frame (already sanitized). Notes gear changes and lift-offs.
    pub fn set_params(&mut self, p: &EngineParams) {
        if p.profile != self.target.profile || self.fresh {
            self.set_voice(p.profile);
        }
        if self.fresh {
            self.rpm.value = p.rpm;
            self.throttle.value = p.throttle;
            self.last_throttle = p.throttle;
            self.fresh = false;
        }
        let old = self.target;
        if old.gear >= 1 && p.gear >= 1 && p.gear != old.gear {
            self.shift_left = p.shift_ms * 0.001;
        }
        let span = p.redline_rpm - p.idle_rpm;
        let rpm_n = unit((p.rpm - p.idle_rpm) / span);
        if self.last_throttle > 0.55 && p.throttle < 0.2 && rpm_n > 0.5 && self.voice.pops > 0.0 {
            self.pops_left = 2 + self.noise.next_u32() % 4;
            self.pop_timer = self.noise.range(0.03, 0.09);
        }
        self.last_throttle = p.throttle;
        self.target = *p;
    }

    /// Fills `out` (mono) with the next `out.len()` samples.
    pub fn render(&mut self, out: &mut [f32]) {
        let cyl = f32::from(self.voice.cylinders.clamp(1, MAX_CYLINDERS as u8));
        let n_harm = self.voice.n_harmonics.min(HARMONICS);
        for o in out.iter_mut() {
            if self.tick == 0 {
                self.update_tick();
            }
            self.tick = (self.tick + 1) % TICK;
            *o = self.sample(cyl, n_harm);
        }
    }

    /// Once per [`TICK`]: filter coefficients, amplitudes and events.
    fn update_tick(&mut self) {
        let t = self.target;
        let v = self.voice;
        let dt = TICK as f32 * DT;
        let span = t.redline_rpm - t.idle_rpm;
        let rpm_n = unit((self.rpm.value - t.idle_rpm) / span);
        let thr = self.throttle.value;

        // Shift: the torque cut.
        if self.shift_left > 0.0 {
            self.shift_left -= dt;
        }
        // Limiter: at the redline under throttle, cut at `limiter_hz`.
        self.limiting = t.rpm >= 0.985 * t.redline_rpm && t.throttle > 0.5;
        if self.limiting {
            self.limiter_phase = fract(self.limiter_phase + v.limiter_hz * dt);
        } else {
            self.limiter_phase = 0.0;
        }
        self.idle_phase = fract(self.idle_phase + 0.8 * dt);

        // Pops after a lift-off, and an occasional crackle on a closed throttle up high.
        if self.pops_left > 0 {
            self.pop_timer -= dt;
            if self.pop_timer <= 0.0 {
                let level = self.noise.range(0.5, 1.0);
                self.pop(level);
                self.pops_left -= 1;
                self.pop_timer = self.noise.range(0.04, 0.16);
            }
        } else if t.throttle < 0.05 && t.gear >= 1 && rpm_n > 0.45 && v.pops > 0.0 {
            let chance = dt * 2.5 * (rpm_n - 0.45) * 2.0;
            if self.noise.unit() < chance {
                let level = self.noise.range(0.25, 0.55);
                self.pop(level);
            }
        }

        let dip = self.dip.value;
        let lp_hz =
            (v.lp_base_hz * (0.8 + 1.6 * rpm_n) + v.lp_open_hz * thr * thr) * (0.45 + 0.55 * dip);
        self.lp.set(lp_hz, 0.8);
        self.exhaust.set(300.0 + 1500.0 * rpm_n, 1.0);
        self.intake.set(1200.0 + 2800.0 * rpm_n, 1.5);
        self.tone_amp = (0.5 + 0.5 * thr) * (0.75 + 0.25 * rpm_n);
        self.exhaust_amp = 1.6 * v.exhaust * (0.2 + 0.8 * thr) * (0.35 + 0.65 * rpm_n);
        self.intake_amp = 1.8 * v.intake * thr * thr * (0.3 + 0.7 * rpm_n);
        self.jitter = v.jitter * (1.0 - 0.67 * rpm_n);
    }

    fn pop(&mut self, level: f32) {
        let l = level * self.voice.pops;
        let tau = self.noise.range(0.004, 0.012);
        self.pop_env = Decay::hit(l, tau);
        self.pop_kick = Decay::hit(0.7 * l, 0.018);
        let hz = self.noise.range(250.0, 900.0);
        self.pop_bp.set(hz, 1.4);
        self.pop_phase = 0.0;
    }

    #[inline]
    fn sample(&mut self, cyl: f32, n_harm: usize) -> f32 {
        let t = &self.target;
        let v = &self.voice;
        // Targets with the idle hunt and the limiter's rpm sag.
        let idle_hunt = if t.throttle < 0.1 && t.rpm < 1.15 * t.idle_rpm {
            1.0 + 0.012 * sin_turns(self.idle_phase)
        } else {
            1.0
        };
        let gate_target = if self.limiting && self.limiter_phase > 0.55 {
            0.25
        } else {
            1.0
        };
        let gate = self.gate.step(gate_target);
        let rpm = self
            .rpm
            .step(t.rpm * idle_hunt * (1.0 - 0.04 * (1.0 - gate)));
        self.throttle.step(t.throttle);
        let dip = self.dip.step(if self.shift_left > 0.0 { 0.5 } else { 1.0 });

        // The engine cycle, and which cylinder fires.
        self.phase += rpm * (DT / 120.0);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let fire = self.phase * cyl;
        let cylinder = fire as u32;
        if cylinder != self.cylinder {
            self.cylinder = cylinder;
            let pattern = v.pattern[(cylinder as usize).min(MAX_CYLINDERS - 1)];
            self.fire_target = pattern * (1.0 + self.jitter * self.noise.white());
        }
        let fire_gain = self.fire_gain.step(self.fire_target);
        let ff = fract(fire);

        let mut tone = v.sub * sin_turns(0.5 * fire);
        let mut k = 1.0;
        for &a in &v.harmonics[..n_harm] {
            tone += a * sin_turns(ff * k);
            k += 1.0;
        }
        tone *= self.norm * fire_gain * self.tone_amp;

        let pulse = 0.55 + 0.45 * sin_turns(ff);
        let exhaust = self.exhaust.band(self.noise.white()) * self.exhaust_amp * pulse;
        let intake = self.intake.band(self.noise.white()) * self.intake_amp;
        let body = self.lp.low(tone + exhaust + intake);

        let mut pop = 0.0;
        if self.pop_env.active() || self.pop_kick.active() {
            let crack = self.pop_bp.band(self.noise.white()) * self.pop_env.step() * 6.0;
            let kick = sin_turns(self.pop_phase) * self.pop_kick.step();
            self.pop_phase = fract(self.pop_phase + 70.0 * DT);
            pop = crack + kick;
        }
        let y = (body * dip * gate + 0.5 * pop) * LEVEL * v.level;
        if y.is_finite() {
            y
        } else {
            self.reset_filters();
            0.0
        }
    }

    fn reset_filters(&mut self) {
        self.lp.reset();
        self.exhaust.reset();
        self.intake.reset();
        self.pop_bp.reset();
    }

    /// The firing frequency now, Hz (for tests and the preview tool).
    pub fn firing_hz(&self) -> f32 {
        self.rpm.value * f32::from(self.voice.cylinders) / 120.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::dsp::SAMPLE_RATE;

    fn params(rpm: f32, throttle: f32) -> EngineParams {
        EngineParams {
            profile: EngineProfile::V8Flatplane,
            rpm,
            idle_rpm: 900.0,
            redline_rpm: 7200.0,
            throttle,
            gear: 3,
            shift_ms: 250.0,
        }
    }

    /// Magnitude of the `hz` component of `x` (a single DFT bin, Goertzel-free and slow).
    fn bin(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let t = f64::from(hz) * i as f64 / f64::from(SAMPLE_RATE);
            let a = t * core::f64::consts::TAU;
            re += f64::from(v) * a.cos();
            im += f64::from(v) * a.sin();
        }
        ((re * re + im * im).sqrt() * 2.0 / x.len() as f64) as f32
    }

    fn render(p: EngineParams, seconds: f32) -> Vec<f32> {
        let mut e = EngineSynth::new();
        e.set_params(&p);
        let mut warm = vec![0.0; 9600];
        e.render(&mut warm);
        let mut out = vec![0.0; (seconds * SAMPLE_RATE) as usize];
        e.render(&mut out);
        out
    }

    #[test]
    fn profiles_resolve_by_name() {
        for p in EngineProfile::ALL {
            assert_eq!(EngineProfile::from_name(p.name()), p);
            assert!(usize::from(p.voice().cylinders) <= MAX_CYLINDERS);
            assert!(p.voice().n_harmonics >= 6 && p.voice().n_harmonics <= HARMONICS);
        }
        assert_eq!(EngineProfile::from_name("rotary"), EngineProfile::Default);
    }

    #[test]
    fn pitch_tracks_rpm() {
        // Firing frequency = rpm × 8 / 120: 60 Hz at idle, 400 Hz at 6000.
        for (rpm, fire) in [(900.0, 60.0), (3000.0, 200.0), (6000.0, 400.0)] {
            let x = render(params(rpm, 0.6), 0.5);
            let at = bin(&x, fire);
            let off = bin(&x, fire * 1.37);
            assert!(
                at > 4.0 * off,
                "{rpm} rpm: {at} at {fire} Hz vs {off} off it"
            );
            let second = bin(&x, 2.0 * fire);
            assert!(second > 0.1 * at, "{rpm} rpm: harmonics present");
        }
    }

    #[test]
    fn output_is_bounded_and_has_no_dc() {
        for profile in EngineProfile::ALL {
            for (rpm, thr) in [(900.0, 0.0), (4000.0, 1.0), (7200.0, 1.0)] {
                let mut p = params(rpm, thr);
                p.profile = profile;
                let x = render(p, 0.5);
                let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                let mean = x.iter().map(|&v| f64::from(v)).sum::<f64>() / x.len() as f64;
                assert!(peak > 0.02 && peak < 1.0, "{profile:?} {rpm}: peak {peak}");
                assert!(mean.abs() < 0.01, "{profile:?} {rpm}: dc {mean}");
            }
        }
    }

    #[test]
    fn a_shift_dips_and_a_lift_pops() {
        let mut e = EngineSynth::new();
        let mut p = params(6500.0, 1.0);
        e.set_params(&p);
        let mut buf = vec![0.0; 4800];
        e.render(&mut buf);
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let before = rms(&buf);
        p.gear = 4;
        p.rpm = 5000.0;
        e.set_params(&p);
        e.render(&mut buf[..2400]);
        let during = rms(&buf[1200..2400]);
        assert!(during < 0.85 * before, "dip: {during} vs {before}");
        // Lift-off at high rpm queues pops.
        p.throttle = 0.0;
        p.rpm = 6000.0;
        e.set_params(&p);
        assert!(e.pops_left >= 2);
    }

    #[test]
    fn the_limiter_stutters() {
        let x = render(params(7200.0, 1.0), 1.0);
        // Envelope in 5 ms windows swings with the cut.
        let env: Vec<f32> = x
            .chunks(240)
            .map(|c| c.iter().fold(0.0f32, |m, v| m.max(v.abs())))
            .collect();
        let lo = env.iter().fold(1.0f32, |m, &v| m.min(v));
        let hi = env.iter().fold(0.0f32, |m, &v| m.max(v));
        assert!(lo < 0.5 * hi, "{lo} {hi}");
    }

    #[test]
    fn block_cost() {
        let mut e = EngineSynth::new();
        e.set_params(&params(5000.0, 0.8));
        let mut buf = [0.0f32; 256];
        // The best of ten runs, so other tests running alongside do not count.
        let mut per = f64::INFINITY;
        for _ in 0..10 {
            let t = std::time::Instant::now();
            for _ in 0..200 {
                e.render(&mut buf);
            }
            per = per.min(t.elapsed().as_secs_f64() * 1000.0 / 200.0);
        }
        println!("engine synth: {per:.4} ms per 256-frame block");
        if !cfg!(debug_assertions) {
            assert!(per < 0.3, "{per} ms");
        }
    }
}
