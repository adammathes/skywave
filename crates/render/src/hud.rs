//! The HUD: text and rectangles drawn over the finished frame.
//!
//! Items are laid out in a virtual 1280×720 space (at `ui_scale` 1) that is letterboxed
//! into the largest centered 16:9 box of the target: a 16:10 Steam Deck screen or a 4:3
//! window gets bars above and below, an ultra-wide one at the sides, so layouts never
//! stretch. `ui_scale` (1–2) enlarges everything for TV distance by shrinking the virtual
//! space to `1280/ui_scale × 720/ui_scale` over the same box. [`HudLayout::safe_rect`]
//! insets that space by [`HUD_SAFE_MARGIN`] on every side; put anything that must be
//! readable on a TV inside it. Items may extend past the virtual space (a full-screen dim
//! behind a menu) and are clipped only by the target.
//!
//! Text is drawn with the [`crate::font`] face at a whole number of target pixels per font
//! pixel (the nearest to `scale` × pixels per virtual unit, at least 1) from a
//! pixel-snapped origin, so it is always crisp; widths therefore differ from
//! `font::text_width(text) * scale` by under half a font pixel per glyph.
//!
//! Image items ([`HudImage`]) draw the map texture ([`crate::Renderer::upload_hud_image`]), the
//! stills texture ([`crate::Renderer::upload_hud_image`]) or a flat shape (a disc, the player
//! arrow) with an arbitrary rotation of their content and an optional circular or
//! rounded-rectangle mask, anti-aliased in the shader. They are not pixel-snapped.
//! Everything, images included, goes out in the same single instanced draw: the HUD bind
//! group holds the font atlas, the map and the stills, and each quad's flags pick one.
//!
//! The frame's items are plain data ([`Hud`]): a slice of [`HudItem`]s whose text lives in
//! one shared string. [`HudBuilder`] fills both and reuses its buffers frame to frame.

use std::fmt::{self, Write as _};

use bytemuck::{Pod, Zeroable};

use crate::atlas;
use crate::font::{self, CELL_H, CELL_W, GLYPH_H, LINE_HEIGHT, SOLID_CELL};

/// Virtual HUD width at `ui_scale` 1.
pub const HUD_WIDTH: f32 = 1280.0;
/// Virtual HUD height at `ui_scale` 1.
pub const HUD_HEIGHT: f32 = 720.0;
/// Safe-area inset on each side, as a fraction of the HUD's width and height.
pub const HUD_SAFE_MARGIN: f32 = 0.03;
/// Range of the UI scale setting.
pub const UI_SCALE_MIN: f32 = 1.0;
pub const UI_SCALE_MAX: f32 = 2.0;
/// Quads (glyphs and rectangles) the HUD pass draws per frame; the rest are dropped and
/// counted in [`crate::RenderStats::hud_quads_dropped`].
pub const MAX_HUD_QUADS: usize = 4096;

/// A byte range of [`Hud::text`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextRange {
    pub start: u32,
    pub len: u32,
}

/// One thing to draw. Coordinates are virtual units from the top-left of the HUD space.
/// Colors are sRGB-encoded RGBA with straight alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HudItem {
    /// A run of text whose top-left (top of the cell, not the baseline) is at `(x, y)`.
    /// `scale` is virtual units per font pixel (2 makes 14×18-unit capitals' cells).
    /// `'\n'` starts a new line [`LINE_HEIGHT`] font pixels further down.
    Text {
        x: f32,
        y: f32,
        scale: f32,
        color: [u8; 4],
        text: TextRange,
    },
    /// A filled rectangle (menu panels, focus bars, graph bars).
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [u8; 4],
    },
    /// A map, shape or dot; see [`HudImage`].
    Image(HudImage),
}

/// What an image quad shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HudTexture {
    /// The map texture from [`crate::Renderer::upload_hud_image`], tinted by the item's color.
    /// Transparent outside uv 0..1, and everywhere before a map is uploaded.
    Map,
    /// A flat fill in the item's color (uv unused).
    #[default]
    Solid,
    /// The player arrow: a dart pointing up (toward -y) in the quad, before rotation, in
    /// the item's color (uv unused).
    Arrow,
    /// The stills texture from [`crate::Renderer::upload_hud_image`] (pictures side by side:
    /// `uv` picks one), tinted by the item's color; content rotation is ignored.
    /// Transparent outside uv 0..1, and everywhere before stills are uploaded.
    Still,
}

