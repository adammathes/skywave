//! Browser shell for Skywave: owns the canvas, picks WebGPU or falls back to WebGL2,
//! feeds keyboard, mouse and gamepad input to the input mapper and drives the frame loop
//! with `requestAnimationFrame`.
//!
//! JavaScript entry point (see `web/index.html`):
//! `await run(canvas, backend => { window.skywaveReady = true; })`.
//!
//! Gamepads are polled with `navigator.getGamepads()` every animation frame and read by
//! the W3C standard mapping (`app::pad_table`). Pads the browser does not map as
//! "standard" are used, by the same indices, only while no standard pad is connected.
//!
//! Mouse look: pressing the right button on the canvas requests pointer lock and releasing
//! it exits, so look-while-held needs no separate capture click. Browsers that refuse the
//! lock keep a free cursor, and `movementX/Y` still drive the look.
//!
//! Settings and bindings are kept in `localStorage` (key `skywave.settings`). `?menu=1`
//! opens the settings menu at startup.
//!
//! The town streams round the camera within `app::stream::StreamBudget::WEB`, timed with
//! `performance.now()`; `window.skywaveStreamIdle` turns true when nothing is left to
//! stream (at once with `?stream=0`), and `window.skywaveStreamStats` then holds the
//! streamer's counters.
//!
//! The car radio: every frame the app's `audio::RadioCommand`s go to the page's player
//! glue, `window.skywaveRadio` (the radio block of `web/index.html`, an `<audio>` element):
//! `command(kind, url, token, value, name)` with kind `play`, `stop`, `volume` or `noise`;
//! its reports come back from `takeEvent()` as `"<started|failed|blocked> <token>"`
//! ([`parse_player_event`]), and the radio's state goes to `report(text)` whenever it
//! changes ([`radio_state_text`]). Without the glue the radio stays silent. The page may
//! ask for a stream URL with `window.prompt` (Settings, Audio, "Add stream URL").

#[cfg(target_arch = "wasm32")]
mod web;

/// The first value of `key` in a URL query string such as `?seed=123&car=palisade_wagon`.
/// The scene keys (`seed`, `car`, `cam`, `t`, `pos`, `yaw`, `lights`, `fly`, `still`, `map`,
/// `ui`, `stream`) are read by `app::SceneSetup::from_query`; `menu=1` opens the settings menu and
/// `selftest=1` runs the determinism self-test instead of the game.
pub fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
}

/// True when the URL query asks to open the settings menu at startup (`menu=1`), for
/// screenshots of the menu.
pub fn menu_from_query(query: &str) -> bool {
    query_param(query, "menu") == Some("1")
}

/// Picks "the" gamepad from the connected ones, given as `(index, has standard mapping)`.
/// Standard pads win over others; among equals the current pad stays, else the first
/// listed (lowest index) is taken.
pub fn choose_pad(
    current: Option<u32>,
    pads: impl IntoIterator<Item = (u32, bool)>,
) -> Option<u32> {
    let mut first_standard = None;
    let mut first_any = None;
    let mut current_standard = None;
    for (index, standard) in pads {
        if Some(index) == current {
            current_standard = Some(standard);
        }
        if standard && first_standard.is_none() {
            first_standard = Some(index);
        }
        if first_any.is_none() {
            first_any = Some(index);
        }
    }
    match (current_standard, first_standard) {
        (Some(true), _) => current,
        (_, Some(s)) => Some(s),
        (Some(false), None) => current,
        (None, None) => first_any,
    }
}

/// A report from the page's radio glue: `"started 3"`, `"failed 3"` or `"blocked 3"`.
pub fn parse_player_event(text: &str) -> Option<audio::PlayerEvent> {
    let (kind, token) = text.split_once(' ')?;
    let token = token.parse().ok()?;
    Some(match kind {
        "started" => audio::PlayerEvent::Started { token },
        "failed" => audio::PlayerEvent::Failed { token },
        "blocked" => audio::PlayerEvent::Blocked { token },
        _ => return None,
    })
}

/// What the page's radio glue is told about the radio (for debugging and the browser
/// tests): `off`, `tuning <id>`, `playing <id>` (asked to play), `live <id>` (sound is
/// coming out) or `error <id>`, then ` blocked` while the browser waits for a gesture.
pub fn radio_state_text(radio: &audio::Radio) -> String {
    let id = radio.channel().map_or("", |c| c.id.as_str());
    let state = match radio.state() {
        audio::RadioState::Off => return "off".to_string(),
        audio::RadioState::Tuning { .. } => "tuning",
        audio::RadioState::Playing { .. } if radio.is_live() => "live",
        audio::RadioState::Playing { .. } => "playing",
        audio::RadioState::Error { .. } => "error",
    };
    let blocked = if radio.is_blocked() { " blocked" } else { "" };
    format!("{state} {id}{blocked}")
}

