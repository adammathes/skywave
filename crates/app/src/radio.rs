//! The car radio as the game uses it: the built-in presets, the [`audio::Radio`] tuned to
//! the scenario's preset (less the stations the user switched off, plus the streams they
//! added), the station keys, the HUD toast and the dash display's scrolling text. The
//! player's own music is the deck's ([`crate::deck`]).
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
pub const PRESETS: [(&str, &str); 4] = [
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
    (
        "2001_basin",
        include_str!("../../../assets/radio/2001_basin.ron"),
    ),
];

/// Each preset's title, as its file gives it (checked by a test).
pub const PRESET_TITLES: [&str; PRESETS.len()] = [
    "1984 SoCal",
    "1977 Chicago",
    "1986 North Shore",
    "2001 Basin",
];

/// The first scenario, whose preset a radio starts on (`docs/SCENARIOS.md`); a world's own
/// comes with [`CarRadio::set_scenario`].
pub const SCENARIO: &str = "socal_beach_1984";
const SCENARIO_PRESET: &str = "1984_socal";

/// Seconds the station toast stays up.
pub const TOAST_SECONDS: f32 = 3.0;
/// Characters per second the dash display scrolls a name that does not fit.
const SCROLL_CHARS_PER_S: f32 = 3.0;
/// Blank characters between the end of a scrolling name and its start.
const SCROLL_GAP: usize = 3;

/// A row of the stations page: a preset stream station or an added stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationRow {
    pub id: String,
    pub name: String,
}

pub struct CarRadio {
    /// The world's scenario id and its preset: the settings remember a preset per scenario.
    scenario: (&'static str, &'static str),
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
    /// What the station playing says is on (its stream's ICY title, where the platform
    /// reads them), and a count of its changes.
    now_playing: Option<String>,
    titles: u32,
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
            scenario: (SCENARIO, SCENARIO_PRESET),
            presets,
            preset,
            radio: Radio::new(Vec::new()),
            rows: Vec::new(),
            started: false,
            toast: 0.0,
            shown_s: 0.0,
            wants_url: false,
            now_playing: None,
            titles: 0,
            gain: 1.0,
        };
        radio.apply(&RadioSettings::default());
        radio
    }

    pub fn radio(&self) -> &Radio {
        &self.radio
    }

    /// The world is scenario `id`, whose own preset is `preset` (one of [`PRESETS`]): the
    /// dial takes the preset picked for it, else that one, from the next
    /// [`CarRadio::apply`].
    pub fn set_scenario(&mut self, id: &'static str, preset: &'static str) {
        self.scenario = (id, preset);
    }

    /// The world's scenario id and its own preset.
    pub fn scenario(&self) -> (&'static str, &'static str) {
        self.scenario
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
        let (id, own) = self.scenario;
        self.preset = preset_index(s.preset_for(id, own));
        let preset = &self.presets[self.preset];
        let all = RadioSettings {
            disabled: Vec::new(),
            ..s.clone()
        };
        self.rows = all
            .channels(preset)
            .into_iter()
            .map(|c: Channel| StationRow {
                id: c.id,
                name: c.name,
            })
            .collect();
        self.radio.set_channels(s.channels(preset));
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
        }
    }

    /// The station keys of one frame. Returns whether the power or the station changed.
    pub fn press(&mut self, input: &InputState) -> bool {
        let before = (self.radio.is_on(), self.radio.station());
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

    /// Turns the radio on at the last station (the source switched to it), with its toast.
    pub fn power_on(&mut self) {
        self.radio.power_on();
        if self.radio.channels().is_empty() {
            self.toast = TOAST_SECONDS;
        } else {
            self.on_tuned();
        }
    }

    /// Turns the radio off without a toast (the source switched away).
    pub fn off(&mut self) {
        self.radio.off();
        self.toast = 0.0;
    }

    /// What the playing station's stream says is on (`None`: nothing, or not read).
    pub fn set_now_playing(&mut self, title: Option<&str>) {
        let title = title.map(audio::music::ascii).filter(|t| !t.is_empty());
        if title != self.now_playing {
            self.now_playing = title;
            self.titles = self.titles.wrapping_add(1);
        }
    }

    /// The playing station's title, and the count of its changes.
    pub fn now_playing(&self) -> (Option<&str>, u32) {
        (self.now_playing.as_deref(), self.titles)
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
        self.radio.on_event(event);
        if !was_error && matches!(self.radio.state(), RadioState::Error { .. }) {
            self.toast = TOAST_SECONDS;
        }
    }

    /// Seconds since the station on the dial changed (or the radio was switched).
    pub fn shown_s(&self) -> f32 {
        self.shown_s
    }

    /// The commands for the platform's player since the last call.
    pub fn commands(&mut self) -> std::vec::Drain<'_, RadioCommand> {
        self.radio.drain_commands()
    }

    /// Writes the power and last station into `s`; true when they changed.
    pub fn store(&self, s: &mut RadioSettings) -> bool {
        let on = self.radio.is_on();
        let last = self.radio.last_id();
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

    /// What the dash display shows `chars` characters at a time: the station name
    /// (scrolling when it does not fit), or "NO SIGNAL"; nothing while off.
    pub fn dash(&self, chars: usize) -> Option<DashText<'_>> {
        let c = self.radio.channel()?;
        let text = if matches!(self.radio.state(), RadioState::Error { .. }) {
            "NO SIGNAL"
        } else {
            c.name.as_str()
        };
        Some(DashText::scrolled(text, chars, self.shown_s))
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

/// Picks preset `index` of [`PRESETS`] for the scenario `(id, its own preset)` (its own
/// is kept as no entry at all).
pub fn choose_preset(s: &mut RadioSettings, scenario: (&str, &str), index: usize) {
    let id = PRESETS[index.min(PRESETS.len() - 1)].0;
    if id == scenario.1 {
        s.presets.remove(scenario.0);
    } else {
        s.presets.insert(scenario.0.to_string(), id.to_string());
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

impl<'a> DashText<'a> {
    /// `text` in a window of `chars`, `shown_s` seconds after it came up: the start held
    /// for a second, then scrolling a character at a time when it does not fit.
    pub fn scrolled(text: &'a str, chars: usize, shown_s: f32) -> DashText<'a> {
        let len = text.len();
        let start = if len <= chars {
            0
        } else {
            let t = (shown_s - 1.0).max(0.0);
            (t * SCROLL_CHARS_PER_S) as usize % (len + SCROLL_GAP)
        };
        DashText { text, start, chars }
    }
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
    fn each_world_keeps_its_own_preset() {
        let mut r = CarRadio::new();
        let mut s = RadioSettings::default();
        choose_preset(&mut s, r.scenario(), 1);
        r.apply(&s);
        assert_eq!(r.preset_index(), 1);
        // Another world starts on its own preset, and a pick there is its own.
        r.set_scenario("mirage_valley_2001", "1986_north_shore");
        r.apply(&s);
        assert_eq!(r.preset_index(), 2);
        choose_preset(&mut s, r.scenario(), 0);
        r.apply(&s);
        assert_eq!(r.preset_index(), 0);
        assert_eq!(
            s.presets.get(SCENARIO).map(String::as_str),
            Some("1977_chicago")
        );
        choose_preset(&mut s, r.scenario(), 2);
        assert!(
            !s.presets.contains_key("mirage_valley_2001"),
            "its own is no entry"
        );
        r.set_scenario(SCENARIO, SCENARIO_PRESET);
        r.apply(&s);
        assert_eq!(r.preset_index(), 1);
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
