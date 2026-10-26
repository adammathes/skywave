//! Skywave's radio, platform-neutral: the station model and the preset files
//! ([`station`]), the switching state machine with its tune-noise phase ([`radio`]), and
//! the user's radio settings ([`settings`]).
//!
//! The crate plays nothing itself. [`Radio`] turns presses (next, previous, off) and
//! player reports ([`PlayerEvent`]) into [`RadioCommand`]s (`Play { url }`, `Stop`,
//! `Volume`, `TuneNoise`) that the platform carries out: an `<audio>` element on the web,
//! a decoder and sink natively. See `docs/AUDIO_RADIO.md`.
//!
//! No audio ships with the game: presets are facts (names, stream URLs, homepages) and the
//! date their terms were checked.

pub mod radio;
pub mod settings;
pub mod station;

pub use radio::{Channel, ErrorReason, PlayerEvent, Radio, RadioCommand, RadioState, TUNE_SECONDS};
pub use settings::{CustomStream, RadioSettings};
pub use station::{Codec, RadioPreset, Source, Station};
