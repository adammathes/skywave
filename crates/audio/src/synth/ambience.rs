//! The place's life, beyond the ocean and the birds ([`super::world`]): what a street
//! sounds like with the engine off. Each part plays by its amount in [`AmbienceParams`],
//! which the app makes from the zones round the car, the clock and the world. All of it
//! is quiet: a bed under the car, never over it.
//!
//! - **City**: a far-off traffic rumble (low-passed noise, a little different in each ear
//!   so it is wide, slowly breathing), with now and then one far car passing across the
//!   stereo image (a swell of band-passed noise, its pitch falling as it passes), and,
//!   where it is dense, a far horn: one or two short two-tone beeps, dull with distance,
//!   echoing off the buildings.
//! - **Freeway**: the steady hiss of tires on a freeway, duller the further it is, with a
//!   truck's low rumble under it and the traffic's slow surges.
//! - **Trees**: wind in leaves, gusting: band-passed noise that swells and fades every few
//!   seconds, brighter in a gust, fluttering as the leaves do.
//! - **Crickets**: three crickets, each its own pitch (4–5 kHz), pace and place in the
//!   stereo image, chirping in pulse trains, over a faint far-off trill.
//! - **Industry**: a transformer's 120 Hz hum and its harmonics, a machine's slow chug,
//!   and now and then a far metal clank (three inharmonic partials) echoing round the yard.
//! - **Foghorn**: a diaphone across the water every half minute: a low, reedy two-tone
//!   blast, the second tone lower ("bee-oh"), dull with distance and echoing.
//!
//! The horn, the clank and the foghorn share one stereo echo (two delay lines a fifth
//! of a second apart, low-passed in the loop): the city's walls and the water answering.

use super::dsp::{fract, pan_gains, sin_turns, unit, Decay, Noise, Slew, Svf, DT};

/// How much of each part to play, `[0, 1]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AmbienceParams {
    /// Downtown, the strip and other busy districts round the car: the far traffic and
    /// horns.
    pub city: f32,
    /// How near a freeway is (1 beside it).
    pub freeway: f32,
    /// Trees round the car: parks, hills, leafy streets.
    pub trees: f32,
    /// Crickets: yards, parks and fields at night.
    pub crickets: f32,
    /// Industrial districts round the car.
    pub industry: f32,
    /// Open water near enough to hear a foghorn across it (worlds that have one).
    pub foghorn: f32,
}

impl AmbienceParams {
    /// Every amount in `[0, 1]` (non-finite ones 0).
    pub fn sanitized(&self) -> AmbienceParams {
        AmbienceParams {
            city: unit(self.city),
            freeway: unit(self.freeway),
            trees: unit(self.trees),
            crickets: unit(self.crickets),
            industry: unit(self.industry),
            foghorn: unit(self.foghorn),
        }
    }
}

/// Echo delays, samples (113 ms and 167 ms), and the feedback.
const ECHO: [usize; 2] = [5424, 8016];
const ECHO_FEEDBACK: f32 = 0.42;
/// The foghorn's tones, Hz, and how long each sounds, s.
const FOGHORN_HZ: [f32; 2] = [174.0, 131.0];
const FOGHORN_S: [f32; 2] = [1.9, 1.1];
/// Seconds between foghorn blasts.
const FOGHORN_EVERY_S: f32 = 27.0;
/// A clank's partials, as ratios of its lowest.
const CLANK_RATIOS: [f32; 3] = [1.0, 2.76, 5.40];

#[derive(Clone, Copy, Debug)]
struct Cricket {
    hz: f32,
    phase: f32,
    /// Seconds into the current chirp, and between chirps.
    t: f32,
    period: f32,
    /// Pulses per chirp, and pulses per second within one.
    pulses: f32,
    rate: f32,
    gain: [f32; 2],
}

/// The ambience's state.
#[derive(Clone, Debug)]
pub struct AmbienceSynth {
    target: AmbienceParams,
    noise: Noise,

    city: Slew,
    city_l: Svf,
    city_r: Svf,
    breath: f32,
    breath_target: f32,
    pass_timer: f32,
    pass_t: f32,
    pass_dur: f32,
    pass_level: f32,
    pass_pan: [f32; 2],
    pass_hz: f32,
    pass_bp: Svf,

    horn_timer: f32,
    horn_t: f32,
    horn_dur: f32,
    horn_beeps: u32,
    horn_hz: [f32; 2],
    horn_phase: [f32; 2],
    horn_level: f32,
    horn_pan: [f32; 2],
    horn_lp: Svf,

