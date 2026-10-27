//! The controls card: what every action is on the keyboard and on the controller, read
//! from the live bindings so a remap shows. It shows for [`START_SECONDS`] when a running
//! game starts (never in a still scene) until any input, and comes and goes on the Controls
//! action (H or F1; Start held on the pad) or the pause menu's Show controls item. It pauses
//! nothing, but while asked for it takes the keys and sticks as the full map does (the car
//! coasts); it takes the driving HUD's place and hides the performance overlay. Two
//! columns, Keyboard and Controller, beside each row's name when the safe area is wide
//! enough (UI scale up to 1.25); narrower, each row's inputs go on a line under its name.
//! When it does not fit (any scale over 1) it scrolls with the menu's up and down.

use core::fmt::{self, Display};

use render::{font, HudBuilder};

use crate::input::{Action, Binding, DeviceKind, InputState, Key, PadAxis, PadButton, RawInput};
use crate::settings::CONTROLS_DISMISSALS;

use super::draw::{self, ACCENT, DIM, PANEL, RULE, SHADE, TEXT};
use super::names;
use super::Ui;

/// Seconds the card shows at the start of a game.
pub const START_SECONDS: f32 = 6.0;
/// The longest frame the start timer counts, s, so a loading hitch does not use it up.
const MAX_TICK: f32 = 0.1;
/// Text scale, line height, padding, column gap and the narrow layout's indent, virtual units.
const S: f32 = 2.0;
const TITLE_S: f32 = 3.0;
const LH: f32 = 22.0;
const PAD: f32 = 16.0;
const GAP: f32 = 16.0;
const INDENT: f32 = 16.0;
const MAX_W: f32 = 1180.0;
/// The narrowest input column of the wide layout: 15 characters.
const MIN_CELL_W: f32 = 15.0 * 16.0;
/// Lines one press of up or down scrolls.
const SCROLL_STEP: usize = 2;

/// One row of the card: its group, its name and the actions whose inputs it lists. A
/// `merge` row names inputs by family (the arrow keys, the D-pad, a stick) and lists each
/// once, so all four directions of a stick read "LS".
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub group: &'static str,
    pub label: &'static str,
    pub actions: &'static [Action],
    pub merge: bool,
}

const fn row(group: &'static str, label: &'static str, actions: &'static [Action]) -> Row {
    Row {
        group,
        label,
        actions,
        merge: false,
    }
}

const fn merged(group: &'static str, label: &'static str, actions: &'static [Action]) -> Row {
    Row {
        group,
        label,
        actions,
        merge: true,
    }
}

/// Every action but the free-fly camera's, once each.
pub const ROWS: [Row; 29] = [
    merged("Drive", "Steer", &[Action::Steer]),
    row("Drive", "Throttle", &[Action::Throttle]),
    row("Drive", "Brake / reverse", &[Action::Brake]),
    row("Drive", "Handbrake", &[Action::Handbrake]),
    row("Drive", "Horn", &[Action::Horn]),
    row("Drive", "Reset car", &[Action::Reset]),
    row("Drive", "Next car", &[Action::VehicleNext]),
    row("Camera", "Cycle camera", &[Action::CameraCycle]),
    merged("Camera", "Look around", &[Action::LookX, Action::LookY]),
    row("Camera", "Look back", &[Action::LookBack]),
    row("Lights", "Headlights", &[Action::Headlights]),
    row("Lights", "High beams", &[Action::HighBeams]),
    row("Radio", "On/off; deck play/pause", &[Action::RadioOff]),
    row("Radio", "Next station / track", &[Action::RadioNext]),
    row("Radio", "Prev. station / track", &[Action::RadioPrev]),
    row("Radio", "Radio / deck", &[Action::DeckSource]),
    row("Radio", "Next album", &[Action::AlbumNext]),
    row("Radio", "Previous album", &[Action::AlbumPrev]),
    row("Radio", "Deck mode", &[Action::DeckMode]),
    row("Menus", "Pause menu", &[Action::Pause]),
    row("Menus", "Map", &[Action::MapToggle]),
    row("Menus", "Map zoom", &[Action::MapZoom]),
    row("Menus", "Time of day", &[Action::TimeScrub]),
    merged(
        "Menus",
        "Move",
        &[
            Action::MenuUp,
            Action::MenuDown,
            Action::MenuLeft,
            Action::MenuRight,
        ],
    ),
    row("Menus", "Select", &[Action::MenuConfirm]),
    row("Menus", "Back", &[Action::MenuBack]),
    row("Help", "This card", &[Action::Controls]),
    row("Help", "Performance", &[Action::DebugOverlay]),
    row("Help", "Screenshot", &[Action::Screenshot]),
];

