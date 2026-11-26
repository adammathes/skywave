//! The engine: exhaust pulses through fixed resonances, shaped per car by an
//! [`EngineProfile`] (the spec's `feel.engine_sound`).
//!
//! An engine is heard as a train of exhaust pulses (one per cylinder firing) ringing the
//! pipes, the body and the cabin. The pulses follow the rpm; the resonances do not, and that
//! is what makes it sound like a machine rather than an organ:
//! - one phase runs at the engine-cycle rate, `rpm / 120` (a four-stroke fires every
//!   cylinder once per two revolutions), and each cylinder fires a smooth `sin²` pulse at
//!   its slot in the cycle, with its own level and a small offset (a cross-plane V8's two
//!   banks are unequal, which is its burble) and a random variation per firing
//!   (cycle-to-cycle combustion, most at idle). Load narrows the pulses: harder, brighter;
//! - the pulse train rings three band-passes at fixed frequencies (the exhaust's and the
//!   body's resonances) plus a direct share, all through a cabin low-pass that opens with
//!   load and rpm;
//! - combustion rasp is noise gated by the pulses (so it is rhythmic, never a hiss), intake
//!   roar is the pulses and noise through the intake's resonance, louder with throttle;
//!   a diesel clatters on every firing;
//! - forced induction: a turbo spools up behind the throttle (a quiet whistle and hiss with
//!   boost) and blows off when the throttle closes on boost; a supercharger whines with rpm.
//!
//! Events: lifting off the throttle at high rpm schedules a few exhaust pops (and a closed
//! throttle on the overrun crackles now and then); at the redline under throttle the
//! limiter cuts whole firings at `limiter_hz`; a change between forward gears unloads the
//! engine for the gearbox's torque cut. Every parameter is slewed per sample.

use super::dsp::{fract, sin_turns, smoothstep, unit, DcBlock, Decay, Noise, Slew, Svf, DT};
use super::EngineParams;

/// Most cylinders a profile may have.
pub const MAX_CYLINDERS: usize = 12;
/// Filter coefficients and events are updated every this many samples.
const TICK: usize = 32;
/// Overall engine level, before the bus gain.
const LEVEL: f32 = 0.85;
/// Longest exhaust pulse, s: at low rpm a pulse is this short blow-down, not a wide hump,
/// so idle chugs instead of humming.
const PULSE_MAX_S: f32 = 0.005;
/// How far a closed throttle at idle turns the engine down (0.45: to 55%, -5 dB).
const IDLE_CUT: f32 = 0.45;

/// The engine sounds a car spec may name in `feel.engine_sound`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineProfile {
    /// A flat-plane V8: even firing bank to bank, bright and rasping, revs freely, crackles.
    V8Flatplane = 0,
    /// A cross-plane V8 in no hurry: the banks' uneven pulses burble, deep and dark.
    V8Lazy = 1,
    /// An inline four: even, eager at the top, some intake roar.
    I4 = 2,
    /// A V6: mid-weighted, a little uneven bank to bank.
    V6 = 3,
    /// Anything else: an inline six.
    #[default]
    Default = 4,
    /// A small, quiet, well-muffled four (economy cars, minivans).
    I4Soft = 5,
    /// A turbo four: muffled by the turbine, a whistle with boost and a blow-off on a lift.
    I4Turbo = 6,
    /// A turbo V6.
    V6Turbo = 7,
    /// An inline six: smooth and silky, a clean howl up top.
    I6 = 8,
    /// A turbo inline six.
    I6Turbo = 9,
    /// A cross-plane V8 with a supercharger's whine on top.
    V8Blown = 10,
    /// A V12: smooth, many small pulses, a high howl.
    V12 = 11,
    /// A twin-rotor rotary with turbos: sharp port pulses, a raspy "brap" at idle.
    RotaryTurbo = 12,
    /// A big turbo diesel six: low, clattering, a turbo whistle.
    Diesel = 13,
}

