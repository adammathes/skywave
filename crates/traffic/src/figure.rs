//! The pedestrian figure: a seeded, articulated low-poly person and its poses.
//!
//! **Figure.** [`FigureSpec::from_seed`] draws a height (1.5–1.95 m), a build (slim to
//! heavy), shoulder breadth, posture, hair, an era outfit and a palette from a seed;
//! [`build`] turns it into one vertex-colored [`MeshData`] per [`Part`] (600–1200
//! triangles in all) plus the [`Skeleton`] that poses them. Thirteen parts: pelvis, torso,
//! head (with neck and hair), upper and lower arms (hands on the lower), upper and lower
//! legs, and feet on their own ankle joints so a foot can stay flat on the ground through
//! the stance. Each part is modelled in its own frame with its joint at the origin, limbs
//! hanging along `-y`; the figure frame is the car's (`+x` right, `+y` up, `-z` forward)
//! with the origin on the ground between the feet. Shapes are smooth-shaded lofts of
//! elliptical rings; clothing is color bands with a cuff or hem step where cloth ends,
//! plus a few small pieces (shirt front, tie, sunglasses, headband). No logos, text or
//! faces beyond a nose.
//!
//! **Poses.** [`Skeleton::walk`] gives the joint angles ([`Joints`]) for a gait phase and
//! speed (stand with idle sway, stroll, walk 1.2–1.6 m/s): planted footprints that roll
//! heel to toe, two-bone leg IK, the pelvis as high as the planted legs reach (so it bobs
//! by itself), arms swinging opposite the legs, hips and shoulders counter-rotating, a
//! slouch and a lean. [`Skeleton::step_aside`] and [`Skeleton::hop`] are the poses for
//! getting out of a car's way, made to be blended in with [`Joints::blend`].
//! [`Skeleton::pose`] turns joints into one rigid transform per part ([`Xform`]), which the
//! renderer draws as instances of the part meshes: one mesh set per figure, no per-frame
//! rebuild, and no pose function allocates.
//!
//! Everything is a pure function of its inputs with deterministic math (`crate::math`), so
//! the same seed gives bit-identical meshes everywhere.

use alloc::vec::Vec;

use skywave_core::{Rng, Seed};
pub use vehicle::shape::MeshData;

use crate::math::{atan2, sin_cos, sqrt};

type V3 = [f32; 3];
/// A rotation as rows: `p' = m · p`.
type M3 = [V3; 3];

const PI: f32 = core::f32::consts::PI;
const TAU: f32 = core::f32::consts::TAU;

/// Number of parts per figure.
pub const PARTS: usize = 13;

/// The parts, in an order where every parent comes before its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Part {
    /// The root: joint at the center between the hip joints.
    Pelvis = 0,
    /// Joint at the waist.
    Torso,
    /// Neck, head and hair; joint at the base of the neck.
    Head,
    UpperArmL,
    /// Forearm and hand; joint at the elbow.
    LowerArmL,
    UpperArmR,
    LowerArmR,
    UpperLegL,
    /// Joint at the knee.
    LowerLegL,
    /// Shoe or bare foot; joint at the ankle.
    FootL,
    UpperLegR,
    LowerLegR,
    FootR,
}

/// Each part's parent (the pelvis is its own).
pub const PARENT: [usize; PARTS] = [0, 0, 1, 1, 3, 1, 5, 0, 7, 8, 0, 10, 11];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Era {
    Seventies,
    Eighties,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outfit {
    /// Tee and shorts, sneakers, often tube socks.
    ShortsTee,
    /// Sleeveless dress to the knee, sandals.
    Sundress,
    /// Long trousers (jeans, cords, slacks) and a jacket open over a shirt.
    JeansJacket,
    /// Suit: matching jacket and trousers, shirt, a tie in the seventies.
    Suit,
    /// Tank top, swim trunks, sandals.
    Beach,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hair {
    Bald,
    Cropped,
    Short,
    /// To the shoulders at the back.
    Long,
    /// Big rounded volume.
    Full,
}

/// Linear RGBA colors (the renderer's vertex color encoding), converted from the sRGB
/// palette tables; the skin-contrast check (`off_skin`) compares those sRGB values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub skin: [u8; 4],
    pub hair: [u8; 4],
    pub top: [u8; 4],
    /// Shirt under a jacket, tee sleeve bands, the stripes on tube socks.
    pub trim: [u8; 4],
    pub bottom: [u8; 4],
    pub shoes: [u8; 4],
    /// Tie, headband.
    pub accent: [u8; 4],
}

/// Everything that makes one figure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FigureSpec {
    pub height_m: f32,
    /// 0 slim, 1 heavy.
    pub build: f32,
    /// 0 narrow shoulders and wider hips, 1 broad shoulders.
    pub shoulders: f32,
    /// 0 upright, 1 a marked slouch.
    pub slouch: f32,
    pub era: Era,
    pub outfit: Outfit,
    pub hair: Hair,
    pub headband: bool,
    pub sunglasses: bool,
    pub socks: bool,
    pub palette: Palette,
}

/// Sub-system key for figure seeds.
const FIGURE_KEY: u64 = 0x5045_4453; // "PEDS"

/// sRGB `0xRRGGBB` to a linear color (the same polynomial as `vehicle::shape`).
fn lin(hex: u32) -> [u8; 4] {
    let f = |c: u32| {
        let x = c as f32 / 255.0;
        let y = x * (x * (x * 0.305_306_01 + 0.682_171_1) + 0.012_522_878);
        (y * 255.0 + 0.5).clamp(0.0, 255.0) as u8
    };
    [f(hex >> 16 & 255), f(hex >> 8 & 255), f(hex & 255), 255]
}

/// Skin tones, light to dark.
const SKIN: [u32; 10] = [
    0xf3d5c0, 0xe8bfa0, 0xdcae8a, 0xcd9a72, 0xb98259, 0xa06a45, 0x8a5636, 0x70432a, 0x58331f,
    0x432617,
];
/// Hair colors; the last two are grey.
const HAIR: [u32; 9] = [
    0x1a1512, 0x2e2119, 0x4a3324, 0x6e4e33, 0xa0784b, 0xcdb07a, 0x7a3a20, 0x8e8b86, 0xc9c5bc,
];
const TOPS_70: [u32; 12] = [
    0xc8962e, 0xa34a28, 0x7a7f2e, 0x6b4a2f, 0xe6dabf, 0xd9782d, 0x6e2433, 0x8fb3d9, 0xd4a743,
    0x2f6f6a, 0xb8563a, 0x9c8f5a,
];
const BOTTOMS_70: [u32; 7] = [
    0x3c5a82, 0x2b3f5e, 0x5c4129, 0xb39a72, 0xd8ccb0, 0x7d5a3a, 0x4a4f3a,
];
const TOPS_80: [u32; 12] = [
    0xe8559a, 0x2fb3b0, 0xb39ddb, 0x9fe0c0, 0xf2f2ee, 0x26262a, 0x3a6fe0, 0xf07a62, 0xf2d64b,
    0xf4b393, 0x8a4fc4, 0x5bc0e8,
];
const BOTTOMS_80: [u32; 8] = [
    0x7f9bbd, 0xe9e6df, 0x2a2a2e, 0xc2b28f, 0x4d6c96, 0xe8a6c0, 0x8fd6c8, 0x3a3d4a,
];
const SUITS_70: [u32; 6] = [0x5a3e28, 0xa8875e, 0x2c3550, 0x5e2430, 0x9ab8d6, 0x6d6a4a];
const SUITS_80: [u32; 6] = [0x8a8d93, 0xe3dccb, 0x8ccfc5, 0x2a3148, 0x3c3c42, 0xc9b8d8];
const SHIRTS: [u32; 6] = [0xf2efe6, 0xdfe8f2, 0xf0e2c8, 0xe9d3dc, 0x2a2a2e, 0xd8e6d0];
const SHOES_DARK: [u32; 4] = [0x1e1b1a, 0x3e2a1c, 0x5a3b24, 0x2a2622];
const SNEAKERS: [u32; 4] = [0xe8e8e4, 0xdedad0, 0x2a5ea8, 0xc8382e];
const SANDALS: [u32; 3] = [0x6a4a2e, 0x2a2622, 0xc89a62];

const ACCENTS: [u32; 6] = [0x7a2a1e, 0x2a3a6e, 0xc8962e, 0xe8559a, 0x2fb3b0, 0xf2f2ee];
const LENSES: [u8; 4] = [10, 10, 12, 255];

impl FigureSpec {
    /// The figure for `seed` in `era`.
    pub fn from_seed(seed: u64, era: Era) -> FigureSpec {
        let mut r = Rng::new(Seed(seed), &[FIGURE_KEY, era as u64]);
        let skin_i = pick(&mut r, SKIN.len());
        // Darker skin mostly goes with dark hair, or grey.
        let hair_hex = if skin_i >= 6 {
            [
                0x1a1512, 0x1a1512, 0x2e2119, 0x2e2119, 0x1a1512, 0x2e2119, 0x4a3324, 0x8e8b86,
            ][pick(&mut r, 8)]
        } else if pick(&mut r, 8) == 0 {
            HAIR[7 + pick(&mut r, 2)]
        } else {
            HAIR[pick(&mut r, 7)]
        };
        let outfit_w: [u32; 5] = match era {
            Era::Seventies => [25, 17, 28, 15, 15],
            Era::Eighties => [30, 15, 20, 15, 20],
        };
        let outfit = [
            Outfit::ShortsTee,
            Outfit::Sundress,
            Outfit::JeansJacket,
            Outfit::Suit,
            Outfit::Beach,
        ][weighted(&mut r, &outfit_w)];
        let hair_w: [u32; 5] = match era {
            Era::Seventies => [5, 10, 30, 30, 25],
            Era::Eighties => [5, 15, 35, 30, 15],
        };
        let hair = [
            Hair::Bald,
            Hair::Cropped,
            Hair::Short,
            Hair::Long,
            Hair::Full,
        ][weighted(&mut r, &hair_w)];
        let (tops, bottoms, suits): (&[u32], &[u32], &[u32]) = match era {
            Era::Seventies => (&TOPS_70, &BOTTOMS_70, &SUITS_70),
            Era::Eighties => (&TOPS_80, &BOTTOMS_80, &SUITS_80),
        };
        // Cloth never matches the skin, so nobody reads as underdressed.
        let skin = SKIN[skin_i];
        let mut top = off_skin(tops, pick(&mut r, tops.len()), skin);
        let mut trim = tops[pick(&mut r, tops.len())];
        let mut bottom = off_skin(bottoms, pick(&mut r, bottoms.len()), skin);
        let mut shoes = SHOES_DARK[pick(&mut r, SHOES_DARK.len())];
        let accent = ACCENTS[pick(&mut r, ACCENTS.len())];
        match outfit {
            Outfit::ShortsTee => {
                shoes = SNEAKERS[pick(&mut r, SNEAKERS.len())];
                trim = SHIRTS[pick(&mut r, 3)];
            }
            Outfit::Sundress => {
                bottom = top;
                shoes = off_skin(&SANDALS, pick(&mut r, SANDALS.len()), skin);
            }
            Outfit::JeansJacket => {
                trim = SHIRTS[pick(&mut r, SHIRTS.len())];
                if pick(&mut r, 3) > 0 {
                    // Denim of the era.
                    bottom = bottoms[pick(&mut r, 2)];
                }
            }
            Outfit::Suit => {
                bottom = suits[pick(&mut r, suits.len())];
                top = bottom;
                trim = match era {
                    Era::Seventies => SHIRTS[pick(&mut r, SHIRTS.len() - 2)],
                    // A pastel tee under the jacket.
                    Era::Eighties => tops[pick(&mut r, tops.len())],
                };
            }
            // Sandal soles stand apart from the skin, so the feet don't read as bare.
            Outfit::Beach => shoes = off_skin(&SANDALS, pick(&mut r, SANDALS.len()), skin),
        }
        let height_m = 1.5 + 0.45 * (r.next_f32() + r.next_f32()) * 0.5;
        let build = (r.next_f32() + r.next_f32()) * 0.5;
        let shoulders = r.next_f32();
        let slouch = r.next_f32();
        let casual = matches!(outfit, Outfit::ShortsTee | Outfit::Beach);
        let headband = era == Era::Eighties && casual && hair != Hair::Bald && pick(&mut r, 3) == 0;
        let sunglasses = pick(&mut r, if outfit == Outfit::Beach { 2 } else { 6 }) == 0;
        let socks = outfit == Outfit::ShortsTee && pick(&mut r, 10) < 6;
        FigureSpec {
            height_m,
            build,
            shoulders,
            slouch,
            era,
            outfit,
            hair,
            headband,
            sunglasses,
            socks,
            palette: Palette {
                skin: lin(SKIN[skin_i]),
                hair: lin(hair_hex),
                top: lin(top),
                trim: lin(trim),
                bottom: lin(bottom),
                shoes: lin(shoes),
                accent: lin(accent),
            },
        }
    }
}

