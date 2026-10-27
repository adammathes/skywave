//! Stroke letterforms for signs, ours and drawn from scratch: each glyph is a few
//! polylines on a grid 20 units to the cap height, laid out into [`Strokes`] (polylines in
//! em, cap height 1, with a stroke width) and built as neon tubes, a backlit box, channel
//! letters or flat paint.
//!
//! Four styles ([`LetterStyle`]):
//! - **Block** capitals: square, chamfered bowls, even spacing.
//! - **Condensed** sans: the block skeleton narrowed to tall chamfered letters, a thinner
//!   stroke.
//! - **Script**: connected lowercase for neon, a tube that runs on from letter to letter,
//!   a rounded capital at the start of each word, slanted.
//! - **Slab** serif for painted signs: the block skeleton with slab serifs on every stem
//!   end at the baseline or cap line and beaks on the arms.
//!
//! Text is uppercase ASCII letters, digits, spaces and apostrophes; anything else lays
//! out as a space. Everything here is pure arithmetic on its inputs.

use alloc::vec::Vec;

use crate::mesh::{quad, Frame, Paint, WorldMesh};

/// A lettering style.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LetterStyle {
    Block = 0,
    Condensed = 1,
    Script = 2,
    Slab = 3,
}

impl LetterStyle {
    pub const ALL: [LetterStyle; 4] = [
        LetterStyle::Block,
        LetterStyle::Condensed,
        LetterStyle::Script,
        LetterStyle::Slab,
    ];
}

/// How a style is drawn, in grid units (20 to the cap height).
struct Metrics {
    /// Stroke width.
    stroke: f32,
    /// Horizontal scale of the block skeleton.
    narrow: f32,
    /// Space between letters, ink to ink.
    gap: f32,
    /// Width of a space, ink to ink.
    space: f32,
    /// Horizontal shear per unit of height.
    slant: f32,
}

fn metrics(style: LetterStyle) -> Metrics {
    match style {
        LetterStyle::Block => Metrics {
            stroke: 3.2,
            narrow: 1.0,
            gap: 2.8,
            space: 8.0,
            slant: 0.0,
        },
        LetterStyle::Condensed => Metrics {
            stroke: 2.4,
            narrow: 0.62,
            gap: 2.2,
            space: 6.0,
            slant: 0.0,
        },
        LetterStyle::Script => Metrics {
            stroke: 1.5,
            narrow: 0.8,
            gap: 2.0,
            space: 7.0,
            slant: 0.24,
        },
        LetterStyle::Slab => Metrics {
            stroke: 2.8,
            narrow: 0.95,
            gap: 3.4,
            space: 8.0,
            slant: 0.0,
        },
    }
}

type P = (i8, i8);
/// A glyph: its width and its strokes.
type Glyph = (i8, &'static [&'static [P]]);

