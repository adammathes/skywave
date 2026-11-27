//! The sounds that say which place this is, played inside [`super::ambience`] by their
//! amounts in [`AmbienceParams`]: the generic beds say "a city", these say "this city".
//! Each is an event on its own timer (a train every minute or so), heard when its amount
//! is up as the timer fires, except the carhop speaker, which plays while it is near.
//!
//! - **Elevated train** (Lake City): a train on its steel structure going by over ten
//!   seconds: the structure's roar, the wheels' paired clacks over the rail joints, car
//!   after car, and sometimes a flange squeal on the curve, panning past.
//! - **Cable car bell** (the Bay): a gripman ringing a rhythm on a small bronze bell,
//!   four inharmonic partials, a block or two away.
//! - **Gulls** (the Bay): a gull's long call, a run of "kyow" notes, each a buzzy
//!   harmonic tone rising then falling, with a raspy edge.
//! - **Helicopter** (Mirage Bowl): a news helicopter crossing overhead over twenty
//!   seconds: blade slap at about 20 a second, Doppler-shifted, over its turbine.
//! - **Ice cream truck** (Mirage Bowl, by day): a music-box chime tune of our own drifting
//!   through the neighborhood as the truck passes.
//! - **Carhop speaker** (any world's hot dog and burger stands): a tinny speaker on a pole
//!   playing a doo-wop loop of our own, and the grill's sizzle.
//!
//! Every tune here is original: a melody written for the game, nothing quoted.

use super::ambience::AmbienceParams;
use super::dsp::{exp2, fract, pan_gains, sin_turns, unit, Decay, Noise, Svf, DT};

/// A pass: how far in, how long, and the pan it travels.
#[derive(Clone, Copy, Debug)]
struct Pass {
    t: f32,
    dur: f32,
    pan: [f32; 2],
    level: f32,
}

impl Pass {
    const IDLE: Pass = Pass {
        t: 0.0,
        dur: 0.0,
        pan: [0.0; 2],
        level: 0.0,
    };

    fn start(n: &mut Noise, dur: f32, level: f32) -> Pass {
        let from = n.range(-1.0, 1.0);
        Pass {
            t: 0.0,
            dur,
            pan: [from, -from * n.range(0.5, 1.0)],
            level,
        }
    }

    fn on(&self) -> bool {
        self.t < self.dur
    }

    /// Fraction of the way through.
    fn x(&self) -> f32 {
        self.t / self.dur
    }

    /// Loudness: up over the first `rise` of the pass, down over the rest, smoothly.
    fn env(&self, rise: f32) -> f32 {
        let x = self.x();
        let s = if x < rise {
            x / rise
        } else {
            1.0 - (x - rise) / (1.0 - rise)
        };
        let s = unit(s);
        s * s * (3.0 - 2.0 * s)
    }

    fn gains(&self) -> [f32; 2] {
        pan_gains(self.pan[0] + (self.pan[1] - self.pan[0]) * self.x())
    }
}

/// MIDI note to Hz.
fn hz(note: f32) -> f32 {
    440.0 * exp2((note - 69.0) / 12.0)
}

/// The ice cream truck's tune: (eighth-note step, MIDI note), 32 steps a loop, F major.
const CHIME_TUNE: [(u8, u8); 22] = [
    (0, 72),
    (2, 77),
    (3, 76),
    (4, 74),
    (6, 72),
    (8, 69),
    (9, 70),
    (10, 72),
    (12, 77),
    (14, 74),
    (16, 72),
    (18, 70),
    (19, 69),
    (20, 67),
    (22, 69),
    (23, 70),
    (24, 72),
    (25, 74),
    (26, 72),
    (27, 70),
    (28, 69),
    (30, 65),
];
/// Seconds per eighth note.
const CHIME_STEP_S: f32 = 0.2;

