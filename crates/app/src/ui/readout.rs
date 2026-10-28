//! The corner readout: what the car is playing, at the top right of the driving HUD (the
//! corner opposite the minimap), under the clock and the station toast. A badge for the
//! source (TAPE, CD or MP3 for the deck by era, RADIO) with the mode and transport state
//! beside it, then three lines: artist, album and song for the deck, the station's name
//! and what its stream says is on for the radio (its ICY title natively; the web player
//! reads none, so its format there). A line that does not
//! fit scrolls a character at a time. It shows for [`SHOW_S`] after anything changes (a
//! transport key, a new track or station, the source) and fades out over [`FADE_S`].
//!
//! Text is printable ASCII, as the HUD font draws it: tags keep their letters and lose
//! their accents ("Café" reads "Cafe"); characters outside Latin-1 are left out, and a name
//! with nothing left falls back to the folder's or file's name ([`audio::deck`]).

use core::fmt::{self, Display};

use audio::deck::Source;
use render::{font, HudBuilder};

use super::Ui;
use crate::input::{Action, InputState};
use crate::radio::DashText;
use audio::RadioState;

/// Seconds the readout stays up after a change, then fades out over `FADE_S`.
pub const SHOW_S: f32 = 6.0;
pub const FADE_S: f32 = 1.0;
/// A readout that has nothing new: hidden.
pub const HIDDEN_S: f32 = 1e9;
/// Text scale, and the most characters a line shows before it scrolls.
const S: f32 = 2.0;
const MAX_CHARS: usize = 28;
/// Below the safe area's top: room for the clock and the two lines of the station toast.
const TOP: f32 = 52.0;
/// Gap between the badge and the first line, and between the block and anything it avoids.
const GAP: f32 = 6.0;

const BADGE: [u8; 4] = [255, 178, 62, 255];
const ON_BADGE: [u8; 4] = [26, 18, 8, 255];
const BRIGHT: [u8; 4] = [238, 234, 222, 255];
const DIM: [u8; 4] = [196, 202, 214, 255];
const SHADOW: [u8; 4] = [0, 0, 0, 170];

/// The transport and source keys: any of them shows the readout again.
const KEYS: [Action; 7] = [
    Action::RadioNext,
    Action::RadioPrev,
    Action::RadioOff,
    Action::DeckSource,
    Action::DeckMode,
    Action::AlbumNext,
    Action::AlbumPrev,
];

pub fn any_audio_key(s: &InputState) -> bool {
    KEYS.iter().any(|&a| s.just_pressed(a))
}

/// How opaque the readout is `t` seconds after its last change.
pub fn alpha(t: f32) -> f32 {
    (1.0 - (t - SHOW_S) / FADE_S).clamp(0.0, 1.0)
}

/// What the readout shows: the badge, the status beside it and three lines.
pub(super) struct Content<'a> {
    pub badge: &'static str,
    pub status: [&'static str; 2],
    pub lines: [&'a str; 3],
}

impl Display for Content<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}{}", self.badge, self.status[0], self.status[1])?;
        for l in self.lines {
            write!(f, "\n{l}")?;
        }
        Ok(())
    }
}

impl Ui {
    /// Shows the readout again when the source, the deck's track or the radio's station
    /// or power changed; else runs its clock.
    pub(super) fn note_readout(&mut self, dt: f32) {
        let r = self.radio.radio();
        let seen = (
            self.source(),
            self.deck.deck().changes(),
            r.station(),
            r.is_on(),
            self.radio.now_playing().1,
        );
        match self.readout_seen {
            Some(was) if was != seen => self.readout_s = 0.0,
            _ => self.readout_s += dt,
        }
        self.readout_seen = Some(seen);
    }