/// The block skeleton: capitals, digits and the apostrophe.
fn block(c: u8) -> Glyph {
    const O: &[P] = &[
        (3, 0),
        (0, 3),
        (0, 17),
        (3, 20),
        (7, 20),
        (10, 17),
        (10, 3),
        (7, 0),
        (3, 0),
    ];
    const P_BOWL: &[P] = &[
        (0, 0),
        (0, 20),
        (7, 20),
        (10, 17),
        (10, 13),
        (7, 10),
        (0, 10),
    ];
    match c {
        b'A' => (10, &[&[(0, 0), (5, 20), (10, 0)], &[(2, 7), (8, 7)]]),
        b'B' => (
            10,
            &[
                &[(0, 0), (0, 20), (7, 20), (9, 18), (9, 12), (7, 10), (0, 10)],
                &[(7, 10), (10, 8), (10, 2), (8, 0), (0, 0)],
            ],
        ),
        b'C' => (
            10,
            &[&[
                (10, 17),
                (7, 20),
                (3, 20),
                (0, 17),
                (0, 3),
                (3, 0),
                (7, 0),
                (10, 3),
            ]],
        ),
        b'D' => (
            10,
            &[&[(0, 0), (0, 20), (6, 20), (10, 16), (10, 4), (6, 0), (0, 0)]],
        ),
        b'E' => (
            10,
            &[&[(10, 20), (0, 20), (0, 0), (10, 0)], &[(0, 10), (7, 10)]],
        ),
        b'F' => (10, &[&[(10, 20), (0, 20), (0, 0)], &[(0, 10), (7, 10)]]),
        b'G' => (
            10,
            &[&[
                (10, 17),
                (7, 20),
                (3, 20),
                (0, 17),
                (0, 3),
                (3, 0),
                (7, 0),
                (10, 3),
                (10, 9),
                (6, 9),
            ]],
        ),
        b'H' => (
            10,
            &[
                &[(0, 0), (0, 20)],
                &[(10, 0), (10, 20)],
                &[(0, 10), (10, 10)],
            ],
        ),
        b'I' => (0, &[&[(0, 0), (0, 20)]]),
        b'J' => (9, &[&[(9, 20), (9, 3), (6, 0), (3, 0), (0, 3), (0, 6)]]),
        b'K' => (
            10,
            &[&[(0, 0), (0, 20)], &[(10, 20), (0, 8)], &[(4, 12), (10, 0)]],
        ),
        b'L' => (9, &[&[(0, 20), (0, 0), (9, 0)]]),
        b'M' => (12, &[&[(0, 0), (0, 20), (6, 8), (12, 20), (12, 0)]]),
        b'N' => (10, &[&[(0, 0), (0, 20), (10, 0), (10, 20)]]),
        b'O' => (10, &[O]),
        b'P' => (10, &[P_BOWL]),
        b'Q' => (10, &[O, &[(6, 5), (11, -1)]]),
        b'R' => (10, &[P_BOWL, &[(5, 10), (10, 0)]]),
        b'S' => (
            10,
            &[&[
                (10, 17),
                (7, 20),
                (3, 20),
                (0, 17),
                (0, 13),
                (3, 10),
                (7, 10),
                (10, 7),
                (10, 3),
                (7, 0),
                (3, 0),
                (0, 3),
            ]],
        ),
        b'T' => (10, &[&[(0, 20), (10, 20)], &[(5, 20), (5, 0)]]),
        b'U' => (10, &[&[(0, 20), (0, 3), (3, 0), (7, 0), (10, 3), (10, 20)]]),
        b'V' => (10, &[&[(0, 20), (5, 0), (10, 20)]]),
        b'W' => (14, &[&[(0, 20), (3, 0), (7, 13), (11, 0), (14, 20)]]),
        b'X' => (10, &[&[(0, 20), (10, 0)], &[(0, 0), (10, 20)]]),
        b'Y' => (10, &[&[(0, 20), (5, 10), (10, 20)], &[(5, 10), (5, 0)]]),
        b'Z' => (10, &[&[(0, 20), (10, 20), (0, 0), (10, 0)]]),
        b'0' => (10, &[O]),
        b'1' => (5, &[&[(0, 16), (5, 20), (5, 0)]]),
        b'2' => (
            10,
            &[&[
                (0, 17),
                (3, 20),
                (7, 20),
                (10, 17),
                (10, 12),
                (0, 0),
                (10, 0),
            ]],
        ),
        b'3' => (
            10,
            &[
                &[
                    (0, 17),
                    (3, 20),
                    (7, 20),
                    (10, 17),
                    (10, 13),
                    (7, 10),
                    (3, 10),
                ],
                &[(7, 10), (10, 7), (10, 3), (7, 0), (3, 0), (0, 3)],
            ],
        ),
        b'4' => (11, &[&[(8, 0), (8, 20), (0, 6), (11, 6)]]),
        b'5' => (
            10,
            &[&[
                (10, 20),
                (0, 20),
                (0, 11),
                (7, 11),
                (10, 8),
                (10, 3),
                (7, 0),
                (0, 0),
            ]],
        ),
        b'6' => (
            10,
            &[&[
                (9, 20),
                (4, 20),
                (0, 15),
                (0, 3),
                (3, 0),
                (7, 0),
                (10, 3),
                (10, 8),
                (7, 11),
                (0, 11),
            ]],
        ),
        b'7' => (10, &[&[(0, 20), (10, 20), (3, 0)]]),
        b'8' => (
            10,
            &[
                &[
                    (3, 10),
                    (0, 13),
                    (0, 17),
                    (3, 20),
                    (7, 20),
                    (10, 17),
                    (10, 13),
                    (7, 10),
                    (3, 10),
                ],
                &[
                    (3, 10),
                    (0, 7),
                    (0, 3),
                    (3, 0),
                    (7, 0),
                    (10, 3),
                    (10, 7),
                    (7, 10),
                ],
            ],
        ),
        b'9' => (
            10,
            &[&[
                (1, 0),
                (6, 0),
                (10, 5),
                (10, 17),
                (7, 20),
                (3, 20),
                (0, 17),
                (0, 12),
                (3, 9),
                (10, 9),
            ]],
        ),
        b'\'' => (0, &[&[(0, 20), (0, 15)]]),
        _ => (-1, &[]),
    }
}