/// The carhop loop: four bars of 8 eighths, I-vi-IV-V in C (roots as MIDI notes), and the
/// lead over it, (step, note).
const CARHOP_ROOTS: [u8; 4] = [48, 45, 41, 43];
const CARHOP_LEAD: [(u8, u8); 14] = [
    (0, 76),
    (2, 79),
    (3, 76),
    (4, 72),
    (8, 76),
    (10, 74),
    (12, 72),
    (16, 77),
    (18, 76),
    (20, 74),
    (22, 72),
    (24, 74),
    (26, 71),
    (28, 67),
];
const CARHOP_STEP_S: f32 = 0.24;

/// A small bronze bell's partials, as ratios of its strike note, their levels and decays.
const BELL_RATIOS: [f32; 4] = [1.0, 2.38, 3.07, 4.53];
const BELL_LEVELS: [f32; 4] = [1.0, 0.55, 0.4, 0.2];
/// Cable car bell rhythms, in eighths (a 1 strikes).
const BELL_RHYTHMS: [u16; 4] = [
    0b1101_1011_0000_0000,
    0b1011_0110_1101_0000,
    0b1110_1110_0000_0000,
    0b1001_0010_0100_1111,
];

/// The signature sounds' state.
#[derive(Clone, Debug)]
pub struct Signatures {
    train_timer: f32,
    train: Pass,
    train_roar: Svf,
    train_body: Svf,
    clack_bp: Svf,
    clack: Decay,
    clack_clock: f32,
    clack_phase: f32,
    squeal: bool,
    squeal_phase: [f32; 2],
    train_lp_l: Svf,
    train_lp_r: Svf,

    bell_timer: f32,
    bell_step: f32,
    bell_rhythm: u16,
    bell_hz: f32,
    bell_pan: [f32; 2],
    bell_level: f32,
    bell: [Decay; 4],
    bell_phase: [f32; 4],
    bell_click: Decay,
    bell_click_bp: Svf,

    gull_timer: f32,
    gull_notes: u32,
    gull_t: f32,
    gull_dur: f32,
    gull_f0: f32,
    gull_phase: f32,
    gull_pan: [f32; 2],
    gull_level: f32,
    gull_bp: Svf,

    heli_timer: f32,
    heli: Pass,
    heli_blade: f32,
    heli_slap: Decay,
    heli_thump: f32,
    heli_lp: Svf,
    heli_turbine: Svf,

    truck_timer: f32,
    truck: Pass,
    chime_step: f32,
    chime: [Decay; 2],
    chime_hz: [f32; 2],
    chime_phase: [f32; 2],
    chime_voice: usize,

    carhop_step: f32,
    carhop_phase: [f32; 5],
    carhop_hz: [f32; 5],
    carhop_lead: Decay,
    carhop_lead_hz: f32,
    carhop_chord: Decay,
    carhop_bass: Decay,
    speaker_hp: Svf,
    speaker_lp: Svf,
    sizzle_bp: Svf,
}

impl Default for Signatures {
    fn default() -> Self {
        Self::new()
    }
}

