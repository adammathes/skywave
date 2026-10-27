//! A tiny menu model, gamepad first.
//!
//! "Immediate-ish": a page's items are rebuilt from the game state whenever they are
//! needed (see `pages::build`), so a menu never holds stale copies of settings; only the
//! navigation state lives here: the page stack, and the focus and scroll of each page.
//! Everything is reachable with up/down (focus, wrapping), left/right (change a value),
//! confirm and back, so a d-pad or stick and two buttons drive every menu; holding a
//! direction repeats it ([`Repeat`]).

use crate::input::{Action, DeviceKind, InputState, Sign};

use super::pages::{ItemId, Page};

/// How a slider's value reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// "1.25x".
    Times,
    /// "75%" (the value is a fraction).
    Percent,
    /// "0.08".
    Plain,
    /// "06:30" (the value is hours).
    Clock,
    /// "62 deg".
    Degrees,
    /// "+3 cm".
    Centimeters,
}

/// What an item is and its current value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Widget {
    Toggle(bool),
    Slider {
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        unit: Unit,
    },
    Choice {
        options: &'static [&'static str],
        index: usize,
    },
    /// Does something when confirmed.
    Action,
    /// Opens another page.
    Submenu(Page),
    /// Remaps one direction (`sign`) of an action on a device: confirming waits for the next
    /// raw input.
    Capture {
        device: DeviceKind,
        action: Action,
        sign: Sign,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub id: ItemId,
    pub label: &'static str,
    pub widget: Widget,
}

/// One navigation input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
}

/// What a navigation input did, for the caller to apply to the game state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    None,
    /// The focus moved.
    Moved,
    Toggle(ItemId, bool),
    Slide(ItemId, f32),
    Choose(ItemId, usize),
    Activate(ItemId),
    Capture {
        device: DeviceKind,
        action: Action,
        sign: Sign,
    },
    /// A page was pushed.
    Opened(Page),
    /// The top page was popped (back to the one below).
    Closed(Page),
    /// Back on the bottom page: leave the menu.
    Exit,
}

/// Deepest page stack (Pause → Settings → Controls → Gamepad → Analog axes → axis).
pub const MAX_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Level {
    page: Page,
    focus: usize,
    scroll: usize,
}

/// The page stack with each page's focus and scroll.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuNav {
    levels: [Level; MAX_DEPTH],
    depth: usize,
}

impl MenuNav {
    pub fn new(root: Page) -> MenuNav {
        let level = Level {
            page: root,
            focus: 0,
            scroll: 0,
        };
        MenuNav {
            levels: [level; MAX_DEPTH],
            depth: 1,
        }
    }

    fn top(&self) -> &Level {
        &self.levels[self.depth - 1]
    }

    fn top_mut(&mut self) -> &mut Level {
        &mut self.levels[self.depth - 1]
    }

    pub fn page(&self) -> Page {
        self.top().page
    }

    /// Pages from the bottom of the stack to the top.
    pub fn path(&self) -> impl Iterator<Item = Page> + '_ {
        self.levels[..self.depth].iter().map(|l| l.page)
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn focus(&self) -> usize {
        self.top().focus
    }

    /// First visible row of the top page.
    pub fn scroll(&self) -> usize {
        self.top().scroll
    }

    /// Opens `page` on top (focus on its first item). Ignored when the stack is full.
    pub fn push(&mut self, page: Page) {
        if self.depth < MAX_DEPTH {
            self.levels[self.depth] = Level {
                page,
                focus: 0,
                scroll: 0,
            };
            self.depth += 1;
        }
    }

    /// Puts the top page's focus on item `index` (kept in range and in view by
    /// [`MenuNav::fit`]).
    pub fn set_focus(&mut self, index: usize) {
        self.top_mut().focus = index;
    }

    /// Closes the top page; false (and nothing changes) on the bottom one.
    pub fn pop(&mut self) -> bool {
        if self.depth > 1 {
            self.depth -= 1;
            true
        } else {
            false
        }
    }

