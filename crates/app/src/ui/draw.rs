//! Lays the UI out into the frame's HUD: the menu panel (or the capture prompt over it),
//! the driving HUD with the minimap, the full-screen map, and the performance overlay.
//! Everything sits inside the HUD's safe area at the user's UI scale; text is formatted
//! straight into the HUD's text buffer.

use core::fmt::{self, Display};

use render::{font, HudBuilder, HudImage};

use crate::daylight::{Clock, Daylight};
use crate::input::{Action, Binding, DeviceKind, Sign};
use crate::map;
use crate::map::view::{MapGeom, Minimap, MINIMAP_DIAMETER};
use crate::radio::ndc_to_hud;

use super::menu::{Unit, Widget};
use super::names::{self, action_label, device_label, device_noun};

use super::pages::{self, ItemId, Page, QUALITY_NAMES};
use super::roster;
use super::{covers, Toast, Ui, FRAME_HISTORY};
use crate::settings::{FrameRate, Overlay, Quality};

/// What the UI shows besides its own state.
pub struct DrawCtx<'a> {
    pub daylight: &'a Daylight,
    pub clock: &'a Clock,
    pub camera: [f32; 3],
    pub ticks: u64,
    /// The car (or free-fly camera) on the ground, `[x, z]`, and its heading (sim
    /// convention), for the maps.
    pub car: [f32; 2],
    pub heading: f32,
    /// The car radio's display on screen (cockpit view only).
    pub radio_display: Option<RadioDisplay>,
}

/// The radio display quad as the cockpit camera sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadioDisplay {
    /// Corners in normalized device coordinates (y up): bottom-left, bottom-right,
    /// top-right, top-left as the driver sees them.
    pub ndc: [[f32; 2]; 4],
    /// Width over height of the target they were projected for.
    pub aspect: f32,
    /// The target's size in pixels, when the platform said (text is drawn at whole target
    /// pixels per font pixel, so how much fits depends on it).
    pub target: Option<(u32, u32)>,
    /// The display's glow, sRGB.
    pub color: [u8; 4],
}

const TEXT: [u8; 4] = [238, 234, 222, 255];
const DIM: [u8; 4] = [150, 158, 176, 255];
const ACCENT: [u8; 4] = [255, 178, 62, 255];
const ON_ACCENT: [u8; 4] = [26, 18, 8, 255];
const GOOD: [u8; 4] = [128, 222, 150, 255];
const PANEL: [u8; 4] = [12, 16, 28, 246];
const DIALOG: [u8; 4] = [24, 30, 50, 250];
const SHADE: [u8; 4] = [0, 0, 0, 110];
const RULE: [u8; 4] = [60, 68, 92, 255];
const TRACK: [u8; 4] = [62, 70, 94, 255];
const SHADOW: [u8; 4] = [0, 0, 0, 170];
const GRAPH_OK: [u8; 4] = [110, 210, 130, 230];
const GRAPH_SLOW: [u8; 4] = [255, 178, 62, 230];
const GRAPH_BAD: [u8; 4] = [235, 80, 70, 230];
/// The minimap's rim and what shows past the place's edge; the full map's backdrop.
const MAP_RIM: [u8; 4] = [16, 20, 30, 235];
const MAP_SHADE: [u8; 4] = [8, 10, 18, 250];
const ARROW_EDGE: [u8; 4] = [24, 16, 8, 255];
/// Minimap rim width and arrow size, virtual units.
const RIM: f32 = 4.0;
const MINI_ARROW: f32 = 24.0;
const FULL_ARROW: f32 = 26.0;
/// Lines in the full overlay's stats block, under the graph.
const STAT_LINES: f32 = 13.0;
/// Room under the full map for its controls line, virtual units.
pub(super) const FULL_MAP_FOOTER: f32 = 32.0;

/// Menu text scale (virtual units per font pixel) and metrics in virtual units.
const S: f32 = 2.0;
const TITLE_S: f32 = 3.0;
const PAD: f32 = 36.0;
const ROW_H: f32 = 34.0;
const LINE_H: f32 = font::LINE_HEIGHT as f32 * S;
const MAX_PANEL_W: f32 = 880.0;
/// Panel space above the first row (crumbs, title, rule) and below the last (rule, hints).
const HEADER_H: f32 = 98.0;
const FOOTER_H: f32 = 62.0;
/// The Vehicle page: its panel's width (the rest of the safe area is the turntable's
/// stage, when it is at least `STAGE_MIN_W` wide), the gap between them, the room above
/// the stage for the car's name, and the spec lines under the rows.
const VEHICLE_PANEL_W: [f32; 2] = [470.0, 600.0];
const STAGE_GAP: f32 = 16.0;
const STAGE_MIN_W: f32 = 220.0;
const STAGE_TITLE_H: f32 = 56.0;
const SPEC_LINES: usize = 7;
/// Where a spec line's value starts, from the panel's text edge.
const SPEC_VALUE_X: f32 = 150.0;
/// Paint swatch size and spacing.
const SWATCH: f32 = 20.0;
const SWATCH_GAP: f32 = 8.0;
/// The World page: room for its cards, the gap between them, their padding, the lines of
/// text under the still, and a card's backdrop.
const CARDS_H: f32 = 360.0;
/// Cards on one page of the World page (more page with left and right).
pub(super) const CARDS_PER_PAGE: usize = 3;
const CARD_GAP: f32 = 20.0;
const CARD_PAD: f32 = 12.0;
const CARD_LINES: usize = 4;
const CARD: [u8; 4] = [30, 36, 56, 255];
/// A stub world's still is greyed.
const STUB_TINT: [u8; 4] = [120, 120, 128, 255];

fn width(text: &str, scale: f32) -> f32 {
    font::text_width(text) as f32 * scale
}

/// Text whose right edge is at `right`, optionally with a drop shadow one font pixel down
/// and right.
fn text_right(
    hud: &mut HudBuilder,
    right: f32,
    y: f32,
    scale: f32,
    color: [u8; 4],
    shadow: bool,
    args: fmt::Arguments,
) -> f32 {
    if shadow {
        hud.text_fmt(right + scale, y + scale, scale, SHADOW, args);
        let w = width(hud.last_text(), scale);
        hud.shift_last(-w);
    }
    hud.text_fmt(right, y, scale, color, args);
    let w = width(hud.last_text(), scale);
    hud.shift_last(-w);
    w
}

