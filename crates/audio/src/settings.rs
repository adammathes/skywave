//! The user's radio settings: volume, power and last station (restored next session),
//! stations switched off, streams the user added, and the preset chosen per scenario. The
//! app stores them as the `radio` section of its settings document.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::radio::Channel;
use crate::station::{stream_url_ok, RadioPreset, MAX_NAME};

/// Most streams a user can add.
pub const MAX_CUSTOM: usize = 16;
/// Most station ids kept switched off.
const MAX_DISABLED: usize = 256;
/// Longest id kept (station or preset), bytes.
const MAX_ID: usize = 64;

/// A stream the user added by URL.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomStream {
    /// `custom_<n>`.
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RadioSettings {
    /// 0 to 1.
    pub volume: f32,
    /// The radio was on when the last session ended: it comes back on.
    pub on: bool,
    /// Id of the station last tuned.
    pub last_station: Option<String>,
    /// Ids of stations left off the dial.
    pub disabled: Vec<String>,
    pub custom: Vec<CustomStream>,
    /// Scenario id to the id of the preset the user picked for it.
    pub presets: BTreeMap<String, String>,
}

impl Default for RadioSettings {
    fn default() -> RadioSettings {
        RadioSettings {
            volume: 0.8,
            on: false,
            last_station: None,
            disabled: Vec::new(),
            custom: Vec::new(),
            presets: BTreeMap::new(),
        }
    }
}

