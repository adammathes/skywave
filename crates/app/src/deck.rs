//! The deck as the game uses it: the player's music library (the folders the platform
//! handed over, indexed by [`audio::deck::Library::build`]), the [`audio::deck::Deck`]
//! transport on the radio's keys while the deck is the source, its toast, and what the
//! corner readout and the cockpit dash show of it.
//!
//! What the deck is follows the world's era ([`Kind::for_year`] of the scenario's `year`):
//! a tape deck in the 1980s worlds, a CD player in 2001, an MP3 player from 2010. Like the
//! radio it starts silent every session: the last session's mode, track and second are
//! restored, and nothing plays until a key.

use audio::deck::{Deck, Folder, Kind, Library, Mode};
use audio::{DeckSettings, PlayerEvent, RadioCommand};

use crate::input::{Action, InputState};
use crate::radio::{DashText, TOAST_SECONDS};

/// Settings keep the second into a playing track at this granularity (exact when
/// paused), so a playing deck saves the settings at most this often, s.
const SAVE_EVERY_S: f32 = 5.0;

/// What the deck's toast says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckToast {
    /// A new album (tape or disc) went in.
    Album,
    Mode,
    /// The deck became the source.
    Source,
    /// There is no music to play.
    Empty,
    /// Every track failed to play.
    Failed,
}

pub struct CarDeck {
    deck: Deck,
    kind: Kind,
    /// The folders the platform handed over, in the order they came.
    folders: Vec<Folder>,
    /// Ids of folders the user removed, for the platform to forget ("*": all of them).
    forgotten: Vec<String>,
    /// The platform should ask the user for a music folder.
    wants_music: bool,
    started: bool,
    toast: Option<(DeckToast, f32)>,
}

impl Default for CarDeck {
    fn default() -> CarDeck {
        CarDeck::new()
    }
}

impl CarDeck {
    /// An empty, stopped deck of the first world's era (a tape deck in 1984).
    pub fn new() -> CarDeck {
        CarDeck {
            deck: Deck::new(),
            kind: Kind::for_year(crate::scenario::get(0).year),
            folders: Vec::new(),
            forgotten: Vec::new(),
            wants_music: false,
            started: false,
            toast: None,
        }
    }

