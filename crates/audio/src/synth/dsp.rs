//! Deterministic DSP building blocks: a polynomial sine, `exp2`, white noise, a one-pole
//! smoother and a state-variable filter.
//!
//! Only `+ - * /`, `floor`, `sqrt` and comparisons, all correctly rounded in IEEE 754, so a
//! render is bit-identical on every platform and in WebAssembly: nothing calls the
//! platform's `sin`/`exp`/`pow`, and nothing uses `mul_add`.

/// Frames per second.
pub const SAMPLE_RATE: f32 = 48_000.0;
/// Seconds per frame.
pub const DT: f32 = 1.0 / SAMPLE_RATE;

/// `sin(2π t)`: `t` in turns, any finite value. Absolute error < 4e-6.
#[inline]
pub fn sin_turns(t: f32) -> f32 {
    let mut x = t - (t + 0.5).floor(); // [-0.5, 0.5)
    if x > 0.25 {
        x = 0.5 - x;
    } else if x < -0.25 {
        x = -0.5 - x;
    }
    let y = x * core::f32::consts::TAU; // [-π/2, π/2]
    let y2 = y * y;
    y * (1.0
        + y2 * (-1.0 / 6.0 + y2 * (1.0 / 120.0 + y2 * (-1.0 / 5040.0 + y2 * (1.0 / 362_880.0)))))
}

/// The fractional part of a non-negative phase, `[0, 1)`.
#[inline]
pub fn fract(t: f32) -> f32 {
    t - t.floor()
}

/// `2^x`, relative error < 2e-6 for `x` in `[-126, 127]`; 0 below, a large finite value above.
#[inline]
pub fn exp2(x: f32) -> f32 {
    if x < -126.0 {
        return 0.0;
    }
    let x = if x > 127.0 { 127.0 } else { x };
    let n = x.floor();
    let f = x - n; // [0, 1)
                   // 2^f on [0, 1): the Taylor series of e^(f ln 2) to the 7th power.
    let l = core::f32::consts::LN_2 * f;
    let p = 1.0
        + l * (1.0
            + l * (0.5
                + l * (1.0 / 6.0
                    + l * (1.0 / 24.0 + l * (1.0 / 120.0 + l * (1.0 / 720.0 + l / 5040.0))))));
    let scale = f32::from_bits(((n as i32 + 127) as u32) << 23);
    p * scale
}

/// `e^x`, see [`exp2`].
#[inline]
pub fn exp(x: f32) -> f32 {
    exp2(x * core::f32::consts::LOG2_E)
}

/// `x` clamped into `[lo, hi]`; non-finite `x` becomes `lo`.
#[inline]
pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x >= lo {
        if x <= hi {
            x
        } else {
            hi
        }
    } else {
        lo
    }
}

/// `x` clamped into `[0, 1]` (NaN → 0).
#[inline]
pub fn unit(x: f32) -> f32 {
    clamp(x, 0.0, 1.0)
}

/// Smooth 0 → 1 between `lo` and `hi`.
#[inline]
pub fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = unit((x - lo) / (hi - lo));
    t * t * (3.0 - 2.0 * t)
}

/// Per-sample coefficient of a one-pole smoother with time constant `tau_s`.
pub fn tau_coef(tau_s: f32) -> f32 {
    if tau_s <= 0.0 {
        1.0
    } else {
        1.0 - exp(-DT / tau_s)
    }
}

/// White noise: xorshift32, uniform in `[-1, 1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Noise(u32);

impl Noise {
    pub fn new(seed: u32) -> Noise {
        Noise(if seed == 0 { 0x9e37_79b9 } else { seed })
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in `[-1, 1)`.
    #[inline]
    pub fn white(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
    }

    /// Uniform in `[0, 1)`.
    #[inline]
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform in `[lo, hi)`.
    #[inline]
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// A value that follows its target exponentially, one step per sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slew {
    pub value: f32,
    pub coef: f32,
}

impl Slew {
    pub fn new(value: f32, tau_s: f32) -> Slew {
        Slew {
            value,
            coef: tau_coef(tau_s),
        }
    }

    #[inline]
    pub fn step(&mut self, target: f32) -> f32 {
        self.value += (target - self.value) * self.coef;
        self.value
    }
}

/// The three outputs of one [`Svf`] step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvfOut {
    pub low: f32,
    pub band: f32,
    pub high: f32,
}

/// A trapezoidal state-variable filter (stable while its cutoff moves).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

impl Svf {
    pub fn new(cutoff_hz: f32, q: f32) -> Svf {
        let mut f = Svf {
            ic1: 0.0,
            ic2: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            k: 1.0,
        };
        f.set(cutoff_hz, q);
        f
    }

