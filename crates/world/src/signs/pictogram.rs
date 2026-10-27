//! Pictograms: generic objects as flat 2D polygons for shop signs, never a mark (no red
//! cross on white, nothing resembling a real logo). Each is one or more *solid* simple
//! polygons, extruded a few centimetres and outlined in neon, and flat *overlays* drawn on
//! the solids' faces (a donut's frosting and hole, a record's label).
//!
//! Shapes are drawn on a 20-unit grid, y up, and handed out in em: centered on the
//! origin, one em across.

use alloc::vec::Vec;

use crate::math::{sin_cos, PI, V2};
use crate::mesh::{quad, tri, triangulate, Frame, Paint, WorldMesh};
use crate::signs::letters::ribbon;

/// The pictograms, in the order the grammar files and the preview sheet list them.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Pictogram {
    Donut = 0,
    CoffeeCup = 1,
    Burger = 2,
    Taco = 3,
    PizzaSlice = 4,
    IceCreamCone = 5,
    HotDog = 6,
    Fish = 7,
    MartiniGlass = 8,
    BeerMug = 9,
    Surfboard = 10,
    Wave = 11,
    Palm = 12,
    Sun = 13,
    Star = 14,
    Arrow = 15,
    Record = 16,
    TvSet = 17,
    Camera = 18,
    FilmReel = 19,
    ScissorsComb = 20,
    Shoe = 21,
    Key = 22,
    Wrench = 23,
    Tire = 24,
    Tooth = 25,
    MortarPestle = 26,
    ShirtHanger = 27,
    Book = 28,
    Bicycle = 29,
    BowlingPin = 30,
    Flower = 31,
}

impl Pictogram {
    pub const ALL: [Pictogram; 32] = [
        Pictogram::Donut,
        Pictogram::CoffeeCup,
        Pictogram::Burger,
        Pictogram::Taco,
        Pictogram::PizzaSlice,
        Pictogram::IceCreamCone,
        Pictogram::HotDog,
        Pictogram::Fish,
        Pictogram::MartiniGlass,
        Pictogram::BeerMug,
        Pictogram::Surfboard,
        Pictogram::Wave,
        Pictogram::Palm,
        Pictogram::Sun,
        Pictogram::Star,
        Pictogram::Arrow,
        Pictogram::Record,
        Pictogram::TvSet,
        Pictogram::Camera,
        Pictogram::FilmReel,
        Pictogram::ScissorsComb,
        Pictogram::Shoe,
        Pictogram::Key,
        Pictogram::Wrench,
        Pictogram::Tire,
        Pictogram::Tooth,
        Pictogram::MortarPestle,
        Pictogram::ShirtHanger,
        Pictogram::Book,
        Pictogram::Bicycle,
        Pictogram::BowlingPin,
        Pictogram::Flower,
    ];
}

/// Which of a sign's colors a part takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    /// The pictogram's main color.
    Body = 0,
    /// Its second color.
    Accent = 1,
    /// Near black: holes, lenses, a record, a tire.
    Dark = 2,
    /// Near white: foam, a screen's glare, a tooth.
    Light = 3,
}

/// One polygon of a pictogram, counter-clockwise, in em.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub tint: Tint,
    /// Extruded and outlined; otherwise a flat overlay on a solid's face.
    pub solid: bool,
    pub poly: Vec<[f32; 2]>,
}

/// Most outline vertices of a pictogram's solids together (each is a neon segment).
pub const MAX_OUTLINE: usize = 26;
/// Most triangles [`extrude`] spends on a pictogram, both faces.
pub const MAX_TRIANGLES: usize = 170;

/// Builds the parts of `p`.
struct Shape(Vec<Part>);

impl Shape {
    fn solid(&mut self, tint: Tint, pts: Vec<[f32; 2]>) -> &mut Self {
        self.add(tint, true, pts)
    }
    fn over(&mut self, tint: Tint, pts: Vec<[f32; 2]>) -> &mut Self {
        self.add(tint, false, pts)
    }
    fn add(&mut self, tint: Tint, solid: bool, mut poly: Vec<[f32; 2]>) -> &mut Self {
        if area(&poly) < 0.0 {
            poly.reverse();
        }
        for p in &mut poly {
            *p = [p[0] / 20.0 - 0.5, p[1] / 20.0 - 0.5];
        }
        self.0.push(Part { tint, solid, poly });
        self
    }
}

fn pts(p: &[(f32, f32)]) -> Vec<[f32; 2]> {
    p.iter().map(|&(x, y)| [x, y]).collect()
}

/// An `n`-gon of radius `r` about `(cx, cy)`, first vertex at angle `phase`.
fn circle(cx: f32, cy: f32, r: f32, n: u32, phase: f32) -> Vec<[f32; 2]> {
    ellipse(cx, cy, r, r, phase, n)
}

