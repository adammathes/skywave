//! One file thread per `PlayFile`: read an MP3 from the player's music folder, seek to
//! where the station's shuffle is, decode and write into the [`Ring`] at the pace the
//! audio callback takes it (the ring is full most of the time), until the file ends or the
//! station is switched away from.
//!
//! It reports `Started` once the prebuffer is in, like a stream, and `Ended` once the
//! whole file has been played, the ring drained (so the next track does not cut its
//! tail). A file that does not open or holds no audio it decodes reports `Failed`, and the
//! radio skips to the next track.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use audio::music::{self, HEAD_BYTES};
use audio::PlayerEvent;

use super::decode::Mp3;
use super::ring::Ring;
use super::stream::{Note, Stream};

/// How it went.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Played to its end.
    Ended,
    /// The station was switched away from.
    Gone,
}

/// Plays the file at `path` from `start_s` seconds into it, reporting with `token`, as the
/// ring's writer of `epoch`.
pub fn play(path: &str, start_s: f32, token: u32, epoch: u32, ring: &Ring, notes: &Sender<Note>) {
    let mut w = Stream::new(token, epoch, ring, notes);
    let event = match decode_file(path, start_s, &mut w) {
        Ok(Outcome::Gone) => return,
        Ok(Outcome::Ended) => {
            // Let the callback play what is still in the ring.
            while w.wanted() && ring.available() > 0 {
                thread::sleep(Duration::from_millis(20));
            }
            if !w.wanted() {
                return;
            }
            PlayerEvent::Ended { token }
        }
        Err(e) => {
            log::warn!("radio: {path}: {e}");
            PlayerEvent::Failed { token }
        }
    };
    let _ = notes.send(Note::Event(event));
}

fn decode_file(path: &str, start_s: f32, w: &mut Stream) -> Result<Outcome, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let at = start_offset(&mut file, len, start_s)?;
    file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
    let mut mp3 = Mp3::new();
    let mut chunk = vec![0u8; 16 * 1024];
    let mut pcm = Vec::with_capacity(32 * 1024);
    loop {
        let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            mp3.finish(&mut pcm);
        } else {
            mp3.feed(&chunk[..n], &mut pcm);
        }
        if w.write(&pcm).is_err() || !w.wanted() {
            return Ok(Outcome::Gone);
        }
        pcm.clear();
        if n == 0 {
            break;
        }
    }
    if mp3.frames == 0 {
        return Err("no MP3 audio in it".into());
    }
    // Shorter than the prebuffer, it still played.
    w.mark_started();
    Ok(Outcome::Ended)
}