/// The first color of `list` from `i` on (wrapping) that stands apart from `skin`.
fn off_skin(list: &[u32], i: usize, skin: u32) -> u32 {
    let d = |a: u32, b: u32| {
        let c = |s: u32| ((a >> s & 255) as i32 - (b >> s & 255) as i32).pow(2);
        c(16) + c(8) + c(0)
    };
    (0..list.len())
        .map(|k| list[(i + k) % list.len()])
        .find(|&c| d(c, skin) >= 70 * 70)
        .unwrap_or(list[i])
}

fn pick(r: &mut Rng, n: usize) -> usize {
    r.range(0, n as u32) as usize
}

fn weighted(r: &mut Rng, w: &[u32]) -> usize {
    let total: u32 = w.iter().sum();
    let mut x = r.range(0, total);
    for (i, &wi) in w.iter().enumerate() {
        if x < wi {
            return i;
        }
        x -= wi;
    }
    w.len() - 1
}

// ---------------------------------------------------------------------------------------
// Small vector math.

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V3) -> V3 {
    let l = sqrt(dot(a, a));
    if l > 0.0 {
        scale(a, 1.0 / l)
    } else {
        a
    }
}
fn mul(m: &M3, p: V3) -> V3 {
    [dot(m[0], p), dot(m[1], p), dot(m[2], p)]
}
/// `mᵀ · p`.
fn tmul(m: &M3, p: V3) -> V3 {
    [
        m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2],
        m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2],
        m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2],
    ]
}
fn mmul(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn fract(x: f32) -> f32 {
    let t = x as i64 as f32;
    let f = x - if t > x { t - 1.0 } else { t };
    if f >= 1.0 {
        0.0
    } else {
        f
    }
}
fn sin(x: f32) -> f32 {
    sin_cos(x).0
}
fn cos(x: f32) -> f32 {
    sin_cos(x).1
}
fn acos(x: f32) -> f32 {
    let x = x.clamp(-1.0, 1.0);
    atan2(sqrt(1.0 - x * x), x)
}

/// `Ry(yaw) · Rz(roll) · Rx(pitch)` from `[pitch, yaw, roll]`. For a limb hanging along
/// `-y`, positive pitch swings it forward (`-z`) and positive roll swings it out to `+x`;
/// positive yaw turns `-z` toward `-x` (left).
pub fn euler(a: V3) -> M3 {
    let (sp, cp) = sin_cos(a[0]);
    let (sy, cy) = sin_cos(a[1]);
    let (sr, cr) = sin_cos(a[2]);
    let rx = [[1.0, 0.0, 0.0], [0.0, cp, -sp], [0.0, sp, cp]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cr, -sr, 0.0], [sr, cr, 0.0], [0.0, 0.0, 1.0]];
    mmul(&ry, &mmul(&rz, &rx))
}

// ---------------------------------------------------------------------------------------
// Mesh building.

/// A vertex-colored mesh under construction.
#[derive(Default)]
struct Mesh(MeshData);

impl Mesh {
    fn vert(&mut self, p: V3, n: V3, c: [u8; 4]) -> u32 {
        let m = &mut self.0;
        m.positions.push(p);
        m.normals.push(n);
        m.uvs.push([0.0, 0.0]);
        m.colors.push(c);
        m.emissive.push([0; 4]);
        (m.positions.len() - 1) as u32
    }

    /// A smooth loft through `pts.len() / segs` rings of `segs` points. Ring `k` point `j`
    /// is at `center_k + radius · (sin θ_j · U + cos θ_j · V)` with `U × V` against the
    /// direction the rings advance (for rings going down `-y`: `U = +x`, `V = -z`), so the
    /// faces come out counter-clockwise from outside. Coincident consecutive rings make a
    /// hard color seam. `cap_top`/`cap_bottom` close the first/last ring with a fan to the
    /// given point.
    fn loft(
        &mut self,
        pts: &[V3],
        cols: &[[u8; 4]],
        segs: usize,
        cap_top: Option<V3>,
        cap_bottom: Option<V3>,
    ) {
        let rings = pts.len() / segs;
        let at = |k: usize, j: usize| pts[k * segs + j % segs];
        let base = self.0.positions.len() as u32;
        for k in 0..rings {
            for j in 0..segs {
                let p = at(k, j);
                let mut kp = k;
                while kp > 0 && at(kp, j) == p {
                    kp -= 1;
                }
                let mut kn = k;
                while kn + 1 < rings && at(kn, j) == p {
                    kn += 1;
                }
                let around = sub(at(k, j + 1), at(k, j + segs - 1));
                let along = sub(at(kn, j), at(kp, j));
                let n = norm(cross(around, along));
                self.vert(p, n, cols[k * segs + j]);
            }
        }
        let idx = |k: usize, j: usize| base + (k * segs + j % segs) as u32;
        for k in 0..rings.saturating_sub(1) {
            for j in 0..segs {
                if at(k, j) == at(k + 1, j) && at(k, j + 1) == at(k + 1, j + 1) {
                    continue;
                }
                let (a, b, c, d) = (idx(k, j), idx(k + 1, j), idx(k + 1, j + 1), idx(k, j + 1));
                self.0.indices.extend_from_slice(&[a, d, c, a, c, b]);
            }
        }
        for (cap, k, top) in [(cap_top, 0, true), (cap_bottom, rings - 1, false)] {
            let Some(c) = cap else { continue };
            let mut mid = [0.0; 3];
            for j in 0..segs {
                mid = add(mid, at(k, j));
            }
            let mid = scale(mid, 1.0 / segs as f32);
            let n = norm(sub(c, mid));
            let ci = self.vert(c, n, cols[k * segs]);
            for j in 0..segs {
                let (p0, p1) = (idx(k, j), idx(k, j + 1));
                if top {
                    self.0.indices.extend_from_slice(&[ci, p1, p0]);
                } else {
                    self.0.indices.extend_from_slice(&[ci, p0, p1]);
                }
            }
        }
    }

    /// A flat triangle facing away from `inside`.
    fn flat(&mut self, a: V3, b: V3, c: V3, col: [u8; 4], inside: V3) {
        let n = cross(sub(b, a), sub(c, a));
        let mid = scale(add(add(a, b), c), 1.0 / 3.0);
        let (b, c, n) = if dot(n, sub(mid, inside)) >= 0.0 {
            (b, c, n)
        } else {
            (c, b, scale(n, -1.0))
        };
        let n = norm(n);
        let i = self.vert(a, n, col);
        let j = self.vert(b, n, col);
        let k = self.vert(c, n, col);
        self.0.indices.extend_from_slice(&[i, j, k]);
    }

    /// A strip of flat quads between two polylines, facing away from `inside`.
    fn strip(&mut self, top: &[V3], bottom: &[V3], col: [u8; 4], inside: V3) {
        for i in 0..top.len() - 1 {
            self.flat(top[i], bottom[i], bottom[i + 1], col, inside);
            self.flat(top[i], bottom[i + 1], top[i + 1], col, inside);
        }
    }
}

/// One profile station of a vertical loft: height, half widths across and front to back,
/// and the center's `z` offset (negative is forward).
#[derive(Clone, Copy)]
struct Station {
    y: f32,
    rx: f32,
    rz: f32,
    cz: f32,
}

const fn st(y: f32, rx: f32, rz: f32, cz: f32) -> Station {
    Station { y, rx, rz, cz }
}

/// The profile at height `y`, linearly between stations (sorted top to bottom), clamped.
fn station_at(prof: &[Station], y: f32) -> Station {
    if y >= prof[0].y {
        return prof[0];
    }
    for w in prof.windows(2) {
        if y >= w[1].y {
            let t = (w[0].y - y) / (w[0].y - w[1].y);
            return st(
                y,
                lerp(w[0].rx, w[1].rx, t),
                lerp(w[0].rz, w[1].rz, t),
                lerp(w[0].cz, w[1].cz, t),
            );
        }
    }
    prof[prof.len() - 1]
}

/// Cloth over part of a loft: from `top` down to `bottom` (part-local `y`), `thick` out from
/// the body. The first band containing a height wins.
#[derive(Clone, Copy)]
struct Band {
    top: f32,
    bottom: f32,
    col: [u8; 4],
    thick: f32,
}

const ALL: f32 = 1e3;

fn band(top: f32, bottom: f32, col: [u8; 4], thick: f32) -> Band {
    Band {
        top,
        bottom,
        col,
        thick,
    }
}

/// Points and colors of a vertical loft through `prof` (top to bottom) dressed in `bands`:
/// a ring at every station and two coincident or stepped rings wherever the cloth
/// changes. `flare(y)` widens the rings (bell-bottoms).
fn dress(
    prof: &[Station],
    bands: &[Band],
    skin: [u8; 4],
    segs: usize,
    flare: &dyn Fn(f32) -> f32,
) -> (Vec<V3>, Vec<[u8; 4]>) {
    let (y0, y1) = (prof[0].y, prof[prof.len() - 1].y);
    let mut ys: Vec<f32> = prof.iter().map(|s| s.y).collect();
    for b in bands {
        for y in [b.top, b.bottom] {
            if y < y0 && y > y1 {
                ys.push(y);
            }
        }
    }
    ys.sort_by(|a, b| b.total_cmp(a));
    ys.dedup();
    let attr = |y: f32| {
        bands
            .iter()
            .find(|b| y <= b.top && y >= b.bottom)
            .map_or((skin, 0.0), |b| (b.col, b.thick))
    };
    let eps = 1e-5;
    let mut pts = Vec::new();
    let mut cols = Vec::new();
    for (i, &y) in ys.iter().enumerate() {
        let above = attr(y + eps);
        let below = attr(y - eps);
        let mut ring = |(col, thick): ([u8; 4], f32)| {
            let s = station_at(prof, y);
            let f = flare(y);
            for j in 0..segs {
                let (sn, cs) = sin_cos(TAU * j as f32 / segs as f32);
                pts.push([(s.rx + thick + f) * sn, y, s.cz - (s.rz + thick + f) * cs]);
                cols.push(col);
            }
        };
        if i == 0 {
            ring(below);
        } else if i == ys.len() - 1 {
            ring(above);
        } else {
            ring(above);
            if above != below {
                ring(below);
            }
        }
    }
    (pts, cols)
}

fn scaled(prof: &[Station], k: f32, kx: f32, kz: f32) -> Vec<Station> {
    prof.iter()
        .map(|s| st(s.y * k, s.rx * k * kx, s.rz * k * kz, s.cz * k))
        .collect()
}

fn no_flare(_: f32) -> f32 {
    0.0
}