impl RadioSettings {
    /// Everything in range: volume clamped (non-finite: the default), custom streams with
    /// a bad URL, an unprintable name or a repeated id dropped, lists capped, overlong ids
    /// forgotten.
    pub fn sanitized(mut self) -> RadioSettings {
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0.0, 1.0)
        } else {
            RadioSettings::default().volume
        };
        let ok_id = |s: &String| !s.is_empty() && s.len() <= MAX_ID;
        if !self.last_station.as_ref().is_some_and(ok_id) {
            self.last_station = None;
        }
        self.disabled.retain(ok_id);
        self.disabled.sort();
        self.disabled.dedup();
        self.disabled.truncate(MAX_DISABLED);
        let mut custom: Vec<CustomStream> = Vec::new();
        for c in self.custom {
            let printable = !c.name.trim().is_empty()
                && c.name.len() <= MAX_NAME
                && c.name.bytes().all(|b| (0x20..0x7f).contains(&b));
            let fresh = !custom.iter().any(|o| o.id == c.id);
            if ok_id(&c.id) && printable && stream_url_ok(&c.url) && fresh {
                custom.push(c);
            }
        }
        custom.truncate(MAX_CUSTOM);
        self.custom = custom;
        self.presets.retain(|k, v| k.len() <= MAX_ID && ok_id(v));
        self
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.iter().any(|d| d == id)
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) {
        self.disabled.retain(|d| d != id);
        if !on && self.disabled.len() < MAX_DISABLED {
            self.disabled.push(id.to_string());
            self.disabled.sort();
        }
    }

    /// The preset picked for `scenario`, else `default`.
    pub fn preset_for<'a>(&'a self, scenario: &str, default: &'a str) -> &'a str {
        self.presets.get(scenario).map_or(default, String::as_str)
    }

    /// The dial: `preset`'s enabled stream stations in file order, then the user's
    /// enabled streams.
    pub fn channels(&self, preset: &RadioPreset) -> Vec<Channel> {
        let custom = self.custom.iter().map(|c| Channel {
            id: c.id.clone(),
            name: c.name.clone(),
            format: "added stream".to_string(),
            urls: vec![c.url.clone()],
        });
        preset
            .stations
            .iter()
            .filter_map(Channel::from_station)
            .chain(custom)
            .filter(|c| self.is_enabled(&c.id))
            .collect()
    }

    /// Adds a stream by URL, named after its host. Returns the new station's id.
    pub fn add_custom(&mut self, url: &str) -> Result<String, String> {
        let url = url.trim();
        if !stream_url_ok(url) {
            return Err("the stream URL must start with https://".into());
        }
        if self.custom.len() >= MAX_CUSTOM {
            return Err(format!("at most {MAX_CUSTOM} added streams"));
        }
        if let Some(c) = self.custom.iter().find(|c| c.url == url) {
            return Ok(c.id.clone());
        }
        let n = (1..)
            .find(|n| !self.custom.iter().any(|c| c.id == format!("custom_{n}")))
            .unwrap_or(0);
        let host = url["https://".len()..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        let host: String = host
            .chars()
            .filter(|c| c.is_ascii_graphic())
            .take(MAX_NAME - "Stream ".len())
            .collect();
        let id = format!("custom_{n}");
        self.custom.push(CustomStream {
            id: id.clone(),
            name: format!("Stream {host}"),
            url: url.to_string(),
        });
        Ok(id)
    }

    /// Forgets every added stream.
    pub fn clear_custom(&mut self) {
        let ids: Vec<String> = self.custom.drain(..).map(|c| c.id).collect();
        self.disabled.retain(|d| !ids.contains(d));
        if self
            .last_station
            .as_ref()
            .is_some_and(|l| l.starts_with("custom_"))
        {
            self.last_station = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset() -> RadioPreset {
        RadioPreset::parse(
            r#"RadioPreset(id: "p", title: "P", scenario: "s", stations: [
                Station(id: "a", name: "A", era: "1970s", format: "f", source: Stream(
                    urls: ["https://a/1"], homepage: "https://a/", codec: Aac,
                    bitrate_kbps: 64, terms_checked: "2026-10-26", verified: false)),
                Station(id: "f", name: "F", era: "1970s", format: "f",
                    source: Files(folder_hint: "f")),
                Station(id: "b", name: "B", era: "1980s", format: "f", source: Stream(
                    urls: ["https://b/1"], homepage: "https://b/", codec: Mp3,
                    bitrate_kbps: 128, terms_checked: "2026-10-26", verified: false)),
            ])"#,
        )
        .unwrap()
    }

    #[test]
    fn round_trips() {
        let mut s = RadioSettings {
            volume: 0.35,
            on: true,
            last_station: Some("b".into()),
            ..RadioSettings::default()
        };
        s.set_enabled("a", false);
        s.add_custom("https://radio.example.org/live.mp3").unwrap();
        s.presets
            .insert("socal_beach_1984".into(), "1986_north_shore".into());
        let text = ron::to_string(&s).unwrap();
        let back: RadioSettings = ron::from_str(&text).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.clone().sanitized(), s);
        // Missing fields take their defaults.
        let partial: RadioSettings = ron::from_str("(volume: 0.5)").unwrap();
        assert_eq!(
            partial,
            RadioSettings {
                volume: 0.5,
                ..RadioSettings::default()
            }
        );
    }

    #[test]
    fn the_dial_is_enabled_streams_then_added_ones() {
        let mut s = RadioSettings::default();
        let ids = |s: &RadioSettings| -> Vec<String> {
            s.channels(&preset()).into_iter().map(|c| c.id).collect()
        };
        assert_eq!(
            ids(&s),
            ["a", "b"],
            "files stations are not on the web dial"
        );
        let id = s.add_custom(" https://x.example/stream?x=1 ").unwrap();
        assert_eq!(id, "custom_1");
        assert_eq!(s.custom[0].name, "Stream x.example");
        assert_eq!(s.add_custom("https://x.example/stream?x=1").unwrap(), id);
        assert_eq!(ids(&s), ["a", "b", "custom_1"]);
        s.set_enabled("a", false);
        s.set_enabled("a", false);
        assert_eq!(s.disabled, ["a"]);
        assert_eq!(ids(&s), ["b", "custom_1"]);
        s.set_enabled("a", true);
        assert_eq!(ids(&s), ["a", "b", "custom_1"]);
        assert!(s.add_custom("http://insecure.example/").is_err());
        s.set_enabled("custom_1", false);
        s.last_station = Some("custom_1".into());
        s.clear_custom();
        assert!(s.custom.is_empty() && s.disabled.is_empty() && s.last_station.is_none());
        assert_eq!(s.preset_for("socal", "1984_socal"), "1984_socal");
        s.presets.insert("socal".into(), "1977_chicago".into());
        assert_eq!(s.preset_for("socal", "1984_socal"), "1977_chicago");
    }

    #[test]
    fn sanitizing_drops_what_cannot_be_right() {
        let s = RadioSettings {
            volume: f32::INFINITY,
            last_station: Some(String::new()),
            disabled: vec!["b".into(), "a".into(), "b".into(), String::new()],
            custom: vec![
                CustomStream {
                    id: "custom_1".into(),
                    name: "Ok".into(),
                    url: "https://ok/".into(),
                },
                CustomStream {
                    id: "custom_1".into(),
                    name: "Same id".into(),
                    url: "https://ok/2".into(),
                },
                CustomStream {
                    id: "custom_2".into(),
                    name: "Bad url".into(),
                    url: "javascript:alert(1)".into(),
                },
                CustomStream {
                    id: "custom_3".into(),
                    name: "Caf\u{e9}".into(),
                    url: "https://ok/3".into(),
                },
            ],
            ..RadioSettings::default()
        }
        .sanitized();
        assert_eq!(s.volume, 0.8);
        assert_eq!(s.last_station, None);
        assert_eq!(s.disabled, ["a", "b"]);
        assert_eq!(s.custom.len(), 1);
        assert_eq!(
            RadioSettings {
                volume: 4.0,
                ..RadioSettings::default()
            }
            .sanitized()
            .volume,
            1.0
        );
    }
}
