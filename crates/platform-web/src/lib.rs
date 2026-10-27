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
//! opens the settings menu at startup, `?menu=vehicle` the Vehicle page (on the car and paint
//! of `car=` and `paint=`).
//!
//! The town streams round the camera within `app::stream::StreamBudget::WEB`, timed with
//! `performance.now()`; `window.skywaveStreamIdle` turns true when nothing is left to
//! stream (at once with `?stream=0`), and `window.skywaveStreamStats` then holds the
//! streamer's counters.
//!
//! Sound: `sfx` renders the mixer each animation frame for the page's AudioWorklet glue
//! (`window.skywaveSfx`), which starts on the first key, click or touch.
//!
//! The car radio: every frame the app's `audio::RadioCommand`s go to the page's player
//! glue, `window.skywaveRadio` (the radio block of `web/index.html`, an `<audio>` element):
//! `command(kind, url, token, value, name)` with kind `play`, `file` (a music file by its
//! key, `value` the second to start at), `stop` or `volume` (the dial sweep between
//! stations is synthesized in the mixer, so it plays through `sfx`); its reports come back
//! from `takeEvent()` as `"<started|failed|blocked|ended> <token>"`
//! ([`parse_player_event`]), and the radio's state goes to `report(text)` whenever it
//! changes ([`radio_state_text`]). Without the glue the radio stays silent. The page may
//! ask for a stream URL with `window.prompt` (Settings, Audio, "Add stream URL").
//!
//! The player's own music: `window.skywaveMusic` (the music block) holds the folders the
//! user picked or dropped, one station each. The game calls `pick()` for "Add music folder"
//! (false: no user activation to open a picker from, as after a gamepad press), takes each
//! folder read with `takeFolder()` (`{path, name, announce, kept, files: [{key, name,
//! size, head, audio}]}`: `path` the folder's stable id, `head` the file's first
//! `audio::music::HEAD_BYTES`, `audio` a few KiB where its audio starts when a large tag
//! hides it), calls `forget(path)` for a folder the user removed (`"*"`: all), and reads
//! `again` (the folders of last session must be picked again). Files never leave the page.
//!
//! **Backends.** WebGPU is tried first where the browser offers it; if anything fails
//! before the first frame (no adapter, `requestDevice` rejected, the canvas refusing its
//! configuration or giving no texture, an error while the pipelines are built or while a
//! probe frame is drawn, the device lost) the attempt is torn down and the game starts
//! again on WebGL2 on a fresh canvas (a canvas keeps the first kind of context it gave
//! out). The reason goes to the console and to the page (`onNote`). `?gl=1` goes straight
//! to WebGL2; `?gpu=1` uses WebGPU only, failing loudly, for diagnosis. `?diag=1` runs the
//! start-up checks on both backends without the game and reports them as text. See
//! [`fallback`] and docs/RENDERING.md.

#[cfg(target_arch = "wasm32")]
mod diag;
#[cfg(target_arch = "wasm32")]
mod sfx;
#[cfg(target_arch = "wasm32")]
mod web;

/// The first value of `key` in a URL query string such as `?seed=123&car=homestead_wagon`.
/// The scene keys (`seed`, `car`, `cam`, `t`, `pos`, `yaw`, `lights`, `fly`, `still`, `map`,
/// `ui`, `stream`) are read by `app::SceneSetup::from_query`; `menu=1` opens the settings menu,
/// `menu=vehicle` the pause menu's Vehicle page, `selftest=1` runs the determinism self-test
/// and `diag=1` the start-up diagnostics instead of the game, and `gl=1` / `gpu=1` pick the backend ([`BackendChoice`]).
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

/// True when the URL query asks to open the Vehicle page at startup (`menu=vehicle`), for
/// screenshots of it.
pub fn vehicle_menu_from_query(query: &str) -> bool {
    query_param(query, "menu") == Some("vehicle")
}

/// True when the URL query asks to open the World page at startup (`menu=worlds`), for
/// screenshots of it.
pub fn worlds_menu_from_query(query: &str) -> bool {
    query_param(query, "menu") == Some("worlds")
}

/// The graphics backends the browser build can run on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebBackend {
    WebGpu,
    WebGl2,
}

impl WebBackend {
    /// The name the page, the console and the overlay show.
    pub fn label(self) -> &'static str {
        match self {
            WebBackend::WebGpu => "WebGPU",
            WebBackend::WebGl2 => "WebGL2",
        }
    }
}

/// Which backend the URL asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// WebGPU where the browser offers it, else (or when it fails) WebGL2.
    Auto,
    /// `gl=1`: WebGL2 only.
    WebGl2,
    /// `gpu=1`: WebGPU only, no fallback, so its failures stay visible.
    WebGpu,
}

impl BackendChoice {
    /// From the URL query (`gl=1` wins over `gpu=1`), or from the page's `backend` option
    /// ("gl" / "gpu") when it gives one.
    pub fn from_query(query: &str, option: Option<&str>) -> BackendChoice {
        match option {
            Some("gl") => return BackendChoice::WebGl2,
            Some("gpu") => return BackendChoice::WebGpu,
            _ => {}
        }
        if query_param(query, "gl") == Some("1") {
            BackendChoice::WebGl2
        } else if query_param(query, "gpu") == Some("1") {
            BackendChoice::WebGpu
        } else {
            BackendChoice::Auto
        }
    }