/// Why the card is up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shown {
    /// At the start of the game, with this many seconds left.
    Start { remaining: f32 },
    /// The player asked for it.
    Asked,
}

/// The card's state and the text buffers it reuses.
#[derive(Clone, Debug, Default)]
pub struct ControlsCard {
    shown: Option<Shown>,
    /// The first line shown.
    scroll: usize,
    /// Lines that fit and lines in all, as last drawn.
    fit: usize,
    total: usize,
    /// The card used this frame's input (to close or scroll it).
    pub(super) took: bool,
    cell: String,
    part: String,
}

/// One line of the card's body.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    /// The narrow layout's group heading.
    Group(&'static str),
    /// A row: its name and inputs (wide), or its name alone (narrow).
    Row(usize),
    /// The narrow layout's line of a row's inputs.
    Inputs(usize),
}

/// The card's lines in order.
fn lines(wide: bool) -> impl Iterator<Item = Line> {
    ROWS.iter().enumerate().flat_map(move |(i, r)| {
        let first = i == 0 || ROWS[i - 1].group != r.group;
        let group = (!wide && first).then_some(Line::Group(r.group));
        let inputs = (!wide).then_some(Line::Inputs(i));
        [group, Some(Line::Row(i)), inputs].into_iter().flatten()
    })
}

/// Any action pressed this frame, or the wheel turned.
fn any_input(s: &InputState) -> bool {
    Action::ALL.iter().any(|&a| s.just_pressed(a)) || s.wheel() != 0.0
}

/// The family an input belongs to on a `merge` row.
fn family(raw: RawInput) -> Option<&'static str> {
    match raw {
        RawInput::Key(Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight) => {
            Some("Arrows")
        }
        RawInput::PadButton(
            PadButton::DpadUp | PadButton::DpadDown | PadButton::DpadLeft | PadButton::DpadRight,
        ) => Some("D-pad"),
        RawInput::PadAxis(PadAxis::LeftX | PadAxis::LeftY) => Some("LS"),
        RawInput::PadAxis(PadAxis::RightX | PadAxis::RightY) => Some("RS"),
        RawInput::MouseAxis(_) => Some("Mouse"),
        _ => None,
    }
}

fn write_entry(out: &mut String, b: &Binding, merge: bool) {
    use fmt::Write;
    match (merge, family(b.raw)) {
        (true, Some(name)) => {
            if let Some(hold) = b.hold {
                let _ = names::write_raw(out, hold, crate::input::Sign::Full);
                out.push_str(" + ");
            }
            out.push_str(name);
        }
        _ => {
            let _ = write!(out, "{}", draw::BindingName(b));
        }
    }
}

/// Writes a row's inputs on `device` into `out`: each input once, " / " between them,
/// "-" for none.
fn write_cell(out: &mut String, part: &mut String, ui: &Ui, device: DeviceKind, row: &Row) {
    out.clear();
    for &action in row.actions {
        for b in ui.bindings.get(device, action) {
            part.clear();
            write_entry(part, b, row.merge);
            if !out.split(" / ").any(|e| e == part.as_str()) {
                if !out.is_empty() {
                    out.push_str(" / ");
                }
                out.push_str(part);
            }
        }
    }
    if out.is_empty() {
        out.push('-');
    }
}

/// Cuts `text` to fit `w` at scale `S`, ending it with "..".
fn clip(text: &mut String, w: f32) {
    if draw::width(text, S) <= w {
        return;
    }
    while !text.is_empty() && draw::width(text, S) + draw::width("..", S) > w {
        text.pop();
    }
    text.push_str("..");
}

/// The inputs that toggle the card on a device, "H or F1" ("Start (hold)").
struct Toggle<'a>(&'a Ui, DeviceKind);