    pub(super) fn readout(&self) -> Content<'_> {
        match self.source() {
            Source::Deck => Content {
                badge: self.deck.kind().badge(),
                status: self.deck.status(),
                lines: self.deck.lines(),
            },
            Source::Radio => {
                let r = self.radio.radio();
                let status = if r.is_blocked() {
                    "PRESS A KEY"
                } else {
                    match r.state() {
                        RadioState::Off => "OFF",
                        RadioState::Tuning { .. } => "...",
                        RadioState::Playing { .. } if r.is_live() => ">",
                        RadioState::Playing { .. } => "...",
                        RadioState::Error { .. } => "NO SIGNAL",
                    }
                };
                let lines = match r.channel() {
                    _ if r.channels().is_empty() => ["No stations", "", ""],
                    Some(c) if r.is_on() => {
                        let now = self.radio.now_playing().0;
                        [c.name.as_str(), now.unwrap_or(c.format.as_str()), ""]
                    }
                    _ => ["", "", ""],
                };
                Content {
                    badge: "RADIO",
                    status: ["", status],
                    lines,
                }
            }
        }
    }

    /// The readout as text: badge and status, then a line each (shown or faded).
    pub fn readout_text(&self) -> String {
        self.readout().to_string()
    }

    /// What the cockpit's radio display shows, `chars` at a time: the station, or the
    /// deck's "Artist - Title".
    pub(super) fn dash(&self, chars: usize) -> Option<DashText<'_>> {
        match self.source() {
            Source::Radio => self.radio.dash(chars),
            Source::Deck => self.deck.dash(chars, self.readout_s),
        }
    }

    /// Draws the readout, right-aligned at the safe area's right edge. Lines that would
    /// reach down to `avoid` (the top of the status toast at the bottom left, with how far
    /// right it reaches) while overlapping it are left out until it is gone.
    pub(super) fn draw_readout(&self, hud: &mut HudBuilder, avoid: Option<(f32, f32)>) {
        let a = alpha(self.readout_s);
        if a <= 0.0 {
            return;
        }
        let fade = |c: [u8; 4]| [c[0], c[1], c[2], (f32::from(c[3]) * a) as u8];
        let [sx, sy, sw, _] = hud.safe_rect();
        let right = sx + sw;
        let cell = font::CELL_W as f32 * S;
        let chars = (((sw * 0.5 - GAP) / cell).floor() as usize).clamp(8, MAX_CHARS);
        let left = right - chars as f32 * cell;
        let line_h = font::LINE_HEIGHT as f32 * S;
        let text_h = font::CELL_H as f32 * S;
        // The lowest y the block may reach.
        let floor = match avoid {
            Some((y, reach)) if reach > left - GAP => y - GAP,
            _ => f32::MAX,
        };
        let c = self.readout();
        // The badge: dark letters on a lit box, the status left of it.
        let mut y = sy + TOP;
        let pad = 4.0;
        let badge_w = font::text_width(c.badge) as f32 * S + 2.0 * pad;
        let badge_h = text_h + 2.0;
        if y + badge_h > floor {
            return;
        }
        hud.rect(right - badge_w, y, badge_w, badge_h, fade(BADGE));
        hud.text(right - badge_w + pad, y + 2.0, S, fade(ON_BADGE), c.badge);
        let status_right = right - badge_w - GAP;
        let [m, st] = c.status;
        let w = (font::text_width(m) + font::text_width(st) + 1) as f32 * S;
        if status_right - w >= left {
            text_right(
                hud,
                status_right,
                y + 2.0,
                fade,
                BRIGHT,
                format_args!("{m}{st}"),
            );
        }
        y += badge_h + GAP;
        for (i, line) in c.lines.into_iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            if y + text_h > floor {
                break;
            }
            let text = DashText::scrolled(line, chars, self.readout_s);
            let color = if i == 1 { DIM } else { BRIGHT };
            text_right(hud, right, y, fade, color, format_args!("{text}"));
            y += line_h;
        }
    }
}

/// Text with a drop shadow whose right edge is at `right` (the font is proportional, so it
/// is measured as drawn).
fn text_right(
    hud: &mut HudBuilder,
    right: f32,
    y: f32,
    fade: impl Fn([u8; 4]) -> [u8; 4],
    color: [u8; 4],
    args: fmt::Arguments,
) {
    for (dx, c) in [(S, SHADOW), (0.0, color)] {
        hud.text_fmt(right + dx, y + dx, S, fade(c), args);
        let w = font::text_width(hud.last_text()) as f32 * S;
        hud.shift_last(-w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_shows_then_fades() {
        assert_eq!(alpha(0.0), 1.0);
        assert_eq!(alpha(SHOW_S), 1.0);
        assert!((alpha(SHOW_S + FADE_S * 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(alpha(SHOW_S + FADE_S), 0.0);
        assert_eq!(alpha(HIDDEN_S), 0.0);
    }
}