/// The byte where `start_s` seconds into the file are: past its ID3v2 tag, then the share
/// of the audio bytes that `start_s` is of its length (exact for constant bitrate, close
/// for variable). Reads the file's head to find both; leaves the position anywhere.
fn start_offset(file: &mut File, len: u64, start_s: f32) -> Result<u64, String> {
    let mut head = Vec::with_capacity(HEAD_BYTES);
    file.by_ref()
        .take(HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .map_err(|e| e.to_string())?;
    let audio_at = music::audio_start(&head).min(len);
    if start_s.is_nan() || start_s <= 0.0 {
        return Ok(audio_at);
    }
    let mut audio = Vec::new();
    if audio_at + 4096 > head.len() as u64 {
        file.seek(SeekFrom::Start(audio_at))
            .and_then(|_| file.by_ref().take(4096).read_to_end(&mut audio))
            .map_err(|e| e.to_string())?;
    }
    let Some(secs) = music::probe(&head, &audio, len).secs.filter(|s| *s > 0.0) else {
        return Ok(audio_at);
    };
    let share = f64::from((start_s / secs).clamp(0.0, 1.0));
    Ok(audio_at + ((len - audio_at) as f64 * share) as u64)
}

/// Spawns the file thread for `PlayFile { key: path, start_s, token }`.
pub fn spawn(
    path: String,
    start_s: f32,
    token: u32,
    epoch: u32,
    ring: Arc<Ring>,
    notes: Sender<Note>,
) {
    let failed = notes.clone();
    let spawned = thread::Builder::new()
        .name("radio-file".into())
        .spawn(move || play(&path, start_s, token, epoch, &ring, &notes));
    if let Err(e) = spawned {
        log::warn!("radio: cannot start a file thread: {e}");
        let _ = failed.send(Note::Event(PlayerEvent::Failed { token }));
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::radio::decode::tests::silent_frame;
    use std::sync::mpsc;
    use std::time::Instant;

    /// An ID3v2.3 tag with these text frames (`TIT2`, `TPE1`, ...).
    pub fn id3(frames: &[(&str, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, text) in frames {
            body.extend_from_slice(id.as_bytes());
            body.extend_from_slice(&(text.len() as u32 + 1).to_be_bytes());
            body.extend_from_slice(&[0, 0, 0]); // flags, Latin-1
            body.extend_from_slice(text.as_bytes());
        }
        let n = body.len() as u32;
        let size = [
            (n >> 21) as u8 & 0x7f,
            (n >> 14) as u8 & 0x7f,
            (n >> 7) as u8 & 0x7f,
            n as u8 & 0x7f,
        ];
        let mut tag = b"ID3\x03\x00\x00".to_vec();
        tag.extend_from_slice(&size);
        tag.extend(body);
        tag
    }

    /// An MP3 of `frames` silent frames (~26 ms each) behind an ID3 tag.
    pub fn mp3_file(frames: usize, tag: &[(&str, &str)]) -> Vec<u8> {
        let mut f = id3(tag);
        for _ in 0..frames {
            f.extend(silent_frame());
        }
        f
    }

    fn scratch(name: &str, bytes: &[u8]) -> String {
        let dir = std::env::temp_dir().join(format!("skywave-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// Plays `path` with a reader draining the ring as a callback would; returns the events
    /// and the samples read.
    fn listen(path: &str, start_s: f32) -> (Vec<Note>, usize) {
        let ring = Arc::new(Ring::new(1 << 16));
        let (tx, rx) = mpsc::channel();
        spawn(
            path.to_string(),
            start_s,
            9,
            ring.epoch(),
            Arc::clone(&ring),
            tx,
        );
        let mut buf = [0.0f32; 4096];
        let mut read = 0;
        let mut notes = Vec::new();
        let t0 = Instant::now();
        loop {
            read += ring.pop(&mut buf);
            match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(n) => notes.push(n),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            assert!(t0.elapsed() < Duration::from_secs(20), "{notes:?}");
        }
        (notes, read)
    }

    #[test]
    fn a_file_plays_to_its_end_after_the_ring_drains() {
        let path = scratch("whole.mp3", &mp3_file(200, &[("TIT2", "Song")]));
        let (notes, read) = listen(&path, 0.0);
        let ev = |e| Note::Event(e);
        assert_eq!(
            notes,
            [
                ev(PlayerEvent::Started { token: 9 }),
                ev(PlayerEvent::Ended { token: 9 })
            ]
        );
        // 200 frames of 1152 samples at 44.1 kHz, as 48 kHz stereo.
        let want = 200.0 * 1152.0 * 48_000.0 / 44_100.0 * 2.0;
        assert!((read as f64 - want).abs() < 2400.0, "{read} vs {want}");
    }

    #[test]
    fn a_file_starts_where_the_shuffle_is() {
        // 200 frames ≈ 5.2 s; from 2.6 s about half of it plays.
        let path = scratch("half.mp3", &mp3_file(200, &[]));
        let (notes, read) = listen(&path, 2.6);
        assert_eq!(
            notes.last(),
            Some(&Note::Event(PlayerEvent::Ended { token: 9 }))
        );
        let whole = 200.0 * 1152.0 * 48_000.0 / 44_100.0 * 2.0;
        assert!(
            (read as f64 / whole - 0.5).abs() < 0.05,
            "{read} of {whole}"
        );
    }

    #[test]
    fn missing_and_empty_files_fail() {
        let (notes, _) = listen("/nonexistent/skywave/track.mp3", 0.0);
        assert_eq!(notes, [Note::Event(PlayerEvent::Failed { token: 9 })]);
        let path = scratch("junk.mp3", &[0x55; 30_000]);
        let (notes, read) = listen(&path, 0.0);
        assert_eq!(notes, [Note::Event(PlayerEvent::Failed { token: 9 })]);
        assert_eq!(read, 0);
    }

    #[test]
    fn a_cut_stops_a_file_quietly() {
        let path = scratch("long.mp3", &mp3_file(2000, &[]));
        let ring = Arc::new(Ring::new(1 << 16));
        let (tx, rx) = mpsc::channel();
        spawn(path, 0.0, 4, ring.epoch(), Arc::clone(&ring), tx);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(Note::Event(PlayerEvent::Started { token: 4 }))
        );
        ring.cut();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}
