//! The radio's switching state machine.
//!
//! ```text
//!            next/prev/power                tick ≥ TUNE_SECONDS
//!   Off ───────────────────▶ Tuning ─────────────────────────▶ Playing ──┐
//!    ▲                        ▲  │ next/prev (restart)            │      │ Failed, URLs left:
//!    │ off                    │  └─────────┘                      │      │ Play the next URL
//!    └────────────────────────┼─────────────── off ───────────────┘◀─────┘
//!                             │ next/prev                         │ Failed, no URL left
//!                             └──────────────── Error ◀───────────┘
//! ```
//!
//! Switching stops the old station and plays [`TUNE_SECONDS`] of the dial sweep before the
//! new one starts: the radio counts its tunes ([`Radio::tunes`]) and the mixer's
//! [`crate::synth::TuneSynth`] plays a burst for each new one. A station whose every URL
//! failed shows [`ErrorReason::NoSignal`] for [`NO_SIGNAL_SECONDS`], then the radio moves up
//! the dial; failed stations are remembered and skipped until one plays again. Every `Play`
//! carries a token; a [`PlayerEvent`] about an older token (a
//! station already switched away from) is ignored. The radio allocates only when it
//! starts a station (the URL in the `Play` command).

/// How long the dial sweep between stations lasts, s: the new station starts as it ends.
pub const TUNE_SECONDS: f32 = 0.5;

/// How long a station that did not play stays on "no signal" before the radio moves on, s.
pub const NO_SIGNAL_SECONDS: f32 = 2.0;

/// A station the radio can play: a stream station of a preset or one the user added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub format: String,
    /// Tried in order until one plays; never empty.
    pub urls: Vec<String>,
}

impl Channel {
    /// The channel of a stream station; `None` for a files station (or one without URLs).
    pub fn from_station(s: &crate::Station) -> Option<Channel> {
        let urls = s.urls();
        (!urls.is_empty()).then(|| Channel {
            id: s.id.clone(),
            name: s.name.clone(),
            format: s.format.clone(),
            urls: urls.to_vec(),
        })
    }
}

/// Why a station is not playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorReason {
    /// Every URL of the station failed to play.
    NoSignal,
}

/// What the radio is doing. Stations are indices into [`Radio::channels`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RadioState {
    Off,
    /// Between stations: the dial sweep for [`TUNE_SECONDS`]; `t` is the time spent so far.
    Tuning {
        from: Option<usize>,
        to: usize,
        t: f32,
    },
    /// Asked the player to play (see [`Radio::is_live`] for whether sound came out).
    Playing {
        station: usize,
    },
    Error {
        station: usize,
        reason: ErrorReason,
    },
}

/// What the platform's player should do, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum RadioCommand {
    /// Play this stream (replacing whatever played); report back with `token`.
    Play { url: String, token: u32 },
    /// Silence.
    Stop,
    /// Radio volume, 0 to 1.
    Volume(f32),
}

/// What the platform's player reports about the `Play` with `token`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerEvent {
    /// Sound is coming out.
    Started { token: u32 },
    /// The URL did not play (network, format, refused).
    Failed { token: u32 },
    /// The browser wants a user gesture before it plays sound; the platform retries on the
    /// next one.
    Blocked { token: u32 },
}

/// The car radio: its stations, its state and the commands waiting for the platform.
#[derive(Clone, Debug)]
pub struct Radio {
    channels: Vec<Channel>,
    state: RadioState,
    /// The station last tuned: where turning the radio on resumes.
    last: Option<usize>,
    /// Which of the playing station's URLs is being tried.
    url: usize,
    token: u32,
    tunes: u32,
    live: bool,
    blocked: bool,
    volume: f32,
    /// Ids of the stations that failed to play this session; the dial skips them.
    failed: Vec<String>,
    /// Time spent in `Error`, s.
    error_t: f32,
    out: Vec<RadioCommand>,
}

impl Radio {
    /// An off radio with these stations at full volume.
    pub fn new(channels: Vec<Channel>) -> Radio {
        Radio {
            channels,
            state: RadioState::Off,
            last: None,
            url: 0,
            token: 0,
            tunes: 0,
            live: false,
            blocked: false,
            volume: 1.0,
            failed: Vec::new(),
            error_t: 0.0,
            out: Vec::with_capacity(8),
        }
    }

    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    pub fn state(&self) -> RadioState {
        self.state
    }

