//! Everything but the engine: tires, road, wind, knocks, the horn and the place's ambience,
//! rendered as stereo for the world bus
//! (and the horn for the ui bus).
//!
//! - **Tire squeal**: white noise through two resonant band-passes (a squeal and its
//!   overtone, wandering slowly in pitch), loud as the sim's combined slip passes the
//!   tire's peak, scaled by load and tire speed; on grass a tire scrubs instead. Left wheels
//!   lean left in the stereo image, right wheels right.
//! - **Rolling**: brown noise low-passed by speed, voiced per surface under the grounded
//!   wheels: asphalt smooth, concrete brighter with an expansion-joint knock every
//!   [`JOINT_SPACING_M`], grass a swishing hiss.
//! - **Wind**: band-passed noise, independent per ear, rising with the square of speed,
//!   with a slow gust.
//! - **Curb kick** and **wall hit**: a low thump, louder the faster the bump and leaning
//!   toward the side of the car it was on; the hit adds a short, soft crunch.
//! - **Horn**: two tones a minor third apart (410 and 500 Hz), brassy through a band-pass,
//!   on its own output for the ui bus.
//! - **Ambience**: an ocean wash (low-passed noise swelling every ~12 s) and, by day, the
//!   odd bird call (a few swept chirps panned somewhere), each scaled by its amount.

use super::dsp::{fract, pan_gains, sin_turns, smoothstep, unit, Decay, Noise, Slew, Svf, DT};
use super::{AudioParams, Surface};

/// Filter coefficients, targets and events are updated every this many samples.
const TICK: usize = 32;
/// Concrete slabs are this long, m.
pub const JOINT_SPACING_M: f32 = 4.6;
/// Horn pitches, Hz.
pub const HORN_HZ: [f32; 2] = [410.0, 500.0];

/// The world sounds' state.
#[derive(Clone, Debug)]
pub struct WorldSynth {
    target: AudioParams,
    fresh: bool,
    noise: Noise,
    noise_r: Noise,
    tick: usize,

    squeal_l: Slew,
    squeal_r: Slew,
    squeal_target: [f32; 2],
    squeal_bp: Svf,
    squeal_bp2: Svf,
    wobble: f32,

    brown: f32,
    roll: Slew,
    roll_target: f32,
    roll_lp: Svf,
    grass: Slew,
    grass_target: f32,
    grass_bp: Svf,
    concrete: f32,
    joint_phase: f32,
    joint: Decay,
    joint_osc: f32,

    wind: Slew,
    wind_target: f32,
    wind_l: Svf,
    wind_r: Svf,
    gust_phase: [f32; 2],

    curb: Decay,
    curb_click: Decay,
    curb_osc: f32,
    curb_gain: [f32; 2],
    click_lp: Svf,
    hit: Decay,
    thud: Decay,
    hit_osc: f32,
    hit_gain: [f32; 2],
    hit_lp: Svf,

    horn: f32,
    horn_phase: [f32; 2],
    horn_bp: Svf,

    ocean: Slew,
    ocean_l: Svf,
    ocean_r: Svf,
    swell_phase: [f32; 2],
    swell: f32,

    bird_timer: f32,
    bird_notes: u32,
    bird_t: f32,
    bird_dur: f32,
    bird_gap: f32,
    bird_f0: f32,
    bird_sweep: f32,
    bird_phase: f32,
    bird_pan: [f32; 2],
    bird_level: f32,
}

impl Default for WorldSynth {
    fn default() -> Self {
        WorldSynth::new()
    }
}

