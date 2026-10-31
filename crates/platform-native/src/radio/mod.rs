//! The native car radio's player: plays what `audio::Radio` asks for (`RadioCommand`s) and
//! reports back (`PlayerEvent`s), like the web page's `<audio>` glue. See
//! `docs/adr/0006-native-streams.md`.
//!
//! Each `Play` cuts the [`Ring`] (the old station falls silent and its thread stops
//! writing) and starts a stream thread (`stream`) that fetches the URL (`http`), splits
//! off the ICY titles (`icy`), decodes MP3 and resamples to 48 kHz (`decode`) into the
//! ring. Each `PlayFile` (a track of the player's music folder, see `crate::music`) starts a
//! file thread (`file`) that reads, seeks, decodes and resamples the same way. The sound
//! sink's callback reads the ring through a [`Tap`] and adds the radio to
//! the mixer's output. The game's thread only sends commands and polls a channel: it never
//! waits on the network.

mod decode;
pub(crate) mod file;
mod http;
mod icy;
mod ring;
mod stream;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use audio::{PlayerEvent, RadioCommand};

pub use ring::Ring;
use stream::{Note, PREBUFFER_SECONDS};

/// Seconds of 48 kHz stereo the ring holds (servers send a burst on connect).
const RING_SECONDS: usize = 4;

/// What the stream threads and the audio callback share: the ring and the volume.
pub struct Output {
    ring: Arc<Ring>,
    /// The radio volume (0-1, already scaled by the master volume and mute), as `f32` bits.
    volume: AtomicU32,
}

impl Default for Output {
    fn default() -> Output {
        Output::new()
    }
}

impl Output {
    pub fn new() -> Output {
        Output {
            ring: Arc::new(Ring::new(RING_SECONDS * decode::RATE as usize * 2)),
            volume: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
}

/// The game thread's side: commands in, events out.
pub struct Player {
    out: Arc<Output>,
    tx: Sender<Note>,
    rx: Receiver<Note>,
    /// The token of the latest `Play` (titles of older ones are dropped).
    token: u32,
    title: Option<String>,
    /// Counts changes of `title`.
    titles: u32,
}

impl Player {
    pub fn new(out: Arc<Output>) -> Player {
        let (tx, rx) = mpsc::channel();
        Player {
            out,
            tx,
            rx,
            token: 0,
            title: None,
            titles: 0,
        }
    }

    pub fn command(&mut self, cmd: &RadioCommand) {
        match cmd {
            RadioCommand::Play { url, token } => {
                let epoch = self.out.ring.cut();
                self.token = *token;
                self.set_title(None);
                log::info!("radio: play {url}");
                stream::spawn(
                    url.clone(),
                    *token,
                    epoch,
                    Arc::clone(&self.out.ring),
                    self.tx.clone(),
                );
            }
            RadioCommand::PlayFile {
                key,
                start_s,
                token,
            } => {
                let epoch = self.out.ring.cut();
                self.token = *token;
                self.set_title(None);
                log::info!("radio: play {key} from {start_s:.0} s");
                file::spawn(
                    key.clone(),
                    *start_s,
                    *token,
                    epoch,
                    Arc::clone(&self.out.ring),
                    self.tx.clone(),
                );
            }
            RadioCommand::Stop => {
                self.out.ring.cut();
                self.token = 0;
                self.set_title(None);
            }
            RadioCommand::Volume(v) => {
                self.out
                    .volume
                    .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
            }
        }
    }

    /// The next report from the stream threads, if any (never waits).
    pub fn poll(&mut self) -> Option<PlayerEvent> {
        while let Ok(note) = self.rx.try_recv() {
            match note {
                Note::Event(e) => return Some(e),
                Note::Title { token, title } if token == self.token => {
                    log::info!("radio: now playing: {title}");
                    self.set_title(Some(title));
                }
                Note::Title { .. } => {}
            }
        }
        None
    }

    fn set_title(&mut self, title: Option<String>) {
        if title != self.title {
            self.title = title;
            self.titles = self.titles.wrapping_add(1);
        }
    }

    /// Counts the changes of [`Player::title`] (so a caller can tell when to look again).
    pub fn titles(&self) -> u32 {
        self.titles
    }

    /// What the playing station says is on (its ICY `StreamTitle`), when it says.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref().filter(|t| !t.is_empty())
    }
}

/// Frames (~5 ms at 48 kHz) over which the radio ramps from its last sample to silence when
/// the ring runs dry or the station is cut, so it never clicks.
const FADE_FRAMES: u32 = 240;

/// The audio callback's side: adds the radio to the mixer's output.
pub struct Tap {
    out: Arc<Output>,
    buf: Vec<f32>,
    /// Waiting for [`PREBUFFER_SECONDS`] of audio before playing (at the start, after
    /// the ring ran dry and after a cut).
    starved: bool,
    gain: f32,
    /// The ring's epoch last seen: a new one means the station was cut.
    epoch: u32,
    /// The last sample added per channel (left, right).
    last: [f32; 2],
    /// The level the fade-out starts from per channel, and the frames of it left.
    tail: [f32; 2],
    fade: u32,
}

impl Tap {
    /// A tap for callbacks of at most `max_samples` interleaved stereo samples.
    pub fn new(out: Arc<Output>, max_samples: usize) -> Tap {
        let epoch = out.ring.epoch();
        Tap {
            out,
            buf: vec![0.0; max_samples],
            starved: true,
            gain: 0.0,
            epoch,
            last: [0.0; 2],
            tail: [0.0; 2],
            fade: 0,
        }
    }