fn text_center(
    hud: &mut HudBuilder,
    center: f32,
    y: f32,
    scale: f32,
    color: [u8; 4],
    args: fmt::Arguments,
) {
    hud.text_fmt(center, y, scale, color, args);
    let w = width(hud.last_text(), scale);
    hud.shift_last(-w * 0.5);
}

struct BindingName<'a>(&'a Binding);

impl Display for BindingName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        names::write_binding(f, self.0)
    }
}

/// The inputs of one bindings row: those of the action on the device that feed `sign`.
struct RowBindings<'a>(&'a [Binding], Sign);

impl Display for RowBindings<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        names::write_bindings(f, self.0.iter().filter(|b| covers(b, self.1)))
    }
}

/// The first input of `action` on `device` (falling back to the keyboard), for button
/// hints like "[A] Select".
struct Glyph<'a>(&'a Ui, DeviceKind, Action);

impl Display for Glyph<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Glyph(ui, device, action) = *self;
        let first = ui
            .bindings
            .get(device, action)
            .first()
            .or_else(|| ui.bindings.get(DeviceKind::KeyboardMouse, action).first());
        match first {
            Some(b) => names::write_raw(f, b.raw, b.sign),
            None => f.write_str("?"),
        }
    }
}

/// The device's two MapZoom inputs, "out/in" ("-/=", "L1/R1").
struct ZoomGlyphs<'a>(&'a Ui, DeviceKind);

impl Display for ZoomGlyphs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ZoomGlyphs(ui, device) = *self;
        let list = ui.bindings.get(device, Action::MapZoom);
        if list.is_empty() {
            return f.write_str("?");
        }
        for (i, b) in list.iter().take(2).enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            names::write_raw(f, b.raw, b.sign)?;
        }
        Ok(())
    }
}

struct SliderValue(f32, Unit);

impl Display for SliderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let SliderValue(v, unit) = *self;
        match unit {
            Unit::Times => write!(f, "{v:.2}x"),
            Unit::Percent => write!(f, "{:.0}%", v * 100.0),
            Unit::Plain => write!(f, "{v:.2}"),
            Unit::Degrees => write!(f, "{v:.0} deg"),
            Unit::Centimeters => write!(f, "{v:+.0} cm"),
            Unit::Clock => {
                let minutes = (v * 60.0).round() as u32 % (24 * 60);
                write!(f, "{:02}:{:02}", minutes / 60, minutes % 60)
            }
        }
    }
}

struct Crumbs<'a>(&'a super::MenuNav);

impl Display for Crumbs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The pages above this one; the pause page only while it is the sole one.
        let depth = self.0.depth();
        let skip = usize::from(depth > 2);
        for (i, page) in self.0.path().take(depth - 1).skip(skip).enumerate() {
            if i > 0 {
                f.write_str(" / ")?;
            }
            f.write_str(pages::title(page))?;
        }
        Ok(())
    }
}

impl Ui {
    /// Lays out this frame's HUD.
    pub(super) fn draw(&mut self, ctx: &DrawCtx) {
        let mut hud = std::mem::take(&mut self.hud);
        hud.begin(self.settings.ui_scale);
        if self.menu.is_some() {
            self.draw_menu(&mut hud);
        } else if self.map.full.open {
            self.draw_full_map(&mut hud, ctx);
        } else {
            self.draw_driving(&mut hud, ctx);
        }
        // Menus and the map hide the overlay (it would sit over them) and keep its mode; so
        // does the background, where frames are slow on purpose and should stay Skip.
        if self.menu.is_none() && !self.map.full.open && self.focused {
            match self.settings.overlay {
                Overlay::Off => {}
                Overlay::Fps => self.draw_fps(&mut hud),
                Overlay::Full => self.draw_overlay(&mut hud, ctx),
            }
        }
        self.hud = hud;
    }

    /// The Vehicle page's paint row: a swatch per palette entry (upper over lower paint),
    /// right-aligned at `right`, the chosen one framed and the row's arrows round them.
    fn draw_swatches(&self, hud: &mut HudBuilder, right: f32, y: f32, index: usize, focused: bool) {
        let Some(car) = roster::roster().get(self.browse[0]) else {
            return;
        };
        let n = car.paints.len();
        let fg = if focused { ON_ACCENT } else { TEXT };
        let ty = y + (ROW_H - 4.0 - 18.0) * 0.5;
        let arrow = width(">", S);
        text_right(hud, right, ty, S, fg, false, format_args!(">"));
        let mut x = right - arrow - 12.0 - n as f32 * (SWATCH + SWATCH_GAP) + SWATCH_GAP;
        hud.text(x - 12.0 - arrow, ty, S, fg, "<");
        let sy = y + (ROW_H - 4.0 - SWATCH) * 0.5;
        for (k, [upper, lower]) in car.paints.iter().enumerate() {
            if k == index {
                hud.rect(x - 4.0, sy - 4.0, SWATCH + 8.0, SWATCH + 8.0, fg);
            }
            let rgb = |c: [u8; 3]| [c[0], c[1], c[2], 255];
            hud.rect(x, sy, SWATCH, SWATCH * 0.5, rgb(*upper));
            hud.rect(x, sy + SWATCH * 0.5, SWATCH, SWATCH * 0.5, rgb(*lower));
            x += SWATCH + SWATCH_GAP;
        }
    }

