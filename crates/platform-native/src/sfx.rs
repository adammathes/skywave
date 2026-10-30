//! The sound sink: a `cpal` output stream whose callback runs the `audio::mixer::Mixer` on
//! the audio thread.
//!
//! The main thread hands over each frame's `AudioParams` through a mutex it holds only to
//! copy them; the audio thread only ever `try_lock`s it and keeps the last parameters when
//! it is busy, so it never waits. The mixer itself lives on the audio thread. The mixer runs
//! at 48 kHz; a device that cannot gets the rate nearest to it, and the callback resamples
//! (`Resampler`). No device at all leaves the game silent with a log line.
//! The car radio's stream, when there is one, is added to the mixer's output here
//! (`radio::Tap`).
//!
//! A stream that dies (the device was unplugged, the server went away) raises a flag;
//! `Sfx::recover` on the main thread drops it and reopens the default device, retrying with
//! backoff until one opens.

use std::cmp::Reverse;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use audio::mixer::{Mixer, SAMPLE_RATE};
use audio::synth::AudioParams;

use crate::radio;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, Sample, SampleFormat, SizedSample, StreamConfig};

/// The buffer size asked of the device, frames (≈ 5 ms; `docs/AUDIO_RADIO.md` wants < 20 ms
/// for the engine).
const BUFFER_FRAMES: u32 = 256;
/// Stereo frames rendered per mixer call in the callback.
const CHUNK_FRAMES: usize = 512;
/// The first wait before reopening a lost device, doubled after each failure up to
/// `RETRY_MAX`.
const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(8);

pub struct Sfx {
    params: Arc<Mutex<AudioParams>>,
    radio: Option<Arc<radio::Output>>,
    /// The live stream; `None` while the device is lost.
    stream: Option<cpal::Stream>,
    /// Set by the stream's error callback when the stream is dead.
    lost: Arc<AtomicBool>,
    retry_at: Instant,
    backoff: Duration,
}

impl Sfx {
    /// Opens the default output device, or logs why not. `radio` is the car radio's
    /// output, played with the mixer's.
    pub fn start(radio: Option<Arc<radio::Output>>) -> Option<Sfx> {
        let params = Arc::new(Mutex::new(AudioParams::default()));
        let lost = Arc::new(AtomicBool::new(false));
        match open(&params, radio.clone(), &lost) {
            Ok(stream) => Some(Sfx {
                params,
                radio,
                stream: Some(stream),
                lost,
                retry_at: Instant::now(),
                backoff: RETRY_MIN,
            }),
            Err(e) => {
                log::warn!("sound off: {e}");
                None
            }
        }
    }

    /// This frame's sound parameters.
    pub fn set_params(&self, p: &AudioParams) {
        if let Ok(mut shared) = self.params.lock() {
            *shared = *p;
        }
    }

    /// Reopens the default device once the stream has died, retrying with backoff. Call
    /// every frame; it does nothing while the stream is alive.
    pub fn recover(&mut self, now: Instant) {
        if !self.lost.load(Ordering::Relaxed) {
            return;
        }
        if self.stream.take().is_some() {
            log::warn!("sound: output lost, reopening");
            self.retry_at = now;
            self.backoff = RETRY_MIN;
        }
        if now < self.retry_at {
            return;
        }
        self.lost.store(false, Ordering::Relaxed);
        match open(&self.params, self.radio.clone(), &self.lost) {
            Ok(stream) => self.stream = Some(stream),
            Err(e) => {
                log::debug!("sound: reopen failed: {e}");
                self.lost.store(true, Ordering::Relaxed);
                self.retry_at = now + self.backoff;
                self.backoff = (self.backoff * 2).min(RETRY_MAX);
            }
        }
    }
}

