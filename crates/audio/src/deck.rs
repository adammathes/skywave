//! The deck: the player's own music as a second source beside the radio, a tape deck, a
//! CD player or an MP3 player by the world's era ([`Kind`]).
//!
//! The platform hands over the folders the user picked or dropped ([`Folder`]: each file's
//! key, path in the folder and what [`crate::music::probe`] read of it); [`Library::build`]
//! indexes them as Artist › Album › tracks, from the folder layout (`Artist/Album/NN
//! Title.mp3`, the common music-app export) with the tags overriding it. An album is a
//! tape or a disc.
//!
//! [`Deck`] is the transport: play/pause, next/previous track (previous within
//! [`PREV_RESTART_S`] of a track's start goes back a track, later it restarts the track),
//! next/previous album, picking an album, and three [`Mode`]s. It sends the same
//! [`RadioCommand`]s as the radio (`PlayFile`, `Stop`) to the same player, with tokens of its
//! own (the high bit set) so each ignores the other's reports. Everything is plain integer
//! and float arithmetic over the library: the same files give the same order everywhere.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::music::{ascii, number, seed, Playlist, Probe, Track, MAX_TRACKS, NOMINAL_TRACK_S};
use crate::radio::{PlayerEvent, RadioCommand};

/// Previous within this many seconds of a track's start goes to the previous track; later
/// it restarts the track.
pub const PREV_RESTART_S: f32 = 3.0;
/// The album of the files of a drop of loose files.
pub const MIXTAPE: &str = "Mixtape";
const UNKNOWN_ARTIST: &str = "Unknown Artist";
const VARIOUS: &str = "Various Artists";
/// The deck's tokens have this bit set; the radio's never reach it.
const TOKEN_BIT: u32 = 1 << 31;

/// What the car plays: the radio or the deck.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Radio,
    Deck,
}

/// The order the deck plays the library in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Album order, then the next album in artist order.
    #[default]
    Normal,
    /// The whole library in a seeded shuffle, no repeats until every track has played.
    Shuffle,
    /// The whole library as an already-playing station: pressing play (or coming back to
    /// the deck) lands where its broadcast would be, mid-track.
    Continuous,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Normal, Mode::Shuffle, Mode::Continuous];

    /// The mode after this one, wrapping.
    pub fn next(self) -> Mode {
        match self {
            Mode::Normal => Mode::Shuffle,
            Mode::Shuffle => Mode::Continuous,
            Mode::Continuous => Mode::Normal,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Normal => "Normal",
            Mode::Shuffle => "Shuffle",
            Mode::Continuous => "Continuous",
        }
    }
}

/// What the deck is, by the world's year.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Worlds up to 1989.
    Tape,
    /// 1990 to 2009.
    Cd,
    /// From 2010.
    Mp3,
}

impl Kind {
    pub fn for_year(year: u16) -> Kind {
        match year {
            ..=1989 => Kind::Tape,
            1990..=2009 => Kind::Cd,
            _ => Kind::Mp3,
        }
    }

    /// Its badge on the HUD.
    pub fn badge(self) -> &'static str {
        match self {
            Kind::Tape => "TAPE",
            Kind::Cd => "CD",
            Kind::Mp3 => "MP3",
        }
    }
}

/// One audio file of a folder, as the platform found it.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    /// What the platform plays it by.
    pub key: String,
    /// Its path in the folder, `/` between names, the file name last.
    pub path: String,
    pub probe: Probe,
}

/// A folder the user picked or dropped (or a drop of loose files: a mixtape).
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    /// The platform's stable id for it.
    pub id: String,
    /// Its own name (an album's, when the user picked one album).
    pub name: String,
    /// Loose files: one album called [`MIXTAPE`] whatever their tags say.
    pub mixtape: bool,
    pub files: Vec<FileInfo>,
}

/// An album of the library: a tape or a disc. Its tracks are
/// `Library::tracks[first..first + len]`, in play order.
#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    /// Printable ASCII, never empty.
    pub artist: String,
    pub title: String,
    pub year: Option<u16>,
    pub first: usize,
    pub len: usize,
}

/// The player's music: every track in Normal order (album by album, albums by artist, year
/// and title), and the albums.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    /// The album of each track.
    album_of: Vec<u32>,
    /// Seeds the shuffles: a hash of every track's key.
    seed: u64,
}

/// A file on its way into an album.
struct Entry<'a> {
    file: &'a FileInfo,
    title: String,
    no: Option<u32>,
}