impl Display for Toggle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = self.0.bindings.get(self.1, Action::Controls);
        if list.is_empty() {
            return f.write_str("Pause menu");
        }
        for (i, b) in list.iter().enumerate() {
            if i > 0 {
                f.write_str(" or ")?;
            }
            names::write_binding(f, b)?;
        }
        Ok(())
    }
}

/// The card's own inputs on a device, each followed by " / " (none when it is unbound), for
/// the close hint.
struct CloseKeys<'a>(&'a Ui, DeviceKind);

impl Display for CloseKeys<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0.bindings.get(self.1, Action::Controls) {
            names::write_binding(f, b)?;
            f.write_str(" / ")?;
        }
        Ok(())
    }
}

impl ControlsCard {
    pub fn shown(&self) -> Option<Shown> {
        self.shown
    }

    /// The card scrolls: it has more lines than fit, as last drawn.
    pub fn scrolls(&self) -> bool {
        self.total > self.fit
    }

    fn show(&mut self, shown: Shown) {
        self.shown = Some(shown);
        self.scroll = 0;
    }
}

impl Ui {
    pub fn controls(&self) -> &ControlsCard {
        &self.controls
    }

    /// The card is on screen: asked for, or the start's while nothing else is up.
    pub fn controls_up(&self) -> bool {
        match self.controls.shown {
            Some(Shown::Asked) => true,
            Some(Shown::Start { .. }) => self.menu.is_none() && !self.map.full.open,
            None => false,
        }
    }

    /// The card asked for is up: it takes the keys and sticks (the car coasts under it).
    pub fn controls_take_input(&self) -> bool {
        self.controls.shown == Some(Shown::Asked)
    }

    /// Shows the card at the start of a game, unless the setting says not to.
    pub(super) fn show_controls_at_start(&mut self) {
        if self.settings.controls_at_start {
            self.controls.show(Shown::Start {
                remaining: START_SECONDS,
            });
        }
    }

    /// Takes down the start's card, if it is up (a new world in the same session).
    pub(crate) fn drop_start_controls(&mut self) {
        if matches!(self.controls.shown, Some(Shown::Start { .. })) {
            self.controls.shown = None;
        }
    }

    /// Shows the card until the player closes it.
    pub(super) fn open_controls(&mut self) {
        self.controls.show(Shown::Asked);
    }

    /// The player dismissed the start's card: after [`CONTROLS_DISMISSALS`] sessions of
    /// that, it stops showing at the start.
    fn controls_dismissed(&mut self) {
        let s = &mut self.settings;
        s.controls_dismissed = (s.controls_dismissed + 1).min(CONTROLS_DISMISSALS);
        if s.controls_dismissed >= CONTROLS_DISMISSALS {
            s.controls_at_start = false;
        }
        self.dirty = true;
        self.save_if_dirty();
    }

    /// One frame of input for the card. Returns true when the card took the frame's input
    /// (nothing else in the UI should act on it).
    pub(super) fn controls_input(&mut self, s: &InputState, dt: f32) -> bool {
        match self.controls.shown {
            None => {
                let free = self.menu.is_none() && !self.map.full.open;
                if free && s.just_pressed(Action::Controls) {
                    self.open_controls();
                    return true;
                }
                false
            }
            Some(Shown::Start { remaining }) => {
                if self.menu.is_some() || self.map.full.open {
                    // Something else came up at the start: it had its moment.
                    self.controls.shown = None;
                    return false;
                }
                if self.scroll_controls(s) {
                    // Reading on: the time starts again.
                    self.controls.shown = Some(Shown::Start {
                        remaining: START_SECONDS,
                    });
                    return true;
                }
                if any_input(s) {
                    self.controls.shown = None;
                    self.controls_dismissed();
                    // The toggle only closes it; any other input goes on to do its job.
                    return s.just_pressed(Action::Controls);
                }
                let tick = if self.focused {
                    dt.clamp(0.0, MAX_TICK)
                } else {
                    0.0
                };
                let remaining = remaining - tick;
                self.controls.shown = (remaining > 0.0).then_some(Shown::Start { remaining });
                false
            }
            Some(Shown::Asked) => {
                if self.scroll_controls(s) {
                    return true;
                }
                if [Action::Controls, Action::Pause, Action::MenuBack]
                    .iter()
                    .any(|&a| s.just_pressed(a))
                {
                    self.controls.shown = None;
                    return true;
                }
                if self.menu.is_none() && s.just_pressed(Action::MapToggle) {
                    // The map takes the screen.
                    self.controls.shown = None;
                    return false;
                }
                // The card has the input, over the menu and over the road.
                true
            }
        }
    }

