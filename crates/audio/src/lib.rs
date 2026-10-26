//! Skywave's sound (`docs/AUDIO_RADIO.md`): everything is synthesized at 48 kHz stereo.
//!
//! [`synth`] makes the engine, tire, wind, impact, horn and ambience sounds from a plain
//! [`synth::AudioParams`] the app fills every frame; [`mixer`] runs them in 256-frame blocks
//! into buses with their own gains, a soft limiter and the master volume. The platform owns
//! the sink and pulls interleaved `f32` stereo from [`mixer::Mixer::render`].

pub mod mixer;
pub mod synth;