    /// Stops playing: ramps out from the last sample and waits for a prebuffer, then
    /// ramps back in from zero gain.
    fn starve(&mut self) {
        self.starved = true;
        self.gain = 0.0;
        self.tail = self.last;
        self.last = [0.0; 2];
        self.fade = FADE_FRAMES;
    }

    /// Adds the radio to `stereo` (interleaved, 48 kHz) at its volume, softly limited.
    /// Allocates nothing and never waits.
    pub fn mix_into(&mut self, stereo: &mut [f32]) {
        let epoch = self.out.ring.epoch();
        if epoch != self.epoch {
            // The station was cut (stopped or switched): fade out what was playing.
            self.epoch = epoch;
            if !self.starved {
                self.starve();
            }
        }
        let ring = &self.out.ring;
        let prebuffer = (PREBUFFER_SECONDS * decode::RATE as f32) as usize * 2;
        let len = stereo.len().min(self.buf.len());
        let n = if self.starved && ring.available() < prebuffer {
            0
        } else {
            let n = ring.pop(&mut self.buf[..len]);
            self.starved = false;
            n
        };
        // The ring ran dry: play what there was, then fade out and wait for more.
        let dry = n < len && !self.starved;
        if n == 0 && !dry && self.fade == 0 {
            return;
        }
        let target = self.out.volume();
        for (i, o) in stereo.iter_mut().enumerate() {
            let ch = i & 1;
            if i == n && dry {
                self.starve();
            }
            let mut x = 0.0;
            if i < n {
                // ~5 ms to follow a volume change, so a slider never zippers.
                self.gain += (target - self.gain) * 0.004;
                x = self.buf[i] * self.gain;
                self.last[ch] = x;
            }
            if self.fade > 0 {
                x += self.tail[ch] * (self.fade as f32 / FADE_FRAMES as f32);
                if ch == 1 {
                    self.fade -= 1;
                }
            } else if i >= n {
                break;
            }
            *o = soft_clip(*o + x);
        }
    }
}

/// Unity up to 0.9, then bends toward 1.0: the radio over the engine never clips hard.
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh()).copysign(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tap_waits_for_a_prebuffer_and_adds_at_the_volume() {
        let out = Arc::new(Output::new());
        let mut player = Player::new(Arc::clone(&out));
        player.command(&RadioCommand::Volume(0.5));
        let mut tap = Tap::new(Arc::clone(&out), 512);
        let e = out.ring.epoch();
        out.ring.push(e, &[0.4; 1000]).unwrap();
        let mut mix = [0.1f32; 512];
        tap.mix_into(&mut mix);
        assert_eq!(mix, [0.1; 512], "less than the prebuffer: nothing yet");
        let pre = (PREBUFFER_SECONDS * 48_000.0) as usize * 2;
        out.ring.push(e, &vec![0.4; pre]).unwrap();
        for _ in 0..8 {
            mix = [0.1; 512];
            tap.mix_into(&mut mix);
        }
        // The gain has settled at the volume: 0.1 + 0.4 × 0.5.
        assert!((mix[511] - 0.3).abs() < 1e-3, "{}", mix[511]);
        // A stop fades out within ~5 ms, then adds nothing.
        player.command(&RadioCommand::Stop);
        mix = [0.1; 512];
        tap.mix_into(&mut mix);
        assert!(mix[0] > 0.29, "{}", mix[0]);
        assert_eq!(mix[2 * FADE_FRAMES as usize..], [0.1; 32]);
        mix = [0.1; 512];
        tap.mix_into(&mut mix);
        assert_eq!(mix, [0.1; 512]);
    }

    /// A full-scale tone through an underrun (mid-callback, near a peak), its resumption
    /// and a stop never steps further from one sample to the next than the tone itself.
    #[test]
    fn the_tap_never_clicks_on_an_underrun_or_a_stop() {
        let out = Arc::new(Output::new());
        let mut player = Player::new(Arc::clone(&out));
        let mut tap = Tap::new(Arc::clone(&out), 512);
        let e = out.ring.epoch();
        // 375 Hz: a period of exactly 128 frames, peaks at frame 32 + 128k.
        let mut phase = 0usize;
        let mut tone = |frames: usize| {
            let mut v = Vec::with_capacity(frames * 2);
            for _ in 0..frames {
                let s = (core::f32::consts::TAU * (phase % 128) as f32 / 128.0).sin();
                v.extend_from_slice(&[s, -s]);
                phase += 1;
            }
            v
        };
        let mut played = Vec::new();
        let mut run = |tap: &mut Tap, callbacks: usize| {
            for _ in 0..callbacks {
                let mut mix = [0.0f32; 512];
                tap.mix_into(&mut mix);
                played.extend_from_slice(&mix);
            }
        };
        // 12 064 frames end on a peak, 0.125 into a 256-frame callback: an underrun.
        out.ring.push(e, &tone(94 * 128 + 32)).unwrap();
        run(&mut tap, 60);
        // It comes back, then is stopped mid-tone.
        out.ring.push(e, &tone(94 * 128)).unwrap();
        run(&mut tap, 30);
        player.command(&RadioCommand::Stop);
        run(&mut tap, 10);
        let peak = played.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.95, "the tone played at full scale: {peak}");
        // The tone's own step is 2π/128 ≈ 0.05.
        let step = played
            .windows(3)
            .map(|w| (w[2] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(step < 0.06, "largest step {step}");
        assert_eq!(
            played[played.len() - 512..],
            [0.0; 512],
            "silent at the end"
        );
    }

    /// Plays a file through the player and the real callback tap (fed in callback-sized
    /// pieces at about real time); returns the player's reports within 10 s.
    fn play_through_the_tap(path: &str, start_s: f32) -> Vec<PlayerEvent> {
        let out = Arc::new(Output::new());
        let mut player = Player::new(Arc::clone(&out));
        let mut tap = Tap::new(Arc::clone(&out), 1024);
        player.command(&RadioCommand::PlayFile {
            key: path.to_string(),
            start_s,
            token: 1,
        });
        let mut events = Vec::new();
        let t0 = std::time::Instant::now();
        while t0.elapsed() < std::time::Duration::from_secs(10) {
            let mut mix = [0.0f32; 1024];
            tap.mix_into(&mut mix);
            while let Some(e) = player.poll() {
                events.push(e);
            }
            if matches!(
                events.last(),
                Some(PlayerEvent::Ended { .. } | PlayerEvent::Failed { .. })
            ) {
                break;
            }
            // 512 frames at 48 kHz, a little faster than real time.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        events
    }

    /// The end of a track always plays out and `Ended` comes, even when less than the
    /// tap's prebuffer is left: a short file, and one entered just before its end.
    #[test]
    fn a_track_ends_through_the_tap_however_little_is_left() {
        use file::tests::{mp3_file, scratch};
        let done = [
            PlayerEvent::Started { token: 1 },
            PlayerEvent::Ended { token: 1 },
        ];
        // 5 frames ≈ 0.13 s.
        let short = scratch("tap-short.mp3", &mp3_file(5, &[]));
        assert_eq!(play_through_the_tap(&short, 0.0), done);
        // ~3 s, from 2.9 s.
        let tail = scratch("tap-tail.mp3", &mp3_file(115, &[]));
        assert_eq!(play_through_the_tap(&tail, 2.9), done);
    }

    #[test]
    fn soft_clip_is_unity_below_the_knee_and_bounded_above() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.9), -0.9);
        for x in [1.0f32, 1.5, 3.0, 100.0] {
            let y = soft_clip(x);
            assert!(y > 0.9 && y <= 1.0, "{x} -> {y}");
            assert_eq!(soft_clip(-x), -y);
        }
        assert!(soft_clip(1.2) > soft_clip(1.0));
    }

    #[test]
    fn stale_titles_are_dropped() {
        let out = Arc::new(Output::new());
        let mut player = Player::new(out);
        player.token = 2;
        let send = |token, t: &str| {
            player.tx.send(Note::Title {
                token,
                title: t.into(),
            })
        };
        send(1, "old").unwrap();
        send(2, "new").unwrap();
        player
            .tx
            .send(Note::Event(PlayerEvent::Started { token: 2 }))
            .unwrap();
        assert_eq!(player.poll(), Some(PlayerEvent::Started { token: 2 }));
        assert_eq!(player.title(), Some("new"));
        assert_eq!(player.poll(), None);
    }
}