/// The script lowercase: x-height 10, ascenders 20, descenders to -8. The first stroke
/// runs from the entry at the left (near the baseline) to the exit, where the next letter's
/// entry joins it; other strokes are dots and bars.
fn script(c: u8) -> Glyph {
    match c {
        b'A' => (
            9,
            &[&[
                (0, 1),
                (4, 7),
                (7, 10),
                (4, 10),
                (1, 7),
                (1, 2),
                (3, 0),
                (6, 3),
                (7, 10),
                (7, 2),
                (9, 0),
            ]],
        ),
        b'B' => (
            8,
            &[&[
                (0, 1),
                (3, 11),
                (4, 17),
                (3, 20),
                (1, 17),
                (1, 2),
                (3, 0),
                (6, 1),
                (7, 5),
                (5, 8),
                (2, 6),
                (8, 6),
            ]],
        ),
        b'C' => (
            8,
            &[&[
                (0, 1),
                (3, 8),
                (6, 10),
                (7, 8),
                (6, 10),
                (3, 10),
                (1, 7),
                (1, 2),
                (3, 0),
                (6, 1),
                (8, 2),
            ]],
        ),
        b'D' => (
            9,
            &[&[
                (0, 1),
                (4, 7),
                (7, 10),
                (4, 10),
                (1, 7),
                (1, 2),
                (3, 0),
                (6, 3),
                (7, 20),
                (7, 2),
                (9, 0),
            ]],
        ),
        b'E' => (
            8,
            &[&[
                (0, 1),
                (3, 3),
                (7, 6),
                (6, 9),
                (4, 10),
                (1, 7),
                (1, 2),
                (3, 0),
                (6, 1),
                (8, 2),
            ]],
        ),
        b'F' => (
            7,
            &[
                &[
                    (0, 1),
                    (3, 9),
                    (5, 16),
                    (4, 20),
                    (2, 17),
                    (2, 0),
                    (5, 1),
                    (7, 1),
                ],
                &[(0, 10), (6, 10)],
            ],
        ),
        b'G' => (
            9,
            &[&[
                (0, 1),
                (4, 7),
                (7, 10),
                (4, 10),
                (1, 7),
                (1, 3),
                (3, 1),
                (6, 3),
                (7, 10),
                (7, -5),
                (5, -8),
                (2, -7),
                (1, -5),
                (4, -2),
                (9, 0),
            ]],
        ),
        b'H' => (
            9,
            &[&[
                (0, 1),
                (3, 11),
                (4, 17),
                (3, 20),
                (1, 17),
                (1, 0),
                (1, 5),
                (4, 9),
                (6, 9),
                (7, 6),
                (7, 2),
                (9, 0),
            ]],
        ),
        b'I' => (
            5,
            &[
                &[(0, 1), (3, 10), (2, 2), (3, 0), (5, 1)],
                &[(3, 13), (3, 15)],
            ],
        ),
        b'J' => (
            6,
            &[
                &[(0, 1), (3, 10), (3, -5), (1, -8), (0, -6), (2, -2), (6, 1)],
                &[(3, 13), (3, 15)],
            ],
        ),
        b'K' => (
            9,
            &[&[
                (0, 1),
                (3, 11),
                (4, 17),
                (3, 20),
                (1, 17),
                (1, 0),
                (1, 4),
                (4, 8),
                (6, 9),
                (6, 7),
                (2, 5),
                (5, 4),
                (7, 1),
                (9, 0),
            ]],
        ),
        b'L' => (
            5,
            &[&[
                (0, 1),
                (3, 11),
                (4, 17),
                (3, 20),
                (1, 17),
                (1, 2),
                (3, 0),
                (5, 1),
            ]],
        ),
        b'M' => (
            13,
            &[&[
                (0, 1),
                (2, 6),
                (2, 10),
                (2, 0),
                (2, 7),
                (4, 10),
                (6, 9),
                (6, 0),
                (6, 7),
                (8, 10),
                (10, 9),
                (10, 2),
                (11, 0),
                (13, 1),
            ]],
        ),
        b'N' => (
            9,
            &[&[
                (0, 1),
                (2, 6),
                (2, 10),
                (2, 0),
                (2, 7),
                (4, 10),
                (6, 9),
                (6, 2),
                (7, 0),
                (9, 1),
            ]],
        ),
        b'O' => (
            8,
            &[&[
                (0, 1),
                (3, 7),
                (4, 10),
                (1, 7),
                (1, 2),
                (3, 0),
                (5, 0),
                (7, 3),
                (6, 8),
                (4, 10),
                (8, 8),
            ]],
        ),
        b'P' => (
            9,
            &[&[
                (0, 1),
                (2, 7),
                (2, 10),
                (2, -8),
                (2, 5),
                (4, 9),
                (6, 9),
                (8, 6),
                (8, 3),
                (6, 0),
                (3, 1),
                (9, 1),
            ]],
        ),
        b'Q' => (
            9,
            &[&[
                (0, 1),
                (4, 7),
                (7, 10),
                (4, 10),
                (1, 7),
                (1, 2),
                (3, 0),
                (6, 3),
                (7, 10),
                (7, -8),
                (7, 0),
                (9, 1),
            ]],
        ),
        b'R' => (
            8,
            &[&[
                (0, 1),
                (2, 6),
                (2, 10),
                (2, 0),
                (2, 6),
                (5, 9),
                (7, 9),
                (8, 8),
            ]],
        ),
        b'S' => (
            7,
            &[&[
                (0, 1),
                (4, 10),
                (6, 5),
                (6, 2),
                (4, 0),
                (2, 0),
                (1, 2),
                (4, 1),
                (7, 1),
            ]],
        ),
        b'T' => (
            7,
            &[
                &[(0, 1), (3, 9), (4, 16), (3, 2), (5, 0), (7, 1)],
                &[(0, 10), (7, 10)],
            ],
        ),
        b'U' => (
            9,
            &[&[
                (0, 1),
                (1, 7),
                (1, 10),
                (1, 3),
                (3, 0),
                (5, 1),
                (7, 5),
                (7, 10),
                (7, 2),
                (9, 0),
            ]],
        ),
        b'V' => (
            8,
            &[&[(0, 1), (1, 6), (2, 10), (4, 0), (6, 7), (6, 10), (8, 9)]],
        ),
        b'W' => (
            12,
            &[&[
                (0, 1),
                (1, 6),
                (1, 10),
                (3, 0),
                (6, 8),
                (9, 0),
                (10, 7),
                (10, 10),
                (12, 9),
            ]],
        ),
        b'X' => (8, &[&[(0, 1), (2, 10), (6, 0), (8, 1)], &[(6, 10), (2, 0)]]),
        b'Y' => (
            9,
            &[&[
                (0, 1),
                (1, 7),
                (1, 10),
                (1, 3),
                (3, 0),
                (5, 1),
                (7, 5),
                (7, 10),
                (7, -5),
                (5, -8),
                (2, -7),
                (1, -5),
                (4, -2),
                (9, 0),
            ]],
        ),
        b'Z' => (8, &[&[(0, 1), (1, 10), (7, 10), (1, 0), (6, 0), (8, 1)]]),
        _ => (-1, &[]),
    }
}

