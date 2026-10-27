//! One stream thread per `Play`: connect, read, split off ICY titles, decode, resample and
//! write into the [`Ring`] until the station is switched away from (its epoch ends).
//!
//! It reports `Started` once [`PREBUFFER_SECONDS`] of audio is in the ring. A stream that
//! never got that far fails at once (a refused connection, an HTTP error, a codec we do not
//! decode, nothing within [`STALL`]), so the radio tries the station's next URL and then
//! shows NO SIGNAL, as on the web. A stream that played and then dropped reconnects with
//! backoff ([`Reconnect`]) and fails only when that runs out.

use std::io::Read;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use audio::PlayerEvent;

use super::decode::{Mp3, RATE};
use super::http;
use super::icy::Icy;
use super::ring::{Gone, Ring};

/// Audio in the ring before a stream counts as started, and before the callback plays
/// from an empty ring again, s.
pub const PREBUFFER_SECONDS: f32 = 0.25;
/// A stream that sends no audio for this long is dropped: before it started it fails (the
/// web player's limit too), after that it reconnects.
pub const STALL: Duration = Duration::from_secs(10);

/// What a stream thread tells the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    Event(PlayerEvent),
    /// The station's now-playing text (ICY `StreamTitle`), empty when it cleared it.
    Title {
        token: u32,
        title: String,
    },
}

/// What to do after a connection ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Next {
    RetryAfter(Duration),
    GiveUp,
}

/// Reconnecting a stream that played: 0.5, 1, 2, 4, 8 s apart (from `first`), then give
/// up. Five seconds of steady play since the last connect start the count again.
#[derive(Clone, Copy, Debug)]
pub struct Reconnect {
    tries: u32,
    first: Duration,
}

impl Default for Reconnect {
    fn default() -> Reconnect {
        Reconnect {
            tries: 0,
            first: Duration::from_millis(500),
        }
    }
}

impl Reconnect {
    pub const MAX_TRIES: u32 = 5;
    const STEADY: Duration = Duration::from_secs(5);

    /// A connection ended; `ever_started` whether this stream ever played, `played` how
    /// long the connection that ended did.
    pub fn on_drop(&mut self, ever_started: bool, played: Duration) -> Next {
        if !ever_started {
            return Next::GiveUp;
        }
        if played >= Self::STEADY {
            self.tries = 0;
        }
        if self.tries >= Self::MAX_TRIES {
            return Next::GiveUp;
        }
        let delay = self.first * (1 << self.tries);
        self.tries += 1;
        Next::RetryAfter(delay)
    }
}

/// Why a connection ended.
enum End {
    /// The station was switched away from: stop quietly.
    Gone,
    Dropped(String),
}

impl From<Gone> for End {
    fn from(_: Gone) -> End {
        End::Gone
    }
}

/// Plays `url` into `ring` as the writer of `epoch`, reporting with `token`, until the
/// epoch ends or the stream fails.
pub fn run(url: &str, token: u32, epoch: u32, ring: &Ring, notes: &Sender<Note>) {
    run_with(Reconnect::default(), STALL, url, token, epoch, ring, notes);
}

