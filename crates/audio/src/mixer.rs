//! The mixer (`docs/AUDIO_RADIO.md`, "Mixer"): 48 kHz interleaved stereo `f32`, rendered in
//! fixed blocks of [`BLOCK_FRAMES`].
//!
//! Each block the synths fill three buses: [`Bus::Engine`], [`Bus::World`] (tires, road,
//! wind, knocks, ambience) and [`Bus::Ui`] (the horn, and later menu sounds). Every bus has
//! a gain (the engine and world volumes, ducked under the radio by [`duck_gain`] and faded
//! out while paused; the ui bus is fixed), then the master volume and a DC blocker, then a
//! soft limiter:
//! a peak follower with instant attack and a 150 ms release whose gain curve bends from
//! unity at [`KNEE`] toward [`CEILING`], which only an infinite input would reach. Gains move once per block
//! and ramp linearly across it, so volume changes never click.
//!
//! [`Mixer::render`] takes any even-length buffer (the platform's callback size) and
//! allocates nothing: blocks are rendered into fixed arrays and copied out.

use crate::synth::dsp::{exp, tau_coef, unit, DcBlock};
use crate::synth::{AudioParams, EngineSynth, WorldSynth};

/// Frames per second.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Frames per block.
pub const BLOCK_FRAMES: usize = 256;
/// Interleaved samples per block.
pub const BLOCK_SAMPLES: usize = BLOCK_FRAMES * CHANNELS;
/// The limiter leaves everything below this alone...
pub const KNEE: f32 = 0.6;
/// ...and never lets a sample past this (-0.5 dBFS).
pub const CEILING: f32 = 0.944;
/// Limiter release, s.
const RELEASE_S: f32 = 0.15;
/// Share of the way to its target a bus gain moves per block (≈ 30 ms time constant).
const GAIN_STEP: f32 = 0.16;
/// How far the engine and world duck under a full-volume radio (0.45: to 55%, -5 dB).
pub const DUCK_DEPTH: f32 = 0.45;

/// A mixer bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Engine = 0,
    World = 1,
    Ui = 2,
}

pub const BUS_COUNT: usize = 3;

/// Gain of the engine and world buses with the radio at `radio` (`[0, 1]`, its volume
/// times whether it plays): an ease-out from 1 to `1 - DUCK_DEPTH`, so the first bit of
/// radio already makes room.
pub fn duck_gain(radio: f32) -> f32 {
    let d = unit(radio);
    1.0 - DUCK_DEPTH * d * (2.0 - d)
}

/// A volume setting in `[0, 1]` as an amplitude: squared, so the slider feels even.
pub fn volume_gain(v: f32) -> f32 {
    let v = unit(v);
    v * v
}

/// Counters for tests and the debug overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MixerStats {
    pub blocks: u64,
    /// Samples the limiter turned down.
    pub limited: u64,
    /// Largest output sample magnitude so far.
    pub peak: f32,
    /// Non-finite samples replaced with silence (should stay 0).
    pub nonfinite: u64,
}

/// The whole sound: synths, buses, gains and limiter.
#[derive(Clone, Debug)]
pub struct Mixer {
    params: AudioParams,
    engine: EngineSynth,
    world: WorldSynth,
    buses: [[f32; BLOCK_SAMPLES]; BUS_COUNT],
    mono: [f32; BLOCK_FRAMES],
    gains: [f32; BUS_COUNT],
    master: f32,
    dc: [DcBlock; CHANNELS],
    env: f32,
    release: f32,
    block: [f32; BLOCK_SAMPLES],
    /// Next sample of `block` to hand out.
    read: usize,
    stats: MixerStats,
}

impl Default for Mixer {
    fn default() -> Self {
        Mixer::new()
    }
}

impl Mixer {
    pub fn new() -> Mixer {
        Mixer {
            params: AudioParams::default(),
            engine: EngineSynth::new(),
            world: WorldSynth::new(),
            buses: [[0.0; BLOCK_SAMPLES]; BUS_COUNT],
            mono: [0.0; BLOCK_FRAMES],
            gains: [0.0; BUS_COUNT],
            master: 0.0,
            dc: [DcBlock::default(); CHANNELS],
            env: 0.0,
            release: 1.0 - tau_coef(RELEASE_S),
            block: [0.0; BLOCK_SAMPLES],
            read: BLOCK_SAMPLES,
            stats: MixerStats::default(),
        }
    }

    /// This frame's parameters. Sanitized here, so anything goes.
    pub fn set_params(&mut self, p: &AudioParams) {
        let p = p.sanitized();
        self.engine.set_params(&p.engine);
        self.world.set_params(&p);
        self.params = p;
    }

