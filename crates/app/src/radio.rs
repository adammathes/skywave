//! The car radio as the game uses it: the built-in presets, the [`audio::Radio`] tuned to
//! the scenario's preset (less the stations the user switched off, plus the streams they
//! added and their own music folders), the station keys, the HUD toast and the dash
//! display's scrolling text (a files station shows the track playing).
//!
//! The radio starts with the first frame that runs the game (never in a still scene, so
//! screenshot goldens stay silent and unchanged): volume from the settings, the last
//! station restored, and back on if it was on when the last session ended. The platform
//! drains its [`audio::RadioCommand`]s every frame and reports back with
//! [`audio::PlayerEvent`]s ([`crate::App::radio_commands`], [`crate::App::radio_event`]).

use core::fmt::{self, Display};

use audio::{Channel, PlayerEvent, Radio, RadioCommand, RadioPreset, RadioSettings, RadioState};

use crate::input::{Action, InputState};

/// The presets compiled into the game, as `(id, RON text)`, in the order the settings
/// page offers them.
pub const PRESETS: [(&str, &str); 3] = [
    (
        "1984_socal",
        include_str!("../../../assets/radio/1984_socal.ron"),
    ),
    (
        "1977_chicago",
        include_str!("../../../assets/radio/1977_chicago.ron"),
    ),
    (
        "1986_north_shore",
        include_str!("../../../assets/radio/1986_north_shore.ron"),
    ),
];

/// Each preset's title, as its file gives it (checked by a test).
pub const PRESET_TITLES: [&str; PRESETS.len()] = ["1984 SoCal", "1977 Chicago", "1986 North Shore"];

/// The scenario every place is part of for now, and its preset (`docs/SCENARIOS.md`).
pub const SCENARIO: &str = "socal_beach_1984";
const SCENARIO_PRESET: &str = "1984_socal";

/// Seconds the station toast stays up.
pub const TOAST_SECONDS: f32 = 3.0;
/// Characters per second the dash display scrolls a name that does not fit.
const SCROLL_CHARS_PER_S: f32 = 3.0;
/// Blank characters between the end of a scrolling name and its start.
const SCROLL_GAP: usize = 3;

/// A row of the stations page: a preset stream station, an added stream or a music folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationRow {
    pub id: String,
    pub name: String,
}

pub struct CarRadio {
    presets: Vec<RadioPreset>,
    /// Index into `presets` of the one on the dial.
    preset: usize,
    radio: Radio,
    /// The stations page: every station of the preset and every added stream, enabled or
    /// not.
    rows: Vec<StationRow>,
    started: bool,
    /// Seconds the toast has left.
    toast: f32,
    /// Seconds since the station on the dash was tuned (drives its scrolling).
    shown_s: f32,
    /// The platform should ask the user for a stream URL.
    wants_url: bool,
    /// The platform should ask the user for a music folder.
    wants_music: bool,
    /// The user's music folders, by name, as stations.
    files: Vec<Channel>,
    /// Each folder station's id and the platform's path for it (what it forgets it by).
    paths: Vec<(String, String)>,
    /// Paths of folders the user removed, for the platform to forget ("*": all of them).
    forgotten: Vec<String>,
    /// The last station is a music folder the platform has not handed over yet: it
    /// becomes the station the power key resumes when it arrives, unless a key came first.
    pending: Option<String>,
    /// What the radio volume is scaled by: the master volume, 0 when muted.
    gain: f32,
}

impl Default for CarRadio {
    fn default() -> CarRadio {
        CarRadio::new()
    }
}