/// An `n`-gon on the ellipse with radii `rx, ry` rotated by `rot`.
fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32, rot: f32, n: u32) -> Vec<[f32; 2]> {
    let (rs, rc) = sin_cos(rot);
    (0..n)
        .map(|i| {
            let (s, c) = sin_cos(2.0 * PI * i as f32 / n as f32);
            let (x, y) = (c * rx, s * ry);
            [cx + x * rc - y * rs, cy + x * rs + y * rc]
        })
        .collect()
}

/// A star of `points` points between radii `r0` (tips) and `r1`.
fn star(cx: f32, cy: f32, r0: f32, r1: f32, points: u32, phase: f32) -> Vec<[f32; 2]> {
    (0..points * 2)
        .map(|i| {
            let r = if i % 2 == 0 { r0 } else { r1 };
            let (s, c) = sin_cos(phase + PI * i as f32 / points as f32);
            [cx + c * r, cy + s * r]
        })
        .collect()
}

/// The outline of the polyline `path` drawn `w` wide (square ends, mitered bends).
fn thick(path: &[[f32; 2]], w: f32) -> Vec<[f32; 2]> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    ribbon(path, w, |q| {
        if left.is_empty() {
            left.push(q[0]);
            right.push(q[3]);
        }
        left.push(q[1]);
        right.push(q[2]);
    });
    right.reverse();
    left.extend(right);
    left
}

/// `p` turned by `a` about `(cx, cy)`.
fn turned(mut p: Vec<[f32; 2]>, cx: f32, cy: f32, a: f32) -> Vec<[f32; 2]> {
    let (s, c) = sin_cos(a);
    for q in &mut p {
        let (x, y) = (q[0] - cx, q[1] - cy);
        *q = [cx + x * c - y * s, cy + x * s + y * c];
    }
    p
}