    /// The backend to start on, given whether the browser has `navigator.gpu`.
    pub fn first(self, has_webgpu: bool) -> WebBackend {
        match self {
            BackendChoice::Auto if has_webgpu => WebBackend::WebGpu,
            BackendChoice::Auto | BackendChoice::WebGl2 => WebBackend::WebGl2,
            // Tried anyway, so the failure says why.
            BackendChoice::WebGpu => WebBackend::WebGpu,
        }
    }
}

/// Why starting on a backend failed, before the first frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartFailure {
    /// `navigator.gpu` is missing or `requestAdapter()` gave nothing.
    NoWebGpu,
    /// No adapter could drive the canvas.
    NoAdapter,
    /// `requestDevice` rejected (limits or features the browser would not grant).
    Device,
    /// The canvas would not give a surface, take a configuration or hand out a texture.
    Surface,
    /// An error while the pipelines and render targets were built.
    Pipeline,
    /// An error while the probe frame was drawn and presented.
    FirstFrame,
    /// The device was lost during start-up.
    DeviceLost,
    /// The wasm trapped (a panic) before the first frame; seen by the page.
    Panic,
}

impl StartFailure {
    pub const ALL: [StartFailure; 8] = [
        StartFailure::NoWebGpu,
        StartFailure::NoAdapter,
        StartFailure::Device,
        StartFailure::Surface,
        StartFailure::Pipeline,
        StartFailure::FirstFrame,
        StartFailure::DeviceLost,
        StartFailure::Panic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StartFailure::NoWebGpu => "no WebGPU adapter",
            StartFailure::NoAdapter => "no adapter for the canvas",
            StartFailure::Device => "device request rejected",
            StartFailure::Surface => "canvas surface unusable",
            StartFailure::Pipeline => "pipeline creation error",
            StartFailure::FirstFrame => "first frame error",
            StartFailure::DeviceLost => "device lost",
            StartFailure::Panic => "crashed",
        }
    }

    /// The `onNote`/`failure` name the page passes back for [`StartFailure::Panic`] retries.
    pub fn from_label(label: &str) -> Option<StartFailure> {
        StartFailure::ALL.into_iter().find(|f| f.label() == label)
    }
}

/// What to do after starting on `failed` went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fallback {
    /// The backend to try next, if any.
    pub retry: Option<WebBackend>,
    /// One line for the console and the page.
    pub note: String,
}

/// The fallback decision: any WebGPU failure before the first frame retries on WebGL2,
/// unless the URL forced WebGPU (`gpu=1`); a WebGL2 failure is final.
pub fn fallback(
    choice: BackendChoice,
    failed: WebBackend,
    why: StartFailure,
    detail: &str,
) -> Fallback {
    let retry = match (choice, failed) {
        (BackendChoice::WebGpu, _) | (_, WebBackend::WebGl2) => None,
        (_, WebBackend::WebGpu) => Some(WebBackend::WebGl2),
    };
    let detail = detail.trim();
    let mut note = format!("{} failed: {}", failed.label(), why.label());
    if !detail.is_empty() {
        note.push_str(" (");
        // One line (a panic puts its message on the second), and browser messages can
        // run to pages: the first few hundred bytes are the useful part.
        let line = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut end = line.len().min(300);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        note.push_str(&line[..end]);
        note.push(')');
    }
    match retry {
        Some(next) => {
            note.push_str("; falling back to ");
            note.push_str(next.label());
        }
        None if choice == BackendChoice::WebGpu => note.push_str("; no fallback with gpu=1"),
        None => {}
    }
    Fallback { retry, note }
}