impl WorldSynth {
    pub fn new() -> WorldSynth {
        WorldSynth {
            target: AudioParams::default(),
            fresh: true,
            noise: Noise::new(0x0bad_5eed),
            noise_r: Noise::new(0x1234_5679),
            tick: 0,
            squeal_l: Slew::new(0.0, 0.03),
            squeal_r: Slew::new(0.0, 0.03),
            squeal_target: [0.0; 2],
            squeal_bp: Svf::new(850.0, 8.0),
            squeal_bp2: Svf::new(1700.0, 10.0),
            wobble: 0.0,
            brown: 0.0,
            roll: Slew::new(0.0, 0.05),
            roll_target: 0.0,
            roll_lp: Svf::new(200.0, 0.7),
            grass: Slew::new(0.0, 0.05),
            grass_target: 0.0,
            grass_bp: Svf::new(1800.0, 0.8),
            concrete: 0.0,
            joint_phase: 0.0,
            joint: Decay::SILENT,
            joint_osc: 0.0,
            wind: Slew::new(0.0, 0.1),
            wind_target: 0.0,
            wind_l: Svf::new(300.0, 0.6),
            wind_r: Svf::new(300.0, 0.6),
            gust_phase: [0.0, 0.37],
            curb: Decay::SILENT,
            curb_click: Decay::SILENT,
            curb_osc: 0.0,
            curb_gain: [1.0, 1.0],
            click_lp: Svf::new(1500.0, 0.7),
            hit: Decay::SILENT,
            thud: Decay::SILENT,
            hit_osc: 0.0,
            hit_gain: [1.0, 1.0],
            hit_lp: Svf::new(1800.0, 0.7),
            horn: 0.0,
            horn_phase: [0.0; 2],
            horn_bp: Svf::new(2000.0, 1.0),
            ocean: Slew::new(0.0, 0.5),
            ocean_l: Svf::new(500.0, 0.6),
            ocean_r: Svf::new(520.0, 0.6),
            swell_phase: [0.0, 0.6],
            swell: 0.0,
            bird_timer: 1.0,
            bird_notes: 0,
            bird_t: 0.0,
            bird_dur: 0.08,
            bird_gap: 0.05,
            bird_f0: 3000.0,
            bird_sweep: 0.0,
            bird_phase: 0.0,
            bird_pan: [0.7, 0.7],
            bird_level: 0.0,
        }
    }

    /// This frame's parameters (already sanitized). New curb kicks and hits start their
    /// sounds; the first parameters only note the counters.
    pub fn set_params(&mut self, p: &AudioParams) {
        if !self.fresh {
            if p.curb_kicks != self.target.curb_kicks {
                let level = 0.7 * (0.2 + 0.8 * unit(p.curb_speed_mps / 24.0));
                self.curb = Decay::hit(level, 0.07);
                self.curb_click = Decay::hit(0.5 * level, 0.004);
                self.curb_osc = 0.0;
                self.curb_gain = side_gains(p.curb_side);
            }
            if p.hits != self.target.hits {
                let level = 0.15 + 0.55 * unit(p.hit_speed_mps / 12.0);
                if level > self.hit.value {
                    self.hit = Decay::hit(level, 0.045);
                    self.thud = Decay::hit(level, 0.06);
                    self.hit_osc = 0.0;
                    self.hit_gain = side_gains(p.hit_side);
                }
            }
        }
        self.fresh = false;
        self.target = *p;
    }

    /// Fills `world` and `ui` (interleaved stereo, the same length) with the next frames:
    /// the horn goes to `ui` (the car's own signals), everything else to `world`.
    pub fn render(&mut self, world: &mut [f32], ui: &mut [f32]) {
        for (w, u) in world.chunks_exact_mut(2).zip(ui.chunks_exact_mut(2)) {
            if self.tick == 0 {
                self.update_tick();
            }
            self.tick = (self.tick + 1) % TICK;
            let [l, r, horn] = self.sample();
            w[0] = l;
            w[1] = r;
            u[0] = horn;
            u[1] = horn;
        }
    }