/// `WheelEvent.deltaMode` values.
pub const DOM_DELTA_PIXEL: u32 = 0;
pub const DOM_DELTA_LINE: u32 = 1;
pub const DOM_DELTA_PAGE: u32 = 2;
/// One wheel notch is about this many pixels (Chrome, Safari; native uses the same) or
/// lines (Firefox).
const PIXELS_PER_NOTCH: f64 = 100.0;
const LINES_PER_NOTCH: f64 = 3.0;

/// A `WheelEvent`'s vertical scroll in notches, + away from the user (DOM `deltaY` is +
/// toward the user).
pub fn wheel_notches(delta_y: f64, delta_mode: u32) -> f32 {
    let per_notch = match delta_mode {
        DOM_DELTA_PIXEL => PIXELS_PER_NOTCH,
        DOM_DELTA_LINE => LINES_PER_NOTCH,
        _ => 1.0,
    };
    let n = -delta_y / per_notch;
    if n.is_finite() {
        n as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_other_parameters() {
        let q = "?seed=3&car=palisade_wagon&cam=cockpit&fly=1";
        assert_eq!(query_param(q, "car"), Some("palisade_wagon"));
        assert_eq!(query_param(q, "cam"), Some("cockpit"));
        assert_eq!(query_param(q, "fly"), Some("1"));
        assert_eq!(query_param(q, "ca"), None);
        assert_eq!(query_param("?cars=2", "car"), None);
        assert_eq!(query_param("", "car"), None);
    }

    #[test]
    fn parses_menu_parameter() {
        assert!(menu_from_query("?menu=1"));
        assert!(menu_from_query("?seed=4&menu=1"));
        assert!(!menu_from_query("?menu=0"));
        assert!(!menu_from_query("?mymenu=1"));
        assert!(!menu_from_query(""));
    }

    #[test]
    fn standard_pads_win_and_the_current_pad_sticks() {
        /// (current, connected pads as (index, standard), chosen)
        type Case = (Option<u32>, &'static [(u32, bool)], Option<u32>);
        let cases: [Case; 9] = [
            (None, &[], None),
            (Some(2), &[], None),
            (None, &[(0, true), (1, true)], Some(0)),
            (Some(1), &[(0, true), (1, true)], Some(1)),
            // A non-standard pad is skipped while a standard one is there...
            (None, &[(0, false), (1, true)], Some(1)),
            (Some(0), &[(0, false), (1, true)], Some(1)),
            // ...and used when it is all there is.
            (None, &[(0, false), (3, false)], Some(0)),
            (Some(3), &[(0, false), (3, false)], Some(3)),
            // The current pad went away.
            (Some(0), &[(1, false), (2, true)], Some(2)),
        ];
        for (current, pads, want) in cases {
            assert_eq!(
                choose_pad(current, pads.iter().copied()),
                want,
                "{current:?} {pads:?}"
            );
        }
    }

    #[test]
    fn radio_reports_and_states_read_and_print() {
        use audio::PlayerEvent::{Blocked, Failed, Started};
        assert_eq!(parse_player_event("started 3"), Some(Started { token: 3 }));
        assert_eq!(parse_player_event("failed 12"), Some(Failed { token: 12 }));
        assert_eq!(parse_player_event("blocked 1"), Some(Blocked { token: 1 }));
        for bad in ["", "started", "started x", "paused 3", "failed -1"] {
            assert_eq!(parse_player_event(bad), None, "{bad}");
        }
        let channel = audio::Channel {
            id: "a".into(),
            name: "A".into(),
            format: "f".into(),
            urls: vec!["https://a/1".into()],
        };
        let mut r = audio::Radio::new(vec![channel]);
        assert_eq!(radio_state_text(&r), "off");
        r.next();
        assert_eq!(radio_state_text(&r), "tuning a");
        r.tick(1.0);
        assert_eq!(radio_state_text(&r), "playing a");
        r.on_event(Blocked { token: r.token() });
        assert_eq!(radio_state_text(&r), "playing a blocked");
        r.on_event(Started { token: r.token() });
        assert_eq!(radio_state_text(&r), "live a");
        r.on_event(Failed { token: r.token() });
        assert_eq!(radio_state_text(&r), "error a");
    }

    #[test]
    fn wheel_deltas_become_notches_away_from_the_user() {
        assert_eq!(wheel_notches(-100.0, DOM_DELTA_PIXEL), 1.0);
        assert_eq!(wheel_notches(250.0, DOM_DELTA_PIXEL), -2.5);
        assert_eq!(wheel_notches(3.0, DOM_DELTA_LINE), -1.0);
        assert_eq!(wheel_notches(-1.0, DOM_DELTA_PAGE), 1.0);
        assert_eq!(wheel_notches(f64::NAN, DOM_DELTA_PIXEL), 0.0);
    }
}
