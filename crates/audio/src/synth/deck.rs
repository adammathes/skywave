//! The deck's mechanism on a transport press ([`super::AudioParams::deck_clicks`]): a tape
//! deck's key clunking down and its capstan motor spinning up, a CD player's laser sled
//! seeking in quick ticks over the disc's whir, or an MP3 player's soft button click. Short
//! (at most [`LONGEST_S`]), on the ui bus, at the radio volume. A new press starts it again.

use super::dsp::{exp2, fract, sin_turns, smoothstep, Decay, Noise, Svf, DT};

/// The longest of the three sounds, s.
pub const LONGEST_S: f32 = 0.3;

/// Which mechanism, as [`super::AudioParams::deck_kind`] gives it.
const TAPE: u8 = 0;
const CD: u8 = 1;
/// How long each sounds, s.
const LENGTH_S: [f32; 3] = [0.22, LONGEST_S, 0.04];
/// The CD sled's ticks: this many, this far apart, s.
const SEEK_TICKS: u32 = 5;
const SEEK_GAP_S: f32 = 0.035;
/// The tape key's latch clicks home this long after it goes down, s.
const LATCH_S: f32 = 0.06;

#[derive(Clone, Debug)]
pub struct DeckSynth {
    /// The last [`super::AudioParams::deck_clicks`] seen.
    count: u32,
    kind: u8,
    /// Seconds into the sound; at or past its length it is silent.
    t: f32,
    volume: f32,
    noise: Noise,
    click: Decay,
    click_bp: Svf,
    thump: Decay,
    thump_phase: f32,
    motor: Svf,
    whine_phase: f32,
}

impl Default for DeckSynth {
    fn default() -> Self {
        DeckSynth::new()
    }
}

impl DeckSynth {
    pub fn new() -> DeckSynth {
        DeckSynth {
            count: 0,
            kind: TAPE,
            t: LONGEST_S,
            volume: 0.0,
            noise: Noise::new(0x6465_636b),
            click: Decay::SILENT,
            click_bp: Svf::new(2500.0, 1.2),
            thump: Decay::SILENT,
            thump_phase: 0.0,
            motor: Svf::new(320.0, 2.0),
            whine_phase: 0.0,
        }
    }

    /// This frame's press count, mechanism (0 tape, 1 CD, 2 MP3) and radio volume: a new
    /// count starts the sound.
    pub fn set_params(&mut self, clicks: u32, kind: u8, volume: f32) {
        if clicks != self.count {
            self.count = clicks;
            self.kind = kind.min(2);
            self.t = 0.0;
            self.click = Decay::SILENT;
            self.thump = Decay::SILENT;
            self.click_bp
                .set(if self.kind == CD { 3200.0 } else { 2500.0 }, 1.2);
            self.click_bp.reset();
            self.motor.reset();
            self.thump_phase = 0.0;
            self.whine_phase = 0.0;
        }
        self.volume = volume;
    }

    fn length(&self) -> f32 {
        LENGTH_S[usize::from(self.kind)]
    }

    /// Sounding now (the mixer skips it otherwise).
    pub fn active(&self) -> bool {
        self.t < self.length()
    }

    /// Adds the sound to the interleaved stereo `out`.
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

    /// Whether `at` s passed during this sample.
    fn passes(&self, at: f32) -> bool {
        self.t <= at && self.t + DT > at
    }

    fn sample(&mut self) -> f32 {
        let t = self.t;
        let white = self.noise.white();
        let mut out = 0.0;
        match self.kind {
            TAPE => {
                if self.passes(0.0) {
                    self.thump = Decay::hit(0.3, 0.012);
                    self.click = Decay::hit(0.35, 0.0015);
                }
                if self.passes(LATCH_S) {
                    self.click = Decay::hit(0.25, 0.001);
                }
                if self.thump.active() {
                    self.thump_phase = fract(self.thump_phase + 110.0 * DT);
                    out += self.thump.step() * sin_turns(self.thump_phase);
                }
                // The capstan motor coming up to speed: a band of noise rising in pitch.
                let env = smoothstep(0.02, 0.07, t) * (1.0 - smoothstep(0.15, 0.22, t));
                if env > 0.0 {
                    self.motor
                        .set(220.0 * exp2(0.6 * smoothstep(0.02, 0.15, t)), 2.0);
                    out += 0.25 * env * self.motor.band(white);
                }
            }
            CD => {
                for k in 0..SEEK_TICKS {
                    if self.passes(k as f32 * SEEK_GAP_S) {
                        self.click = Decay::hit(0.22 - 0.02 * k as f32, 0.0008);
                    }
                }
                // The disc spinning up under the sled: a faint whine gliding up.
                let env = smoothstep(0.0, 0.05, t) * (1.0 - smoothstep(0.2, LONGEST_S, t));
                let hz = 900.0 * exp2(0.6 * t / LONGEST_S);
                self.whine_phase = fract(self.whine_phase + hz * DT);
                out += 0.02 * env * sin_turns(self.whine_phase);
            }
            _ => {
                if self.passes(0.0) {
                    self.click = Decay::hit(0.18, 0.001);
                }
            }
        }
        if self.click.active() {
            out += self.click.step() * self.click_bp.band(white) * 4.0;
        }
        self.t += DT;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(kind: u8, volume: f32) -> Vec<f32> {
        let mut s = DeckSynth::new();
        s.set_params(1, kind, volume);
        let mut out = vec![0.0; (LONGEST_S * 1.5 * 48_000.0) as usize * 2];
        s.render(&mut out);
        out
    }

    #[test]
    fn each_mechanism_is_short_and_bounded() {
        for kind in 0..3u8 {
            let out = sound(kind, 1.0);
            let last = out.iter().rposition(|v| *v != 0.0).unwrap();
            let len = LENGTH_S[usize::from(kind)];
            assert!(last <= (len * 48_000.0) as usize * 2 + 4, "{kind}");
            assert!(
                out[..200].iter().any(|v| *v != 0.0),
                "{kind} starts at once"
            );
            assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1.0), "{kind}");
            assert!(sound(kind, 0.0).iter().all(|v| *v == 0.0));
        }
        // A CD seek ticks: separate bursts, not one.
        let cd = sound(CD, 1.0);
        let at = |s: f32| (s * 48_000.0) as usize * 2;
        let peak = |a: &[f32]| a.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak(&cd[at(0.035)..at(0.038)]) > 4.0 * peak(&cd[at(0.02)..at(0.03)]));
    }

    #[test]
    fn a_new_press_restarts_and_the_same_count_does_not() {
        let mut s = DeckSynth::new();
        s.set_params(0, TAPE, 1.0);
        assert!(!s.active());
        s.set_params(2, TAPE, 1.0);
        assert!(s.active());
        let mut buf = vec![0.0; 2 * 48_000];
        s.render(&mut buf);
        assert!(!s.active());
        s.set_params(2, CD, 1.0);
        assert!(!s.active());
        s.set_params(3, 9, 1.0);
        assert!(s.active());
    }
}
