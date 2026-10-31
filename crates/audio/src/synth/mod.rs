//! The synthesized sounds (`docs/AUDIO_RADIO.md`, "Engine and world sound"): no recorded
//! audio anywhere. [`engine`] is the car's engine, [`world`] everything else the car and the
//! place make, [`tune`] the radio's dial between stations, [`deck`] the deck's mechanism,
//! [`chime`] the green wave's bell. All read one [`AudioParams`], a
//! plain struct the app fills every frame from the car's sim state, the collider, the place
//! and the radio.
//!
//! Events (a curb kick, a wall hit) are counters rather than flags, so a sink that skips a
//! parameter update (the native audio thread reads them without waiting) never loses one,
//! and one that sees the same update twice never doubles it.

pub mod chime;
pub mod deck;
pub mod dsp;
pub mod engine;
pub mod tune;
pub mod world;

pub use chime::ChimeSynth;
pub use deck::DeckSynth;
pub use engine::{EngineProfile, EngineSynth};
pub use tune::TuneSynth;
pub use world::WorldSynth;

/// What a tire rolls on (the collider's surfaces).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    #[default]
    Asphalt = 0,
    Concrete = 1,
    Grass = 2,
}

/// One wheel, from the sim's `WheelState`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelParams {
    /// Combined slip relative to the tire's peak: below 1 it grips, above it slides.
    pub slip: f32,
    /// Normal load relative to the wheel's share of the car's weight (1 at rest).
    pub load: f32,
    /// The faster of the ground speed under the tire and its surface speed, m/s: a tire
    /// spinning on the spot still squeals.
    pub speed_mps: f32,
    pub grounded: bool,
    pub surface: Surface,
}

/// The engine, from the car's spec and sim state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineParams {
    pub profile: EngineProfile,
    pub rpm: f32,
    pub idle_rpm: f32,
    pub redline_rpm: f32,
    /// Smoothed throttle pedal, `[0, 1]` (the sim's, so reverse's pedal too).
    pub throttle: f32,
    /// -1 reverse, 0 neutral, 1.. forward. A change between forward gears dips the sound
    /// for `shift_ms` (the gearbox's torque cut).
    pub gear: i8,
    pub shift_ms: f32,
}

impl Default for EngineParams {
    fn default() -> Self {
        EngineParams {
            profile: EngineProfile::Default,
            rpm: 900.0,
            idle_rpm: 900.0,
            redline_rpm: 6500.0,
            throttle: 0.0,
            gear: 1,
            shift_ms: 250.0,
        }
    }
}

/// Passing NPC cars the world bus voices at once: the nearest.
pub const TRAFFIC_VOICES: usize = 4;

/// One passing car's engine, as the listener hears it: the app pans and attenuates it by
/// where the car is and pitches it by its speed and the Doppler shift.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrafficVoice {
    /// Level at the ear, `[0, 1]` (0: no car in this voice).
    pub gain: f32,
    /// Where it is, `[-1, 1]` (left to right).
    pub pan: f32,
    /// Firing frequency heard, Hz.
    pub hz: f32,
}

/// Everything the sound needs for one frame. Plain data; `Default` is a car idling in
/// first gear on asphalt, at full volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioParams {
    /// Volumes, `[0, 1]`: everything, the engine bus, the world bus.
    pub master: f32,
    pub engine_volume: f32,
    pub world_volume: f32,
    pub muted: bool,
    /// The game is paused (a menu is open): the car and world fade out.
    pub paused: bool,
    /// How loud the radio is, `[0, 1]`: the engine and world buses duck under it
    /// ([`crate::mixer::duck_gain`]).
    pub radio: f32,
    pub engine: EngineParams,
    /// Forward speed, m/s (negative reversing).
    pub speed_mps: f32,
    /// Front-left, front-right, rear-left, rear-right.
    pub wheels: [WheelParams; 4],
    /// No roof over the front seats (a convertible or a targa): the wind is heard in the
    /// open rather than through the cabin.
    pub open_top: bool,
    /// Counts every curb kick so far, and how fast the latest wheel hit its curb, m/s.
    pub curb_kicks: u32,
    pub curb_speed_mps: f32,
    /// Counts every wall, post or box hit so far, and how hard the latest was, m/s.
    pub hits: u32,
    pub hit_speed_mps: f32,
    /// Which side of the car the latest curb kick and hit were on, `[-1, 1]` (left to
    /// right): the bump leans that way in the stereo image.
    pub curb_side: f32,
    pub hit_side: f32,
    pub horn: bool,
    /// Ambience amounts, `[0, 1]`: ocean wash near beach and water, birds by day in
    /// residential streets and parks.
    pub ocean: f32,
    pub birds: f32,
    /// A railroad crossing's bells at the listener, `[0, 1]`.
    pub bell: f32,
    /// The nearest passing cars' engines.
    pub traffic: [TrafficVoice; TRAFFIC_VOICES],
    /// Counts every tune of the radio so far ([`crate::Radio::tunes`]): each new one plays
    /// the dial sweep between stations...
    pub tunes: u32,
    /// ...at this level, `[0, 1]`: the radio volume setting (the deck's click too).
    pub tune_volume: f32,
    /// Counts every transport press of the deck that moves its mechanism
    /// ([`crate::deck::Deck::clicks`]): each new one plays its click...
    pub deck_clicks: u32,
    /// ...of this mechanism: 0 a tape deck, 1 a CD player, 2 an MP3 player.
    pub deck_kind: u8,
    /// Counts every signal passed on green in a run (the green wave, `traffic::Streak`):
    /// each new one rings the chime...
    pub waves: u32,
    /// ...as the run's `wave_run`th light (1 the first): higher and fuller as it grows.
    pub wave_run: u8,
}