/// Laid-out lettering: polylines in em (cap height 1, `x` from the ink's left edge, `y` up
/// from the baseline) drawn `width` wide. A polyline whose last point repeats its first is
/// closed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strokes {
    pub points: Vec<[f32; 2]>,
    /// Where each polyline starts in `points`; the last ends at `points.len()`.
    pub starts: Vec<u32>,
    /// Stroke width, in the same units as the points.
    pub width: f32,
    /// Ink extent: `[x0, y0, x1, y1]`.
    pub bounds: [f32; 4],
}

impl Strokes {
    /// The polylines.
    pub fn lines(&self) -> impl Iterator<Item = &[[f32; 2]]> {
        let n = self.starts.len();
        (0..n).map(move |i| {
            let a = self.starts[i] as usize;
            let b = self
                .starts
                .get(i + 1)
                .map_or(self.points.len(), |&s| s as usize);
            &self.points[a..b]
        })
    }

    /// Segments over all polylines: what a builder spends two triangles a face on.
    pub fn segments(&self) -> usize {
        self.lines().map(|l| l.len().saturating_sub(1)).sum()
    }

    /// Ink width.
    pub fn ink_w(&self) -> f32 {
        self.bounds[2] - self.bounds[0]
    }

    /// Ink height.
    pub fn ink_h(&self) -> f32 {
        self.bounds[3] - self.bounds[1]
    }