/// The canvas's drawing-buffer size for a CSS client size and `devicePixelRatio`: scaled
/// down as a whole (keeping the aspect) to fit `max` (the device's widest texture), at least
/// 1×1. `None` while the canvas has no layout size (hidden, or not laid out yet), so a
/// surface is never configured at 0×0 or from a bogus ratio.
pub fn canvas_size(client: (i32, i32), device_pixel_ratio: f64, max: u32) -> Option<(u32, u32)> {
    let dpr = if device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0 {
        device_pixel_ratio
    } else {
        1.0
    };
    let (w, h) = (f64::from(client.0) * dpr, f64::from(client.1) * dpr);
    if w < 0.5 || h < 0.5 {
        return None;
    }
    let max = f64::from(max.max(1));
    let scale = (max / w.max(h)).min(1.0);
    let fit = |x: f64| ((x * scale).round() as u32).clamp(1, max as u32);
    Some((fit(w), fit(h)))
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
        "ended" => audio::PlayerEvent::Ended { token },
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
    fn backend_choice_from_the_query_and_the_page() {
        assert_eq!(BackendChoice::from_query("", None), BackendChoice::Auto);
        assert_eq!(
            BackendChoice::from_query("?gl=1", None),
            BackendChoice::WebGl2
        );
        assert_eq!(
            BackendChoice::from_query("?seed=2&gpu=1", None),
            BackendChoice::WebGpu
        );
        assert_eq!(
            BackendChoice::from_query("?gpu=1&gl=1", None),
            BackendChoice::WebGl2
        );
        assert_eq!(
            BackendChoice::from_query("?gl=0", None),
            BackendChoice::Auto
        );
        assert_eq!(
            BackendChoice::from_query("?gpu=1", Some("gl")),
            BackendChoice::WebGl2
        );
        assert_eq!(
            BackendChoice::from_query("", Some("gpu")),
            BackendChoice::WebGpu
        );
        assert_eq!(BackendChoice::Auto.first(true), WebBackend::WebGpu);
        assert_eq!(BackendChoice::Auto.first(false), WebBackend::WebGl2);
        assert_eq!(BackendChoice::WebGl2.first(true), WebBackend::WebGl2);
        assert_eq!(BackendChoice::WebGpu.first(false), WebBackend::WebGpu);
    }

    #[test]
    fn every_webgpu_failure_falls_back_to_webgl2() {
        for why in StartFailure::ALL {
            let f = fallback(
                BackendChoice::Auto,
                WebBackend::WebGpu,
                why,
                "the reason:\n  more ",
            );
            assert_eq!(f.retry, Some(WebBackend::WebGl2), "{why:?}");
            assert_eq!(
                f.note,
                format!(
                    "WebGPU failed: {} (the reason: more); falling back to WebGL2",
                    why.label()
                )
            );
            assert_eq!(StartFailure::from_label(why.label()), Some(why));
            // Forced WebGPU never falls back; WebGL2 has nothing to fall back to.
            let forced = fallback(BackendChoice::WebGpu, WebBackend::WebGpu, why, "");
            assert_eq!(forced.retry, None, "{why:?}");
            assert!(
                forced.note.ends_with("no fallback with gpu=1"),
                "{}",
                forced.note
            );
            for choice in [BackendChoice::Auto, BackendChoice::WebGl2] {
                assert_eq!(fallback(choice, WebBackend::WebGl2, why, "x").retry, None);
            }
        }
        let long = "é".repeat(400);
        assert!(
            fallback(
                BackendChoice::Auto,
                WebBackend::WebGpu,
                StartFailure::Device,
                &long
            )
            .note
            .len()
                < 400
        );
    }

    #[test]
    fn canvas_size_follows_the_pixel_ratio_within_the_texture_limit() {
        assert_eq!(canvas_size((1280, 720), 2.0, 8192), Some((2560, 1440)));
        assert_eq!(canvas_size((1280, 720), 1.25, 8192), Some((1600, 900)));
        // Too big for the device: scaled down as a whole.
        assert_eq!(canvas_size((5000, 2500), 2.0, 8192), Some((8192, 4096)));
        assert_eq!(canvas_size((3000, 10), 4.0, 2048), Some((2048, 7)));
        // Not laid out: nothing to configure.
        assert_eq!(canvas_size((0, 720), 2.0, 8192), None);
        assert_eq!(canvas_size((1280, 0), 2.0, 8192), None);
        assert_eq!(canvas_size((-3, 5), 1.0, 8192), None);
        // A bogus ratio counts as 1; a sliver is at least a pixel.
        assert_eq!(canvas_size((640, 360), f64::NAN, 8192), Some((640, 360)));
        assert_eq!(canvas_size((640, 360), 0.0, 8192), Some((640, 360)));
        assert_eq!(canvas_size((1, 1), 0.6, 8192), Some((1, 1)));
    }

    #[test]
    fn reads_other_parameters() {
        let q = "?seed=3&car=homestead_wagon&cam=cockpit&fly=1";
        assert_eq!(query_param(q, "car"), Some("homestead_wagon"));
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
        assert!(vehicle_menu_from_query("?still=1&menu=vehicle"));
        assert!(!vehicle_menu_from_query("?menu=1"));
        assert!(worlds_menu_from_query("?menu=worlds&still=1"));
        assert!(!worlds_menu_from_query("?menu=vehicle"));
        assert!(!menu_from_query("?menu=vehicle"));
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
        use audio::PlayerEvent::{Blocked, Ended, Failed, Started};
        assert_eq!(parse_player_event("started 3"), Some(Started { token: 3 }));
        assert_eq!(parse_player_event("failed 12"), Some(Failed { token: 12 }));
        assert_eq!(parse_player_event("blocked 1"), Some(Blocked { token: 1 }));
        assert_eq!(parse_player_event("ended 4"), Some(Ended { token: 4 }));
        for bad in ["", "started", "started x", "paused 3", "failed -1"] {
            assert_eq!(parse_player_event(bad), None, "{bad}");
        }
        let channel = audio::Channel {
            id: "a".into(),
            name: "A".into(),
            format: "f".into(),
            urls: vec!["https://a/1".into()],
            tracks: Vec::new(),
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