impl Signatures {
    pub fn new() -> Signatures {
        Signatures {
            train_timer: 2.0,
            train: Pass::IDLE,
            train_roar: Svf::new(110.0, 0.7),
            train_body: Svf::new(380.0, 1.2),
            clack_bp: Svf::new(1300.0, 2.0),
            clack: Decay::SILENT,
            clack_clock: 0.0,
            clack_phase: 0.0,
            squeal: false,
            squeal_phase: [0.0; 2],
            train_lp_l: Svf::new(3000.0, 0.6),
            train_lp_r: Svf::new(3200.0, 0.6),
            bell_timer: 2.0,
            bell_step: f32::INFINITY,
            bell_rhythm: 0,
            bell_hz: 1180.0,
            bell_pan: [0.7; 2],
            bell_level: 0.0,
            bell: [Decay::SILENT; 4],
            bell_phase: [0.0; 4],
            bell_click: Decay::SILENT,
            bell_click_bp: Svf::new(3500.0, 1.5),
            gull_timer: 1.5,
            gull_notes: 0,
            gull_t: 0.0,
            gull_dur: 0.0,
            gull_f0: 1000.0,
            gull_phase: 0.0,
            gull_pan: [0.7; 2],
            gull_level: 0.0,
            gull_bp: Svf::new(2200.0, 1.5),
            heli_timer: 1.0,
            heli: Pass::IDLE,
            heli_blade: 0.0,
            heli_slap: Decay::SILENT,
            heli_thump: 0.0,
            heli_lp: Svf::new(700.0, 0.8),
            heli_turbine: Svf::new(320.0, 1.0),
            truck_timer: 2.5,
            truck: Pass::IDLE,
            chime_step: 0.0,
            chime: [Decay::SILENT; 2],
            chime_hz: [440.0; 2],
            chime_phase: [0.0; 2],
            chime_voice: 0,
            carhop_step: 0.0,
            carhop_phase: [0.0; 5],
            carhop_hz: [0.0; 5],
            carhop_lead: Decay::SILENT,
            carhop_lead_hz: 440.0,
            carhop_chord: Decay::SILENT,
            carhop_bass: Decay::SILENT,
            speaker_hp: Svf::new(450.0, 0.7),
            speaker_lp: Svf::new(3200.0, 0.9),
            sizzle_bp: Svf::new(5500.0, 0.8),
        }
    }

    /// The slow part: timers and the events they start.
    pub fn tick(&mut self, p: &AmbienceParams, n: &mut Noise, dt: f32) {
        // The train.
        if self.train.on() {
            self.train.t += dt;
        } else {
            self.train_timer -= dt;
            if self.train_timer <= 0.0 {
                self.train_timer = n.range(35.0, 70.0);
                if p.el_train > 0.05 {
                    let dur = n.range(9.0, 12.0);
                    self.train = Pass::start(n, dur, p.el_train);
                    self.squeal = n.unit() < 0.6;
                }
            }
        }

        // The cable car's bell.
        if self.bell_step >= 16.0 {
            self.bell_timer -= dt;
            if self.bell_timer <= 0.0 {
                self.bell_timer = n.range(9.0, 22.0);
                if p.cable_car > 0.05 {
                    self.bell_step = -1e-3;
                    self.bell_rhythm = BELL_RHYTHMS[(n.next_u32() % 4) as usize];
                    self.bell_hz = n.range(1100.0, 1300.0);
                    self.bell_pan = pan_gains(n.range(-0.8, 0.8));
                    self.bell_level = p.cable_car * n.range(0.6, 1.0);
                }
            }
        }

        // Gulls.
        if self.gull_notes == 0 {
            self.gull_timer -= dt;
            if self.gull_timer <= 0.0 {
                self.gull_timer = n.range(4.0, 12.0);
                if p.gulls > 0.05 {
                    self.gull_notes = 3 + n.next_u32() % 5;
                    self.gull_t = 0.0;
                    self.gull_dur = n.range(0.32, 0.45);
                    self.gull_f0 = n.range(850.0, 1100.0);
                    self.gull_pan = pan_gains(n.range(-0.9, 0.9));
                    self.gull_level = p.gulls * n.range(0.5, 1.0);
                }
            }
        }

        // The helicopter.
        if self.heli.on() {
            self.heli.t += dt;
        } else {
            self.heli_timer -= dt;
            if self.heli_timer <= 0.0 {
                self.heli_timer = n.range(50.0, 100.0);
                if p.helicopter > 0.05 {
                    let dur = n.range(18.0, 24.0);
                    self.heli = Pass::start(n, dur, p.helicopter);
                }
            }
        }

        // The ice cream truck.
        if self.truck.on() {
            self.truck.t += dt;
        } else {
            self.truck_timer -= dt;
            if self.truck_timer <= 0.0 {
                self.truck_timer = n.range(60.0, 120.0);
                if p.ice_cream > 0.05 {
                    let dur = n.range(16.0, 22.0);
                    self.truck = Pass::start(n, dur, p.ice_cream);
                    self.chime_step = -1e-3;
                }
            }
        }
    }