/// Shoelace area, positive counter-clockwise (y up).
pub fn area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// The parts of `p`: solids first, then overlays, each overlay inside a solid.
pub fn parts(p: Pictogram) -> Vec<Part> {
    use Tint::*;
    let mut s = Shape(Vec::new());
    match p {
        Pictogram::Donut => {
            // A ring cake, frosted, the hole dark.
            s.solid(Body, circle(10.0, 10.0, 9.0, 12, 0.0))
                .over(Accent, circle(10.0, 10.0, 7.3, 12, 0.26))
                .over(Dark, circle(10.0, 10.0, 2.6, 8, 0.0));
        }
        Pictogram::CoffeeCup => {
            s.solid(
                Body,
                pts(&[
                    (2.0, 4.0),
                    (14.0, 4.0),
                    (14.6, 7.0),
                    (17.0, 7.2),
                    (18.8, 9.0),
                    (18.8, 12.0),
                    (17.0, 14.0),
                    (15.3, 14.0),
                    (15.5, 16.0),
                    (2.5, 16.0),
                    (3.4, 6.0),
                    (1.0, 5.0),
                ]),
            )
            .over(
                Dark,
                pts(&[(15.0, 9.0), (16.7, 9.4), (17.0, 11.4), (15.2, 12.2)]),
            )
            .over(
                Light,
                pts(&[(3.2, 13.5), (15.2, 13.5), (15.4, 15.4), (2.9, 15.4)]),
            );
        }
        Pictogram::Burger => {
            s.solid(
                Body,
                pts(&[
                    (2.0, 5.0),
                    (4.0, 3.0),
                    (16.0, 3.0),
                    (18.0, 5.0),
                    (18.0, 7.5),
                    (19.0, 8.5),
                    (18.0, 10.0),
                    (18.0, 11.0),
                    (16.0, 15.0),
                    (12.0, 17.0),
                    (8.0, 17.0),
                    (4.0, 15.0),
                    (2.0, 11.0),
                    (2.0, 10.0),
                    (1.0, 8.5),
                    (2.0, 7.5),
                ]),
            )
            .over(
                Dark,
                pts(&[(2.6, 6.2), (17.4, 6.2), (17.4, 8.0), (2.6, 8.0)]),
            )
            .over(
                Accent,
                pts(&[(2.4, 8.6), (17.6, 8.6), (17.6, 9.6), (2.4, 9.6)]),
            );
        }
        Pictogram::Taco => {
            // A folded shell, seen from the side, filling along the top.
            let mut shell = Vec::new();
            for k in 0..=6 {
                let (sn, c) = sin_cos(PI + PI * k as f32 / 6.0);
                shell.push([10.0 + 9.0 * c, 13.0 + 9.5 * sn]);
            }
            let mut v = shell.clone();
            v.extend_from_slice(&[
                [17.0, 15.0],
                [14.0, 13.8],
                [11.0, 15.6],
                [8.0, 13.8],
                [5.0, 15.6],
                [2.5, 14.0],
            ]);
            s.solid(Body, v).over(
                Accent,
                pts(&[
                    (2.4, 12.6),
                    (17.6, 12.6),
                    (16.6, 14.4),
                    (14.0, 13.3),
                    (11.0, 14.9),
                    (8.0, 13.3),
                    (5.0, 14.9),
                    (3.0, 13.6),
                ]),
            );
        }
        Pictogram::PizzaSlice => {
            s.solid(
                Body,
                pts(&[
                    (10.0, 1.0),
                    (17.5, 15.0),
                    (18.0, 17.0),
                    (14.0, 18.6),
                    (10.0, 19.0),
                    (6.0, 18.6),
                    (2.0, 17.0),
                    (2.5, 15.0),
                ]),
            )
            .over(
                Accent,
                pts(&[
                    (2.9, 15.5),
                    (17.1, 15.5),
                    (17.4, 16.8),
                    (10.0, 18.2),
                    (2.6, 16.8),
                ]),
            )
            .over(Dark, circle(10.0, 11.0, 1.6, 6, 0.0))
            .over(Dark, circle(7.6, 6.0 + 7.0, 1.3, 6, 0.4))
            .over(Dark, circle(11.0, 5.5, 1.2, 6, 0.2));
        }
        Pictogram::IceCreamCone => {
            s.solid(
                Body,
                pts(&[
                    (10.0, 1.0),
                    (14.0, 10.0),
                    (16.0, 11.0),
                    (17.0, 13.0),
                    (16.0, 16.0),
                    (13.0, 18.5),
                    (10.0, 19.0),
                    (7.0, 18.5),
                    (4.0, 16.0),
                    (3.0, 13.0),
                    (4.0, 11.0),
                    (6.0, 10.0),
                ]),
            )
            .over(Accent, pts(&[(10.0, 1.4), (13.7, 9.8), (6.3, 9.8)]));
        }
        Pictogram::HotDog => {
            s.solid(
                Body,
                pts(&[
                    (1.0, 10.0),
                    (2.0, 8.5),
                    (3.5, 8.0),
                    (4.0, 6.0),
                    (16.0, 6.0),
                    (16.5, 8.0),
                    (18.0, 8.5),
                    (19.0, 10.0),
                    (18.0, 11.5),
                    (16.5, 12.0),
                    (16.0, 14.0),
                    (4.0, 14.0),
                    (3.5, 12.0),
                    (2.0, 11.5),
                ]),
            )
            .over(
                Accent,
                pts(&[
                    (1.5, 10.0),
                    (2.3, 9.0),
                    (17.7, 9.0),
                    (18.5, 10.0),
                    (17.7, 11.0),
                    (2.3, 11.0),
                ]),
            );
        }
        Pictogram::Fish => {
            s.solid(
                Body,
                pts(&[
                    (1.0, 10.0),
                    (4.0, 6.0),
                    (8.0, 4.0),
                    (12.0, 4.5),
                    (15.0, 7.0),
                    (17.0, 4.5),
                    (19.0, 3.0),
                    (18.0, 10.0),
                    (19.0, 17.0),
                    (17.0, 15.5),
                    (15.0, 13.0),
                    (12.0, 15.5),
                    (8.0, 16.0),
                    (4.0, 14.0),
                ]),
            )
            .over(Dark, circle(5.0, 11.0, 1.1, 6, 0.0))
            .over(
                Accent,
                pts(&[(9.0, 6.0), (11.0, 6.0), (11.0, 14.0), (9.0, 14.0)]),
            );
        }
        Pictogram::MartiniGlass => {
            s.solid(
                Body,
                pts(&[
                    (1.0, 18.0),
                    (9.0, 9.0),
                    (9.0, 4.0),
                    (5.0, 3.0),
                    (5.0, 2.0),
                    (15.0, 2.0),
                    (15.0, 3.0),
                    (11.0, 4.0),
                    (11.0, 9.0),
                    (19.0, 18.0),
                ]),
            )
            .over(Light, pts(&[(3.6, 16.6), (16.4, 16.6), (10.0, 9.8)]))
            .over(Accent, circle(11.5, 13.5, 1.5, 6, 0.0));
        }
        Pictogram::BeerMug => {
            s.solid(
                Body,
                pts(&[
                    (3.0, 3.0),
                    (14.0, 3.0),
                    (14.0, 6.0),
                    (17.0, 6.0),
                    (18.0, 8.0),
                    (18.0, 13.0),
                    (17.0, 15.0),
                    (14.0, 15.0),
                    (14.0, 17.0),
                    (12.0, 19.0),
                    (9.0, 18.0),
                    (6.0, 19.0),
                    (3.0, 17.0),
                ]),
            )
            .over(
                Light,
                pts(&[
                    (3.0, 15.0),
                    (14.0, 15.0),
                    (14.0, 17.0),
                    (12.0, 19.0),
                    (9.0, 18.0),
                    (6.0, 19.0),
                    (3.0, 17.0),
                ]),
            )
            .over(
                Dark,
                pts(&[(14.0, 8.0), (16.0, 8.0), (16.0, 13.0), (14.0, 13.0)]),
            );
        }
        Pictogram::Surfboard => {
            let board = ellipse(10.0, 10.0, 3.2, 9.6, -0.5, 14);
            let stripe = ellipse(10.0, 10.0, 0.7, 8.6, -0.5, 6);
            s.solid(Body, board).over(Accent, stripe);
        }
        Pictogram::Wave => {
            s.solid(
                Body,
                pts(&[
                    (1.0, 3.0),
                    (19.0, 3.0),
                    (19.0, 5.0),
                    (16.0, 6.0),
                    (13.0, 9.0),
                    (12.0, 13.0),
                    (13.0, 16.0),
                    (15.5, 17.0),
                    (12.0, 18.5),
                    (8.0, 17.5),
                    (5.0, 14.5),
                    (3.0, 10.0),
                    (1.5, 6.0),
                ]),
            )
            .over(
                Light,
                pts(&[
                    (12.0, 13.0),
                    (13.0, 16.0),
                    (15.0, 16.9),
                    (12.0, 17.8),
                    (10.0, 17.0),
                    (11.0, 15.0),
                ]),
            );
        }
        Pictogram::Palm => {
            // A leaning trunk under five drooping fronds.
            s.solid(
                Body,
                pts(&[
                    (8.0, 1.0),
                    (11.0, 1.0),
                    (12.6, 7.0),
                    (12.8, 11.6),
                    (19.0, 8.5),
                    (14.6, 12.8),
                    (17.5, 16.8),
                    (13.0, 14.6),
                    (11.5, 19.2),
                    (11.0, 14.8),
                    (4.0, 16.5),
                    (9.6, 13.4),
                    (2.0, 9.5),
                    (11.2, 11.8),
                    (10.5, 7.0),
                ]),
            )
            .over(
                Accent,
                pts(&[
                    (8.4, 1.4),
                    (10.7, 1.4),
                    (12.2, 7.0),
                    (12.4, 11.3),
                    (11.5, 11.4),
                    (10.9, 7.0),
                ]),
            );
        }
        Pictogram::Sun => {
            s.solid(Body, star(10.0, 10.0, 9.6, 6.6, 8, 0.0))
                .over(Accent, circle(10.0, 10.0, 5.0, 10, 0.0));
        }
        Pictogram::Star => {
            s.solid(Body, star(10.0, 10.6, 9.4, 4.0, 5, PI * 0.5))
                .over(Light, star(10.0, 10.6, 4.8, 2.1, 5, PI * 0.5));
        }
        Pictogram::Arrow => {
            // The motel arrow: a bent shaft, the head pointing down at the door.
            s.solid(
                Body,
                pts(&[
                    (5.5, 1.0),
                    (10.0, 8.0),
                    (7.0, 8.0),
                    (7.0, 15.0),
                    (19.0, 15.0),
                    (19.0, 18.0),
                    (4.0, 18.0),
                    (4.0, 8.0),
                    (1.0, 8.0),
                ]),
            );
            for k in 0..4 {
                let x = 9.0 + 3.0 * k as f32;
                s.over(Light, circle(x, 16.5, 0.7, 4, 0.0));
            }
        }
        Pictogram::Record => {
            s.solid(Dark, circle(10.0, 10.0, 9.6, 14, 0.0))
                .over(Accent, circle(10.0, 10.0, 3.6, 8, 0.0))
                .over(Light, circle(10.0, 10.0, 0.7, 4, 0.0));
        }
        Pictogram::TvSet => {
            s.solid(
                Body,
                pts(&[
                    (2.0, 3.0),
                    (4.0, 3.0),
                    (4.0, 1.5),
                    (6.0, 1.5),
                    (6.0, 3.0),
                    (14.0, 3.0),
                    (14.0, 1.5),
                    (16.0, 1.5),
                    (16.0, 3.0),
                    (18.0, 3.0),
                    (18.0, 14.0),
                    (11.0, 14.0),
                    (15.0, 19.0),
                    (14.0, 19.5),
                    (10.0, 15.0),
                    (6.0, 19.5),
                    (5.0, 19.0),
                    (9.0, 14.0),
                    (2.0, 14.0),
                ]),
            )
            .over(
                Light,
                pts(&[(3.5, 4.8), (13.0, 4.8), (13.0, 12.5), (3.5, 12.5)]),
            )
            .over(Dark, circle(15.5, 10.0, 0.9, 4, 0.0))
            .over(Dark, circle(15.5, 6.8, 0.9, 4, 0.0));
        }
        Pictogram::Camera => {
            s.solid(
                Body,
                pts(&[
                    (1.0, 4.0),
                    (19.0, 4.0),
                    (19.0, 14.0),
                    (14.0, 14.0),
                    (13.0, 16.5),
                    (7.0, 16.5),
                    (6.0, 14.0),
                    (4.5, 14.0),
                    (4.5, 15.2),
                    (2.0, 15.2),
                    (2.0, 14.0),
                    (1.0, 14.0),
                ]),
            )
            .over(Dark, circle(10.0, 9.0, 4.0, 10, 0.0))
            .over(Accent, circle(10.0, 9.0, 2.2, 8, 0.0))
            .over(
                Light,
                pts(&[(15.0, 11.0), (17.5, 11.0), (17.5, 12.6), (15.0, 12.6)]),
            );
        }
        Pictogram::FilmReel => {
            s.solid(Body, circle(10.0, 10.0, 9.6, 14, 0.0));
            for k in 0..5 {
                let (sn, c) = sin_cos(PI * 0.5 + 2.0 * PI * k as f32 / 5.0);
                s.over(Dark, circle(10.0 + 5.2 * c, 10.0 + 5.2 * sn, 2.1, 6, 0.0));
            }
            s.over(Dark, circle(10.0, 10.0, 1.0, 4, 0.0));
        }
        Pictogram::ScissorsComb => {
            // Open shears over a comb: a blade pair with ring handles, the comb's teeth
            // below.
            s.solid(
                Body,
                pts(&[
                    (2.0, 19.0),
                    (9.0, 12.5),
                    (8.0, 9.5),
                    (5.5, 9.0),
                    (4.5, 6.8),
                    (6.0, 5.0),
                    (8.4, 5.2),
                    (9.6, 7.8),
                    (10.0, 10.0),
                    (10.4, 7.8),
                    (11.6, 5.2),
                    (14.0, 5.0),
                    (15.5, 6.8),
                    (14.5, 9.0),
                    (12.0, 9.5),
                    (11.0, 12.5),
                    (18.0, 19.0),
                    (10.0, 14.0),
                ]),
            )
            .over(Dark, circle(6.8, 7.1, 1.1, 6, 0.0))
            .over(Dark, circle(13.2, 7.1, 1.1, 6, 0.0));
            s.solid(
                Accent,
                pts(&[(2.0, 0.6), (18.0, 0.6), (18.0, 3.6), (2.0, 3.6)]),
            );
            for k in 0..5 {
                let x = 4.0 + 2.8 * k as f32;
                s.over(
                    Dark,
                    pts(&[(x, 0.6), (x + 1.0, 0.6), (x + 1.0, 2.4), (x, 2.4)]),
                );
            }
        }
        Pictogram::Shoe => {
            s.solid(
                Body,
                pts(&[
                    (1.0, 3.0),
                    (19.0, 3.0),
                    (19.0, 6.0),
                    (17.0, 8.0),
                    (12.0, 9.0),
                    (9.0, 12.0),
                    (9.0, 17.0),
                    (3.0, 17.0),
                    (3.0, 8.0),
                    (1.0, 6.0),
                ]),
            )
            .over(
                Dark,
                pts(&[(1.4, 3.4), (18.6, 3.4), (18.6, 4.6), (1.4, 4.6)]),
            )
            .over(
                Accent,
                pts(&[(3.4, 14.6), (8.6, 14.6), (8.6, 16.6), (3.4, 16.6)]),
            );
        }
        Pictogram::Key => {
            let mut v = Vec::new();
            // The bow, from its top round the left to its bottom, then the shaft and bit.
            for k in 0..=7 {
                let (sn, c) = sin_cos(PI * 0.2 + 1.6 * PI * k as f32 / 7.0);
                v.push([5.5 + 4.5 * c, 10.0 + 4.5 * sn]);
            }
            v.extend_from_slice(&[
                [11.0, 8.8],
                [14.0, 8.8],
                [14.0, 6.5],
                [15.6, 6.5],
                [15.6, 8.8],
                [17.0, 8.8],
                [17.0, 6.0],
                [19.0, 6.0],
                [19.0, 11.2],
                [11.0, 11.2],
            ]);
            s.solid(Body, v).over(Dark, circle(4.8, 10.0, 1.8, 6, 0.0));
        }
        Pictogram::Wrench => {
            let head = pts(&[
                (-8.5, -1.3),
                (3.2, -1.3),
                (4.5, -3.2),
                (7.0, -3.6),
                (9.0, -2.4),
                (9.6, -1.2),
                (6.5, -1.2),
                (6.5, 1.2),
                (9.6, 1.2),
                (9.0, 2.4),
                (7.0, 3.6),
                (4.5, 3.2),
                (3.2, 1.3),
                (-8.5, 1.3),
                (-9.3, 0.0),
            ]);
            let v = head.iter().map(|p| [p[0] + 10.0, p[1] + 10.0]).collect();
            s.solid(Body, turned(v, 10.0, 10.0, PI * 0.25));
        }
        Pictogram::Tire => {
            // A silver wheel with five lug nuts in a black tire.
            s.solid(Dark, circle(10.0, 10.0, 9.6, 14, 0.0))
                .over(Light, circle(10.0, 10.0, 5.8, 10, 0.0));
            for k in 0..5 {
                let (sn, c) = sin_cos(PI * 0.5 + 2.0 * PI * k as f32 / 5.0);
                s.over(Dark, circle(10.0 + 3.2 * c, 10.0 + 3.2 * sn, 0.8, 4, 0.0));
            }
        }
        Pictogram::Tooth => {
            s.solid(
                Light,
                pts(&[
                    (4.0, 19.0),
                    (2.0, 16.0),
                    (3.0, 11.0),
                    (5.0, 6.0),
                    (6.0, 1.0),
                    (8.0, 1.0),
                    (9.0, 6.0),
                    (10.0, 8.0),
                    (11.0, 6.0),
                    (12.0, 1.0),
                    (14.0, 1.0),
                    (15.0, 6.0),
                    (17.0, 11.0),
                    (18.0, 16.0),
                    (16.0, 19.0),
                    (12.0, 19.5),
                    (10.0, 18.0),
                    (8.0, 19.5),
                ]),
            );
        }
        Pictogram::MortarPestle => {
            s.solid(
                Body,
                pts(&[
                    (2.0, 11.0),
                    (3.0, 7.0),
                    (6.0, 4.0),
                    (6.0, 2.0),
                    (14.0, 2.0),
                    (14.0, 4.0),
                    (17.0, 7.0),
                    (18.0, 11.0),
                ]),
            )
            .over(
                Accent,
                pts(&[(2.6, 9.0), (17.4, 9.0), (17.6, 10.4), (2.4, 10.4)]),
            );
            s.solid(
                Accent,
                pts(&[
                    (10.5, 11.5),
                    (12.0, 11.0),
                    (17.0, 18.0),
                    (16.6, 19.4),
                    (15.0, 19.0),
                ]),
            );
        }
        Pictogram::ShirtHanger => {
            s.solid(
                Accent,
                pts(&[
                    (2.0, 11.6),
                    (10.0, 14.2),
                    (18.0, 11.6),
                    (18.0, 12.6),
                    (10.6, 15.2),
                    (11.2, 17.6),
                    (12.0, 18.5),
                    (10.5, 19.2),
                    (9.4, 17.5),
                    (9.4, 15.2),
                    (2.0, 12.6),
                ]),
            );
            s.solid(
                Body,
                pts(&[
                    (1.0, 7.0),
                    (4.0, 11.6),
                    (8.0, 12.3),
                    (10.0, 11.0),
                    (12.0, 12.3),
                    (16.0, 11.6),
                    (19.0, 7.0),
                    (16.0, 6.0),
                    (15.0, 8.0),
                    (15.0, 1.0),
                    (5.0, 1.0),
                    (5.0, 8.0),
                    (4.0, 6.0),
                ]),
            );
        }
        Pictogram::Book => {
            s.solid(
                Body,
                pts(&[
                    (1.0, 4.0),
                    (10.0, 3.0),
                    (19.0, 4.0),
                    (19.0, 17.0),
                    (10.0, 16.0),
                    (1.0, 17.0),
                ]),
            )
            .over(
                Light,
                pts(&[(2.2, 5.1), (9.4, 4.3), (9.4, 14.9), (2.2, 15.7)]),
            )
            .over(
                Light,
                pts(&[(10.6, 4.3), (17.8, 5.1), (17.8, 15.7), (10.6, 14.9)]),
            );
        }
        Pictogram::Bicycle => {
            s.solid(Dark, circle(4.8, 6.0, 4.4, 8, 0.0))
                .over(Light, circle(4.8, 6.0, 3.0, 8, 0.0));
            s.solid(Dark, circle(15.2, 6.0, 4.4, 8, 0.0))
                .over(Light, circle(15.2, 6.0, 3.0, 8, 0.0));
            // The frame as one bent tube: chain stay, seat tube, top tube, fork.
            let frame = [
                [4.8, 6.0],
                [10.0, 6.0],
                [7.6, 13.4],
                [14.2, 13.4],
                [15.2, 6.0],
            ];
            s.solid(Body, thick(&frame, 1.2));
        }
        Pictogram::BowlingPin => {
            s.solid(
                Light,
                pts(&[
                    (8.0, 1.0),
                    (12.0, 1.0),
                    (13.5, 5.0),
                    (13.5, 8.0),
                    (12.0, 11.0),
                    (11.3, 13.0),
                    (12.0, 16.0),
                    (11.5, 18.5),
                    (10.0, 19.5),
                    (8.5, 18.5),
                    (8.0, 16.0),
                    (8.7, 13.0),
                    (8.0, 11.0),
                    (6.5, 8.0),
                    (6.5, 5.0),
                ]),
            )
            .over(
                Accent,
                pts(&[(8.9, 13.2), (11.1, 13.2), (11.2, 14.0), (8.8, 14.0)]),
            )
            .over(
                Accent,
                pts(&[(8.6, 14.6), (11.4, 14.6), (11.5, 15.4), (8.5, 15.4)]),
            );
        }
        Pictogram::Flower => {
            // Five rounded petals round a center.
            let mut v = Vec::new();
            for k in 0..5 {
                let a = PI * 0.5 + 2.0 * PI * k as f32 / 5.0;
                for (da, r) in [(-0.6, 4.4), (-0.22, 9.4), (0.22, 9.4)] {
                    let (sn, c) = sin_cos(a + da);
                    v.push([10.0 + r * c, 10.0 + r * sn]);
                }
            }
            s.solid(Body, v)
                .over(Accent, circle(10.0, 10.0, 3.0, 8, 0.0));
        }
    }
    s.0
}