/// Which HUD image [`crate::Renderer::upload_hud_image`] replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudImageTarget {
    /// The map [`HudTexture::Map`] quads show.
    Map,
    /// The stills [`HudTexture::Still`] quads show.
    Stills,
}

/// The outline an image quad is cut to. Masks are not rotated with the content.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum HudMask {
    #[default]
    None,
    /// The ellipse inscribed in the quad (a circle for a square quad).
    Circle,
    /// The quad with corners rounded to this radius, virtual units.
    Rounded(f32),
}

/// A textured or flat quad whose content may be rotated (minimap, map, arrow, dots).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudImage {
    /// Top-left corner and size in virtual units.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// For [`HudTexture::Map`] and [`HudTexture::Still`]: the texture rectangle (0..1) shown in the quad before
    /// rotation: left, top, right, bottom. It may extend past 0..1 (shown transparent).
    pub uv: [f32; 4],
    /// Radians: the content turns clockwise on screen about the quad's center by this
    /// much (the quad and its mask stay axis-aligned).
    pub rotation: f32,
    pub texture: HudTexture,
    pub mask: HudMask,
    /// sRGB RGBA with straight alpha: the fill, or the map's tint (white shows it as is).
    pub color: [u8; 4],
}

impl HudImage {
    /// A flat `color` disc of radius `r` centered on `(cx, cy)`.
    pub fn disc(cx: f32, cy: f32, r: f32, color: [u8; 4]) -> HudImage {
        HudImage {
            x: cx - r,
            y: cy - r,
            w: 2.0 * r,
            h: 2.0 * r,
            mask: HudMask::Circle,
            color,
            ..HudImage::default()
        }
    }

    /// The player arrow `size` units tall centered on `(cx, cy)`, pointing `rotation`
    /// radians clockwise from up.
    pub fn arrow(cx: f32, cy: f32, size: f32, rotation: f32, color: [u8; 4]) -> HudImage {
        HudImage {
            x: cx - size * 0.5,
            y: cy - size * 0.5,
            w: size,
            h: size,
            rotation,
            texture: HudTexture::Arrow,
            color,
            ..HudImage::default()
        }
    }
}

/// The HUD for one frame, drawn in order (later items on top).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hud<'a> {
    pub items: &'a [HudItem],
    /// Backing store of every [`HudItem::Text`]'s range.
    pub text: &'a str,
    /// 1–2; see the module docs.
    pub ui_scale: f32,
}

impl<'a> Hud<'a> {
    /// Nothing to draw.
    pub const EMPTY: Hud<'static> = Hud {
        items: &[],
        text: "",
        ui_scale: 1.0,
    };

    /// The text of an item's range, or "" if the range is not in [`Hud::text`].
    pub fn str(&self, range: TextRange) -> &'a str {
        range_str(self.text, range)
    }
}

impl Default for Hud<'_> {
    fn default() -> Self {
        Hud::EMPTY
    }
}

fn range_str(text: &str, range: TextRange) -> &str {
    let start = range.start as usize;
    text.get(start..start.saturating_add(range.len as usize))
        .unwrap_or("")
}

/// Clamps a UI scale into [`UI_SCALE_MIN`]..=[`UI_SCALE_MAX`]; non-finite values give 1.
pub fn clamp_ui_scale(ui_scale: f32) -> f32 {
    if ui_scale.is_finite() {
        ui_scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX)
    } else {
        UI_SCALE_MIN
    }
}

/// Fills a [`Hud`] frame by frame without allocating once its buffers have grown.
#[derive(Clone, Debug)]
pub struct HudBuilder {
    items: Vec<HudItem>,
    text: String,
    ui_scale: f32,
}

/// Empty and unallocated, so `std::mem::take` of a builder costs nothing; [`HudBuilder::new`]
/// is the pre-sized one.
impl Default for HudBuilder {
    fn default() -> Self {
        HudBuilder {
            items: Vec::new(),
            text: String::new(),
            ui_scale: 1.0,
        }
    }
}

impl HudBuilder {
    /// Pre-sized for a busy frame.
    pub fn new() -> HudBuilder {
        HudBuilder {
            items: Vec::with_capacity(1024),
            text: String::with_capacity(8192),
            ui_scale: 1.0,
        }
    }

    /// Clears everything for a new frame laid out at `ui_scale`.
    pub fn begin(&mut self, ui_scale: f32) {
        self.items.clear();
        self.text.clear();
        self.ui_scale = clamp_ui_scale(ui_scale);
    }

