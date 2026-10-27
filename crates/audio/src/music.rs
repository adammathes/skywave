//! The player's own music as stations: a folder of files (or one drop of them) becomes a
//! station on the dial next to the stream presets.
//!
//! The platform finds the files and reads the first [`HEAD_BYTES`] of each; [`probe`]
//! takes the title and artist from an ID3v2 tag (`TIT2`/`TPE1`, or `TT2`/`TP1` in v2.2) and
//! the length from the first MPEG audio frame (its Xing/Info frame count, else the
//! constant bitrate and the file size). Other formats (AAC, Ogg, FLAC, WAV: whatever the
//! platform decodes) get the file name and [`NOMINAL_TRACK_S`].
//!
//! A station plays its tracks in a seeded shuffle that is "already playing": its position is
//! the radio's running clock plus a per-station offset, modulo the playlist's length, so
//! tuning back to a station lands where it would be had it played all along ([`Playlist`]).
//! Everything here is plain integer and float arithmetic: the same files give the same
//! order and position everywhere.

/// Bytes of each file the platform reads for [`probe`]: enough for an ID3v2 tag without
/// cover art in front of the text frames, and the first audio frame.
pub const HEAD_BYTES: usize = 64 * 1024;
/// Length assumed for a track whose length [`probe`] cannot tell, s.
pub const NOMINAL_TRACK_S: f32 = 240.0;
/// Longest label kept for the dash, characters.
pub const MAX_LABEL: usize = 96;
/// Most tracks a station keeps (the rest of a huge folder is left out).
pub const MAX_TRACKS: usize = 4096;

/// What [`probe`] found in the start of a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub title: Option<String>,
    pub artist: Option<String>,
    /// The length, when the file is MPEG audio with a readable first frame.
    pub secs: Option<f32>,
}

/// One track of a station.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// What the platform plays it by (a path, or an index into the page's file list).
    pub key: String,
    /// What the dash shows: "Artist - Title", the title, or the file name; printable ASCII.
    pub label: String,
    /// Its length, s (positive).
    pub secs: f32,
}

impl Track {
    /// The track of file `file_name` (its key `key`) from what [`probe`] read of it.
    pub fn new(key: String, file_name: &str, probe: &Probe) -> Track {
        let stem = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
        let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
        let label = match (&probe.artist, &probe.title) {
            (Some(a), Some(t)) => format!("{a} - {t}"),
            (None, Some(t)) => t.clone(),
            _ => stem.to_string(),
        };
        let mut label = ascii(&label);
        if label.is_empty() {
            label = ascii(stem);
        }
        if label.is_empty() {
            label = "Track".to_string();
        }
        let secs = probe
            .secs
            .filter(|s| s.is_finite() && *s >= 1.0)
            .unwrap_or(NOMINAL_TRACK_S);
        Track { key, label, secs }
    }
}

/// The id of the station made from the folder at `path` (relative to what the user picked
/// or dropped, the picked folder's name first): `files_` and a hash of the path, so the
/// dial, the on/off switches and the last station survive a new session, and two folders
/// with the same name ("Disc 1") stay two stations.
pub fn station_id(path: &str) -> String {
    format!("files_{:08x}", fnv1a(path.as_bytes()) as u32)
}

/// A folder name as the dash shows it: printable ASCII, at most `max` characters, never
/// empty.
pub fn station_name(name: &str, max: usize) -> String {
    let mut s: String = ascii(name).chars().take(max).collect();
    s.truncate(s.trim_end().len());
    if s.is_empty() {
        s = "My music".to_string();
    }
    s
}

/// A files station's tracks in their seeded shuffle, and where in them the station is at a
/// given time.
#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    /// Track indices in play order.
    pub order: Vec<u32>,
    /// Where the station's clock starts, s into the playlist.
    pub offset_s: f64,
    /// The whole playlist's length, s.
    pub total_s: f64,
}