    /// The Vehicle page's spec lines for the car on the turntable, the first `lines` of:
    /// the stat strip (shortened where it does not fit `w`), class, era, engine, drive,
    /// 0-60 and top speed.
    fn draw_specs(&self, hud: &mut HudBuilder, x: f32, y: f32, w: f32, lines: usize) {
        let Some(car) = roster::roster().get(self.browse[0]) else {
            return;
        };
        let strip = [car.stats.as_str(), car.short.as_str()]
            .into_iter()
            .find(|t| width(t, S) <= w)
            .unwrap_or("");
        hud.text(x, y, S, ACCENT, strip);
        let rows: [(&str, fmt::Arguments); SPEC_LINES - 1] = [
            ("Class", format_args!("{}", car.class)),
            ("Era", format_args!("{}", car.era)),
            ("Engine", format_args!("{}", car.engine)),
            ("Drive", format_args!("{}", car.drive)),
            ("0-60 mph", format_args!("{:.1} s", car.zero_to_60_s)),
            ("Top speed", format_args!("{:.0} mph", car.top_mph)),
        ];
        for (k, (label, value)) in rows.into_iter().enumerate().take(lines.saturating_sub(1)) {
            let ly = y + (k + 1) as f32 * LINE_H;
            hud.text(x, ly, S, DIM, label);
            hud.text_fmt(x + SPEC_VALUE_X, ly, S, TEXT, value);
        }
    }

    /// Over the turntable's stage: the car's name, and where it is in the roster.
    fn draw_stage_title(&self, hud: &mut HudBuilder, stage: [f32; 4]) {
        let Some(car) = roster::roster().get(self.browse[0]) else {
            return;
        };
        let [x, y, w, _] = stage;
        let cx = x + 0.5 * w;
        let ty = y - STAGE_TITLE_H;
        // Too long for the stage at the title size: the menu size.
        let scale = if width(&car.name, TITLE_S) <= w {
            TITLE_S
        } else {
            S
        };
        let shadowed = |hud: &mut HudBuilder, y: f32, scale: f32, color, args: fmt::Arguments| {
            text_center(hud, cx + scale, y + scale, scale, SHADOW, args);
            text_center(hud, cx, y, scale, color, args);
        };
        shadowed(hud, ty, scale, TEXT, format_args!("{}", car.name));
        let n = roster::roster().len();
        let line = ty + scale * font::LINE_HEIGHT as f32 + 2.0;
        shadowed(
            hud,
            line,
            S,
            TEXT,
            format_args!("{} of {n}", self.browse[0] + 1),
        );
    }

    /// The World page: cards for the worlds side by side in `[x, y, w, h]` (each its still,
    /// title, kind and year, cars and stations), the focused one framed, the one the app is
    /// marked "here", a stub's still greyed and marked "coming". Up to [`CARDS_PER_PAGE`]
    /// a page, the page with the focus shown and "< 1 / 2 >" under them when there are
    /// more; past a UI scale of 1.5 one a page, its text beside the still.
    fn draw_cards(&self, hud: &mut HudBuilder, area: [f32; 4], focus: usize) {
        let [x, y, w, mut h] = area;
        let n = self.items.len().max(1);
        let side = self.settings.ui_scale > 1.5;
        let per = if side { 1 } else { n.min(CARDS_PER_PAGE) };
        let (page, pages) = (focus / per, n.div_ceil(per));
        if pages > 1 {
            h -= LINE_H + 6.0;
            let args = format_args!("< {} / {} >", page + 1, pages);
            text_center(hud, x + 0.5 * w, y + h + 6.0, S, DIM, args);
        }
        let cw = (w - CARD_GAP * (per - 1) as f32) / per as f32;
        let (still_w, still_h) = crate::scenario::STILL_SIZE;
        let aspect = still_w as f32 / still_h as f32;
        let text_h = CARD_LINES as f32 * LINE_H + 10.0;
        let (sw, sh) = if side {
            let sh = (h - 2.0 * CARD_PAD).min(0.5 * cw / aspect).max(0.0);
            (sh * aspect, sh)
        } else {
            let sh = ((cw - 2.0 * CARD_PAD) / aspect)
                .min(h - text_h - 2.0 * CARD_PAD)
                .max(0.0);
            (sh * aspect, sh)
        };
        let first = page * per;
        for (slot, item) in self.items.iter().enumerate().skip(first).take(per) {
            let ItemId::World(k) = item.id else { continue };
            let s = crate::scenario::get(k);
            let cx = x + (slot - first) as f32 * (cw + CARD_GAP);
            if slot == focus {
                hud.rect(cx - 4.0, y - 4.0, cw + 8.0, h + 8.0, ACCENT);
            }
            hud.rect(cx, y, cw, h, CARD);
            let (ix, iy) = if side {
                (cx + CARD_PAD, y + 0.5 * (h - sh))
            } else {
                (cx + 0.5 * (cw - sw), y + CARD_PAD)
            };
            if sh >= 24.0 {
                hud.image(HudImage {
                    x: ix,
                    y: iy,
                    w: sw,
                    h: sh,
                    uv: crate::scenario::still_uv(k),
                    texture: render::HudTexture::Still,
                    mask: render::HudMask::Rounded(4.0),
                    color: if s.stub { STUB_TINT } else { [255; 4] },
                    ..HudImage::default()
                });
            }
            let (tx, ty) = if side {
                (ix + sw + CARD_PAD, y + 0.5 * (h - text_h) + 5.0)
            } else {
                (cx + CARD_PAD, iy + sh + 10.0)
            };
            let right = cx + cw - CARD_PAD;
            hud.text(tx, ty, S, if s.stub { DIM } else { TEXT }, &s.title);
            let line = |k: usize| ty + k as f32 * LINE_H;
            hud.text_fmt(tx, line(1), S, DIM, format_args!("{}, {}", s.kind, s.year));
            let cars = crate::scenario::lineup(k).cars.len();
            let stations = crate::scenario::station_counts()[k];
            hud.text_fmt(tx, line(2), S, DIM, format_args!("{cars} cars"));
            hud.text_fmt(tx, line(3), S, DIM, format_args!("{stations} stations"));
            // Beside the short lines, clear of the longer ones at any UI scale.
            if k == self.scenario {
                text_right(hud, right, line(2), S, GOOD, false, format_args!("here"));
            }
            if s.stub {
                let at = line(3);
                text_right(hud, right, at, S, ACCENT, false, format_args!("coming"));
            }
        }
    }