    /// Maps every point through the affine `[a, b, c, d, e, f]`:
    /// `(x, y) -> (a x + b y + e, c x + d y + f)`; the stroke width scales by `scale`.
    pub fn transform(&mut self, t: [f32; 6], scale: f32) {
        for p in &mut self.points {
            let [x, y] = *p;
            *p = [t[0] * x + t[1] * y + t[4], t[2] * x + t[3] * y + t[5]];
        }
        let b = self.bounds;
        let corners = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
            .map(|[x, y]| [t[0] * x + t[1] * y + t[4], t[2] * x + t[3] * y + t[5]]);
        let mut nb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for [x, y] in corners {
            nb = [nb[0].min(x), nb[1].min(y), nb[2].max(x), nb[3].max(y)];
        }
        self.bounds = nb;
        self.width *= scale;
    }

    /// Scales by `s` and moves the ink's lower-left corner (the baseline for capitals) to
    /// `(x, y)`.
    pub fn place(&mut self, s: f32, x: f32, y: f32) {
        let [bx, _, _, _] = self.bounds;
        self.transform([s, 0.0, 0.0, s, x - bx * s, y], s);
    }

    /// Mirrors across the vertical line at `x = axis` (lettering seen from behind still
    /// reads left to right).
    pub fn mirror(&mut self, axis: f32) {
        self.transform([-1.0, 0.0, 0.0, 1.0, 2.0 * axis, 0.0], 1.0);
    }

    fn push(&mut self, line: &[[f32; 2]]) {
        if line.len() < 2 {
            return;
        }
        self.starts.push(self.points.len() as u32);
        self.points.extend_from_slice(line);
    }
}

/// Lays out `text` in `style`: cap height 1, ink from `x = 0`.
pub fn layout(text: &[u8], style: LetterStyle) -> Strokes {
    let m = metrics(style);
    let mut out = Strokes {
        width: m.stroke / 20.0,
        ..Strokes::default()
    };
    let mut pen = 0.0f32;
    let mut line: Vec<[f32; 2]> = Vec::new();
    // Script: the running tube of the current word's lowercase.
    let mut tube: Vec<[f32; 2]> = Vec::new();
    let mut word_start = true;
    let mut first = true;
    for &c in text {
        let c = c.to_ascii_uppercase();
        let lower = style == LetterStyle::Script && !word_start && c.is_ascii_alphabetic();
        let (w, strokes) = if lower { script(c) } else { block(c) };
        if w < 0 {
            // A space (or anything we do not letter) ends the word.
            out.push(&tube);
            tube.clear();
            pen += m.space - m.gap;
            word_start = true;
            continue;
        }
        if !first && !lower {
            pen += m.gap;
        }
        if c == b'\'' && style == LetterStyle::Script {
            out.push(&tube);
            tube.clear();
        }
        let narrow = if lower { 1.0 } else { m.narrow };
        for (k, s) in strokes.iter().enumerate() {
            line.clear();
            line.extend(
                s.iter()
                    .map(|&(x, y)| [pen + f32::from(x) * narrow, f32::from(y)]),
            );
            match style {
                LetterStyle::Script if !lower => round_corners(&mut line),
                _ => {}
            }
            if lower && k == 0 {
                // Run the tube on from the last letter's exit; from a high exit (o, v, w)
                // straight into the letter, skipping its entry.
                let high = tube.last().is_some_and(|p| p[1] > 3.0);
                tube.extend_from_slice(&line[usize::from(high)..]);
            } else {
                out.push(&line);
            }
            if style == LetterStyle::Slab && !lower {
                serifs(&mut out, &line, m.stroke);
            }
        }
        pen += f32::from(w) * narrow;
        if !lower && c.is_ascii_alphabetic() {
            word_start = false;
        }
        if lower {
            // Joined: the next letter's entry starts where this one exits.
            if let Some(&[ex, _]) = tube.last() {
                pen = pen.max(ex);
            }
        } else if style == LetterStyle::Script {
            pen += m.gap * 0.5;
        }
        first = false;
    }
    out.push(&tube);
    // Shear, then move into em with the ink's corner at the origin.
    let hw = m.stroke * 0.5;
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &mut out.points {
        p[0] += p[1] * m.slant;
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    if out.points.is_empty() {
        out.bounds = [0.0; 4];
        return out;
    }
    let s = 1.0 / (20.0 + m.stroke);
    for p in &mut out.points {
        *p = [(p[0] - x0 + hw) * s, (p[1] + hw) * s];
    }
    out.bounds = [0.0, y0 * s, (x1 - x0 + 2.0 * hw) * s, (y1 + 2.0 * hw) * s];
    out.width = m.stroke * s;
    out
}

/// Cuts every corner of `line` a quarter of the way along both sides: a script capital's
/// rounded strokes.
fn round_corners(line: &mut Vec<[f32; 2]>) {
    if line.len() < 3 {
        return;
    }
    let closed = line[0] == line[line.len() - 1];
    let n = line.len();
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let p = line[i];
        let (prev, next) = if i == 0 || i == n - 1 {
            if !closed {
                out.push(p);
                continue;
            }
            (line[n - 2], line[1])
        } else {
            (line[i - 1], line[i + 1])
        };
        let a = [p[0] - prev[0], p[1] - prev[1]];
        let b = [next[0] - p[0], next[1] - p[1]];
        let q0 = [p[0] - a[0] * 0.25, p[1] - a[1] * 0.25];
        let q1 = [p[0] + b[0] * 0.25, p[1] + b[1] * 0.25];
        if i == 0 {
            // A closed line starts and ends on the cut.
            out.push(q1);
        } else if i == n - 1 {
            out.push(q0);
            out.push(out[0]);
        } else {
            out.push(q0);
            out.push(q1);
        }
    }
    *line = out;
}