/// The right hand's thumb on its fist (the forearm's frame): a tapered five-sided prism
/// from the heel of the palm down the `-z` end of the knuckles, over the curled fingers on
/// the palm's side (`-x`), its root inside the fist. `lw` moves it out with the build. The
/// left hand's is its mirror.
fn thumb(m: &mut Mesh, h: f32, lw: f32, skin: [u8; 4]) {
    const SIDES: usize = 5;
    let base = scale([-0.008 * lw, -0.158, -0.019], h);
    let tip = scale([-0.011 * lw, -0.197, -0.027], h);
    let d = norm(sub(tip, base));
    // Rings advance along `d`: `U × V = -d` as `loft` wants.
    let u = norm(cross([0.0, 0.0, 1.0], d));
    let v = cross(u, d);
    let mut pts = Vec::new();
    for (t, r) in [(0.0, 0.0085), (1.0, 0.0065)] {
        let c = add(base, scale(sub(tip, base), t));
        for j in 0..SIDES {
            let (sn, cs) = sin_cos(TAU * j as f32 / SIDES as f32);
            pts.push(add(c, add(scale(u, r * h * sn), scale(v, r * h * cs))));
        }
    }
    let cols = alloc::vec![skin; pts.len()];
    let end = add(tip, scale(d, 0.005 * h));
    m.loft(&pts, &cols, SIDES, None, Some(end));
}

// ---------------------------------------------------------------------------------------
// The figure.

/// Joint layout and limb lengths of one figure, in meters. Rest pose: standing straight,
/// arms hanging, every part's rotation the identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skeleton {
    pub height_m: f32,
    /// Each part's joint in its parent's frame at rest; the pelvis's is in the figure frame.
    pub offset: [V3; PARTS],
    pub thigh_m: f32,
    pub shin_m: f32,
    /// Ankle joint above the sole.
    pub ankle_m: f32,
    /// Heel behind and toe ahead of the ankle.
    pub heel_m: f32,
    pub toe_m: f32,
    pub upper_arm_m: f32,
    /// 0 upright to 1 slouched.
    pub slouch: f32,
    /// Arms held out from the body (radians), more for a heavier build.
    pub arm_out: f32,
}

/// A built figure: one mesh per part in that part's frame, and the skeleton that poses
/// them.
#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub spec: FigureSpec,
    pub skeleton: Skeleton,
    pub parts: [MeshData; PARTS],
}

impl Figure {
    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|m| m.triangle_count()).sum()
    }
}

/// Figure from a seed (`FigureSpec::from_seed` then [`build`]).
pub fn figure(seed: u64, era: Era) -> Figure {
    build(&FigureSpec::from_seed(seed, era))
}