    freeway: Slew,
    freeway_l: Svf,
    freeway_r: Svf,
    freeway_lp_l: Svf,
    freeway_lp_r: Svf,
    surge: f32,
    surge_target: f32,
    rumble_bp: Svf,

    trees: Slew,
    gust: f32,
    gust_target: f32,
    leaves_l: Svf,
    leaves_r: Svf,
    flutter: [f32; 2],

    crickets: Slew,
    cricket: [Cricket; 3],
    trill_phase: f32,
    trill_am: f32,

    industry: Slew,
    hum_phase: f32,
    chug_phase: f32,
    chug_bp: Svf,
    clank_timer: f32,
    clank: Decay,
    clank_hz: f32,
    clank_phase: [f32; 3],
    clank_pan: [f32; 2],

    fog_timer: f32,
    fog_t: f32,
    fog_level: f32,
    fog_phase: f32,
    fog_lp: Svf,

    echo: [Vec<f32>; 2],
    echo_at: usize,
    echo_lp: [f32; 2],
}

impl Default for AmbienceSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl AmbienceSynth {
    pub fn new() -> AmbienceSynth {
        let mut noise = Noise::new(0x51ab_7e11);
        let mut cricket = |hz: f32, pan: f32| Cricket {
            hz,
            phase: 0.0,
            t: noise.range(0.0, 0.6),
            period: noise.range(0.42, 0.75),
            pulses: (3 + noise.next_u32() % 3) as f32,
            rate: noise.range(28.0, 40.0),
            gain: {
                let g = pan_gains(pan);
                let level = noise.range(0.6, 1.0);
                [g[0] * level, g[1] * level]
            },
        };
        let cricket = [
            cricket(4480.0, -0.7),
            cricket(4930.0, 0.55),
            cricket(4190.0, 0.1),
        ];
        AmbienceSynth {
            target: AmbienceParams::default(),
            noise,
            city: Slew::new(0.0, 1.0),
            city_l: Svf::new(260.0, 0.55),
            city_r: Svf::new(290.0, 0.55),
            breath: 1.0,
            breath_target: 1.0,
            pass_timer: 2.0,
            pass_t: 0.0,
            pass_dur: 0.0,
            pass_level: 0.0,
            pass_pan: [0.0; 2],
            pass_hz: 600.0,
            pass_bp: Svf::new(600.0, 0.9),
            horn_timer: 9.0,
            horn_t: 0.0,
            horn_dur: 0.0,
            horn_beeps: 0,
            horn_hz: [400.0, 480.0],
            horn_phase: [0.0; 2],
            horn_level: 0.0,
            horn_pan: [0.7; 2],
            horn_lp: Svf::new(1100.0, 0.7),
            freeway: Slew::new(0.0, 1.0),
            freeway_l: Svf::new(750.0, 0.6),
            freeway_r: Svf::new(820.0, 0.6),
            freeway_lp_l: Svf::new(2000.0, 0.6),
            freeway_lp_r: Svf::new(2000.0, 0.6),
            surge: 1.0,
            surge_target: 1.0,
            rumble_bp: Svf::new(90.0, 0.8),
            trees: Slew::new(0.0, 1.0),
            gust: 0.0,
            gust_target: 0.0,
            leaves_l: Svf::new(2500.0, 0.5),
            leaves_r: Svf::new(2700.0, 0.5),
            flutter: [0.0; 2],
            crickets: Slew::new(0.0, 1.5),
            cricket,
            trill_phase: 0.0,
            trill_am: 0.0,
            industry: Slew::new(0.0, 1.0),
            hum_phase: 0.0,
            chug_phase: 0.0,
            chug_bp: Svf::new(60.0, 1.2),
            clank_timer: 3.0,
            clank: Decay::SILENT,
            clank_hz: 500.0,
            clank_phase: [0.0; 3],
            clank_pan: [0.7; 2],
            fog_timer: 4.0,
            fog_t: f32::INFINITY,
            fog_level: 0.0,
            fog_phase: 0.0,
            fog_lp: Svf::new(700.0, 0.7),
            echo: [vec![0.0; ECHO[0]], vec![0.0; ECHO[1]]],
            echo_at: 0,
            echo_lp: [0.0; 2],
        }
    }

    pub fn set_params(&mut self, p: &AmbienceParams) {
        self.target = p.sanitized();
    }