impl Default for AudioParams {
    fn default() -> Self {
        let wheel = WheelParams {
            slip: 0.0,
            load: 1.0,
            speed_mps: 0.0,
            grounded: true,
            surface: Surface::Asphalt,
        };
        AudioParams {
            master: 1.0,
            engine_volume: 1.0,
            world_volume: 1.0,
            muted: false,
            paused: false,
            radio: 0.0,
            engine: EngineParams::default(),
            speed_mps: 0.0,
            wheels: [wheel; 4],
            open_top: false,
            curb_kicks: 0,
            curb_speed_mps: 0.0,
            hits: 0,
            hit_speed_mps: 0.0,
            curb_side: 0.0,
            hit_side: 0.0,
            horn: false,
            ocean: 0.0,
            birds: 0.0,
            bell: 0.0,
            traffic: [TrafficVoice::default(); TRAFFIC_VOICES],
            tunes: 0,
            tune_volume: 1.0,
            deck_clicks: 0,
            deck_kind: 0,
            waves: 0,
            wave_run: 0,
        }
    }
}

impl AudioParams {
    /// Every number finite and in range (non-finite ones take a safe value), so no input
    /// can make the synths produce NaN or blow up.
    pub fn sanitized(&self) -> AudioParams {
        use dsp::{clamp, unit};
        let fin = |v: f32, default: f32| if v.is_finite() { v } else { default };
        let e = &self.engine;
        let idle = clamp(e.idle_rpm, 300.0, 3000.0);
        let redline = clamp(e.redline_rpm, idle + 500.0, 12_000.0);
        let mut wheels = self.wheels;
        for w in &mut wheels {
            w.slip = clamp(w.slip, 0.0, 10.0);
            w.load = clamp(w.load, 0.0, 4.0);
            w.speed_mps = clamp(w.speed_mps, 0.0, 150.0);
        }
        let traffic = self.traffic.map(|v| TrafficVoice {
            gain: unit(v.gain),
            pan: clamp(fin(v.pan, 0.0), -1.0, 1.0),
            hz: clamp(fin(v.hz, 0.0), 0.0, 400.0),
        });
        AudioParams {
            master: unit(self.master),
            engine_volume: unit(self.engine_volume),
            world_volume: unit(self.world_volume),
            radio: unit(self.radio),
            engine: EngineParams {
                profile: e.profile,
                rpm: clamp(fin(e.rpm, idle), 0.0, redline * 1.2),
                idle_rpm: idle,
                redline_rpm: redline,
                throttle: unit(e.throttle),
                gear: e.gear,
                shift_ms: clamp(e.shift_ms, 0.0, 2000.0),
            },
            speed_mps: clamp(fin(self.speed_mps, 0.0), -150.0, 150.0),
            wheels,
            curb_speed_mps: clamp(self.curb_speed_mps, 0.0, 100.0),
            hit_speed_mps: clamp(self.hit_speed_mps, 0.0, 100.0),
            curb_side: clamp(fin(self.curb_side, 0.0), -1.0, 1.0),
            hit_side: clamp(fin(self.hit_side, 0.0), -1.0, 1.0),
            ocean: unit(self.ocean),
            birds: unit(self.birds),
            bell: unit(self.bell),
            traffic,
            tune_volume: unit(self.tune_volume),
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizing_fixes_every_bad_number() {
        let mut p = AudioParams {
            master: f32::NAN,
            speed_mps: f32::NAN,
            ocean: 7.0,
            ..AudioParams::default()
        };
        p.traffic[1] = TrafficVoice {
            gain: f32::NAN,
            pan: 4.0,
            hz: f32::INFINITY,
        };
        p.engine.rpm = f32::INFINITY;
        p.engine.redline_rpm = f32::NAN;
        p.wheels[2].slip = f32::NEG_INFINITY;
        let s = p.sanitized();
        assert_eq!(s.master, 0.0);
        assert_eq!(s.engine.rpm, s.engine.idle_rpm);
        assert!(s.engine.redline_rpm > s.engine.idle_rpm);
        assert_eq!(s.wheels[2].slip, 0.0);
        assert_eq!(s.speed_mps, 0.0);
        assert_eq!(s.ocean, 1.0);
        assert_eq!((s.traffic[1].gain, s.traffic[1].pan), (0.0, 1.0));
        assert_eq!(s.traffic[1].hz, 0.0);
        assert_eq!(AudioParams::default().sanitized(), AudioParams::default());
    }
}