impl CarRadio {
    /// An off radio on the scenario's default preset.
    pub fn new() -> CarRadio {
        let presets: Vec<RadioPreset> = PRESETS
            .iter()
            .map(|(_, text)| RadioPreset::parse(text).expect("the built-in presets are valid"))
            .collect();
        let preset = preset_index(SCENARIO_PRESET);
        let mut radio = CarRadio {
            presets,
            preset,
            radio: Radio::new(Vec::new()),
            rows: Vec::new(),
            started: false,
            toast: 0.0,
            shown_s: 0.0,
            wants_url: false,
            wants_music: false,
            files: Vec::new(),
            paths: Vec::new(),
            forgotten: Vec::new(),
            pending: None,
            gain: 1.0,
        };
        radio.apply(&RadioSettings::default());
        radio
    }

    pub fn radio(&self) -> &Radio {
        &self.radio
    }

    #[cfg(test)]
    fn radio_mut(&mut self) -> &mut Radio {
        &mut self.radio
    }

    pub fn preset(&self) -> &RadioPreset {
        &self.presets[self.preset]
    }

    /// Index of the preset on the dial, into [`PRESETS`].
    pub fn preset_index(&self) -> usize {
        self.preset
    }

    pub fn rows(&self) -> &[StationRow] {
        &self.rows
    }

    pub fn started(&self) -> bool {
        self.started
    }

    /// Scales the radio volume by `gain` (the master volume, 0 when muted) from the next
    /// [`CarRadio::apply`] or [`CarRadio::start`] on.
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    /// Follows the settings: the scenario's preset, the enabled stations and added
    /// streams, and once started the volume (times the gain). The station playing keeps
    /// playing if it is still there.
    pub fn apply(&mut self, s: &RadioSettings) {
        self.preset = preset_index(s.preset_for(SCENARIO, SCENARIO_PRESET));
        let preset = &self.presets[self.preset];
        let all = RadioSettings {
            disabled: Vec::new(),
            ..s.clone()
        };
        self.rows = all
            .channels(preset, &self.files)
            .into_iter()
            .map(|c: Channel| StationRow {
                id: c.id,
                name: c.name,
            })
            .collect();
        self.radio.set_channels(s.channels(preset, &self.files));
        if self.started {
            self.radio.set_volume(s.volume * self.gain);
        }
    }

    /// Starts the radio from the settings (once): the last station restored, the radio off.
    /// The radio never comes on by itself: a session starts silent whatever the last one
    /// ended with, and the power key resumes the last station.
    pub fn start(&mut self, s: &RadioSettings) {
        if self.started {
            return;
        }
        self.started = true;
        self.apply(s);
        if let Some(id) = &s.last_station {
            self.radio.restore(id);
            let missing = self.radio.last_id() != Some(id.as_str());
            if missing && id.starts_with("files_") {
                // A music folder the platform is still reading (or must ask for again).
                self.pending = Some(id.clone());
            }
        }
    }

    /// Puts music folder `station` (a [`Channel::files`]) on the dial, replacing the one of
    /// the same path, folders in name order after the streams. The last station of the
    /// previous session, if it is this folder, is what the power key resumes (the radio
    /// stays off: a session starts silent).
    pub fn add_files(&mut self, path: &str, station: Channel, s: &RadioSettings) {
        let id = station.id.clone();
        self.files.retain(|c| c.id != id);
        self.paths.retain(|(i, _)| *i != id);
        self.paths.push((id.clone(), path.to_string()));
        self.files.push(station);
        self.files
            .sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        self.apply(s);
        if self.pending.as_ref() == Some(&id) {
            self.pending = None;
            if !self.radio.is_on() {
                self.radio.restore(&id);
            }
        }
    }

    /// The music folders on the dial (on or off).
    pub fn files(&self) -> &[Channel] {
        &self.files
    }

    /// Takes music folder `id` off the dial for good: the platform forgets it.
    pub fn remove_files(&mut self, id: &str, s: &RadioSettings) {
        self.files.retain(|c| c.id != id);
        if let Some(i) = self.paths.iter().position(|(i, _)| i == id) {
            self.forgotten.push(self.paths.remove(i).1);
        }
        self.apply(s);
    }

