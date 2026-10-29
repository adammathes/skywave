//! The green wave's chime ([`super::AudioParams::waves`]): each signal passed on green in
//! a run without stopping plays one soft bell note, on the world bus. The first is barely
//! there, high and short; each light after climbs a pentatonic step and grows a little
//! louder, longer and fuller (a fifth above from the third light, an octave below from the
//! fifth), so a long run builds into a warm chord. A new light starts its note again.

use super::dsp::{exp2, fract, sin_turns, Decay, DT};

/// The first light's note, Hz (E5), and the steps up the scale after it, semitones.
const BASE_HZ: f32 = 659.25;
const STEPS: [f32; 8] = [0.0, 2.0, 4.0, 7.0, 9.0, 12.0, 14.0, 16.0];
/// Level and decay time of the first light's note, and what each light after adds, up to
/// the last step.
const LEVEL: f32 = 0.02;
const LEVEL_STEP: f32 = 0.011;
const DECAY_S: f32 = 0.35;
const DECAY_STEP_S: f32 = 0.12;
/// The bell's bright overtone (a little above the third harmonic): its share and decay.
const SHIMMER: f32 = 0.25;
const SHIMMER_S: f32 = 0.08;
/// The fifth above joins from this light, the octave below from this one, at these shares.
const FIFTH_FROM: u8 = 3;
const SUB_FROM: u8 = 5;
const FIFTH: f32 = 0.45;
const SUB: f32 = 0.5;
/// Attack, s: soft, so the note blooms rather than clicks.
const ATTACK_S: f32 = 0.012;

#[derive(Clone, Debug)]
pub struct ChimeSynth {
    /// The last [`super::AudioParams::waves`] seen.
    count: u32,
    hz: f32,
    /// How full the note is: the light's place in the run, from 1.
    run: u8,
    t: f32,
    body: Decay,
    shimmer: Decay,
    phases: [f32; 4],
}

impl Default for ChimeSynth {
    fn default() -> Self {
        ChimeSynth::new()
    }
}

impl ChimeSynth {
    pub fn new() -> ChimeSynth {
        ChimeSynth {
            count: 0,
            hz: BASE_HZ,
            run: 1,
            t: 0.0,
            body: Decay::SILENT,
            shimmer: Decay::SILENT,
            phases: [0.0; 4],
        }
    }

    /// This frame's light count and the run the latest light made: a new count rings.
    pub fn set_params(&mut self, waves: u32, run: u8) {
        if waves != self.count {
            self.count = waves;
            self.run = run.max(1);
            let k = usize::from(self.run - 1).min(STEPS.len() - 1);
            self.hz = BASE_HZ * exp2(STEPS[k] / 12.0);
            self.t = 0.0;
            self.body = Decay::hit(
                LEVEL + LEVEL_STEP * k as f32,
                DECAY_S + DECAY_STEP_S * k as f32,
            );
            self.shimmer = Decay::hit(SHIMMER, SHIMMER_S);
            self.phases = [0.0; 4];
        }
    }

    /// Sounding now (the mixer skips it otherwise).
    pub fn active(&self) -> bool {
        self.body.active()
    }

    /// Adds the note to the interleaved stereo `out`.
    pub fn render(&mut self, out: &mut [f32]) {
        for f in out.chunks_exact_mut(2) {
            if !self.active() {
                return;
            }
            let v = self.sample();
            f[0] += v;
            f[1] += v;
        }
    }

    fn sample(&mut self) -> f32 {
        let attack = (self.t / ATTACK_S).min(1.0);
        self.t += DT;
        let env = self.body.step() * attack;
        let shimmer = if self.shimmer.active() {
            self.shimmer.step()
        } else {
            0.0
        };
        let ratios = [1.0, 3.01, 1.5, 0.5];
        let mut gains = [1.0, shimmer, 0.0, 0.0];
        if self.run >= FIFTH_FROM {
            gains[2] = FIFTH;
        }
        if self.run >= SUB_FROM {
            gains[3] = SUB;
        }
        let mut out = 0.0;
        for k in 0..4 {
            self.phases[k] = fract(self.phases[k] + self.hz * ratios[k] * DT);
            if gains[k] > 0.0 {
                out += gains[k] * sin_turns(self.phases[k]);
            }
        }
        env * out
    }
}