    fn update_tick(&mut self) {
        let p = &self.target;
        let dt = TICK as f32 * DT;
        let speed = p.speed_mps.abs();

        // Tires.
        let mut squeal = [0.0f32; 4];
        let mut on = [0.0f32; 3];
        let mut grounded = 0.0;
        for (i, w) in p.wheels.iter().enumerate() {
            if !w.grounded {
                continue;
            }
            grounded += 1.0;
            on[w.surface as usize] += 1.0;
            let grip = match w.surface {
                Surface::Asphalt => 1.0,
                Surface::Concrete => 0.85,
                Surface::Grass => 0.0,
            };
            squeal[i] = grip
                * smoothstep(0.9, 1.7, w.slip)
                * unit(w.speed_mps / 4.0)
                * unit(0.3 + 0.7 * w.load);
        }
        self.squeal_target = [squeal[0].max(squeal[2]), squeal[1].max(squeal[3])];
        let wobble = self.noise.white();
        let mean = 0.5 * (self.squeal_target[0] + self.squeal_target[1]);
        self.wobble += (wobble - self.wobble) * 0.02;
        let hz = 780.0 + 160.0 * mean + 140.0 * self.wobble;
        self.squeal_bp.set(hz, 8.0);
        self.squeal_bp2.set(2.15 * hz, 10.0);

        // Rolling, by surface.
        let frac = |n: f32| if grounded > 0.0 { n / grounded } else { 0.0 };
        let (fa, fc, fg) = (frac(on[0]), frac(on[1]), frac(on[2]));
        let s = unit(speed / 35.0);
        let curve = s * (2.0 - s);
        self.roll_target = (0.16 * fa + 0.2 * fc + 0.12 * fg) * curve;
        self.roll_lp
            .set((90.0 + 10.0 * speed) * (1.0 + 0.3 * fc), 0.7);
        self.grass_target = fg * 0.22 * unit(speed / 8.0) * (0.6 + 0.4 * curve);
        self.concrete = fc;
        self.joint_phase += speed / JOINT_SPACING_M * dt;
        if self.joint_phase >= 1.0 {
            self.joint_phase = fract(self.joint_phase);
            if fc > 0.0 && speed > 2.0 {
                self.joint = Decay::hit(0.22 * fc * unit(speed / 15.0), 0.025);
                self.joint_osc = 0.0;
            }
        }

        // Wind.
        let v = unit(speed / 45.0);
        self.gust_phase[0] = fract(self.gust_phase[0] + 0.13 * dt);
        self.gust_phase[1] = fract(self.gust_phase[1] + 0.31 * dt);
        let gust =
            1.0 + 0.22 * sin_turns(self.gust_phase[0]) + 0.12 * sin_turns(self.gust_phase[1]);
        self.wind_target = 0.4 * v * v * gust;
        let wind_hz = 200.0 + 9.0 * speed;
        self.wind_l.set(wind_hz, 0.6);
        self.wind_r.set(wind_hz * 1.07, 0.6);

        // Ocean swell.
        self.swell_phase[0] = fract(self.swell_phase[0] + 0.083 * dt);
        self.swell_phase[1] = fract(self.swell_phase[1] + 0.051 * dt);
        let a = sin_turns(self.swell_phase[0]).max(0.0);
        let b = sin_turns(self.swell_phase[1]).max(0.0);
        self.swell = 0.3 + 0.5 * a * a + 0.2 * b;

        // Birds: a call every few seconds while there are birds about.
        if self.bird_notes == 0 {
            self.bird_timer -= dt;
            if self.bird_timer <= 0.0 {
                let amount = p.birds;
                self.bird_timer = self.noise.range(1.5, 6.0) / amount.max(0.1);
                if amount > 0.02 {
                    self.bird_notes = 2 + self.noise.next_u32() % 4;
                    self.bird_t = 0.0;
                    self.bird_dur = self.noise.range(0.05, 0.11);
                    self.bird_gap = self.noise.range(0.03, 0.08);
                    self.bird_f0 = self.noise.range(2600.0, 4200.0);
                    self.bird_sweep = self.noise.range(-900.0, 900.0);
                    self.bird_pan = pan_gains(self.noise.range(-0.9, 0.9));
                    self.bird_level = 0.035 * amount;
                }
            }
        }
    }