    pub fn ui_scale(&self) -> f32 {
        self.ui_scale
    }

    /// The virtual space's size at this frame's UI scale.
    pub fn size(&self) -> [f32; 2] {
        HudLayout::virtual_size(self.ui_scale)
    }

    /// The safe area `[x, y, w, h]` at this frame's UI scale.
    pub fn safe_rect(&self) -> [f32; 4] {
        HudLayout::safe_rect(self.ui_scale)
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        self.items.push(HudItem::Rect { x, y, w, h, color });
    }

    pub fn image(&mut self, image: HudImage) {
        self.items.push(HudItem::Image(image));
    }

    pub fn text(&mut self, x: f32, y: f32, scale: f32, color: [u8; 4], text: &str) {
        self.text_fmt(x, y, scale, color, format_args!("{text}"));
    }

    /// Text formatted straight into the shared buffer (no intermediate `String`).
    pub fn text_fmt(&mut self, x: f32, y: f32, scale: f32, color: [u8; 4], args: fmt::Arguments) {
        let start = self.text.len();
        // Writing to a String cannot fail.
        let _ = self.text.write_fmt(args);
        let text = TextRange {
            start: start as u32,
            len: (self.text.len() - start) as u32,
        };
        self.items.push(HudItem::Text {
            x,
            y,
            scale,
            color,
            text,
        });
    }

    /// Appends everything `hud` draws, in order (its text copied into this builder), so a
    /// frame can lay more over a HUD built elsewhere.
    pub fn extend(&mut self, hud: &Hud<'_>) {
        for &item in hud.items {
            match item {
                HudItem::Text {
                    x,
                    y,
                    scale,
                    color,
                    text,
                } => self.text(x, y, scale, color, hud.str(text)),
                other => self.items.push(other),
            }
        }
    }

    /// The text of the last item, if it is text (for measuring what was just written).
    pub fn last_text(&self) -> &str {
        match self.items.last() {
            Some(HudItem::Text { text, .. }) => range_str(&self.text, *text),
            _ => "",
        }
    }

    /// Moves the last item right by `dx` (e.g. to right-align text after measuring it).
    pub fn shift_last(&mut self, dx: f32) {
        match self.items.last_mut() {
            Some(HudItem::Text { x, .. } | HudItem::Rect { x, .. }) => *x += dx,
            Some(HudItem::Image(image)) => image.x += dx,
            None => {}
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn view(&self) -> Hud<'_> {
        Hud {
            items: &self.items,
            text: &self.text,
            ui_scale: self.ui_scale,
        }
    }
}

/// Where the virtual HUD space lands on a target of a given size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudLayout {
    /// The letterboxed 16:9 box in target pixels: `[x, y, w, h]`.
    pub viewport: [f32; 4],
    /// Target pixels per virtual unit.
    pub scale: f32,
    /// Size of the virtual space: `[HUD_WIDTH, HUD_HEIGHT] / ui_scale`.
    pub size: [f32; 2],
}

impl HudLayout {
    pub fn new(target: (u32, u32), ui_scale: f32) -> HudLayout {
        let (tw, th) = (target.0.max(1) as f32, target.1.max(1) as f32);
        let w = tw.min(th * HUD_WIDTH / HUD_HEIGHT);
        let h = w * HUD_HEIGHT / HUD_WIDTH;
        let size = HudLayout::virtual_size(ui_scale);
        HudLayout {
            viewport: [(tw - w) * 0.5, (th - h) * 0.5, w, h],
            scale: w / size[0],
            size,
        }
    }

    /// The virtual space's size at `ui_scale` (clamped to 1–2).
    pub fn virtual_size(ui_scale: f32) -> [f32; 2] {
        let s = clamp_ui_scale(ui_scale);
        [HUD_WIDTH / s, HUD_HEIGHT / s]
    }

    /// The safe area `[x, y, w, h]` in virtual units at `ui_scale`: the virtual space less
    /// [`HUD_SAFE_MARGIN`] on each side.
    pub fn safe_rect(ui_scale: f32) -> [f32; 4] {
        let [w, h] = HudLayout::virtual_size(ui_scale);
        let (mx, my) = (w * HUD_SAFE_MARGIN, h * HUD_SAFE_MARGIN);
        [mx, my, w - 2.0 * mx, h - 2.0 * my]
    }