    pub fn deck(&self) -> &Deck {
        &self.deck
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// The world is of `year`: the deck is that era's.
    pub fn set_year(&mut self, year: u16) {
        self.kind = Kind::for_year(year);
    }

    /// The library `old` has, in this deck too (a world loaded after the platform handed
    /// the folders over: it does so only when they are picked or at start-up).
    pub fn carry_folders(&mut self, old: &CarDeck) {
        self.folders = old.folders.clone();
        self.rebuild();
    }

    /// Picks up the last session's mode, track and second (once). Nothing plays.
    pub fn start(&mut self, s: &DeckSettings) {
        if self.started {
            return;
        }
        self.started = true;
        self.deck.set_mode(s.mode);
        if let Some(key) = &s.track {
            self.deck.restore(key, s.at_s, s.pass);
        }
    }

    /// Picks up `s` again (the settings were loaded after the first frame), unless a key
    /// has played something.
    pub fn reload(&mut self, s: &DeckSettings) {
        if !self.deck.is_playing() {
            self.started = false;
            self.start(s);
        }
    }

    /// Puts folder `folder` in the library (replacing the one with its id).
    pub fn add_folder(&mut self, folder: Folder) {
        self.folders.retain(|f| f.id != folder.id);
        self.folders.push(folder);
        self.rebuild();
    }

    /// Takes folder `id` out of the library for good: the platform forgets it.
    pub fn remove_folder(&mut self, id: &str) {
        let before = self.folders.len();
        self.folders.retain(|f| f.id != id);
        if self.folders.len() != before {
            self.forgotten.push(id.to_string());
            self.rebuild();
        }
    }

    /// Empties the library, and has the platform forget every folder (remembered ones that
    /// never came back too).
    pub fn clear(&mut self) {
        self.folders.clear();
        self.forgotten.push("*".to_string());
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.deck.set_library(Library::build(&self.folders));
    }

    /// The ids of the folders removed since the last call ("*": forget them all).
    pub fn take_forgotten(&mut self) -> Vec<String> {
        std::mem::take(&mut self.forgotten)
    }

    /// Asks the platform for a music folder (settings, "Music library").
    pub fn request_music(&mut self) {
        self.wants_music = true;
    }

    pub fn take_music_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_music)
    }

    /// The transport keys of one frame while the deck is the source: play/pause on the
    /// power key, tracks on next/previous, albums, the mode. Returns whether one was
    /// pressed.
    pub fn press(&mut self, input: &InputState) -> bool {
        let pressed = |a| input.just_pressed(a);
        let empty = self.deck.library().is_empty();
        if pressed(Action::RadioOff) {
            self.deck.toggle();
        } else if pressed(Action::RadioNext) {
            self.deck.next();
        } else if pressed(Action::RadioPrev) {
            self.deck.prev();
        } else if pressed(Action::AlbumNext) {
            self.deck.next_album();
            self.show(DeckToast::Album);
        } else if pressed(Action::AlbumPrev) {
            self.deck.prev_album();
            self.show(DeckToast::Album);
        } else if pressed(Action::DeckMode) {
            self.deck.cycle_mode();
            self.show(DeckToast::Mode);
        } else {
            return false;
        }
        if empty {
            self.show(DeckToast::Empty);
        }
        true
    }

    /// Plays album `a` (the Library page picked it).
    pub fn pick_album(&mut self, a: usize) {
        self.deck.pick_album(a);
        self.show(DeckToast::Album);
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.deck.set_mode(mode);
    }

    /// The deck became the source: it plays (with its toast).
    pub fn select(&mut self) {
        self.deck.play();
        self.show(if self.deck.library().is_empty() {
            DeckToast::Empty
        } else {
            DeckToast::Source
        });
    }

    /// The radio became the source: the deck stops where it is.
    pub fn deselect(&mut self) {
        self.deck.pause();
        self.toast = None;
    }

    fn show(&mut self, t: DeckToast) {
        self.toast = Some((t, TOAST_SECONDS));
    }

    /// Runs the deck's clock and the toast by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        self.deck.tick(dt);
        if let Some((_, t)) = &mut self.toast {
            *t -= dt;
            if *t <= 0.0 {
                self.toast = None;
            }
        }
    }

    /// A report from the platform's player (the radio's are ignored by their tokens).
    pub fn on_event(&mut self, event: PlayerEvent) {
        let failed = self.deck.has_failed();
        self.deck.on_event(event);
        if !failed && self.deck.has_failed() {
            self.show(DeckToast::Failed);
        }
    }

    /// The commands for the platform's player since the last call.
    pub fn commands(&mut self) -> std::vec::Drain<'_, RadioCommand> {
        self.deck.drain_commands()
    }

    /// Writes the mode, the track loaded, the second into it (to [`SAVE_EVERY_S`] while
    /// playing) and the Shuffle pass into `s`; true when they changed.
    pub fn store(&self, s: &mut DeckSettings) -> bool {
        let Some(track) = self.deck.track() else {
            // Nothing loaded yet (the library still on its way): keep what was saved.
            if s.mode == self.deck.mode() {
                return false;
            }
            s.mode = self.deck.mode();
            return true;
        };
        let at = self.deck.position_s();
        let at = if self.deck.is_playing() {
            (at / SAVE_EVERY_S).floor() * SAVE_EVERY_S
        } else {
            at.floor()
        };
        let pass = self.deck.pass();
        let same = s.mode == self.deck.mode()
            && s.track.as_deref() == Some(track.key.as_str())
            && s.at_s == at
            && s.pass == pass;
        if same {
            return false;
        }
        s.mode = self.deck.mode();
        s.track = Some(track.key.clone());
        s.at_s = at;
        s.pass = pass;
        true
    }

    /// The toast's two lines, while it is up: `(title, detail)` pieces.
    pub fn toast(&self) -> Option<DeckToastLines<'_>> {
        if self.deck.is_blocked() {
            return Some(DeckToastLines {
                kind: self.kind,
                toast: None,
                deck: &self.deck,
            });
        }
        let (t, _) = self.toast?;
        Some(DeckToastLines {
            kind: self.kind,
            toast: Some(t),
            deck: &self.deck,
        })
    }

    /// The readout's three lines: artist, album and song of the track loaded (or what to
    /// do when there is no music).
    pub fn lines(&self) -> [&str; 3] {
        if self.deck.library().is_empty() {
            return ["No music yet", "Settings > Audio", "> Music library"];
        }
        match (self.deck.track(), self.deck.album()) {
            (Some(t), Some((_, a))) => [&t.artist, &a.title, &t.title],
            _ => ["", "", ""],
        }
    }

    /// The readout's status beside the badge: the mode unless Normal, then the transport
    /// state.
    pub fn status(&self) -> [&'static str; 2] {
        let mode = match self.deck.mode() {
            Mode::Normal => "",
            Mode::Shuffle => "SHUFFLE ",
            Mode::Continuous => "CONTINUOUS ",
        };
        let state = if self.deck.is_blocked() {
            "PRESS A KEY"
        } else if self.deck.is_playing() {
            ">"
        } else if self.deck.has_failed() {
            "NO PLAY"
        } else {
            "||"
        };
        [mode, state]
    }

    /// What the dash display shows `chars` at a time `shown_s` after the track came up:
    /// "Artist - Title", scrolling; nothing without a track loaded.
    pub fn dash(&self, chars: usize, shown_s: f32) -> Option<DashText<'_>> {
        let t = self.deck.track()?;
        Some(DashText::scrolled(&t.label, chars, shown_s))
    }
}

