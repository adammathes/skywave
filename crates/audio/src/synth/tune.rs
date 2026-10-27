//! The radio dial between stations: [`crate::TUNE_SECONDS`] of a sweep across the band, on
//! the ui bus, at the radio's volume.
//!
//! Static whose band-pass center sweeps up the dial (500 Hz to 4 kHz, with a little hand
//! jitter) and crackles, a faint carrier whistle gliding down as the dial closes on the
//! station (the beat between the two falling toward zero), then a squelch click that shuts
//! it all off, and silence until the station starts at the end of the burst. A new tune
//! (a press while one plays) starts the burst again.

use super::dsp::{exp2, fract, sin_turns, smoothstep, Decay, Noise, Svf, DT};
use crate::radio::TUNE_SECONDS;

/// When the squelch closes, s: the static and whistle stop and the click sounds.
const SQUELCH_S: f32 = TUNE_SECONDS - 0.08;
/// The static's band-pass center sweeps from here, Hz...
const SWEEP_FROM_HZ: f32 = 500.0;
/// ...up this many octaves by the squelch.
const SWEEP_OCTAVES: f32 = 3.0;
/// The whistle glides from here, Hz, down this many octaves (to ~260 Hz) by the squelch,
/// starting at `WHISTLE_START_S`.
const WHISTLE_FROM_HZ: f32 = 2400.0;
const WHISTLE_OCTAVES: f32 = 3.2;
const WHISTLE_START_S: f32 = 0.08;
/// Levels before the radio volume: static, crackle pops, whistle, click.
const STATIC_LEVEL: f32 = 0.42;
const CRACKLE_LEVEL: f32 = 0.5;
const WHISTLE_LEVEL: f32 = 0.05;
const CLICK_LEVEL: f32 = 0.35;
/// Crackle pops per second, on average.
const CRACKLES_PER_S: f32 = 40.0;

/// The tune burst.
#[derive(Clone, Debug)]
pub struct TuneSynth {
    /// The last [`super::AudioParams::tunes`] seen.
    count: u32,
    /// Seconds into the burst; at or past [`TUNE_SECONDS`] it is silent.
    t: f32,
    volume: f32,
    noise: Noise,
    band: Svf,
    crackle: Decay,
    crackle_bp: Svf,
    /// The receiver's bandwidth: a low-pass over the static.
    tone: Svf,
    whistle_phase: f32,
    click: Decay,
    click_phase: f32,
}

impl Default for TuneSynth {
    fn default() -> Self {
        TuneSynth::new()
    }
}

impl TuneSynth {
    pub fn new() -> TuneSynth {
        TuneSynth {
            count: 0,
            t: TUNE_SECONDS,
            volume: 0.0,
            noise: Noise::new(0x7475_6e65),
            band: Svf::new(SWEEP_FROM_HZ, 1.4),
            crackle: Decay::SILENT,
            crackle_bp: Svf::new(1800.0, 0.8),
            tone: Svf::new(4500.0, 0.707),
            whistle_phase: 0.0,
            click: Decay::SILENT,
            click_phase: 0.0,
        }
    }

    /// This frame's tune count and radio volume (sanitized): a new count starts a burst.
    pub fn set_params(&mut self, tunes: u32, volume: f32) {
        if tunes != self.count {
            self.count = tunes;
            self.t = 0.0;
            self.band.reset();
            self.crackle_bp.reset();
            self.tone.reset();
            self.whistle_phase = 0.0;
            self.crackle = Decay::SILENT;
            self.click = Decay::SILENT;
        }
        self.volume = volume;
    }

    /// Sounding now (the mixer skips it otherwise).
    pub fn active(&self) -> bool {
        self.t < TUNE_SECONDS
    }

    /// Adds the burst to the interleaved stereo `out`.
    pub fn render(&mut self, out: &mut [f32]) {
        for f in out.chunks_exact_mut(2) {
            if !self.active() {
                return;
            }
            let v = self.sample() * self.volume;
            f[0] += v;
            f[1] += v;
        }
    }