    pub fn is_on(&self) -> bool {
        self.state != RadioState::Off
    }

    /// The station tuned, being tuned to or failing.
    pub fn station(&self) -> Option<usize> {
        match self.state {
            RadioState::Off => None,
            RadioState::Tuning { to, .. } => Some(to),
            RadioState::Playing { station } | RadioState::Error { station, .. } => Some(station),
        }
    }

    pub fn channel(&self) -> Option<&Channel> {
        self.station().and_then(|i| self.channels.get(i))
    }

    /// The id of the station last tuned (what settings keep).
    pub fn last_id(&self) -> Option<&str> {
        self.last
            .and_then(|i| self.channels.get(i))
            .map(|c| c.id.as_str())
    }

    /// Makes `id` the station turning the radio on resumes (ignored if unknown).
    pub fn restore(&mut self, id: &str) {
        if let Some(i) = self.channels.iter().position(|c| c.id == id) {
            self.last = Some(i);
        }
    }

    /// Sound is coming out of the playing station.
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// The player waits for a user gesture before it can play.
    pub fn is_blocked(&self) -> bool {
        self.blocked
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// Sets the volume (clamped to 0–1) and tells the player when it changed.
    pub fn set_volume(&mut self, v: f32) {
        let v = if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            1.0
        };
        if v != self.volume {
            self.volume = v;
            self.out.push(RadioCommand::Volume(v));
        }
    }

    /// Replaces the stations (the user enabled or disabled some, or picked another
    /// preset). The station playing keeps playing when it is still there (found by id);
    /// otherwise the radio turns off.
    pub fn set_channels(&mut self, channels: Vec<Channel>) {
        let id = |i: Option<usize>| i.and_then(|i| self.channels.get(i)).map(|c| c.id.clone());
        let (current, last) = (id(self.station()), id(self.last));
        let find = |id: &Option<String>| {
            id.as_ref()
                .and_then(|id| channels.iter().position(|c| &c.id == id))
        };
        let (new_current, new_last) = (find(&current), find(&last));
        self.channels = channels;
        self.last = new_last;
        match (self.state, new_current) {
            (RadioState::Off, _) => {}
            (_, None) => self.off(),
            (RadioState::Tuning { t, .. }, Some(to)) => {
                self.state = RadioState::Tuning { from: None, to, t };
            }
            (RadioState::Playing { .. }, Some(station)) => {
                self.state = RadioState::Playing { station };
            }
            (RadioState::Error { reason, .. }, Some(station)) => {
                self.state = RadioState::Error { station, reason };
            }
        }
    }

    /// Next station up the dial (from off: the last station, or the first).
    pub fn next(&mut self) {
        self.step(1);
    }

    /// Next station down the dial (from off: the last station, or the last one).
    pub fn prev(&mut self) {
        self.step(-1);
    }

    fn step(&mut self, dir: i32) {
        let n = self.channels.len();
        if n == 0 {
            return;
        }
        let to = match self.station() {
            Some(i) if dir > 0 => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => match self.last {
                Some(i) if i < n => i,
                _ if dir > 0 => 0,
                _ => n - 1,
            },
        };
        self.tune(self.skip_failed(to, dir).unwrap_or(to));
    }

    /// The first station from `to` along `dir` that has not failed, if any.
    fn skip_failed(&self, to: usize, dir: i32) -> Option<usize> {
        let n = self.channels.len();
        (0..n)
            .map(|k| {
                if dir > 0 {
                    (to + k) % n
                } else {
                    (to + n - k) % n
                }
            })
            .find(|&i| !self.failed.contains(&self.channels[i].id))
    }

    /// Turns the radio on at the last station (or the first).
    pub fn power_on(&mut self) {
        if self.state == RadioState::Off {
            self.step(1);
        }
    }

    pub fn off(&mut self) {
        if self.state != RadioState::Off {
            self.state = RadioState::Off;
            self.live = false;
            self.blocked = false;
            self.out.push(RadioCommand::Stop);
        }
    }