    /// Takes every music folder off the dial, and has the platform forget them all
    /// (remembered ones that never came back too).
    pub fn clear_files(&mut self, s: &RadioSettings) {
        self.files.clear();
        self.paths.clear();
        self.pending = None;
        self.forgotten.push("*".to_string());
        self.apply(s);
    }

    /// The paths of the folders removed since the last call ("*": forget them all).
    pub fn take_forgotten(&mut self) -> Vec<String> {
        std::mem::take(&mut self.forgotten)
    }

    /// The station keys of one frame. Returns whether the power or the station changed.
    pub fn press(&mut self, input: &InputState) -> bool {
        let before = (self.radio.is_on(), self.radio.station());
        let any = [Action::RadioNext, Action::RadioPrev, Action::RadioOff]
            .iter()
            .any(|&a| input.just_pressed(a));
        if any {
            // The user tunes: a folder still on its way no longer takes over.
            self.pending = None;
        }
        if input.just_pressed(Action::RadioNext) {
            self.radio.next();
        } else if input.just_pressed(Action::RadioPrev) {
            self.radio.prev();
        } else if input.just_pressed(Action::RadioOff) {
            self.radio.toggle_power();
        } else {
            return false;
        }
        if self.radio.channels().is_empty() {
            // Nothing on the dial: say so.
            self.toast = TOAST_SECONDS;
        } else {
            self.on_tuned();
        }
        before != (self.radio.is_on(), self.radio.station())
    }

    fn on_tuned(&mut self) {
        self.toast = TOAST_SECONDS;
        self.shown_s = 0.0;
    }

    /// Advances the tuning phase, the toast and the dash scroll by `dt` seconds. A station
    /// the radio moved to on its own (after "no signal") gets its toast.
    pub fn tick(&mut self, dt: f32) {
        let before = self.radio.station();
        self.radio.tick(dt);
        self.toast = (self.toast - dt).max(0.0);
        self.shown_s += dt;
        if self.radio.station() != before {
            self.on_tuned();
        }
    }

    /// A report from the platform's player. A station that just failed for good shows its
    /// toast again ("no signal").
    pub fn on_event(&mut self, event: PlayerEvent) {
        let was_error = matches!(self.radio.state(), RadioState::Error { .. });
        let track = self.radio.track().map(|t| t.key.clone());
        self.radio.on_event(event);
        if !was_error && matches!(self.radio.state(), RadioState::Error { .. }) {
            self.toast = TOAST_SECONDS;
        }
        if self.radio.track().map(|t| &t.key) != track.as_ref() {
            // A new track on the dash: start its scroll over.
            self.shown_s = 0.0;
        }
    }