/// Slab serifs: a bar across each stroke end on the baseline or cap line that does not
/// already run along it, and a beak down (up) from each arm end on the cap line (baseline).
fn serifs(out: &mut Strokes, line: &[[f32; 2]], stroke: f32) {
    let n = line.len();
    if n < 2 || line[0] == line[n - 1] {
        return;
    }
    let w = stroke * 1.05;
    for (end, inner) in [(line[0], line[1]), (line[n - 1], line[n - 2])] {
        let on_line = end[1] == 0.0 || end[1] == 20.0;
        if !on_line {
            continue;
        }
        let flat = end[1] == inner[1];
        if flat {
            let dy = if end[1] == 0.0 { w * 1.3 } else { -w * 1.3 };
            out.push(&[end, [end[0], end[1] + dy]]);
        } else {
            out.push(&[[end[0] - w, end[1]], [end[0] + w, end[1]]]);
        }
    }
}

fn len(v: [f32; 2]) -> f32 {
    crate::math::sqrt(v[0] * v[0] + v[1] * v[1])
}

fn unit(v: [f32; 2]) -> [f32; 2] {
    let l = len(v).max(1e-9);
    [v[0] / l, v[1] / l]
}

/// The quads of a polyline drawn `w` wide: mitered joins (the miter capped at twice the
/// half width), square caps a half width past each end of an open line, and none on a
/// closed one. Each quad is `[a_left, b_left, b_right, a_right]`.
pub fn ribbon(line: &[[f32; 2]], w: f32, mut f: impl FnMut([[f32; 2]; 4])) {
    let n = line.len();
    if n < 2 {
        return;
    }
    let closed = n > 2 && line[0] == line[n - 1];
    let hw = w * 0.5;
    let dir = |i: usize| unit([line[i + 1][0] - line[i][0], line[i + 1][1] - line[i][1]]);
    let left = |d: [f32; 2]| [-d[1], d[0]];
    // The offset at vertex `i`: (left side, right side).
    let side = |i: usize| -> ([f32; 2], [f32; 2]) {
        let p = line[i];
        let (d0, d1) = if i == 0 {
            if closed {
                (dir(n - 2), dir(0))
            } else {
                let d = dir(0);
                let q = [p[0] - d[0] * hw, p[1] - d[1] * hw];
                let l = left(d);
                return (
                    [q[0] + l[0] * hw, q[1] + l[1] * hw],
                    [q[0] - l[0] * hw, q[1] - l[1] * hw],
                );
            }
        } else if i == n - 1 {
            if closed {
                (dir(n - 2), dir(0))
            } else {
                let d = dir(n - 2);
                let q = [p[0] + d[0] * hw, p[1] + d[1] * hw];
                let l = left(d);
                return (
                    [q[0] + l[0] * hw, q[1] + l[1] * hw],
                    [q[0] - l[0] * hw, q[1] - l[1] * hw],
                );
            }
        } else {
            (dir(i - 1), dir(i))
        };
        let (l0, l1) = (left(d0), left(d1));
        let m = [l0[0] + l1[0], l0[1] + l1[1]];
        let ml = len(m);
        let (m, k) = if ml < 1e-3 {
            // Doubling back: square across.
            (l0, hw)
        } else {
            let m = [m[0] / ml, m[1] / ml];
            let c = (m[0] * l0[0] + m[1] * l0[1]).max(0.5);
            (m, hw / c)
        };
        (
            [p[0] + m[0] * k, p[1] + m[1] * k],
            [p[0] - m[0] * k, p[1] - m[1] * k],
        )
    };
    let mut a = side(0);
    for i in 1..n {
        let b = side(i);
        f([a.0, b.0, b.1, a.1]);
        a = b;
    }
}