/// The meshes and skeleton for `spec`.
pub fn build(spec: &FigureSpec) -> Figure {
    let h = spec.height_m.clamp(1.4, 2.1);
    let b = spec.build.clamp(0.0, 1.0);
    let sh = spec.shoulders.clamp(0.0, 1.0);
    let pal = spec.palette;
    let skin = pal.skin;
    // Girth of the trunk and of the limbs, shoulder and hip breadth, belly.
    let gw = 0.86 + 0.42 * b;
    let lw = 0.88 + 0.3 * b;
    let shf = (0.94 + 0.14 * sh) * (0.96 + 0.08 * b);
    let hipf = 1.06 - 0.1 * sh;
    let belly = (b - 0.35).max(0.0) * 0.05;
    let sg = sqrt(gw);

    let thigh = 0.225 * h;
    let shin = 0.245 * h;
    let ankle = 0.04 * h;
    let ua = 0.186 * h;
    let hip_x = 0.05 * hipf * (0.92 + 0.1 * b) * h;
    let shx = 0.098 * shf * h;
    let mut offset = [[0.0; 3]; PARTS];
    offset[Part::Pelvis as usize] = [0.0, thigh + shin + ankle, 0.0];
    offset[Part::Torso as usize] = [0.0, 0.10 * h, 0.0];
    offset[Part::Head as usize] = [0.0, 0.226 * h, 0.006 * h];
    offset[Part::UpperArmL as usize] = [-shx, 0.203 * h, 0.008 * h];
    offset[Part::UpperArmR as usize] = [shx, 0.203 * h, 0.008 * h];
    offset[Part::LowerArmL as usize] = [0.0, -ua, 0.0];
    offset[Part::LowerArmR as usize] = [0.0, -ua, 0.0];
    offset[Part::UpperLegL as usize] = [-hip_x, 0.0, 0.0];
    offset[Part::UpperLegR as usize] = [hip_x, 0.0, 0.0];
    offset[Part::LowerLegL as usize] = [0.0, -thigh, 0.0];
    offset[Part::LowerLegR as usize] = [0.0, -thigh, 0.0];
    offset[Part::FootL as usize] = [0.0, -shin, 0.0];
    offset[Part::FootR as usize] = [0.0, -shin, 0.0];
    let skeleton = Skeleton {
        height_m: h,
        offset,
        thigh_m: thigh,
        shin_m: shin,
        ankle_m: ankle,
        heel_m: 0.03 * h,
        toe_m: 0.113 * h,
        upper_arm_m: ua,
        slouch: spec.slouch.clamp(0.0, 1.0),
        arm_out: 0.07 + 0.1 * b,
    };

    let jacket = matches!(spec.outfit, Outfit::JeansJacket | Outfit::Suit);
    let bare_shoulders = matches!(spec.outfit, Outfit::Sundress | Outfit::Beach);
    let cloth = 0.005 * h;

    // Pelvis.
    let pelvis_prof = [
        st(0.105, 0.083 * hipf, 0.056, -belly * 0.6),
        st(0.06, 0.093 * hipf, 0.06, -belly * 0.5),
        st(0.01, 0.103 * hipf, 0.066, 0.008),
        st(-0.035, 0.096 * hipf, 0.062, 0.004),
    ];
    let pelvis_prof = scaled(&pelvis_prof, h, gw, gw);
    let mut pelvis = Mesh::default();
    {
        let mut bands = Vec::new();
        if jacket {
            bands.push(band(ALL, 0.025 * h, pal.top, cloth));
        }
        bands.push(band(ALL, -ALL, pal.bottom, 0.0));
        let (pts, cols) = dress(&pelvis_prof, &bands, skin, 10, &no_flare);
        let bottom = [0.0, -0.06 * h, 0.004 * h];
        let top = [0.0, 0.105 * h, -belly * 0.6 * h];
        pelvis.loft(&pts, &cols, 10, Some(top), Some(bottom));
        if spec.outfit == Outfit::Sundress {
            let skirt = [
                st(0.05, 0.097 * hipf, 0.064, 0.0),
                // Full enough at the hem that a striding thigh stays inside.
                st(-0.06, 0.12 * hipf, 0.095, -0.004),
                st(-0.165, 0.138 * hipf, 0.135, -0.014),
            ];
            let skirt = scaled(&skirt, h, gw, gw);
            let (pts, cols) = dress(
                &skirt,
                &[band(ALL, -ALL, pal.top, 0.0)],
                skin,
                10,
                &no_flare,
            );
            pelvis.loft(&pts, &cols, 10, None, None);
        }
    }

    // Torso.
    let lerp_hs = lerp(hipf, shf, 0.35);
    let torso_prof = [
        st(0.226, 0.05 * sg / gw, 0.04 / gw, 0.006),
        st(0.205, 0.094 * shf * sg / gw, 0.052 * sg / gw, 0.004),
        st(0.17, 0.1 * shf * sg / gw, 0.063 * sg / gw, -0.004),
        st(
            0.12,
            0.094 * shf * sg / gw,
            0.066 * sg / gw,
            -0.006 - belly * 0.4,
        ),
        st(0.06, 0.087 * lerp_hs, 0.061, -belly),
        // The hem stands clear of the pelvis under it, also when the torso leans.
        st(0.0, 0.093 * hipf, 0.066, -belly * 0.6),
        st(-0.04, 0.104 * hipf, 0.074, -belly * 0.5 + 0.002),
    ];
    let torso_prof = scaled(&torso_prof, h, gw, gw);
    let mut torso = Mesh::default();
    {
        let mut bands = Vec::new();
        if bare_shoulders {
            bands.push(band(ALL, 0.188 * h, skin, 0.0));
        }
        bands.push(band(ALL, -ALL, pal.top, 0.0));
        let (pts, cols) = dress(&torso_prof, &bands, skin, 10, &no_flare);
        torso.loft(&pts, &cols, 10, Some([0.0, 0.232 * h, 0.006 * h]), None);
        if jacket {
            // Shirt front in the open jacket, and a tie in the seventies suit.
            let inside = [0.0, 0.14 * h, 0.0];
            let front = |x: f32, y: f32, out: f32| {
                let s = station_at(&torso_prof, y);
                let u = (x / s.rx).clamp(-1.0, 1.0);
                [x, y, s.cz - s.rz * sqrt(1.0 - u * u) - out]
            };
            let rows = [0.206, 0.17, 0.13, 0.09];
            let half = |y: f32| 0.036 * h * (y - 0.09) / (0.206 - 0.09);
            let out = 0.003 * h;
            let l: Vec<V3> = rows.iter().map(|&y| front(-half(y), y * h, out)).collect();
            let r: Vec<V3> = rows.iter().map(|&y| front(half(y), y * h, out)).collect();
            torso.strip(&l, &r, pal.trim, inside);
            if spec.outfit == Outfit::Suit && spec.era == Era::Seventies {
                let w = 0.009 * h;
                let out = 0.006 * h;
                let ys = [0.204, 0.15, 0.1];
                let l: Vec<V3> = ys.iter().map(|&y| front(-w, y * h, out)).collect();
                let r: Vec<V3> = ys.iter().map(|&y| front(w, y * h, out)).collect();
                torso.strip(&l, &r, pal.accent, inside);
                let tip = front(0.0, 0.085 * h, out);
                torso.flat(l[2], r[2], tip, pal.accent, inside);
            }
        }
    }

    // Head: about 0.23 m chin to crown at 1.75 m; shorter people have relatively bigger
    // heads.
    let hh = 0.23 * sqrt(h / 1.75);
    let head_prof = [
        st(0.86, 0.2, 0.27, 0.02),
        st(0.72, 0.31, 0.40, 0.02),
        st(0.52, 0.35, 0.44, 0.01),
        st(0.30, 0.335, 0.42, -0.01),
        st(0.05, 0.28, 0.34, -0.06),
        st(-0.09, 0.15, 0.15, -0.17),
    ];
    let head_prof = scaled(&head_prof, hh, 1.0, 1.0);
    let mut head = Mesh::default();
    {
        let (pts, cols) = dress(&head_prof, &[], skin, 8, &no_flare);
        head.loft(
            &pts,
            &cols,
            8,
            Some([0.0, 0.91 * hh, 0.02 * hh]),
            Some([0.0, -0.10 * hh, -0.14 * hh]),
        );
        let front = |x: f32, y: f32| {
            let s = station_at(&head_prof, y);
            let u = (x / s.rx).clamp(-1.0, 1.0);
            [x, y, s.cz - s.rz * sqrt(1.0 - u * u)]
        };
        let inside = [0.0, 0.3 * hh, 0.0];
        let tip = add(front(0.0, 0.29 * hh), [0.0, 0.0, -0.075 * hh]);
        let l = front(-0.06 * hh, 0.27 * hh);
        let r = front(0.06 * hh, 0.27 * hh);
        let t = front(0.0, 0.46 * hh);
        head.flat(t, l, tip, skin, inside);
        head.flat(t, tip, r, skin, inside);
        head.flat(l, r, tip, skin, inside);

        // Hair: a shell over the skull down to a hairline that is high at the front, lower
        // at the sides and lowest at the back.
        let style = match spec.hair {
            Hair::Bald => None,
            Hair::Cropped => Some((0.015, 0.76, 0.5, 0.32, 0.0)),
            Hair::Short => Some((0.045, 0.74, 0.42, 0.22, 0.0)),
            Hair::Long => Some((0.05, 0.72, 0.1, -0.5, 0.0)),
            Hair::Full => Some((0.10, 0.70, 0.3, 0.12, 0.06)),
        };
        if let Some((t, f, s, k, lift)) = style {
            let segs = 8;
            let bottom = |th: f32| {
                // The side height holds until close to the front, so long hair frames the
                // face; behind, it eases down to the back.
                let c = cos(th);
                if c > 0.0 {
                    lerp(s, f, smooth(0.6, 0.95, c))
                } else {
                    lerp(s, k, smooth(0.0, 1.0, -c))
                }
            };
            let us = [0.0, 0.4, 1.0];
            let mut pts = Vec::new();
            for &u in &us {
                for j in 0..segs {
                    let th = TAU * j as f32 / segs as f32;
                    let y = lerp(0.86, bottom(th), u);
                    let s = station_at(&head_prof, y.max(0.3) * hh);
                    let (sn, cs) = sin_cos(th);
                    let up = lift * (1.0 - u);
                    pts.push([
                        (s.rx + t * hh) * sn,
                        (y + up) * hh,
                        s.cz - (s.rz + t * hh) * cs,
                    ]);
                }
            }
            let cols = alloc::vec![pal.hair; pts.len()];
            head.loft(
                &pts,
                &cols,
                segs,
                Some([0.0, (0.92 + t + lift) * hh, 0.02 * hh]),
                None,
            );
            if spec.headband {
                let mut pts = Vec::new();
                for y in [0.66, 0.58] {
                    for j in 0..segs {
                        let th = TAU * j as f32 / segs as f32;
                        let s = station_at(&head_prof, y * hh);
                        let r = (t.max(0.02) + 0.012) * hh;
                        let (sn, cs) = sin_cos(th);
                        pts.push([(s.rx + r) * sn, y * hh, s.cz - (s.rz + r) * cs]);
                    }
                }
                let cols = alloc::vec![pal.accent; pts.len()];
                head.loft(&pts, &cols, segs, None, None);
            }
        }
        if spec.sunglasses {
            let ring = |y: f32| -> Vec<V3> {
                (0..5)
                    .map(|i| {
                        let th = (i as f32 - 2.0) * 0.55;
                        let s = station_at(&head_prof, y * hh);
                        let r = 0.03 * hh;
                        let (sn, cs) = sin_cos(th);
                        [(s.rx + r) * sn, y * hh, s.cz - (s.rz + r) * cs]
                    })
                    .collect()
            };
            head.strip(&ring(0.52), &ring(0.42), LENSES, inside);
        }
    }

    // The head sits on a neck 0.04 h long.
    let up = [0.0, 0.04 * h, -0.004 * h];
    for p in &mut head.0.positions {
        *p = add(*p, up);
    }
    {
        let prof = [
            st(0.05, 0.027 * sg, 0.03, 0.003),
            st(-0.015, 0.03 * sg, 0.032, 0.0),
        ];
        let (pts, cols) = dress(&scaled(&prof, h, 1.0, 1.0), &[], skin, 6, &no_flare);
        head.loft(&pts, &cols, 6, None, None);
    }

    // Arms.
    let sleeve = match spec.outfit {
        Outfit::ShortsTee => 0.42 * ua,
        Outfit::JeansJacket | Outfit::Suit => ua + 0.14 * h,
        Outfit::Sundress | Outfit::Beach => 0.0,
    };
    let ua_prof = scaled(
        &[
            st(0.0, 0.032, 0.032, 0.0),
            st(-0.05, 0.029, 0.03, 0.0),
            st(-0.186, 0.021, 0.022, 0.0),
            st(-0.2, 0.015, 0.016, 0.0),
        ],
        h,
        lw,
        lw,
    );
    // The forearm, then the hand closed into a fist: its knuckles across `z`, thickest at
    // the grip point ([`GRIP_H`]) so a steering wheel's rim runs through it and out both
    // ends; the thumb is [`thumb`].
    let la_prof = scaled(
        &[
            st(0.012, 0.018, 0.018, 0.0),
            st(0.0, 0.023, 0.023, 0.0),
            st(-0.146, 0.016, 0.0165, 0.0),
            st(-0.165, 0.016, 0.0235, 0.0),
            st(-0.19, 0.018, 0.0255, 0.0),
            st(-0.212, 0.012, 0.0195, 0.0),
        ],
        h,
        lw,
        1.0,
    );
    let sleeve_bands = |start: f32, end: f32| -> Vec<Band> {
        // Cloth covers the arm from the shoulder down to `sleeve`; `start` is the part's
        // joint distance from the shoulder.
        let mut v = Vec::new();
        if sleeve > start {
            let bottom = -(sleeve - start);
            if spec.outfit == Outfit::ShortsTee && spec.era == Era::Seventies {
                v.push(band(bottom + 0.022 * h, bottom, pal.trim, cloth * 1.3));
            }
            if bottom > -end {
                v.push(band(ALL, bottom, pal.top, cloth));
            } else {
                v.push(band(ALL, -ALL, pal.top, cloth));
            }
        }
        v
    };
    let arm = |prof: &[Station], start: f32, end: f32, cap: f32| {
        let mut m = Mesh::default();
        let bands = sleeve_bands(start, end);
        let (pts, cols) = dress(prof, &bands, skin, 6, &no_flare);
        let top = [0.0, prof[0].y + cap, prof[0].cz];
        let last = prof[prof.len() - 1];
        let bottom = [0.0, last.y - 0.006 * h, last.cz];
        m.loft(&pts, &cols, 6, Some(top), Some(bottom));
        m
    };
    // The shoulder's cap reaches up into the torso; the elbow's is a small knob.
    let upper_arm = arm(&ua_prof, 0.0, ua, 0.024 * h).0;
    let mut lower_arm = arm(&la_prof, ua, 0.146 * h, 0.006 * h);
    thumb(&mut lower_arm, h, lw, skin);
    let lower_arm = lower_arm.0;

    // Legs.
    let pants = match spec.outfit {
        Outfit::JeansJacket | Outfit::Suit => thigh + shin - 0.004 * h,
        Outfit::ShortsTee => match spec.era {
            Era::Seventies => 0.3 * thigh,
            Era::Eighties => 0.5 * thigh,
        },
        Outfit::Beach => 0.42 * thigh,
        Outfit::Sundress => 0.0,
    };
    let bells = spec.era == Era::Seventies && spec.outfit == Outfit::JeansJacket;
    let thigh_prof = scaled(
        &[
            st(0.0, 0.052, 0.055, 0.006),
            st(-0.17, 0.037, 0.039, -0.002),
            st(-0.225, 0.031, 0.032, 0.0),
            st(-0.24, 0.025, 0.027, 0.0),
        ],
        h,
        lw,
        lw,
    );
    let shin_prof = scaled(
        &[
            st(0.0, 0.031, 0.032, 0.0),
            st(-0.07, 0.033, 0.036, 0.007),
            st(-0.2, 0.021, 0.022, 0.0),
            st(-0.245, 0.018, 0.019, 0.0),
        ],
        h,
        lw,
        lw,
    );
    let leg_bands = |start: f32, end: f32| -> Vec<Band> {
        let mut v = Vec::new();
        if pants > start {
            let bottom = -(pants - start);
            let bottom = if bottom > -end { bottom } else { -ALL };
            v.push(band(ALL, bottom, pal.bottom, cloth));
        }
        if spec.socks && start > 0.0 {
            let ank = -shin;
            let sock = lin(0xf0eee6);
            v.push(band(ank + 0.135 * h, ank + 0.12 * h, pal.trim, cloth * 0.7));
            v.push(band(ank + 0.135 * h, -ALL, sock, cloth * 0.6));
        }
        v
    };
    let upper_leg = {
        let mut m = Mesh::default();
        let (pts, cols) = dress(&thigh_prof, &leg_bands(0.0, thigh), skin, 7, &no_flare);
        m.loft(
            &pts,
            &cols,
            7,
            Some([0.0, 0.045 * h, 0.004 * h]),
            Some([0.0, -0.245 * h, 0.0]),
        );
        m.0
    };
    let lower_leg = {
        let mut m = Mesh::default();
        let flare = |y: f32| {
            if bells {
                0.016 * h * ((-y - 0.11 * h) / (0.135 * h)).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let (pts, cols) = dress(&shin_prof, &leg_bands(thigh, shin), skin, 6, &flare);
        let bottom = [0.0, -shin - 0.012 * h, 0.0];
        m.loft(&pts, &cols, 6, Some([0.0, 0.016 * h, 0.0]), Some(bottom));
        m.0
    };
    // Foot: a loft from heel to toe with a flat sole, joint at the ankle.
    let foot = {
        let mut m = Mesh::default();
        let sole = -ankle;
        let barefoot = matches!(spec.outfit, Outfit::Sundress | Outfit::Beach);
        let foot_col = if barefoot { skin } else { pal.shoes };
        let fs = [
            (0.032, 0.019, 0.034),
            (0.01, 0.023, 0.056),
            (-0.07, 0.028, 0.031),
            (-0.113, 0.02, 0.016),
        ];
        let segs = 6;
        let mut pts = Vec::new();
        let mut cols = Vec::new();
        let fk = 0.97 + 0.06 * b;
        // In the foot's frame; the ankle sits 0.04 h above the sole.
        for &(z, rx, fh) in &fs {
            let (z, rx, fh) = (z * h, rx * h * fk, fh * h);
            for j in 0..segs {
                let th = TAU * (j as f32 + 0.5) / segs as f32;
                let (sn, cs) = sin_cos(th);
                let y = (sole + fh * 0.5 + fh * 0.62 * cs).clamp(sole, sole + fh);
                pts.push([rx * sn, y, z]);
                // Sandals: a dark sole under bare feet.
                cols.push(if barefoot && y <= sole {
                    pal.shoes
                } else {
                    foot_col
                });
            }
        }
        let heel = [0.0, sole + 0.018 * h, 0.036 * h];
        let toe = [0.0, sole + 0.007 * h, -0.118 * h];
        m.loft(&pts, &cols, segs, Some(heel), Some(toe));
        m.0
    };

    let mirror = |m: &MeshData| {
        let mut o = m.clone();
        // `0.0 - x` keeps zeros positive.
        for p in &mut o.positions {
            p[0] = 0.0 - p[0];
        }
        for n in &mut o.normals {
            n[0] = 0.0 - n[0];
        }
        for t in o.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
        o
    };
    let parts = [
        pelvis.0,
        torso.0,
        head.0,
        mirror(&upper_arm),
        mirror(&lower_arm),
        upper_arm,
        lower_arm,
        mirror(&upper_leg),
        mirror(&lower_leg),
        mirror(&foot),
        upper_leg,
        lower_leg,
        foot,
    ];
    Figure {
        spec: *spec,
        skeleton,
        parts,
    }
}

// ---------------------------------------------------------------------------------------
// Poses.

/// A rigid transform, `p' = rot · p + pos`, with `rot` as rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub rot: M3,
    pub pos: V3,
}

impl Xform {
    pub const IDENTITY: Xform = Xform {
        rot: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        pos: [0.0; 3],
    };

    pub fn apply(&self, p: V3) -> V3 {
        add(mul(&self.rot, p), self.pos)
    }
}

/// Joint angles of a pose: the pelvis joint's position in the figure frame and every
/// part's `[pitch, yaw, roll]` relative to its parent ([`euler`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Joints {
    pub root: V3,
    pub rot: [V3; PARTS],
}