    /// The commands for the platform's player since the last call.
    pub fn commands(&mut self) -> std::vec::Drain<'_, RadioCommand> {
        self.radio.drain_commands()
    }

    /// Writes the power and last station into `s`; true when they changed.
    pub fn store(&self, s: &mut RadioSettings) -> bool {
        let on = self.radio.is_on();
        // Still waiting for the last session's folder: keep it as the last station.
        let last = self.radio.last_id().or(self.pending.as_deref());
        if s.on == on && s.last_station.as_deref() == last {
            return false;
        }
        s.on = on;
        s.last_station = last.map(str::to_string);
        true
    }

    /// Asks the platform for a stream URL (settings, "Add stream URL").
    pub fn request_url(&mut self) {
        self.wants_url = true;
    }

    /// Takes the request for a stream URL.
    pub fn take_url_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_url)
    }

    /// Asks the platform for a music folder (settings, "Add music folder").
    pub fn request_music(&mut self) {
        self.wants_music = true;
    }

    /// Takes the request for a music folder.
    pub fn take_music_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_music)
    }

    /// The toast's text, while it is up.
    pub fn toast(&self) -> Option<ToastLines<'_>> {
        if self.radio.is_blocked() {
            return Some(ToastLines::Blocked);
        }
        if self.toast <= 0.0 {
            return None;
        }
        let n = self.radio.channels().len();
        Some(match (self.radio.state(), self.radio.channel()) {
            (_, _) if n == 0 => ToastLines::Empty,
            (RadioState::Off, _) | (_, None) => ToastLines::Off,
            (RadioState::Error { station, .. }, Some(c)) => ToastLines::NoSignal(station + 1, n, c),
            (_, Some(c)) => {
                let i = self.radio.station().unwrap_or(0);
                ToastLines::Station(i + 1, n, c)
            }
        })
    }

    /// What the dash display shows `chars` characters at a time: the station name, or the
    /// track a files station plays (scrolling when it does not fit), or "NO SIGNAL";
    /// nothing while off.
    pub fn dash(&self, chars: usize) -> Option<DashText<'_>> {
        let c = self.radio.channel()?;
        let text = if matches!(self.radio.state(), RadioState::Error { .. }) {
            "NO SIGNAL"
        } else if let Some(t) = self.radio.track() {
            t.label.as_str()
        } else {
            c.name.as_str()
        };
        let len = text.len();
        let start = if len <= chars {
            0
        } else {
            // Hold the start for a second, then scroll.
            let t = (self.shown_s - 1.0).max(0.0);
            (t * SCROLL_CHARS_PER_S) as usize % (len + SCROLL_GAP)
        };
        Some(DashText { text, start, chars })
    }
}

/// Index into [`PRESETS`] of the preset `id` (the scenario's default if unknown).
fn preset_index(id: &str) -> usize {
    PRESETS
        .iter()
        .position(|(p, _)| *p == id)
        .or_else(|| PRESETS.iter().position(|(p, _)| *p == SCENARIO_PRESET))
        .unwrap_or(0)
}

/// Picks preset `index` of [`PRESETS`] for the scenario (its default is kept as no
/// entry at all).
pub fn choose_preset(s: &mut RadioSettings, index: usize) {
    let id = PRESETS[index.min(PRESETS.len() - 1)].0;
    if id == SCENARIO_PRESET {
        s.presets.remove(SCENARIO);
    } else {
        s.presets.insert(SCENARIO.to_string(), id.to_string());
    }
}

/// The station toast: what it says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToastLines<'a> {
    /// Station `i` of `n` (from 1) being tuned or playing.
    Station(usize, usize, &'a Channel),
    NoSignal(usize, usize, &'a Channel),
    Off,
    /// No station on the dial (all switched off).
    Empty,
    /// The browser waits for a key press before it plays sound.
    Blocked,
}

impl ToastLines<'_> {
    /// The first (bright) line.
    pub fn title(&self) -> impl Display + '_ {
        Line(self, true)
    }

    /// The second (dim) line, possibly empty.
    pub fn detail(&self) -> impl Display + '_ {
        Line(self, false)
    }
}

struct Line<'a>(&'a ToastLines<'a>, bool);

impl Display for Line<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.0, self.1) {
            (ToastLines::Station(i, n, c), true) => write!(f, "{i}/{n}  {}", c.name),
            (ToastLines::Station(_, _, c), false) => f.write_str(&c.format),
            (ToastLines::NoSignal(i, n, c), true) => write!(f, "{i}/{n}  {}", c.name),
            (ToastLines::NoSignal(..), false) => f.write_str("No signal: the stream did not play"),
            (ToastLines::Off, true) => f.write_str("Radio off"),
            (ToastLines::Empty, true) => f.write_str("No stations"),
            (ToastLines::Empty, false) => f.write_str("Turn some on in Settings > Audio"),
            (ToastLines::Blocked, true) => f.write_str("Press any key for radio"),
            (ToastLines::Off | ToastLines::Blocked, false) => Ok(()),
        }
    }
}

/// A window of `chars` characters into `text` from `start`, wrapping round with a gap
/// (the text is ASCII, as station names are).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DashText<'a> {
    pub text: &'a str,
    pub start: usize,
    pub chars: usize,
}