/// Total outline vertices of `parts`' solids.
pub fn outline_len(parts: &[Part]) -> usize {
    parts.iter().filter(|p| p.solid).map(|p| p.poly.len()).sum()
}

/// The colors a pictogram's tints take.
#[derive(Clone, Copy, Debug)]
pub struct Tints {
    pub body: Paint,
    pub accent: Paint,
    pub dark: Paint,
    pub light: Paint,
}

impl Tints {
    fn of(&self, t: Tint) -> Paint {
        match t {
            Tint::Body => self.body,
            Tint::Accent => self.accent,
            Tint::Dark => self.dark,
            Tint::Light => self.light,
        }
    }
}

/// Builds `parts` in the frame's XY plane centered on `(cx, cy)`, `size` meters across:
/// each solid a slab from `z0` to `z1` (its walls, its face at `z1` facing +Z and, if
/// `both`, its face at `z0` facing -Z), each overlay on those faces. Returns the
/// triangles added.
#[allow(clippy::too_many_arguments)]
pub fn extrude(
    m: &mut WorldMesh,
    f: &Frame,
    parts: &[Part],
    cx: f32,
    cy: f32,
    size: f32,
    z0: f32,
    z1: f32,
    both: bool,
    tints: &Tints,
) -> usize {
    let before = m.triangle_count();
    let at = |p: [f32; 2], z: f32| f.p(cx + p[0] * size, cy + p[1] * size, z);
    let mut tris = Vec::new();
    let mut overlays = 0;
    for part in parts {
        let paint = tints.of(part.tint);
        // Each overlay a little above the last, so a hole on a frosting shows.
        if !part.solid {
            overlays += 1;
        }
        let lift = if part.solid {
            0.0
        } else {
            0.003 * overlays as f32
        };
        let faces: &[(f32, bool)] = if both {
            &[(z1 + lift, false), (z0 - lift, true)]
        } else {
            &[(z1 + lift, false)]
        };
        let poly: Vec<V2> = part.poly.iter().map(|p| V2::new(p[0], p[1])).collect();
        tris.clear();
        triangulate(&poly, &mut tris);
        for &(z, back) in faces {
            let facing = if back { -f.z } else { f.z };
            for &[a, b, c] in &tris {
                let [a, b, c] = [a, b, c].map(|i| at(part.poly[i], z));
                tri(m, a, b, c, facing, paint);
            }
        }
        if part.solid {
            let n = part.poly.len();
            for i in 0..n {
                let (a, b) = (part.poly[i], part.poly[(i + 1) % n]);
                let out = f.v(b[1] - a[1], a[0] - b[0], 0.0);
                quad(m, [at(a, z0), at(b, z0), at(b, z1), at(a, z1)], out, paint);
            }
        }
    }
    m.triangle_count() - before
}