fn open(
    params: &Arc<Mutex<AudioParams>>,
    radio: Option<Arc<radio::Output>>,
    lost: &Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no audio output device")?;
    let ranges = device
        .supported_output_configs()
        .map_err(|e| e.to_string())?;
    // 48 kHz, else the rate nearest it; then f32, then stereo, then any channel count.
    let mut best = None;
    for range in ranges {
        let rate = SAMPLE_RATE
            .max(range.min_sample_rate())
            .min(range.max_sample_rate());
        let Some(config) = range.try_with_sample_rate(rate) else {
            continue;
        };
        let score = (
            rate == SAMPLE_RATE,
            config.sample_format() == SampleFormat::F32,
            config.channels() == 2,
            config.channels() >= 2,
            Reverse(rate.abs_diff(SAMPLE_RATE)),
        );
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, config));
        }
    }
    let (_, supported) = best.ok_or("the output device has no usable config")?;
    let buffer_size = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } if (*min..=*max).contains(&BUFFER_FRAMES) => {
            cpal::BufferSize::Fixed(BUFFER_FRAMES)
        }
        _ => cpal::BufferSize::Default,
    };
    let config = StreamConfig {
        channels: supported.channels(),
        sample_rate: supported.sample_rate(),
        buffer_size,
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, params, radio, lost),
        SampleFormat::I16 => build::<i16>(&device, &config, params, radio, lost),
        SampleFormat::I32 => build::<i32>(&device, &config, params, radio, lost),
        SampleFormat::U16 => build::<u16>(&device, &config, params, radio, lost),
        other => Err(format!("unsupported sample format {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    log::info!("sound: {config:?}, {}", supported.sample_format());
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    params: &Arc<Mutex<AudioParams>>,
    radio: Option<Arc<radio::Output>>,
    lost: &Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    let shared = Arc::clone(params);
    let mut mixer = Box::new(Mixer::new());
    let mut resampler = Resampler::new(config.sample_rate);
    let mut stereo = vec![0.0f32; CHUNK_FRAMES * 2];
    let mut tap = radio.map(|out| radio::Tap::new(out, CHUNK_FRAMES * 2));
    let callback = move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
        if let Ok(p) = shared.try_lock() {
            mixer.set_params(&p);
        }
        for chunk in out.chunks_mut(CHUNK_FRAMES * channels) {
            let frames = chunk.len() / channels;
            let dst = &mut stereo[..frames * 2];
            resampler.fill(dst, |src| {
                mixer.render(src);
                if let Some(tap) = &mut tap {
                    tap.mix_into(src);
                }
            });
            spread(dst, chunk, channels);
        }
    };
    let lost = Arc::clone(lost);
    let on_error = move |e: cpal::Error| {
        // Glitches and reroutes keep the stream playing; anything else means it is dead.
        let kind = e.kind();
        if !matches!(
            kind,
            ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
        ) {
            lost.store(true, Ordering::Relaxed);
        }
        log::warn!("sound stream: {e}");
    };
    device
        .build_output_stream(*config, callback, on_error, None)
        .map_err(|e| e.to_string())
}

/// Linear resampling of the 48 kHz interleaved stereo mix to the device's rate, pulled a
/// chunk of source at a time. At 48 kHz it passes samples through unchanged (two frames
/// late). Allocates only in `new`.
struct Resampler {
    /// Source frames per output frame.
    step: f64,
    /// Position between `prev` and `next`, in [0, 1).
    frac: f64,
    prev: [f32; 2],
    next: [f32; 2],
    /// The last source chunk and the frames of it used so far.
    src: Vec<f32>,
    read: usize,
}

impl Resampler {
    fn new(rate: u32) -> Resampler {
        Resampler {
            step: f64::from(SAMPLE_RATE) / f64::from(rate.max(1)),
            frac: 0.0,
            prev: [0.0; 2],
            next: [0.0; 2],
            src: vec![0.0; CHUNK_FRAMES * 2],
            read: CHUNK_FRAMES,
        }
    }