    /// Up and down scroll the card while it has more lines than fit. Returns whether they
    /// did.
    fn scroll_controls(&mut self, s: &InputState) -> bool {
        let card = &mut self.controls;
        if !card.scrolls() {
            return false;
        }
        let last = card.total - card.fit;
        if s.just_pressed(Action::MenuDown) {
            card.scroll = (card.scroll + SCROLL_STEP).min(last);
        } else if s.just_pressed(Action::MenuUp) {
            card.scroll = card.scroll.saturating_sub(SCROLL_STEP);
        } else {
            return false;
        }
        true
    }

    /// Lays the card out at the middle of the safe area.
    pub(super) fn draw_controls(&mut self, hud: &mut HudBuilder) {
        let [sx, sy, sw, sh] = hud.safe_rect();
        if self.menu.is_some() {
            let [vw, vh] = hud.size();
            hud.rect(-vw, -vh, 3.0 * vw, 3.0 * vh, SHADE);
        }
        let cw = sw.min(MAX_W);
        let label_w = ROWS
            .iter()
            .map(|r| draw::width(r.label, S))
            .fold(0.0, f32::max);
        let group_w = ROWS
            .iter()
            .map(|r| draw::width(r.group, S))
            .fold(0.0, f32::max);
        let inner = cw - 2.0 * PAD;
        let wide_cell = (inner - group_w - label_w - 3.0 * GAP) * 0.5;
        let wide = wide_cell >= MIN_CELL_W;
        let (kb_x, cell_w) = if wide {
            (group_w + label_w + 2.0 * GAP, wide_cell)
        } else {
            (INDENT, (inner - INDENT - GAP) * 0.5)
        };
        let pad_x = kb_x + cell_w + GAP;
        let cap = font::GLYPH_H as f32 * S;
        let title_h = font::GLYPH_H as f32 * TITLE_S;
        // Title (with the column headings beside it when wide), rule; the body; rule, footer.
        let heads_h = if wide { 0.0 } else { LH };
        let header_h = PAD + title_h + 8.0 + heads_h + 10.0;
        let footer_h = 6.0 + 2.0 + 8.0 + cap + PAD;
        let total = lines(wide).count();
        let wanted = header_h + total as f32 * LH + footer_h;
        let ch = wanted.min(sh);
        let fit = (((ch - header_h - footer_h) / LH).floor().max(1.0) as usize).min(total);
        let card = &mut self.controls;
        card.fit = fit;
        card.total = total;
        card.scroll = card.scroll.min(total - fit);
        let scroll = card.scroll;

        let cx = sx + (sw - cw) * 0.5;
        let top = sy + (sh - ch) * 0.5;
        let x0 = cx + PAD;
        hud.rect(cx, top, cw, ch, PANEL);
        hud.rect(cx, top, cw, 4.0, ACCENT);
        hud.text(x0, top + PAD, TITLE_S, TEXT, "Controls");
        let heads_y = if wide {
            top + PAD + title_h - cap
        } else {
            top + PAD + title_h + 8.0
        };
        hud.text(x0 + kb_x, heads_y, S, DIM, "Keyboard");
        hud.text(x0 + pad_x, heads_y, S, DIM, "Controller");
        let rule_y = top + header_h - 6.0;
        hud.rect(x0, rule_y, inner, 2.0, RULE);

        let mut cell = std::mem::take(&mut self.controls.cell);
        let mut part = std::mem::take(&mut self.controls.part);
        let body = top + header_h;
        for (n, line) in lines(wide).skip(scroll).take(fit).enumerate() {
            let y = body + n as f32 * LH;
            let inputs = match line {
                Line::Group(g) => {
                    hud.text(x0, y, S, ACCENT, g);
                    None
                }
                Line::Row(i) => {
                    let r = &ROWS[i];
                    let label_x = if wide { group_w + GAP } else { 0.0 };
                    let first = i == 0 || ROWS[i - 1].group != r.group;
                    if wide && first {
                        hud.text(x0, y, S, ACCENT, r.group);
                    }
                    hud.text(x0 + label_x, y, S, TEXT, r.label);
                    wide.then_some(i)
                }
                Line::Inputs(i) => Some(i),
            };
            if let Some(i) = inputs {
                for (device, x) in [
                    (DeviceKind::KeyboardMouse, kb_x),
                    (DeviceKind::Gamepad, pad_x),
                ] {
                    write_cell(&mut cell, &mut part, self, device, &ROWS[i]);
                    clip(&mut cell, cell_w);
                    hud.text(x0 + x, y, S, if wide { TEXT } else { DIM }, &cell);
                }
            }
        }
        self.controls.cell = cell;
        self.controls.part = part;

        let bottom = top + ch;
        let foot_y = bottom - PAD - cap;
        hud.rect(x0, foot_y - 10.0, inner, 2.0, RULE);
        let d = self.last_device;
        let more = if self.controls.scrolls() {
            match d {
                DeviceKind::Gamepad => "   [D-pad] More",
                DeviceKind::KeyboardMouse => "   [Up/Down] More",
            }
        } else {
            ""
        };
        match self.controls.shown {
            Some(Shown::Start { .. }) => {
                hud.text_fmt(
                    x0,
                    foot_y,
                    S,
                    DIM,
                    format_args!("{}: controls{more}", Toggle(self, d)),
                );
            }
            _ => {
                let back = match d {
                    DeviceKind::Gamepad => "B",
                    DeviceKind::KeyboardMouse => "Esc",
                };
                hud.text_fmt(
                    x0,
                    foot_y,
                    S,
                    DIM,
                    format_args!("[{}{back}] Close{more}", CloseKeys(self, d)),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Context;

    #[test]
    fn every_action_but_free_fly_is_on_one_row() {
        for a in Action::ALL {
            let n = ROWS.iter().filter(|r| r.actions.contains(&a)).count();
            let want = usize::from(a.context() != Context::Fly);
            assert_eq!(n, want, "{a:?}");
        }
        // Groups are contiguous.
        for (i, r) in ROWS.iter().enumerate().skip(1) {
            if r.group != ROWS[i - 1].group {
                assert!(ROWS[..i].iter().all(|p| p.group != r.group), "{}", r.group);
            }
        }
    }

    #[test]
    fn cells_name_inputs_once_by_family_on_merged_rows() {
        let ui = Ui::new();
        let (mut out, mut part) = (String::new(), String::new());
        let cell = |out: &mut String, part: &mut String, d, label: &str| {
            let r = ROWS.iter().find(|r| r.label == label).unwrap();
            write_cell(out, part, &ui, d, r);
            out.clone()
        };
        let kb = DeviceKind::KeyboardMouse;
        let pad = DeviceKind::Gamepad;
        assert_eq!(cell(&mut out, &mut part, kb, "Steer"), "A / D / Arrows");
        assert_eq!(cell(&mut out, &mut part, pad, "Steer"), "LS");
        assert_eq!(cell(&mut out, &mut part, kb, "Look around"), "RMB + Mouse");
        assert_eq!(cell(&mut out, &mut part, pad, "Look around"), "RS");
        assert_eq!(cell(&mut out, &mut part, kb, "Move"), "Arrows");
        assert_eq!(cell(&mut out, &mut part, pad, "Move"), "D-pad / LS");
        assert_eq!(cell(&mut out, &mut part, kb, "Throttle"), "W / Up");
        assert_eq!(cell(&mut out, &mut part, pad, "Reset car"), "Select (hold)");
        assert_eq!(cell(&mut out, &mut part, kb, "This card"), "H / F1");
        assert_eq!(cell(&mut out, &mut part, pad, "This card"), "Start (hold)");
        assert_eq!(cell(&mut out, &mut part, pad, "Next car"), "-");
        let mut text = String::from("D-pad right / D-pad left");
        clip(&mut text, draw::width("D-pad right", S));
        assert_eq!(text, "D-pad righ..");
    }
}