    /// The power button: off when on, on (at the last station) when off.
    pub fn toggle_power(&mut self) {
        if self.is_on() {
            self.off();
        } else {
            self.power_on();
        }
    }

    /// Tunes to station `to` (ignored when there is no such station).
    pub fn tune(&mut self, to: usize) {
        if to >= self.channels.len() {
            return;
        }
        let from = self.station();
        if matches!(self.state, RadioState::Playing { .. }) {
            self.out.push(RadioCommand::Stop);
        }
        self.live = false;
        self.blocked = false;
        self.state = RadioState::Tuning { from, to, t: 0.0 };
        self.last = Some(to);
        self.tunes = self.tunes.wrapping_add(1);
    }

    /// Counts every tune so far (for [`crate::synth::AudioParams::tunes`]): each starts a
    /// dial sweep.
    pub fn tunes(&self) -> u32 {
        self.tunes
    }

    /// Advances the tuning phase by `dt` seconds; the new station starts at its end.
    /// After [`NO_SIGNAL_SECONDS`] of "no signal" the radio tunes the next working station.
    pub fn tick(&mut self, dt: f32) {
        if let RadioState::Error { station, .. } = self.state {
            self.error_t += dt.max(0.0);
            if self.error_t >= NO_SIGNAL_SECONDS {
                let next = (station + 1) % self.channels.len();
                if let Some(to) = self.skip_failed(next, 1) {
                    self.tune(to);
                    return;
                }
            }
        }
        if let RadioState::Tuning { from, to, t } = self.state {
            let t = t + dt.max(0.0);
            if t >= TUNE_SECONDS {
                self.state = RadioState::Playing { station: to };
                self.url = 0;
                self.play_url();
            } else {
                self.state = RadioState::Tuning { from, to, t };
            }
        }
    }

    fn play_url(&mut self) {
        let RadioState::Playing { station } = self.state else {
            return;
        };
        self.token = self.token.wrapping_add(1);
        let url = self.channels[station].urls[self.url].clone();
        self.out.push(RadioCommand::Play {
            url,
            token: self.token,
        });
    }

    /// Takes a report from the player. Reports about an earlier `Play` are ignored.
    pub fn on_event(&mut self, event: PlayerEvent) {
        let RadioState::Playing { station } = self.state else {
            return;
        };
        match event {
            PlayerEvent::Started { token } if token == self.token => {
                self.live = true;
                self.blocked = false;
                let id = &self.channels[station].id;
                self.failed.retain(|f| f != id);
            }
            PlayerEvent::Blocked { token } if token == self.token => self.blocked = true,
            PlayerEvent::Failed { token } if token == self.token => {
                self.live = false;
                self.blocked = false;
                self.url += 1;
                if self.url < self.channels[station].urls.len() {
                    self.play_url();
                } else {
                    self.state = RadioState::Error {
                        station,
                        reason: ErrorReason::NoSignal,
                    };
                    self.error_t = 0.0;
                    let id = &self.channels[station].id;
                    if !self.failed.contains(id) {
                        self.failed.push(id.clone());
                    }
                    self.out.push(RadioCommand::Stop);
                }
            }
            _ => {}
        }
    }

