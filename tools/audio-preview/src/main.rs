//! `audio-preview`: renders the synthesized sounds through the full mixer to 16-bit WAV
//! files and prints a numeric look at each, for checking the synth without speakers.
//!
//! Usage: `audio-preview [OUT_DIR]` (default `audio-preview-out`). Writes
//! `idle_<profile>.wav` (5 s at 900 rpm per engine profile), `sweep.wav` (idle, then full
//! throttle to the redline and the limiter), `shift.wav` (a pull, an upshift, a lift with
//! pops) and `squeal.wav` (a slide on asphalt). For each it prints the peak and RMS level,
//! the DC offset, and where the energy is: the firing frequency and its harmonics for the
//! engine, the squeal band for the tires. Also prints the synth's cost per 256-frame block.

use std::path::{Path, PathBuf};
use std::time::Instant;

use audio::mixer::{Mixer, BLOCK_SAMPLES, SAMPLE_RATE};
use audio::synth::{AudioParams, EngineProfile, Surface, WheelParams};

const SR: f32 = SAMPLE_RATE as f32;
/// Parameter updates per second, like a 60 Hz game.
const FPS: f32 = 60.0;
const IDLE: f32 = 900.0;
const REDLINE: f32 = 7200.0;

fn base(profile: EngineProfile) -> AudioParams {
    let mut p = AudioParams::default();
    p.engine.profile = profile;
    p.engine.idle_rpm = IDLE;
    p.engine.redline_rpm = REDLINE;
    p
}

/// Renders `seconds` of `scene(t)` (interleaved stereo).
fn render(seconds: f32, scene: impl Fn(f32) -> AudioParams) -> Vec<f32> {
    let mut m = Mixer::new();
    let per = (SR / FPS) as usize;
    let frames = (seconds * FPS) as usize;
    let mut out = vec![0.0; frames * per * 2];
    for (i, chunk) in out.chunks_exact_mut(per * 2).enumerate() {
        m.set_params(&scene(i as f32 / FPS));
        m.render(chunk);
    }
    out
}

fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let data = (samples.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + data as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    v.extend_from_slice(&(SAMPLE_RATE * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data.to_le_bytes());
    for &s in samples {
        let q = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        v.extend_from_slice(&q.to_le_bytes());
    }
    std::fs::write(path, v)
}

fn mono(x: &[f32]) -> Vec<f32> {
    x.chunks_exact(2).map(|f| 0.5 * (f[0] + f[1])).collect()
}

/// Amplitude of the `hz` component of `x` (a Hann-windowed single DFT bin).
fn amp(x: &[f32], hz: f32) -> f32 {
    let n = x.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    let w = std::f64::consts::TAU * f64::from(hz) / f64::from(SR);
    for (i, &v) in x.iter().enumerate() {
        let hann = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n).cos();
        let a = w * i as f64;
        re += f64::from(v) * hann * a.cos();
        im += f64::from(v) * hann * a.sin();
    }
    ((re * re + im * im).sqrt() * 4.0 / n) as f32
}

/// The strongest component within `rel` of `hz`: (frequency, amplitude).
fn peak_near(x: &[f32], hz: f32, rel: f32) -> (f32, f32) {
    let (lo, hi) = (hz * (1.0 - rel), hz * (1.0 + rel));
    let step = (hz * 0.004).max(0.25);
    let mut best = (hz, 0.0);
    let mut f = lo;
    while f <= hi {
        let a = amp(x, f);
        if a > best.1 {
            best = (f, a);
        }
        f += step;
    }
    best
}

fn db(a: f32) -> f32 {
    20.0 * a.max(1e-9).log10()
}

fn levels(name: &str, x: &[f32]) {
    let peak = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    let dc = x.iter().map(|&v| f64::from(v)).sum::<f64>() / x.len() as f64;
    println!(
        "{name}: peak {:.1} dBFS, rms {:.1} dBFS, dc {:+.5}",
        db(peak),
        db(rms),
        dc
    );
}

/// Firing-frequency check over one window: the measured peak near the expected firing
/// frequency, and harmonics 2..4 relative to it.
fn engine_line(label: &str, x: &[f32], rpm: f32, cylinders: f32) {
    let fire = rpm * cylinders / 120.0;
    let (f, a) = peak_near(x, fire, 0.08);
    let off = amp(x, fire * 1.5);
    let h: Vec<String> = (2..=4)
        .map(|k| format!("h{k} {:+.1} dB", db(amp(x, fire * k as f32)) - db(a)))
        .collect();
    println!(
        "  {label}: {rpm:5.0} rpm -> expect {fire:6.1} Hz, peak {f:6.1} Hz ({:+.1} dB over 1.5x), {}",
        db(a) - db(off),
        h.join(", ")
    );
}

