//! Stations and the preset files that list them (`assets/radio/<era>_<scenario>.ron`).
//!
//! ```ron
//! RadioPreset(
//!     id: "1984_socal", title: "1984 SoCal", scenario: "socal_beach_1984",
//!     stations: [
//!         Station(
//!             id: "somafm_seventies", name: "SomaFM Left Coast 70s", era: "1970s",
//!             format: "mellow 70s album rock",
//!             source: Stream(
//!                 urls: ["https://ice1.somafm.com/seventies-128-mp3"],
//!                 homepage: "https://somafm.com/seventies/",
//!                 codec: Mp3, bitrate_kbps: 128,
//!                 terms_checked: "2026-10-26", verified: false,
//!             ),
//!         ),
//!     ],
//! )
//! ```
//!
//! A station's name is the real service's own channel name (a fact), shown on the dash as
//! it is. Names and formats are printable ASCII, since the HUD font is.

use serde::{Deserialize, Serialize};

/// Longest station name the dash and the toast take, characters.
pub const MAX_NAME: usize = 48;
/// Longest stream URL, bytes.
pub const MAX_URL: usize = 512;

/// How a stream is encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    Mp3,
    Aac,
}

/// Where a station's music comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Source {
    /// Internet radio over HTTPS (Icecast/SHOUTcast MP3 or AAC). `urls` are tried in order
    /// (mirrors of the same stream); `homepage` credits the broadcaster; `terms_checked` is
    /// the date (`YYYY-MM-DD`) its listening terms were read; `verified` turns true once
    /// someone has heard it play from the listed URL (`tools/check-streams.mjs`).
    Stream {
        urls: Vec<String>,
        homepage: String,
        codec: Codec,
        bitrate_kbps: u16,
        terms_checked: String,
        verified: bool,
    },
    /// The user's own files: a folder whose name suggests this station.
    Files { folder_hint: String },
}

/// One station on the dial.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Station {
    /// Unique within its preset: lowercase letters, digits and `_`.
    pub id: String,
    /// What the dash shows: the service's own channel name.
    pub name: String,
    /// The decade the music mostly comes from, `"1970s"`.
    pub era: String,
    /// A few words on what it plays, for the toast.
    pub format: String,
    pub source: Source,
}

impl Station {
    /// The stream URLs, empty for a files station.
    pub fn urls(&self) -> &[String] {
        match &self.source {
            Source::Stream { urls, .. } => urls,
            Source::Files { .. } => &[],
        }
    }

    /// Checks the station's fields (see the module docs).
    pub fn validate(&self) -> Result<(), String> {
        let at = |what: &str| format!("station `{}`: {what}", self.id);
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(at("id must be lowercase letters, digits and _"));
        }
        if !shown_text(&self.name) || self.name.len() > MAX_NAME {
            return Err(at("name must be 1-48 printable ASCII characters"));
        }
        if !shown_text(&self.format) || !shown_text(&self.era) {
            return Err(at("era and format must be printable ASCII"));
        }
        match &self.source {
            Source::Stream {
                urls,
                homepage,
                bitrate_kbps,
                terms_checked,
                ..
            } => {
                if urls.is_empty() {
                    return Err(at("a stream needs at least one URL"));
                }
                if let Some(u) = urls.iter().find(|u| !stream_url_ok(u)) {
                    return Err(at(&format!("`{u}` is not an https:// URL")));
                }
                if !stream_url_ok(homepage) {
                    return Err(at("homepage must be an https:// URL"));
                }
                if !(16..=512).contains(bitrate_kbps) {
                    return Err(at("bitrate_kbps must be 16-512"));
                }
                if !is_date(terms_checked) {
                    return Err(at("terms_checked must be YYYY-MM-DD"));
                }
            }
            Source::Files { folder_hint } => {
                if folder_hint.trim().is_empty() {
                    return Err(at("folder_hint is empty"));
                }
            }
        }
        Ok(())
    }
}

/// A preset file: the stations of one era and scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioPreset {
    /// `<era>_<scenario>`, the file's name.
    pub id: String,
    /// What the settings page shows.
    pub title: String,
    /// The scenario it was made for (`docs/SCENARIOS.md`).
    pub scenario: String,
    pub stations: Vec<Station>,
}

impl RadioPreset {
    /// Parses and validates a preset file. The error names the preset and, for a parse
    /// failure, the line and column.
    pub fn parse(text: &str) -> Result<RadioPreset, String> {
        let preset: RadioPreset = ron::from_str(text).map_err(|e| format!("radio preset: {e}"))?;
        preset.validate()?;
        Ok(preset)
    }