impl Display for DashText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.text.as_bytes();
        if b.len() <= self.chars {
            return f.write_str(self.text);
        }
        let period = b.len() + SCROLL_GAP;
        for k in 0..self.chars {
            let i = (self.start + k) % period;
            let c = b.get(i).map_or(' ', |&c| c as char);
            fmt::Write::write_char(f, c)?;
        }
        Ok(())
    }
}

/// The HUD point (virtual units, HUD space `size`) of normalized device coordinates `ndc`
/// (y up) on a target of width/height `aspect`: the HUD's 16:9 box is letterboxed into the
/// target (`render::HudLayout`), the scene fills it.
pub fn ndc_to_hud(ndc: [f32; 2], aspect: f32, size: [f32; 2]) -> [f32; 2] {
    const BOX: f32 = render::HUD_WIDTH / render::HUD_HEIGHT;
    // In units of the target's height.
    let (tw, th) = (aspect.max(1e-3), 1.0);
    let w = tw.min(th * BOX);
    let h = w / BOX;
    let px = (ndc[0] + 1.0) * 0.5 * tw - (tw - w) * 0.5;
    let py = (1.0 - ndc[1]) * 0.5 * th - (th - h) * 0.5;
    [px / w * size[0], py / h * size[1]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_presets_parse_with_their_titles() {
        let radio = CarRadio::new();
        for (i, (id, _)) in PRESETS.iter().enumerate() {
            let p = &radio.presets[i];
            assert_eq!(p.id, *id);
            assert_eq!(p.title, PRESET_TITLES[i]);
            assert!(p.stations.iter().all(|s| !s.urls().is_empty()));
        }
        assert_eq!(radio.preset().id, "1984_socal");
        assert_eq!(radio.preset().scenario, SCENARIO);
        for p in &radio.presets {
            assert!((5..=7).contains(&p.stations.len()), "{}", p.id);
        }
        // Off until started, and silent.
        assert!(!radio.radio().is_on());
        assert_eq!(radio.rows().len(), radio.radio().channels().len());
    }

    fn press(r: &mut CarRadio, action: Action) -> bool {
        let mut input = InputState::default();
        input.set(action, 1.0);
        input.set_just_pressed(action, true);
        r.press(&input)
    }

    #[test]
    fn keys_tune_and_toast() {
        let mut r = CarRadio::new();
        r.start(&RadioSettings::default());
        let volume: Vec<_> = r.commands().collect();
        assert_eq!(volume, [RadioCommand::Volume(0.8)]);
        assert!(r.toast().is_none());
        assert!(press(&mut r, Action::RadioNext));
        let first = r.radio().channels()[0].clone();
        let t = r.toast().unwrap();
        assert_eq!(t, ToastLines::Station(1, 7, &first));
        assert_eq!(t.title().to_string(), "1/7  Radio Paradise Mellow Mix");
        assert_eq!(t.detail().to_string(), "mellow rock and soft pop");
        assert_eq!(r.radio().tunes(), 1);
        r.tick(audio::TUNE_SECONDS);
        let cmds: Vec<_> = r.commands().collect();
        assert!(matches!(&cmds[..], [RadioCommand::Play { url, .. }] if url == &first.urls[0]));
        r.tick(TOAST_SECONDS);
        assert!(r.toast().is_none());
        // Every URL fails: no signal, toast again, the dash says so.
        for _ in &first.urls {
            let token = r.radio().token();
            r.on_event(PlayerEvent::Failed { token });
        }
        assert!(matches!(r.toast(), Some(ToastLines::NoSignal(1, 7, _))));
        assert_eq!(r.dash(12).unwrap().to_string(), "NO SIGNAL");
        // Then the radio moves on by itself, and the dial skips the failed station.
        r.tick(audio::NO_SIGNAL_SECONDS);
        assert!(matches!(r.toast(), Some(ToastLines::Station(2, 7, _))));
        assert!(press(&mut r, Action::RadioPrev));
        assert_eq!(r.radio().station(), Some(6));
        assert!(press(&mut r, Action::RadioOff));
        assert_eq!(r.toast(), Some(ToastLines::Off));
        assert!(r.dash(12).is_none());
        let mut s = RadioSettings::default();
        assert!(r.store(&mut s));
        assert_eq!(
            (s.on, s.last_station.as_deref()),
            (false, Some("radioparadise_global"))
        );
        assert!(!r.store(&mut s));
    }

    #[test]
    fn start_restores_the_last_station_but_stays_off() {
        let mut r = CarRadio::new();
        let s = RadioSettings {
            on: true,
            last_station: Some("radioparadise_main".into()),
            volume: 0.5,
            ..RadioSettings::default()
        };
        r.start(&s);
        // Off, silent, no toast: nothing plays until the power key, even though the last
        // session ended with the radio on.
        assert!(!r.radio().is_on());
        assert!(r.toast().is_none());
        // Only the volume goes to the player; nothing plays.
        assert!(r
            .commands()
            .all(|c| !matches!(c, RadioCommand::Play { .. })));
        // Power resumes the station that was playing.
        r.radio_mut().toggle_power();
        assert_eq!(
            r.radio().channel().map(|c| c.id.as_str()),
            Some("radioparadise_main")
        );
        // Starting twice does nothing more.
        r.commands().for_each(drop);
        r.start(&RadioSettings::default());
        assert_eq!(r.commands().count(), 0);
    }

    #[test]
    fn settings_pick_the_preset_and_the_stations() {
        let mut r = CarRadio::new();
        let mut s = RadioSettings::default();
        s.presets.insert(SCENARIO.into(), "1986_north_shore".into());
        s.set_enabled("wmxm", false);
        s.add_custom("https://example.org/live").unwrap();
        r.apply(&s);
        assert_eq!(r.preset_index(), 2);
        assert_eq!(r.rows().len(), 7, "every station and the added stream");
        let ids: Vec<&str> = r.radio().channels().iter().map(|c| c.id.as_str()).collect();
        assert!(!ids.contains(&"wmxm") && ids.contains(&"custom_1"));
        s.presets.insert(SCENARIO.into(), "no_such_preset".into());
        r.apply(&s);
        assert_eq!(r.preset_index(), 0);
        // Everything off: the keys say so.
        for row in r.rows().to_vec() {
            s.set_enabled(&row.id, false);
        }
        r.apply(&s);
        assert!(!press(&mut r, Action::RadioNext));
        assert_eq!(r.toast(), Some(ToastLines::Empty));
    }

    #[test]
    fn long_names_scroll_on_the_dash() {
        let mut r = CarRadio::new();
        r.start(&RadioSettings::default());
        press(&mut r, Action::RadioNext);
        // "Radio Paradise Mellow Mix" is 25 characters.
        assert_eq!(r.dash(40).unwrap().to_string(), "Radio Paradise Mellow Mix");
        assert_eq!(r.dash(8).unwrap().to_string(), "Radio Pa");
        r.tick(1.0 + 2.0 / SCROLL_CHARS_PER_S);
        assert_eq!(r.dash(8).unwrap().to_string(), "dio Para");
        let wrap = DashText {
            text: "ABCDE",
            start: 3,
            chars: 4,
        };
        assert_eq!(wrap.to_string(), "DE  ");
        let wrap = DashText { start: 7, ..wrap };
        assert_eq!(wrap.to_string(), " ABC");
    }

    fn folder(name: &str) -> Channel {
        let tracks = (0..3)
            .map(|i| audio::Track {
                key: format!("{name}/{i}"),
                label: format!("Artist - Song {i}"),
                secs: 180.0,
            })
            .collect();
        Channel::files(name, name, tracks).unwrap()
    }

    #[test]
    fn music_folders_join_the_dial_and_the_last_one_resumes() {
        let mine = folder("Yacht");
        let s = RadioSettings {
            on: true,
            last_station: Some(mine.id.clone()),
            ..RadioSettings::default()
        };
        let mut r = CarRadio::new();
        r.start(&s);
        // Off, as every session starts; the folder is not there yet, and the settings keep
        // it as the last station.
        assert!(!r.radio().is_on());
        let mut kept = s.clone();
        r.store(&mut kept);
        assert_eq!(kept.last_station, s.last_station);
        r.add_files("Wave", folder("Wave"), &s);
        r.add_files("Yacht", mine.clone(), &s);
        // Still off and silent; the power key resumes the folder.
        assert!(!r.radio().is_on());
        assert!(r
            .commands()
            .all(|c| !matches!(c, RadioCommand::PlayFile { .. })));
        assert!(press(&mut r, Action::RadioOff));
        assert_eq!(
            r.radio().channel().map(|c| c.id.as_str()),
            Some(mine.id.as_str())
        );
        let ids: Vec<&str> = r.rows().iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            &ids[7..],
            [folder("Wave").id.as_str(), mine.id.as_str()],
            "by name, after the streams"
        );
        assert_eq!(
            r.dash(40).unwrap().to_string(),
            "Yacht",
            "the folder's name while tuning"
        );
        r.tick(audio::TUNE_SECONDS);
        let cmds: Vec<_> = r.commands().collect();
        assert!(
            cmds.iter().any(
                |c| matches!(c, RadioCommand::PlayFile { key, .. } if key.starts_with("Yacht/"))
            ),
            "{cmds:?}"
        );
        // Playing: the dash shows the track.
        assert!(r
            .dash(40)
            .unwrap()
            .to_string()
            .starts_with("Artist - Song "));
        // A folder the user switched off is not on the dial, but still on the stations page.
        let mut off = s.clone();
        off.set_enabled(&mine.id, false);
        r.apply(&off);
        assert!(r.radio().channels().iter().all(|c| c.id != mine.id));
        assert!(r.rows().iter().any(|row| row.id == mine.id));
        // Removed: off the stations page too, and the platform forgets its path.
        r.remove_files(&mine.id, &s);
        assert!(r.rows().iter().all(|row| row.id != mine.id));
        assert_eq!(r.take_forgotten(), ["Yacht"]);
        r.clear_files(&s);
        assert!(r.files().is_empty());
        assert_eq!(r.take_forgotten(), ["*"]);
        assert!(r.take_forgotten().is_empty());
    }

    #[test]
    fn a_key_before_the_folder_arrives_wins() {
        let mine = folder("Yacht");
        let s = RadioSettings {
            on: true,
            last_station: Some(mine.id.clone()),
            ..RadioSettings::default()
        };
        let mut r = CarRadio::new();
        r.start(&s);
        press(&mut r, Action::RadioNext);
        let first = r.radio().station();
        r.add_files("Yacht", mine, &s);
        assert_eq!(
            r.radio().station(),
            first,
            "the folder arriving does not retune"
        );
    }

    #[test]
    fn ndc_maps_onto_the_letterboxed_hud() {
        let size = [1280.0, 720.0];
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
        // 16:9: corners to corners, y down.
        assert!(close(ndc_to_hud([-1.0, 1.0], 16.0 / 9.0, size), [0.0, 0.0]));
        assert!(close(
            ndc_to_hud([1.0, -1.0], 16.0 / 9.0, size),
            [1280.0, 720.0]
        ));
        // 16:10: the HUD box has bars above and below, so the scene's top edge is above it.
        let top = ndc_to_hud([0.0, 1.0], 1.6, size);
        assert!(close(top, [640.0, -40.0]), "{top:?}");
        // 21:9: bars at the sides.
        let left = ndc_to_hud([-1.0, 0.0], 21.0 / 9.0, size);
        assert!(close(left, [-200.0, 360.0]), "{left:?}");
    }
}