impl EngineProfile {
    pub const ALL: [EngineProfile; 14] = [
        EngineProfile::V8Flatplane,
        EngineProfile::V8Lazy,
        EngineProfile::I4,
        EngineProfile::V6,
        EngineProfile::Default,
        EngineProfile::I4Soft,
        EngineProfile::I4Turbo,
        EngineProfile::V6Turbo,
        EngineProfile::I6,
        EngineProfile::I6Turbo,
        EngineProfile::V8Blown,
        EngineProfile::V12,
        EngineProfile::RotaryTurbo,
        EngineProfile::Diesel,
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
            EngineProfile::I4Soft => "i4_soft",
            EngineProfile::I4Turbo => "i4_turbo",
            EngineProfile::V6Turbo => "v6_turbo",
            EngineProfile::I6 => "i6",
            EngineProfile::I6Turbo => "i6_turbo",
            EngineProfile::V8Blown => "v8_blown",
            EngineProfile::V12 => "v12",
            EngineProfile::RotaryTurbo => "rotary_turbo",
            EngineProfile::Diesel => "diesel",
        }
    }

    pub fn voice(self) -> &'static Voice {
        match self {
            EngineProfile::V8Flatplane => &V8_FLATPLANE,
            EngineProfile::V8Lazy => &V8_LAZY,
            EngineProfile::I4 => &I4,
            EngineProfile::V6 => &V6,
            EngineProfile::Default | EngineProfile::I6 => &I6,
            EngineProfile::I4Soft => &I4_SOFT,
            EngineProfile::I4Turbo => &I4_TURBO,
            EngineProfile::V6Turbo => &V6_TURBO,
            EngineProfile::I6Turbo => &I6_TURBO,
            EngineProfile::V8Blown => &V8_BLOWN,
            EngineProfile::V12 => &V12,
            EngineProfile::RotaryTurbo => &ROTARY_TURBO,
            EngineProfile::Diesel => &DIESEL,
        }
    }
}

/// What a profile sounds like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voice {
    /// Firings per engine cycle (cylinders; a twin-rotor rotary fires four).
    pub cylinders: u8,
    /// Each firing's level, in firing order (the first `cylinders` are used).
    pub gain: [f32; MAX_CYLINDERS],
    /// Each firing's offset from its even slot, in firing intervals.
    pub offset: [f32; MAX_CYLINDERS],
    /// Pulse width at no load, in firing intervals (`(0, 1]`), and how much of it full load
    /// takes away: narrower pulses are harder and brighter.
    pub width: f32,
    pub snap: f32,
    /// Random variation of each firing at idle, fading to a third of it at the redline.
    pub jitter: f32,
    /// The exhaust and body resonances: `[Hz, Q, gain]` each.
    pub formants: [[f32; 3]; 3],
    /// The pulse train's share heard straight, unresonated.
    pub direct: f32,
    /// Cabin low-pass cutoff closed (idle, no load) and fully open, Hz.
    pub lp_hz: [f32; 2],
    /// Combustion rasp (noise gated by the pulses): level and band center, Hz.
    pub rasp: f32,
    pub rasp_hz: f32,
    /// Intake roar: level and resonance, Hz.
    pub intake: f32,
    pub intake_hz: f32,
    /// Clatter on every firing (a diesel's knock), level.
    pub clatter: f32,
    /// Turbo (whistle, hiss, blow-off) and supercharger whine levels; 0 for neither.
    pub turbo: f32,
    pub blower: f32,
    /// Exhaust pop level (0: never pops).
    pub pops: f32,
    /// Rev-limiter cut rate, Hz.
    pub limiter_hz: f32,
    /// Overall level, relative.
    pub level: f32,
}

const EVEN: [f32; MAX_CYLINDERS] = [0.0; MAX_CYLINDERS];
const ONES: [f32; MAX_CYLINDERS] = [1.0; MAX_CYLINDERS];

/// Firing order 1-8-4-3-6-5-7-2 with the odd cylinders on the left bank: L R R L R L L R.
/// The near bank is louder and the far one's pipe a little longer.
const CROSS_GAIN: [f32; MAX_CYLINDERS] = [
    1.0, 0.72, 0.72, 1.0, 0.72, 1.0, 1.0, 0.72, 0.0, 0.0, 0.0, 0.0,
];
const CROSS_OFFSET: [f32; MAX_CYLINDERS] = [
    0.0, 0.07, 0.07, 0.0, 0.07, 0.0, 0.0, 0.07, 0.0, 0.0, 0.0, 0.0,
];