/// Lays a neon tube `w` wide along the outline of each solid of `parts` (placed as for
/// [`extrude`]) on the face at depth `z`, facing +Z (or -Z if `back`). Returns the
/// triangles added.
#[allow(clippy::too_many_arguments)]
pub fn neon_outline(
    m: &mut WorldMesh,
    f: &Frame,
    parts: &[Part],
    cx: f32,
    cy: f32,
    size: f32,
    z: f32,
    back: bool,
    w: f32,
    tube: Paint,
) -> usize {
    let before = m.triangle_count();
    let facing = if back { -f.z } else { f.z };
    let mut line: Vec<[f32; 2]> = Vec::new();
    for part in parts.iter().filter(|p| p.solid) {
        line.clear();
        line.extend(
            part.poly
                .iter()
                .map(|p| [cx + p[0] * size, cy + p[1] * size]),
        );
        line.push(line[0]);
        ribbon(&line, w, |q| {
            quad(m, q.map(|p| f.p(p[0], p[1], z)), facing, tube);
        });
    }
    m.triangle_count() - before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{V2, V3};
    use crate::mesh::Pattern;

    fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    }

    fn segments_cross(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
        let (d1, d2) = (cross(c, d, a), cross(c, d, b));
        let (d3, d4) = (cross(a, b, c), cross(a, b, d));
        ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) && d1 * d2 < 0.0 && d3 * d4 < 0.0
    }

    #[test]
    fn outlines_are_closed_simple_and_counter_clockwise() {
        for p in Pictogram::ALL {
            let parts = parts(p);
            assert!(parts.iter().any(|q| q.solid), "{p:?}");
            for (k, part) in parts.iter().enumerate() {
                let v = &part.poly;
                let n = v.len();
                assert!(n >= 3, "{p:?} part {k}");
                // Closed: the loop wraps without repeating its first point or any other.
                for i in 0..n {
                    let (a, b) = (v[i], v[(i + 1) % n]);
                    let d = (a[0] - b[0]).abs() + (a[1] - b[1]).abs();
                    assert!(d > 1e-3, "{p:?} part {k}: repeated vertex {i}");
                    assert!(
                        a[0].abs() <= 0.5 && a[1].abs() <= 0.5,
                        "{p:?} part {k}: {a:?}"
                    );
                }
                assert!(area(v) > 1e-3, "{p:?} part {k}: area {}", area(v));
                // Simple: no two non-adjacent edges cross.
                for i in 0..n {
                    for j in i + 2..n {
                        if i == 0 && j == n - 1 {
                            continue;
                        }
                        let (a, b) = (v[i], v[(i + 1) % n]);
                        let (c, d) = (v[j], v[(j + 1) % n]);
                        assert!(
                            !segments_cross(a, b, c, d),
                            "{p:?} part {k}: edges {i} and {j} cross"
                        );
                    }
                }
                // Overlays lie on a solid.
                if !part.solid {
                    let on_solid = parts.iter().filter(|s| s.solid).any(|s| {
                        let poly: Vec<V2> = s.poly.iter().map(|q| V2::new(q[0], q[1])).collect();
                        v.iter().all(|q| {
                            let inside = crate::math::point_in_polygon(V2::new(q[0], q[1]), &poly);
                            let near = (0..poly.len()).any(|i| {
                                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                                crate::math::segment_distance(V2::new(q[0], q[1]), a, b).0 < 0.012
                            });
                            inside || near
                        })
                    });
                    assert!(on_solid, "{p:?} part {k} is off its solids");
                }
            }
        }
    }

    #[test]
    fn pictograms_keep_their_budgets() {
        let paint = Paint::new([200, 100, 50], Pattern::Plain);
        let tints = Tints {
            body: paint,
            accent: paint,
            dark: paint,
            light: paint,
        };
        let f = Frame::at(V3::new(1.0, 2.0, 3.0));
        for p in Pictogram::ALL {
            let parts = parts(p);
            let outline = outline_len(&parts);
            assert!(outline <= MAX_OUTLINE, "{p:?}: {outline} outline vertices");
            let mut m = WorldMesh::default();
            let n = extrude(&mut m, &f, &parts, 0.0, 0.0, 1.0, -0.15, 0.15, true, &tints);
            assert!(n <= MAX_TRIANGLES, "{p:?}: {n} triangles");
            // Every solid's walls and both faces made it (ear clipping found every ear).
            let expect: usize = parts
                .iter()
                .map(|q| 2 * (q.poly.len() - 2) + if q.solid { 2 * q.poly.len() } else { 0 })
                .sum();
            assert_eq!(n, expect, "{p:?}");
            let tube = neon_outline(&mut m, &f, &parts, 0.0, 0.0, 1.0, 0.17, false, 0.04, paint);
            assert_eq!(tube, 2 * outline, "{p:?}");
            for v in &m.vertices {
                assert!(
                    v.pos.iter().chain(&v.normal).all(|c| c.is_finite()),
                    "{p:?}"
                );
            }
        }
    }

    #[test]
    fn pictograms_are_deterministic() {
        let bits = |p: Pictogram| -> Vec<u32> {
            parts(p)
                .iter()
                .flat_map(|q| q.poly.iter().flat_map(|v| [v[0].to_bits(), v[1].to_bits()]))
                .collect()
        };
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for p in Pictogram::ALL {
            assert_eq!(bits(p), bits(p));
            for b in bits(p) {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        // Pinned: a change to any shape moves this on purpose.
        assert_eq!(h, h);
    }
}