impl Joints {
    /// `a` toward `b` by `t` (0–1), component by component.
    pub fn blend(a: &Joints, b: &Joints, t: f32) -> Joints {
        let t = t.clamp(0.0, 1.0);
        let l = |x: V3, y: V3| {
            [
                lerp(x[0], y[0], t),
                lerp(x[1], y[1], t),
                lerp(x[2], y[2], t),
            ]
        };
        let mut out = *a;
        out.root = l(a.root, b.root);
        for k in 0..PARTS {
            out.rot[k] = l(a.rot[k], b.rot[k]);
        }
        out
    }
}

/// Fraction of a walk cycle each foot is on the ground.
pub const WALK_DUTY: f32 = 0.62;
/// One idle sway cycle when standing.
pub const IDLE_PERIOD_S: f32 = 4.0;
/// How far a [`Skeleton::step_aside`] carries the figure sideways.
pub const ASIDE_M: f32 = 0.5;
/// How far a [`Skeleton::hop`] carries the figure sideways.
pub const HOP_M: f32 = 0.6;

/// Sideways displacement (along the figure's `+x` times `side`'s sign) a behaviour applies
/// at `t` of a step aside, so the planted feet stay put on the ground.
pub fn aside_offset(t: f32) -> f32 {
    ASIDE_M * smooth(0.0, 1.0, t)
}

/// Sideways displacement at `t` of a hop: all of it while airborne.
pub fn hop_offset(t: f32) -> f32 {
    HOP_M * smooth(0.3, 0.7, t)
}

/// The look yaw ([`Skeleton::step_aside`]) toward a point `(dx, dz)` away in the figure
/// frame: 0 straight ahead, positive to the left.
pub fn look_yaw(dx: f32, dz: f32) -> f32 {
    atan2(-dx, -dz)
}

/// Angles of a two-bone limb rooted at the origin, bones `a` then `b`, whose end should
/// reach `t`: `(pitch, roll, bend)` for the upper bone (`[pitch, 0, roll]`) and the lower
/// bone's bend (`[-bend, 0, 0]`). Near full reach the distance eases toward it instead of
/// clamping, so the knee never snaps straight (the bend's slope stays bounded); a target
/// out of reach falls short by a few millimeters.
fn two_bone(t: V3, a: f32, b: f32) -> (f32, f32, f32) {
    const SOFT: f32 = 0.005;
    let roll = atan2(t[0], -t[1]);
    let d = sqrt(t[0] * t[0] + t[1] * t[1]);
    let f = -t[2];
    let mut x = sqrt(d * d + f * f) / (a + b);
    if x > 1.0 - SOFT {
        // C1 at the joint, approaching 1 without reaching it.
        x = 1.0 - SOFT * SOFT / (x - 1.0 + 2.0 * SOFT);
    }
    let dist = (x * (a + b)).max((a - b).abs() + 1e-4);
    let phi = atan2(f, d);
    let alpha = acos((a * a + dist * dist - b * b) / (2.0 * a * dist));
    let bend = PI - acos((a * a + b * b - dist * dist) / (2.0 * a * b));
    (phi + alpha, roll, bend)
}

impl Skeleton {
    /// Length of one walk cycle (two steps) at `speed` m/s.
    pub fn stride_m(&self, speed: f32) -> f32 {
        self.height_m * (0.42 + 0.23 * speed.clamp(0.6, 2.0))
    }

    /// Gait cycles per second at `speed`: the walk's cadence, easing into the idle sway's
    /// slow cycle when standing. A behaviour advances the phase by this times `dt`.
    pub fn phase_rate(&self, speed: f32) -> f32 {
        let v = speed.clamp(0.6, 2.0);
        lerp(
            1.0 / IDLE_PERIOD_S,
            v / self.stride_m(v),
            smooth(0.15, 0.7, speed),
        )
    }

    /// Pelvis height standing straight, feet flat.
    fn stand_height(&self) -> f32 {
        (self.thigh_m + self.shin_m) * 0.9998 + self.ankle_m
    }

    /// Feet track: half the distance between the two ankles.
    fn track(&self) -> f32 {
        self.offset[Part::UpperLegR as usize][0] * 0.9
    }

    /// One rigid transform per part, in the figure frame.
    pub fn pose(&self, j: &Joints, out: &mut [Xform; PARTS]) {
        out[0] = Xform {
            rot: euler(j.rot[0]),
            pos: j.root,
        };
        for k in 1..PARTS {
            let parent = out[PARENT[k]];
            out[k] = Xform {
                rot: mmul(&parent.rot, &euler(j.rot[k])),
                pos: parent.apply(self.offset[k]),
            };
        }
    }

    /// Torso lean, slouch and a level gaze; arms hanging plumb.
    fn posture(&self, j: &mut Joints, lean: f32) {
        let torso = -(0.04 + 0.07 * self.slouch) - lean;
        j.rot[Part::Torso as usize][0] += torso;
        j.rot[Part::Head as usize][0] += -0.8 * torso - 0.02 - 0.06 * self.slouch;
        for arm in [Part::UpperArmL, Part::UpperArmR] {
            j.rot[arm as usize][0] -= torso;
        }
    }

    fn arms(&self, j: &mut Joints, pitch: [f32; 2], out: f32, elbow: [f32; 2]) {
        let arms = [
            (Part::UpperArmL, Part::LowerArmL, -1.0),
            (Part::UpperArmR, Part::LowerArmR, 1.0),
        ];
        for (k, (upper, lower, side)) in arms.into_iter().enumerate() {
            let u = &mut j.rot[upper as usize];
            u[0] += pitch[k];
            u[2] += side * (self.arm_out + out);
            j.rot[lower as usize][0] += elbow[k];
        }
    }

    /// Where the ankle is for a footprint whose heel is at `(x, z)`, `lift` above the
    /// ground, with the foot pitched `toe` radians (positive toe up): a foot rolls about
    /// its heel when toe up and about its toe when heel up, so the footprint stays put.
    fn ankle_point(&self, x: f32, lift: f32, z: f32, toe: f32) -> V3 {
        let (s, c) = sin_cos(toe);
        // From the contact to the ankle along the foot: back from the toe, ahead of the heel.
        let (dz, at) = if toe >= 0.0 {
            (-self.heel_m, z)
        } else {
            (self.toe_m, z - self.heel_m - self.toe_m)
        };
        [
            x,
            lift + self.ankle_m * c - dz * s,
            at + self.ankle_m * s + dz * c,
        ]
    }

    /// Puts leg `side` (0 left, 1 right) on its footprint (see [`Self::ankle_point`]),
    /// given the pelvis already in `j`: two-bone IK to the ankle, knee forward, then the
    /// foot joint takes up the rest so the foot keeps its pitch, level across and pointing
    /// ahead.
    fn leg(&self, j: &mut Joints, side: usize, x: f32, lift: f32, z: f32, toe: f32) {
        let (upper, lower, foot) = [
            (Part::UpperLegL, Part::LowerLegL, Part::FootL),
            (Part::UpperLegR, Part::LowerLegR, Part::FootR),
        ][side];
        let pelvis = Xform {
            rot: euler(j.rot[0]),
            pos: j.root,
        };
        let hip = pelvis.apply(self.offset[upper as usize]);
        let ankle = self.ankle_point(x, lift, z, toe);
        let (pitch, roll, bend) = two_bone(
            tmul(&pelvis.rot, sub(ankle, hip)),
            self.thigh_m,
            self.shin_m,
        );
        let p = j.rot[0];
        j.rot[upper as usize] = [pitch, 0.0, roll];
        j.rot[lower as usize] = [-bend, 0.0, 0.0];
        j.rot[foot as usize] = [toe - (pitch - bend) - p[0], -p[1], -(roll + p[2])];
    }

    /// The gait at `phase` (cycles; any real, period 1) and `speed` m/s: standing with an
    /// idle sway at 0, a stroll below 1.2 m/s, a walk at 1.2–1.6. At phase 0 the left heel
    /// strikes; the right strikes at 0.5. Every angle is periodic in the phase.
    pub fn walk(&self, phase: f32, speed: f32) -> Joints {
        let p = fract(phase);
        let h = self.height_m;
        let w = smooth(0.15, 0.7, speed);
        let idle = 1.0 - w;
        let k = speed.clamp(0.6, 2.0) / 1.4;
        // The heel lands `a` ahead of where it stands and leaves `a` behind; the ankle is
        // under the hip at mid-stance.
        let a = self.stride_m(speed) * WALK_DUTY * 0.5 * w;
        let (s1, c1) = sin_cos(TAU * p);
        let c2 = cos(2.0 * TAU * p);
        // Heel position along the body (forward negative), lift and foot pitch at leg
        // phase `q`: heel strike toe up, flat, heel off, toe off, swing through.
        let strike = 0.22 * w;
        let push = -0.55 * w;
        let foot = |q: f32| {
            if q < WALK_DUTY {
                let toe = if q < 0.12 {
                    strike * (1.0 - smooth(0.0, 0.12, q))
                } else {
                    push * smooth(0.35, WALK_DUTY, q)
                };
                (self.heel_m - a + 2.0 * a * q / WALK_DUTY, 0.0, toe)
            } else {
                let u = (q - WALK_DUTY) / (1.0 - WALK_DUTY);
                let z = a - a * (1.0 - cos(PI * u));
                let lift = 0.04 * h * w * (0.6 + 0.4 * k) * sin(PI * u);
                (
                    self.heel_m + z,
                    lift,
                    lerp(push, strike, smooth(0.1, 0.85, u)),
                )
            }
        };
        let feet = [foot(p), foot(fract(p + 0.5))];
        let tx = self.track();
        let sway = -0.022 * h / 1.75 * w * cos(TAU * (p - WALK_DUTY * 0.5));
        let x = sway + 0.012 * idle * s1;
        // The pelvis rides as high as the planted legs reach straight: it dips at double
        // support and rises over each stance by itself. A lifted foot doesn't hold it down.
        let hx = self.offset[Part::UpperLegR as usize][0];
        let reach = self.thigh_m + self.shin_m;
        let mut y = self.stand_height();
        for (side, &(z, lift, toe)) in feet.iter().enumerate() {
            let s = if side == 0 { -1.0 } else { 1.0 };
            let ankle = self.ankle_point(s * tx, lift, z, toe);
            let dx = ankle[0] - s * hx - x;
            let over = sqrt((reach * reach * 0.9995 - dx * dx - ankle[2] * ankle[2]).max(0.0));
            y = y.min(over + ankle[1] + 3.0 * lift);
        }

        let mut j = Joints {
            root: [x, y, 0.0],
            rot: [[0.0; 3]; PARTS],
        };
        let yaw = 0.08 * w * k;
        j.rot[Part::Pelvis as usize] = [0.0, -yaw * c1, -0.015 * idle * s1];
        j.rot[Part::Torso as usize] = [0.012 * w * c2, 1.6 * yaw * c1, 0.02 * idle * s1];
        j.rot[Part::Head as usize][1] = -0.6 * yaw * c1 + 0.18 * idle * sin(TAU * p + 1.3);
        self.posture(&mut j, 0.05 * w * k);
        let swing = 0.32 * w * k;
        let sw = [-swing * c1, swing * c1];
        let elbow = |s: f32| 0.2 + 0.25 * w * k + 0.35 * s.max(0.0);
        let breathe = 0.03 * idle * s1;
        self.arms(
            &mut j,
            [sw[0] + breathe, sw[1] + breathe],
            0.0,
            [elbow(sw[0]), elbow(sw[1])],
        );
        for (side, &(z, lift, toe)) in feet.iter().enumerate() {
            let x = if side == 0 { -tx } else { tx };
            self.leg(&mut j, side, x, lift, z, toe);
        }
        j
    }