    pub fn params(&self) -> &AudioParams {
        &self.params
    }

    pub fn stats(&self) -> MixerStats {
        self.stats
    }

    /// Fills `out` with interleaved stereo frames (an odd trailing sample gets the next
    /// left one). Allocates nothing.
    pub fn render(&mut self, out: &mut [f32]) {
        let mut i = 0;
        while i < out.len() {
            if self.read >= BLOCK_SAMPLES {
                self.fill_buses();
                self.mix_down();
                self.read = 0;
            }
            let n = (BLOCK_SAMPLES - self.read).min(out.len() - i);
            out[i..i + n].copy_from_slice(&self.block[self.read..self.read + n]);
            self.read += n;
            i += n;
        }
    }

    /// Where each bus's gain is heading, and the master's.
    fn targets(&self) -> ([f32; BUS_COUNT], f32) {
        let p = &self.params;
        let live = if p.paused { 0.0 } else { duck_gain(p.radio) };
        let buses = [
            volume_gain(p.engine_volume) * live,
            volume_gain(p.world_volume) * live,
            1.0,
        ];
        let master = if p.muted { 0.0 } else { volume_gain(p.master) };
        (buses, master)
    }

    fn fill_buses(&mut self) {
        self.engine.render(&mut self.mono);
        let [engine, world, ui] = &mut self.buses;
        for (f, &m) in engine.chunks_exact_mut(2).zip(&self.mono) {
            f[0] = m;
            f[1] = m;
        }
        self.world.render(world, ui);
    }

    /// Sums the buses into `block` with ramped gains, then limits.
    fn mix_down(&mut self) {
        let (targets, master_target) = self.targets();
        let from = self.gains;
        let mut to = [0.0; BUS_COUNT];
        for b in 0..BUS_COUNT {
            to[b] = from[b] + (targets[b] - from[b]) * GAIN_STEP;
        }
        let m_from = self.master;
        let m_to = m_from + (master_target - m_from) * GAIN_STEP;
        let inv = 1.0 / BLOCK_FRAMES as f32;
        for j in 0..BLOCK_FRAMES {
            let t = (j + 1) as f32 * inv;
            let mut l = 0.0;
            let mut r = 0.0;
            for b in 0..BUS_COUNT {
                let g = from[b] + (to[b] - from[b]) * t;
                l += self.buses[b][2 * j] * g;
                r += self.buses[b][2 * j + 1] * g;
            }
            let m = m_from + (m_to - m_from) * t;
            let l = self.dc[0].step(l * m);
            let r = self.dc[1].step(r * m);
            let [l, r] = self.limit(l, r);
            self.block[2 * j] = l;
            self.block[2 * j + 1] = r;
        }
        self.gains = to;
        self.master = m_to;
        self.stats.blocks += 1;
    }

    /// One frame through the limiter.
    #[inline]
    fn limit(&mut self, l: f32, r: f32) -> [f32; 2] {
        if !(l.is_finite() && r.is_finite()) {
            self.stats.nonfinite += 1;
            return [0.0, 0.0];
        }
        let peak = l.abs().max(r.abs());
        self.env = if peak > self.env {
            peak
        } else {
            self.env * self.release
        };
        let mut gain = 1.0;
        if self.env > KNEE {
            // Output level for this envelope: KNEE + room × (1 - e^(-over / room)), which
            // leaves the knee with slope 1 and tends to the ceiling.
            let room = CEILING - KNEE;
            let level = KNEE + room * (1.0 - exp(-(self.env - KNEE) / room));
            gain = level / self.env;
            if peak > KNEE {
                self.stats.limited += 1;
            }
        }
        let out = [l * gain, r * gain];
        self.stats.peak = self.stats.peak.max(out[0].abs()).max(out[1].abs());
        out
    }
}

#[cfg(test)]
pub(crate) mod script {
    //! A fixed drive for tests: parameters at 60 frames a second.

    use crate::synth::{AudioParams, EngineProfile, Surface, WheelParams};