    /// One frame: world left and right, and the (mono) horn.
    #[inline]
    fn sample(&mut self) -> [f32; 3] {
        let p = &self.target;
        let w = self.noise.white();
        let w2 = self.noise_r.white();
        let mut l = 0.0;
        let mut r = 0.0;

        // Squeal.
        let sl = self.squeal_l.step(self.squeal_target[0]);
        let sr = self.squeal_r.step(self.squeal_target[1]);
        if sl + sr > 1e-4 {
            let s = (self.squeal_bp.band(w) + 0.35 * self.squeal_bp2.band(w)) * 9.0;
            l += s * (0.75 * sl + 0.35 * sr) * 0.35;
            r += s * (0.35 * sl + 0.75 * sr) * 0.35;
        }

        // Rolling, grass and joints.
        self.brown = self.brown * 0.997 + 0.03 * w;
        let roll = self.roll.step(self.roll_target);
        let rumble = self.roll_lp.low(self.brown) * roll * 2.5;
        l += rumble;
        r += rumble;
        let grass = self.grass.step(self.grass_target);
        if grass > 1e-4 {
            let g = self.grass_bp.band(w2) * grass * 3.0;
            l += g;
            r += g;
        }
        if self.joint.active() {
            let knock = self.joint.step() * sin_turns(self.joint_osc);
            self.joint_osc = fract(self.joint_osc + 95.0 * DT);
            l += knock;
            r += knock;
        }

        // Wind.
        let wind = self.wind.step(self.wind_target);
        if wind > 1e-5 {
            l += self.wind_l.band(w) * wind * 3.0;
            r += self.wind_r.band(w2) * wind * 3.0;
        }

        // Curb thump.
        if self.curb.active() || self.curb_click.active() {
            let c = self.curb.step();
            let thump = c * sin_turns(self.curb_osc);
            self.curb_osc = fract(self.curb_osc + (50.0 + 40.0 * c) * DT);
            let click = self.click_lp.low(w) * self.curb_click.step() * 2.0;
            l += (thump + click) * self.curb_gain[0];
            r += (thump + click) * self.curb_gain[1];
        }

        // Wall hit: a thud and a short, soft crunch.
        if self.hit.active() || self.thud.active() {
            let level = self.hit.step();
            let crackle = if self.noise.unit() < 0.06 { w2 } else { 0.0 };
            let crunch = (self.hit_lp.low(w) * 2.0 + crackle) * level;
            let t = self.thud.step();
            let thud = t * sin_turns(self.hit_osc);
            self.hit_osc = fract(self.hit_osc + (60.0 + 30.0 * t) * DT);
            l += (crunch + thud) * self.hit_gain[0];
            r += (crunch + thud) * self.hit_gain[1];
        }

        // Horn.
        let mut horn = 0.0;
        let want = if p.horn { 1.0 } else { 0.0 };
        let k = if want > self.horn { 0.0035 } else { 0.0005 };
        self.horn += (want - self.horn) * k;
        if self.horn > 1e-4 {
            let mut tone = 0.0;
            for (ph, hz) in self.horn_phase.iter_mut().zip(HORN_HZ) {
                let x = *ph;
                tone += sin_turns(x)
                    + 0.5 * sin_turns(2.0 * x)
                    + 0.33 * sin_turns(3.0 * x)
                    + 0.25 * sin_turns(4.0 * x)
                    + 0.2 * sin_turns(5.0 * x);
                *ph = fract(x + hz * DT);
            }
            let bright = self.horn_bp.band(tone);
            horn = (0.35 * tone + 0.9 * bright) * self.horn * 0.12;
        }

        // Ocean.
        let ocean = self.ocean.step(0.12 * p.ocean);
        if ocean > 1e-5 {
            let g = ocean * self.swell * 4.0;
            l += self.ocean_l.low(w2) * g;
            r += self.ocean_r.low(w) * g;
        }

        // Bird call.
        if self.bird_notes > 0 {
            if self.bird_t < self.bird_dur {
                let x = self.bird_t / self.bird_dur;
                let env = sin_turns(0.5 * x);
                let hz = self.bird_f0 + self.bird_sweep * x + 150.0 * sin_turns(25.0 * self.bird_t);
                self.bird_phase = fract(self.bird_phase + hz * DT);
                let b = sin_turns(self.bird_phase) * env * self.bird_level;
                l += b * self.bird_pan[0];
                r += b * self.bird_pan[1];
            }
            self.bird_t += DT;
            if self.bird_t >= self.bird_dur + self.bird_gap {
                self.bird_notes -= 1;
                self.bird_t = 0.0;
                self.bird_f0 *= self.noise.range(0.9, 1.12);
            }
        }

        if l.is_finite() && r.is_finite() && horn.is_finite() {
            [l, r, horn]
        } else {
            self.reset_filters();
            [0.0; 3]
        }
    }

    fn reset_filters(&mut self) {
        for f in [
            &mut self.squeal_bp,
            &mut self.squeal_bp2,
            &mut self.roll_lp,
            &mut self.grass_bp,
            &mut self.wind_l,
            &mut self.wind_r,
            &mut self.click_lp,
            &mut self.hit_lp,
            &mut self.horn_bp,
            &mut self.ocean_l,
            &mut self.ocean_r,
        ] {
            f.reset();
        }
        self.brown = 0.0;
    }
}