    /// Getting out of the way: a sidestep toward `side` (its sign; positive is the figure's
    /// right) at `t` (0 start, 1 done) with a startled lift of the arms, looking toward
    /// `look` ([`look_yaw`]) all the while. The behaviour moves the figure by
    /// [`aside_offset`]`(t)` toward `side` so the feet stay planted. At `t = 0` it is the
    /// standing pose; blend into it from the walk with [`Joints::blend`].
    pub fn step_aside(&self, t: f32, side: f32, look: f32) -> Joints {
        let t = t.clamp(0.0, 1.0);
        let h = self.height_m;
        let s = if side < 0.0 { -1.0 } else { 1.0 };
        let arc = sin(PI * t);
        let mut j = Joints {
            root: [0.0, self.stand_height() - 0.03 * h * arc, 0.0],
            rot: [[0.0; 3]; PARTS],
        };
        j.rot[0][2] = -s * 0.05 * arc;
        j.rot[Part::Torso as usize][2] = -s * 0.05 * arc;
        let lw = smooth(0.0, 0.2, t);
        let yaw = look.clamp(-1.9, 1.9) * lw;
        j.rot[Part::Torso as usize][1] = 0.3 * yaw;
        j.rot[Part::Head as usize] = [-0.04 * lw, 0.7 * yaw, 0.0];
        self.posture(&mut j, 0.06 * arc);
        let g = smooth(0.0, 0.25, t) * (1.0 - 0.6 * smooth(0.55, 1.0, t));
        // Startled: elbows out, hands up toward the chest.
        self.arms(&mut j, [0.2 * g; 2], 0.45 * g, [0.2 + 1.9 * g; 2]);

        let body = s * aside_offset(t);
        let tx = self.track();
        let lead_u = (t / 0.5).clamp(0.0, 1.0);
        let trail_u = ((t - 0.4) / 0.5).clamp(0.0, 1.0);
        let (lead, trail) = if s > 0.0 { (1, 0) } else { (0, 1) };
        for (leg, start, u, lift) in [
            (lead, s * tx, lead_u, 0.07 * h),
            (trail, -s * tx, trail_u, 0.05 * h),
        ] {
            let x = start + s * ASIDE_M * smooth(0.0, 1.0, u) - body;
            self.leg(&mut j, leg, x, lift * sin(PI * u), self.heel_m, 0.0);
        }
        j
    }

    /// A small startled hop toward `side` at `t` (0–1): crouch, up and over, land. The
    /// behaviour moves the figure by [`hop_offset`]`(t)` toward `side`; the pose itself
    /// only rises and tucks. At `t = 0` and `1` it is the standing pose.
    pub fn hop(&self, t: f32, side: f32) -> Joints {
        let t = t.clamp(0.0, 1.0);
        let h = self.height_m;
        let stand = self.stand_height();
        let c = 0.07 * h;
        let (y, arm) = if t < 0.3 {
            let s = smooth(0.0, 0.3, t);
            (stand - c * s, -0.5 * s)
        } else if t < 0.7 {
            let u = (t - 0.3) / 0.4;
            (
                stand - c + (c + 0.12 * h) * sin(PI * u),
                lerp(-0.5, 0.9, smooth(0.0, 0.5, u)),
            )
        } else {
            let s = smooth(0.7, 1.0, t);
            (stand - c * (1.0 - s), 0.9 * (1.0 - s))
        };
        let crouch = ((stand - y) / c).clamp(0.0, 1.0);
        let s = if side < 0.0 { -1.0 } else { 1.0 };
        let mut j = Joints {
            root: [0.0, y, 0.0],
            rot: [[0.0; 3]; PARTS],
        };
        j.rot[0][2] = -s * 0.06 * sin(PI * t);
        self.posture(&mut j, 0.3 * crouch);
        let out = 0.3 * sin(PI * t);
        self.arms(&mut j, [arm; 2], out, [0.3 + 0.6 * crouch; 2]);
        // Feet tuck only while airborne; crouch and landing keep them planted.
        let lift = if t > 0.3 && t < 0.7 {
            (y - (stand - c)).max(0.0) * 0.7
        } else {
            0.0
        };
        let tx = self.track();
        let toe = -0.4 * smooth(0.0, 0.05 * h, lift);
        self.leg(&mut j, 0, -tx, lift, self.heel_m, toe);
        self.leg(&mut j, 1, tx, lift, self.heel_m, toe);
        j
    }
}

/// Seat height of the place's benches above the ground they stand on (`world::props`).
pub const SEAT_M: f32 = 0.46;

/// [`Skeleton::phase_rate`] for a figure `height_m` tall at `speed`, without building it:
/// a behaviour advances a figure's gait phase by this times `dt` knowing only its seed's
/// height ([`FigureSpec::height_m`]).
pub fn phase_rate(height_m: f32, speed: f32) -> f32 {
    let v = speed.clamp(0.6, 2.0);
    let stride = height_m * (0.42 + 0.23 * v);
    lerp(1.0 / IDLE_PERIOD_S, v / stride, smooth(0.15, 0.7, speed))
}

/// How far ahead of its seat point a figure `height_m` tall stands before it sits down
/// ([`Skeleton::sit`] at `t = 0`).
pub fn sit_reach(height_m: f32) -> f32 {
    0.85 * 0.225 * height_m
}

impl Skeleton {
    /// Sitting down on a bench seat [`SEAT_M`] high at `t` (0 standing, 1 seated): the
    /// pelvis goes back and down onto the seat with a forward lean on the way, the hands
    /// come to rest on the thighs. The figure's origin is the seat point; at `t = 0` it
    /// stands [`sit_reach`] ahead of it (`-z`), on the feet that stay planted throughout,
    /// so a behaviour switches the origin from the feet to the seat as sitting starts.
    pub fn sit(&self, t: f32) -> Joints {
        let t = t.clamp(0.0, 1.0);
        let u = smooth(0.0, 1.0, t);
        let h = self.height_m;
        let reach = sit_reach(h);
        let dip = sin(PI * t);
        let mut j = Joints {
            root: [
                0.0,
                lerp(self.stand_height(), SEAT_M + 0.05 * h, u),
                lerp(-reach, 0.0, u),
            ],
            rot: [[0.0; 3]; PARTS],
        };
        self.posture(&mut j, 0.35 * dip - 0.05 * u);
        self.arms(&mut j, [0.45 * u; 2], 0.05 * u, [0.2 + 0.8 * u; 2]);
        let tx = self.track();
        let heel = self.heel_m - reach;
        self.leg(&mut j, 0, -tx, 0.0, heel, 0.0);
        self.leg(&mut j, 1, tx, 0.0, heel, 0.0);
        j
    }
}

// ---------------------------------------------------------------------------------------
// Serving.

impl Skeleton {
    /// Leaning out of a service window to hand something over: standing, the back tipped
    /// forward by `lean` (0 upright, 1 out over the sill), the right arm held out ahead by
    /// `reach` (0 hanging, 1 about level), the elbow a little bent, the left forearm on the sill. Both 0 is the standing
    /// pose ([`Skeleton::walk`] at rest).
    pub fn serve(&self, lean: f32, reach: f32) -> Joints {
        let mut j = self.walk(0.0, 0.0);
        let tip = 0.5 * smooth(0.0, 1.0, lean);
        let reach = smooth(0.0, 1.0, reach);
        j.rot[Part::Torso as usize][0] -= tip;
        j.rot[Part::Head as usize][0] += 0.7 * tip;
        // The free hand comes up to rest on the sill.
        j.rot[Part::UpperArmL as usize][0] += tip + 0.5 * lean;
        j.rot[Part::LowerArmL as usize][0] += 1.0 * lean;
        j.rot[Part::UpperArmR as usize][0] += tip + 1.25 * reach;
        j.rot[Part::LowerArmR as usize][0] += 0.3 * reach;
        j
    }

    /// The right palm in the figure frame for pose `j`.
    pub fn hand(&self, j: &Joints) -> V3 {
        let mut xf = [Xform::IDENTITY; PARTS];
        self.pose(j, &mut xf);
        xf[Part::LowerArmR as usize].apply([0.0, -GRIP_H * self.height_m, 0.0])
    }
}

// ---------------------------------------------------------------------------------------
// Driving.

/// Where a driver sits, in the figure frame: `+x` right, `+y` up, `-z` forward, the origin
/// anywhere (the car's own frame scaled to the figure does).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seat {
    /// The hip joint (the seat's hip point).
    pub hip: V3,
    /// How far the seat back leans back from upright, radians.
    pub recline: f32,
    /// Where each heel rests on the floor, left then right.
    pub heels: [V3; 2],
    /// Each foot's pitch on its pedal or the floor, radians, toe up.
    pub toe: f32,
    /// Where each hand grips the wheel rim, left then right.
    pub hands: [V3; 2],
    /// Head turn, radians, positive to the left.
    pub look: f32,
}

/// Grip point along the forearm from the elbow, as a share of the height: the palm.
const GRIP_H: f32 = 0.19;

/// The figure's eye above the neck joint at rest (the head part's frame), as shares of the
/// height and of the head: the head sits 0.04 h up its neck, the eyes 0.47 of its height.
fn eye_in_head(h: f32) -> V3 {
    let hh = 0.23 * sqrt(h / 1.75);
    [0.0, 0.04 * h + 0.47 * hh, -0.004 * h - 0.3 * hh]
}

/// `[pitch, yaw, roll]` of the rotation `m` ([`euler`]'s inverse).
fn angles(m: &M3) -> V3 {
    let roll = atan2(m[1][0], sqrt(m[0][0] * m[0][0] + m[2][0] * m[2][0]));
    [atan2(-m[1][2], m[1][1]), atan2(-m[2][0], m[0][0]), roll]
}

impl Skeleton {
    /// The eye's distance from the hip joint sitting straight up, m: what a seat scales the
    /// figure by to put its eyes where the car's are.
    pub fn hip_to_eye(&self) -> f32 {
        let e = eye_in_head(self.height_m);
        self.offset[Part::Torso as usize][1] + self.offset[Part::Head as usize][1] + e[1]
    }

    /// The eye in the figure frame for `j` (a posed figure's eye point).
    pub fn eye(&self, j: &Joints) -> V3 {
        let mut xf = [Xform::IDENTITY; PARTS];
        self.pose(j, &mut xf);
        xf[Part::Head as usize].apply(eye_in_head(self.height_m))
    }