    fn sample(&mut self) -> f32 {
        let t = self.t;
        self.t += DT;
        let white = self.noise.white();
        let open = smoothstep(0.0, 0.012, t) * (1.0 - smoothstep(SQUELCH_S - 0.004, SQUELCH_S, t));
        let mut out = 0.0;
        if open > 0.0 {
            // Static: the band-pass center sweeps up the dial with a 7 Hz jitter.
            let u = t / SQUELCH_S;
            let jitter = 1.0 + 0.12 * sin_turns(7.0 * t);
            self.band
                .set(SWEEP_FROM_HZ * exp2(SWEEP_OCTAVES * u) * jitter, 1.4);
            let fizz = self.band.band(white);
            // Crackle: sparse pops, each a short decaying burst of the same noise.
            if self.noise.unit() < CRACKLES_PER_S * DT {
                self.crackle = Decay::hit(self.noise.range(0.4, 1.0), 0.002);
            }
            let pop = self.crackle_bp.band(white * self.crackle.step());
            // Whistle: the carrier beating against the dial, gliding down, swelling in.
            let mut whistle = 0.0;
            if t >= WHISTLE_START_S {
                let w = (t - WHISTLE_START_S) / (SQUELCH_S - WHISTLE_START_S);
                let hz = WHISTLE_FROM_HZ * exp2(-WHISTLE_OCTAVES * w);
                self.whistle_phase = fract(self.whistle_phase + hz * DT);
                whistle = sin_turns(self.whistle_phase) * smoothstep(0.0, 0.4, w);
            }
            let hiss = self.tone.low(STATIC_LEVEL * fizz + CRACKLE_LEVEL * pop);
            out = (hiss + WHISTLE_LEVEL * whistle) * open;
        }
        // The squelch shutting: one click.
        if t < SQUELCH_S && self.t >= SQUELCH_S {
            self.click = Decay::hit(CLICK_LEVEL, 0.003);
        }
        if self.click.active() {
            self.click_phase = fract(self.click_phase + 1400.0 * DT);
            out += self.click.step() * (sin_turns(self.click_phase) + 0.3 * white);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(volume: f32) -> Vec<f32> {
        let mut s = TuneSynth::new();
        s.set_params(1, volume);
        let mut out = vec![0.0; (TUNE_SECONDS * 1.5 * 48_000.0) as usize * 2];
        s.render(&mut out);
        out
    }

    #[test]
    fn the_burst_is_a_sweep_then_silence() {
        let out = burst(1.0);
        let frame = |s: f32| (s * 48_000.0) as usize * 2;
        let last = out.iter().rposition(|v| *v != 0.0).unwrap();
        // Sound from the start, done by the end of the tune phase, silent after.
        assert!((0.4..=0.8).contains(&TUNE_SECONDS));
        assert!(out[..frame(0.02)].iter().any(|v| *v != 0.0));
        assert!(last < frame(TUNE_SECONDS) && last > frame(SQUELCH_S));
        // The sweep moves up: more of the early static is in the lows than the late.
        let zero_crossings = |a: &[f32]| {
            a.chunks_exact(2)
                .collect::<Vec<_>>()
                .windows(2)
                .filter(|w| (w[0][0] >= 0.0) != (w[1][0] >= 0.0))
                .count()
        };
        let early = zero_crossings(&out[frame(0.02)..frame(0.1)]);
        let late = zero_crossings(&out[frame(0.3)..frame(0.38)]);
        assert!(2 * late > 3 * early, "{early} {late}");
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1.0));
        // Volume scales it; zero silences it.
        assert!(burst(0.0).iter().all(|v| *v == 0.0));
        let half = burst(0.5);
        assert!(out
            .iter()
            .zip(&half)
            .all(|(a, b)| (a * 0.5 - b).abs() < 1e-6));
    }

    #[test]
    fn a_new_tune_restarts_and_the_same_count_does_not() {
        let mut s = TuneSynth::new();
        assert!(!s.active());
        s.set_params(0, 1.0);
        assert!(!s.active(), "no tune yet");
        s.set_params(3, 1.0);
        let mut buf = vec![0.0; 2 * 48_000];
        s.render(&mut buf);
        assert!(!s.active());
        s.set_params(3, 1.0);
        assert!(!s.active());
        s.set_params(4, 1.0);
        assert!(s.active());
    }
}