/// A ribbon quad: `[a_left, b_left, b_right, a_right]`.
type Quad = [[f32; 2]; 4];

/// How lettering is built.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Finish {
    /// Glowing tubes standing a little off the face.
    Neon = 0,
    /// Dark strokes on a lit panel (the panel is the caller's).
    Backlit = 1,
    /// Strokes extruded off the face.
    Channel = 2,
    /// Flat paint on the face.
    Painted = 3,
}

/// How far each finish stands off the face it is drawn on, m.
pub fn standoff(finish: Finish) -> f32 {
    match finish {
        Finish::Neon => 0.02,
        Finish::Backlit => 0.004,
        Finish::Channel => 0.0,
        Finish::Painted => 0.002,
    }
}

/// Builds `s` (points already in the frame's XY plane, meters) on the face at depth `z`,
/// facing +Z (or -Z if `back`): `paint` is the tube, the ink or the letters' faces. Channel
/// letters are extruded `depth` off the face with their sides in `side`. Returns the
/// triangles added.
#[allow(clippy::too_many_arguments)]
pub fn build(
    m: &mut WorldMesh,
    f: &Frame,
    s: &Strokes,
    finish: Finish,
    z: f32,
    back: bool,
    paint: Paint,
    side: Paint,
    depth: f32,
) -> usize {
    let before = m.triangle_count();
    let dz = if back { -1.0 } else { 1.0 };
    let face = z + dz * standoff(finish);
    let facing = if back { -f.z } else { f.z };
    let at = |p: [f32; 2], z: f32| f.p(p[0], p[1], z);
    let top = face
        + if finish == Finish::Channel {
            dz * depth
        } else {
            0.0
        };
    // Outward in the face's plane from the edge `a -> b` of a ribbon quad's outline.
    let out = |a: [f32; 2], b: [f32; 2]| f.v(a[1] - b[1], b[0] - a[0], 0.0);
    for line in s.lines() {
        let mut ends: Option<(Quad, Quad)> = None;
        ribbon(line, s.width, |q| {
            quad(m, q.map(|p| at(p, top)), facing, paint);
            if finish == Finish::Channel {
                for (a, b) in [(q[0], q[1]), (q[2], q[3])] {
                    quad(
                        m,
                        [at(a, face), at(b, face), at(b, top), at(a, top)],
                        out(a, b),
                        side,
                    );
                }
                ends = Some((ends.map_or(q, |e| e.0), q));
            }
        });
        if let Some((first, last)) = ends {
            if line[0] != line[line.len() - 1] {
                for (a, b) in [(first[3], first[0]), (last[1], last[2])] {
                    quad(
                        m,
                        [at(a, face), at(b, face), at(b, top), at(a, top)],
                        out(a, b),
                        side,
                    );
                }
            }
        }
    }
    m.triangle_count() - before
}

/// Draws `s` (points in the frame's XY plane, meters) flat on the plane at depth `z`,
/// facing +Z (or -Z if `back`).
pub fn flat(m: &mut WorldMesh, f: &Frame, s: &Strokes, z: f32, back: bool, paint: Paint) {
    let facing = if back { -f.z } else { f.z };
    for line in s.lines() {
        ribbon(line, s.width, |q| {
            quad(m, q.map(|p| f.p(p[0], p[1], z)), facing, paint);
        });
    }
}