    /// Target pixel position of a virtual point (unrounded).
    pub fn to_pixels(&self, x: f32, y: f32) -> [f32; 2] {
        [
            self.viewport[0] + x * self.scale,
            self.viewport[1] + y * self.scale,
        ]
    }

    /// Whole target pixels per font pixel for text at `scale`.
    pub fn font_pixels(&self, scale: f32) -> u32 {
        let p = (scale * self.scale).round();
        if p.is_finite() && p >= 1.0 {
            p as u32
        } else {
            1
        }
    }
}

/// GPU layout of one HUD quad (vertex buffer, step mode instance); mirrors `HudIn` in
/// shaders/hud.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct HudQuad {
    /// Target pixels: left, top, right, bottom.
    pub rect: [f32; 4],
    /// Atlas uv (0..1): left, top, right, bottom.
    pub uv: [f32; 4],
    /// sRGB RGBA, straight alpha.
    pub color: [u8; 4],
    /// x: content rotation (rad, clockwise), y: map mip level, z: mask corner radius (px),
    /// w: flags, `kind + 8 * mask` (see [`QUAD_FONT`] and [`MASK_CIRCLE`]).
    pub params: [f32; 4],
}

/// Quad kinds (low three bits of the flags): sample the font atlas (glyphs and
/// rectangles), the map, nothing (flat), draw the arrow shape, or sample the stills.
/// Mirrored in shaders/hud.wgsl.
pub(crate) const QUAD_FONT: u32 = 0;
pub(crate) const QUAD_MAP: u32 = 1;
pub(crate) const QUAD_SOLID: u32 = 2;
pub(crate) const QUAD_ARROW: u32 = 3;
pub(crate) const QUAD_STILL: u32 = 4;
/// Masks (flags / 8).
pub(crate) const MASK_CIRCLE: u32 = 1;
pub(crate) const MASK_ROUNDED: u32 = 2;

fn flags(kind: u32, mask: u32) -> f32 {
    (kind + 8 * mask) as f32
}

/// Turns `hud` into quads for a target of size `target`, appending to `out` until it holds
/// `capacity` quads. `map_size` and `stills_size` are the map's and the stills' textures'
/// sizes in texels (for their mip levels). Returns the number of quads that did not fit.
pub(crate) fn build_quads(
    hud: &Hud,
    target: (u32, u32),
    map_size: [u32; 2],
    stills_size: [u32; 2],
    out: &mut Vec<HudQuad>,
    capacity: usize,
) -> u32 {
    let layout = HudLayout::new(target, hud.ui_scale);
    let (aw, ah) = (font::ATLAS_WIDTH as f32, font::ATLAS_HEIGHT as f32);
    let uv_rect = |x: u32, y: u32, w: u32, h: u32| {
        [
            x as f32 / aw,
            y as f32 / ah,
            (x + w) as f32 / aw,
            (y + h) as f32 / ah,
        ]
    };
    // The middle texel of the solid cell: every sample of it is fully covered.
    let [sx, sy] = font::cell_origin(SOLID_CELL);
    let solid = uv_rect(sx + CELL_W / 2 - 1, sy + CELL_H / 2 - 1, 1, 1);
    let mut dropped = 0;
    let mut push = |q: HudQuad| {
        if out.len() < capacity {
            out.push(q);
        } else {
            dropped += 1;
        }
    };
    for item in hud.items {
        match *item {
            HudItem::Rect { x, y, w, h, color } => {
                let [x0, y0] = layout.to_pixels(x, y);
                let [x1, y1] = layout.to_pixels(x + w, y + h);
                let (x0, y0, x1, y1) = (x0.round(), y0.round(), x1.round(), y1.round());
                if x1 > x0 && y1 > y0 && color[3] > 0 {
                    push(HudQuad {
                        rect: [x0, y0, x1, y1],
                        uv: solid,
                        color,
                        params: [0.0; 4],
                    });
                }
            }
            HudItem::Image(image) => {
                let [x0, y0] = layout.to_pixels(image.x, image.y);
                let [x1, y1] = layout.to_pixels(image.x + image.w, image.y + image.h);
                if !(x1 > x0 && y1 > y0 && image.color[3] > 0 && image.rotation.is_finite()) {
                    continue;
                }
                let (kind, size) = match image.texture {
                    HudTexture::Map => (QUAD_MAP, map_size),
                    HudTexture::Solid => (QUAD_SOLID, map_size),
                    HudTexture::Arrow => (QUAD_ARROW, map_size),
                    HudTexture::Still => (QUAD_STILL, stills_size),
                };
                let (mask, radius) = match image.mask {
                    HudMask::None => (0, 0.0),
                    HudMask::Circle => (MASK_CIRCLE, 0.0),
                    HudMask::Rounded(r) => (MASK_ROUNDED, (r * layout.scale).max(0.0)),
                };
                // Texels per target pixel picks the mip level (the full map minifies it).
                let uv = image.uv;
                let tx = (uv[2] - uv[0]).abs() * size[0] as f32 / (x1 - x0);
                let ty = (uv[3] - uv[1]).abs() * size[1] as f32 / (y1 - y0);
                let lod = tx.max(ty).max(1.0).log2();
                push(HudQuad {
                    rect: [x0, y0, x1, y1],
                    uv,
                    color: image.color,
                    params: [image.rotation, lod, radius, flags(kind, mask)],
                });
            }
            HudItem::Text {
                x,
                y,
                scale,
                color,
                text,
            } => {
                if color[3] == 0 {
                    continue;
                }
                let p = layout.font_pixels(scale) as f32;
                let [ox, oy] = layout.to_pixels(x, y);
                let (ox, mut py) = (ox.round(), oy.round());
                let mut px = ox;
                for c in hud.str(text).chars() {
                    if c == '\n' {
                        px = ox;
                        py += LINE_HEIGHT as f32 * p;
                        continue;
                    }
                    let index = font::glyph_index(c);
                    let w = u32::from(font::GLYPHS[index].width);
                    if c != ' ' {
                        let [u, v] = font::cell_origin(index);
                        push(HudQuad {
                            rect: [px, py, px + w as f32 * p, py + GLYPH_H as f32 * p],
                            uv: uv_rect(u, v, w, GLYPH_H),
                            color,
                            params: [0.0, 0.0, 0.0, flags(QUAD_FONT, 0)],
                        });
                    }
                    px += (w + 1) as f32 * p;
                }
            }
        }
    }
    dropped
}