    fn draw_menu(&mut self, hud: &mut HudBuilder) {
        let [sx, sy, sw, sh] = hud.safe_rect();
        let [vw, vh] = hud.size();
        self.stage = None;
        let Some(menu) = &self.menu else { return };
        let page = menu.page();
        let vehicle = page == Page::Vehicle && self.capture.is_none();
        // The Vehicle page's spec lines take the note's place, as many as leave room for
        // its rows.
        let spec_lines = if vehicle {
            let room = sh - HEADER_H - self.items.len() as f32 * ROW_H - FOOTER_H - 12.0;
            ((room / LINE_H).floor().max(0.0) as usize).min(SPEC_LINES)
        } else {
            0
        };
        // The panel fits its content, up to the safe area's height (then rows scroll).
        let note_lines = match (page, pages::note(page)) {
            (Page::Conflict, _) if self.conflict.is_some() => 2,
            (_, Some(note)) => note.split('\n').count(),
            _ => spec_lines,
        };
        let note_h = if note_lines > 0 {
            note_lines as f32 * LINE_H + 12.0
        } else {
            0.0
        };
        let content = if page == Page::Worlds {
            CARDS_H
        } else {
            self.items.len().max(1) as f32 * ROW_H
        };
        let wanted = HEADER_H + content + note_h + FOOTER_H;
        let ph = wanted.min(sh);
        let mut pw = sw.min(MAX_PANEL_W);
        let mut px = sx + (sw - pw) * 0.5;
        let top = sy + (sh - ph) * 0.5;
        // The Vehicle page sits at the left, the turntable's stage to its right.
        let panel_w = (0.5 * sw).clamp(VEHICLE_PANEL_W[0], VEHICLE_PANEL_W[1]);
        if vehicle && sw - panel_w - STAGE_GAP >= STAGE_MIN_W {
            (px, pw) = (sx, panel_w);
            let x = sx + pw + STAGE_GAP;
            // Level with the panel: the name at its top, the turntable below.
            let h = ph.max(0.6 * sh);
            let y = (top + 0.5 * ph - 0.5 * h).max(sy);
            self.stage = Some([x, y + STAGE_TITLE_H, sx + sw - x, h - STAGE_TITLE_H]);
        }
        // Dim the whole target, letterbox bars included, round the stage.
        match self.stage {
            Some([x, _, w, _]) => {
                hud.rect(-vw, -vh, vw + x, 3.0 * vh, SHADE);
                hud.rect(x + w, -vh, 2.0 * vw - x - w, 3.0 * vh, SHADE);
            }
            None => hud.rect(-vw, -vh, 3.0 * vw, 3.0 * vh, SHADE),
        }
        let bottom = top + ph;
        hud.rect(px, top, pw, ph, PANEL);
        hud.rect(px, top, pw, 4.0, ACCENT);
        hud.text_fmt(
            px + PAD,
            top + 22.0,
            S,
            DIM,
            format_args!("{}", Crumbs(menu)),
        );
        hud.text(px + PAD, top + 46.0, TITLE_S, TEXT, pages::title(page));
        hud.rect(px + PAD, top + 84.0, pw - 2.0 * PAD, 2.0, RULE);

        // Footer: button hints and the status message.
        let footer = bottom - 40.0;
        hud.rect(px + PAD, footer - 14.0, pw - 2.0 * PAD, 2.0, RULE);
        let d = self.last_device;
        let adjustable = self.items.get(menu.focus()).is_some_and(|i| {
            matches!(
                i.widget,
                Widget::Toggle(_) | Widget::Slider { .. } | Widget::Choice { .. }
            )
        });
        let adjust = if adjustable {
            "   [Left/Right] Adjust"
        } else {
            ""
        };
        if self.items.is_empty() {
            let back = Glyph(self, d, Action::MenuBack);
            hud.text_fmt(px + PAD, footer, S, DIM, format_args!("[{back}] Back"));
        } else if page == Page::Worlds {
            hud.text_fmt(
                px + PAD,
                footer,
                S,
                DIM,
                format_args!(
                    "[{}] Go there   [{}] Back",
                    Glyph(self, d, Action::MenuConfirm),
                    Glyph(self, d, Action::MenuBack),
                ),
            );
        } else if page == Page::Vehicle {
            hud.text_fmt(
                px + PAD,
                footer,
                S,
                DIM,
                format_args!(
                    "[{}] Drive   [{}] Back",
                    Glyph(self, d, Action::MenuConfirm),
                    Glyph(self, d, Action::MenuBack),
                ),
            );
        } else {
            hud.text_fmt(
                px + PAD,
                footer,
                S,
                DIM,
                format_args!(
                    "[{}] Select   [{}] Back{adjust}",
                    Glyph(self, d, Action::MenuConfirm),
                    Glyph(self, d, Action::MenuBack),
                ),
            );
        }
        if let Some(t) = self.toast() {
            let args = ToastText(t, &self.music_name);
            text_right(
                hud,
                px + pw - PAD,
                footer,
                S,
                ACCENT,
                false,
                format_args!("{args}"),
            );
        }

        // A note above the footer (the conflict question, license text, hints).
        let rows_bottom = bottom - FOOTER_H - note_h;
        let y = rows_bottom + 6.0;
        if page == Page::Conflict {
            if let Some(p) = &self.conflict {
                hud.text_fmt(
                    px + PAD,
                    y,
                    S,
                    TEXT,
                    format_args!(
                        "{} is already bound to {}.\nMove it to {}, or keep it where it is?",
                        BindingName(&p.conflict.binding),
                        action_label(p.conflict.existing, Sign::Full),
                        action_label(p.conflict.wanted, p.sign),
                    ),
                );
            }
        } else if let Some(note) = pages::note(page) {
            hud.text(px + PAD, y, S, DIM, note);
        } else if spec_lines > 0 {
            self.draw_specs(hud, px + PAD, y, pw - 2.0 * PAD, spec_lines);
        }
        if let Some(stage) = self.stage {
            self.draw_stage_title(hud, stage);
        }

        // Rows (or the World page's cards).
        let rows_top = top + HEADER_H;
        if page == Page::Worlds {
            let area = [
                px + PAD,
                rows_top,
                pw - 2.0 * PAD,
                rows_bottom - rows_top - 8.0,
            ];
            self.draw_cards(hud, area, menu.focus());
            return;
        }
        // (A hair over, so a panel sized for n rows shows n.)
        let rows = (((rows_bottom - rows_top) / ROW_H + 1e-3).floor() as usize).max(1);
        let len = self.items.len();
        if let Some(menu) = &mut self.menu {
            menu.fit(len, rows);
        }
        let Some(menu) = &self.menu else { return };
        let (focus, scroll) = (menu.focus(), menu.scroll());
        let right = px + pw - PAD;
        for (row, item) in self.items.iter().enumerate().skip(scroll).take(rows) {
            let y = rows_top + (row - scroll) as f32 * ROW_H;
            let ty = y + (ROW_H - 4.0 - 18.0) * 0.5;
            let focused = row == focus;
            if focused {
                hud.rect(px + 14.0, y, pw - 28.0, ROW_H - 4.0, ACCENT);
            }
            let (fg, dim) = if focused {
                (ON_ACCENT, ON_ACCENT)
            } else {
                (TEXT, DIM)
            };
            match (item.id, &self.conflict) {
                (ItemId::KeepExisting, Some(p)) => hud.text_fmt(
                    px + PAD,
                    ty,
                    S,
                    fg,
                    format_args!(
                        "{}{}",
                        item.label,
                        action_label(p.conflict.existing, Sign::Full)
                    ),
                ),
                (ItemId::UseNew, Some(p)) => hud.text_fmt(
                    px + PAD,
                    ty,
                    S,
                    fg,
                    format_args!("{}{}", item.label, action_label(p.conflict.wanted, p.sign)),
                ),
                (ItemId::Station(i), _) => {
                    let name = self.radio.rows().get(i).map_or("?", |r| r.name.as_str());
                    hud.text(px + PAD, ty, S, fg, name);
                }
                (ItemId::RemoveFolder(i), _) => {
                    let name = self.radio.rows().get(i).map_or("?", |r| r.name.as_str());
                    hud.text_fmt(px + PAD, ty, S, fg, format_args!("Remove {name}"));
                }
                _ => hud.text(px + PAD, ty, S, fg, item.label),
            }
            match item.widget {
                Widget::Toggle(on) => {
                    let (text, color) = match (on, focused) {
                        (true, false) => ("On", GOOD),
                        (true, true) => ("On", fg),
                        (false, _) => ("Off", dim),
                    };
                    text_right(hud, right, ty, S, color, false, format_args!("{text}"));
                }
                Widget::Slider {
                    value,
                    min,
                    max,
                    unit,
                    ..
                } => {
                    let shown = SliderValue(value, unit);
                    text_right(hud, right, ty, S, fg, false, format_args!("{shown}"));
                    let (tw, tx) = (150.0, right - 112.0 - 150.0);
                    let frac = if max > min {
                        ((value - min) / (max - min)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let ty = y + (ROW_H - 4.0) * 0.5 - 3.0;
                    let (track, fill) = if focused {
                        ([150, 100, 30, 255], ON_ACCENT)
                    } else {
                        (TRACK, ACCENT)
                    };
                    hud.rect(tx, ty, tw, 6.0, track);
                    hud.rect(tx, ty, tw * frac, 6.0, fill);
                }
                Widget::Choice { index, .. } if item.id == ItemId::Paint => {
                    self.draw_swatches(hud, right, y, index, focused);
                }
                Widget::Choice { index, .. } if item.id == ItemId::FrameRate => {
                    let hz = self.panel_hz();
                    match FrameRate::ALL.get(index) {
                        Some(&f) if index > 0 => {
                            let fps = f.fps(hz);
                            let args = format_args!("< {fps:.0} fps >");
                            text_right(hud, right, ty, S, fg, false, args);
                        }
                        _ => {
                            let args = format_args!("< Panel ({hz:.0}) >");
                            text_right(hud, right, ty, S, fg, false, args);
                        }
                    }
                }
                Widget::Choice { options, index } => {
                    let name = options.get(index).copied().unwrap_or("?");
                    text_right(hud, right, ty, S, fg, false, format_args!("< {name} >"));
                }
                Widget::Submenu(_) => {
                    text_right(hud, right, ty, S, dim, false, format_args!(">"));
                }
                Widget::Action => {}
                Widget::Capture {
                    device,
                    action,
                    sign,
                } => {
                    let list = RowBindings(self.bindings.get(device, action), sign);
                    text_right(
                        hud,
                        right,
                        ty,
                        S,
                        if focused { fg } else { TEXT },
                        false,
                        format_args!("{list}"),
                    );
                }
            }
            if item.id == ItemId::VehicleCar && self.browse[0] == self.vehicle {
                // A dot before the car being driven.
                let dot = if focused { ON_ACCENT } else { GOOD };
                let cy = y + (ROW_H - 4.0) * 0.5;
                hud.image(HudImage::disc(px + 24.0, cy, 5.0, dot));
            }
        }
        if scroll > 0 {
            hud.text(px + pw - 26.0, rows_top - 14.0, S, DIM, "^");
        }
        if scroll + rows < len {
            hud.text(
                px + pw - 26.0,
                rows_top + rows as f32 * ROW_H - 10.0,
                S,
                DIM,
                "v",
            );
        }

        if let Some(cap) = self.capture {
            hud.rect(px, top, pw, sh, SHADE);
            let (bw, bh) = (pw.min(640.0), 200.0);
            let (bx, by) = (px + (pw - bw) * 0.5, sy + (sh - bh) * 0.5);
            hud.rect(bx, by, bw, bh, DIALOG);
            hud.rect(bx, by, bw, 4.0, ACCENT);
            let cx = bx + bw * 0.5;
            text_center(
                hud,
                cx,
                by + 28.0,
                S,
                DIM,
                format_args!("Press the new input for"),
            );
            text_center(
                hud,
                cx,
                by + 62.0,
                TITLE_S,
                TEXT,
                format_args!("{}", action_label(cap.action, cap.sign)),
            );
            text_center(
                hud,
                cx,
                by + 108.0,
                S,
                DIM,
                format_args!("on the {}", device_noun(cap.device)),
            );
            text_center(
                hud,
                cx,
                by + 150.0,
                S,
                ACCENT,
                format_args!(
                    "[{}] Cancel   ({:.0} s)",
                    Glyph(self, cap.device, Action::MenuBack),
                    cap.remaining.ceil()
                ),
            );
        }
    }

    fn draw_driving(&self, hud: &mut HudBuilder, ctx: &DrawCtx) {
        let [sx, sy, sw, sh] = hud.safe_rect();
        // The toast sits above the minimap when there is one.
        let mut toast_y = sy + sh - font::CAP_HEIGHT as f32 * 2.0;
        if let (true, Some(geom)) = (self.settings.minimap, &self.map.geom) {
            self.draw_minimap(hud, ctx, geom);
            toast_y -= MINIMAP_DIAMETER + RIM + 16.0;
        }
        let right = sx + sw;
        let minutes = (ctx.clock.seconds_of_day / 60.0) as u32 % (24 * 60);
        text_right(
            hud,
            right,
            sy,
            3.0,
            TEXT,
            true,
            format_args!("{:02}:{:02}", minutes / 60, minutes % 60),
        );
        // Speed: big number and small unit sharing a baseline at the bottom right.
        let baseline = sy + sh;
        let cap = font::CAP_HEIGHT as f32;
        let unit_w = text_right(
            hud,
            right,
            baseline - cap * 2.0,
            2.0,
            TEXT,
            true,
            format_args!("mph"),
        );
        let x = right - unit_w - 12.0;
        let y = baseline - cap * 5.0;
        match self.speed_mph {
            Some(mph) => text_right(hud, x, y, 5.0, TEXT, true, format_args!("{:.0}", mph.abs())),
            None => text_right(hud, x, y, 5.0, TEXT, true, format_args!("--")),
        };
        if let Some(t) = self.toast() {
            let args = ToastText(t, &self.music_name);
            hud.text_fmt(sx, toast_y, 2.0, ACCENT, format_args!("{args}"));
        }
        if let Some(d) = &ctx.radio_display {
            self.draw_radio_display(hud, d);
        }
        // The station toast: two lines centered at the top.
        if let Some(t) = self.radio.toast() {
            let cx = sx + sw * 0.5;
            let title = t.title();
            text_center(
                hud,
                cx + 2.0,
                sy + 2.0,
                2.0,
                SHADOW,
                format_args!("{title}"),
            );
            text_center(hud, cx, sy, 2.0, ACCENT, format_args!("{title}"));
            let detail = t.detail();
            let y = sy + LINE_H;
            text_center(
                hud,
                cx + 2.0,
                y + 2.0,
                2.0,
                SHADOW,
                format_args!("{detail}"),
            );
            text_center(hud, cx, y, 2.0, TEXT, format_args!("{detail}"));
        }
    }

    /// The station on the radio's display: the quad's screen box darkened like an unlit
    /// segment display, the name (scrolling when long) in the display's glow color,
    /// centered vertically. Nothing while the radio is off.
    fn draw_radio_display(&self, hud: &mut HudBuilder, d: &RadioDisplay) {
        let size = hud.size();
        let pts = d.ndc.map(|p| ndc_to_hud(p, d.aspect, size));
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for [x, y] in pts {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let on_screen = x1 > 0.0 && y1 > 0.0 && x0 < size[0] && y0 < size[1];
        if !on_screen || h < 6.0 || w < 3.0 * h || w > size[0] {
            return;
        }
        // Capitals about 45% of the display's height. The HUD draws a font pixel as a whole
        // number of target pixels, so measure with what it will draw; without the target's
        // size, allow for rounding up by half.
        let scale = (h * 0.45 / font::CAP_HEIGHT as f32).max(0.5);
        let drawn = match d.target {
            Some(t) => {
                let ppv = render::HudLayout::new(t, hud.ui_scale()).scale;
                (scale * ppv).round().max(1.0) / ppv
            }
            None => scale * 1.5,
        };
        let pad = h * 0.15;
        let chars = ((w - 2.0 * pad) / (font::CELL_W as f32 * drawn)).floor() as usize;
        let Some(text) = self.radio.dash(chars.max(1)) else {
            return;
        };
        hud.rect(x0, y0, w, h, [4, 8, 6, 225]);
        let ty = y0 + (h - font::CAP_HEIGHT as f32 * drawn) * 0.5;
        hud.text_fmt(x0 + pad, ty, scale, d.color, format_args!("{text}"));
    }

    /// The minimap's circle at the bottom left of the safe area: the map around the car
    /// (turned with it, or north-up), a north badge on the rim, and the player arrow.
    fn draw_minimap(&self, hud: &mut HudBuilder, ctx: &DrawCtx, geom: &MapGeom) {
        let [sx, sy, _, sh] = hud.safe_rect();
        let r = MINIMAP_DIAMETER * 0.5;
        let center = [sx + r + RIM, sy + sh - r - RIM];
        let s = &self.settings;
        let m = Minimap::new(
            center,
            r,
            ctx.car,
            ctx.heading,
            s.minimap_rotate,
            s.minimap_range_m,
        );
        hud.image(HudImage::disc(center[0], center[1], r + RIM, MAP_RIM));
        hud.image(m.image(geom));
        // The player: always at the center.
        let rot = m.arrow_rotation();
        hud.image(HudImage::arrow(
            center[0],
            center[1],
            MINI_ARROW + 6.0,
            rot,
            ARROW_EDGE,
        ));
        hud.image(HudImage::arrow(
            center[0], center[1], MINI_ARROW, rot, ACCENT,
        ));
        // North: a badge with an N, straddling the rim where north is.
        let n = m.north();
        let [bx, by] = [center[0] + n[0] * r, center[1] + n[1] * r];
        hud.image(HudImage::disc(bx, by, 13.0, ACCENT));
        hud.image(HudImage::disc(bx, by, 11.0, MAP_RIM));
        let w = width("N", 2.0);
        hud.text(bx - w * 0.5, by - font::CAP_HEIGHT as f32, 2.0, ACCENT, "N");
    }

    /// The full-screen map: the whole place fit to the safe area (panned and zoomed),
    /// labelled points of interest, the player arrow, the time and the controls.
    fn draw_full_map(&self, hud: &mut HudBuilder, ctx: &DrawCtx) {
        let [vw, vh] = hud.size();
        hud.rect(-vw, -vh, 3.0 * vw, 3.0 * vh, MAP_SHADE);
        let Some(geom) = &self.map.geom else { return };
        let rect = self.full_map_rect();
        let [sx, sy, sw, sh] = rect;
        let view = self.map.full.view(geom, rect);
        if let Some(img) = view.image(geom, 10.0) {
            let pad = 4.0;
            hud.image(HudImage {
                x: img.x - pad,
                y: img.y - pad,
                w: img.w + 2.0 * pad,
                h: img.h + 2.0 * pad,
                mask: render::HudMask::Rounded(10.0 + pad),
                ..HudImage::disc(0.0, 0.0, 0.0, MAP_RIM)
            });
            hud.image(img);
        }
        for poi in &self.map.pois {
            let q = view.to_screen(poi.pos);
            if !view.contains(q) {
                continue;
            }
            let [r, g, b] = map::poi_color(poi.kind);
            hud.image(HudImage::disc(q[0], q[1], 7.0, ARROW_EDGE));
            hud.image(HudImage::disc(q[0], q[1], 5.0, [r, g, b, 255]));
            let (tx, ty) = (q[0] + 11.0, q[1] - font::CAP_HEIGHT as f32);
            hud.text(tx + 2.0, ty + 2.0, 2.0, SHADOW, &poi.name);
            hud.text(tx, ty, 2.0, TEXT, &poi.name);
        }
        let q = view.to_screen(ctx.car);
        if view.contains(q) {
            let rot = -ctx.heading;
            hud.image(HudImage::arrow(
                q[0],
                q[1],
                FULL_ARROW + 6.0,
                rot,
                ARROW_EDGE,
            ));
            hud.image(HudImage::arrow(q[0], q[1], FULL_ARROW, rot, ACCENT));
        }

        // Title and time at the top corners, the controls centered under the map.
        hud.text(sx + 2.0, sy + 2.0, TITLE_S, SHADOW, "Map");
        hud.text(sx, sy, TITLE_S, TEXT, "Map");
        let minutes = (ctx.clock.seconds_of_day / 60.0) as u32 % (24 * 60);
        text_right(
            hud,
            sx + sw,
            sy,
            3.0,
            TEXT,
            true,
            format_args!("{:02}:{:02}", minutes / 60, minutes % 60),
        );

        let d = self.last_device;
        let pan = if d == DeviceKind::Gamepad {
            "Stick"
        } else {
            "Arrows"
        };
        let wheel = if d == DeviceKind::Gamepad {
            ""
        } else {
            "/Wheel"
        };
        text_center(
            hud,
            sx + sw * 0.5,
            sy + sh + FULL_MAP_FOOTER - LINE_H,
            S,
            DIM,
            format_args!(
                "[{}] Close   [{pan}] Pan   [{}{wheel}] Zoom",
                Glyph(self, d, Action::MapToggle),
                ZoomGlyphs(self, d),
            ),
        );
    }

    /// The average frame time over [`FRAME_HISTORY`] frames (skipping empty slots), the
    /// frame rate it makes and the slowest frame, ms.
    fn frame_stats(&self) -> (f32, f32, f32) {
        let (mut sum, mut n, mut max) = (0.0f32, 0u32, 0.0f32);
        for &ms in self.frame_ms.iter().filter(|&&ms| ms > 0.0) {
            sum += ms;
            n += 1;
            max = max.max(ms);
        }
        let avg = if n > 0 { sum / n as f32 } else { 0.0 };
        let fps = if avg > 0.0 { 1000.0 / avg } else { 0.0 };
        (avg, fps, max)
    }

    /// The FPS overlay: the frame rate alone, in the top left corner of the safe area,
    /// which the driving HUD leaves empty (clock top right, speed bottom right, minimap
    /// and toasts bottom left). The station toast's two lines are centered at the top and
    /// a long name reaches the corner at large UI scales, so while it is up the number sits
    /// below them.
    fn draw_fps(&self, hud: &mut HudBuilder) {
        let [sx, sy, _, _] = hud.safe_rect();
        let sy = if self.radio.toast().is_some() {
            sy + 2.0 * LINE_H + 4.0
        } else {
            sy
        };
        let (_, fps, _) = self.frame_stats();
        hud.text_fmt(sx + 2.0, sy + 2.0, 2.0, SHADOW, format_args!("{fps:.1}"));
        hud.text_fmt(sx, sy, 2.0, TEXT, format_args!("{fps:.1}"));
    }

    fn draw_overlay(&self, hud: &mut HudBuilder, ctx: &DrawCtx) {
        let [sx, sy, _, _] = hud.safe_rect();
        let s = 2.0;
        let line = font::LINE_HEIGHT as f32 * s;
        let (graph_w, graph_h) = (FRAME_HISTORY as f32 * 2.0, 48.0);
        let r = &self.render_stats;
        let gpu_line = r.gpu_timing as u32 as f32;
        let label_line = !self.gpu_label.is_empty() as u32 as f32;
        let lines = STAT_LINES + gpu_line + label_line;
        hud.rect(
            sx - 10.0,
            sy - 10.0,
            760.0,
            20.0 + line + graph_h + 10.0 + lines * line,
            [0, 0, 0, 160],
        );

        let samples =
            || (0..FRAME_HISTORY).map(|i| self.frame_ms[(self.frame_head + i) % FRAME_HISTORY]);
        let (avg, fps, max) = self.frame_stats();
        hud.text_fmt(
            sx,
            sy,
            s,
            TEXT,
            format_args!("{avg:5.2} ms  {fps:4.0} fps  max {max:5.1}"),
        );

        // Frame-time graph, oldest on the left; full height is 33.3 ms (30 fps).
        let gy = sy + line + 4.0;
        hud.rect(sx, gy, graph_w, graph_h, [255, 255, 255, 24]);
        for (i, ms) in samples().enumerate() {
            let h = (ms / 33.3).clamp(0.0, 1.0) * graph_h;
            let color = match ms {
                m if m <= 17.5 => GRAPH_OK,
                m if m <= 34.0 => GRAPH_SLOW,
                _ => GRAPH_BAD,
            };
            hud.rect(sx + i as f32 * 2.0, gy + graph_h - h, 2.0, h, color);
        }
        hud.rect(
            sx,
            gy + graph_h * (1.0 - 16.7 / 33.3),
            graph_w,
            1.0,
            [255, 255, 255, 110],
        );

        let d = ctx.daylight;
        let [cx, cy, cz] = ctx.camera;
        let y = gy + graph_h + 10.0;
        hud.text_fmt(
            sx,
            y,
            s,
            TEXT,
            format_args!(
                "{}  draws {}  (shadow {}, bloom {})  cpu {:.2} ms\n\
                 instances {}  drawn {}  shadow {}/{}/{}\n\
                 tris {:.0}k  shadow {:.0}k\n\
                 shadow {} x{} ({} drawn)  {} {}\n\
                 view {}x{} of {}x{}  bloom {}\n\
                 lights {}/{}  coronas {}\n\
                 sim {} steps/frame  tick {}\n\
                 traffic {} cars  step {:.3} ms\n\
                 peds {} people  step {:.3} ms\n\
                 sun {:.1} deg  az {:.0}\n\
                 cam {:.1}, {:.1}, {:.1}\n\
                 hud {} quads  ({} dropped)\n\
                 frame {:?}  world {}  ui {}  skip {}",
                if r.backend.is_empty() { "?" } else { r.backend },
                r.draw_calls,
                r.shadow_draw_calls,
                r.bloom_draw_calls,
                self.cpu_submit_ms,
                r.instances,
                r.instances_drawn,
                r.shadow_instances[0],
                r.shadow_instances[1],
                r.shadow_instances[2],
                r.triangles as f64 / 1000.0,
                r.shadow_triangles as f64 / 1000.0,
                r.shadow_size,
                r.cascades,
                r.cascades_drawn,
                QUALITY_NAMES[Quality::ALL
                    .iter()
                    .position(|&q| q == self.quality())
                    .unwrap_or(2)],
                if self.settings.bloom { "" } else { "bloom off" },
                r.render_size.0,
                r.render_size.1,
                r.surface_size.0,
                r.surface_size.1,
                r.bloom_levels,
                r.lights_used,
                r.lights_submitted,
                r.coronas,
                self.last_steps,
                ctx.ticks,
                self.traffic.0,
                self.traffic.1,
                self.peds.0,
                self.peds.1,
                d.sun_elevation_deg,
                d.sun_azimuth_deg,
                cx,
                cy,
                cz,
                r.hud_quads,
                r.hud_quads_dropped,
                r.last_work,
                r.frames,
                r.ui_frames,
                r.skipped_frames,
            ),
        );
        let mut y = y + STAT_LINES * line;
        if r.gpu_timing {
            // Medians over the last 64 frames read back; the parts (each cascade; the
            // depth pre-pass, meshes and sky) where timestamps work inside passes.
            let ms = |p: usize| r.gpu_median_us[p] as f32 / 1000.0;
            let part = |k: usize| r.gpu_median_part_us[k] as f32 / 1000.0;
            let total: u32 = r.gpu_median_us.iter().sum();
            hud.text_fmt(
                sx,
                y,
                s,
                TEXT,
                format_args!(
                    "gpu med {:.2} ms: sh {:.2} fw {:.2} co {:.2} bl {:.2} tm {:.2} hud {:.2}{}",
                    total as f32 / 1000.0,
                    ms(0),
                    ms(1),
                    ms(2),
                    ms(3),
                    ms(4),
                    ms(5),
                    GpuParts(
                        r.gpu_parts
                            .then(|| { [part(0), part(1), part(2), part(3), part(4), part(6)] })
                    ),
                ),
            );
            y += line;
        }
        if !self.gpu_label.is_empty() {
            hud.text(sx, y, s, DIM, &self.gpu_label);
        }
    }
}

/// The F3 overlay's GPU parts: the cascades, then the forward pass's depth pre-pass,
/// meshes and sky, ms.
struct GpuParts(Option<[f32; 6]>);

impl Display for GpuParts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some([c0, c1, c2, z, m, sky]) => write!(
                f,
                " | c {c0:.2}/{c1:.2}/{c2:.2} z {z:.2} m {m:.2} sky {sky:.2}"
            ),
            None => Ok(()),
        }
    }
}