    /// The slow part, every `dt` seconds (a few hundred times a second): drifts, timers
    /// and the events they start.
    pub fn tick(&mut self, dt: f32) {
        let p = self.target;
        let n = &mut self.noise;

        // The city breathes: a new level every second or so, eased toward.
        if n.unit() < dt {
            self.breath_target = n.range(0.45, 1.3);
        }
        self.breath += (self.breath_target - self.breath) * dt;
        // A far car passing, every few seconds.
        if self.pass_t >= self.pass_dur {
            self.pass_timer -= dt;
            if self.pass_timer <= 0.0 {
                self.pass_timer = n.range(1.5, 6.0) / p.city.max(0.2);
                if p.city > 0.05 {
                    self.pass_t = 0.0;
                    self.pass_dur = n.range(2.5, 5.0);
                    self.pass_level = n.range(0.5, 1.0);
                    let from = n.range(-0.9, 0.9);
                    self.pass_pan = [from, -from * n.range(0.3, 1.0)];
                    self.pass_hz = n.range(450.0, 800.0);
                }
            }
        } else {
            self.pass_t += dt;
            let x = self.pass_t / self.pass_dur;
            // The Doppler fall, a tenth of the pitch, as it goes by.
            self.pass_bp.set(self.pass_hz * (1.05 - 0.1 * x), 0.9);
        }
        // A far horn where it is busy.
        if self.horn_beeps == 0 {
            self.horn_timer -= dt;
            if self.horn_timer <= 0.0 {
                self.horn_timer = n.range(8.0, 22.0) / p.city.max(0.1);
                if p.city > 0.35 {
                    self.horn_beeps = if n.unit() < 0.35 { 2 } else { 1 };
                    self.horn_t = 0.0;
                    self.horn_dur = n.range(0.12, 0.32);
                    let pitch = n.range(0.8, 1.25);
                    self.horn_hz = [400.0 * pitch, 400.0 * pitch * n.range(1.18, 1.26)];
                    self.horn_level = n.range(0.5, 1.0) * p.city;
                    self.horn_pan = pan_gains(n.range(-0.9, 0.9));
                }
            }
        }

        // Freeway surges, a little every second or two.
        if n.unit() < 0.7 * dt {
            self.surge_target = n.range(0.75, 1.2);
        }
        self.surge += (self.surge_target - self.surge) * 0.8 * dt;
        let near = self.freeway.value;
        let hz = 600.0 + 2400.0 * near * near;
        self.freeway_lp_l.set(hz, 0.6);
        self.freeway_lp_r.set(hz * 1.08, 0.6);

        // Gusts in the trees: a new strength every couple of seconds, often calm.
        if n.unit() < 0.45 * dt {
            let g = n.unit();
            self.gust_target = g * g;
        }
        self.gust += (self.gust_target - self.gust) * 0.7 * dt;
        let bright = 2000.0 + 2500.0 * self.gust;
        self.leaves_l.set(bright, 0.5);
        self.leaves_r.set(bright * 1.1, 0.5);

        // A clank in the yard now and then.
        self.clank_timer -= dt;
        if self.clank_timer <= 0.0 {
            self.clank_timer = n.range(3.0, 11.0);
            if p.industry > 0.1 {
                self.clank = Decay::hit(n.range(0.4, 1.0) * p.industry, n.range(0.12, 0.3));
                self.clank_hz = n.range(280.0, 900.0);
                self.clank_phase = [0.0; 3];
                self.clank_pan = pan_gains(n.range(-0.8, 0.8));
            }
        }

        // The foghorn keeps its own time whether or not it is heard, as a real one does.
        self.fog_timer -= dt;
        if self.fog_timer <= 0.0 {
            self.fog_timer = FOGHORN_EVERY_S;
            self.fog_t = 0.0;
        }
        self.fog_level = p.foghorn;
    }