const V8_LAZY: Voice = Voice {
    cylinders: 8,
    gain: CROSS_GAIN,
    offset: CROSS_OFFSET,
    width: 0.8,
    snap: 0.35,
    jitter: 0.08,
    formants: [[90.0, 1.4, 1.0], [240.0, 2.2, 0.55], [650.0, 3.0, 0.22]],
    direct: 0.35,
    lp_hz: [420.0, 1900.0],
    rasp: 0.22,
    rasp_hz: 900.0,
    intake: 0.3,
    intake_hz: 420.0,
    clatter: 0.0,
    turbo: 0.0,
    blower: 0.0,
    pops: 0.6,
    limiter_hz: 12.0,
    level: 1.0,
};

const V8_BLOWN: Voice = Voice {
    rasp: 0.3,
    blower: 1.0,
    pops: 0.8,
    ..V8_LAZY
};

const V8_FLATPLANE: Voice = Voice {
    cylinders: 8,
    gain: [
        1.0, 0.86, 1.0, 0.86, 1.0, 0.86, 1.0, 0.86, 0.0, 0.0, 0.0, 0.0,
    ],
    offset: EVEN,
    width: 0.6,
    snap: 0.4,
    jitter: 0.04,
    formants: [[150.0, 1.4, 0.8], [460.0, 2.4, 0.7], [1300.0, 3.0, 0.35]],
    direct: 0.3,
    lp_hz: [800.0, 3800.0],
    rasp: 0.45,
    rasp_hz: 1700.0,
    intake: 0.5,
    intake_hz: 700.0,
    clatter: 0.0,
    turbo: 0.0,
    blower: 0.0,
    pops: 1.0,
    limiter_hz: 16.0,
    level: 1.0,
};

const V12: Voice = Voice {
    cylinders: 12,
    gain: [
        1.0, 0.94, 1.0, 0.94, 1.0, 0.94, 1.0, 0.94, 1.0, 0.94, 1.0, 0.94,
    ],
    offset: EVEN,
    width: 0.75,
    snap: 0.3,
    jitter: 0.025,
    formants: [[210.0, 1.4, 0.7], [620.0, 2.2, 0.75], [1650.0, 3.0, 0.45]],
    direct: 0.3,
    lp_hz: [1000.0, 4500.0],
    rasp: 0.35,
    rasp_hz: 2300.0,
    intake: 0.6,
    intake_hz: 900.0,
    clatter: 0.0,
    turbo: 0.0,
    blower: 0.0,
    pops: 0.8,
    limiter_hz: 18.0,
    level: 1.0,
};

const I6: Voice = Voice {
    cylinders: 6,
    gain: [
        1.0, 0.97, 1.0, 0.98, 1.0, 0.97, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ],
    offset: EVEN,
    width: 0.7,
    snap: 0.35,
    jitter: 0.03,
    formants: [[130.0, 1.4, 0.85], [400.0, 2.3, 0.6], [1100.0, 3.0, 0.32]],
    direct: 0.3,
    lp_hz: [650.0, 3000.0],
    rasp: 0.28,
    rasp_hz: 1400.0,
    intake: 0.5,
    intake_hz: 600.0,
    clatter: 0.0,
    turbo: 0.0,
    blower: 0.0,
    pops: 0.4,
    limiter_hz: 15.0,
    level: 1.0,
};

const I6_TURBO: Voice = Voice {
    turbo: 1.0,
    rasp: 0.22,
    pops: 0.5,
    ..I6
};

const V6: Voice = Voice {
    cylinders: 6,
    gain: [
        1.0, 0.85, 1.0, 0.85, 1.0, 0.85, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ],
    offset: [
        0.0, 0.04, 0.0, 0.04, 0.0, 0.04, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ],
    width: 0.65,
    snap: 0.35,
    jitter: 0.045,
    formants: [[120.0, 1.4, 0.85], [360.0, 2.3, 0.6], [950.0, 3.0, 0.3]],
    direct: 0.3,
    lp_hz: [600.0, 2800.0],
    rasp: 0.32,
    rasp_hz: 1200.0,
    intake: 0.5,
    intake_hz: 550.0,
    clatter: 0.0,
    turbo: 0.0,
    blower: 0.0,
    pops: 0.4,
    limiter_hz: 15.0,
    level: 1.0,
};

