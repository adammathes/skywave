//! Fixed-timestep accumulator: the simulation always advances in whole `SIM_DT` steps and
//! rendering interpolates between the last two states with `alpha`.

/// Simulation rate.
pub const SIM_HZ: u32 = 120;
/// Simulation step length in seconds.
pub const SIM_DT: f64 = 1.0 / SIM_HZ as f64;
/// Upper bound on steps per frame. Frames slower than this (debugger, tab in background)
/// make the game run slower instead of spiralling into ever longer catch-up frames.
pub const MAX_STEPS_PER_FRAME: u32 = 8;

/// What one frame of real time asks of the simulation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Steps {
    /// Number of `SIM_DT` steps to run now.
    pub count: u32,
    /// How far real time is past the last step, in `[0, 1)` steps. Render
    /// `lerp(previous, current, alpha)`.
    pub alpha: f32,
}

/// Accumulates real frame time into whole simulation steps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FixedStep {
    accumulator: f64,
}

impl FixedStep {
    pub const fn new() -> FixedStep {
        FixedStep { accumulator: 0.0 }
    }

    /// Adds `dt_real` seconds of wall-clock time. Negative or non-finite input counts as zero;
    /// input longer than `MAX_STEPS_PER_FRAME` steps is clamped.
    pub fn advance(&mut self, dt_real: f64) -> Steps {
        let max_frame = MAX_STEPS_PER_FRAME as f64 * SIM_DT;
        let dt = if dt_real.is_finite() && dt_real > 0.0 {
            dt_real.min(max_frame)
        } else {
            0.0
        };
        self.accumulator += dt;
        let mut count = 0;
        while self.accumulator >= SIM_DT && count < MAX_STEPS_PER_FRAME {
            self.accumulator -= SIM_DT;
            count += 1;
        }
        // Only reachable through float rounding at the clamp: never carry a full step.
        if self.accumulator >= SIM_DT {
            self.accumulator = 0.0;
        }
        Steps {
            count,
            alpha: (self.accumulator / SIM_DT) as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_steps_and_alpha() {
        let mut fs = FixedStep::new();
        let s = fs.advance(SIM_DT * 2.5);
        assert_eq!(s.count, 2);
        assert!((s.alpha - 0.5).abs() < 1e-6);
        let s = fs.advance(SIM_DT * 0.25);
        assert_eq!(s.count, 0);
        assert!((s.alpha - 0.75).abs() < 1e-6);
        let s = fs.advance(SIM_DT * 0.5);
        assert_eq!(s.count, 1);
        assert!((s.alpha - 0.25).abs() < 1e-6);
    }

    #[test]
    fn sixty_hz_frames_run_two_steps_each() {
        let mut fs = FixedStep::new();
        let total: u32 = (0..600).map(|_| fs.advance(1.0 / 60.0).count).sum();
        assert!((1199..=1200).contains(&total), "{total}");
    }

    #[test]
    fn clamps_long_and_bad_frames() {
        let mut fs = FixedStep::new();
        assert_eq!(fs.advance(10.0).count, MAX_STEPS_PER_FRAME);
        let s = fs.advance(0.0);
        assert_eq!(s.count, 0);
        assert!(s.alpha < 1.0);
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert_eq!(fs.advance(bad).count, 0);
        }
    }

    #[test]
    fn alpha_is_always_below_one() {
        let mut fs = FixedStep::new();
        let mut t = 0.001;
        for _ in 0..10_000 {
            let s = fs.advance(t);
            assert!((0.0..1.0).contains(&s.alpha), "{}", s.alpha);
            t = (t * 1.37) % 0.09;
        }
    }
}
