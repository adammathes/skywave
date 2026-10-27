//! Skywave's sound (`docs/AUDIO_RADIO.md`): the synthesized effects and the radio.
//!
//! Effects: everything is synthesized at 48 kHz stereo. [`synth`] makes the engine, tire,
//! wind, impact, horn and ambience sounds from a plain [`synth::AudioParams`] the app fills
//! every frame; [`mixer`] runs them in 256-frame blocks into buses with their own gains, a
//! soft limiter and the master volume. The platform owns the sink and pulls interleaved
//! `f32` stereo from [`mixer::Mixer::render`].
//!
//! Radio, platform-neutral: the station model and the preset files ([`station`]), the
//! switching state machine with its tune-noise phase ([`radio`]), and the user's radio
//! settings ([`settings`]). The crate plays no stream itself. [`Radio`] turns presses
//! (next, previous, off) and player reports ([`PlayerEvent`]) into [`RadioCommand`]s
//! (`Play { url }`, `Stop`, `Volume`, `TuneNoise`) that the platform carries out: an
//! `<audio>` element on the web, a decoder and sink natively.
//!
//! No recorded audio ships with the game: presets are facts (names, stream URLs,
//! homepages) and the date their terms were checked.

pub mod mixer;
pub mod radio;
pub mod settings;
pub mod station;
pub mod synth;

pub use radio::{
    Channel, ErrorReason, PlayerEvent, Radio, RadioCommand, RadioState, NO_SIGNAL_SECONDS,
    TUNE_SECONDS,
};
pub use settings::{CustomStream, RadioSettings};
pub use station::{Codec, RadioPreset, Source, Station};
