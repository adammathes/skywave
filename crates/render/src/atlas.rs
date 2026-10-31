//! The pattern-texture atlas: sixteen 256² tiles generated procedurally at startup from a
//! few parameters each (no bitmaps are shipped), stored in one 1024² sRGB RGBA8 texture.
//!
//! Tiles are laid out row-major, four per row, in [`Tile`] order. Every pattern repeats
//! seamlessly across its tile, so a surface tiles by repeating quads ([`crate::Mesh`] has
//! builders for that). Mips are box-filtered per tile in linear light and stop at 32 px
//! tiles, so patterns stay crisp and no level mixes neighbouring tiles.
//!
//! Most tiles are *tintable*: near-neutral with a mean around 0.8, meant to be multiplied
//! by a palette vertex color. A few are *pre-colored* (both bricks, window glass) and are
//! meant for a white or lightly tinted vertex color.

/// Edge of the atlas texture in texels.
pub const ATLAS_SIZE: u32 = 1024;
/// Edge of one tile in texels.
pub const TILE_SIZE: u32 = 256;
/// Tiles per atlas row (and column).
pub const TILES_PER_ROW: u32 = ATLAS_SIZE / TILE_SIZE;
/// Mip levels: tiles of 256, 128, 64 and 32 px; nothing smaller, so patterns stay crisp.
pub const ATLAS_MIP_LEVELS: u32 = 4;
/// Texels (at level 0) kept clear of each tile edge by [`tile_uv`]: half a texel of the
/// smallest mip, so linear filtering never reads a neighbouring tile.
pub const TILE_INSET: u32 = 1 << (ATLAS_MIP_LEVELS - 2);

/// Atlas tiles, by index. Sizes are a suggestion of the real-world span of one tile.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tile {
    /// Plain white: untextured meshes sample this and multiply by 1. Default vertex uv.
    White = 0,
    /// Asphalt grain with light aggregate specks. Tintable; ~4 m.
    Asphalt = 1,
    /// Cast concrete, mottled with pores. Tintable; ~3 m.
    Concrete = 2,
    /// Red brick in running bond with light mortar, 8 courses × 4 bricks. Pre-colored; ~0.6 m.
    BrickRed = 3,
    /// Stucco: soft trowel blotches over sand grain. Tintable; ~2 m.
    Stucco = 4,
    /// One window: light frame, mullion cross, four panes of dark glass with a sky-reflection
    /// gradient. Pre-colored; one window per tile.
    WindowGlass = 5,
    /// Roof gravel at two pebble scales. Tintable; ~2 m.
    RoofGravel = 6,
    /// Sand with wind ripples. Tintable; ~2 m.
    Sand = 7,
    /// Lawn: vertical blade streaks and faint mowing bands. Tintable (use a green); ~2 m.
    Lawn = 8,
    /// Wood clapboard siding, 8 boards with shadow lips. Tintable; ~1.6 m.
    WoodSiding = 9,
    /// Awning fabric: 8 vertical stripes alternating full and 1/3 brightness. Tintable; ~2 m.
    AwningStripes = 10,
    /// Egg-crate grille: bright bars over a dark recess. Tintable (chrome or body color).
    GrilleMesh = 11,
    /// Sidewalk: 2 × 2 concrete slabs with seams. Tintable; ~3 m (1.5 m slabs).
    Sidewalk = 12,
    /// Tan brick with dark mortar (same generator as [`Tile::BrickRed`]). Pre-colored.
    BrickTan = 13,
    /// Asphalt roof shingles: staggered tabs, 8 rows. Tintable; ~2 m.
    RoofShingles = 14,
    /// Plowed soil: furrows across v with clods. Tintable (use a brown); ~4 m.
    Soil = 15,
}

impl Tile {
    /// Every tile in index order.
    pub const ALL: [Tile; 16] = [
        Tile::White,
        Tile::Asphalt,
        Tile::Concrete,
        Tile::BrickRed,
        Tile::Stucco,
        Tile::WindowGlass,
        Tile::RoofGravel,
        Tile::Sand,
        Tile::Lawn,
        Tile::WoodSiding,
        Tile::AwningStripes,
        Tile::GrilleMesh,
        Tile::Sidewalk,
        Tile::BrickTan,
        Tile::RoofShingles,
        Tile::Soil,
    ];

    /// The tile's index in the atlas.
    pub const fn index(self) -> u32 {
        self as u32
    }

    /// Texel origin of the tile at mip level 0.
    pub const fn origin(self) -> (u32, u32) {
        let i = self.index();
        (
            (i % TILES_PER_ROW) * TILE_SIZE,
            (i / TILES_PER_ROW) * TILE_SIZE,
        )
    }

