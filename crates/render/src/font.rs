//! Skywave's HUD font: a bold, slightly rounded 7×9 pixel face for printable ASCII
//! (32–126), drawn for this project (credit: ours; no font file is shipped or derived
//! from). Glyphs are `const` bit rows below; [`FontAtlas::generate`] rasterizes them at
//! startup into one small single-channel texture.
//!
//! Metrics, in font pixels: a 9-row cell with the cap height in rows 0–6 (the baseline
//! is under row 6), lowercase x-height in rows 2–6 and descenders in rows 7–8. Glyphs
//! are proportional ([`Glyph::width`] columns, plus one column of spacing); the digits are
//! all 6 wide so numbers line up in columns. Lines are [`LINE_HEIGHT`] apart.
//!
//! The atlas holds every glyph once, at 1×. The HUD pass draws a glyph at an integer
//! number of screen pixels per font pixel (1×, 2×, 4×, …) with nearest sampling, so text
//! stays exactly as crisp as the bitmap at every scale.

/// Columns available to a glyph.
pub const GLYPH_W: u32 = 7;
/// Rows in a glyph cell.
pub const GLYPH_H: u32 = 9;
/// Rows from the top of the cell to the baseline.
pub const CAP_HEIGHT: u32 = 7;
/// Distance between baselines of consecutive lines.
pub const LINE_HEIGHT: u32 = 12;
/// First and last character with a glyph.
pub const FIRST_CHAR: char = ' ';
pub const LAST_CHAR: char = '~';
/// Glyphs in the font: printable ASCII.
pub const GLYPH_COUNT: usize = 95;
/// Characters without a glyph draw this one.
pub const FALLBACK_CHAR: char = '?';

/// Atlas cells: a glyph plus a one-texel gutter right and below.
pub const CELL_W: u32 = GLYPH_W + 1;
pub const CELL_H: u32 = GLYPH_H + 1;
/// Cells per atlas row.
pub const ATLAS_COLUMNS: u32 = 16;
/// Atlas size in texels (one byte each).
pub const ATLAS_WIDTH: u32 = ATLAS_COLUMNS * CELL_W;
pub const ATLAS_HEIGHT: u32 = 64;
/// The atlas cell after the last glyph is solid white: rectangles sample its middle.
pub const SOLID_CELL: usize = GLYPH_COUNT;

/// One glyph: `rows[y]` holds the pixels of row `y`, bit 6 the leftmost column, so each
/// binary literal below reads like the glyph itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    /// Columns used, from the left; the pen advances `width + 1`.
    pub width: u8,
    pub rows: [u8; GLYPH_H as usize],
}

impl Glyph {
    /// Whether the pixel at column `x`, row `y` is set.
    pub fn pixel(&self, x: u32, y: u32) -> bool {
        x < GLYPH_W && y < GLYPH_H && self.rows[y as usize] & (1 << (GLYPH_W - 1 - x)) != 0
    }
}

/// Index of `c`'s glyph in [`GLYPHS`]; characters outside printable ASCII get
/// [`FALLBACK_CHAR`]'s.
pub fn glyph_index(c: char) -> usize {
    let c = if (FIRST_CHAR..=LAST_CHAR).contains(&c) {
        c
    } else {
        FALLBACK_CHAR
    };
    c as usize - FIRST_CHAR as usize
}

pub fn glyph(c: char) -> &'static Glyph {
    &GLYPHS[glyph_index(c)]
}

/// Pen advance after `c`, in font pixels.
pub fn advance(c: char) -> u32 {
    u32::from(glyph(c).width) + 1
}

/// Width of `text` in font pixels: its widest line (lines split at `'\n'`), without the
/// spacing after the last glyph.
pub fn text_width(text: &str) -> u32 {
    text.split('\n')
        .map(|line| line.chars().map(advance).sum::<u32>().saturating_sub(1))
        .max()
        .unwrap_or(0)
}