    /// Seated at the wheel: the hips on `seat.hip`, the back on the seat back, the heels
    /// on the floor with the feet on the pedals, the hands gripping the rim where `seat`
    /// says (two-bone arms, elbows down and a little out) and the head level, turned by
    /// `seat.look`. A target out of reach is reached for as far as the limb goes.
    pub fn drive(&self, seat: &Seat) -> Joints {
        let mut j = Joints {
            root: seat.hip,
            rot: [[0.0; 3]; PARTS],
        };
        // The pelvis tips back with the seat; the back sits on it, rounded a little by a
        // slouch; the head comes level and turns with the look, the shoulders a little.
        let back = seat.recline.clamp(0.0, 0.6);
        let round = -0.04 - 0.06 * self.slouch;
        let look = seat.look.clamp(-1.3, 1.3);
        j.rot[Part::Pelvis as usize][0] = back;
        j.rot[Part::Torso as usize] = [round, 0.12 * look, 0.0];
        for side in 0..2 {
            let h = seat.heels[side];
            self.leg(&mut j, side, h[0], h[1], h[2], seat.toe);
        }
        let a = self.upper_arm_m;
        let b = GRIP_H * self.height_m;
        let arms = [
            (Part::UpperArmL, Part::LowerArmL, -1.0),
            (Part::UpperArmR, Part::LowerArmR, 1.0),
        ];
        // A wheel out of easy reach brings the shoulders forward: the back leans off the
        // seat back until the farther hand is within reach, up to 0.6 rad.
        let mut xf = [Xform::IDENTITY; PARTS];
        let lever = self.offset[Part::UpperArmL as usize][1];
        let mut lean = 0.0;
        for _ in 0..3 {
            self.pose(&j, &mut xf);
            let torso = xf[Part::Torso as usize];
            let short = arms
                .iter()
                .enumerate()
                .map(|(k, &(upper, _, _))| {
                    let d = sub(seat.hands[k], torso.apply(self.offset[upper as usize]));
                    sqrt(dot(d, d)) - 0.96 * (a + b)
                })
                .fold(0.0f32, f32::max);
            if short <= 0.0 || lean >= 0.6 {
                break;
            }
            let more = (short / lever).min(0.6 - lean);
            lean += more;
            j.rot[Part::Torso as usize][0] -= more;
        }
        self.pose(&j, &mut xf);
        let torso = xf[Part::Torso as usize];
        // The head turns about the vertical, whatever the back's lean: its rotation in the
        // figure frame, taken back into the torso's.
        let head = euler([-0.02, 0.85 * look, 0.0]);
        let mut rel = [[0.0; 3]; 3];
        for (c, col) in (0..3)
            .map(|c| tmul(&torso.rot, [head[0][c], head[1][c], head[2][c]]))
            .enumerate()
        {
            for r in 0..3 {
                rel[r][c] = col[r];
            }
        }
        j.rot[Part::Head as usize] = angles(&rel);
        // Arms: the shoulders where the torso puts them, each hand on its grip.
        for (k, (upper, lower, s)) in arms.into_iter().enumerate() {
            let shoulder = self.offset[upper as usize];
            // The hand from the shoulder, in the torso's frame.
            let mut d = tmul(&torso.rot, sub(seat.hands[k], torso.apply(shoulder)));
            let len = sqrt(dot(d, d)).clamp(1e-3, (a + b) * 0.995);
            d = scale(norm(d), len);
            let dir = norm(d);
            // The elbow bends toward its pole: down, out to the side and a little back.
            let pole = [0.55 * s, -1.0, 0.25];
            let n = norm(sub(pole, scale(dir, dot(pole, dir))));
            let cos_a = ((a * a + len * len - b * b) / (2.0 * a * len)).clamp(-1.0, 1.0);
            let sin_a = sqrt(1.0 - cos_a * cos_a);
            let elbow = add(scale(dir, a * cos_a), scale(n, a * sin_a));
            // The upper arm's frame: `+y` from the elbow up to the shoulder, `-z` toward
            // the hand across the bend, `x = y × z`.
            let y = scale(elbow, -1.0 / a);
            let to_hand = sub(d, elbow);
            let fwd = norm(sub(to_hand, scale(y, dot(to_hand, y))));
            let z = scale(fwd, -1.0);
            let x = cross(y, z);
            let m = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
            j.rot[upper as usize] = angles(&m);
            // The forearm swings forward (`+pitch`) by the elbow's bend.
            let cos_e = dot(scale(y, -1.0), norm(to_hand));
            j.rot[lower as usize] = [acos(cos_e), 0.0, 0.0];
        }
        j
    }
}

/// The player's drivers to pick from ([`FigureSpec::preset`]): a name for the picker and
/// how each looks. Invented people; the clothes are the era's.
pub const PRESETS: usize = 4;

/// Picker names of the [`PRESETS`].
pub const PRESET_NAMES: [&str; PRESETS] = ["Jacket", "Suit", "Tee", "Sundress"];

/// Which of the presets' wardrobes a world dresses them in: the 1980s coast, or 2001.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wardrobe {
    Eighties = 0,
    Aughts = 1,
}

/// One preset: build, outfit and hair, skin and hair colors, and per wardrobe (1980s,
/// 2001) top, trim, bottom, shoes and accent, sRGB.
struct Preset {
    height_m: f32,
    build: f32,
    shoulders: f32,
    slouch: f32,
    outfit: Outfit,
    hair: Hair,
    sunglasses: bool,
    skin: u32,
    hair_rgb: u32,
    clothes: [[u32; 5]; 2],
}

/// Grown-up, put-together looks for a drive: a jacket over a tee, a suit, a weekend tee and
/// shorts, a sundress; sunglasses on two.
const PRESET_LOOKS: [Preset; PRESETS] = [
    Preset {
        height_m: 1.8,
        build: 0.45,
        shoulders: 0.75,
        slouch: 0.3,
        outfit: Outfit::JeansJacket,
        hair: Hair::Short,
        sunglasses: true,
        skin: SKIN[3],
        hair_rgb: HAIR[1],
        clothes: [
            // 1980s: black over a white tee, stonewash.
            [0x26262a, 0xf2f2ee, 0x7f9bbd, 0x1e1b1a, 0x2a3a6e],
            // 2001: an olive field jacket over a grey tee, dark indigo.
            [0x5a5f3a, 0x9a9a96, 0x24304a, 0x3e2a1c, 0x2a3a6e],
        ],
    },
    Preset {
        height_m: 1.84,
        build: 0.55,
        shoulders: 0.85,
        slouch: 0.1,
        outfit: Outfit::Suit,
        hair: Hair::Cropped,
        sunglasses: false,
        skin: SKIN[7],
        hair_rgb: HAIR[0],
        clothes: [
            // 1980s: charcoal over a white shirt.
            [0x3c3c42, 0xf2efe6, 0x3c3c42, 0x1e1b1a, 0x2a3a6e],
            // 2001: black over a pale blue shirt, open at the neck.
            [0x1f1f22, 0xcfdcea, 0x1f1f22, 0x1e1b1a, 0x2a3a6e],
        ],
    },
    Preset {
        height_m: 1.72,
        build: 0.35,
        shoulders: 0.5,
        slouch: 0.45,
        outfit: Outfit::ShortsTee,
        hair: Hair::Long,
        sunglasses: false,
        skin: SKIN[1],
        hair_rgb: HAIR[2],
        clothes: [
            // 1980s: a white tee, navy shorts.
            [0xf2f2ee, 0xdfe8f2, 0x2a3148, 0xe8e8e4, 0x3a6fe0],
            // 2001: a navy tee, khaki cargo shorts, grey sneakers.
            [0x26324a, 0x9a9a96, 0xa89a78, 0xc8c8c4, 0x3a6fe0],
        ],
    },
    Preset {
        height_m: 1.68,
        build: 0.35,
        shoulders: 0.25,
        slouch: 0.2,
        outfit: Outfit::Sundress,
        hair: Hair::Full,
        sunglasses: true,
        skin: SKIN[5],
        hair_rgb: HAIR[6],
        clothes: [
            // 1980s: coral.
            [0xf07a62, 0xf07a62, 0xf07a62, 0xc89a62, 0x2fb3b0],
            // 2001: deep teal.
            [0x2a5d6a, 0x2a5d6a, 0x2a5d6a, 0x2a2622, 0xc8962e],
        ],
    },
];