    /// The next frame: what plays dry, and what goes to the echo.
    #[inline]
    pub fn sample(&mut self, p: &AmbienceParams, n: &mut Noise) -> ([f32; 2], [f32; 2]) {
        let mut dry = [0.0f32; 2];
        let mut send = [0.0f32; 2];
        let w = n.white();

        if self.train.on() {
            self.train_sample(w, &mut dry);
        }
        if self.bell_step < 16.0 {
            self.bell_sample(w, &mut dry, &mut send);
        }
        if self.gull_notes > 0 {
            self.gull_sample(n, &mut dry);
        }
        if self.heli.on() {
            self.heli_sample(w, &mut dry);
        }
        if self.truck.on() {
            self.truck_sample(&mut send);
        }
        if p.carhop > 1e-3 {
            self.carhop_sample(p.carhop, w, n, &mut dry);
        }
        (dry, send)
    }

    fn train_sample(&mut self, w: f32, out: &mut [f32; 2]) {
        let tr = &self.train;
        let env = tr.env(0.42) * tr.level;
        // Clacks: each car's two trucks over a joint, a car every 1.05 s at speed.
        self.clack_clock += DT / 1.05;
        if self.clack_clock >= 1.0 {
            self.clack_clock -= 1.0;
        }
        let c = self.clack_clock;
        let joint = |a: f32| a <= c && c < a + DT / 1.05;
        if joint(0.0) || joint(0.16) || joint(0.5) || joint(0.66) {
            self.clack = Decay::hit(1.0, 0.025);
            self.clack_phase = 0.0;
        }
        let k = self.clack.step();
        let clack = (self.clack_bp.band(w) * 0.8 + sin_turns(self.clack_phase) * 0.6) * k;
        self.clack_phase = fract(self.clack_phase + 75.0 * DT);
        let roar = self.train_roar.band(w) * 1.3 + self.train_body.band(w) * 0.5;
        let mut x = roar + clack;
        // The flange squeal, through the middle of the pass, wavering.
        if self.squeal {
            let xs = tr.x();
            let s = unit((xs - 0.3) / 0.1).min(unit((0.7 - xs) / 0.15));
            if s > 0.0 {
                let wob = sin_turns(self.squeal_phase[1]) * 40.0;
                self.squeal_phase[1] = fract(self.squeal_phase[1] + 5.5 * DT);
                self.squeal_phase[0] = fract(self.squeal_phase[0] + (2650.0 + wob) * DT);
                let ph = self.squeal_phase[0];
                x += (sin_turns(ph) + 0.4 * sin_turns(1.41 * ph)) * s * 0.06;
            }
        }
        let [gl, gr] = tr.gains();
        let g = env * 0.18;
        out[0] += self.train_lp_l.low(x) * g * gl;
        out[1] += self.train_lp_r.low(x) * g * gr;
    }

    fn bell_sample(&mut self, w: f32, out: &mut [f32; 2], send: &mut [f32; 2]) {
        let before = self.bell_step.floor();
        self.bell_step += DT / 0.16;
        let now = self.bell_step.floor();
        let strike = now != before && (0.0..16.0).contains(&now);
        if strike && self.bell_rhythm & (0x8000 >> now as u32) != 0 {
            for (k, d) in self.bell.iter_mut().enumerate() {
                *d = Decay::hit(BELL_LEVELS[k], 0.14 / (1.0 + 0.5 * k as f32));
            }
            self.bell_click = Decay::hit(1.0, 0.004);
        }
        let mut tone = 0.0;
        for k in 0..4 {
            let e = self.bell[k].step();
            tone += sin_turns(self.bell_phase[k]) * e;
            self.bell_phase[k] = fract(self.bell_phase[k] + self.bell_hz * BELL_RATIOS[k] * DT);
        }
        // The clapper's tick on each strike.
        let click = self.bell_click_bp.band(w) * self.bell_click.step() * 0.8;
        let b = (tone + click) * self.bell_level * 0.03;
        // Mostly dry, a little off the walls: the rhythm must stay crisp.
        out[0] += b * self.bell_pan[0];
        out[1] += b * self.bell_pan[1];
        send[0] += 0.25 * b * self.bell_pan[0];
        send[1] += 0.25 * b * self.bell_pan[1];
        // Let the last strike ring out before the bell counts as done.
        if self.bell_step >= 16.0 && self.bell[0].value > 1e-3 {
            self.bell_step = 15.99;
        }
    }