    /// New cutoff (clamped to 10 Hz .. 0.45 × the sample rate) and resonance.
    pub fn set(&mut self, cutoff_hz: f32, q: f32) {
        let fc = clamp(cutoff_hz, 10.0, 0.45 * SAMPLE_RATE);
        // g = tan(π fc / fs), as sin/cos of fc / (2 fs) turns.
        let t = fc / (2.0 * SAMPLE_RATE);
        let g = sin_turns(t) / sin_turns(t + 0.25);
        self.k = 1.0 / clamp(q, 0.1, 50.0);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline]
    pub fn step(&mut self, x: f32) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut {
            low: v2,
            band: v1,
            high: x - self.k * v1 - v2,
        }
    }

    #[inline]
    pub fn low(&mut self, x: f32) -> f32 {
        self.step(x).low
    }

    /// Band-pass normalized to unity gain at the center.
    #[inline]
    pub fn band(&mut self, x: f32) -> f32 {
        let k = self.k;
        self.step(x).band * k
    }

    /// Clears the state (after a non-finite input).
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
}

/// A decaying envelope: `value` multiplies by `coef` every sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decay {
    pub value: f32,
    pub coef: f32,
}

impl Decay {
    pub const SILENT: Decay = Decay {
        value: 0.0,
        coef: 0.0,
    };

    /// Starts at `level`, falling to 1/e in `tau_s`.
    pub fn hit(level: f32, tau_s: f32) -> Decay {
        Decay {
            value: level,
            coef: 1.0 - tau_coef(tau_s),
        }
    }

    #[inline]
    pub fn step(&mut self) -> f32 {
        let v = self.value;
        self.value *= self.coef;
        if self.value < 1e-5 {
            self.value = 0.0;
        }
        v
    }

    pub fn active(&self) -> bool {
        self.value > 0.0
    }
}

/// A DC blocker: a one-pole high-pass at ~10 Hz.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
}

impl DcBlock {
    /// `1 - 2π × 10 Hz / 48 kHz`.
    const R: f32 = 0.998_691;

    #[inline]
    pub fn step(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + Self::R * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Equal-power-ish stereo gains for `pan` in `[-1, 1]` (left to right): `[left, right]`,
/// both 1/√2 in the middle.
pub fn pan_gains(pan: f32) -> [f32; 2] {
    let p = 0.5 * (clamp(pan, -1.0, 1.0) + 1.0);
    // sin/cos of a quarter turn scaled by p.
    [sin_turns(0.25 * (1.0 - p)), sin_turns(0.25 * p)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_matches_std() {
        let mut worst: f32 = 0.0;
        for i in -4000..4000 {
            let t = i as f32 / 997.0;
            let e = (sin_turns(t) - (t * core::f32::consts::TAU).sin()).abs();
            worst = worst.max(e);
        }
        assert!(worst < 4e-6, "{worst}");
    }

    #[test]
    fn exp_matches_std() {
        for i in -200..60 {
            let x = i as f32 * 0.37;
            let rel = (exp(x) - x.exp()).abs() / x.exp();
            assert!(rel < 1e-5, "{x}: {rel}");
        }
        assert_eq!(exp2(-200.0), 0.0);
        assert!(exp2(500.0).is_finite());
    }

    #[test]
    fn clamp_handles_nan() {
        assert_eq!(unit(f32::NAN), 0.0);
        assert_eq!(clamp(5.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp(-5.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn noise_is_uniform_and_centered() {
        let mut n = Noise::new(1);
        let (mut sum, mut lo, mut hi) = (0.0f64, 1.0f32, -1.0f32);
        for _ in 0..100_000 {
            let v = n.white();
            sum += f64::from(v);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!((sum / 100_000.0).abs() < 0.01);
        assert!(lo >= -1.0 && hi < 1.0 && lo < -0.99 && hi > 0.99);
    }

    #[test]
    fn svf_low_pass_passes_dc_and_band_peaks_at_center() {
        let mut f = Svf::new(1000.0, 0.707);
        let mut y = 0.0;
        for _ in 0..4800 {
            y = f.low(1.0);
        }
        assert!((y - 1.0).abs() < 1e-3);
        // A 1 kHz tone through a 1 kHz band-pass keeps ~unity gain; 100 Hz is cut.
        let gain = |hz: f32| {
            let mut f = Svf::new(1000.0, 2.0);
            let mut peak: f32 = 0.0;
            for i in 0..9600 {
                let v = f.band(sin_turns(hz * i as f32 * DT));
                if i > 4800 {
                    peak = peak.max(v.abs());
                }
            }
            peak
        };
        assert!((gain(1000.0) - 1.0).abs() < 0.02);
        assert!(gain(100.0) < 0.1);
    }

    #[test]
    fn dc_block_removes_offset_and_keeps_bass() {
        let mut d = DcBlock::default();
        let mut y = 0.0;
        for _ in 0..48_000 {
            y = d.step(0.5);
        }
        assert!(y.abs() < 1e-3);
        let mut d = DcBlock::default();
        let mut peak: f32 = 0.0;
        for i in 0..96_000 {
            let v = d.step(sin_turns(40.0 * i as f32 * DT));
            if i > 48_000 {
                peak = peak.max(v.abs());
            }
        }
        assert!(peak > 0.95, "{peak}");
    }

    #[test]
    fn pan_is_balanced_in_the_middle() {
        let [l, r] = pan_gains(0.0);
        assert!((l - r).abs() < 1e-6 && (l * l + r * r - 1.0).abs() < 1e-4);
        let [l, r] = pan_gains(-1.0);
        assert!(l > 0.999 && r.abs() < 1e-5);
    }
}