    /// At least one station, every station valid, no id twice, a printable title.
    pub fn validate(&self) -> Result<(), String> {
        let at = |what: String| format!("radio preset `{}`: {what}", self.id);
        if self.stations.is_empty() {
            return Err(at("no stations".into()));
        }
        if !shown_text(&self.title) {
            return Err(at("title must be printable ASCII".into()));
        }
        for (i, s) in self.stations.iter().enumerate() {
            s.validate().map_err(at)?;
            if self.stations[..i].iter().any(|o| o.id == s.id) {
                return Err(at(format!("station id `{}` twice", s.id)));
            }
        }
        Ok(())
    }
}

/// Non-empty printable ASCII (what the HUD font draws).
fn shown_text(s: &str) -> bool {
    !s.trim().is_empty() && s.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// An `https://` URL with a host, no whitespace or control characters, at most
/// [`MAX_URL`] bytes. Players on an `https://` page cannot fetch plain `http://` streams
/// (mixed content), so presets only list secure ones.
pub fn stream_url_ok(url: &str) -> bool {
    url.len() <= MAX_URL
        && url
            .strip_prefix("https://")
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
        && url.bytes().all(|b| b.is_ascii_graphic())
}

/// `YYYY-MM-DD` with a plausible month and day.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: core::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !digits(0..4) {
        return false;
    }
    if !digits(5..7) || !digits(8..10) {
        return false;
    }
    let month = (b[5] - b'0') * 10 + (b[6] - b'0');
    let day = (b[8] - b'0') * 10 + (b[9] - b'0');
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        RadioPreset(
            id: "1984_test", title: "Test", scenario: "test",
            stations: [
                Station(
                    id: "a", name: "Service A", era: "1970s", format: "soft rock",
                    source: Stream(
                        urls: ["https://a.example/one", "https://b.example/one"],
                        homepage: "https://a.example/",
                        codec: Mp3, bitrate_kbps: 128,
                        terms_checked: "2026-10-26", verified: false,
                    ),
                ),
                Station(
                    id: "mine", name: "My records", era: "1980s", format: "whatever I own",
                    source: Files(folder_hint: "yacht-rock"),
                ),
            ],
        )
    "#;

    #[test]
    fn parses_a_preset() {
        let p = RadioPreset::parse(SAMPLE).unwrap();
        assert_eq!(p.id, "1984_test");
        assert_eq!(p.stations.len(), 2);
        assert_eq!(p.stations[0].urls().len(), 2);
        assert!(matches!(
            p.stations[0].source,
            Source::Stream {
                codec: Codec::Mp3,
                bitrate_kbps: 128,
                verified: false,
                ..
            }
        ));
        assert!(p.stations[1].urls().is_empty());
    }

    #[test]
    fn rejects_bad_presets() {
        let cases: [(&str, &str, &str); 9] = [
            ("\"https://a.example/one\", ", "", "still fine"),
            ("https://a.example/one", "http://a.example/one", "https"),
            ("id: \"a\"", "id: \"A b\"", "lowercase"),
            ("id: \"mine\"", "id: \"a\"", "twice"),
            ("\"2026-10-26\"", "\"26 Oct\"", "YYYY-MM-DD"),
            ("bitrate_kbps: 128", "bitrate_kbps: 4", "bitrate"),
            ("name: \"Service A\"", "name: \"\"", "name"),
            (
                "verified: false,",
                "verified: false, extra: 1,",
                "radio preset:",
            ),
            (
                "folder_hint: \"yacht-rock\"",
                "folder_hint: \" \"",
                "folder_hint",
            ),
        ];
        for (from, to, want) in cases {
            let text = SAMPLE.replacen(from, to, 1);
            match RadioPreset::parse(&text) {
                Ok(_) => assert_eq!(want, "still fine", "{to}"),
                Err(e) => assert!(e.contains(want), "{to}: {e}"),
            }
        }
        let empty = r#"RadioPreset(id: "x", title: "X", scenario: "s", stations: [])"#;
        assert!(RadioPreset::parse(empty)
            .unwrap_err()
            .contains("no stations"));
    }

    #[test]
    fn urls_and_dates() {
        assert!(stream_url_ok("https://ice1.example.com/x-128-mp3"));
        for bad in [
            "http://x",
            "https://",
            "https:///x",
            "https://a b",
            "ftp://x",
            "",
        ] {
            assert!(!stream_url_ok(bad), "{bad}");
        }
        assert!(!stream_url_ok(&format!("https://{}", "a".repeat(MAX_URL))));
        assert!(is_date("2026-10-26"));
        for bad in [
            "2026-13-01",
            "2026-00-10",
            "2026-1-01",
            "2026/10/26",
            "abcd-10-26",
        ] {
            assert!(!is_date(bad), "{bad}");
        }
    }
}