impl Library {
    /// Indexes `folders` as Artist › Album › tracks, at most [`MAX_TRACKS`] of them.
    ///
    /// From the layout: a file's folder is its album, the folder above that its artist
    /// (`Artist/Album/NN Title.mp3`); a file right in the picked folder is on an album named
    /// after the picked folder; the title is the file name without its extension and its
    /// leading track number ("03 - Title", "1-03 Title"), which also numbers it. Tags
    /// override: `TIT2` the title, `TPE1` the track's artist, `TPE2` (else the one `TPE1`
    /// all its tracks share) the album's artist, `TALB` the album (a folder whose files name
    /// different albums is that many albums), `TRCK`/`TPOS` the order, `TYER`/`TDRC` the
    /// year. Tracks play by disc, then track number, then file name; albums sort by
    /// artist, year and title (case-insensitive). Names are printable ASCII as the HUD font
    /// draws them (accents dropped, other letters left out), with fallbacks when nothing is
    /// left.
    pub fn build(folders: &[Folder]) -> Library {
        // Albums by (folder, folder path, album tag), in a deterministic map.
        let mut groups: BTreeMap<(usize, String, String), Vec<Entry>> = BTreeMap::new();
        let mut count = 0;
        for (fi, folder) in folders.iter().enumerate() {
            for file in &folder.files {
                if count >= MAX_TRACKS {
                    break;
                }
                count += 1;
                let (dir, name) = file.path.rsplit_once('/').unwrap_or(("", &file.path));
                let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
                let (no, rest) = leading_number(stem);
                let title = if rest.is_empty() { stem } else { rest };
                let key = if folder.mixtape {
                    (fi, String::new(), String::new())
                } else {
                    let album = file.probe.album.as_deref().map(ascii).unwrap_or_default();
                    (fi, dir.to_string(), album.to_ascii_lowercase())
                };
                groups.entry(key).or_default().push(Entry {
                    file,
                    title: title.to_string(),
                    no: file.probe.track.or(no),
                });
            }
        }
        // A folder's untagged files join its album when its tagged files name only one (a
        // partly tagged album stays one album).
        let untagged: Vec<(usize, String, String)> = groups
            .keys()
            .filter(|(_, _, album)| album.is_empty())
            .cloned()
            .collect();
        for key in untagged {
            let mut tagged = groups
                .keys()
                .filter(|(f, d, a)| *f == key.0 && *d == key.1 && !a.is_empty());
            if let (Some(only), None) = (tagged.next().cloned(), tagged.next()) {
                let moved = groups.remove(&key).unwrap_or_default();
                groups.entry(only).or_default().extend(moved);
            }
        }
        let mut albums: Vec<(Album, Vec<Track>, String)> = Vec::new();
        for ((fi, dir, _), mut entries) in groups {
            let folder = &folders[fi];
            entries.sort_by(|a, b| {
                let k = |e: &Entry| (e.file.probe.disc.unwrap_or(0), e.no.unwrap_or(u32::MAX));
                k(a).cmp(&k(b)).then_with(|| a.file.path.cmp(&b.file.path))
            });
            let dirs: Vec<&str> = dir.split('/').filter(|d| !d.is_empty()).collect();
            let (artist, title) = if folder.mixtape {
                (VARIOUS.to_string(), MIXTAPE.to_string())
            } else {
                let folder_artist = (dirs.len() >= 2).then(|| dirs[dirs.len() - 2]);
                let folder_album = dirs.last().copied().unwrap_or(&folder.name);
                // Tags from whichever files have them (a partly tagged album).
                let tag = |f: fn(&Probe) -> Option<&String>| {
                    entries
                        .iter()
                        .find_map(|e| f(&e.file.probe).map(String::as_str))
                };
                let first_artist = tag(|p| p.artist.as_ref());
                let shared = first_artist.filter(|a| {
                    entries
                        .iter()
                        .filter_map(|e| e.file.probe.artist.as_deref())
                        .all(|b| b == *a)
                });
                let artist = tag(|p| p.album_artist.as_ref())
                    .or(shared)
                    .or(folder_artist)
                    .or(first_artist.map(|_| VARIOUS));
                (
                    named(artist, UNKNOWN_ARTIST),
                    named(
                        tag(|p| p.album.as_ref()).or(Some(folder_album)),
                        "Unknown Album",
                    ),
                )
            };
            let year = entries.iter().find_map(|e| e.file.probe.year);
            let tracks: Vec<Track> = entries
                .iter()
                .map(|e| {
                    let p = &e.file.probe;
                    let fallback = match e.no {
                        Some(n) => format!("Track {n}"),
                        None => "Track".to_string(),
                    };
                    let artist = named(p.artist.as_deref(), &artist);
                    let title = named(p.title.as_deref().or(Some(&e.title)), &fallback);
                    Track {
                        key: e.file.key.clone(),
                        label: format!("{artist} - {title}"),
                        artist,
                        title,
                        secs: p
                            .secs
                            .filter(|s| s.is_finite() && *s >= 1.0)
                            .unwrap_or(NOMINAL_TRACK_S),
                    }
                })
                .collect();
            let album = Album {
                artist,
                title,
                year,
                first: 0,
                len: tracks.len(),
            };
            albums.push((album, tracks, folder.id.clone()));
        }
        albums.sort_by(|(a, ta, ia), (b, tb, ib)| {
            let k = |x: &Album| {
                (
                    x.artist.to_ascii_lowercase(),
                    x.year.unwrap_or(u16::MAX),
                    x.title.to_ascii_lowercase(),
                )
            };
            k(a).cmp(&k(b))
                .then_with(|| ia.cmp(ib))
                .then_with(|| ta[0].key.cmp(&tb[0].key))
        });
        let mut lib = Library::default();
        for (i, (mut album, tracks, _)) in albums.into_iter().enumerate() {
            album.first = lib.tracks.len();
            lib.album_of.extend((0..tracks.len()).map(|_| i as u32));
            lib.tracks.extend(tracks);
            lib.albums.push(album);
        }
        lib.seed = lib.tracks.iter().fold(seed(b"deck"), |h, t| {
            h.rotate_left(5) ^ seed(t.key.as_bytes())
        });
        lib
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// The album of track `i`.
    pub fn album_of(&self, i: usize) -> Option<usize> {
        self.album_of.get(i).map(|&a| a as usize)
    }

    /// The index of the track whose key is `key`.
    pub fn find(&self, key: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.key == key)
    }
}

