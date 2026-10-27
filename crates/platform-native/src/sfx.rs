//! The sound sink: a `cpal` output stream at 48 kHz whose callback runs the
//! `audio::mixer::Mixer` on the audio thread.
//!
//! The main thread hands over each frame's `AudioParams` through a mutex it holds only to
//! copy them; the audio thread only ever `try_lock`s it and keeps the last parameters when
//! it is busy, so it never waits. The mixer itself lives on the audio thread. Devices
//! that cannot run at 48 kHz, or no device at all, leave the game silent with a log line.
//! The car radio's stream, when there is one, is added to the mixer's output here
//! (`radio::Tap`).

use std::sync::{Arc, Mutex};

use audio::mixer::{Mixer, SAMPLE_RATE};
use audio::synth::AudioParams;

use crate::radio;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};

/// The buffer size asked of the device, frames (≈ 5 ms; `docs/AUDIO_RADIO.md` wants < 20 ms
/// for the engine).
const BUFFER_FRAMES: u32 = 256;
/// Stereo frames rendered per mixer call in the callback.
const CHUNK_FRAMES: usize = 512;

pub struct Sfx {
    params: Arc<Mutex<AudioParams>>,
    _stream: cpal::Stream,
}

impl Sfx {
    /// Opens the default output device, or logs why not. `radio` is the car radio's
    /// output, played with the mixer's.
    pub fn start(radio: Option<Arc<radio::Output>>) -> Option<Sfx> {
        match open(radio) {
            Ok(sfx) => Some(sfx),
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
}

fn open(radio: Option<Arc<radio::Output>>) -> Result<Sfx, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no audio output device")?;
    let ranges = device
        .supported_output_configs()
        .map_err(|e| e.to_string())?;
    // 48 kHz, preferring f32, then stereo, then any channel count.
    let mut best: Option<(u32, cpal::SupportedStreamConfig)> = None;
    for range in ranges {
        let Some(config) = range.try_with_sample_rate(SAMPLE_RATE) else {
            continue;
        };
        let score = u32::from(config.sample_format() == SampleFormat::F32) * 4
            + u32::from(config.channels() == 2) * 2
            + u32::from(config.channels() >= 2);
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, config));
        }
    }
    let (_, supported) = best.ok_or("the output device cannot play 48 kHz")?;
    let buffer_size = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } if (*min..=*max).contains(&BUFFER_FRAMES) => {
            cpal::BufferSize::Fixed(BUFFER_FRAMES)
        }
        _ => cpal::BufferSize::Default,
    };
    let config = StreamConfig {
        channels: supported.channels(),
        sample_rate: SAMPLE_RATE,
        buffer_size,
    };
    let params = Arc::new(Mutex::new(AudioParams::default()));
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, &params, radio),
        SampleFormat::I16 => build::<i16>(&device, &config, &params, radio),
        SampleFormat::I32 => build::<i32>(&device, &config, &params, radio),
        SampleFormat::U16 => build::<u16>(&device, &config, &params, radio),
        other => Err(format!("unsupported sample format {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    log::info!("sound: {config:?}, {}", supported.sample_format());
    Ok(Sfx {
        params,
        _stream: stream,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    params: &Arc<Mutex<AudioParams>>,
    radio: Option<Arc<radio::Output>>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    let shared = Arc::clone(params);
    let mut mixer = Box::new(Mixer::new());
    let mut stereo = vec![0.0f32; CHUNK_FRAMES * 2];
    let mut tap = radio.map(|out| radio::Tap::new(out, CHUNK_FRAMES * 2));
    let callback = move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
        if let Ok(p) = shared.try_lock() {
            mixer.set_params(&p);
        }
        for chunk in out.chunks_mut(CHUNK_FRAMES * channels) {
            let frames = chunk.len() / channels;
            let src = &mut stereo[..frames * 2];
            mixer.render(src);
            if let Some(tap) = &mut tap {
                tap.mix_into(src);
            }
            spread(src, chunk, channels);
        }
    };
    device
        .build_output_stream(*config, callback, |e| log::warn!("sound stream: {e}"), None)
        .map_err(|e| e.to_string())
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
}