    /// The tile's usable uv rectangle `[u0, v0, u1, v1]` (v down, as in the texture),
    /// inset by [`TILE_INSET`] texels.
    pub fn rect(self) -> [f32; 4] {
        let (x, y) = self.origin();
        let s = ATLAS_SIZE as f32;
        let inset = TILE_INSET as f32;
        [
            (x as f32 + inset) / s,
            (y as f32 + inset) / s,
            (x + TILE_SIZE) as f32 / s - inset / s,
            (y + TILE_SIZE) as f32 / s - inset / s,
        ]
    }
}

/// Atlas uv of the tile's corners in the order bottom-left, bottom-right, top-right,
/// top-left: the order [`crate::Mesh::push_quad`] takes a wall's corners in, so the pattern
/// stands upright.
pub fn tile_uv(tile: Tile) -> [[f32; 2]; 4] {
    let [u0, v0, u1, v1] = tile.rect();
    [[u0, v1], [u1, v1], [u1, v0], [u0, v0]]
}

/// Encodes an atlas uv in `[0, 1]` as the vertex's normalized `u16` pair.
pub fn uv_unorm(uv: [f32; 2]) -> [u16; 2] {
    uv.map(|c| (c.clamp(0.0, 1.0) * 65535.0).round() as u16)
}

/// The generated atlas: one sRGB-encoded RGBA8 image per mip level.
pub struct TextureAtlas {
    levels: Vec<Vec<u8>>,
}

impl TextureAtlas {
    /// Generates every tile and its mips. Deterministic; takes a few milliseconds.
    pub fn generate() -> TextureAtlas {
        let mut levels: Vec<Vec<u8>> = (0..ATLAS_MIP_LEVELS)
            .map(|l| vec![0u8; ((ATLAS_SIZE >> l) * (ATLAS_SIZE >> l) * 4) as usize])
            .collect();
        let n = TILE_SIZE as usize;
        for tile in Tile::ALL {
            let mut lin = vec![[0f32; 4]; n * n];
            for y in 0..n {
                for x in 0..n {
                    let [r, g, b] = texel(tile, x as u32, y as u32).map(|c| c.clamp(0.0, 1.0));
                    lin[y * n + x] = [r, g, b, 1.0];
                }
            }
            let (ox, oy) = tile.origin();
            let mut size = n;
            for (l, level) in levels.iter_mut().enumerate() {
                if l > 0 {
                    // Stays inside the tile: tiles are a power of two on a side.
                    lin = halve(&lin, size as u32, size as u32);
                    size /= 2;
                }
                let stride = (ATLAS_SIZE >> l) as usize;
                let (bx, by) = ((ox >> l) as usize, (oy >> l) as usize);
                for y in 0..size {
                    for x in 0..size {
                        let p = lin[y * size + x];
                        let o = ((by + y) * stride + bx + x) * 4;
                        level[o] = srgb8(p[0]);
                        level[o + 1] = srgb8(p[1]);
                        level[o + 2] = srgb8(p[2]);
                        level[o + 3] = 255;
                    }
                }
            }
        }
        TextureAtlas { levels }
    }

    /// RGBA8 (sRGB) texels of mip `level`, row-major, `ATLAS_SIZE >> level` wide.
    pub fn level(&self, level: u32) -> &[u8] {
        &self.levels[level as usize]
    }

    /// One texel of mip `level`.
    pub fn texel(&self, level: u32, x: u32, y: u32) -> [u8; 4] {
        let w = ATLAS_SIZE >> level;
        let o = ((y * w + x) * 4) as usize;
        let l = &self.levels[level as usize];
        [l[o], l[o + 1], l[o + 2], l[o + 3]]
    }
}

/// The mip level below a `w × h` image of linear (premultiplied, if it has alpha) texels:
/// half the size (rounded down, at least 1), each texel the mean of a 2×2 block (an odd
/// last row or column is dropped). The atlas's and the HUD images' mips share it, so every
/// mip is filtered in linear light.
pub(crate) fn halve(src: &[[f32; 4]], w: u32, h: u32) -> Vec<[f32; 4]> {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![[0f32; 4]; (nw * nh) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (2 * x + dx).min(w - 1);
                let sy = (2 * y + dy).min(h - 1);
                let p = src[(sy * w + sx) as usize];
                for k in 0..4 {
                    sum[k] += p[k] * 0.25;
                }
            }
            out[(y * nw + x) as usize] = sum;
        }
    }
    out
}

