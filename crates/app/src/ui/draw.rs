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

use super::menu::{Unit, Widget};
use super::names::{self, action_label, device_label, device_noun};
use super::pages::{self, ItemId, Page};
use super::{covers, Toast, Ui, FRAME_HISTORY};

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
        if self.settings.debug_overlay {
            self.draw_overlay(&mut hud, ctx);
        }
        self.hud = hud;
    }

    fn draw_menu(&mut self, hud: &mut HudBuilder) {
        let [sx, sy, sw, sh] = hud.safe_rect();
        let [vw, vh] = hud.size();
        // Dim the whole target, letterbox bars included.
        hud.rect(-vw, -vh, 3.0 * vw, 3.0 * vh, SHADE);
        let Some(menu) = &self.menu else { return };
        let page = menu.page();
        // The panel fits its content, up to the safe area's height (then rows scroll).
        let note_lines = match (page, pages::note(page)) {
            (Page::Conflict, _) if self.conflict.is_some() => 2,
            (_, Some(note)) => note.split('\n').count(),
            _ => 0,
        };
        let note_h = if note_lines > 0 {
            note_lines as f32 * LINE_H + 12.0
        } else {
            0.0
        };
        let wanted = HEADER_H + self.items.len().max(1) as f32 * ROW_H + note_h + FOOTER_H;
        let ph = wanted.min(sh);
        let pw = sw.min(MAX_PANEL_W);
        let px = sx + (sw - pw) * 0.5;
        let top = sy + (sh - ph) * 0.5;
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
            let args = ToastText(t);
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
        }

        // Rows.
        let rows_top = top + HEADER_H;
        let rows = (((rows_bottom - rows_top) / ROW_H).floor() as usize).max(1);
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
            let args = ToastText(t);
            hud.text_fmt(sx, toast_y, 2.0, ACCENT, format_args!("{args}"));
        }
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

    fn draw_overlay(&self, hud: &mut HudBuilder, ctx: &DrawCtx) {
        let [sx, sy, _, _] = hud.safe_rect();
        let s = 2.0;
        let line = font::LINE_HEIGHT as f32 * s;
        let (graph_w, graph_h) = (FRAME_HISTORY as f32 * 2.0, 48.0);
        let lines = 6.0;
        hud.rect(
            sx - 10.0,
            sy - 10.0,
            440.0,
            20.0 + line + graph_h + 10.0 + lines * line,
            [0, 0, 0, 160],
        );

        let samples =
            || (0..FRAME_HISTORY).map(|i| self.frame_ms[(self.frame_head + i) % FRAME_HISTORY]);
        let (mut sum, mut n, mut max) = (0.0f32, 0u32, 0.0f32);
        for ms in samples().filter(|&ms| ms > 0.0) {
            sum += ms;
            n += 1;
            max = max.max(ms);
        }
        let avg = if n > 0 { sum / n as f32 } else { 0.0 };
        let fps = if avg > 0.0 { 1000.0 / avg } else { 0.0 };
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

        let r = &self.render_stats;
        let d = ctx.daylight;
        let [cx, cy, cz] = ctx.camera;
        let y = gy + graph_h + 10.0;
        hud.text_fmt(
            sx,
            y,
            s,
            TEXT,
            format_args!(
                "draws {}  (shadow {}, bloom {})\n\
                 instances {}  lights {}\n\
                 sim {} steps/frame  tick {}\n\
                 sun {:.1} deg  az {:.0}\n\
                 cam {:.1}, {:.1}, {:.1}\n\
                 hud {} quads  ({} dropped)",
                r.draw_calls,
                r.shadow_draw_calls,
                r.bloom_draw_calls,
                r.instances,
                r.point_lights,
                self.last_steps,
                ctx.ticks,
                d.sun_elevation_deg,
                d.sun_azimuth_deg,
                cx,
                cy,
                cz,
                r.hud_quads,
                r.hud_quads_dropped,
            ),
        );
    }
}

struct ToastText(Toast);

impl Display for ToastText {
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
        }
    }
}