const V6_TURBO: Voice = Voice {
    turbo: 1.0,
    rasp: 0.25,
    pops: 0.5,
    ..V6
};

const I4: Voice = Voice {
    cylinders: 4,
    gain: [1.0, 0.95, 1.0, 0.95, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    offset: EVEN,
    width: 0.55,
    snap: 0.4,
    jitter: 0.04,
    formants: [[180.0, 1.4, 0.7], [540.0, 2.2, 0.7], [1500.0, 3.0, 0.35]],
    direct: 0.3,
    lp_hz: [750.0, 3400.0],
    rasp: 0.4,
    rasp_hz: 1700.0,
    intake: 0.7,
    intake_hz: 800.0,
    clatter: 0.0,
    turbo: 0.0,
    blower: 0.0,
    pops: 0.5,
    limiter_hz: 18.0,
    level: 1.0,
};

const I4_SOFT: Voice = Voice {
    width: 0.75,
    snap: 0.25,
    formants: [[160.0, 1.4, 0.8], [480.0, 2.0, 0.5], [1300.0, 2.5, 0.2]],
    lp_hz: [550.0, 2000.0],
    rasp: 0.18,
    intake: 0.4,
    pops: 0.0,
    ..I4
};

const I4_TURBO: Voice = Voice {
    turbo: 1.0,
    rasp: 0.3,
    lp_hz: [700.0, 3000.0],
    pops: 0.7,
    ..I4
};

const ROTARY_TURBO: Voice = Voice {
    cylinders: 4,
    gain: [1.0, 0.85, 1.0, 0.85, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    offset: EVEN,
    width: 0.35,
    snap: 0.3,
    jitter: 0.09,
    formants: [[230.0, 1.3, 0.6], [780.0, 2.0, 0.8], [2000.0, 2.5, 0.45]],
    direct: 0.25,
    lp_hz: [900.0, 4200.0],
    rasp: 0.6,
    rasp_hz: 2100.0,
    intake: 0.5,
    intake_hz: 900.0,
    clatter: 0.0,
    turbo: 0.8,
    blower: 0.0,
    pops: 1.0,
    limiter_hz: 20.0,
    level: 1.3,
};

const DIESEL: Voice = Voice {
    cylinders: 6,
    gain: ONES,
    offset: EVEN,
    width: 0.6,
    snap: 0.2,
    jitter: 0.05,
    formants: [[100.0, 1.4, 0.9], [300.0, 2.2, 0.5], [850.0, 3.0, 0.3]],
    direct: 0.35,
    lp_hz: [500.0, 1800.0],
    rasp: 0.15,
    rasp_hz: 800.0,
    intake: 0.4,
    intake_hz: 500.0,
    clatter: 1.0,
    turbo: 0.7,
    blower: 0.0,
    pops: 0.0,
    limiter_hz: 10.0,
    level: 1.0,
};

/// The engine sound of one car.
#[derive(Clone, Debug)]
pub struct EngineSynth {
    target: EngineParams,
    voice: Voice,
    /// The first parameters snap the slews instead of sweeping from the defaults.
    fresh: bool,
    rpm: Slew,
    throttle: Slew,
    /// Load: the throttle, less while the gearbox cuts torque.
    load: Slew,
    /// Shift dip and limiter cut, multipliers.
    dip: Slew,
    /// Engine-cycle phase, `[0, 1)`.
    phase: f32,
    /// Firing intervals since each cylinder fired, and the level of its pulse.
    at: [f32; MAX_CYLINDERS],
    level: [f32; MAX_CYLINDERS],
    formants: [Svf; 3],
    lp: Svf,
    dc: DcBlock,
    rasp: Svf,
    intake: Svf,
    clatter: Decay,
    clatter_bp: Svf,
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
    /// Turbo: turbine speed and boost (`[0, 1]`), the whistle's phase, the blow-off.
    spool: f32,
    boost: f32,
    whistle_phase: f32,
    whistle_hiss: Svf,
    bov: Decay,
    bov_bp: Svf,
    /// Supercharger whine phase.
    blower_phase: f32,
    /// Per-tick values.
    tone_amp: f32,
    width: f32,
    rasp_amp: f32,
    intake_amp: f32,
    jitter: f32,
    limiting: bool,
    whistle_amp: f32,
    whistle_hz: f32,
    hiss_amp: f32,
    blower_amp: f32,
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
            fresh: true,
            rpm: Slew::new(target.rpm, 0.02),
            throttle: Slew::new(0.0, 0.03),
            load: Slew::new(0.0, 0.04),
            dip: Slew::new(1.0, 0.02),
            phase: 0.0,
            at: [0.0; MAX_CYLINDERS],
            level: [1.0; MAX_CYLINDERS],
            formants: [Svf::new(100.0, 1.0); 3],
            lp: Svf::new(1000.0, 0.7),
            dc: DcBlock::default(),
            rasp: Svf::new(1000.0, 0.8),
            intake: Svf::new(600.0, 2.0),
            clatter: Decay::SILENT,
            clatter_bp: Svf::new(2600.0, 1.5),
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
            spool: 0.0,
            boost: 0.0,
            whistle_phase: 0.0,
            whistle_hiss: Svf::new(4000.0, 3.0),
            bov: Decay::SILENT,
            bov_bp: Svf::new(2000.0, 1.2),
            blower_phase: 0.0,
            tone_amp: 0.0,
            width: 0.5,
            rasp_amp: 0.0,
            intake_amp: 0.0,
            jitter: 0.0,
            limiting: false,
            whistle_amp: 0.0,
            whistle_hz: 2000.0,
            hiss_amp: 0.0,
            blower_amp: 0.0,
            tick: 0,
        };
        s.set_voice(target.profile);
        s
    }

    fn set_voice(&mut self, profile: EngineProfile) {
        self.voice = *profile.voice();
        for (f, &[hz, q, _]) in self.formants.iter_mut().zip(&self.voice.formants) {
            f.set(hz, q);
        }
        self.rasp.set(self.voice.rasp_hz, 0.8);
        self.intake.set(self.voice.intake_hz, 2.0);
    }

    /// The engine this frame (already sanitized). Notes gear changes and lift-offs.
    pub fn set_params(&mut self, p: &EngineParams) {
        if p.profile != self.target.profile || self.fresh {
            self.set_voice(p.profile);
        }
        if self.fresh {
            self.rpm.value = p.rpm;
            self.throttle.value = p.throttle;
            self.load.value = p.throttle;
            self.last_throttle = p.throttle;
            self.fresh = false;
        }
        let old = self.target;
        if old.gear >= 1 && p.gear >= 1 && p.gear != old.gear {
            self.shift_left = p.shift_ms * 0.001;
            // A turbo's dump valve chirps on a shift on boost.
            if self.boost > 0.4 && p.throttle > 0.5 {
                self.blow_off(0.5 * self.boost, 0.07);
                self.boost *= 0.6;
            }
        }
        let span = p.redline_rpm - p.idle_rpm;
        let rpm_n = unit((p.rpm - p.idle_rpm) / span);
        let lift = self.last_throttle > 0.55 && p.throttle < 0.2;
        if lift && rpm_n > 0.5 && self.voice.pops > 0.0 {
            self.pops_left = 2 + self.noise.next_u32() % 4;
            self.pop_timer = self.noise.range(0.03, 0.09);
        }
        if lift && self.boost > 0.3 {
            self.blow_off(self.boost, 0.22);
            self.boost = 0.0;
        }
        self.last_throttle = p.throttle;
        self.target = *p;
    }

    fn blow_off(&mut self, level: f32, tau: f32) {
        if self.voice.turbo > 0.0 {
            self.bov = Decay::hit(level * self.voice.turbo, tau);
        }
    }

    /// Fills `out` (mono) with the next `out.len()` samples.
    pub fn render(&mut self, out: &mut [f32]) {
        let cyl = usize::from(self.voice.cylinders).clamp(1, MAX_CYLINDERS);
        for o in out.iter_mut() {
            if self.tick == 0 {
                self.update_tick();
            }
            self.tick = (self.tick + 1) % TICK;
            *o = self.sample(cyl);
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
        let load = self.load.value;

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
        self.idle_phase = fract(self.idle_phase + 0.5 * dt);

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

        // Turbo: the turbine follows the exhaust flow, slower up than down; boost is the
        // spool with the throttle open.
        if v.turbo > 0.0 {
            let flow = unit(0.15 + 0.85 * load) * smoothstep(0.1, 0.75, rpm_n);
            let tau = if flow > self.spool { 0.7 } else { 0.35 };
            self.spool += (flow - self.spool) * unit(dt / tau);
            let want = self.spool * smoothstep(0.3, 0.8, load);
            self.boost += (want - self.boost) * unit(dt / 0.25);
            self.whistle_hz = 1500.0 + 3000.0 * self.spool;
            self.whistle_amp = v.turbo * 0.008 * self.spool * self.spool;
            self.hiss_amp = v.turbo * 0.05 * self.boost;
            self.whistle_hiss.set(1.2 * self.whistle_hz, 3.0);
        }
        self.blower_amp = v.blower * 0.025 * (0.3 + 0.7 * load) * (0.2 + 0.8 * rpm_n);

        // Load narrows the pulses and opens the cabin; idle (closed throttle, low rpm) is
        // held down to a low, steady hum under the mix.
        self.width = (v.width * (1.0 - v.snap * load)).clamp(0.15, 1.0);
        let open = unit(0.65 * load + 0.35 * rpm_n);
        let dip = self.dip.value;
        let lp_hz = (v.lp_hz[0] + (v.lp_hz[1] - v.lp_hz[0]) * open) * (0.6 + 0.4 * dip);
        self.lp.set(lp_hz, 0.7);
        if self.bov.active() {
            let k = self.bov.value / (v.turbo + 1e-6);
            self.bov_bp.set(700.0 + 2600.0 * unit(k), 1.2);
        }
        let idle = (1.0 - thr) * (1.0 - unit(rpm_n / 0.15));
        let calm = 1.0 - IDLE_CUT * idle;
        // The overrun (closed throttle, high rpm) is hollow: softer pulses.
        let overrun = (1.0 - unit(thr / 0.15)) * smoothstep(0.1, 0.5, rpm_n);
        // Fewer cylinders fire fewer pulses a second: low down, make up for it (to the
        // level of an eight).
        let few = (8.0 / f32::from(v.cylinders.max(1))).sqrt();
        let fill = 1.0 + (few - 1.0) * (1.0 - rpm_n);
        self.tone_amp =
            calm * fill * (0.4 + 0.6 * load) * (0.7 + 0.3 * rpm_n) * (1.0 - 0.35 * overrun);
        self.rasp_amp = v.rasp * (0.15 + 0.85 * load) * (0.4 + 0.6 * rpm_n);
        self.intake_amp = v.intake * thr * thr * (0.3 + 0.7 * rpm_n);
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
    fn sample(&mut self, cyl: usize) -> f32 {
        let t = &self.target;
        let v = &self.voice;
        let idle_hunt = if t.throttle < 0.1 && t.rpm < 1.15 * t.idle_rpm {
            1.0 + 0.004 * sin_turns(self.idle_phase)
        } else {
            1.0
        };
        let cut = self.limiting && self.limiter_phase > 0.55;
        let rpm = self
            .rpm
            .step(t.rpm * idle_hunt * if cut { 0.97 } else { 1.0 });
        self.throttle.step(t.throttle);
        let shifting = self.shift_left > 0.0;
        self.load.step(if shifting {
            0.15 * t.throttle
        } else {
            t.throttle
        });
        let dip = self.dip.step(if shifting { 0.6 } else { 1.0 });

        // The engine cycle; each firing's pulse.
        self.phase = fract(self.phase + rpm * (DT / 120.0));
        let pos = self.phase * cyl as f32;
        let fire_hz = rpm * cyl as f32 / 120.0;
        let width = self.width.min(PULSE_MAX_S * fire_hz).max(0.05);
        let inv_width = 1.0 / width;
        let inv_cyl = 1.0 / cyl as f32;
        let mut pulses = 0.0;
        let mut fired = false;
        for c in 0..cyl {
            // Firing intervals since this cylinder last fired, `[0, cyl)`.
            let at = fract((pos - c as f32 - v.offset[c]) * inv_cyl) * cyl as f32;
            if at < self.at[c] {
                // A new firing: its strength this time (a cut on the limiter).
                let gate = if cut { 0.1 } else { 1.0 };
                self.level[c] = v.gain[c] * gate * (1.0 + self.jitter * self.noise.white());
                fired = true;
            }
            self.at[c] = at;
            let x = at * inv_width;
            if x < 1.0 {
                // sin²(πx): smooth at both ends, so the spectrum falls fast and never buzzes.
                pulses += self.level[c] * (0.5 - 0.5 * sin_turns(x + 0.25));
            }
        }
        if fired && v.clatter > 0.0 {
            let rpm_n = unit((rpm - t.idle_rpm) / (t.redline_rpm - t.idle_rpm));
            self.clatter = Decay::hit(v.clatter * (1.0 - 0.6 * rpm_n), 0.0015);
        }

        let ac = self.dc.step(pulses);
        let mut body = v.direct * ac;
        for (f, fv) in self.formants.iter_mut().zip(&v.formants) {
            body += fv[2] * f.band(ac);
        }
        let n = self.noise.white();
        let rasp = self.rasp.band(n * pulses) * self.rasp_amp;
        let intake = self.intake.band(0.5 * ac + 0.25 * n) * self.intake_amp;
        let mut y = self.lp.low(body * self.tone_amp + rasp + intake);
        if self.clatter.active() {
            y += self.clatter_bp.band(self.noise.white()) * self.clatter.step() * 0.6;
        }

        // Forced induction.
        if v.turbo > 0.0 {
            self.whistle_phase = fract(self.whistle_phase + self.whistle_hz * DT);
            y += self.whistle_amp * sin_turns(self.whistle_phase);
            y += self.hiss_amp * self.whistle_hiss.band(self.noise.white());
            if self.bov.active() {
                y += self.bov_bp.band(self.noise.white()) * self.bov.step() * 0.35;
            }
        }
        if v.blower > 0.0 {
            // A roots blower: 12 lobe passes per revolution.
            self.blower_phase = fract(self.blower_phase + rpm * (12.0 * DT / 60.0));
            let b = sin_turns(self.blower_phase) + 0.3 * sin_turns(2.0 * self.blower_phase);
            y += self.blower_amp * b;
        }

        let mut pop = 0.0;
        if self.pop_env.active() || self.pop_kick.active() {
            let crack = self.pop_bp.band(self.noise.white()) * self.pop_env.step() * 6.0;
            let kick = sin_turns(self.pop_phase) * self.pop_kick.step();
            self.pop_phase = fract(self.pop_phase + 70.0 * DT);
            pop = crack + kick;
        }
        let y = (y * dip + 0.5 * pop) * LEVEL * v.level;
        if y.is_finite() {
            y
        } else {
            self.reset_filters();
            0.0
        }
    }

    fn reset_filters(&mut self) {
        for f in &mut self.formants {
            f.reset();
        }
        self.lp.reset();
        self.dc = DcBlock::default();
        self.rasp.reset();
        self.intake.reset();
        self.clatter_bp.reset();
        self.whistle_hiss.reset();
        self.bov_bp.reset();
        self.pop_bp.reset();
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

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn profiles_resolve_by_name() {
        for p in EngineProfile::ALL {
            assert_eq!(EngineProfile::from_name(p.name()), p);
            let v = p.voice();
            assert!(v.cylinders >= 1 && usize::from(v.cylinders) <= MAX_CYLINDERS);
            assert!(v.width > 0.0 && v.width <= 1.0 && v.snap < 1.0);
        }
        assert_eq!(EngineProfile::from_name("steam"), EngineProfile::Default);
    }

    #[test]
    fn pitch_tracks_rpm() {
        // Firing frequency = rpm × 8 / 120: 60 Hz at idle, 200 at 3000, 400 at 6000.
        for (rpm, fire) in [(900.0, 60.0), (3000.0, 200.0), (6000.0, 400.0)] {
            let x = render(params(rpm, 0.6), 0.5);
            let at = bin(&x, fire);
            let off = bin(&x, fire * 1.37);
            assert!(
                at > 4.0 * off,
                "{rpm} rpm: {at} at {fire} Hz vs {off} off it"
            );
        }
    }

    /// A cross-plane V8's banks are unequal, so it has energy at the low engine orders
    /// below the firing frequency (the burble); a flat-plane's banks alternate, so none.
    #[test]
    fn a_cross_plane_v8_burbles() {
        let half = |profile| {
            let mut p = params(1500.0, 0.3);
            p.profile = profile;
            let x = render(p, 1.0);
            let orders: f32 = [1.0, 2.0, 3.0, 5.0, 6.0, 7.0]
                .iter()
                .map(|k| bin(&x, 12.5 * k))
                .sum();
            orders / bin(&x, 100.0)
        };
        let lazy = half(EngineProfile::V8Lazy);
        let flat = half(EngineProfile::V8Flatplane);
        assert!(lazy > 0.15, "{lazy}");
        assert!(lazy > 2.0 * flat, "{lazy} vs {flat}");
    }

    /// Nothing buzzes: at a full-throttle 6000 rpm, energy above 5 kHz is a small share.
    #[test]
    fn no_buzz_up_top() {
        for profile in EngineProfile::ALL {
            let mut p = params(6000.0, 1.0);
            p.profile = profile;
            let x = render(p, 0.25);
            // A one-pole high-pass split at ~5 kHz.
            let a = 0.52;
            let (mut lp, mut hi) = (0.0f32, Vec::with_capacity(x.len()));
            for &v in &x {
                lp += a * (v - lp);
                hi.push(v - lp);
            }
            let share = rms(&hi) / rms(&x);
            assert!(share < 0.2, "{profile:?}: {share}");
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

    /// Every engine is about as loud as the others on a pull (within 4 dB of the median).
    #[test]
    fn profiles_are_balanced() {
        let mut levels: Vec<(EngineProfile, f32)> = EngineProfile::ALL
            .into_iter()
            .map(|profile| {
                let mut p = params(4000.0, 0.8);
                p.profile = profile;
                (profile, 20.0 * rms(&render(p, 0.5)).log10())
            })
            .collect();
        for (profile, db) in &levels {
            let mut p = params(900.0, 0.0);
            p.profile = *profile;
            let idle = 20.0 * rms(&render(p, 0.5)).log10();
            p.rpm = 7000.0;
            p.throttle = 1.0;
            let x = render(p, 0.5);
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            println!("{profile:?}: pull {db:.1} dB, idle {idle:.1} dB, top peak {peak:.2}");
        }
        levels.sort_by(|a, b| a.1.total_cmp(&b.1));
        let median = levels[levels.len() / 2].1;
        for (profile, db) in levels {
            assert!(
                (db - median).abs() < 4.0,
                "{profile:?}: {db:.1} vs {median:.1}"
            );
        }
    }

    #[test]
    fn a_shift_dips_and_a_lift_pops() {
        let mut e = EngineSynth::new();
        let mut p = params(6500.0, 1.0);
        e.set_params(&p);
        let mut buf = vec![0.0; 4800];
        e.render(&mut buf);
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

    /// A turbo spools up under load and blows off when the throttle closes on boost.
    #[test]
    fn a_turbo_spools_and_blows_off() {
        let mut e = EngineSynth::new();
        let mut p = params(5500.0, 1.0);
        p.profile = EngineProfile::I4Turbo;
        e.set_params(&p);
        let mut buf = vec![0.0; 96_000];
        e.render(&mut buf[..4800]);
        let early = e.boost;
        e.render(&mut buf);
        assert!(early < 0.5 && e.boost > 0.6, "{early} -> {}", e.boost);
        p.throttle = 0.0;
        e.set_params(&p);
        assert!(e.bov.active() && e.boost == 0.0);
        // A non-turbo engine never blows off.
        let mut e = EngineSynth::new();
        p.throttle = 1.0;
        p.profile = EngineProfile::V8Lazy;
        e.set_params(&p);
        e.render(&mut buf);
        p.throttle = 0.0;
        e.set_params(&p);
        assert!(!e.bov.active());
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
        let mut p = params(5000.0, 0.8);
        p.profile = EngineProfile::V12;
        e.set_params(&p);
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