    /// Parameters `frame` sixtieths of a second into a looping 12 s drive: idle, a pull
    /// through the gears to the limiter, a lift with pops, a slide with squeal, a curb, a
    /// wall tap, the horn, and the beach with birds, on every surface.
    pub fn at(frame: u32) -> AudioParams {
        let t = (frame % 720) as f32 / 60.0;
        let mut p = AudioParams::default();
        p.engine.profile = EngineProfile::ALL[(frame / 720) as usize % EngineProfile::ALL.len()];
        p.engine.idle_rpm = 900.0;
        p.engine.redline_rpm = 7200.0;
        let (rpm, throttle, gear) = if t < 1.0 {
            (900.0, 0.0, 1)
        } else if t < 6.0 {
            // Up through three gears, the last one into the limiter.
            let u = (t - 1.0) / 5.0;
            let gear = 1 + (u * 3.0).min(2.0) as i8;
            let within = (u * 3.0).fract();
            let rpm = if gear == 3 {
                4000.0 + 3300.0 * within
            } else {
                3000.0 + 3800.0 * within
            };
            (rpm, 1.0, gear)
        } else if t < 8.0 {
            (6500.0 - 1500.0 * (t - 6.0), 0.0, 3)
        } else {
            (2500.0, 0.4, 2)
        };
        p.engine.rpm = rpm;
        p.engine.throttle = throttle;
        p.engine.gear = gear;
        p.speed_mps = 4.0 * t;
        let surface = [Surface::Asphalt, Surface::Concrete, Surface::Grass][(t / 4.0) as usize % 3];
        let slip = if (8.0..10.0).contains(&t) { 2.0 } else { 0.4 };
        p.wheels = [WheelParams {
            slip,
            load: 1.1,
            speed_mps: p.speed_mps,
            grounded: true,
            surface,
        }; 4];
        p.curb_kicks = frame / 150;
        p.curb_speed_mps = 30.0;
        p.hits = frame / 330;
        p.hit_speed_mps = 6.0;
        p.horn = (10.0..10.6).contains(&t);
        p.ocean = if t > 9.0 { 1.0 } else { 0.0 };
        p.birds = if t > 9.0 { 1.0 } else { 0.0 };
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Surface;
    use std::cell::Cell;

    /// Frames per 60 Hz video frame.
    const PER_FRAME: usize = 800;

    fn render_script(
        mixer: &mut Mixer,
        frames: u32,
        mut each: impl FnMut(u32, &mut AudioParams),
    ) -> Vec<f32> {
        let mut out = vec![0.0; frames as usize * PER_FRAME * 2];
        for (f, chunk) in out.chunks_exact_mut(PER_FRAME * 2).enumerate() {
            let f = f as u32;
            let mut p = script::at(f);
            each(f, &mut p);
            mixer.set_params(&p);
            mixer.render(chunk);
        }
        out
    }

    #[test]
    fn sixty_seconds_flat_out_never_clips() {
        let mut m = Mixer::new();
        // Everything at once and loud: the drive, plus every wheel sliding fast, the horn,
        // a hit every quarter second and both ambiences.
        let out = render_script(&mut m, 3600, |f, p| {
            for w in &mut p.wheels {
                w.slip = 3.0;
                w.speed_mps = 40.0;
                w.surface = Surface::Asphalt;
            }
            p.speed_mps = 45.0;
            p.horn = true;
            p.hits = f / 15;
            p.hit_speed_mps = 20.0;
            p.curb_kicks = f / 10;
            p.ocean = 1.0;
            p.birds = 1.0;
        });
        let stats = m.stats();
        assert_eq!(stats.nonfinite, 0);
        assert!(out.iter().all(|v| v.is_finite()));
        let peak = out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(peak <= CEILING, "peak {peak}");
        assert!(peak > 0.3, "silent? {peak}");
        assert_eq!(stats.blocks, 3600 * PER_FRAME as u64 / BLOCK_FRAMES as u64);
    }

    #[test]
    fn hot_buses_are_limited_softly() {
        let mut m = Mixer::new();
        m.gains = [1.0; BUS_COUNT];
        m.master = 1.0;
        for _ in 0..50 {
            for (b, bus) in m.buses.iter_mut().enumerate() {
                for (j, s) in bus.iter_mut().enumerate() {
                    let ph = (j / 2) as f32 / BLOCK_FRAMES as f32 * (b + 1) as f32;
                    *s = 3.0 * crate::synth::dsp::sin_turns(ph);
                }
            }
            m.mix_down();
        }
        let peak = m.block.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(peak <= CEILING && peak > 0.85, "{peak}");
        assert!(m.stats().limited > 0);
        // Below the knee nothing changes.
        let mut q = Mixer::new();
        q.gains = [1.0; BUS_COUNT];
        q.master = 1.0;
        for (j, s) in q.buses[0].iter_mut().enumerate() {
            *s = 0.25 * crate::synth::dsp::sin_turns((j / 2) as f32 / 64.0);
        }
        let input = q.buses[0];
        for _ in 0..20 {
            q.buses[0] = input;
            q.mix_down();
        }
        // Through the DC blocker a 750 Hz tone keeps its level to within 1%.
        let peak = q.block.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!((peak - 0.25).abs() < 0.0025, "{peak}");
        assert_eq!(q.stats().limited, 0);
    }

    #[test]
    fn ducking_curve() {
        assert_eq!(duck_gain(0.0), 1.0);
        assert!((duck_gain(1.0) - 0.55).abs() < 1e-6);
        assert!((duck_gain(0.5) - (1.0 - 0.45 * 0.75)).abs() < 1e-6);
        let mut last = 1.0;
        for i in 1..=100 {
            let g = duck_gain(i as f32 / 100.0);
            assert!(g < last);
            last = g;
        }
        // Ease-out: half the radio does three quarters of the ducking.
        assert!(1.0 - duck_gain(0.5) > 0.7 * (1.0 - duck_gain(1.0)));
        assert_eq!(duck_gain(f32::NAN), 1.0);
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn volume_mute_pause_and_radio() {
        let level = |f: &dyn Fn(&mut AudioParams)| {
            let mut m = Mixer::new();
            let out = render_script(&mut m, 120, |_, p| {
                p.engine.rpm = 3000.0;
                p.engine.throttle = 0.5;
                p.engine.gear = 2;
                f(p);
            });
            rms(&out[out.len() / 2..])
        };
        let full = level(&|_| {});
        let half = level(&|p| p.master = 0.5);
        assert!((half / full - 0.25).abs() < 0.03, "{half} {full}");
        assert_eq!(level(&|p| p.muted = true), 0.0);
        assert!(level(&|p| p.paused = true) < 1e-4);
        let ducked = level(&|p| p.radio = 1.0);
        assert!((ducked / full - 0.55).abs() < 0.05, "{ducked} {full}");
        assert!(level(&|p| p.engine_volume = 0.0) < 0.5 * full);
        let silent = level(&|p| {
            p.engine_volume = 0.0;
            p.world_volume = 0.0;
        });
        assert!(silent < 1e-4, "{silent}");
    }

    #[test]
    fn any_buffer_size_gives_the_same_stream() {
        let p = script::at(100);
        let mut a = Mixer::new();
        a.set_params(&p);
        let mut whole = vec![0.0; 9000];
        a.render(&mut whole);
        let mut b = Mixer::new();
        b.set_params(&p);
        let mut pieces = vec![0.0; 9000];
        for chunk in pieces.chunks_mut(74) {
            b.render(chunk);
        }
        assert!(whole
            .iter()
            .zip(&pieces)
            .all(|(x, y)| x.to_bits() == y.to_bits()));
    }

    thread_local! {
        static ALLOCS: Cell<u64> = const { Cell::new(0) };
    }

    struct Counting;

    // SAFETY: forwards to the system allocator; the counter is a const thread-local Cell,
    // which needs no allocation of its own.
    unsafe impl std::alloc::GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            ALLOCS.with(|c| c.set(c.get() + 1));
            unsafe { std::alloc::System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static GLOBAL: Counting = Counting;

    #[test]
    fn rendering_allocates_nothing() {
        let mut m = Box::new(Mixer::new());
        let mut buf = vec![0.0; PER_FRAME * 2];
        let before = ALLOCS.with(Cell::get);
        for f in 0..240 {
            m.set_params(&script::at(f));
            m.render(&mut buf);
        }
        assert_eq!(ALLOCS.with(Cell::get), before);
    }

    /// FNV-1a, 64-bit.
    fn fnv(bytes: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    /// A 32-bit float stereo WAV.
    fn wav(samples: &[f32]) -> Vec<u8> {
        let data = (samples.len() * 4) as u32;
        let mut v = Vec::with_capacity(44 + data as usize);
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        v.extend_from_slice(&(CHANNELS as u16).to_le_bytes());
        v.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        v.extend_from_slice(&(SAMPLE_RATE * 8).to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        for s in samples {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    /// The scripted drive (12 s per engine profile, all five) renders bit-identically
    /// everywhere. `SKYWAVE_AUDIO_WAV=path` also writes it out for listening.
    ///
    /// Re-pin deliberately, in its own commit, when the synth changes.
    #[test]
    fn scripted_drive_renders_deterministically() {
        const PINNED: u64 = 0xa0b5_ad55_4d1a_fe81;
        let mut m = Mixer::new();
        let out = render_script(&mut m, 720 * 5, |_, _| {});
        let bytes = wav(&out);
        if let Ok(path) = std::env::var("SKYWAVE_AUDIO_WAV") {
            std::fs::write(&path, &bytes).expect("write the WAV");
        }
        let hash = fnv(&bytes);
        assert_eq!(hash, PINNED, "audio hash changed: {hash:#018x}");
    }
}