fn main() -> std::io::Result<()> {
    let out_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "audio-preview-out".into()),
    );
    std::fs::create_dir_all(&out_dir)?;

    for profile in EngineProfile::ALL {
        let x = render(5.0, |_| base(profile));
        let name = format!("idle_{}.wav", profile.name());
        write_wav(&out_dir.join(&name), &x)?;
        levels(&name, &x);
        let m = mono(&x);
        let cyl = f32::from(profile.voice().cylinders);
        engine_line("idle", &m[48_000..96_000], IDLE, cyl);
    }

    // Idle, then a 4 s full-throttle sweep to the redline, then the limiter.
    let sweep_rpm = |t: f32| {
        if t < 1.0 {
            IDLE
        } else {
            (IDLE + (REDLINE - IDLE) * (t - 1.0) / 4.0).min(REDLINE)
        }
    };
    let x = render(6.0, |t| {
        let mut p = base(EngineProfile::V8Flatplane);
        p.engine.rpm = sweep_rpm(t);
        p.engine.throttle = if t < 1.0 { 0.0 } else { 1.0 };
        p.engine.gear = 3;
        p
    });
    write_wav(&out_dir.join("sweep.wav"), &x)?;
    levels("sweep.wav", &x);
    let m = mono(&x);
    for t in [1.5f32, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5] {
        let c = (t * SR) as usize;
        engine_line(
            &format!("t={t:.1}s"),
            &m[c - 2400..c + 2400],
            sweep_rpm(t),
            8.0,
        );
    }
    let env: Vec<f32> = m[(5.2 * SR) as usize..(5.8 * SR) as usize]
        .chunks(240)
        .map(|c| c.iter().fold(0.0f32, |a, v| a.max(v.abs())))
        .collect();
    let (lo, hi) = env
        .iter()
        .fold((1.0f32, 0.0f32), |(l, h), &v| (l.min(v), h.max(v)));
    println!(
        "  limiter (5.2..5.8 s): 5 ms envelope swings {:.1} dB",
        db(hi) - db(lo)
    );

    // A pull in second, an upshift at 1.5 s, a lift at 3 s.
    let shift = |t: f32| {
        let mut p = base(EngineProfile::V8Flatplane);
        let (rpm, thr, gear) = if t < 1.5 {
            (3000.0 + 3600.0 * t / 1.5, 1.0, 2)
        } else if t < 3.0 {
            (4620.0 + 1380.0 * (t - 1.5) / 1.5, 1.0, 3)
        } else {
            (6000.0 - 3500.0 * ((t - 3.0) / 1.5).min(1.0), 0.0, 3)
        };
        p.engine.rpm = rpm;
        p.engine.throttle = thr;
        p.engine.gear = gear;
        p
    };
    let x = render(4.5, shift);
    write_wav(&out_dir.join("shift.wav"), &x)?;
    levels("shift.wav", &x);
    let m = mono(&x);
    let rms = |a: f32, b: f32| {
        let s = &m[(a * SR) as usize..(b * SR) as usize];
        db((s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt())
    };
    println!(
        "  rms 1.30-1.50 s {:.1} dB, shift dip 1.55-1.70 s {:.1} dB, after 1.90-2.10 s {:.1} dB",
        rms(1.3, 1.5),
        rms(1.55, 1.7),
        rms(1.9, 2.1)
    );
    let crest = |a: f32, b: f32| {
        let s = &m[(a * SR) as usize..(b * SR) as usize];
        let peak = s.iter().fold(0.0f32, |p, v| p.max(v.abs()));
        let r = (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        db(peak) - db(r)
    };
    println!(
        "  crest factor before lift 2.5-3.0 s {:.1} dB, after lift (pops) 3.0-3.8 s {:.1} dB",
        crest(2.5, 3.0),
        crest(3.0, 3.8)
    );

    // A slide: slip 0.5, then 2.2 for two seconds, then 0.5.
    let x = render(4.0, |t| {
        let mut p = base(EngineProfile::V8Flatplane);
        p.engine.rpm = 3500.0;
        p.engine.throttle = 0.3;
        p.engine.gear = 2;
        p.engine_volume = 0.3;
        p.speed_mps = 15.0;
        let slip = if (1.0..3.0).contains(&t) { 2.2 } else { 0.5 };
        p.wheels = [WheelParams {
            slip,
            load: 1.1,
            speed_mps: 15.0,
            grounded: true,
            surface: Surface::Asphalt,
        }; 4];
        p
    });
    write_wav(&out_dir.join("squeal.wav"), &x)?;
    levels("squeal.wav", &x);
    let m = mono(&x);
    for (label, a, b) in [
        ("grip", 0.3, 0.9),
        ("slide", 1.5, 2.5),
        ("grip again", 3.4, 3.9),
    ] {
        let s = &m[(a * SR) as usize..(b * SR) as usize];
        let (f, band) = peak_near(s, 900.0, 0.3);
        println!(
            "  {label}: strongest 630..1170 Hz component {f:.0} Hz at {:.1} dBFS",
            db(band)
        );
    }

    // Cost: the engine alone isn't separable from outside the crate; time the whole mixer.
    let mut mixer = Mixer::new();
    mixer.set_params(&shift(2.0));
    let mut buf = [0.0f32; BLOCK_SAMPLES];
    let blocks = 4000;
    let t = Instant::now();
    for _ in 0..blocks {
        mixer.render(&mut buf);
    }
    println!(
        "mixer (engine + world + limiter): {:.4} ms per 256-frame block{}",
        t.elapsed().as_secs_f64() * 1000.0 / f64::from(blocks),
        if cfg!(debug_assertions) {
            " (debug build)"
        } else {
            ""
        }
    );
    println!("wrote {}", out_dir.display());
    Ok(())
}