    /// Keeps the focus on an item after the page's items changed, and the focused row
    /// within `rows` visible rows.
    pub fn fit(&mut self, len: usize, rows: usize) {
        let rows = rows.max(1);
        let level = self.top_mut();
        level.focus = level.focus.min(len.saturating_sub(1));
        if level.focus < level.scroll {
            level.scroll = level.focus;
        } else if level.focus >= level.scroll + rows {
            level.scroll = level.focus + 1 - rows;
        }
        level.scroll = level.scroll.min(len.saturating_sub(rows));
    }

    /// Applies one navigation input to the top page, whose items are `items`.
    pub fn apply(&mut self, nav: Nav, items: &[Item]) -> Effect {
        let len = items.len();
        match nav {
            Nav::Back => {
                let page = self.page();
                return if self.pop() {
                    Effect::Closed(page)
                } else {
                    Effect::Exit
                };
            }
            Nav::Up | Nav::Down if len > 0 => {
                let f = self.top().focus.min(len - 1);
                self.top_mut().focus = if nav == Nav::Down {
                    (f + 1) % len
                } else {
                    (f + len - 1) % len
                };
                return Effect::Moved;
            }
            _ => {}
        }
        let Some(item) = items.get(self.top().focus) else {
            return Effect::None;
        };
        let (forward, backward) = (
            matches!(nav, Nav::Right | Nav::Confirm),
            matches!(nav, Nav::Left),
        );
        match item.widget {
            Widget::Toggle(on) if forward || backward => Effect::Toggle(item.id, !on),
            Widget::Slider {
                value,
                min,
                max,
                step,
                ..
            } if nav == Nav::Left || nav == Nav::Right => {
                let dir = if nav == Nav::Right { 1.0 } else { -1.0 };
                let next = step_slider(value, min, max, step, dir);
                if next != value {
                    Effect::Slide(item.id, next)
                } else {
                    Effect::None
                }
            }
            Widget::Choice { options, index } if !options.is_empty() && (forward || backward) => {
                let n = options.len();
                let next = if backward {
                    (index + n - 1) % n
                } else {
                    (index + 1) % n
                };
                Effect::Choose(item.id, next)
            }
            Widget::Action if nav == Nav::Confirm => Effect::Activate(item.id),
            Widget::Submenu(page) if nav == Nav::Confirm || nav == Nav::Right => {
                self.push(page);
                Effect::Opened(page)
            }
            Widget::Capture {
                device,
                action,
                sign,
            } if nav == Nav::Confirm => Effect::Capture {
                device,
                action,
                sign,
            },
            _ => Effect::None,
        }
    }
}

/// One slider step from `value` in direction `dir` (±1), snapped to the step grid from
/// `min` and clamped. Steps are decimal (0.01, 0.05, 0.25), so the result settles on the
/// nearest millionth before rounding to f32: repeated steps never drift, and 0.08 + 0.01
/// is exactly `0.09f32`.
pub fn step_slider(value: f32, min: f32, max: f32, step: f32, dir: f32) -> f32 {
    if step <= 0.0 || !value.is_finite() {
        return value.clamp(min, max);
    }
    let (value, min64, step) = (f64::from(value), f64::from(min), f64::from(step));
    let k = ((value - min64) / step).round() + f64::from(dir);
    let v = ((min64 + k * step) * 1e6).round() / 1e6;
    (v as f32).clamp(min, max)
}

/// Seconds a held direction waits before repeating, then between repeats.
pub const REPEAT_DELAY: f32 = 0.38;
pub const REPEAT_INTERVAL: f32 = 0.07;

/// The menu actions of one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuInput {
    /// Held: up, down, left, right.
    pub held: [bool; 4],
    /// Pressed this frame (even if already released): up, down, left, right.
    pub pressed: [bool; 4],
    pub confirm: bool,
    pub back: bool,
}

const DIRECTIONS: [(Action, Nav); 4] = [
    (Action::MenuUp, Nav::Up),
    (Action::MenuDown, Nav::Down),
    (Action::MenuLeft, Nav::Left),
    (Action::MenuRight, Nav::Right),
];