impl FigureSpec {
    /// The player's preset driver `i` (wrapping) dressed from `wardrobe`. The figure is
    /// built as the eighties' (no seventies flares or tie in either).
    pub fn preset(i: usize, wardrobe: Wardrobe) -> FigureSpec {
        let p = &PRESET_LOOKS[i % PRESETS];
        let [top, trim, bottom, shoes, accent] = p.clothes[wardrobe as usize];
        FigureSpec {
            height_m: p.height_m,
            build: p.build,
            shoulders: p.shoulders,
            slouch: p.slouch,
            era: Era::Eighties,
            outfit: p.outfit,
            hair: p.hair,
            headband: false,
            sunglasses: p.sunglasses,
            socks: false,
            palette: Palette {
                skin: lin(p.skin),
                hair: lin(p.hair_rgb),
                top: lin(top),
                trim: lin(trim),
                bottom: lin(bottom),
                shoes: lin(shoes),
                accent: lin(accent),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skywave_core::hash_bytes;
    use std::vec::Vec as StdVec;

    const ERAS: [Era; 2] = [Era::Seventies, Era::Eighties];

    fn mesh_hash(f: &Figure) -> u64 {
        let mut bytes = StdVec::new();
        for m in &f.parts {
            for p in m.positions.iter().chain(&m.normals) {
                for v in p {
                    bytes.extend_from_slice(&v.to_bits().to_le_bytes());
                }
            }
            for c in m.colors.iter().chain(&m.emissive) {
                bytes.extend_from_slice(c);
            }
            for i in &m.indices {
                bytes.extend_from_slice(&i.to_le_bytes());
            }
        }
        hash_bytes(0, &bytes)
    }

    /// Lowest point of a posed part.
    fn lowest(f: &Figure, xf: &[Xform; PARTS], part: Part) -> f32 {
        let k = part as usize;
        f.parts[k]
            .positions
            .iter()
            .map(|&p| xf[k].apply(p)[1])
            .fold(f32::MAX, f32::min)
    }

    /// A car seat for figure `sk`: hips 0.3 m over the floor, the wheel in easy reach ahead,
    /// hands at ten to two turned by `steer`.
    fn test_seat(sk: &Skeleton, steer: f32, look: f32) -> Seat {
        let h = sk.height_m;
        let hip = [0.0, 0.3, 0.0];
        let wheel = [0.0, 0.3 + 0.2 * h, -0.34];
        let r = 0.19;
        let grip = |a: f32| {
            let (s, c) = sin_cos(a + steer);
            [wheel[0] - r * c, wheel[1] + r * s, wheel[2]]
        };
        Seat {
            hip,
            recline: 0.3,
            heels: [[-0.12, 0.0, -0.75], [0.12, 0.0, -0.75]],
            toe: 0.5,
            hands: [grip(0.35), grip(PI - 0.35)],
            look,
        }
    }

    #[test]
    fn a_driver_grips_the_wheel_with_heels_down_and_looks_level() {
        for seed in 0..40 {
            let era = ERAS[seed as usize % 2];
            let f = figure(seed, era);
            let sk = &f.skeleton;
            let mut xf = [Xform::IDENTITY; PARTS];
            for steer in [-1.2f32, -0.5, 0.0, 0.5, 1.2] {
                for look in [-1.0, 0.0, 1.0] {
                    let seat = test_seat(sk, steer, look);
                    let j = sk.drive(&seat);
                    sk.pose(&j, &mut xf);
                    assert!(xf.iter().all(|x| x.pos.iter().all(|v| v.is_finite())));
                    for (k, lower) in [Part::LowerArmL, Part::LowerArmR].into_iter().enumerate() {
                        let hand = xf[lower as usize].apply([0.0, -GRIP_H * sk.height_m, 0.0]);
                        let miss = sqrt(dot(sub(hand, seat.hands[k]), sub(hand, seat.hands[k])));
                        assert!(
                            miss < 0.01,
                            "seed {seed} steer {steer}: hand {k} off by {miss}"
                        );
                    }
                    // Elbows below the shoulders' line to the hands: down, not up.
                    for (upper, lower) in [
                        (Part::UpperArmL, Part::LowerArmL),
                        (Part::UpperArmR, Part::LowerArmR),
                    ] {
                        let (sh, el) = (xf[upper as usize].pos, xf[lower as usize].pos);
                        assert!(el[1] < sh[1], "seed {seed}: elbow over the shoulder");
                    }
                    for (k, foot) in [Part::FootL, Part::FootR].into_iter().enumerate() {
                        let y = lowest(&f, &xf, foot);
                        assert!((-0.03..0.05).contains(&y), "seed {seed}: foot {k} at {y}");
                    }
                    // The head stays level, turned by the look.
                    let head = xf[Part::Head as usize].rot;
                    assert!(head[1][1] > 0.97, "seed {seed}: head tipped {:?}", head);
                    let fwd = mul(&head, [0.0, 0.0, -1.0]);
                    let yaw = atan2(-fwd[0], -fwd[2]);
                    assert!(
                        (yaw - look).abs() < 0.3 + 0.2 * look.abs(),
                        "{yaw} for {look}"
                    );
                }
            }
            let eye = sk.eye(&sk.drive(&test_seat(sk, 0.0, 0.0)));
            let up = eye[1] - 0.3;
            assert!(
                up > 0.75 * sk.hip_to_eye() && up < sk.hip_to_eye(),
                "eye {eye:?}"
            );
        }
    }

    #[test]
    fn presets_build_within_budget_and_differ() {
        let mut hashes = StdVec::new();
        for wardrobe in [Wardrobe::Eighties, Wardrobe::Aughts] {
            for i in 0..PRESETS {
                let f = build(&FigureSpec::preset(i, wardrobe));
                assert!((600..=1200).contains(&f.triangle_count()));
                hashes.push(mesh_hash(&f));
            }
        }
        for (i, a) in hashes.iter().enumerate() {
            assert!(hashes[i + 1..].iter().all(|b| b != a));
        }
        assert_eq!(
            FigureSpec::preset(PRESETS, Wardrobe::Eighties),
            FigureSpec::preset(0, Wardrobe::Eighties)
        );
    }

    #[test]
    fn sitting_lands_on_the_seat_with_the_feet_down() {
        for seed in 0..40 {
            let f = figure(seed, ERAS[seed as usize % 2]);
            let sk = &f.skeleton;
            assert_eq!(
                phase_rate(sk.height_m, 1.3).to_bits(),
                sk.phase_rate(1.3).to_bits()
            );
            let mut xf = [Xform::IDENTITY; PARTS];
            for i in 0..=20 {
                let j = sk.sit(i as f32 / 20.0);
                sk.pose(&j, &mut xf);
                for foot in [Part::FootL, Part::FootR] {
                    let y = lowest(&f, &xf, foot);
                    assert!((-0.02..0.03).contains(&y), "seed {seed} t {i}: foot at {y}");
                }
                assert!(xf.iter().all(|x| x.pos.iter().all(|v| v.is_finite())));
            }
            let seat = sk.sit(1.0);
            assert!(seat.root[2].abs() < 1e-6);
            assert!((seat.root[1] - SEAT_M - 0.05 * sk.height_m).abs() < 1e-4);
            assert!((sk.sit(0.0).root[2] + sit_reach(sk.height_m)).abs() < 1e-6);
        }
    }

    #[test]
    fn every_figure_is_within_the_triangle_budget() {
        let (mut lo, mut hi) = (usize::MAX, 0);
        let mut outfits = [0usize; 5];
        for era in ERAS {
            for seed in 0..300 {
                let f = figure(seed, era);
                let n = f.triangle_count();
                lo = lo.min(n);
                hi = hi.max(n);
                outfits[f.spec.outfit as usize] += 1;
                assert!(
                    (600..=1200).contains(&n),
                    "{era:?} seed {seed}: {n} triangles"
                );
                assert!((1.5..=1.95).contains(&f.spec.height_m));
                for m in &f.parts {
                    assert!(!m.indices.is_empty());
                    assert_eq!(m.positions.len(), m.normals.len());
                    assert_eq!(m.positions.len(), m.colors.len());
                    assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
                    assert!(m.positions.iter().flatten().all(|v| v.is_finite()));
                }
            }
        }
        std::println!("triangles per figure: {lo}..={hi}; outfits {outfits:?}");
        assert!(outfits.iter().all(|&n| n > 30), "{outfits:?}");
    }

    #[test]
    fn same_seed_gives_the_same_bytes() {
        for era in ERAS {
            for seed in [0, 1, 7, 12345] {
                let a = figure(seed, era);
                let b = figure(seed, era);
                assert_eq!(a, b);
                assert_eq!(mesh_hash(&a), mesh_hash(&b));
            }
            assert_ne!(mesh_hash(&figure(1, era)), mesh_hash(&figure(2, era)));
        }
        // Pinned across platforms; moves only on purpose, in its own commit.
        let pinned = [
            mesh_hash(&figure(1, Era::Seventies)),
            mesh_hash(&figure(1, Era::Eighties)),
        ];
        assert_eq!(
            pinned,
            [0x3cdc_8b19_2075_1cf5, 0xf95e_4003_3c6e_92a8],
            "{pinned:#x?}"
        );
    }

    #[test]
    fn walk_cycle_has_no_joint_jumps_across_the_wrap() {
        let f = figure(3, Era::Seventies);
        let sk = f.skeleton;
        for speed in [0.0, 0.4, 0.9, 1.2, 1.4, 1.6] {
            let n = 2000;
            let mut prev = sk.walk(-1.0 / n as f32, speed);
            // Two full cycles through the wrap, plus phases outside [0, 1).
            for i in 0..=2 * n {
                let p = i as f32 / n as f32;
                let cur = sk.walk(p, speed);
                for k in 0..PARTS {
                    for a in 0..3 {
                        let d = (cur.rot[k][a] - prev.rot[k][a]).abs();
                        assert!(d < 0.03, "speed {speed} phase {p} part {k} axis {a}: {d}");
                    }
                }
                for a in 0..3 {
                    assert!((cur.root[a] - prev.root[a]).abs() < 0.003);
                }
                prev = cur;
            }
            let (a, b) = (sk.walk(0.25, speed), sk.walk(3.25, speed));
            for k in 0..PARTS {
                for x in 0..3 {
                    assert!((a.rot[k][x] - b.rot[k][x]).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn feet_stay_on_the_ground_over_a_cycle() {
        for era in ERAS {
            for seed in 0..12 {
                let f = figure(seed, era);
                let sk = f.skeleton;
                let mut xf = [Xform::IDENTITY; PARTS];
                for speed in [0.0, 0.9, 1.2, 1.4, 1.6] {
                    for i in 0..100 {
                        let p = i as f32 / 100.0;
                        sk.pose(&sk.walk(p, speed), &mut xf);
                        let l = lowest(&f, &xf, Part::FootL);
                        let r = lowest(&f, &xf, Part::FootR);
                        assert!(l > -0.02 && r > -0.02, "seed {seed} {speed} {p}: {l} {r}");
                        assert!(l.min(r) < 0.02, "seed {seed} {speed} {p}: {l} {r}");
                        // The shins and thighs stay above the ground too.
                        for part in [Part::LowerLegL, Part::LowerLegR] {
                            assert!(lowest(&f, &xf, part) > 0.0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn arms_swing_opposite_the_legs() {
        let sk = figure(5, Era::Eighties).skeleton;
        // Left heel strike: left leg ahead, left arm behind.
        let j = sk.walk(0.0, 1.4);
        let (ul, ur) = (Part::UpperLegL as usize, Part::UpperLegR as usize);
        let (al, ar) = (Part::UpperArmL as usize, Part::UpperArmR as usize);
        assert!(j.rot[ul][0] > j.rot[ur][0] + 0.3);
        assert!(j.rot[al][0] < j.rot[ar][0] - 0.2);
        let j = sk.walk(0.5, 1.4);
        assert!(j.rot[ur][0] > j.rot[ul][0] + 0.3);
        assert!(j.rot[ar][0] < j.rot[al][0] - 0.2);
        // Faster walking swings wider.
        let swing = |s: f32| sk.walk(0.0, s).rot[ar][0] - sk.walk(0.0, s).rot[al][0];
        assert!(swing(1.6) > swing(1.2) && swing(1.2) > swing(0.0));
        // The pelvis bobs: lowest at double support, highest mid-stance.
        assert!(sk.walk(0.0, 1.4).root[1] + 0.02 < sk.walk(0.31, 1.4).root[1]);
    }

    #[test]
    fn serving_leans_out_and_holds_the_hand_ahead() {
        let sk = figure(3, Era::Eighties).skeleton;
        assert_eq!(sk.serve(0.0, 0.0), sk.walk(0.0, 0.0));
        let rest = sk.hand(&sk.serve(0.0, 0.0));
        let out = sk.hand(&sk.serve(1.0, 1.0));
        let j = sk.serve(1.0, 1.0);
        assert!(j.rot.iter().flatten().chain(&j.root).all(|v| v.is_finite()));
        // Ahead (-z) by most of an arm and raised to about the chest.
        assert!(out[2] < rest[2] - 0.5, "{rest:?} -> {out:?}");
        assert!(
            out[1] > rest[1] + 0.35 && out[1] < 0.8 * sk.height_m,
            "{out:?}"
        );
        // The head goes out with the back.
        let (e0, e1) = (sk.eye(&sk.serve(0.0, 0.0)), sk.eye(&sk.serve(1.0, 0.0)));
        assert!(e1[2] < e0[2] - 0.15, "{e0:?} -> {e1:?}");
    }

    #[test]
    fn step_aside_and_hop_start_and_end_standing() {
        let f = figure(8, Era::Seventies);
        let sk = f.skeleton;
        let stand = sk.walk(0.0, 0.0);
        let mut xf = [Xform::IDENTITY; PARTS];
        for side in [-1.0, 1.0] {
            let look = look_yaw(side * 3.0, -2.0);
            for i in 0..=50 {
                let t = i as f32 / 50.0;
                for j in [sk.step_aside(t, side, look), sk.hop(t, side)] {
                    assert!(j.rot.iter().flatten().chain(&j.root).all(|v| v.is_finite()));
                    sk.pose(&j, &mut xf);
                    let feet = lowest(&f, &xf, Part::FootL).min(lowest(&f, &xf, Part::FootR));
                    assert!(feet > -0.02, "{t}: {feet}");
                }
            }
            for j in [
                sk.step_aside(0.0, side, look),
                sk.hop(0.0, side),
                sk.hop(1.0, side),
            ] {
                assert!((j.root[1] - stand.root[1]).abs() < 0.01);
                sk.pose(&j, &mut xf);
                let feet = lowest(&f, &xf, Part::FootL).min(lowest(&f, &xf, Part::FootR));
                assert!(feet.abs() < 0.01, "starts or ends {feet} m off the ground");
            }
            // Crouching and landing, the feet stay planted.
            for t in [0.15, 0.3, 0.7, 0.85] {
                sk.pose(&sk.hop(t, side), &mut xf);
                let feet = lowest(&f, &xf, Part::FootL).min(lowest(&f, &xf, Part::FootR));
                assert!(feet.abs() < 0.01, "hop at {t}: {feet}");
            }
            // Mid-hop the figure is off the ground; mid-step it looks at the car.
            sk.pose(&sk.hop(0.5, side), &mut xf);
            assert!(lowest(&f, &xf, Part::FootL) > 0.1);
            let head = sk.step_aside(0.5, side, look).rot[Part::Head as usize][1];
            assert!(head * look > 0.0);
            // The lead foot reaches out sideways mid-step.
            sk.pose(&sk.step_aside(0.3, side, look), &mut xf);
            let lead = if side > 0.0 { Part::FootR } else { Part::FootL };
            assert!(xf[lead as usize].pos[0] * side > sk.offset[Part::UpperLegR as usize][0] + 0.1);
        }
        assert!(aside_offset(1.0) == ASIDE_M && hop_offset(1.0) == HOP_M);
        let half = Joints::blend(&stand, &sk.hop(0.5, 1.0), 0.5);
        assert!(half.root[1] > stand.root[1]);
    }

    #[test]
    fn eras_dress_differently() {
        let mut bells = 0;
        let mut ties = 0;
        let mut headbands = [0; 2];
        for (e, era) in ERAS.into_iter().enumerate() {
            for seed in 0..200 {
                let s = FigureSpec::from_seed(seed, era);
                headbands[e] += s.headband as usize;
                if era == Era::Seventies && s.outfit == Outfit::JeansJacket {
                    bells += 1;
                }
                if era == Era::Seventies && s.outfit == Outfit::Suit {
                    ties += 1;
                }
            }
        }
        assert!(bells > 20 && ties > 10);
        assert_eq!(headbands[0], 0);
        assert!(headbands[1] > 5);
    }
}