/// A toast and the music folder name it may name.
struct ToastText<'a>(Toast, &'a str);

impl Display for ToastText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Toast::Bound(b, action, sign) => {
                write!(f, "{} -> {}", BindingName(&b), action_label(action, sign))
            }
            Toast::Kept(b, action) => write!(
                f,
                "{} stays on {}",
                BindingName(&b),
                action_label(action, Sign::Full)
            ),
            Toast::Cancelled => f.write_str("Remap cancelled"),
            Toast::WrongDevice(d) => write!(f, "Use the {}", device_noun(d)),
            Toast::Southpaw => f.write_str("Sticks swapped"),
            Toast::SwapTriggers => f.write_str("Throttle and brake on the right stick"),
            Toast::Reset(d) => write!(f, "{} reset to defaults", device_label(d)),
            Toast::Vehicle(i) => match roster::roster().get(i) {
                Some(car) => f.write_str(&car.name),
                None => Ok(()),
            },
            Toast::CarReset => f.write_str("Car reset"),
            Toast::StreamAdded => f.write_str("Stream added to the stations"),
            Toast::MusicAdded { tracks, kept } => {
                let s = if tracks == 1 { "" } else { "s" };
                write!(f, "{}: {tracks} track{s} on the dial", self.1)?;
                if !kept {
                    f.write_str(" (pick it again next time)")?;
                }
                Ok(())
            }
            Toast::MusicEmpty => f.write_str("No music found in that folder"),
            Toast::MusicNeedsKeyboard => {
                f.write_str("Use the keyboard or mouse, or drop a folder on the page")
            }
            Toast::MusicRemoved => f.write_str("Music folder removed"),
            Toast::StreamRejected => f.write_str("Not added: use an https:// stream URL"),
            Toast::World(i) => {
                let s = crate::scenario::get(i);
                write!(f, "{}, {}", s.title, s.year)
            }
            Toast::UnknownWorld => write!(
                f,
                "No such world; starting in {}",
                crate::scenario::get(0).title
            ),
        }
    }
}