impl MenuInput {
    pub fn from_state(s: &InputState) -> MenuInput {
        MenuInput {
            held: DIRECTIONS.map(|(a, _)| s.pressed(a)),
            pressed: DIRECTIONS.map(|(a, _)| s.just_pressed(a)),
            confirm: s.just_pressed(Action::MenuConfirm),
            back: s.just_pressed(Action::MenuBack),
        }
    }
}

/// Turns held directions into repeated [`Nav`]s: one on the press, then after
/// [`REPEAT_DELAY`] one every [`REPEAT_INTERVAL`] while held.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Repeat {
    held: [bool; 4],
    timer: [f32; 4],
}

impl Repeat {
    /// Forgets the press edges of whatever is held now, so a direction already down when a
    /// menu opens does not act until pressed again.
    pub fn reset(&mut self, input: &MenuInput) {
        self.held = input.held;
        self.timer = [f32::INFINITY; 4];
    }

    /// At most one navigation input per frame: confirm, back, then the first direction due.
    pub fn update(&mut self, input: &MenuInput, dt: f32) -> Option<Nav> {
        let mut out = if input.confirm {
            Some(Nav::Confirm)
        } else if input.back {
            Some(Nav::Back)
        } else {
            None
        };
        for (i, &(_, nav)) in DIRECTIONS.iter().enumerate() {
            let fire = if input.pressed[i] || (input.held[i] && !self.held[i]) {
                self.timer[i] = REPEAT_DELAY;
                true
            } else if input.held[i] {
                self.timer[i] -= dt;
                if self.timer[i] <= 0.0 {
                    self.timer[i] = (self.timer[i] + REPEAT_INTERVAL).max(0.0);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            self.held[i] = input.held[i];
            if fire && out.is_none() {
                out = Some(nav);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> [Item; 5] {
        [
            Item {
                id: ItemId::Bloom,
                label: "Bloom",
                widget: Widget::Toggle(true),
            },
            Item {
                id: ItemId::UiScale,
                label: "UI scale",
                widget: Widget::Slider {
                    value: 1.0,
                    min: 1.0,
                    max: 2.0,
                    step: 0.25,
                    unit: Unit::Times,
                },
            },
            Item {
                id: ItemId::ClockSpeed,
                label: "Clock",
                widget: Widget::Choice {
                    options: &["a", "b", "c"],
                    index: 0,
                },
            },
            Item {
                id: ItemId::Resume,
                label: "Resume",
                widget: Widget::Action,
            },
            Item {
                id: ItemId::Open(Page::Video),
                label: "Video",
                widget: Widget::Submenu(Page::Video),
            },
        ]
    }

    #[test]
    fn focus_wraps_and_widgets_respond() {
        let items = items();
        let mut m = MenuNav::new(Page::Settings);
        assert_eq!(m.apply(Nav::Up, &items), Effect::Moved);
        assert_eq!(m.focus(), 4);
        assert_eq!(m.apply(Nav::Down, &items), Effect::Moved);
        assert_eq!(m.focus(), 0);
        assert_eq!(
            m.apply(Nav::Confirm, &items),
            Effect::Toggle(ItemId::Bloom, false)
        );
        assert_eq!(
            m.apply(Nav::Left, &items),
            Effect::Toggle(ItemId::Bloom, false)
        );
        m.apply(Nav::Down, &items);
        assert_eq!(m.apply(Nav::Left, &items), Effect::None, "at the minimum");
        assert_eq!(
            m.apply(Nav::Right, &items),
            Effect::Slide(ItemId::UiScale, 1.25)
        );
        assert_eq!(m.apply(Nav::Confirm, &items), Effect::None);
        m.apply(Nav::Down, &items);
        assert_eq!(
            m.apply(Nav::Left, &items),
            Effect::Choose(ItemId::ClockSpeed, 2)
        );
        assert_eq!(
            m.apply(Nav::Right, &items),
            Effect::Choose(ItemId::ClockSpeed, 1)
        );
        m.apply(Nav::Down, &items);
        assert_eq!(m.apply(Nav::Right, &items), Effect::None);
        assert_eq!(
            m.apply(Nav::Confirm, &items),
            Effect::Activate(ItemId::Resume)
        );
        m.apply(Nav::Down, &items);
        assert_eq!(m.apply(Nav::Confirm, &items), Effect::Opened(Page::Video));
        assert_eq!((m.page(), m.depth(), m.focus()), (Page::Video, 2, 0));
        assert_eq!(m.path().collect::<Vec<_>>(), [Page::Settings, Page::Video]);
        assert_eq!(m.apply(Nav::Back, &[]), Effect::Closed(Page::Video));
        assert_eq!((m.page(), m.focus()), (Page::Settings, 4), "focus is kept");
        assert_eq!(m.apply(Nav::Back, &items), Effect::Exit);
        // An empty page only goes back.
        assert_eq!(m.apply(Nav::Down, &[]), Effect::None);
        assert_eq!(m.apply(Nav::Confirm, &[]), Effect::None);
    }

    #[test]
    fn scrolling_keeps_the_focus_visible() {
        let mut m = MenuNav::new(Page::Settings);
        let many = [items()[3]; 20];
        m.fit(20, 6);
        assert_eq!(m.scroll(), 0);
        for _ in 0..8 {
            m.apply(Nav::Down, &many);
        }
        m.fit(20, 6);
        assert_eq!((m.focus(), m.scroll()), (8, 3));
        m.apply(Nav::Up, &many);
        m.apply(Nav::Up, &many);
        m.apply(Nav::Up, &many);
        m.apply(Nav::Up, &many);
        m.apply(Nav::Up, &many);
        m.apply(Nav::Up, &many);
        m.fit(20, 6);
        assert_eq!((m.focus(), m.scroll()), (2, 2));
        // The list shrank under the focus.
        m.fit(1, 6);
        assert_eq!((m.focus(), m.scroll()), (0, 0));
    }

    #[test]
    fn slider_steps_snap_and_clamp() {
        assert_eq!(step_slider(1.0, 1.0, 2.0, 0.25, 1.0), 1.25);
        assert_eq!(step_slider(1.9, 1.0, 2.0, 0.25, 1.0), 2.0);
        assert_eq!(step_slider(0.08, 0.0, 0.3, 0.01, 1.0), 0.09);
        let mut v = 0.25;
        for _ in 0..100 {
            v = step_slider(v, 0.25, 4.0, 0.05, 1.0);
        }
        assert_eq!(v, 4.0);
        assert!(step_slider(f32::NAN, 0.0, 1.0, 0.1, 1.0).is_nan());
    }

    #[test]
    fn held_directions_repeat_after_a_delay() {
        let mut r = Repeat::default();
        let held = |right: bool| MenuInput {
            held: [false, false, false, right],
            ..MenuInput::default()
        };
        let dt = 1.0 / 60.0;
        let mut fired = Vec::new();
        for frame in 0..60 {
            if r.update(&held(frame < 54), dt) == Some(Nav::Right) {
                fired.push(frame);
            }
        }
        // Frame 0, then after ~0.38 s every ~0.07 s until released at frame 54.
        assert_eq!(fired[0], 0);
        assert_eq!(fired[1], 23);
        assert!(fired
            .windows(2)
            .skip(1)
            .all(|w| (4..=5).contains(&(w[1] - w[0]))));
        assert!(fired.len() >= 7 && *fired.last().unwrap() < 54);
        // A tap that came and went between frames still counts once.
        let tap = MenuInput {
            pressed: [true, false, false, false],
            ..MenuInput::default()
        };
        assert_eq!(r.update(&tap, dt), Some(Nav::Up));
        assert_eq!(r.update(&MenuInput::default(), dt), None);
        // Held when the menu opened: nothing until pressed again.
        r.reset(&held(true));
        assert_eq!(r.update(&held(true), dt), None);
        assert_eq!(r.update(&held(false), dt), None);
        assert_eq!(r.update(&held(true), dt), Some(Nav::Right));
        // Confirm and back come first.
        let both = MenuInput {
            confirm: true,
            back: true,
            ..held(true)
        };
        assert_eq!(r.update(&both, dt), Some(Nav::Confirm));
    }
}