    /// The commands for the player since the last call, oldest first.
    pub fn drain_commands(&mut self) -> std::vec::Drain<'_, RadioCommand> {
        self.out.drain(..)
    }

    /// The token of the latest `Play`.
    pub fn token(&self) -> u32 {
        self.token
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RadioCommand::{Play, Stop, Volume};

    fn channel(id: &str, urls: &[&str]) -> Channel {
        Channel {
            id: id.into(),
            name: id.to_uppercase(),
            format: "test".into(),
            urls: urls.iter().map(|u| u.to_string()).collect(),
        }
    }

    fn radio() -> Radio {
        Radio::new(vec![
            channel("a", &["https://a/1", "https://a/2"]),
            channel("b", &["https://b/1"]),
            channel("c", &["https://c/1"]),
        ])
    }

    #[derive(Clone, Copy, Debug)]
    enum In {
        Next,
        Prev,
        Off,
        Power,
        Tick(f32),
        /// A report about the latest Play (or, with a nonzero offset, an older one).
        Started(u32),
        Failed(u32),
        Blocked(u32),
    }

    /// Short state names for the table.
    fn kind(s: RadioState) -> String {
        match s {
            RadioState::Off => "off".into(),
            RadioState::Tuning { from, to, .. } => format!("tuning {from:?}->{to}"),
            RadioState::Playing { station } => format!("playing {station}"),
            RadioState::Error { station, .. } => format!("error {station}"),
        }
    }

    fn play(url: &str, token: u32) -> RadioCommand {
        Play {
            url: url.into(),
            token,
        }
    }

    #[test]
    fn switching_table() {
        use In::*;
        // (input, state after, commands it sent, whether it tuned: one dial sweep)
        let table: Vec<(In, &str, Vec<RadioCommand>, bool)> = vec![
            (Tick(1.0), "off", vec![], false),
            (Started(0), "off", vec![], false),
            (Off, "off", vec![], false),
            (Next, "tuning None->0", vec![], true),
            (Tick(0.3), "tuning None->0", vec![], false),
            (Tick(0.25), "playing 0", vec![play("https://a/1", 1)], false),
            (Started(0), "playing 0", vec![], false),
            // Switching stops the old station, then tunes.
            (Next, "tuning Some(0)->1", vec![Stop], true),
            // Pressing again while tuning restarts from the target.
            (Next, "tuning Some(1)->2", vec![], true),
            (Tick(0.5), "playing 2", vec![play("https://c/1", 2)], false),
            (Next, "tuning Some(2)->0", vec![Stop], true),
            (Tick(0.5), "playing 0", vec![play("https://a/1", 3)], false),
            // A stale report is ignored; a fresh failure tries the next URL...
            (Failed(1), "playing 0", vec![], false),
            (Failed(0), "playing 0", vec![play("https://a/2", 4)], false),
            // ...and when none is left the station is in error, silent.
            (Failed(0), "error 0", vec![Stop], false),
            (Failed(0), "error 0", vec![], false),
            // From an error the dial still moves (nothing to stop).
            (Prev, "tuning Some(0)->2", vec![], true),
            (
                Tick(TUNE_SECONDS),
                "playing 2",
                vec![play("https://c/1", 5)],
                false,
            ),
            (Blocked(0), "playing 2", vec![], false),
            (Off, "off", vec![Stop], false),
            // Power resumes the last station; prev from off does too.
            (Power, "tuning None->2", vec![], true),
            (Power, "off", vec![Stop], false),
            (Prev, "tuning None->2", vec![], true),
            (Off, "off", vec![Stop], false),
        ];
        let mut r = radio();
        for (i, (input, want, cmds, tuned)) in table.into_iter().enumerate() {
            let token = r.token();
            let tunes = r.tunes();
            match input {
                Next => r.next(),
                Prev => r.prev(),
                Off => r.off(),
                Power => r.toggle_power(),
                Tick(dt) => r.tick(dt),
                Started(back) => r.on_event(PlayerEvent::Started {
                    token: token - back,
                }),
                Failed(back) => r.on_event(PlayerEvent::Failed {
                    token: token - back,
                }),
                Blocked(back) => r.on_event(PlayerEvent::Blocked {
                    token: token - back,
                }),
            }
            assert_eq!(kind(r.state()), want, "row {i}: {input:?}");
            let got: Vec<RadioCommand> = r.drain_commands().collect();
            assert_eq!(got, cmds, "row {i}: {input:?}");
            assert_eq!(r.tunes() - tunes, u32::from(tuned), "row {i}: {input:?}");
        }
    }

    /// Fails every URL of the station being played.
    fn fail_all(r: &mut Radio) {
        while matches!(r.state(), RadioState::Playing { .. }) {
            let token = r.token();
            r.on_event(PlayerEvent::Failed { token });
        }
    }

    #[test]
    fn no_signal_moves_on_and_the_dial_skips_failed_stations() {
        let mut r = radio();
        r.next();
        r.tick(TUNE_SECONDS);
        fail_all(&mut r);
        assert_eq!(kind(r.state()), "error 0");
        r.tick(NO_SIGNAL_SECONDS * 0.9);
        assert_eq!(kind(r.state()), "error 0", "no signal shows for a while");
        let tunes = r.tunes();
        r.tick(NO_SIGNAL_SECONDS * 0.2);
        assert_eq!(kind(r.state()), "tuning Some(0)->1");
        assert_eq!(r.tunes(), tunes + 1, "moving on sweeps the dial");
        r.tick(TUNE_SECONDS);
        r.drain_commands().for_each(drop);
        // From b the dial skips a both ways.
        r.next();
        r.tick(TUNE_SECONDS);
        assert_eq!(r.station(), Some(2));
        r.next();
        assert_eq!(r.station(), Some(1));
        r.prev();
        assert_eq!(r.station(), Some(2));
        // b and c fail too: nothing left, so "no signal" stays.
        r.tick(TUNE_SECONDS);
        fail_all(&mut r);
        r.tick(NO_SIGNAL_SECONDS);
        r.tick(TUNE_SECONDS);
        fail_all(&mut r);
        assert_eq!(kind(r.state()), "error 1");
        r.tick(NO_SIGNAL_SECONDS * 3.0);
        assert_eq!(kind(r.state()), "error 1");
        // The dial still turns (all failed: plain next), and a station that plays again is
        // no longer skipped.
        r.next();
        r.tick(TUNE_SECONDS);
        assert_eq!(r.station(), Some(2));
        let token = r.token();
        r.on_event(PlayerEvent::Started { token });
        r.next();
        r.next();
        assert_eq!(r.station(), Some(2), "a and b skipped, c plays");
    }

    #[test]
    fn live_and_blocked_follow_the_player() {
        let mut r = radio();
        r.next();
        r.tick(TUNE_SECONDS);
        assert!(!r.is_live());
        let token = r.token();
        r.on_event(PlayerEvent::Blocked { token });
        assert!(r.is_blocked() && !r.is_live());
        r.on_event(PlayerEvent::Started { token });
        assert!(r.is_live() && !r.is_blocked());
        assert_eq!(r.channel().map(|c| c.id.as_str()), Some("a"));
        r.next();
        assert!(!r.is_live(), "switching drops live");
    }

    #[test]
    fn no_stations_stays_off() {
        let mut r = Radio::new(Vec::new());
        r.next();
        r.prev();
        r.toggle_power();
        r.tune(0);
        r.tick(1.0);
        assert_eq!(r.state(), RadioState::Off);
        assert_eq!(r.drain_commands().count(), 0);
    }

    #[test]
    fn volume_is_clamped_and_sent_on_change() {
        let mut r = radio();
        r.set_volume(0.5);
        r.set_volume(0.5);
        r.set_volume(3.0);
        r.set_volume(f32::NAN);
        r.set_volume(-1.0);
        let got: Vec<_> = r.drain_commands().collect();
        assert_eq!(got, [Volume(0.5), Volume(1.0), Volume(0.0)]);
        assert_eq!(r.volume(), 0.0);
    }

    #[test]
    fn restore_and_set_channels_keep_stations_by_id() {
        let mut r = radio();
        r.restore("b");
        r.restore("nope");
        assert_eq!(r.last_id(), Some("b"));
        r.power_on();
        r.tick(1.0);
        assert_eq!(r.state(), RadioState::Playing { station: 1 });
        r.drain_commands().for_each(drop);
        // "a" disabled: "b" moves to index 0 and keeps playing.
        r.set_channels(vec![channel("b", &["https://b/1"]), channel("c", &["x"])]);
        assert_eq!(r.state(), RadioState::Playing { station: 0 });
        assert_eq!(r.last_id(), Some("b"));
        assert_eq!(r.drain_commands().count(), 0);
        // "b" gone too: the radio turns off.
        r.set_channels(vec![channel("c", &["x"])]);
        assert_eq!(r.state(), RadioState::Off);
        assert_eq!(r.drain_commands().collect::<Vec<_>>(), [Stop]);
        assert_eq!(r.last_id(), None);
        r.next();
        assert_eq!(r.station(), Some(0));
    }

    #[test]
    fn files_stations_have_no_channel() {
        let s = crate::Station {
            id: "mine".into(),
            name: "Mine".into(),
            era: "1980s".into(),
            format: "mine".into(),
            source: crate::Source::Files {
                folder_hint: "x".into(),
            },
        };
        assert_eq!(Channel::from_station(&s), None);
    }
}