fn run_with(
    mut reconnect: Reconnect,
    stall: Duration,
    url: &str,
    token: u32,
    epoch: u32,
    ring: &Ring,
    notes: &Sender<Note>,
) {
    let mut st = Stream {
        stall,
        ..Stream::new(token, epoch, ring, notes)
    };
    loop {
        let (end, played) = st.connect_and_play(url);
        let End::Dropped(why) = end else {
            return;
        };
        match reconnect.on_drop(st.started, played) {
            Next::RetryAfter(delay) => {
                log::info!("radio: {url}: {why}; reconnecting in {delay:?}");
                let until = Instant::now() + delay;
                while Instant::now() < until {
                    if !st.wanted() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
            Next::GiveUp => {
                log::warn!("radio: {url}: {why}");
                let _ = notes.send(Note::Event(PlayerEvent::Failed { token }));
                return;
            }
        }
    }
}

/// A stream's writer into the ring (also what a file thread writes through).
pub(super) struct Stream<'a> {
    token: u32,
    epoch: u32,
    ring: &'a Ring,
    notes: &'a Sender<Note>,
    started: bool,
    /// Samples written since the stream started.
    written: usize,
    title: Option<String>,
    /// How long without decoded audio drops the connection.
    stall: Duration,
}

impl<'a> Stream<'a> {
    /// A writer for the `Play` (or `PlayFile`) with `token`, as the writer of `epoch`.
    pub(super) fn new(
        token: u32,
        epoch: u32,
        ring: &'a Ring,
        notes: &'a Sender<Note>,
    ) -> Stream<'a> {
        Stream {
            token,
            epoch,
            ring,
            notes,
            started: false,
            written: 0,
            title: None,
            stall: STALL,
        }
    }

    pub(super) fn wanted(&self) -> bool {
        self.ring.epoch() == self.epoch
    }

    /// One connection: how it ended, and how long it played (from the connect to its last
    /// audio).
    fn connect_and_play(&mut self, url: &str) -> (End, Duration) {
        let t0 = Instant::now();
        let mut last_audio = t0;
        let end = self.play_once(url, t0, &mut last_audio);
        (end, last_audio - t0)
    }

    fn play_once(&mut self, url: &str, t0: Instant, last_audio: &mut Instant) -> End {
        let deadline = t0 + self.stall;
        let (mut conn, head, body) = match http::open(url, deadline, &|| self.wanted()) {
            Ok(c) => c,
            Err(_) if !self.wanted() => return End::Gone,
            Err(e) => return End::Dropped(e),
        };
        if let Err(e) = check_codec(head.header("content-type")) {
            return End::Dropped(e);
        }
        let metaint = head
            .header("icy-metaint")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut icy = Icy::new(metaint);
        let mut mp3 = Mp3::new();
        let mut bytes = Vec::with_capacity(16 * 1024);
        let mut pcm = Vec::with_capacity(16 * 1024);
        let mut chunk = vec![0u8; 16 * 1024];
        let mut input: &[u8] = &body;
        loop {
            if let Some(title) = icy.feed(input, &mut bytes) {
                self.set_title(title);
            }
            mp3.feed(&bytes, &mut pcm);
            bytes.clear();
            if let Err(g) = self.write(&pcm) {
                return g.into();
            }
            if !pcm.is_empty() {
                *last_audio = Instant::now();
            }
            pcm.clear();
            if !self.wanted() {
                return End::Gone;
            }
            // Open but silent (or sending what does not decode): a drop, like a hang-up.
            if last_audio.elapsed() >= self.stall {
                return End::Dropped(format!("no audio for {:?}", self.stall));
            }
            input = match conn.read(&mut chunk) {
                Ok(0) => return End::Dropped("the server closed the stream".into()),
                Ok(n) => &chunk[..n],
                Err(e) if http::is_timeout(&e) => &[],
                Err(e) => return End::Dropped(e.to_string()),
            };
        }
    }

    /// Writes all of `pcm`, waiting while the ring is full; reports `Started` once the
    /// prebuffer is in.
    pub(super) fn write(&mut self, mut pcm: &[f32]) -> Result<(), Gone> {
        let prebuffer = (PREBUFFER_SECONDS * RATE as f32) as usize * 2;
        while !pcm.is_empty() {
            let n = self.ring.push(self.epoch, pcm)?;
            pcm = &pcm[n..];
            self.written += n;
            if self.written >= prebuffer {
                self.mark_started();
            }
            if n == 0 {
                thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    }

    /// Reports `Started`, once.
    pub(super) fn mark_started(&mut self) {
        if !self.started {
            self.started = true;
            let _ = self
                .notes
                .send(Note::Event(PlayerEvent::Started { token: self.token }));
        }
    }

    fn set_title(&mut self, title: String) {
        if self.title.as_ref() != Some(&title) {
            self.title = Some(title.clone());
            let _ = self.notes.send(Note::Title {
                token: self.token,
                title,
            });
        }
    }
}

/// Streams we decode: MP3 (or no type given, sniffed by the decoder). AAC and the rest
/// fail, so the radio tries the station's next URL.
pub fn check_codec(content_type: Option<&str>) -> Result<(), String> {
    let t = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match t.as_str() {
        ""
        | "audio/mpeg"
        | "audio/mp3"
        | "audio/mpeg3"
        | "audio/x-mpeg"
        | "application/octet-stream" => Ok(()),
        other => Err(format!("{other} is not decoded natively (MP3 only)")),
    }
}

/// Spawns the stream thread for `Play { url, token }`; a thread that cannot start fails.
pub fn spawn(url: String, token: u32, epoch: u32, ring: Arc<Ring>, notes: Sender<Note>) {
    let failed = notes.clone();
    let spawned = thread::Builder::new()
        .name("radio-stream".into())
        .spawn(move || run(&url, token, epoch, &ring, &notes));
    if let Err(e) = spawned {
        log::warn!("radio: cannot start a stream thread: {e}");
        let _ = failed.send(Note::Event(PlayerEvent::Failed { token }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    #[test]
    fn reconnect_backs_off_then_gives_up() {
        let s = Duration::from_secs;
        let mut r = Reconnect::default();
        assert_eq!(
            r.on_drop(false, s(0)),
            Next::GiveUp,
            "never played: fail at once"
        );
        let ms = |m| Next::RetryAfter(Duration::from_millis(m));
        assert_eq!(r.on_drop(true, s(600)), ms(500));
        assert_eq!(r.on_drop(true, s(0)), ms(1000));
        assert_eq!(r.on_drop(true, s(1)), ms(2000));
        assert_eq!(r.on_drop(true, s(0)), ms(4000));
        assert_eq!(r.on_drop(true, s(0)), ms(8000));
        assert_eq!(r.on_drop(true, s(0)), Next::GiveUp);
        assert_eq!(r.on_drop(true, s(0)), Next::GiveUp);
        // Steady play since the last connect starts over.
        assert_eq!(r.on_drop(true, s(5)), ms(500));
    }

    #[test]
    fn only_mp3_is_decoded() {
        assert!(check_codec(Some("audio/mpeg")).is_ok());
        assert!(check_codec(Some("Audio/MPEG; charset=x")).is_ok());
        assert!(check_codec(None).is_ok());
        assert!(check_codec(Some("audio/aac")).is_err());
        assert!(check_codec(Some("audio/aacp")).is_err());
        assert!(check_codec(Some("text/html")).is_err());
    }

    /// An Icecast-like server on this machine: `answer` as the head, then `frames`
    /// generated silent MP3 frames with a title every `metaint` bytes, then it hangs up.
    fn serve(head: &'static str, metaint: usize, frames: usize) -> String {
        serve_then_hold(head, metaint, frames, Duration::ZERO).0
    }

    /// [`serve`], keeping the connection open and silent for `hold` before hanging up; the
    /// flag turns true when it has.
    fn serve_then_hold(
        head: &'static str,
        metaint: usize,
        frames: usize,
        hold: Duration,
    ) -> (String, Arc<AtomicBool>) {
        let hung_up = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&hung_up);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            // One connection only: reconnecting is refused.
            drop(listener);
            let mut req = [0u8; 1024];
            let _ = s.read(&mut req);
            let audio: Vec<u8> = (0..frames)
                .flat_map(|_| super::super::decode::tests::silent_frame())
                .collect();
            let mut body = head.as_bytes().to_vec();
            for (i, part) in audio.chunks(metaint).enumerate() {
                body.extend_from_slice(part);
                if part.len() == metaint {
                    let text = format!("StreamTitle='Song {}';", i / 4);
                    let len = text.len().div_ceil(16);
                    body.push(len as u8);
                    body.extend_from_slice(text.as_bytes());
                    body.resize(body.len() + len * 16 - text.len(), 0);
                }
            }
            let _ = s.write_all(&body);
            thread::sleep(hold);
            drop(s);
            flag.store(true, Ordering::SeqCst);
        });
        (format!("http://127.0.0.1:{port}/stream"), hung_up)
    }

    #[test]
    fn a_local_icecast_stream_plays_with_titles_then_fails_when_it_cannot_reconnect() {
        let url = serve(
            "HTTP/1.0 200 OK\r\nContent-Type: audio/mpeg\r\nicy-metaint: 4000\r\n\r\n",
            4000,
            200,
        );
        let ring = Ring::new(1 << 20);
        let (tx, rx) = mpsc::channel();
        let quick = Reconnect {
            tries: 0,
            first: Duration::from_millis(1),
        };
        run_with(quick, STALL, &url, 7, ring.epoch(), &ring, &tx);
        let notes: Vec<Note> = rx.try_iter().collect();
        // Titles arrive in order, once each (a read that spans several keeps the last).
        let titles: Vec<&str> = notes
            .iter()
            .filter_map(|n| match n {
                Note::Title { token: 7, title } => Some(title.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            titles.len() >= 2 && titles.windows(2).all(|w| w[0] < w[1]),
            "{titles:?}"
        );
        assert_eq!(titles.last(), Some(&"Song 4"));
        assert!(notes.contains(&Note::Event(PlayerEvent::Started { token: 7 })));
        // 200 frames ≈ 5.2 s of audio at 48 kHz stereo, less what the decoder holds back.
        assert!(ring.available() > 4 * 96_000, "{}", ring.available());
        // The server hung up and is gone: reconnecting ran out, and the stream failed.
        assert_eq!(
            notes.last(),
            Some(&Note::Event(PlayerEvent::Failed { token: 7 }))
        );
    }

    /// A server that plays ~2.6 s, then keeps the connection open and sends nothing: the
    /// watchdog drops it while the server still holds it (not the hang-up: the server waits
    /// a minute), reconnecting fails (the server takes one connection), and the stream
    /// reports Failed.
    #[test]
    fn a_stream_that_goes_quiet_is_dropped() {
        let (url, hung_up) = serve_then_hold(
            "HTTP/1.0 200 OK\r\nContent-Type: audio/mpeg\r\n\r\n",
            1 << 30,
            100,
            Duration::from_secs(60),
        );
        let ring = Ring::new(1 << 20);
        let (tx, rx) = mpsc::channel();
        let quick = Reconnect {
            tries: 0,
            first: Duration::from_millis(1),
        };
        let t0 = Instant::now();
        run_with(
            quick,
            Duration::from_millis(1500),
            &url,
            4,
            ring.epoch(),
            &ring,
            &tx,
        );
        let notes: Vec<Note> = rx.try_iter().collect();
        assert_eq!(
            notes,
            [
                Note::Event(PlayerEvent::Started { token: 4 }),
                Note::Event(PlayerEvent::Failed { token: 4 })
            ]
        );
        assert!(
            !hung_up.load(Ordering::SeqCst),
            "failed only after the hang-up: {:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn aac_and_errors_fail_without_starting() {
        let (tx, rx) = mpsc::channel();
        let ring = Ring::new(1 << 16);
        let url = serve(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/aac\r\n\r\n",
            1 << 30,
            10,
        );
        run(&url, 1, ring.epoch(), &ring, &tx);
        let url = serve("HTTP/1.1 404 Not Found\r\n\r\n", 1 << 30, 0);
        run(&url, 2, ring.epoch(), &ring, &tx);
        let notes: Vec<Note> = rx.try_iter().collect();
        assert_eq!(
            notes,
            [
                Note::Event(PlayerEvent::Failed { token: 1 }),
                Note::Event(PlayerEvent::Failed { token: 2 })
            ]
        );
        assert_eq!(ring.available(), 0);
    }

    #[test]
    fn a_cut_stops_the_stream_quietly() {
        let url = serve(
            "ICY 200 OK\r\ncontent-type: audio/mpeg\r\n\r\n",
            1 << 30,
            400,
        );
        let ring = Arc::new(Ring::new(48_000));
        let (tx, rx) = mpsc::channel();
        spawn(url, 3, ring.epoch(), Arc::clone(&ring), tx);
        // The ring fills (0.5 s) and the writer waits; then the station is switched away.
        let t0 = Instant::now();
        while ring.available() < ring.capacity() && t0.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            rx.recv().unwrap(),
            Note::Event(PlayerEvent::Started { token: 3 })
        );
        ring.cut();
        // The thread ends (its sender drops) without reporting anything else.
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert_eq!(ring.available(), 0);
    }

    /// Preset stations the native player cannot play, and why. The live test checks they
    /// still fail, so a station that changes is noticed either way.
    const NATIVE_NO_SIGNAL: [(&str, &str); 3] = [
        ("intergalactic_cbs", "AAC only"),
        ("intergalactic_disco", "AAC only"),
        (
            "wmxm",
            "its server offers only CBC cipher suites, which rustls does not implement",
        ),
    ];

    /// Plays 5 s of every preset station from the internet, each URL in order as the radio
    /// tries them, and checks the audio is real (and that [`NATIVE_NO_SIGNAL`] still fail):
    /// `cargo test -p platform-native -- --ignored --nocapture every_preset_station_plays`.
    #[test]
    #[ignore = "needs the internet"]
    fn every_preset_station_plays() {
        const SECONDS: usize = 5;
        let want = SECONDS * RATE as usize * 2;
        let mut seen = Vec::new();
        let mut failed = Vec::new();
        for (_, text) in app::radio::PRESETS {
            let preset = audio::RadioPreset::parse(text).unwrap();
            for station in &preset.stations {
                if seen.contains(&station.id) || station.urls().is_empty() {
                    continue;
                }
                seen.push(station.id.clone());
                let mut line = format!("FAIL {}", station.id);
                for url in station.urls() {
                    let ring = Arc::new(Ring::new(1 << 20));
                    let (tx, rx) = mpsc::channel();
                    let t0 = Instant::now();
                    spawn(url.clone(), 1, ring.epoch(), Arc::clone(&ring), tx);
                    let mut started = None;
                    let mut failure = false;
                    let mut title = String::new();
                    while ring.available() < want && t0.elapsed() < Duration::from_secs(30) {
                        match rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(Note::Event(PlayerEvent::Started { .. })) => {
                                started = Some(t0.elapsed())
                            }
                            Ok(Note::Event(_)) => {
                                failure = true;
                                break;
                            }
                            Ok(Note::Title { title: t, .. }) => title = t,
                            Err(_) => {}
                        }
                    }
                    let mut pcm = vec![0.0f32; want];
                    let n = ring.pop(&mut pcm);
                    ring.cut();
                    let rms =
                        (pcm[..n].iter().map(|s| s * s).sum::<f32>() / n.max(1) as f32).sqrt();
                    let peak = pcm[..n].iter().fold(0.0f32, |m, s| m.max(s.abs()));
                    if failure || n < want || rms < 1e-3 {
                        println!(
                            "  {}: {url} did not play (failed={failure}, {n} samples)",
                            station.id
                        );
                        continue;
                    }
                    line = format!(
                        "ok   {:<22} {url}  start {:.2}s  {SECONDS}s rms {rms:.3} peak {peak:.2}  \"{title}\"",
                        station.id,
                        started.unwrap_or_default().as_secs_f32(),
                    );
                    break;
                }
                let known = NATIVE_NO_SIGNAL.iter().find(|(id, _)| *id == station.id);
                match (line.starts_with("FAIL"), known) {
                    (true, Some((_, why))) => line = format!("no   {:<22} ({why})", station.id),
                    (true, None) => failed.push(station.id.clone()),
                    (false, Some(_)) => failed.push(format!("{} (plays now)", station.id)),
                    (false, None) => {}
                }
                println!("{line}");
            }
        }
        assert!(failed.is_empty(), "unexpected: {failed:?}");
    }
}