/// The mip levels below a `width × height` RGBA8 image (sRGB-encoded, straight alpha), each
/// half the size of the one above (rounded down, at least 1), down to 1×1. Filtered like the
/// texture atlas ([`atlas::halve`]): in linear light with premultiplied alpha, so a minified
/// image keeps its brightness and transparent texels do not darken its edges; each level is
/// stored encoded with straight alpha again. Load-time only: it allocates every level.
pub(crate) fn mip_chain(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let (mut w, mut h) = (width.max(1), height.max(1));
    let mut lin: Vec<[f32; 4]> = rgba
        .chunks_exact(4)
        .take((w * h) as usize)
        .map(|p| {
            let a = f32::from(p[3]) / 255.0;
            [
                atlas::linear(p[0]) * a,
                atlas::linear(p[1]) * a,
                atlas::linear(p[2]) * a,
                a,
            ]
        })
        .collect();
    let mut levels = Vec::new();
    while w > 1 || h > 1 {
        lin = atlas::halve(&lin, w, h);
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
        let mut out = Vec::with_capacity(lin.len() * 4);
        for p in &lin {
            let inv = if p[3] > 0.0 { 1.0 / p[3] } else { 0.0 };
            out.extend_from_slice(&[
                atlas::srgb8((p[0] * inv).min(1.0)),
                atlas::srgb8((p[1] * inv).min(1.0)),
                atlas::srgb8((p[2] * inv).min(1.0)),
                (p[3] * 255.0).round() as u8,
            ]);
        }
        levels.push((w, h, out));
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_halves_down_to_one_texel() {
        // 4×2: left half black, right half white.
        let mut rgba = Vec::new();
        for _ in 0..2 {
            for x in 0..4 {
                let v = if x < 2 { 0 } else { 255 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let levels = mip_chain(4, 2, &rgba);
        let sizes: Vec<_> = levels.iter().map(|l| (l.0, l.1)).collect();
        assert_eq!(sizes, [(2, 1), (1, 1)]);
        assert_eq!(levels[0].2, [0, 0, 0, 255, 255, 255, 255, 255]);
        // Half black, half white is half the light: 188 encoded, not 128.
        assert_eq!(levels[1].2, [188, 188, 188, 255]);
        assert!(mip_chain(1, 1, &[1, 2, 3, 4]).is_empty());
        assert_eq!(mip_chain(1024, 1024, &vec![0; 1024 * 1024 * 4]).len(), 10);
    }

    #[test]
    fn mip_chain_filters_in_linear_light_with_premultiplied_alpha() {
        // A one-texel black and white checker minifies to 50% linear gray.
        let mut checker = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                let v = if (x + y) % 2 == 0 { 0 } else { 255 };
                checker.extend_from_slice(&[v, v, v, 255]);
            }
        }
        for (w, h, px) in mip_chain(8, 8, &checker) {
            for p in px.chunks_exact(4) {
                assert_eq!(p, [188, 188, 188, 255], "{w}×{h}");
            }
        }
        // An opaque orange texel beside a transparent black one: the edge mip keeps the
        // orange (transparent texels carry no colour) at half coverage.
        let edge = [255, 128, 0, 255, 0, 0, 0, 0];
        assert_eq!(mip_chain(2, 1, &edge), [(1, 1, vec![255, 128, 0, 128])]);
        // Half-covered gray over a transparent texel stays that gray.
        let soft = [100, 100, 100, 128, 0, 0, 0, 0];
        assert_eq!(mip_chain(2, 1, &soft)[0].2, [100, 100, 100, 64]);
    }

    #[test]
    fn extend_copies_another_hud_in_order() {
        let mut a = HudBuilder::new();
        a.begin(1.0);
        a.rect(1.0, 2.0, 3.0, 4.0, [1, 2, 3, 4]);
        a.text(5.0, 6.0, 2.0, [9; 4], "hello");
        let mut b = HudBuilder::new();
        b.begin(1.0);
        b.text(0.0, 0.0, 1.0, [7; 4], "first");
        b.extend(&a.view());
        let v = b.view();
        assert_eq!(v.items.len(), 3);
        assert_eq!(v.items[1], a.view().items[0]);
        match v.items[2] {
            HudItem::Text { x, text, .. } => {
                assert_eq!(x, 5.0);
                assert_eq!(v.str(text), "hello");
            }
            _ => panic!("not text"),
        }
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn letterbox_keeps_16_9_on_every_aspect() {
        // (target, viewport [x, y, w, h], pixels per unit at ui scale 1)
        let cases: [((u32, u32), [f32; 4], f32); 6] = [
            ((1280, 720), [0.0, 0.0, 1280.0, 720.0], 1.0),
            ((1920, 1080), [0.0, 0.0, 1920.0, 1080.0], 1.5),
            ((3840, 2160), [0.0, 0.0, 3840.0, 2160.0], 3.0),
            // 16:10 Steam Deck: bars of 40 px above and below.
            ((1280, 800), [0.0, 40.0, 1280.0, 720.0], 1.0),
            // 4:3: 1024×576 box, bars of 96 px.
            ((1024, 768), [0.0, 96.0, 1024.0, 576.0], 0.8),
            // Ultra-wide 21:9: bars at the sides.
            ((2560, 1080), [320.0, 0.0, 1920.0, 1080.0], 1.5),
        ];
        for (target, viewport, scale) in cases {
            let l = HudLayout::new(target, 1.0);
            assert_eq!(l.viewport, viewport, "{target:?}");
            assert!((l.scale - scale).abs() < 1e-6, "{target:?}");
            assert_eq!(l.size, [HUD_WIDTH, HUD_HEIGHT]);
            // The virtual corners land on the box corners.
            assert!(close(l.to_pixels(0.0, 0.0), [viewport[0], viewport[1]]));
            assert!(close(
                l.to_pixels(HUD_WIDTH, HUD_HEIGHT),
                [viewport[0] + viewport[2], viewport[1] + viewport[3]]
            ));
        }
        // Degenerate targets do not divide by zero.
        assert!(HudLayout::new((0, 0), 1.0).scale.is_finite());
    }

    #[test]
    fn safe_area_is_three_percent_of_the_box() {
        for target in [(1280, 720), (1280, 800), (1024, 768), (3840, 2160)] {
            let l = HudLayout::new(target, 1.0);
            let [x, y, w, h] = HudLayout::safe_rect(1.0);
            let [bx, by, bw, bh] = l.viewport;
            assert!(close(l.to_pixels(x, y), [bx + 0.03 * bw, by + 0.03 * bh]));
            assert!(close(
                l.to_pixels(x + w, y + h),
                [bx + 0.97 * bw, by + 0.97 * bh]
            ));
        }
        let s1 = HudLayout::safe_rect(1.0);
        assert!(close([s1[0], s1[1]], [38.4, 21.6]));
        // At 2× the safe area is the same part of the screen in half the units.
        let s2 = HudLayout::safe_rect(2.0);
        assert!(close([s2[0], s2[1]], [19.2, 10.8]));
        assert!(close([s2[2], s2[3]], [640.0 * 0.94, 360.0 * 0.94]));
    }

    #[test]
    fn ui_scale_enlarges_everything() {
        let one = HudLayout::new((1920, 1080), 1.0);
        let two = HudLayout::new((1920, 1080), 2.0);
        let big = HudLayout::new((1920, 1080), 1.5);
        assert_eq!(two.size, [640.0, 360.0]);
        assert_eq!(two.scale, 2.0 * one.scale);
        assert_eq!(big.scale, 1.5 * one.scale);
        // Text at scale 2: 3 px per font pixel at 1×, 6 px at 2×.
        assert_eq!(one.font_pixels(2.0), 3);
        assert_eq!(two.font_pixels(2.0), 6);
        // Out-of-range settings are clamped.
        assert_eq!(HudLayout::new((1920, 1080), 9.0), two);
        assert_eq!(HudLayout::new((1920, 1080), f32::NAN), one);
        assert_eq!(HudLayout::new((1920, 1080), 0.2), one);
        // Font pixels never drop below one target pixel.
        assert_eq!(HudLayout::new((320, 180), 1.0).font_pixels(1.0), 1);
    }

    #[test]
    fn items_become_snapped_quads_in_order() {
        let mut b = HudBuilder::new();
        b.begin(1.0);
        b.rect(10.25, 20.0, 100.0, 30.0, [0, 0, 0, 128]);
        b.text(40.0, 50.0, 2.0, [255; 4], "Hi you");
        b.rect(0.0, 0.0, 10.0, 10.0, [0, 0, 0, 0]);
        b.text_fmt(0.0, 0.0, 1.0, [255; 4], format_args!("{}\n{}", 1, 2));
        let hud = b.view();
        let mut quads = Vec::new();
        let dropped = build_quads(
            &hud,
            (1920, 1080),
            [1, 1],
            [1, 1],
            &mut quads,
            MAX_HUD_QUADS,
        );
        assert_eq!(dropped, 0);
        // Rect, 5 glyphs (the space draws nothing), invisible rect skipped, 2 glyphs.
        assert_eq!(quads.len(), 1 + 5 + 2);
        assert_eq!(quads[0].rect, [15.0, 30.0, 165.0, 75.0]);
        // "H" at (60, 75), 3 px per font pixel: 7 columns wide, 9 rows tall.
        assert_eq!(quads[1].rect, [60.0, 75.0, 81.0, 102.0]);
        // "i" follows after H's 7 columns and a gap.
        assert_eq!(quads[2].rect[0], 60.0 + 8.0 * 3.0);
        // "y" after "Hi" (8 + 3) and a space (4).
        assert_eq!(quads[3].rect[0], 60.0 + (8.0 + 3.0 + 4.0) * 3.0);
        // Rect uv is a single solid texel; glyph uv covers its columns exactly.
        let u = |t: f32| t * font::ATLAS_WIDTH as f32;
        assert_eq!(u(quads[0].uv[2]) - u(quads[0].uv[0]), 1.0);
        assert_eq!(u(quads[1].uv[2]) - u(quads[1].uv[0]), 7.0);
        // The second line of "1\n2" starts LINE_HEIGHT font pixels lower (2 px each).
        assert_eq!(quads[6].rect[1], 0.0);
        assert_eq!(quads[7].rect[1], 2.0 * LINE_HEIGHT as f32);
        assert_eq!(quads[7].rect[0], quads[6].rect[0]);
        assert!(quads.iter().all(|q| q.color[3] > 0));
    }

    #[test]
    fn images_carry_rotation_mask_and_mip_level() {
        let mut b = HudBuilder::new();
        b.begin(1.0);
        // 200 px showing a quarter of a 1024-texel map: 1.28 texels per pixel.
        b.image(HudImage {
            x: 10.0,
            y: 20.0,
            w: 200.0,
            h: 200.0,
            uv: [0.25, 0.25, 0.5, 0.5],
            rotation: 0.5,
            texture: HudTexture::Map,
            mask: HudMask::Circle,
            color: [255; 4],
        });
        b.image(HudImage::disc(100.0, 100.0, 4.0, [255, 0, 0, 255]));
        b.image(HudImage::arrow(100.0, 100.0, 20.0, -1.0, [255; 4]));
        b.image(HudImage {
            mask: HudMask::Rounded(10.0),
            ..HudImage::disc(0.0, 0.0, 50.0, [0, 0, 0, 100])
        });
        // The second of two 320×200 stills at half size: mip 1 of the stills.
        b.image(HudImage {
            x: 0.0,
            y: 0.0,
            w: 160.0,
            h: 100.0,
            uv: [0.5, 0.0, 1.0, 1.0],
            texture: HudTexture::Still,
            color: [255; 4],
            ..HudImage::default()
        });
        // Invisible or degenerate images are skipped.
        b.image(HudImage::disc(0.0, 0.0, 5.0, [0; 4]));
        b.image(HudImage::disc(0.0, 0.0, 0.0, [255; 4]));
        b.image(HudImage {
            rotation: f32::NAN,
            ..HudImage::disc(0.0, 0.0, 5.0, [255; 4])
        });
        let mut quads = Vec::new();
        let dropped = build_quads(
            &b.view(),
            (1280, 720),
            [1024, 1024],
            [640, 200],
            &mut quads,
            16,
        );
        assert_eq!((dropped, quads.len()), (0, 5));
        assert_eq!(quads[4].params[3], flags(QUAD_STILL, 0));
        assert_eq!(quads[4].uv, [0.5, 0.0, 1.0, 1.0]);
        assert!((quads[4].params[1] - 1.0).abs() < 1e-5);
        let m = quads[0];
        assert_eq!(m.rect, [10.0, 20.0, 210.0, 220.0]);
        assert_eq!(m.uv, [0.25, 0.25, 0.5, 0.5]);
        assert_eq!(m.params[0], 0.5);
        assert!((m.params[1] - 1.28f32.log2()).abs() < 1e-5);
        assert_eq!(m.params[3], flags(QUAD_MAP, MASK_CIRCLE));
        assert_eq!(quads[1].rect, [96.0, 96.0, 104.0, 104.0]);
        assert_eq!(quads[1].params[3], flags(QUAD_SOLID, MASK_CIRCLE));
        assert_eq!(quads[2].params[0], -1.0);
        assert_eq!(quads[2].params[3], flags(QUAD_ARROW, 0));
        // Rounded corners are in target pixels; a magnified map stays on mip 0.
        assert_eq!(quads[3].params[2], 10.0);
        assert_eq!(quads[3].params[3], flags(QUAD_SOLID, MASK_ROUNDED));
        assert_eq!(quads[3].params[1], 0.0);
        quads.clear();
        build_quads(
            &b.view(),
            (2560, 1440),
            [1024, 1024],
            [640, 200],
            &mut quads,
            16,
        );
        assert_eq!(quads[0].params[1], 0.0);
        assert_eq!(quads[3].params[2], 20.0);
        b.shift_last(3.0);
        assert!(matches!(b.view().items.last(), Some(HudItem::Image(i)) if i.x == -2.0));
    }

    #[test]
    fn quads_past_capacity_are_dropped_and_counted() {
        let mut b = HudBuilder::new();
        b.begin(1.0);
        b.text(0.0, 0.0, 1.0, [255; 4], "abcdefghij");
        let mut quads = Vec::with_capacity(4);
        let dropped = build_quads(&b.view(), (1280, 720), [1, 1], [1, 1], &mut quads, 4);
        assert_eq!((quads.len(), dropped), (4, 6));
        assert_eq!(quads.capacity(), 4);
    }

    #[test]
    fn builder_reuses_its_buffers() {
        let mut b = HudBuilder::new();
        let fill = |b: &mut HudBuilder| {
            b.begin(1.25);
            for i in 0..200 {
                b.text_fmt(0.0, i as f32, 2.0, [255; 4], format_args!("line {i:3}"));
                b.rect(0.0, 0.0, 1.0, 1.0, [1; 4]);
            }
        };
        fill(&mut b);
        let caps = (b.items.capacity(), b.text.capacity());
        for _ in 0..10 {
            fill(&mut b);
        }
        assert_eq!((b.items.capacity(), b.text.capacity()), caps);
        assert_eq!(b.len(), 400);
        assert_eq!(b.view().ui_scale, 1.25);
        b.text(0.0, 0.0, 1.0, [255; 4], "last");
        assert_eq!(b.last_text(), "last");
        b.shift_last(5.0);
        assert!(matches!(b.view().items.last(), Some(HudItem::Text { x, .. }) if *x == 5.0));
        assert_eq!(Hud::EMPTY.str(TextRange { start: 3, len: 9 }), "");
    }
}
