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

#[cfg(target_arch = "wasm32")]
mod web;

/// The first value of `key` in a URL query string such as `?seed=123&car=palisade_wagon`.
pub fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
}

/// Reads `seed=<u64>` from a URL query string such as `?seed=123&x=1`.
pub fn seed_from_query(query: &str) -> Option<u64> {
    query_param(query, "seed").and_then(|v| v.parse().ok())
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
    fn parses_seed_parameter() {
        assert_eq!(seed_from_query("?seed=123"), Some(123));
        assert_eq!(
            seed_from_query("?a=1&seed=18446744073709551615"),
            Some(u64::MAX)
        );
        assert_eq!(seed_from_query("seed=7&seed=8"), Some(7));
        assert_eq!(seed_from_query(""), None);
        assert_eq!(seed_from_query("?seed=abc"), None);
        assert_eq!(seed_from_query("?myseed=4"), None);
    }

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
    fn wheel_deltas_become_notches_away_from_the_user() {
        assert_eq!(wheel_notches(-100.0, DOM_DELTA_PIXEL), 1.0);
        assert_eq!(wheel_notches(250.0, DOM_DELTA_PIXEL), -2.5);
        assert_eq!(wheel_notches(3.0, DOM_DELTA_LINE), -1.0);
        assert_eq!(wheel_notches(-1.0, DOM_DELTA_PAGE), 1.0);
        assert_eq!(wheel_notches(f64::NAN, DOM_DELTA_PIXEL), 0.0);
    }
}
