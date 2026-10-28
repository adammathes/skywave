//! Skywave's sound (`docs/AUDIO_RADIO.md`): the synthesized effects and the radio.
//!
//! Effects: everything is synthesized at 48 kHz stereo. [`synth`] makes the engine, tire,
//! wind, impact, horn and ambience sounds from a plain [`synth::AudioParams`] the app fills
//! every frame; [`mixer`] runs them in 256-frame blocks into buses with their own gains, a
//! soft limiter and the master volume. The platform owns the sink and pulls interleaved
//! `f32` stereo from [`mixer::Mixer::render`].
//!
//! Radio and deck, platform-neutral: the station model and the preset files ([`station`]),
//! the switching state machine with its tuning phase ([`radio`]), the player's own music
//! (tags and the seeded shuffle: [`music`]; the library and its transport, a tape deck, CD
//! or MP3 player by era: [`deck`]), and the user's radio settings ([`settings`]). The crate
//! plays no stream or file itself. [`Radio`] and [`deck::Deck`] turn presses and player
//! reports ([`PlayerEvent`]) into [`RadioCommand`]s (`Play { url }`, `PlayFile { key,
//! start_s }`, `Stop`, `Volume`) that the platform carries out: an `<audio>` element on the
//! web, a decoder and sink natively. The dial sweep between stations and the deck's click
//! are synthesized sounds ([`synth::tune`], [`synth::deck`]), played by the mixer like the
//! rest.
//!
//! No recorded audio ships with the game: presets are facts (names, stream URLs,
//! homepages) and the date their terms were checked.

pub mod deck;
pub mod mixer;
pub mod music;
pub mod radio;
pub mod settings;
pub mod station;
pub mod synth;

pub use music::Track;
pub use radio::{
    Channel, ErrorReason, PlayerEvent, Radio, RadioCommand, RadioState, NO_SIGNAL_SECONDS,
    TUNE_SECONDS,
};
pub use settings::{CustomStream, DeckSettings, RadioSettings};
pub use station::{Codec, RadioPreset, Source, Station};