    /// Fills interleaved stereo `out`; `render` fills a whole source chunk when more is
    /// needed.
    fn fill(&mut self, out: &mut [f32], mut render: impl FnMut(&mut [f32])) {
        for frame in out.chunks_exact_mut(2) {
            let t = self.frac as f32;
            frame[0] = self.prev[0] + (self.next[0] - self.prev[0]) * t;
            frame[1] = self.prev[1] + (self.next[1] - self.prev[1]) * t;
            self.frac += self.step;
            while self.frac >= 1.0 {
                self.frac -= 1.0;
                if self.read == CHUNK_FRAMES {
                    render(&mut self.src);
                    self.read = 0;
                }
                self.prev = self.next;
                self.next = [self.src[self.read * 2], self.src[self.read * 2 + 1]];
                self.read += 1;
            }
        }
    }
}

/// Interleaved stereo `src` onto a device with `channels` channels: left and right on the
/// first two, both mixed down for mono, silence on any others.
fn spread<T: Sample + FromSample<f32>>(src: &[f32], out: &mut [T], channels: usize) {
    for (lr, frame) in src.chunks_exact(2).zip(out.chunks_exact_mut(channels)) {
        if channels == 1 {
            frame[0] = T::from_sample(0.5 * (lr[0] + lr[1]));
            continue;
        }
        for (c, s) in frame.iter_mut().enumerate() {
            *s = T::from_sample(match c {
                0 => lr[0],
                1 => lr[1],
                _ => 0.0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_spreads_onto_any_channel_count() {
        let src = [0.5, -0.5, 0.25, 0.75];
        let mut mono = [0.0f32; 2];
        spread(&src, &mut mono, 1);
        assert_eq!(mono, [0.0, 0.5]);
        let mut quad = [9.0f32; 8];
        spread(&src, &mut quad, 4);
        assert_eq!(quad, [0.5, -0.5, 0.0, 0.0, 0.25, 0.75, 0.0, 0.0]);
        let mut ints = [0i16; 4];
        spread(&src, &mut ints, 2);
        assert_eq!(ints[0], i16::from_sample(0.5f32));
        assert_eq!(ints[1], i16::from_sample(-0.5f32));
    }

    /// A 1 kHz sine at 48 kHz, one chunk per call; counts the frames rendered.
    fn sine(phase: &mut u64) -> impl FnMut(&mut [f32]) + '_ {
        move |src: &mut [f32]| {
            for frame in src.chunks_exact_mut(2) {
                let t = *phase as f64 / f64::from(SAMPLE_RATE);
                let s = (t * 1000.0 * std::f64::consts::TAU).sin() as f32;
                frame.copy_from_slice(&[s, s]);
                *phase += 1;
            }
        }
    }

    #[test]
    fn resampler_keeps_length_and_pitch_at_44k() {
        let rate = 44_100;
        let mut r = Resampler::new(rate);
        let mut pulled = 0u64;
        // One second of output in device-sized callbacks.
        let mut out = vec![0.0f32; rate as usize * 2];
        for chunk in out.chunks_mut(441 * 2) {
            r.fill(chunk, sine(&mut pulled));
        }
        // One second of source used, give or take the chunk read ahead.
        let source = f64::from(SAMPLE_RATE);
        assert!(
            (pulled as f64 - source).abs() <= CHUNK_FRAMES as f64,
            "{pulled}"
        );
        // Still 1 kHz: 2000 zero crossings in the second.
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((1998..=2002).contains(&crossings), "{crossings}");
        // Linear interpolation of a 1 kHz sine stays close to it (two source frames late).
        let expect = |i: usize| {
            let t = i as f64 / f64::from(rate) - 2.0 / source;
            (t * 1000.0 * std::f64::consts::TAU).sin() as f32
        };
        let err = (10..left.len())
            .map(|i| (left[i] - expect(i)).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 0.05, "{err}");
    }

    #[test]
    fn resampler_passes_48k_through() {
        let mut r = Resampler::new(SAMPLE_RATE);
        let mut pulled = 0u64;
        let mut out = vec![0.0f32; 1000 * 2];
        r.fill(&mut out, sine(&mut pulled));
        let mut phase = 0u64;
        let mut want = vec![0.0f32; 1000 * 2];
        sine(&mut phase)(&mut want);
        assert_eq!(out[4..], want[..want.len() - 4]);
    }
}