    fn gull_sample(&mut self, n: &mut Noise, out: &mut [f32; 2]) {
        let t = self.gull_t;
        if t < self.gull_dur {
            let x = t / self.gull_dur;
            // Rises fast to its top a quarter of the way, then sags.
            let contour = if x < 0.25 {
                x / 0.25
            } else {
                1.0 - 0.7 * (x - 0.25) / 0.75
            };
            let f = self.gull_f0 * (0.75 + 0.6 * contour);
            let env = unit(x / 0.08).min(unit((1.0 - x) / 0.3));
            self.gull_phase = fract(self.gull_phase + f * DT);
            let ph = self.gull_phase;
            let mut v = 0.0;
            for k in 1..=6 {
                v += sin_turns(k as f32 * ph) / (k as f32).sqrt();
            }
            // The rasp: a fast, noisy flutter on the voice.
            let rasp = 0.75 + 0.25 * n.white();
            let s = (self.gull_bp.band(v) * 1.4 + v * 0.15) * rasp * env * self.gull_level * 0.028;
            out[0] += s * self.gull_pan[0];
            out[1] += s * self.gull_pan[1];
        }
        self.gull_t += DT;
        let gap = if self.gull_notes > 1 { 0.09 } else { 0.0 };
        if self.gull_t >= self.gull_dur + gap {
            self.gull_notes -= 1;
            self.gull_t = 0.0;
            // The first note of a long call is the longest; the rest come quicker.
            self.gull_dur *= 0.8;
            self.gull_f0 *= 1.03;
        }
    }

    fn heli_sample(&mut self, w: f32, out: &mut [f32; 2]) {
        let h = &self.heli;
        let x = h.x();
        let env = h.env(0.45) * h.level;
        // Doppler: a little sharp coming, flat going.
        let rate = 19.0 * (1.04 - 0.08 * x);
        self.heli_blade += rate * DT;
        if self.heli_blade >= 1.0 {
            self.heli_blade -= 1.0;
            self.heli_slap = Decay::hit(1.0, 0.012);
            self.heli_thump = 0.0;
        }
        let e = self.heli_slap.step();
        // Each blade's slap: a burst of air and a low thump under it.
        let thump = sin_turns(self.heli_thump) * e * 0.6;
        self.heli_thump = fract(self.heli_thump + 85.0 * DT);
        let slap = self.heli_lp.low(w) * e * 4.0 + thump;
        let turbine = self.heli_turbine.band(w) * 0.25;
        let s = (slap + turbine) * env * 0.08;
        let [gl, gr] = h.gains();
        out[0] += s * gl;
        out[1] += s * gr;
    }

    fn truck_sample(&mut self, out: &mut [f32; 2]) {
        let before = self.chime_step.floor();
        self.chime_step += DT / CHIME_STEP_S;
        let now = self.chime_step.floor();
        if now != before {
            let step = (now as u32 % 32) as u8;
            if let Some(&(_, note)) = CHIME_TUNE.iter().find(|(s, _)| *s == step) {
                let v = self.chime_voice;
                self.chime[v] = Decay::hit(1.0, 0.35);
                self.chime_hz[v] = hz(f32::from(note));
                self.chime_phase[v] = 0.0;
                self.chime_voice = 1 - v;
            }
        }
        let mut tone = 0.0;
        for v in 0..2 {
            let e = self.chime[v].step();
            let ph = self.chime_phase[v];
            // A music box's tine: the note, a soft octave and a bright, quick partial.
            tone +=
                (sin_turns(ph) + 0.25 * sin_turns(2.0 * ph) + 0.15 * e * sin_turns(5.4 * ph)) * e;
            self.chime_phase[v] = fract(ph + self.chime_hz[v] * DT);
        }
        let tr = &self.truck;
        let s = tone * tr.env(0.5) * tr.level * 0.03;
        let [gl, gr] = tr.gains();
        out[0] += s * gl;
        out[1] += s * gr;
    }