/// A file name's leading track number and the rest: "03 - Title" is `(Some(3), "Title")`,
/// "1-03 Title" (disc 1) `(Some(3), "Title")`; a name that is only a number keeps it.
fn leading_number(stem: &str) -> (Option<u32>, &str) {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let n = digits(stem);
    if n == 0 || n > 3 {
        return (None, stem);
    }
    let mut no = &stem[..n];
    let mut rest = &stem[n..];
    // "1-03": a disc, then the track.
    if let Some(r) = rest.strip_prefix('-') {
        let m = digits(r);
        if (2..=3).contains(&m) {
            no = &r[..m];
            rest = &r[m..];
        }
    }
    let trimmed = rest.trim_start_matches([' ', '-', '.', '_']);
    if trimmed.len() == rest.len() && !rest.is_empty() {
        // "3rd Song": the number is part of the name.
        return (None, stem);
    }
    (number(no), trimmed)
}

/// `name` as the HUD shows it, else `fallback`.
fn named(name: Option<&str>, fallback: &str) -> String {
    let s = name.map(ascii).unwrap_or_default();
    if s.is_empty() {
        fallback.to_string()
    } else {
        s
    }
}

/// The deck's transport over a [`Library`].
#[derive(Clone, Debug)]
pub struct Deck {
    lib: Library,
    mode: Mode,
    /// Track indices in play order for the mode (Normal: the library's order).
    order: Vec<u32>,
    /// Continuous: the broadcast (its order is `order`).
    list: Playlist,
    /// Shuffle: which pass through the library this is (each pass its own shuffle).
    pass: u32,
    /// The place in `order` of the track loaded.
    pos: usize,
    /// Seconds into it.
    at_s: f64,
    playing: bool,
    live: bool,
    blocked: bool,
    /// Every track failed to play in a row: the deck stopped.
    failed: bool,
    misses: usize,
    /// Seconds the deck has existed (ticked), the clock Continuous plays by.
    clock: f64,
    token: u32,
    /// Counts the transport presses that move the tape or the disc (each plays a click).
    clicks: u32,
    /// Counts every change of the track loaded (the HUD shows each).
    changes: u32,
    /// A track and second from the settings, waiting for its track to be in the library.
    restore: Option<(String, f64)>,
    out: Vec<RadioCommand>,
}

impl Default for Deck {
    fn default() -> Deck {
        Deck::new()
    }
}

impl Deck {
    /// An empty, stopped deck in Normal mode.
    pub fn new() -> Deck {
        Deck {
            lib: Library::default(),
            mode: Mode::Normal,
            order: Vec::new(),
            list: Playlist::new(&[], 0),
            pass: 0,
            pos: 0,
            at_s: 0.0,
            playing: false,
            live: false,
            blocked: false,
            failed: false,
            misses: 0,
            clock: 0.0,
            token: 0,
            clicks: 0,
            changes: 0,
            restore: None,
            out: Vec::with_capacity(4),
        }
    }