/// Left and right gains of a bump on `side` of the car (`[-1, 1]`, left to right): a
/// lean, not a pan, so a bump square on the nose plays as it always did.
fn side_gains(side: f32) -> [f32; 2] {
    [1.0 - 0.35 * side, 1.0 + 0.35 * side]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::WheelParams;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// World and horn summed.
    fn render(p: &AudioParams, frames: usize) -> Vec<f32> {
        let mut w = WorldSynth::new();
        w.set_params(p);
        let mut out = vec![0.0; frames * 2];
        let mut ui = vec![0.0; frames * 2];
        w.render(&mut out, &mut ui);
        out.iter_mut().zip(&ui).for_each(|(o, u)| *o += u);
        out
    }

    fn render_into(w: &mut WorldSynth, out: &mut [f32]) {
        let mut ui = vec![0.0; out.len()];
        w.render(out, &mut ui);
    }

    #[test]
    fn standing_still_is_quiet() {
        let x = render(&AudioParams::default(), 9600);
        assert!(rms(&x) < 1e-3, "{}", rms(&x));
    }

    #[test]
    fn wind_and_road_grow_with_speed() {
        let at = |v: f32| {
            let mut p = AudioParams {
                speed_mps: v,
                ..AudioParams::default()
            };
            for w in &mut p.wheels {
                w.speed_mps = v;
            }
            rms(&render(&p, 24_000)[24_000..])
        };
        let (slow, fast) = (at(5.0), at(40.0));
        assert!(fast > 3.0 * slow && slow > 0.0, "{slow} {fast}");
    }

    #[test]
    fn sliding_tires_squeal_on_their_side() {
        // Spinning on the spot: no wind or road noise, only the tires.
        let mut p = AudioParams::default();
        let slide = WheelParams {
            slip: 2.0,
            load: 1.2,
            speed_mps: 15.0,
            grounded: true,
            surface: Surface::Asphalt,
        };
        let grip = WheelParams { slip: 0.3, ..slide };
        p.wheels = [slide, grip, slide, grip];
        let x = render(&p, 24_000);
        let left: Vec<f32> = x[24_000..].iter().step_by(2).copied().collect();
        let right: Vec<f32> = x[24_001..].iter().step_by(2).copied().collect();
        let mut q = p;
        q.wheels = [grip; 4];
        let calm = render(&q, 24_000);
        assert!(
            rms(&x) > 0.05 && rms(&calm) < 1e-3,
            "{} {}",
            rms(&x),
            rms(&calm)
        );
        assert!(rms(&left) > 1.3 * rms(&right));
        // On grass the same slide does not squeal.
        p.speed_mps = 15.0;
        for w in &mut p.wheels {
            w.surface = Surface::Grass;
        }
        let mut q = p;
        for w in &mut q.wheels {
            w.slip = 0.3;
        }
        let grass_slide = rms(&render(&p, 24_000)[24_000..]);
        let grass_roll = rms(&render(&q, 24_000)[24_000..]);
        assert!(
            (grass_slide - grass_roll).abs() < 0.1 * grass_roll,
            "{grass_slide} {grass_roll}"
        );
    }

    #[test]
    fn events_fire_once_per_count() {
        let mut w = WorldSynth::new();
        let mut p = AudioParams::default();
        w.set_params(&p);
        let mut buf = vec![0.0; 4800];
        render_into(&mut w, &mut buf);
        assert!(rms(&buf) < 1e-3);
        p.curb_kicks = 1;
        p.hits = 1;
        p.hit_speed_mps = 8.0;
        w.set_params(&p);
        render_into(&mut w, &mut buf);
        assert!(rms(&buf) > 0.02);
        // The same counts again: nothing new.
        render_into(&mut w, &mut vec![0.0; 48_000]);
        w.set_params(&p);
        render_into(&mut w, &mut buf);
        assert!(rms(&buf) < 1e-3, "{}", rms(&buf));
    }

    #[test]
    fn bumps_scale_with_speed_and_lean_to_their_side() {
        // One curb kick at `speed` on `side`: the RMS of each ear over its thump.
        let bump = |speed: f32, side: f32| {
            let mut w = WorldSynth::new();
            let mut p = AudioParams::default();
            w.set_params(&p);
            p.curb_kicks = 1;
            p.curb_speed_mps = speed;
            p.curb_side = side;
            w.set_params(&p);
            let mut buf = vec![0.0; 9600];
            render_into(&mut w, &mut buf);
            let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
            let r: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
            [rms(&l), rms(&r)]
        };
        let soft = bump(14.0, 0.0);
        let hard = bump(25.0, 0.0);
        assert!(hard[0] > 1.2 * soft[0], "{soft:?} {hard:?}");
        assert_eq!(soft[0], soft[1]);
        let left = bump(20.0, -1.0);
        assert!(left[0] > 1.5 * left[1], "{left:?}");
    }

    #[test]
    fn the_horn_has_two_tones_and_ambience_is_subtle() {
        let p = AudioParams {
            horn: true,
            ..AudioParams::default()
        };
        let x = render(&p, 24_000);
        assert!(rms(&x[9600..]) > 0.03);
        let q = AudioParams {
            ocean: 1.0,
            birds: 1.0,
            ..AudioParams::default()
        };
        let a = rms(&render(&q, 480_000));
        assert!(a > 0.003 && a < 0.08, "{a}");
    }
}