    fn carhop_sample(&mut self, amount: f32, w: f32, n: &mut Noise, out: &mut [f32; 2]) {
        let before = self.carhop_step as u32;
        self.carhop_step += DT / CARHOP_STEP_S;
        if self.carhop_step >= 32.0 {
            self.carhop_step -= 32.0;
        }
        let now = self.carhop_step as u32;
        let bar = (now / 8) as usize;
        let root = f32::from(CARHOP_ROOTS[bar]);
        if now != before {
            let beat = now % 8;
            // Bass on the beat, root and fifth; the chord on the off-beats (the shuffle).
            if beat.is_multiple_of(2) {
                self.carhop_bass = Decay::hit(1.0, 0.18);
            } else {
                self.carhop_chord = Decay::hit(1.0, 0.09);
            }
            if let Some(&(_, note)) = CARHOP_LEAD.iter().find(|(s, _)| u32::from(*s) == now) {
                self.carhop_lead = Decay::hit(1.0, 0.3);
                self.carhop_lead_hz = hz(f32::from(note));
            }
        }
        if now != before || self.carhop_hz[0] == 0.0 {
            // The chord: the root's triad an octave and more up (minor on the vi); the
            // bass walks root and fifth.
            let third = if bar == 1 { 3.0 } else { 4.0 };
            let bass = if now % 4 < 2 { root } else { root + 7.0 };
            self.carhop_hz = [
                hz(bass),
                hz(root + 12.0),
                hz(root + 12.0 + third),
                hz(root + 19.0),
                self.carhop_lead_hz,
            ];
        }
        let mut v = 0.0;
        for (k, (ph, f)) in self.carhop_phase.iter_mut().zip(self.carhop_hz).enumerate() {
            let x = *ph;
            // A square-ish organ for the chord and lead, a round bass.
            let wave = if k == 0 {
                sin_turns(x)
            } else {
                sin_turns(x) + 0.33 * sin_turns(3.0 * x) + 0.2 * sin_turns(5.0 * x)
            };
            let e = match k {
                0 => self.carhop_bass.value,
                4 => self.carhop_lead.value,
                _ => 0.4 * self.carhop_chord.value,
            };
            v += wave * e;
            *ph = fract(x + f * DT);
        }
        self.carhop_bass.step();
        self.carhop_chord.step();
        self.carhop_lead.step();
        // The pole speaker: no lows, no highs, a little overdriven.
        let s = self.speaker_lp.low(self.speaker_hp.step(v).high);
        let s = s / (1.0 + s.abs());
        // The grill: a hiss and a crackle.
        let pop = if n.unit() < 0.004 {
            n.white() * 3.0
        } else {
            0.0
        };
        let sizzle = self.sizzle_bp.band(w * 0.3 + pop) * 0.5;
        let g = amount * 0.05;
        out[0] += (s * 0.9 + sizzle * 0.6) * g;
        out[1] += (s * 0.7 + sizzle) * g;
    }

    /// Clears the filters (after a non-finite sample).
    pub fn reset(&mut self) {
        for f in [
            &mut self.train_roar,
            &mut self.train_body,
            &mut self.clack_bp,
            &mut self.train_lp_l,
            &mut self.train_lp_r,
            &mut self.gull_bp,
            &mut self.bell_click_bp,
            &mut self.heli_lp,
            &mut self.heli_turbine,
            &mut self.speaker_hp,
            &mut self.speaker_lp,
            &mut self.sizzle_bp,
        ] {
            f.reset();
        }
    }
}
