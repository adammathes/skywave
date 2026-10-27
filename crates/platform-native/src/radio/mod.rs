//! The native car radio's player: plays what `audio::Radio` asks for (`RadioCommand`s) and
//! reports back (`PlayerEvent`s), like the web page's `<audio>` glue. See
//! `docs/adr/0006-native-streams.md`.
//!
//! Each `Play` cuts the [`Ring`] (the old station falls silent and its thread stops
//! writing) and starts a stream thread (`stream`) that fetches the URL (`http`), splits
//! off the ICY titles (`icy`), decodes MP3 and resamples to 48 kHz (`decode`) into the
//! ring. The sound sink's callback reads the ring through a [`Tap`] and adds the radio to
//! the mixer's output. The game's thread only sends commands and polls a channel: it never
//! waits on the network.

mod decode;
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

/// The audio callback's side: adds the radio to the mixer's output.
pub struct Tap {
    out: Arc<Output>,
    buf: Vec<f32>,
    /// Waiting for [`PREBUFFER_SECONDS`] of audio before playing (at the start, and after
    /// the ring ran dry).
    starved: bool,
    gain: f32,
}

impl Tap {
    /// A tap for callbacks of at most `max_samples` interleaved stereo samples.
    pub fn new(out: Arc<Output>, max_samples: usize) -> Tap {
        Tap {
            out,
            buf: vec![0.0; max_samples],
            starved: true,
            gain: 0.0,
        }
    }

    /// Adds the radio to `stereo` (interleaved, 48 kHz) at its volume, softly limited.
    /// Allocates nothing and never waits.
    pub fn mix_into(&mut self, stereo: &mut [f32]) {
        let ring = &self.out.ring;
        let prebuffer = (PREBUFFER_SECONDS * decode::RATE as f32) as usize * 2;
        if self.starved && ring.available() < prebuffer {
            return;
        }
        let len = stereo.len().min(self.buf.len());
        let n = ring.pop(&mut self.buf[..len]);
        self.starved = n < len;
        if self.starved {
            // The ring ran dry: play what there was and wait for more.
            self.gain = 0.0;
        }
        let target = self.out.volume();
        for (o, &r) in stereo.iter_mut().zip(&self.buf[..n]) {
            // ~5 ms to follow a volume change, so a slider never zippers.
            self.gain += (target - self.gain) * 0.004;
            *o = soft_clip(*o + r * self.gain);
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
        // A stop silences at once.
        player.command(&RadioCommand::Stop);
        mix = [0.1; 512];
        tap.mix_into(&mut mix);
        assert_eq!(mix, [0.1; 512]);
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