/// Top-left texel of atlas cell `index` (a glyph index, or [`SOLID_CELL`]).
pub fn cell_origin(index: usize) -> [u32; 2] {
    let i = index as u32;
    [(i % ATLAS_COLUMNS) * CELL_W, (i / ATLAS_COLUMNS) * CELL_H]
}

/// The rasterized font: [`ATLAS_WIDTH`] × [`ATLAS_HEIGHT`] coverage bytes (0 or 255), row
/// major, for an `R8Unorm` texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontAtlas {
    pub pixels: Vec<u8>,
}

impl FontAtlas {
    pub fn generate() -> FontAtlas {
        let mut pixels = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT) as usize];
        let mut put = |x: u32, y: u32| pixels[(y * ATLAS_WIDTH + x) as usize] = 255;
        for (i, g) in GLYPHS.iter().enumerate() {
            let [ox, oy] = cell_origin(i);
            for y in 0..GLYPH_H {
                for x in 0..GLYPH_W {
                    if g.pixel(x, y) {
                        put(ox + x, oy + y);
                    }
                }
            }
        }
        let [ox, oy] = cell_origin(SOLID_CELL);
        for y in 0..GLYPH_H {
            for x in 0..GLYPH_W {
                put(ox + x, oy + y);
            }
        }
        FontAtlas { pixels }
    }

    /// Coverage at texel `(x, y)`.
    pub fn texel(&self, x: u32, y: u32) -> u8 {
        self.pixels[(y * ATLAS_WIDTH + x) as usize]
    }
}