impl Playlist {
    /// The shuffle of `tracks` seeded by `seed` (the station's id hashed).
    pub fn new(tracks: &[Track], seed: u64) -> Playlist {
        let mut rng = seed ^ 0x9e37_79b9_7f4a_7c15;
        let mut order: Vec<u32> = (0..tracks.len() as u32).collect();
        // Fisher-Yates.
        for i in (1..order.len()).rev() {
            let j = (splitmix(&mut rng) % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let total_s: f64 = tracks.iter().map(|t| f64::from(t.secs)).sum();
        let offset_s = if total_s > 0.0 {
            (splitmix(&mut rng) >> 11) as f64 / (1u64 << 53) as f64 * total_s
        } else {
            0.0
        };
        Playlist {
            order,
            offset_s,
            total_s,
        }
    }

    /// Where the station is at `clock_s` of the radio's clock: the place in [`Self::order`]
    /// and the seconds into that track.
    pub fn at(&self, tracks: &[Track], clock_s: f64) -> (usize, f32) {
        if self.order.is_empty() || self.total_s <= 0.0 {
            return (0, 0.0);
        }
        let mut t = (clock_s + self.offset_s).rem_euclid(self.total_s);
        for (i, &k) in self.order.iter().enumerate() {
            let len = f64::from(tracks[k as usize].secs);
            if t < len {
                return (i, t as f32);
            }
            t -= len;
        }
        (self.order.len() - 1, 0.0)
    }
}

/// The station seed of `id`.
pub fn seed(id: &str) -> u64 {
    fnv1a(id.as_bytes())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Printable ASCII for the HUD font: Latin-1 letters lose their accents, other characters
/// are dropped, runs of spaces collapse, at most [`MAX_LABEL`] characters.
fn ascii(s: &str) -> String {
    // U+00C0..=U+00FF without their accents ('_' for the few with no plain letter).
    const FOLD: &[u8; 64] = b"AAAAAAACEEEEIIIIDNOOOOO_OUUUUY_saaaaaaaceeeeiiiidnooooo_ouuuuy_y";
    let mut out = String::new();
    for c in s.chars() {
        let c = match c as u32 {
            0x20..=0x7e => c,
            0xc0..=0xff => match FOLD[(c as u32 - 0xc0) as usize] {
                b'_' => continue,
                b => b as char,
            },
            0x2018 | 0x2019 => '\'',
            0x201c | 0x201d => '"',
            0x2013 | 0x2014 => '-',
            0x09 => ' ',
            _ => continue,
        };
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(c);
        if out.len() >= MAX_LABEL {
            break;
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// Title, artist and length from the first bytes of an audio file (`head`, at most
/// [`HEAD_BYTES`] of it) whose whole size is `file_len` bytes. Anything it cannot read is
/// `None`; it never panics on odd input.
pub fn probe(head: &[u8], file_len: u64) -> Probe {
    let mut p = Probe::default();
    let mut audio_at = 0usize;
    if head.len() >= 10 && &head[..3] == b"ID3" && (2..=4).contains(&head[3]) {
        let major = head[3];
        let flags = head[5];
        let size = syncsafe(&head[6..10]) as usize;
        audio_at = 10 + size + if flags & 0x10 != 0 { 10 } else { 0 };
        let end = (10 + size).min(head.len());
        let mut at = 10;
        if flags & 0x40 != 0 && major >= 3 && end >= 14 {
            // Extended header: its size counts itself in v2.4, not in v2.3.
            let n = if major == 4 {
                syncsafe(&head[10..14]) as usize
            } else {
                (be32(&head[10..14]) as usize).saturating_add(4)
            };
            at += n;
        }
        let (id_len, header_len) = if major == 2 { (3, 6) } else { (4, 10) };
        while at + header_len <= end {
            let id = &head[at..at + id_len];
            if id[0] == 0 {
                break; // padding
            }
            let len = match major {
                2 => {
                    (usize::from(head[at + 3]) << 16)
                        | (usize::from(head[at + 4]) << 8)
                        | usize::from(head[at + 5])
                }
                3 => be32(&head[at + 4..at + 8]) as usize,
                _ => syncsafe(&head[at + 4..at + 8]) as usize,
            };
            let body = at + header_len;
            // `len > end - body`, not `body + len > end`: a junk size must not wrap.
            if len == 0 || len > end - body {
                break;
            }
            let data = &head[body..body + len];
            match id {
                b"TIT2" | b"TT2" => p.title = text_frame(data),
                b"TPE1" | b"TP1" => p.artist = text_frame(data),
                _ => {}
            }
            at = body + len;
        }
    }
    p.secs = mpeg_secs(head, audio_at, file_len);
    p
}

fn syncsafe(b: &[u8]) -> u32 {
    b.iter().fold(0, |n, &x| (n << 7) | u32::from(x & 0x7f))
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// An ID3v2 text frame's first string (encoding byte, then Latin-1, UTF-16 with a BOM,
/// UTF-16BE or UTF-8), trimmed; `None` when empty.
fn text_frame(data: &[u8]) -> Option<String> {
    let (&enc, body) = data.split_first()?;
    let s = match enc {
        0 => body
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect(),
        1 | 2 => {
            let (big, body) = match body {
                [0xff, 0xfe, rest @ ..] => (false, rest),
                [0xfe, 0xff, rest @ ..] => (true, rest),
                _ => (enc == 2, body),
            };
            let units = body
                .chunks_exact(2)
                .map(|c| {
                    if big {
                        u16::from_be_bytes([c[0], c[1]])
                    } else {
                        u16::from_le_bytes([c[0], c[1]])
                    }
                })
                .take_while(|&u| u != 0);
            char::decode_utf16(units)
                .map(|c| c.unwrap_or('\u{fffd}'))
                .collect()
        }
        3 => {
            let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
            String::from_utf8_lossy(&body[..end]).into_owned()
        }
        _ => return None,
    };
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The length of an MPEG audio file from its first frame at or after `from` in `head`
/// (searched up to 4 KiB on): the Xing/Info or VBRI frame count when there is one, else
/// the frame's bitrate over the rest of the file.
fn mpeg_secs(head: &[u8], from: usize, file_len: u64) -> Option<f32> {
    let stop = head.len().min(from.saturating_add(4096));
    let mut at = from;
    while at + 4 <= stop {
        if let Some(f) = frame_header(&head[at..at + 4]) {
            // A real frame is followed by another, or by the end of what we read.
            let next = at + f.len;
            let confirmed = next + 4 > head.len() || frame_header(&head[next..next + 4]).is_some();
            if confirmed {
                return Some(f.duration(head, at, file_len));
            }
        }
        at += 1;
    }
    None
}

struct Frame {
    mpeg1: bool,
    mono: bool,
    kbps: u32,
    rate: u32,
    samples: u32,
    len: usize,
}

impl Frame {
    fn duration(&self, head: &[u8], at: usize, file_len: u64) -> f32 {
        // Side information sits after the 4-byte header; Xing/Info follows it.
        let side = match (self.mpeg1, self.mono) {
            (true, false) => 32,
            (true, true) | (false, false) => 17,
            (false, true) => 9,
        };
        let frames = |tag_at: usize| -> Option<u32> {
            let b = head.get(tag_at..tag_at + 12)?;
            let tag = &b[..4];
            ((tag == b"Xing" || tag == b"Info") && b[7] & 1 != 0).then(|| be32(&b[8..12]))
        };
        let vbri = || -> Option<u32> {
            // "VBRI", version, delay, quality (2 each), bytes, frames (4 each).
            let b = head.get(at + 36..at + 54)?;
            (&b[..4] == b"VBRI").then(|| be32(&b[14..18]))
        };
        if let Some(n) = frames(at + 4 + side).or_else(vbri) {
            return n as f32 * self.samples as f32 / self.rate as f32;
        }
        let bytes = file_len.saturating_sub(at as u64);
        bytes as f32 * 8.0 / (self.kbps as f32 * 1000.0)
    }
}

/// An MPEG-1/2/2.5 Layer III frame header, or `None`.
fn frame_header(b: &[u8]) -> Option<Frame> {
    if b[0] != 0xff || b[1] & 0xe0 != 0xe0 {
        return None;
    }
    let version = (b[1] >> 3) & 3; // 3: MPEG-1, 2: MPEG-2, 0: MPEG-2.5
    let layer = (b[1] >> 1) & 3; // 1: Layer III
    if version == 1 || layer != 1 {
        return None;
    }
    let mpeg1 = version == 3;
    const KBPS_V1: [u32; 15] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ];
    const KBPS_V2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    let bi = usize::from(b[2] >> 4);
    let si = usize::from((b[2] >> 2) & 3);
    if bi == 0 || bi == 15 || si == 3 {
        return None;
    }
    let kbps = if mpeg1 { KBPS_V1[bi] } else { KBPS_V2[bi] };
    let rate = [44100, 48000, 32000][si]
        >> match version {
            3 => 0,
            2 => 1,
            _ => 2,
        };
    let samples = if mpeg1 { 1152 } else { 576 };
    let padding = u32::from((b[2] >> 1) & 1);
    let len = (samples / 8 * kbps * 1000 / rate + padding) as usize;
    Some(Frame {
        mpeg1,
        mono: b[3] >> 6 == 3,
        kbps,
        rate,
        samples,
        len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ID3v2.`major` tag with these frames (v2.3 sizes are plain, v2.4 syncsafe).
    fn id3(major: u8, frames: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, data) in frames {
            body.extend_from_slice(*id);
            let n = data.len() as u32;
            let size = if major == 4 {
                [
                    (n >> 21) as u8 & 0x7f,
                    (n >> 14) as u8 & 0x7f,
                    (n >> 7) as u8 & 0x7f,
                    n as u8 & 0x7f,
                ]
            } else {
                n.to_be_bytes()
            };
            body.extend_from_slice(&size);
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(data);
        }
        body.extend_from_slice(&[0; 16]); // padding
        let n = body.len() as u32;
        let mut tag = vec![b'I', b'D', b'3', major, 0, 0];
        tag.extend_from_slice(&[
            (n >> 21) as u8 & 0x7f,
            (n >> 14) as u8 & 0x7f,
            (n >> 7) as u8 & 0x7f,
            n as u8 & 0x7f,
        ]);
        tag.extend_from_slice(&body);
        tag
    }

    /// `n` silent MPEG-1 Layer III frames, 128 kb/s, 44.1 kHz, mono (417 bytes each).
    fn frames(n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..n {
            let mut f = vec![0u8; 417];
            f[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0xc0]);
            out.extend_from_slice(&f);
        }
        out
    }

    #[test]
    fn reads_titles_and_artists_in_every_encoding() {
        let utf16: Vec<u8> = [1u8, 0xff, 0xfe]
            .into_iter()
            .chain("Caf\u{e9} Noir".encode_utf16().flat_map(u16::to_le_bytes))
            .chain([0, 0])
            .collect();
        let tag = id3(3, &[(b"TPE1", b"\0The Example Band\0"), (b"TIT2", &utf16)]);
        let p = probe(&tag, tag.len() as u64);
        assert_eq!(p.artist.as_deref(), Some("The Example Band"));
        assert_eq!(p.title.as_deref(), Some("Caf\u{e9} Noir"));
        let tag = id3(
            4,
            &[(b"TIT2", "\u{3}Ni\u{f1}a".as_bytes()), (b"APIC", &[0; 300])],
        );
        assert_eq!(probe(&tag, 0).title.as_deref(), Some("Ni\u{f1}a"));
        let be: Vec<u8> = [2u8]
            .into_iter()
            .chain("Big".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(
            probe(&id3(4, &[(b"TIT2", &be)]), 0).title.as_deref(),
            Some("Big")
        );
        // v2.2: three-letter ids and sizes.
        let mut v22 = vec![b'I', b'D', b'3', 2, 0, 0, 0, 0, 0, 16];
        v22.extend_from_slice(b"TT2\0\0\x06\0Short");
        v22.extend_from_slice(&[0; 3]);
        assert_eq!(probe(&v22, 0).title.as_deref(), Some("Short"));
    }

    #[test]
    fn odd_input_gives_nothing_and_never_panics() {
        assert_eq!(probe(&[], 0), Probe::default());
        assert_eq!(probe(b"ID3", 3), Probe::default());
        // A frame claiming more than the tag holds, an empty title, a bad encoding.
        let mut tag = id3(3, &[(b"TIT2", b"\0")]);
        assert_eq!(probe(&tag, 0).title, None);
        tag[17] = 0xff;
        let _ = probe(&tag, 0);
        // A v2.3 size near u32::MAX (would wrap a 32-bit `body + len`).
        tag[14..18].copy_from_slice(&[0xff, 0xff, 0xff, 0x00]);
        assert_eq!(probe(&tag, 0).title, None);
        assert_eq!(probe(&id3(3, &[(b"TIT2", b"\x07abc")]), 0).title, None);
        // Every truncation of a tagged file.
        let mut file = id3(4, &[(b"TIT2", b"\0abc")]);
        file.extend(frames(3));
        for n in 0..file.len() {
            let _ = probe(&file[..n], file.len() as u64);
        }
    }

    #[test]
    fn mp3_length_from_the_frame_count_or_the_bitrate() {
        // CBR: 2 s at 128 kb/s after a tag.
        let mut file = id3(3, &[(b"TIT2", b"\0Two seconds")]);
        let tag = file.len();
        let n = (2.0 * 44100.0 / 1152.0) as usize + 1;
        file.extend(frames(n));
        let secs = probe(&file, file.len() as u64).secs.unwrap();
        let want = (file.len() - tag) as f32 * 8.0 / 128_000.0;
        assert!((secs - want).abs() < 1e-3, "{secs} vs {want}");
        // An Info frame says 1000 frames: 1000 × 1152 / 44100 s whatever the size.
        let mut info = frames(2);
        info[4 + 17..4 + 17 + 12]
            .copy_from_slice(&[b'I', b'n', b'f', b'o', 0, 0, 0, 1, 0, 0, 0x03, 0xe8]);
        let secs = probe(&info, 10_000_000).secs.unwrap();
        assert!((secs - 1000.0 * 1152.0 / 44100.0).abs() < 1e-3, "{secs}");
        // A VBRI frame (32 bytes after the header) says 500 frames over 5 MB.
        let mut vbri = frames(2);
        let mut tag = b"VBRI\0\x01\0\0\0\x4b".to_vec();
        tag.extend_from_slice(&5_000_000u32.to_be_bytes());
        tag.extend_from_slice(&500u32.to_be_bytes());
        vbri[36..36 + tag.len()].copy_from_slice(&tag);
        let secs = probe(&vbri, 5_000_000).secs.unwrap();
        assert!((secs - 500.0 * 1152.0 / 44100.0).abs() < 1e-3, "{secs}");
        // Not MPEG audio (an Ogg page): no length.
        assert_eq!(probe(b"OggS\0\x02\0\0\0\0\0\0\0\0", 1000).secs, None);
    }

    #[test]
    fn labels_are_ascii_and_fall_back_to_the_file_name() {
        let p = |artist: Option<&str>, title: Option<&str>| Probe {
            artist: artist.map(str::to_string),
            title: title.map(str::to_string),
            secs: Some(200.0),
        };
        let t = Track::new(
            "0".into(),
            "a/01 Song.mp3",
            &p(Some("Band"), Some("Caf\u{e9}  \u{2018}Noir\u{2019}")),
        );
        assert_eq!(t.label, "Band - Cafe 'Noir'");
        assert_eq!(t.secs, 200.0);
        assert_eq!(
            Track::new("0".into(), "a/01 Song.mp3", &p(None, Some("Only"))).label,
            "Only"
        );
        assert_eq!(
            Track::new("0".into(), "dir\\01 Song.mp3", &p(Some("Band"), None)).label,
            "01 Song"
        );
        let t = Track::new("0".into(), "\u{65e5}\u{672c}.ogg", &Probe::default());
        assert_eq!((t.label.as_str(), t.secs), ("Track", NOMINAL_TRACK_S));
        assert!(
            Track::new("0".into(), &"x".repeat(500), &Probe::default())
                .label
                .len()
                <= MAX_LABEL
        );
        assert_eq!(station_name("  Yacht \u{2014} Rock  ", 48), "Yacht - Rock");
        assert_eq!(station_name("\u{65e5}", 48), "My music");
        assert_eq!(station_id("Yacht Rock"), station_id("Yacht Rock"));
        assert_ne!(station_id("Yacht Rock"), station_id("New Wave"));
        assert!(station_id("x").starts_with("files_") && station_id("x").len() == 14);
    }

    fn tracks(secs: &[f32]) -> Vec<Track> {
        secs.iter()
            .enumerate()
            .map(|(i, &s)| Track {
                key: i.to_string(),
                label: format!("T{i}"),
                secs: s,
            })
            .collect()
    }

    #[test]
    fn the_shuffle_is_seeded_and_covers_every_track() {
        let t = tracks(&[100.0; 50]);
        let a = Playlist::new(&t, seed("files_1"));
        assert_eq!(
            a,
            Playlist::new(&t, seed("files_1")),
            "same seed, same order"
        );
        assert_ne!(a.order, Playlist::new(&t, seed("files_2")).order);
        let mut sorted = a.order.clone();
        sorted.sort();
        assert_eq!(sorted, (0..50).collect::<Vec<u32>>());
        assert_ne!(a.order, sorted, "shuffled");
        assert!(a.offset_s >= 0.0 && a.offset_s < a.total_s);
        // Pinned, so a change to the shuffle is deliberate.
        assert_eq!(&a.order[..6], &[19, 2, 47, 12, 44, 40]);
        assert!(Playlist::new(&[], 1).order.is_empty());
    }

    #[test]
    fn resume_lands_where_the_station_would_be() {
        let t = tracks(&[100.0, 50.0, 200.0]);
        let mut p = Playlist::new(&t, 7);
        p.order = vec![2, 0, 1];
        p.offset_s = 0.0;
        assert_eq!(p.at(&t, 0.0), (0, 0.0));
        assert_eq!(p.at(&t, 150.0), (0, 150.0));
        assert_eq!(p.at(&t, 250.0), (1, 50.0));
        assert_eq!(p.at(&t, 330.0), (2, 30.0));
        // Wraps round the playlist's 350 s, and the offset shifts it.
        assert_eq!(p.at(&t, 350.0 * 3.0 + 10.0), (0, 10.0));
        p.offset_s = 300.0;
        assert_eq!(p.at(&t, 0.0), (2, 0.0));
        assert_eq!(p.at(&t, 60.0), (0, 10.0));
        assert_eq!(Playlist::new(&[], 1).at(&[], 5.0), (0, 0.0));
    }
}