/// Triangles [`build`] spends on `s` on one face.
pub fn triangles(s: &Strokes, finish: Finish) -> usize {
    match finish {
        Finish::Channel => {
            let ends: usize = s
                .lines()
                .filter(|l| l.len() > 1 && l[0] != l[l.len() - 1])
                .count();
            s.segments() * 6 + ends * 4
        }
        _ => s.segments() * 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::V3;
    use crate::mesh::Pattern;

    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'";

    #[test]
    fn every_character_has_a_glyph_in_every_style() {
        for &c in ALPHABET {
            assert!(block(c).0 >= 0, "{}", c as char);
            if c.is_ascii_alphabetic() {
                assert!(script(c).0 > 0, "{}", c as char);
            }
            for style in LetterStyle::ALL {
                let s = layout(&[b'X', c], style);
                assert!(s.segments() > 1, "{style:?} {}", c as char);
                assert!(s
                    .points
                    .iter()
                    .all(|p| p[0].is_finite() && p[1].is_finite()));
            }
        }
    }

    #[test]
    fn capitals_stand_one_em_and_space_out() {
        for style in LetterStyle::ALL {
            let s = layout(b"HELLO", style);
            assert!(s.bounds[0] == 0.0, "{style:?} {:?}", s.bounds);
            // Ink from the baseline to the cap line (script lowercase is shorter).
            assert!((s.bounds[1]).abs() < 1e-4, "{style:?} {:?}", s.bounds);
            assert!((s.bounds[3] - 1.0).abs() < 0.08, "{style:?} {:?}", s.bounds);
            // Points stay inside the ink, inset by half a stroke.
            for p in &s.points {
                assert!(
                    p[0] >= s.bounds[0] + s.width * 0.49 - 1e-4,
                    "{style:?} {p:?}"
                );
                assert!(
                    p[0] <= s.bounds[2] - s.width * 0.49 + 1e-4,
                    "{style:?} {p:?}"
                );
            }
            let two = layout(b"HE", style).ink_w();
            assert!(s.ink_w() > two * 1.6, "{style:?}");
        }
        // Condensed is narrower than block; a space is wider than the gap.
        let (b, c) = (
            layout(b"MANOR", LetterStyle::Block),
            layout(b"MANOR", LetterStyle::Condensed),
        );
        assert!(c.ink_w() < b.ink_w() * 0.8, "{} {}", c.ink_w(), b.ink_w());
        assert!(
            layout(b"A B", LetterStyle::Block).ink_w()
                > layout(b"AB", LetterStyle::Block).ink_w() + 0.2
        );
    }

    #[test]
    fn script_runs_one_tube_per_word() {
        let s = layout(b"DALVEY'S", LetterStyle::Script);
        // The capital, the tube of "alvey", the apostrophe, the tube of "s".
        assert_eq!(s.lines().count(), 4, "{:?}", s.starts);
        let s = layout(b"KESSICK", LetterStyle::Script);
        // The K's three strokes, the tube, and the dot of the i.
        assert_eq!(s.lines().count(), 5, "{:?}", s.starts);
        assert!(s.bounds[1] < 0.0 || layout(b"AG", LetterStyle::Script).bounds[1] < 0.0);
    }

    #[test]
    fn slab_adds_serifs() {
        let block = layout(b"HIT", LetterStyle::Block).lines().count();
        let slab = layout(b"HIT", LetterStyle::Slab).lines().count();
        // H: four, I: two, T: a foot, a bar where the stem meets the top and two beaks.
        assert_eq!(slab, block + 10);
    }

    #[test]
    fn ribbons_are_well_formed_and_builders_count_their_triangles() {
        let ink = Paint::new([200, 40, 90], Pattern::Plain);
        for style in LetterStyle::ALL {
            let mut s = layout(b"OPEN SINCE 1962", style);
            s.place(0.3, 1.0, 2.0);
            for finish in [
                Finish::Neon,
                Finish::Backlit,
                Finish::Channel,
                Finish::Painted,
            ] {
                let mut m = WorldMesh::default();
                let f = Frame::at(V3::new(0.0, 0.0, 0.0));
                let n = build(&mut m, &f, &s, finish, 0.1, false, ink, ink, 0.05);
                assert_eq!(n, m.triangle_count());
                // Degenerate quads are skipped, so this is an upper bound.
                assert!(n <= triangles(&s, finish), "{style:?} {finish:?}");
                assert!(n * 10 >= triangles(&s, finish) * 9, "{style:?} {finish:?}");
                for v in &m.vertices {
                    assert!(v.pos.iter().all(|c| c.is_finite()));
                }
            }
        }
        // A closed square: four quads, the miter corners on the offset square.
        let sq = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]];
        let mut quads = Vec::new();
        ribbon(&sq, 0.2, |q| quads.push(q));
        assert_eq!(quads.len(), 4);
        let c = quads[0][0];
        assert!(
            (c[0] - 0.1).abs() < 1e-5 && (c[1] - 0.1).abs() < 1e-5,
            "{c:?}"
        );
    }

    #[test]
    fn layout_is_deterministic() {
        for style in LetterStyle::ALL {
            let a = layout(b"SUNSET TACOS", style);
            let b = layout(b"SUNSET TACOS", style);
            let bits = |s: &Strokes| -> Vec<u32> {
                s.points
                    .iter()
                    .flat_map(|p| [p[0].to_bits(), p[1].to_bits()])
                    .collect()
            };
            assert_eq!(bits(&a), bits(&b));
        }
    }
}