#[rustfmt::skip]
pub const GLYPHS: [Glyph; GLYPH_COUNT] = [
    // ' '
    Glyph { width: 3, rows: [
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // '!'
    Glyph { width: 2, rows: [
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // '"'
    Glyph { width: 5, rows: [
        0b1101100,
        0b1101100,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // '#'
    Glyph { width: 7, rows: [
        0b0110110,
        0b0110110,
        0b1111111,
        0b0110110,
        0b1111111,
        0b0110110,
        0b0110110,
        0b0000000,
        0b0000000,
    ] },
    // '$'
    Glyph { width: 6, rows: [
        0b0011000,
        0b0111110,
        0b1100000,
        0b0111100,
        0b0000110,
        0b1111100,
        0b0011000,
        0b0000000,
        0b0000000,
    ] },
    // '%'
    Glyph { width: 6, rows: [
        0b1100010,
        0b1100110,
        0b0001100,
        0b0011000,
        0b0110000,
        0b1100110,
        0b1000110,
        0b0000000,
        0b0000000,
    ] },
    // '&'
    Glyph { width: 7, rows: [
        0b0111000,
        0b1101100,
        0b0111000,
        0b0111011,
        0b1101110,
        0b1100110,
        0b0111011,
        0b0000000,
        0b0000000,
    ] },
    // '\''
    Glyph { width: 2, rows: [
        0b1100000,
        0b1100000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // '('
    Glyph { width: 4, rows: [
        0b0011000,
        0b0110000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0110000,
        0b0011000,
        0b0000000,
        0b0000000,
    ] },
    // ')'
    Glyph { width: 4, rows: [
        0b1100000,
        0b0110000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0110000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // '*'
    Glyph { width: 5, rows: [
        0b0000000,
        0b1010100,
        0b0111000,
        0b1111100,
        0b0111000,
        0b1010100,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // '+'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0011000,
        0b0011000,
        0b1111110,
        0b0011000,
        0b0011000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // ','
    Glyph { width: 3, rows: [
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0110000,
        0b0110000,
        0b1100000,
        0b0000000,
    ] },
    // '-'
    Glyph { width: 5, rows: [
        0b0000000,
        0b0000000,
        0b0000000,
        0b1111100,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // '.'
    Glyph { width: 2, rows: [
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // '/'
    Glyph { width: 6, rows: [
        0b0000110,
        0b0000110,
        0b0001100,
        0b0011000,
        0b0110000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // '0'
    Glyph { width: 6, rows: [
        0b0111100,
        0b1100110,
        0b1101110,
        0b1111110,
        0b1110110,
        0b1100110,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // '1'
    Glyph { width: 6, rows: [
        0b0011000,
        0b0111000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // '2'
    Glyph { width: 6, rows: [
        0b0111100,
        0b1100110,
        0b0000110,
        0b0011100,
        0b0110000,
        0b1100000,
        0b1111110,
        0b0000000,
        0b0000000,
    ] },
    // '3'
    Glyph { width: 6, rows: [
        0b0111100,
        0b1100110,
        0b0000110,
        0b0011100,
        0b0000110,
        0b1100110,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // '4'
    Glyph { width: 6, rows: [
        0b0001110,
        0b0011110,
        0b0110110,
        0b1100110,
        0b1111110,
        0b0000110,
        0b0000110,
        0b0000000,
        0b0000000,
    ] },
    // '5'
    Glyph { width: 6, rows: [
        0b1111110,
        0b1100000,
        0b1111100,
        0b0000110,
        0b0000110,
        0b1100110,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // '6'
    Glyph { width: 6, rows: [
        0b0011100,
        0b0110000,
        0b1100000,
        0b1111100,
        0b1100110,
        0b1100110,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // '7'
    Glyph { width: 6, rows: [
        0b1111110,
        0b0000110,
        0b0001100,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0000000,
        0b0000000,
    ] },
    // '8'
    Glyph { width: 6, rows: [
        0b0111100,
        0b1100110,
        0b1100110,
        0b0111100,
        0b1100110,
        0b1100110,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // '9'
    Glyph { width: 6, rows: [
        0b0111100,
        0b1100110,
        0b1100110,
        0b0111110,
        0b0000110,
        0b0001100,
        0b0111000,
        0b0000000,
        0b0000000,
    ] },
    // ':'
    Glyph { width: 2, rows: [
        0b0000000,
        0b0000000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // ';'
    Glyph { width: 3, rows: [
        0b0000000,
        0b0000000,
        0b0110000,
        0b0110000,
        0b0000000,
        0b0110000,
        0b0110000,
        0b1100000,
        0b0000000,
    ] },
    // '<'
    Glyph { width: 5, rows: [
        0b0001100,
        0b0011000,
        0b0110000,
        0b1100000,
        0b0110000,
        0b0011000,
        0b0001100,
        0b0000000,
        0b0000000,
    ] },
    // '='
    Glyph { width: 5, rows: [
        0b0000000,
        0b0000000,
        0b1111100,
        0b0000000,
        0b1111100,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // '>'
    Glyph { width: 5, rows: [
        0b1100000,
        0b0110000,
        0b0011000,
        0b0001100,
        0b0011000,
        0b0110000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // '?'
    Glyph { width: 6, rows: [
        0b0111100,
        0b1100110,
        0b0000110,
        0b0001100,
        0b0011000,
        0b0000000,
        0b0011000,
        0b0000000,
        0b0000000,
    ] },
    // '@'
    Glyph { width: 7, rows: [
        0b0111110,
        0b1100011,
        0b1101111,
        0b1101101,
        0b1101111,
        0b1100000,
        0b0111111,
        0b0000000,
        0b0000000,
    ] },
    // 'A'
    Glyph { width: 7, rows: [
        0b0011100,
        0b0110110,
        0b1100011,
        0b1100011,
        0b1111111,
        0b1100011,
        0b1100011,
        0b0000000,
        0b0000000,
    ] },
    // 'B'
    Glyph { width: 7, rows: [
        0b1111110,
        0b1100011,
        0b1100011,
        0b1111110,
        0b1100011,
        0b1100011,
        0b1111110,
        0b0000000,
        0b0000000,
    ] },
    // 'C'
    Glyph { width: 7, rows: [
        0b0111110,
        0b1100011,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100011,
        0b0111110,
        0b0000000,
        0b0000000,
    ] },
    // 'D'
    Glyph { width: 7, rows: [
        0b1111100,
        0b1100110,
        0b1100011,
        0b1100011,
        0b1100011,
        0b1100110,
        0b1111100,
        0b0000000,
        0b0000000,
    ] },
    // 'E'
    Glyph { width: 6, rows: [
        0b1111110,
        0b1100000,
        0b1100000,
        0b1111100,
        0b1100000,
        0b1100000,
        0b1111110,
        0b0000000,
        0b0000000,
    ] },
    // 'F'
    Glyph { width: 6, rows: [
        0b1111110,
        0b1100000,
        0b1100000,
        0b1111100,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // 'G'
    Glyph { width: 7, rows: [
        0b0111110,
        0b1100011,
        0b1100000,
        0b1101111,
        0b1100011,
        0b1100011,
        0b0111110,
        0b0000000,
        0b0000000,
    ] },
    // 'H'
    Glyph { width: 7, rows: [
        0b1100011,
        0b1100011,
        0b1100011,
        0b1111111,
        0b1100011,
        0b1100011,
        0b1100011,
        0b0000000,
        0b0000000,
    ] },
    // 'I'
    Glyph { width: 4, rows: [
        0b1111000,
        0b0110000,
        0b0110000,
        0b0110000,
        0b0110000,
        0b0110000,
        0b1111000,
        0b0000000,
        0b0000000,
    ] },
    // 'J'
    Glyph { width: 6, rows: [
        0b0011110,
        0b0000110,
        0b0000110,
        0b0000110,
        0b1100110,
        0b1100110,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // 'K'
    Glyph { width: 7, rows: [
        0b1100011,
        0b1100110,
        0b1101100,
        0b1111000,
        0b1101100,
        0b1100110,
        0b1100011,
        0b0000000,
        0b0000000,
    ] },
    // 'L'
    Glyph { width: 6, rows: [
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1111110,
        0b0000000,
        0b0000000,
    ] },
    // 'M'
    Glyph { width: 7, rows: [
        0b1100011,
        0b1110111,
        0b1111111,
        0b1101011,
        0b1100011,
        0b1100011,
        0b1100011,
        0b0000000,
        0b0000000,
    ] },
    // 'N'
    Glyph { width: 7, rows: [
        0b1100011,
        0b1110011,
        0b1111011,
        0b1101111,
        0b1100111,
        0b1100011,
        0b1100011,
        0b0000000,
        0b0000000,
    ] },
    // 'O'
    Glyph { width: 7, rows: [
        0b0111110,
        0b1100011,
        0b1100011,
        0b1100011,
        0b1100011,
        0b1100011,
        0b0111110,
        0b0000000,
        0b0000000,
    ] },
    // 'P'
    Glyph { width: 7, rows: [
        0b1111110,
        0b1100011,
        0b1100011,
        0b1111110,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // 'Q'
    Glyph { width: 7, rows: [
        0b0111110,
        0b1100011,
        0b1100011,
        0b1100011,
        0b1101011,
        0b1100110,
        0b0111011,
        0b0000000,
        0b0000000,
    ] },
    // 'R'
    Glyph { width: 7, rows: [
        0b1111110,
        0b1100011,
        0b1100011,
        0b1111110,
        0b1101100,
        0b1100110,
        0b1100011,
        0b0000000,
        0b0000000,
    ] },
    // 'S'
    Glyph { width: 7, rows: [
        0b0111110,
        0b1100011,
        0b1100000,
        0b0111110,
        0b0000011,
        0b1100011,
        0b0111110,
        0b0000000,
        0b0000000,
    ] },
    // 'T'
    Glyph { width: 6, rows: [
        0b1111110,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0000000,
        0b0000000,
    ] },
    // 'U'
    Glyph { width: 7, rows: [
        0b1100011,
        0b1100011,
        0b1100011,
        0b1100011,
        0b1100011,
        0b1100011,
        0b0111110,
        0b0000000,
        0b0000000,
    ] },
    // 'V'
    Glyph { width: 7, rows: [
        0b1100011,
        0b1100011,
        0b1100011,
        0b1100011,
        0b0110110,
        0b0011100,
        0b0001000,
        0b0000000,
        0b0000000,
    ] },
    // 'W'
    Glyph { width: 7, rows: [
        0b1100011,
        0b1100011,
        0b1100011,
        0b1101011,
        0b1111111,
        0b1110111,
        0b1100011,
        0b0000000,
        0b0000000,
    ] },
    // 'X'
    Glyph { width: 7, rows: [
        0b1100011,
        0b1100011,
        0b0110110,
        0b0011100,
        0b0110110,
        0b1100011,
        0b1100011,
        0b0000000,
        0b0000000,
    ] },
    // 'Y'
    Glyph { width: 6, rows: [
        0b1100110,
        0b1100110,
        0b1100110,
        0b0111100,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0000000,
        0b0000000,
    ] },
    // 'Z'
    Glyph { width: 7, rows: [
        0b1111111,
        0b0000011,
        0b0000110,
        0b0001100,
        0b0011000,
        0b0110000,
        0b1111111,
        0b0000000,
        0b0000000,
    ] },
    // '['
    Glyph { width: 4, rows: [
        0b1111000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1111000,
        0b0000000,
        0b0000000,
    ] },
    // '\\'
    Glyph { width: 6, rows: [
        0b1100000,
        0b1100000,
        0b0110000,
        0b0011000,
        0b0001100,
        0b0000110,
        0b0000110,
        0b0000000,
        0b0000000,
    ] },
    // ']'
    Glyph { width: 4, rows: [
        0b1111000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b1111000,
        0b0000000,
        0b0000000,
    ] },
    // '^'
    Glyph { width: 6, rows: [
        0b0011000,
        0b0111100,
        0b1100110,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // '_'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b1111110,
        0b0000000,
    ] },
    // '`'
    Glyph { width: 3, rows: [
        0b1100000,
        0b0110000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
    // 'a'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b0111100,
        0b0000110,
        0b0111110,
        0b1100110,
        0b0111110,
        0b0000000,
        0b0000000,
    ] },
    // 'b'
    Glyph { width: 6, rows: [
        0b1100000,
        0b1100000,
        0b1111100,
        0b1100110,
        0b1100110,
        0b1100110,
        0b1111100,
        0b0000000,
        0b0000000,
    ] },
    // 'c'
    Glyph { width: 5, rows: [
        0b0000000,
        0b0000000,
        0b0111100,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // 'd'
    Glyph { width: 6, rows: [
        0b0000110,
        0b0000110,
        0b0111110,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0111110,
        0b0000000,
        0b0000000,
    ] },
    // 'e'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b0111100,
        0b1100110,
        0b1111110,
        0b1100000,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // 'f'
    Glyph { width: 5, rows: [
        0b0011100,
        0b0110000,
        0b1111000,
        0b0110000,
        0b0110000,
        0b0110000,
        0b0110000,
        0b0000000,
        0b0000000,
    ] },
    // 'g'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b0111110,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0111110,
        0b0000110,
        0b0111100,
    ] },
    // 'h'
    Glyph { width: 6, rows: [
        0b1100000,
        0b1100000,
        0b1111100,
        0b1100110,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0000000,
        0b0000000,
    ] },
    // 'i'
    Glyph { width: 2, rows: [
        0b1100000,
        0b0000000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // 'j'
    Glyph { width: 4, rows: [
        0b0011000,
        0b0000000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b0011000,
        0b1110000,
    ] },
    // 'k'
    Glyph { width: 6, rows: [
        0b1100000,
        0b1100000,
        0b1100110,
        0b1101100,
        0b1111000,
        0b1101100,
        0b1100110,
        0b0000000,
        0b0000000,
    ] },
    // 'l'
    Glyph { width: 3, rows: [
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0110000,
        0b0000000,
        0b0000000,
    ] },
    // 'm'
    Glyph { width: 7, rows: [
        0b0000000,
        0b0000000,
        0b1111110,
        0b1101011,
        0b1101011,
        0b1101011,
        0b1101011,
        0b0000000,
        0b0000000,
    ] },
    // 'n'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b1111100,
        0b1100110,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0000000,
        0b0000000,
    ] },
    // 'o'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b0111100,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0111100,
        0b0000000,
        0b0000000,
    ] },
    // 'p'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b1111100,
        0b1100110,
        0b1100110,
        0b1100110,
        0b1111100,
        0b1100000,
        0b1100000,
    ] },
    // 'q'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b0111110,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0111110,
        0b0000110,
        0b0000110,
    ] },
    // 'r'
    Glyph { width: 5, rows: [
        0b0000000,
        0b0000000,
        0b1101100,
        0b1111000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // 's'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b0111110,
        0b1100000,
        0b0111100,
        0b0000110,
        0b1111100,
        0b0000000,
        0b0000000,
    ] },
    // 't'
    Glyph { width: 5, rows: [
        0b0110000,
        0b0110000,
        0b1111100,
        0b0110000,
        0b0110000,
        0b0110000,
        0b0011100,
        0b0000000,
        0b0000000,
    ] },
    // 'u'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b1100110,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0111110,
        0b0000000,
        0b0000000,
    ] },
    // 'v'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0111100,
        0b0011000,
        0b0000000,
        0b0000000,
    ] },
    // 'w'
    Glyph { width: 7, rows: [
        0b0000000,
        0b0000000,
        0b1100011,
        0b1101011,
        0b1101011,
        0b1101011,
        0b0110110,
        0b0000000,
        0b0000000,
    ] },
    // 'x'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b1100110,
        0b0111100,
        0b0011000,
        0b0111100,
        0b1100110,
        0b0000000,
        0b0000000,
    ] },
    // 'y'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b1100110,
        0b1100110,
        0b1100110,
        0b1100110,
        0b0111110,
        0b0000110,
        0b0111100,
    ] },
    // 'z'
    Glyph { width: 6, rows: [
        0b0000000,
        0b0000000,
        0b1111110,
        0b0001100,
        0b0011000,
        0b0110000,
        0b1111110,
        0b0000000,
        0b0000000,
    ] },
    // '{'
    Glyph { width: 4, rows: [
        0b0011000,
        0b0110000,
        0b0110000,
        0b1100000,
        0b0110000,
        0b0110000,
        0b0011000,
        0b0000000,
        0b0000000,
    ] },
    // '|'
    Glyph { width: 2, rows: [
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b1100000,
        0b0000000,
    ] },
    // '}'
    Glyph { width: 4, rows: [
        0b1100000,
        0b0110000,
        0b0110000,
        0b0011000,
        0b0110000,
        0b0110000,
        0b1100000,
        0b0000000,
        0b0000000,
    ] },
    // '~'
    Glyph { width: 7, rows: [
        0b0000000,
        0b0000000,
        0b0111011,
        0b1101110,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
        0b0000000,
    ] },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_printable_character_has_a_glyph() {
        assert_eq!(GLYPHS.len(), LAST_CHAR as usize - FIRST_CHAR as usize + 1);
        for c in FIRST_CHAR..=LAST_CHAR {
            let g = glyph(c);
            let w = u32::from(g.width);
            assert!((1..=GLYPH_W).contains(&w), "{c:?} width {w}");
            let lit = (0..GLYPH_H)
                .flat_map(|y| (0..GLYPH_W).map(move |x| (x, y)))
                .filter(|&(x, y)| g.pixel(x, y))
                .collect::<Vec<_>>();
            if c == ' ' {
                assert!(lit.is_empty());
                continue;
            }
            assert!(!lit.is_empty(), "{c:?} is blank");
            // Nothing outside the glyph's own columns, and it starts at column 0 (the
            // digits are tabular and may keep a blank first column).
            assert!(lit.iter().all(|&(x, _)| x < w), "{c:?} spills past {w}");
            if !c.is_ascii_digit() {
                assert!(lit.iter().any(|&(x, _)| x == 0), "{c:?} not left-aligned");
            }
            // Only the descenders and low punctuation reach below the baseline.
            let below = lit.iter().any(|&(_, y)| y >= CAP_HEIGHT);
            assert_eq!(below, "gjpqy,;_|".contains(c), "{c:?} below baseline");
        }
        for d in '0'..='9' {
            assert_eq!(glyph(d).width, 6, "{d:?} is not tabular");
        }
        // Distinct shapes for every character.
        for (a, ga) in GLYPHS.iter().enumerate() {
            for (b, gb) in GLYPHS.iter().enumerate().skip(a + 1) {
                assert_ne!(ga.rows, gb.rows, "{a} and {b} look alike");
            }
        }
        assert_eq!(glyph('\u{e9}'), glyph(FALLBACK_CHAR));
        assert_eq!(glyph('\n'), glyph(FALLBACK_CHAR));
    }

    #[test]
    fn text_width_measures_the_widest_line() {
        assert_eq!(text_width(""), 0);
        assert_eq!(text_width("I"), 4);
        // H (7) + gap + i (2).
        assert_eq!(text_width("Hi"), 10);
        assert_eq!(text_width("Hi\nI"), 10);
        assert_eq!(text_width("0123456789"), 10 * 7 - 1);
        assert_eq!(advance(' '), 4);
    }

    #[test]
    fn atlas_packs_every_glyph_without_overlap() {
        let atlas = FontAtlas::generate();
        assert_eq!(atlas.pixels.len(), (ATLAS_WIDTH * ATLAS_HEIGHT) as usize);
        // Cells lie inside the atlas and never overlap (gutters included).
        let cells: Vec<[u32; 2]> = (0..=SOLID_CELL).map(cell_origin).collect();
        for (i, a) in cells.iter().enumerate() {
            assert!(
                a[0] + CELL_W <= ATLAS_WIDTH && a[1] + CELL_H <= ATLAS_HEIGHT,
                "{i}"
            );
            for b in &cells[i + 1..] {
                let apart = a[0] + CELL_W <= b[0]
                    || b[0] + CELL_W <= a[0]
                    || a[1] + CELL_H <= b[1]
                    || b[1] + CELL_H <= a[1];
                assert!(apart, "cells {a:?} and {b:?} overlap");
            }
        }
        // Each cell holds exactly its glyph; gutters and the unused area stay empty.
        let mut owned = vec![false; atlas.pixels.len()];
        for (i, g) in GLYPHS.iter().enumerate() {
            let [ox, oy] = cell_origin(i);
            for y in 0..GLYPH_H {
                for x in 0..GLYPH_W {
                    let want = if g.pixel(x, y) { 255 } else { 0 };
                    assert_eq!(atlas.texel(ox + x, oy + y), want, "glyph {i} ({x},{y})");
                    owned[((oy + y) * ATLAS_WIDTH + ox + x) as usize] = true;
                }
            }
        }
        let [sx, sy] = cell_origin(SOLID_CELL);
        for y in 0..GLYPH_H {
            for x in 0..GLYPH_W {
                assert_eq!(atlas.texel(sx + x, sy + y), 255);
                owned[((sy + y) * ATLAS_WIDTH + sx + x) as usize] = true;
            }
        }
        for (i, &p) in atlas.pixels.iter().enumerate() {
            if !owned[i] {
                assert_eq!(p, 0, "stray texel {i}");
            }
        }
    }
}