/// The deck's toast: what it says.
#[derive(Clone, Copy, Debug)]
pub struct DeckToastLines<'a> {
    kind: Kind,
    /// `None`: the browser waits for a key before it plays.
    toast: Option<DeckToast>,
    deck: &'a Deck,
}

impl core::fmt::Display for DeckToastLines<'_> {
    /// The title line ("TAPE  Artist - Album").
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let badge = self.kind.badge();
        match (self.toast, self.deck.album()) {
            (None, _) => f.write_str("Press any key for music"),
            (Some(DeckToast::Empty), _) => write!(f, "{badge}  No music"),
            (Some(DeckToast::Failed), _) => write!(f, "{badge}  No track played"),
            (Some(DeckToast::Mode), _) => write!(f, "{badge}  {}", self.deck.mode().name()),
            (Some(_), Some((_, a))) => write!(f, "{badge}  {} - {}", a.artist, a.title),
            (Some(_), None) => f.write_str(badge),
        }
    }
}

impl DeckToastLines<'_> {
    /// The second (dim) line.
    pub fn detail(&self) -> DeckToastDetail<'_> {
        DeckToastDetail(self)
    }
}

pub struct DeckToastDetail<'a>(&'a DeckToastLines<'a>);

impl core::fmt::Display for DeckToastDetail<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let l = self.0;
        match (l.toast, l.deck.album()) {
            (Some(DeckToast::Empty), _) => f.write_str("Add one: Settings > Audio > Music library"),
            (Some(DeckToast::Failed), _) => f.write_str("The files did not play"),
            (Some(DeckToast::Mode), _) => f.write_str(match l.deck.mode() {
                Mode::Normal => "Album by album",
                Mode::Shuffle => "The whole library, no repeats",
                Mode::Continuous => "Already playing, like a station",
            }),
            (Some(DeckToast::Album | DeckToast::Source), Some((_, a))) => {
                if l.kind == Kind::Tape {
                    f.write_str("Side A  ")?;
                }
                if let Some(y) = a.year {
                    write!(f, "{y}  ")?;
                }
                let s = if a.len == 1 { "" } else { "s" };
                write!(f, "{} track{s}", a.len)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::deck::{FileInfo, Source};
    use audio::music::Probe;

    /// Two albums by one artist, laid out as a music app exports them.
    pub(crate) fn library_folder() -> Folder {
        let file = |path: &str| FileInfo {
            key: format!("lib/{path}"),
            path: path.to_string(),
            probe: Probe {
                secs: Some(120.0),
                ..Probe::default()
            },
        };
        Folder {
            id: "lib".into(),
            name: "Music".into(),
            mixtape: false,
            files: vec![
                file("Night Shift/Coastline/01 Sunset Drive.mp3"),
                file("Night Shift/Coastline/02 Palm Shadows.mp3"),
                file("Night Shift/Harbor Lights/01 Pier.mp3"),
            ],
        }
    }

    fn press(d: &mut CarDeck, action: Action) -> bool {
        let mut input = InputState::default();
        input.set(action, 1.0);
        input.set_just_pressed(action, true);
        d.press(&input)
    }

    fn played(d: &mut CarDeck) -> Vec<String> {
        d.commands()
            .filter_map(|c| match c {
                RadioCommand::PlayFile { key, .. } => Some(key),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_deck_follows_the_era() {
        let mut d = CarDeck::default();
        assert_eq!(d.kind(), Kind::Tape);
        d.set_year(2001);
        assert_eq!(d.kind(), Kind::Cd);
        assert_eq!(Kind::for_year(2012).badge(), "MP3");
    }

    #[test]
    fn keys_drive_the_transport_and_the_toast() {
        let mut d = CarDeck::default();
        // Nothing to play: the keys say so.
        assert!(press(&mut d, Action::RadioOff));
        assert_eq!(d.toast().unwrap().to_string(), "TAPE  No music");
        assert_eq!(d.lines()[0], "No music yet");
        d.add_folder(library_folder());
        assert!(!d.deck().is_playing(), "a folder arriving plays nothing");
        assert_eq!(d.lines(), ["Night Shift", "Coastline", "Sunset Drive"]);
        press(&mut d, Action::RadioOff);
        assert_eq!(
            played(&mut d),
            ["lib/Night Shift/Coastline/01 Sunset Drive.mp3"]
        );
        press(&mut d, Action::RadioNext);
        assert_eq!(d.lines()[2], "Palm Shadows");
        press(&mut d, Action::AlbumNext);
        let t = d.toast().unwrap();
        assert_eq!(t.to_string(), "TAPE  Night Shift - Harbor Lights");
        assert_eq!(t.detail().to_string(), "Side A  1 track");
        press(&mut d, Action::DeckMode);
        assert_eq!(d.toast().unwrap().to_string(), "TAPE  Shuffle");
        assert_eq!(d.status(), ["SHUFFLE ", ">"]);
        assert!(!press(&mut d, Action::Horn));
        assert_eq!(d.dash(40, 0.0).unwrap().to_string(), "Night Shift - Pier");
        d.tick(TOAST_SECONDS);
        assert!(d.toast().is_none());
    }

    #[test]
    fn settings_round_trip_the_place() {
        let mut d = CarDeck::default();
        d.start(&DeckSettings::default());
        d.add_folder(library_folder());
        press(&mut d, Action::RadioNext);
        let token = d.deck().token();
        d.on_event(PlayerEvent::Started { token });
        d.tick(12.7);
        let mut s = DeckSettings {
            source: Source::Deck,
            ..DeckSettings::default()
        };
        assert!(d.store(&mut s));
        assert_eq!(s.at_s, 10.0, "to five seconds while playing");
        press(&mut d, Action::RadioOff);
        assert!(d.store(&mut s));
        assert_eq!(s.at_s, 12.0, "to the second when paused");
        assert!(!d.store(&mut s));
        let key = s.track.clone().unwrap();
        assert!(key.ends_with("02 Palm Shadows.mp3"));
        // Next session: the same track and second, silent until a key.
        let mut e = CarDeck::default();
        e.start(&s);
        e.add_folder(library_folder());
        assert_eq!(e.deck().track().unwrap().key, key);
        assert!(e.commands().next().is_none());
        press(&mut e, Action::RadioOff);
        let cmds: Vec<_> = e.commands().collect();
        assert!(matches!(&cmds[..], [RadioCommand::PlayFile { start_s, .. }] if *start_s == 12.0));
        // Removing the folder empties the deck, and the platform forgets it.
        e.remove_folder("lib");
        assert!(e.deck().library().is_empty() && !e.deck().is_playing());
        assert_eq!(e.take_forgotten(), ["lib"]);
        e.clear();
        assert_eq!(e.take_forgotten(), ["*"]);
    }
}
