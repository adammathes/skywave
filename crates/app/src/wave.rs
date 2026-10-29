//! The green wave in the game (`docs/NPCS.md`, "Green waves"): the player's run of
//! signals passed on green ([`traffic::Streak`], stepped with the sim) and the feedback
//! that grows with it. Never a number, a popup or text: each light rings the chime (its
//! count and the run go to the mixer, `audio::synth::chime`), and the speedometer's needle
//! glints and then glows a little warmer the longer the run. A red, a stop, a hard brake
//! or speeding ends the run, and the glow fades out over a couple of seconds.

use traffic::{LaneGraph, Player, Streak};

/// Extra needle glow per light of the run, up to [`GLOW_RUNS`] lights.
const GLOW_STEP: f32 = 0.12;
const GLOW_RUNS: u8 = 8;
/// How quickly the glow follows the run, s (up and, when it ends, down).
const GLOW_TAU_S: f32 = 1.2;
/// The glint each light adds on top, the first light's and how much more each after, and
/// how quickly it fades, s.
const GLINT: f32 = 0.4;
const GLINT_STEP: f32 = 0.08;
const GLINT_TAU_S: f32 = 0.45;

pub struct Wave {
    streak: Streak,
    /// The run as the latest light made it (what its chime plays).
    rung: u8,
    glow: f32,
    glint: f32,
}

impl Wave {
    /// For the signals of the place's lane graph.
    pub fn new(graph: &LaneGraph) -> Wave {
        Wave {
            streak: Streak::new(graph),
            rung: 0,
            glow: 0.0,
            glint: 0.0,
        }
    }

    /// One sim step of `dt` s at signal tick `tick` with the player's car (speed signed).
    pub fn step(&mut self, graph: &LaneGraph, tick: u64, car: &Player, dt: f32) {
        let before = self.streak.greens;
        self.streak.step(graph, tick, car, dt);
        if self.streak.greens != before {
            self.rung = self.streak.run;
            let runs = f32::from(self.rung.min(GLOW_RUNS) - 1);
            self.glint = GLINT + GLINT_STEP * runs;
        }
    }

    /// Eases the glow `dt` s on toward the run.
    pub fn frame(&mut self, dt: f32) {
        let target = GLOW_STEP * f32::from(self.streak.run.min(GLOW_RUNS));
        self.glow += (target - self.glow) * (1.0 - (-dt / GLOW_TAU_S).exp());
        self.glint *= (-dt / GLINT_TAU_S).exp();
    }

    /// The speedometer needle's glow multiplier (1 with no run).
    pub fn speedo_glow(&self) -> f32 {
        1.0 + self.glow + self.glint
    }

    /// For the chime: every light that added to a run so far, and the latest one's run.
    pub fn chime(&self) -> (u32, u8) {
        (self.streak.greens, self.rung)
    }
}