    /// The next frame, left and right.
    #[inline]
    pub fn sample(&mut self) -> [f32; 2] {
        let p = self.target;
        let wl = self.noise.white();
        let wr = self.noise.white();
        let mut l = 0.0;
        let mut r = 0.0;
        let mut send = [0.0f32; 2];

        // City.
        let city = self.city.step(p.city);
        if city > 1e-4 {
            let g = 0.15 * city * self.breath;
            l += self.city_l.low(wl) * g;
            r += self.city_r.low(wr) * g;
            if self.pass_t < self.pass_dur {
                let x = self.pass_t / self.pass_dur;
                let env = sin_turns(0.5 * x);
                let car = self.pass_bp.band(0.5 * (wl + wr)) * env * env * self.pass_level;
                let [gl, gr] =
                    pan_gains(self.pass_pan[0] + (self.pass_pan[1] - self.pass_pan[0]) * x);
                let g = 0.2 * city;
                l += car * gl * g;
                r += car * gr * g;
            }
        }
        if self.horn_beeps > 0 {
            let t = self.horn_t;
            if t < self.horn_dur {
                let env = unit(t / 0.015).min(unit((self.horn_dur - t) / 0.03));
                let mut tone = 0.0;
                for (ph, hz) in self.horn_phase.iter_mut().zip(self.horn_hz) {
                    let x = *ph;
                    tone += sin_turns(x) + 0.5 * sin_turns(2.0 * x) + 0.3 * sin_turns(3.0 * x);
                    *ph = fract(x + hz * DT);
                }
                let h = self.horn_lp.low(tone) * env * self.horn_level * 0.022;
                send[0] += h * self.horn_pan[0];
                send[1] += h * self.horn_pan[1];
            }
            self.horn_t += DT;
            if self.horn_t >= self.horn_dur + 0.12 {
                self.horn_beeps -= 1;
                self.horn_t = 0.0;
            }
        }

        // Freeway.
        let freeway = self.freeway.step(p.freeway);
        if freeway > 1e-4 {
            let g = 0.16 * freeway * self.surge;
            let hl = self.freeway_lp_l.low(self.freeway_l.band(wl));
            let hr = self.freeway_lp_r.low(self.freeway_r.band(wr));
            let rumble = self.rumble_bp.band(wl + wr) * 0.5;
            l += (hl + rumble) * g;
            r += (hr + rumble) * g;
        }

        // Trees.
        let trees = self.trees.step(p.trees);
        if trees > 1e-4 {
            // The flutter: each ear's leaves come and go at a few hertz.
            // (Noise low-passed at ~9 Hz, its size around 0.011.)
            self.flutter[0] += (wl - self.flutter[0]) * 0.0012;
            self.flutter[1] += (wr - self.flutter[1]) * 0.0012;
            let g = 0.06 * trees * (0.08 + self.gust);
            l += self.leaves_l.band(wr) * g * (0.5 + 25.0 * self.flutter[0].abs());
            r += self.leaves_r.band(wl) * g * (0.5 + 25.0 * self.flutter[1].abs());
        }

        // Crickets.
        let crickets = self.crickets.step(p.crickets);
        if crickets > 1e-4 {
            for c in &mut self.cricket {
                c.t += DT;
                if c.t >= c.period {
                    c.t -= c.period;
                }
                let k = c.t * c.rate;
                if k < c.pulses {
                    // Each pulse is the first 60% of its slot, a sine bump.
                    let x = fract(k) / 0.6;
                    if x < 1.0 {
                        let env = sin_turns(0.5 * x);
                        let s = sin_turns(c.phase) * env * crickets * 0.03;
                        l += s * c.gain[0];
                        r += s * c.gain[1];
                    }
                }
                c.phase = fract(c.phase + c.hz * DT);
            }
            // The far trill: a tone at 3.1 kHz pulsing 52 times a second, barely there.
            self.trill_am = fract(self.trill_am + 52.0 * DT);
            self.trill_phase = fract(self.trill_phase + 3100.0 * DT);
            let am = sin_turns(0.5 * self.trill_am);
            let s = sin_turns(self.trill_phase) * am * am * crickets * 0.006;
            l += s;
            r += s * 0.8;
        }

        // Industry.
        let industry = self.industry.step(p.industry);
        if industry > 1e-4 {
            self.hum_phase = fract(self.hum_phase + 120.0 * DT);
            let x = self.hum_phase;
            let hum = sin_turns(x) + 0.6 * sin_turns(2.0 * x) + 0.35 * sin_turns(3.0 * x);
            self.chug_phase = fract(self.chug_phase + 1.7 * DT);
            let chug = sin_turns(self.chug_phase).max(0.0);
            let rumble = self.chug_bp.band(wl - wr) * chug * 0.3;
            let s = (hum * 0.006 + rumble) * industry;
            l += s;
            r += s;
        }
        if self.clank.active() {
            let e = self.clank.step();
            let mut tone = 0.0;
            for (k, (ph, ratio)) in self.clank_phase.iter_mut().zip(CLANK_RATIOS).enumerate() {
                tone += sin_turns(*ph) / (1.0 + k as f32);
                *ph = fract(*ph + self.clank_hz * ratio * DT);
            }
            let c = tone * e * 0.03;
            send[0] += c * self.clank_pan[0];
            send[1] += c * self.clank_pan[1];
        }

        // Foghorn.
        if self.fog_t < FOGHORN_S[0] + FOGHORN_S[1] + 0.5 {
            let t = self.fog_t;
            self.fog_t += DT;
            if self.fog_level > 1e-3 {
                let (hz, env) = if t < FOGHORN_S[0] {
                    (FOGHORN_HZ[0], unit(t / 0.25))
                } else {
                    // A short glide down into the second, softer tone (the grunt), which
                    // fades out.
                    let u = t - FOGHORN_S[0];
                    let glide = unit(u / 0.08);
                    let env = (1.0 - 0.15 * glide) * unit((FOGHORN_S[1] - u) / 0.5);
                    (FOGHORN_HZ[0] + (FOGHORN_HZ[1] - FOGHORN_HZ[0]) * glide, env)
                };
                self.fog_phase = fract(self.fog_phase + hz * DT);
                // Reedy: a band-limited sawtooth's first eight partials.
                let x = self.fog_phase;
                let mut saw = 0.0;
                for k in 1..=8 {
                    saw += sin_turns(k as f32 * x) / k as f32;
                }
                let f = self.fog_lp.low(saw) * env * self.fog_level * 0.05;
                send[0] += f;
                send[1] += f;
            }
        }

        // The echo: what was sent plays dry and into the delay lines, which feed back
        // crossed (left into right), low-passed.
        let i = self.echo_at;
        let [el, er] = [self.echo[0][i % ECHO[0]], self.echo[1][i % ECHO[1]]];
        self.echo_lp[0] += (el - self.echo_lp[0]) * 0.35;
        self.echo_lp[1] += (er - self.echo_lp[1]) * 0.35;
        self.echo[0][i % ECHO[0]] = send[0] + self.echo_lp[1] * ECHO_FEEDBACK;
        self.echo[1][i % ECHO[1]] = send[1] + self.echo_lp[0] * ECHO_FEEDBACK;
        self.echo_at = (i + 1) % (ECHO[0] * ECHO[1]);
        l += send[0] + 0.6 * el;
        r += send[1] + 0.6 * er;

        [l, r]
    }