    pub fn library(&self) -> &Library {
        &self.lib
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Asked the player to play (see [`Self::is_live`] for whether sound came out).
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn is_live(&self) -> bool {
        self.live
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked
    }

    /// Every track failed to play in a row, so the deck stopped.
    pub fn has_failed(&self) -> bool {
        self.failed
    }

    /// The index into [`Library::tracks`] of the track loaded.
    pub fn track_index(&self) -> Option<usize> {
        self.order.get(self.pos).map(|&k| k as usize)
    }

    pub fn track(&self) -> Option<&Track> {
        self.lib.tracks.get(self.track_index()?)
    }

    /// The album of the track loaded, and its index.
    pub fn album(&self) -> Option<(usize, &Album)> {
        let a = self.lib.album_of(self.track_index()?)?;
        Some((a, &self.lib.albums[a]))
    }

    /// Seconds into the track loaded.
    pub fn position_s(&self) -> f32 {
        self.at_s as f32
    }

    /// The Shuffle pass (for the settings).
    pub fn pass(&self) -> u32 {
        self.pass
    }

    /// Counts the clicks of the mechanism (for [`crate::synth::AudioParams::deck_clicks`]).
    pub fn clicks(&self) -> u32 {
        self.clicks
    }

    /// Counts the changes of the track loaded.
    pub fn changes(&self) -> u32 {
        self.changes
    }

    /// The token of the latest `PlayFile`.
    pub fn token(&self) -> u32 {
        self.token
    }

    /// Picks up where the settings say the last session was: the track `key` at `at_s`
    /// seconds, in Shuffle pass `pass`. Applied now if the track is in the library, else
    /// when it arrives (unless a transport key comes first). Nothing plays.
    pub fn restore(&mut self, key: &str, at_s: f32, pass: u32) {
        self.pass = pass;
        self.restore = Some((key.to_string(), f64::from(at_s.max(0.0))));
        self.reorder(None);
        self.apply_restore();
    }

    fn apply_restore(&mut self) {
        let Some((key, at)) = &self.restore else {
            return;
        };
        let Some(k) = self.lib.find(key) else {
            return;
        };
        let at = at.min(f64::from(self.lib.tracks[k].secs) - 1.0).max(0.0);
        self.restore = None;
        self.move_to(k, at);
    }

    /// Replaces the library (a folder came in, or was removed). The track loaded stays
    /// loaded (and playing) if it is still there; otherwise the deck stops.
    pub fn set_library(&mut self, lib: Library) {
        let key = self.track().map(|t| t.key.clone());
        self.lib = lib;
        let k = key.as_deref().and_then(|k| self.lib.find(k));
        self.reorder(k);
        if self.restore.is_some() {
            self.apply_restore();
        } else if k.is_none() {
            self.pos = 0;
            self.at_s = 0.0;
            if self.playing {
                self.pause();
            }
            self.changes = self.changes.wrapping_add(1);
        }
    }

    /// Builds the mode's order and puts the cursor on track `keep` (if given).
    fn reorder(&mut self, keep: Option<usize>) {
        let n = self.lib.tracks.len() as u32;
        self.order = match self.mode {
            Mode::Normal => (0..n).collect(),
            Mode::Shuffle => {
                let s = self.lib.seed ^ u64::from(self.pass).wrapping_mul(0x2545_f491_4f6c_dd1d);
                Playlist::new(&self.lib.tracks, s).order
            }
            Mode::Continuous => {
                self.list = Playlist::new(&self.lib.tracks, self.lib.seed);
                self.list.order.clone()
            }
        };
        if let Some(k) = keep {
            self.pos = self.place_of(k);
            if self.mode == Mode::Continuous {
                self.sync();
            }
        }
    }

    fn place_of(&self, k: usize) -> usize {
        self.order
            .iter()
            .position(|&o| o as usize == k)
            .unwrap_or(0)
    }

    /// Continuous: shifts the broadcast so that it is at the cursor now.
    fn sync(&mut self) {
        if self.list.total_s > 0.0 {
            let at = self.list.start_of(&self.lib.tracks, self.pos) + self.at_s;
            self.list.offset_s = (at - self.clock).rem_euclid(self.list.total_s);
        }
    }

    /// The next mode; the track loaded stays (in Continuous the broadcast takes over: a
    /// playing deck lands where it would be).
    pub fn cycle_mode(&mut self) {
        self.set_mode(self.mode.next());
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        let k = self.track_index();
        self.mode = mode;
        self.pass = 0;
        self.reorder(None);
        if let Some(k) = k {
            self.pos = self.place_of(k);
            if mode == Mode::Shuffle {
                // The pass starts with the track loaded, so it plays every other track
                // before any comes round again.
                self.order.swap(0, self.pos);
                self.pos = 0;
            }
        }
        if mode == Mode::Continuous && self.playing {
            self.tune_in();
            self.start();
        }
    }

    /// Continuous: the cursor where the broadcast is now.
    fn tune_in(&mut self) {
        let (pos, at) = self.list.at(&self.lib.tracks, self.clock);
        if pos != self.pos {
            self.changes = self.changes.wrapping_add(1);
        }
        self.pos = pos;
        self.at_s = f64::from(at);
    }

    /// Play or pause.
    pub fn toggle(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Plays from where the deck is (Continuous: from where the broadcast is, or from the
    /// restored track and second the first time). Nothing happens with an empty library.
    pub fn play(&mut self) {
        if self.lib.is_empty() || self.playing {
            return;
        }
        self.restore = None;
        if self.mode == Mode::Continuous {
            self.tune_in();
        }
        self.clicks = self.clicks.wrapping_add(1);
        self.start();
    }

    /// Stops the player, keeping the place (the source switching away does this too).
    pub fn pause(&mut self) {
        if self.playing {
            self.playing = false;
            self.live = false;
            self.blocked = false;
            self.out.push(RadioCommand::Stop);
        }
    }

    /// The next track (and plays).
    pub fn next(&mut self) {
        if self.lib.is_empty() {
            return;
        }
        self.restore = None;
        self.advance();
        self.clicks = self.clicks.wrapping_add(1);
        self.start();
    }

    /// Back to the start of the track, or to the previous track within
    /// [`PREV_RESTART_S`] of its start (and plays).
    pub fn prev(&mut self) {
        if self.lib.is_empty() {
            return;
        }
        self.restore = None;
        if self.at_s < f64::from(PREV_RESTART_S) {
            let n = self.order.len();
            self.pos = (self.pos + n - 1) % n;
            self.changes = self.changes.wrapping_add(1);
        }
        self.at_s = 0.0;
        if self.mode == Mode::Continuous {
            self.sync();
        }
        self.clicks = self.clicks.wrapping_add(1);
        self.start();
    }

    /// The first track of the next album, wrapping (and plays).
    pub fn next_album(&mut self) {
        let n = self.lib.albums.len();
        if n > 0 {
            let a = self.album().map_or(n - 1, |(a, _)| a);
            self.pick_album((a + 1) % n);
        }
    }

    /// The first track of the previous album, wrapping (and plays).
    pub fn prev_album(&mut self) {
        let n = self.lib.albums.len();
        if n > 0 {
            let a = self.album().map_or(0, |(a, _)| a);
            self.pick_album((a + n - 1) % n);
        }
    }

    /// Album `a`'s first track (and plays). In Shuffle and Continuous the mode's order goes
    /// on from there.
    pub fn pick_album(&mut self, a: usize) {
        let Some(first) = self.lib.albums.get(a).map(|al| al.first) else {
            return;
        };
        self.restore = None;
        self.move_to(first, 0.0);
        self.clicks = self.clicks.wrapping_add(1);
        self.start();
    }

    /// Puts the cursor on track `k` at `at` seconds.
    fn move_to(&mut self, k: usize, at: f64) {
        let pos = self.place_of(k);
        if pos != self.pos || self.track_index() != Some(k) {
            self.changes = self.changes.wrapping_add(1);
        }
        self.pos = pos;
        self.at_s = at;
        if self.mode == Mode::Continuous {
            self.sync();
        }
    }

    /// The cursor to the next track in the mode's order: Normal wraps from the library's
    /// last track to its first; Shuffle starts a new pass (a new shuffle that does not
    /// begin with the track just played); Continuous goes round its broadcast.
    fn advance(&mut self) {
        let n = self.order.len();
        if n == 0 {
            return;
        }
        if self.pos + 1 < n {
            self.pos += 1;
        } else if self.mode == Mode::Shuffle {
            let last = self.order[self.pos];
            self.pass = self.pass.wrapping_add(1);
            self.reorder(None);
            if n > 1 && self.order[0] == last {
                self.order.swap(0, 1);
            }
            self.pos = 0;
        } else {
            self.pos = 0;
        }
        self.at_s = 0.0;
        if self.mode == Mode::Continuous {
            self.sync();
        }
        self.changes = self.changes.wrapping_add(1);
    }

    /// Asks the player for the track loaded from where the deck is in it.
    fn start(&mut self) {
        let Some(track) = self.track() else {
            return;
        };
        let key = track.key.clone();
        self.token = TOKEN_BIT | (self.token.wrapping_add(1) & !TOKEN_BIT);
        self.playing = true;
        self.live = false;
        self.blocked = false;
        self.failed = false;
        self.out.push(RadioCommand::PlayFile {
            key,
            start_s: self.at_s as f32,
            token: self.token,
        });
    }

    /// Runs the clock by `dt` seconds, and the place in the track while sound comes out.
    pub fn tick(&mut self, dt: f32) {
        let dt = f64::from(dt.max(0.0));
        self.clock += dt;
        if self.playing && self.live {
            self.at_s += dt;
        }
    }

    /// A report from the player; reports about another `PlayFile` (or the radio's) are
    /// ignored. A track that ends plays the next in the mode's order; one that fails is
    /// skipped, and when every track failed in a row the deck stops.
    pub fn on_event(&mut self, event: PlayerEvent) {
        if !self.playing {
            return;
        }
        match event {
            PlayerEvent::Started { token } if token == self.token => {
                self.live = true;
                self.blocked = false;
                self.misses = 0;
            }
            PlayerEvent::Blocked { token } if token == self.token => self.blocked = true,
            PlayerEvent::Ended { token } if token == self.token => {
                self.advance();
                self.start();
            }
            PlayerEvent::Failed { token } if token == self.token => {
                self.misses += 1;
                if self.misses >= self.lib.tracks.len() {
                    self.pause();
                    self.failed = true;
                    self.misses = 0;
                } else {
                    self.advance();
                    self.start();
                }
            }
            _ => {}
        }
    }

    /// The commands for the player since the last call, oldest first.
    pub fn drain_commands(&mut self) -> std::vec::Drain<'_, RadioCommand> {
        self.out.drain(..)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RadioCommand::{PlayFile, Stop};

    fn file(path: &str, probe: Probe) -> FileInfo {
        FileInfo {
            key: format!("r/{path}"),
            path: path.to_string(),
            probe: Probe {
                secs: probe.secs.or(Some(100.0)),
                ..probe
            },
        }
    }

    fn folder(files: Vec<FileInfo>) -> Folder {
        Folder {
            id: "r".into(),
            name: "Music".into(),
            mixtape: false,
            files,
        }
    }

    fn tag(artist: &str, album: &str, title: &str, track: u32) -> Probe {
        Probe {
            artist: Some(artist.into()),
            album: Some(album.into()),
            title: Some(title.into()),
            track: Some(track),
            ..Probe::default()
        }
    }

    /// Two artists, three albums, laid out as a music app exports them, no tags.
    fn layout() -> Library {
        Library::build(&[folder(vec![
            file("Zed Band/Late/02 Two.mp3", Probe::default()),
            file("Zed Band/Late/01 One.mp3", Probe::default()),
            file("Ann Arbor/Second/1-02 B.mp3", Probe::default()),
            file("Ann Arbor/First/10 Ten.mp3", Probe::default()),
            file("Ann Arbor/First/9 Nine.mp3", Probe::default()),
            file("Ann Arbor/Second/1-01 A.mp3", Probe::default()),
        ])])
    }

    fn titles(lib: &Library) -> Vec<String> {
        lib.tracks
            .iter()
            .map(|t| format!("{} {}", t.artist, t.title))
            .collect()
    }

    #[test]
    fn the_folder_layout_is_artist_album_tracks() {
        let lib = layout();
        let albums: Vec<(&str, &str, usize)> = lib
            .albums
            .iter()
            .map(|a| (a.artist.as_str(), a.title.as_str(), a.len))
            .collect();
        assert_eq!(
            albums,
            [
                ("Ann Arbor", "First", 2),
                ("Ann Arbor", "Second", 2),
                ("Zed Band", "Late", 2)
            ]
        );
        assert_eq!(
            titles(&lib),
            [
                "Ann Arbor Nine",
                "Ann Arbor Ten",
                "Ann Arbor A",
                "Ann Arbor B",
                "Zed Band One",
                "Zed Band Two"
            ],
            "by track number, not by name"
        );
        assert_eq!(lib.album_of(3), Some(1));
        assert_eq!(lib.find("r/Zed Band/Late/01 One.mp3"), Some(4));
        // Files right in an album folder the user picked: the album is the folder.
        let one = Library::build(&[Folder {
            name: "Picked Album".into(),
            ..folder(vec![
                file("b.ogg", Probe::default()),
                file("a.ogg", Probe::default()),
            ])
        }]);
        assert_eq!(one.albums[0].title, "Picked Album");
        assert_eq!(one.albums[0].artist, "Unknown Artist");
        assert_eq!(titles(&one), ["Unknown Artist a", "Unknown Artist b"]);
        assert_eq!(leading_number("Song"), (None, "Song"));
        assert_eq!(leading_number("07"), (Some(7), ""));
        assert_eq!(leading_number("07. Song"), (Some(7), "Song"));
        assert_eq!(leading_number("2004 - Song"), (None, "2004 - Song"));
    }

    #[test]
    fn tags_override_the_layout() {
        let lib = Library::build(&[folder(vec![
            // Misfiled, with the right tags.
            file("Wrong/Place/x.mp3", tag("Ann Arbor", "Tagged", "Second", 2)),
            file("Wrong/Place/y.mp3", tag("Ann Arbor", "Tagged", "First", 1)),
            // A compilation: one album artist, each track its own.
            file(
                "Comp/Hits/01.mp3",
                Probe {
                    album_artist: Some("Various".into()),
                    year: Some(1983),
                    ..tag("Bea", "Hits", "Sun", 1)
                },
            ),
            file("Comp/Hits/02.mp3", tag("Cal", "Hits", "Moon", 2)),
            // Accents fold to ASCII; a title with nothing printable falls back.
            file(
                "Loose/Caf\u{e9}/03 \u{65e5}.mp3",
                Probe {
                    title: Some("\u{65e5}\u{672c}".into()),
                    ..Probe::default()
                },
            ),
        ])]);
        let albums: Vec<(&str, &str, Option<u16>)> = lib
            .albums
            .iter()
            .map(|a| (a.artist.as_str(), a.title.as_str(), a.year))
            .collect();
        assert_eq!(
            albums,
            [
                ("Ann Arbor", "Tagged", None),
                ("Loose", "Cafe", None),
                ("Various", "Hits", Some(1983))
            ]
        );
        assert_eq!(
            titles(&lib),
            [
                "Ann Arbor First",
                "Ann Arbor Second",
                "Loose Track 3",
                "Bea Sun",
                "Cal Moon"
            ]
        );
        // A partly tagged album stays one album, its untagged track in its place.
        let part = Library::build(&[folder(vec![
            file("Art/Alb/01 One.mp3", tag("Art", "Alb", "One", 1)),
            file("Art/Alb/02 Two.mp3", Probe::default()),
            file("Art/Alb/03 Three.mp3", tag("Art", "Alb", "Three", 3)),
        ])]);
        assert_eq!(part.albums.len(), 1, "{:?}", part.albums);
        assert_eq!(titles(&part), ["Art One", "Art Two", "Art Three"]);
        // A folder whose files name two albums is two albums.
        let two = Library::build(&[folder(vec![
            file("dump/a.mp3", tag("X", "One", "a", 1)),
            file("dump/b.mp3", tag("X", "Two", "b", 1)),
        ])]);
        assert_eq!(two.albums.len(), 2);
        // Loose dropped files: one mixtape, in name order, whatever their tags.
        let tape = Library::build(&[Folder {
            mixtape: true,
            ..folder(vec![
                file("b.mp3", tag("X", "One", "b", 1)),
                file("a.mp3", tag("Y", "Two", "a", 1)),
            ])
        }]);
        assert_eq!(tape.albums.len(), 1);
        assert_eq!(tape.albums[0].title, MIXTAPE);
        assert_eq!(titles(&tape), ["Y a", "X b"]);
        // The same files give the same library.
        assert_eq!(layout(), layout());
        assert_ne!(layout().seed, tape.seed);
    }

    /// The `PlayFile`s since the last call, as (title, start).
    fn played(d: &mut Deck) -> Vec<(String, f32)> {
        let keys: Vec<(String, f32)> = d
            .drain_commands()
            .filter_map(|c| match c {
                PlayFile { key, start_s, .. } => Some((key, start_s)),
                _ => None,
            })
            .collect();
        keys.into_iter()
            .map(|(k, s)| {
                let i = d.library().find(&k).unwrap();
                (d.library().tracks[i].title.clone(), s)
            })
            .collect()
    }

    fn deck() -> Deck {
        let mut d = Deck::new();
        d.set_library(layout());
        d
    }

    fn started(d: &mut Deck) {
        let token = d.token();
        d.on_event(PlayerEvent::Started { token });
    }

    fn ended(d: &mut Deck) {
        let token = d.token();
        d.on_event(PlayerEvent::Ended { token });
    }

    #[test]
    fn normal_plays_albums_in_order_and_wraps_at_the_end() {
        let mut d = deck();
        assert!(!d.is_playing());
        assert_eq!(d.drain_commands().count(), 0, "silent until a key");
        d.toggle();
        assert_eq!(played(&mut d), [("Nine".into(), 0.0)]);
        let mut seen = Vec::new();
        for _ in 0..6 {
            started(&mut d);
            ended(&mut d);
            seen.extend(played(&mut d).into_iter().map(|(t, _)| t));
        }
        // The last track of an album goes on to the next album; the library's last to its
        // first.
        assert_eq!(seen, ["Ten", "A", "B", "One", "Two", "Nine"]);
        // A stale or radio report changes nothing.
        d.on_event(PlayerEvent::Ended { token: 1 });
        d.on_event(PlayerEvent::Ended {
            token: d.token() - 1,
        });
        assert!(played(&mut d).is_empty());
    }

    #[test]
    fn previous_restarts_after_three_seconds_and_goes_back_before() {
        let mut d = deck();
        d.next();
        assert_eq!(played(&mut d), [("Ten".into(), 0.0)]);
        started(&mut d);
        d.tick(PREV_RESTART_S + 0.5);
        assert!((d.position_s() - 3.5).abs() < 1e-6);
        d.prev();
        assert_eq!(played(&mut d), [("Ten".into(), 0.0)], "restarts");
        started(&mut d);
        d.tick(1.0);
        d.prev();
        assert_eq!(played(&mut d), [("Nine".into(), 0.0)], "goes back");
        d.prev();
        assert_eq!(played(&mut d), [("Two".into(), 0.0)], "wraps to the end");
        // Not live (blocked, loading): the place does not move.
        d.tick(10.0);
        assert_eq!(d.position_s(), 0.0);
    }

    #[test]
    fn albums_step_and_pick() {
        let mut d = deck();
        d.next_album();
        d.next_album();
        d.prev_album();
        d.prev_album();
        d.prev_album();
        let got: Vec<String> = played(&mut d).into_iter().map(|(t, _)| t).collect();
        assert_eq!(
            got,
            ["A", "One", "A", "Nine", "One"],
            "from the first album, wrapping"
        );
        d.pick_album(1);
        assert_eq!(played(&mut d), [("A".into(), 0.0)]);
        assert_eq!(
            d.album().map(|(i, a)| (i, a.title.as_str())),
            Some((1, "Second"))
        );
        d.pick_album(9);
        assert!(played(&mut d).is_empty());
        // Pause keeps the place; play goes on from it.
        started(&mut d);
        d.tick(5.0);
        d.toggle();
        assert_eq!(d.drain_commands().collect::<Vec<_>>(), [Stop]);
        d.tick(30.0);
        d.toggle();
        assert_eq!(played(&mut d), [("A".into(), 5.0)]);
    }

    #[test]
    fn shuffle_plays_every_track_once_per_pass() {
        let mut d = deck();
        d.set_mode(Mode::Shuffle);
        d.toggle();
        let mut first = played(&mut d);
        for _ in 0..17 {
            started(&mut d);
            ended(&mut d);
            first.extend(played(&mut d));
        }
        let names: Vec<&str> = first.iter().map(|(t, _)| t.as_str()).collect();
        for pass in names.chunks(6) {
            let mut p = pass.to_vec();
            p.sort();
            assert_eq!(p, ["A", "B", "Nine", "One", "Ten", "Two"], "{names:?}");
        }
        assert_ne!(names[..6], names[6..12], "each pass its own shuffle");
        assert!(names.windows(2).all(|w| w[0] != w[1]), "{names:?}");
        assert_eq!(d.pass(), 2);
        // Seeded: another deck on the same files plays the same.
        let mut e = deck();
        e.set_mode(Mode::Shuffle);
        e.toggle();
        assert_eq!(played(&mut e)[0], first[0]);
        assert_ne!(
            names[..6],
            ["Nine", "Ten", "A", "B", "One", "Two"],
            "not album order"
        );
    }

    #[test]
    fn continuous_lands_where_the_broadcast_is() {
        let mut d = deck();
        d.set_mode(Mode::Continuous);
        let list = Playlist::new(&d.lib.tracks, d.lib.seed);
        d.tick(1234.0);
        d.toggle();
        let (i, at) = list.at(&d.lib.tracks.clone(), 1234.0);
        let tracks = d.lib.tracks.clone();
        let title = |i: usize| tracks[list.order[i] as usize].title.clone();
        let want = title(i);
        assert_eq!(played(&mut d), [(want, at)]);
        // Paused for 250 s: it played on without us.
        started(&mut d);
        d.tick(10.0);
        d.toggle();
        d.tick(250.0);
        d.toggle();
        let (j, at) = list.at(&tracks, 1234.0 + 260.0);
        assert_eq!(played(&mut d), [(title(j), at)]);
        // Next skips the broadcast ahead: paused and back, it is in that track.
        d.next();
        let skipped = played(&mut d);
        assert_eq!(skipped, [(title((j + 1) % 6), 0.0)]);
        started(&mut d);
        d.toggle();
        d.tick(20.0);
        d.toggle();
        assert_eq!(played(&mut d), [(skipped[0].0.clone(), 20.0)]);
        // Into Normal, the track stays; back into Continuous while playing, it lands.
        started(&mut d);
        d.set_mode(Mode::Normal);
        assert!(played(&mut d).is_empty());
        assert_eq!(d.track().unwrap().title, skipped[0].0);
        d.set_mode(Mode::Continuous);
        assert_eq!(played(&mut d).len(), 1);
    }

    #[test]
    fn restore_waits_for_its_track_and_a_key_overrides_it() {
        let mut d = Deck::new();
        d.restore("r/Ann Arbor/Second/1-02 B.mp3", 42.0, 0);
        assert!(d.track().is_none());
        d.set_library(layout());
        assert_eq!(d.track().unwrap().title, "B");
        assert_eq!(d.position_s(), 42.0);
        assert!(!d.is_playing() && d.drain_commands().count() == 0);
        d.toggle();
        assert_eq!(played(&mut d), [("B".into(), 42.0)]);
        // Continuous: the first play lands on the restored second, and the broadcast goes
        // on from there.
        let mut c = Deck::new();
        c.set_mode(Mode::Continuous);
        c.set_library(layout());
        c.tick(500.0);
        c.restore("r/Zed Band/Late/01 One.mp3", 7.0, 0);
        c.tick(3.0);
        c.toggle();
        assert_eq!(played(&mut c), [("One".into(), 10.0)]);
        // A position past the track's end is clamped.
        let mut e = Deck::new();
        e.restore("r/Zed Band/Late/01 One.mp3", 1e6, 0);
        e.set_library(layout());
        assert_eq!(e.position_s(), 99.0);
    }

    #[test]
    fn failures_skip_and_all_failing_stops() {
        let mut d = deck();
        d.toggle();
        played(&mut d);
        let token = d.token();
        d.on_event(PlayerEvent::Failed { token });
        assert_eq!(played(&mut d), [("Ten".into(), 0.0)]);
        for _ in 0..5 {
            let token = d.token();
            d.on_event(PlayerEvent::Failed { token });
        }
        assert!(!d.is_playing() && d.has_failed());
        assert_eq!(d.drain_commands().next_back(), Some(Stop));
        // The tokens are the deck's own.
        assert!(d.token() & TOKEN_BIT != 0);
        // An empty deck does nothing.
        let mut e = Deck::new();
        e.toggle();
        e.next();
        e.prev();
        e.next_album();
        assert_eq!(e.drain_commands().count(), 0);
        // The library loses the track playing: the deck stops.
        d.toggle();
        played(&mut d);
        d.set_library(Library::default());
        assert!(!d.is_playing());
    }

    #[test]
    fn kinds_follow_the_era() {
        assert_eq!(Kind::for_year(1977), Kind::Tape);
        assert_eq!(Kind::for_year(1989), Kind::Tape);
        assert_eq!(Kind::for_year(1990), Kind::Cd);
        assert_eq!(Kind::for_year(2001), Kind::Cd);
        assert_eq!(Kind::for_year(2009), Kind::Cd);
        assert_eq!(Kind::for_year(2010).badge(), "MP3");
        assert_eq!(Mode::Continuous.next(), Mode::Normal);
    }
}