/// An sRGB-encoded 8-bit value decoded to linear light.
pub(crate) fn linear(e: u8) -> f32 {
    let s = f32::from(e) / 255.0;
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light in `[0, 1]` encoded as an 8-bit sRGB value.
pub(crate) fn srgb8(c: f32) -> u8 {
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// Integer hash of a lattice point and a seed (lowbias32-style mixing).
fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut h =
        x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// Uniform in `[0, 1)`.
fn rand(x: u32, y: u32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / 16_777_216.0
}

/// Smooth value noise in `[0, 1)` on a lattice of `cells_x × cells_y` cells across the tile,
/// wrapping at the tile edge so it repeats seamlessly.
fn noise(x: u32, y: u32, cells_x: u32, cells_y: u32, seed: u32) -> f32 {
    let fx = (x as f32 + 0.5) * cells_x as f32 / TILE_SIZE as f32;
    let fy = (y as f32 + 0.5) * cells_y as f32 / TILE_SIZE as f32;
    let (ix, iy) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - ix, fy - iy);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let (x0, y0) = (ix as u32 % cells_x, iy as u32 % cells_y);
    let (x1, y1) = ((x0 + 1) % cells_x, (y0 + 1) % cells_y);
    let a = rand(x0, y0, seed) + (rand(x1, y0, seed) - rand(x0, y0, seed)) * sx;
    let b = rand(x0, y1, seed) + (rand(x1, y1, seed) - rand(x0, y1, seed)) * sx;
    a + (b - a) * sy
}

/// Fractal value noise in `[0, 1)`: `octaves` layers starting at `cells` cells per tile.
fn fbm(x: u32, y: u32, cells: u32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for o in 0..octaves {
        let c = cells << o;
        sum += noise(x, y, c, c, seed.wrapping_add(o * 101)) * amp;
        norm += amp;
        amp *= 0.5;
    }
    sum / norm
}

fn gray(v: f32) -> [f32; 3] {
    [v, v, v]
}

fn scale(c: [f32; 3], s: f32) -> [f32; 3] {
    c.map(|v| v * s)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Linear RGB of one level-0 texel of `tile` (`x`, `y` in `0..TILE_SIZE`).
fn texel(tile: Tile, x: u32, y: u32) -> [f32; 3] {
    let seed = tile.index() * 7919 + 17;
    let grain = rand(x, y, seed) - 0.5;
    match tile {
        Tile::White => gray(1.0),
        Tile::Asphalt => {
            let speck = rand(x, y, seed + 1);
            let base = 0.74 + 0.10 * (fbm(x, y, 8, 3, seed) - 0.5) + 0.16 * grain;
            gray(if speck > 0.985 {
                0.98
            } else if speck < 0.02 {
                base * 0.6
            } else {
                base
            })
        }
        Tile::Concrete => {
            let pore = rand(x, y, seed + 1) > 0.993;
            let v = 0.84 + 0.10 * (fbm(x, y, 4, 4, seed) - 0.5) + 0.05 * grain;
            gray(if pore { v * 0.6 } else { v })
        }
        Tile::BrickRed => brick(x, y, [0.40, 0.12, 0.07], [0.62, 0.60, 0.55], seed),
        Tile::BrickTan => brick(x, y, [0.62, 0.47, 0.29], [0.24, 0.22, 0.20], seed),
        Tile::Stucco => gray(
            0.86 + 0.06 * (fbm(x, y, 2, 2, seed) - 0.5)
                + 0.07 * (fbm(x, y, 16, 3, seed + 1) - 0.5)
                + 0.06 * grain,
        ),
        Tile::WindowGlass => window(x, y),
        Tile::RoofGravel => {
            let big = rand(x / 4, y / 4, seed);
            let small = rand(x / 2, y / 2, seed + 1);
            gray(0.42 + 0.30 * big + 0.16 * small + 0.10 * grain)
        }
        Tile::Sand => {
            let warp = fbm(x, y, 4, 2, seed);
            let ripple = (core::f32::consts::TAU * (y as f32 / 64.0 + warp * 0.6)).sin();
            gray(0.86 + 0.045 * ripple + 0.10 * grain)
        }
        Tile::Lawn => {
            let blades = noise(x, y, 128, 16, seed);
            let band = if (x / 64).is_multiple_of(2) {
                0.03
            } else {
                -0.03
            };
            gray(0.74 + 0.26 * (blades - 0.5) + 0.12 * grain + band)
        }
        Tile::WoodSiding => {
            let (board, ly) = (y / 32, y % 32);
            let streak = noise(x, y, 4, 64, seed);
            let tone = 0.04 * (rand(board, 0, seed + 1) - 0.5);
            let lip = match ly {
                0..=2 => 0.55,
                3..=6 => 0.70 + 0.04 * (ly - 3) as f32,
                30..=31 => 1.05,
                _ => 1.0,
            };
            gray((0.84 + 0.10 * (streak - 0.5) + tone + 0.03 * grain) * lip)
        }
        Tile::AwningStripes => {
            let stripe = if (x / 32).is_multiple_of(2) {
                1.0
            } else {
                0.33
            };
            let weave = if (x + y).is_multiple_of(2) { 0.97 } else { 1.0 };
            gray(stripe * weave * (1.0 + 0.04 * grain))
        }
        Tile::GrilleMesh => {
            let (lx, ly) = (x % 32, y % 16);
            if ly < 4 {
                gray(if ly < 2 { 0.95 } else { 0.75 })
            } else if lx < 3 {
                gray(0.65)
            } else {
                gray(0.05 + 0.03 * (ly as f32 / 16.0))
            }
        }
        Tile::Sidewalk => {
            let (lx, ly) = (x % 128, y % 128);
            let v = 0.82 + 0.08 * (fbm(x, y, 4, 4, seed) - 0.5) + 0.06 * grain;
            gray(if lx < 2 || ly < 2 {
                0.42
            } else if lx < 4 || ly < 4 {
                v * 1.06
            } else {
                v
            })
        }
        Tile::RoofShingles => {
            let row = y / 32;
            let (lx, ly) = ((x + (row % 2) * 16) % 32, y % 32);
            let tab = rand((x + (row % 2) * 16) / 32, row, seed);
            gray(if ly < 3 || lx < 2 {
                0.22
            } else {
                (0.66 + 0.24 * tab + 0.12 * grain) * (0.85 + 0.15 * ly as f32 / 32.0)
            })
        }
        Tile::Soil => {
            let furrow = 0.5 + 0.5 * (core::f32::consts::TAU * y as f32 / 16.0).cos();
            gray(0.50 + 0.32 * furrow + 0.14 * (fbm(x, y, 16, 3, seed) - 0.5) + 0.10 * grain)
        }
    }
}

/// Running-bond brick, 8 courses of 32 px and bricks 64 px long, 4 px mortar joints.
fn brick(x: u32, y: u32, brick: [f32; 3], mortar: [f32; 3], seed: u32) -> [f32; 3] {
    let course = y / 32;
    let sx = x + (course % 2) * 32;
    let (lx, ly) = (sx % 64, y % 32);
    let grain = rand(x, y, seed) - 0.5;
    if lx < 4 || ly < 4 {
        return scale(mortar, 0.92 + 0.16 * rand(x, y, seed + 3));
    }
    let id = (sx / 64) % (TILE_SIZE / 64);
    let tone = 0.80 + 0.35 * rand(id, course, seed + 1);
    let burnt = rand(id, course, seed + 2) > 0.85;
    let mut c = scale(brick, tone * (1.0 + 0.12 * grain));
    if burnt {
        c = scale(c, 0.7);
    }
    // Slight shadow just under the bed joint above.
    if ly < 7 {
        c = scale(c, 0.85 + 0.05 * (ly - 4) as f32);
    }
    c
}

/// A window with a frame, a mullion cross and four panes reflecting a sky gradient.
fn window(x: u32, y: u32) -> [f32; 3] {
    const FRAME: u32 = 14;
    const MULLION: u32 = 5;
    let frame = [0.78, 0.77, 0.74];
    let center = TILE_SIZE / 2;
    let in_frame = x < FRAME || y < FRAME || x >= TILE_SIZE - FRAME || y >= TILE_SIZE - FRAME;
    let in_mullion = x.abs_diff(center) < MULLION || y.abs_diff(center) < MULLION;
    if in_frame || in_mullion {
        // Bevel: lighter on the top/left inner edges.
        return scale(
            frame,
            if x % 128 < FRAME + 2 || y % 128 < FRAME + 2 {
                1.0
            } else {
                0.9
            },
        );
    }
    // Position within the pane, 0 at the top.
    let pane_top = if y < center { FRAME } else { center + MULLION };
    let pane_h = (center - MULLION - FRAME) as f32;
    let t = (y - pane_top) as f32 / pane_h;
    let glass = mix([0.30, 0.40, 0.50], [0.07, 0.10, 0.14], t.clamp(0.0, 1.0));
    // A diagonal highlight streak.
    let d = (x + y) % 160;
    let streak = if (30..44).contains(&d) { 1.35 } else { 1.0 };
    // Inner shadow below the frame and the transom.
    let shadow = if y - pane_top < 6 { 0.7 } else { 1.0 };
    scale(glass, streak * shadow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_distinct_and_white_is_white() {
        assert_eq!(Tile::ALL.len(), (TILES_PER_ROW * TILES_PER_ROW) as usize);
        for (i, t) in Tile::ALL.iter().enumerate() {
            assert_eq!(t.index() as usize, i);
        }
        let atlas = TextureAtlas::generate();
        assert_eq!(atlas.level(0).len(), (ATLAS_SIZE * ATLAS_SIZE * 4) as usize);
        for l in 0..ATLAS_MIP_LEVELS {
            for (x, y) in [(0, 0), (255 >> l, 255 >> l), (17 >> l, 200 >> l)] {
                assert_eq!(atlas.texel(l, x, y), [255; 4], "level {l}");
            }
        }
        // Every non-white tile has texture (variance) and differs from the others.
        let mut means = Vec::new();
        for t in &Tile::ALL[1..] {
            let (ox, oy) = t.origin();
            let (mut sum, mut sq, mut n) = ([0f64; 3], 0f64, 0f64);
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let p = atlas.texel(0, ox + x, oy + y);
                    for k in 0..3 {
                        sum[k] += f64::from(p[k]);
                    }
                    sq += f64::from(p[1]) * f64::from(p[1]);
                    n += 1.0;
                }
            }
            let mean = sum.map(|s| s / n);
            let var = sq / n - mean[1] * mean[1];
            assert!(var > 4.0, "{t:?} is flat ({var})");
            means.push((t, mean));
        }
        for (i, a) in means.iter().enumerate() {
            for b in &means[i + 1..] {
                let d: f64 = (0..3).map(|k| (a.1[k] - b.1[k]).abs()).sum();
                assert!(d > 0.0 || a.0 == b.0, "{:?} == {:?}", a.0, b.0);
            }
        }
    }

    #[test]
    fn patterns_repeat_seamlessly() {
        // The step across the tile seam (last column -> first column) is no larger than
        // typical steps inside the tile, for every tile.
        let atlas = TextureAtlas::generate();
        for t in Tile::ALL {
            let (ox, oy) = t.origin();
            let px = |x: u32, y: u32| atlas.texel(0, ox + x % TILE_SIZE, oy + y % TILE_SIZE);
            let diff = |a: [u8; 4], b: [u8; 4]| -> u32 {
                (0..3).map(|k| a[k].abs_diff(b[k]) as u32).sum()
            };
            let (mut seam, mut inner) = (0u32, 0u32);
            for i in 0..TILE_SIZE {
                seam += diff(px(TILE_SIZE - 1, i), px(0, i)) + diff(px(i, TILE_SIZE - 1), px(i, 0));
                inner += diff(px(127, i), px(128, i)) + diff(px(i, 127), px(i, 128));
            }
            assert!(
                seam <= inner + inner / 2 + 12 * TILE_SIZE,
                "{t:?}: seam {seam} inner {inner}"
            );
        }
    }

    #[test]
    fn mips_stay_inside_their_tile() {
        let atlas = TextureAtlas::generate();
        let last = ATLAS_MIP_LEVELS - 1;
        assert_eq!(TILE_SIZE >> last, 32);
        // White's last-level texels next to its neighbours are still pure white.
        let s = TILE_SIZE >> last;
        for i in 0..s {
            assert_eq!(atlas.texel(last, s - 1, i), [255; 4]);
            assert_eq!(atlas.texel(last, i, s - 1), [255; 4]);
        }
    }

    #[test]
    fn tile_uv_is_inset_and_upright() {
        assert_eq!(TILE_INSET, 4);
        for t in Tile::ALL {
            let [bl, br, tr, tl] = tile_uv(t);
            assert!(bl[1] > tl[1] && br[0] > bl[0]);
            assert_eq!(bl[0], tl[0]);
            assert_eq!(tr[1], tl[1]);
            let (ox, oy) = t.origin();
            let s = ATLAS_SIZE as f32;
            assert!((tl[0] * s - ox as f32 - 4.0).abs() < 1e-3);
            assert!((tl[1] * s - oy as f32 - 4.0).abs() < 1e-3);
            assert!(((br[0] * s) - (ox + TILE_SIZE) as f32 + 4.0).abs() < 1e-3);
        }
        assert_eq!(uv_unorm([0.0, 1.0]), [0, 65535]);
        assert_eq!(uv_unorm([0.5, 2.0]), [32768, 65535]);
    }
}