    /// Clears the filters and the echo (after a non-finite sample).
    pub fn reset(&mut self) {
        for f in [
            &mut self.city_l,
            &mut self.city_r,
            &mut self.pass_bp,
            &mut self.horn_lp,
            &mut self.freeway_l,
            &mut self.freeway_r,
            &mut self.freeway_lp_l,
            &mut self.freeway_lp_r,
            &mut self.rumble_bp,
            &mut self.leaves_l,
            &mut self.leaves_r,
            &mut self.chug_bp,
            &mut self.fog_lp,
        ] {
            f.reset();
        }
        for e in &mut self.echo {
            e.iter_mut().for_each(|v| *v = 0.0);
        }
        self.echo_lp = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(p: AmbienceParams, seconds: f32) -> Vec<f32> {
        let mut a = AmbienceSynth::new();
        a.set_params(&p);
        let mut out = Vec::new();
        for i in 0..(seconds * 48_000.0) as usize {
            if i % 32 == 0 {
                a.tick(32.0 * DT);
            }
            out.extend(a.sample());
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn nothing_plays_without_a_place() {
        let x = render(AmbienceParams::default(), 40.0);
        assert!(x.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn every_part_is_heard_and_stays_a_bed() {
        let none = AmbienceParams::default();
        for (name, p) in [
            ("city", AmbienceParams { city: 1.0, ..none }),
            (
                "freeway",
                AmbienceParams {
                    freeway: 1.0,
                    ..none
                },
            ),
            ("trees", AmbienceParams { trees: 1.0, ..none }),
            (
                "crickets",
                AmbienceParams {
                    crickets: 1.0,
                    ..none
                },
            ),
            (
                "industry",
                AmbienceParams {
                    industry: 1.0,
                    ..none
                },
            ),
            (
                "foghorn",
                AmbienceParams {
                    foghorn: 1.0,
                    ..none
                },
            ),
        ] {
            let x = render(p, 30.0);
            let (r, peak) = (rms(&x), x.iter().fold(0.0f32, |a, v| a.max(v.abs())));
            assert!(
                r > 0.002 && r < 0.05 && peak < 0.3,
                "{name}: rms {r} peak {peak}"
            );
        }
    }

    #[test]
    fn non_finite_amounts_are_silent() {
        let p = AmbienceParams {
            city: f32::NAN,
            freeway: f32::INFINITY,
            ..AmbienceParams::default()
        };
        let s = p.sanitized();
        assert_eq!((s.city, s.freeway), (0.0, 1.0));
    }
}
