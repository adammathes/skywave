//! When the native shell starts its next frame: the frame-rate cap (Settings › Video ›
//! Frame rate, every Nth refresh of the panel) and the background rate. The web counts
//! animation frames instead. The simulation is unaffected: it runs at 120 Hz on whatever
//! time passed between frames.

use crate::settings::FrameRate;

/// Frames a second while the window is in the background (unfocused or hidden; the game
/// is paused there, so these frames draw nothing unless the window needs repainting).
pub const BACKGROUND_FPS: u32 = 4;

/// Seconds between frames for the cap `rate` on a panel refreshing `panel_hz` times a
/// second (`None` for [`FrameRate::Panel`]: vsync paces them), or at [`BACKGROUND_FPS`]
/// when the window is in the background.
pub fn frame_interval(rate: FrameRate, panel_hz: f64, focused: bool) -> Option<f64> {
    if !focused {
        return Some(1.0 / f64::from(BACKGROUND_FPS));
    }
    match rate.divisor() {
        1 => None,
        n => Some(f64::from(n) / panel_hz),
    }
}

/// Frame deadlines on a grid `interval` apart, so a cap that divides the panel's rate
/// lands on every Nth refresh and a late frame does not push every later one back.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pacer {
    /// When the next frame is due, seconds (the platform's clock, any origin).
    next: f64,
    /// The interval the grid was laid for.
    interval: f64,
}

impl Pacer {
    /// Whether a frame may start at `now` for one every `interval` seconds (0: every time
    /// asked). `early` is how far ahead of its deadline a frame still counts as on time:
    /// half a refresh for a platform that can start frames only on refreshes (the web),
    /// else 0. When it is due the deadline moves one interval along the grid, or restarts
    /// from `now` when the frame came more than an interval late or the interval changed.
    pub fn due(&mut self, now: f64, interval: f64, early: f64) -> bool {
        if interval != self.interval {
            self.interval = interval;
            self.next = now;
        }
        if now + early < self.next {
            return false;
        }
        self.next = if now - self.next >= interval {
            now + interval
        } else {
            self.next + interval
        };
        true
    }

    /// Seconds from `now` until a frame is due at `interval` (0 when it is).
    pub fn wait(&self, now: f64, interval: f64) -> f64 {
        if interval != self.interval {
            0.0
        } else {
            (self.next - now).max(0.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames asked for at every refresh of a panel at `hz` for `seconds`, capped at
    /// `rate`: the refreshes (indices) that start one.
    fn run(hz: f64, rate: FrameRate, seconds: f64) -> Vec<u32> {
        let mut p = Pacer::default();
        let refresh = 1.0 / hz;
        let interval = frame_interval(rate, hz, true).unwrap_or(0.0);
        (0..(seconds * hz) as u32)
            .filter(|&i| p.due(f64::from(i) * refresh, interval, 0.5 * refresh))
            .collect()
    }

    #[test]
    fn a_cap_takes_every_nth_refresh() {
        for hz in [60.0, 90.0, 144.0] {
            for &rate in FrameRate::offered(hz) {
                let frames = run(hz, rate, 2.0);
                let n = rate.divisor();
                assert!(
                    frames.windows(2).all(|w| w[1] - w[0] == n),
                    "{hz} Hz, {rate:?}: {frames:?}"
                );
                assert_eq!(frames.len() as f64, 2.0 * rate.fps(hz), "{hz} Hz, {rate:?}");
            }
        }
    }

    #[test]
    fn a_late_frame_restarts_the_grid_and_a_new_interval_starts_at_once() {
        let mut p = Pacer::default();
        assert!(p.due(0.0, 0.1, 0.0));
        assert!(!p.due(0.05, 0.1, 0.0));
        assert!((p.wait(0.05, 0.1) - 0.05).abs() < 1e-9);
        assert!(p.due(0.1, 0.1, 0.0));
        // Stalled for half a second: due, and the next is an interval from now.
        assert!(p.due(0.6, 0.1, 0.0));
        assert!(!p.due(0.65, 0.1, 0.0));
        assert!(p.due(0.7, 0.1, 0.0));
        // Back in front from the background: no wait for the old grid.
        assert_eq!(p.wait(0.71, 0.0), 0.0);
        assert!(p.due(0.71, 0.0, 0.0));
        assert!(p.due(0.711, 0.0, 0.0), "no cap: every frame");
    }

    #[test]
    fn the_background_rate_wins_over_the_cap() {
        assert_eq!(frame_interval(FrameRate::Panel, 90.0, true), None);
        assert_eq!(
            frame_interval(FrameRate::Half, 90.0, true),
            Some(2.0 / 90.0)
        );
        assert_eq!(frame_interval(FrameRate::Panel, 90.0, false), Some(0.25));
        assert_eq!(frame_interval(FrameRate::Third, 60.0, false), Some(0.25));
    }
}
