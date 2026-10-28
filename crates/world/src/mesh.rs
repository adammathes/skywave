//! World mesh data and low-poly builders.
//!
//! Builders append flat-shaded geometry (one normal per face, vertices not shared between
//! faces) unless they say otherwise. Front faces are counter-clockwise; builders that take
//! a desired facing fix the winding themselves, so callers never have to think about it.

use alloc::vec::Vec;

use crate::math::{abs, signed_area, sin_cos, TAU, V2, V3};

pub mod terrain;
use crate::palette::Rgb;

/// One vertex of a world mesh (40 bytes, `repr(C)`, no padding).
///
/// - `color`: linear RGB in unorm8; alpha holds the [`Pattern`] id so a future texture
///   atlas can pick the right tile.
/// - `uv`: surface coordinates in meters (walls: along the wall and up; flat surfaces: world
///   X and Z; slopes: along the horizontal and up the slope), so a pattern tiles at its
///   real-world size.
/// - `emissive`: linear RGB light the surface emits; alpha holds the [`Glow`] class so the
///   renderer can scale windows, neon and street lights independently.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    pub emissive: [u8; 4],
}

/// Indexed triangle list, counter-clockwise front faces, 32-bit indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMesh {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
}

impl WorldMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Surface pattern of a vertex, stored in `color[3]`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Plain = 0,
    Stucco = 1,
    Brick = 2,
    Concrete = 3,
    Asphalt = 4,
    Glass = 5,
    RoofGravel = 6,
    Lawn = 7,
    Awning = 8,
    Foliage = 9,
    Bark = 10,
    Metal = 11,
    Paint = 12,
    RoofTile = 13,
    /// Natural ground: sand, grass, scrub, soil (vertex colored, no tile).
    Terrain = 14,
    /// Open water; a future wave shader keys on it.
    Water = 15,
}

/// What kind of light an emissive surface is, stored in `emissive[3]`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glow {
    None = 0,
    /// Lit windows: scaled by the time-of-day window factor.
    Window = 1,
    /// Neon and sign light boxes.
    Neon = 2,
    /// Street lamps, on from dusk to dawn.
    Lamp = 3,
    /// Traffic signals, always on.
    Signal = 4,
}

/// Color, pattern and emission of a surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paint {
    pub color: [u8; 4],
    pub emissive: [u8; 4],
}

impl Paint {
    pub const fn new(rgb: Rgb, pattern: Pattern) -> Paint {
        Paint {
            color: [rgb[0], rgb[1], rgb[2], pattern as u8],
            emissive: [0, 0, 0, Glow::None as u8],
        }
    }

    pub const fn glowing(self, rgb: Rgb, glow: Glow) -> Paint {
        Paint {
            color: self.color,
            emissive: [rgb[0], rgb[1], rgb[2], glow as u8],
        }
    }

    pub fn rgb(&self) -> Rgb {
        [self.color[0], self.color[1], self.color[2]]
    }

    pub fn rgb_emissive(&self) -> Rgb {
        [self.emissive[0], self.emissive[1], self.emissive[2]]
    }

    pub fn with_rgb(self, rgb: Rgb) -> Paint {
        Paint {
            color: [rgb[0], rgb[1], rgb[2], self.color[3]],
            emissive: self.emissive,
        }
    }
}

/// A local coordinate frame: origin plus three orthonormal axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub o: V3,
    pub x: V3,
    pub y: V3,
    pub z: V3,
}

impl Frame {
    /// Axis-aligned frame at `o`.
    pub fn at(o: V3) -> Frame {
        Frame {
            o,
            x: V3::X,
            y: V3::Y,
            z: V3::Z,
        }
    }

    /// Upright frame at `o` with local X along the horizontal direction `dir` (unit), Y up
    /// and Z to the right of `dir` (the outward side of a counter-clockwise footprint edge).
    pub fn upright(o: V3, dir: V2) -> Frame {
        Frame {
            o,
            x: V3::new(dir.x, 0.0, dir.y),
            y: V3::Y,
            z: V3::new(-dir.y, 0.0, dir.x),
        }
    }

    pub fn p(&self, x: f32, y: f32, z: f32) -> V3 {
        self.o + self.x * x + self.y * y + self.z * z
    }

    pub fn v(&self, x: f32, y: f32, z: f32) -> V3 {
        self.x * x + self.y * y + self.z * z
    }

    /// The same frame moved by a local offset.
    pub fn offset(&self, x: f32, y: f32, z: f32) -> Frame {
        Frame {
            o: self.p(x, y, z),
            ..*self
        }
    }
}

/// Surfaces whose normal has a vertical component in this range (slopes between about 11°
/// and 78°) get slope UVs from [`planar_uv`].
const SLOPE_MIN_NY: f32 = 0.2;
const SLOPE_MAX_NY: f32 = 0.98;

fn vertex(pos: V3, normal: V3, uv: [f32; 2], paint: Paint) -> WorldVertex {
    WorldVertex {
        pos: pos.to_array(),
        normal: normal.to_array(),
        uv,
        color: paint.color,
        emissive: paint.emissive,
    }
}

/// Planar UVs in meters. Slopes (roofs) run u along the horizontal and v up the slope in
/// true length, so courses follow the eaves; other surfaces project along the dominant
/// axis of `n`.
fn planar_uv(p: V3, n: V3) -> [f32; 2] {
    let n = n.normalize();
    if (SLOPE_MIN_NY..SLOPE_MAX_NY).contains(&abs(n.y)) {
        let up = (V3::Y - n * n.y).normalize();
        let along = up.cross(n);
        return [p.dot(along), p.dot(up)];
    }
    let (ax, ay, az) = (abs(n.x), abs(n.y), abs(n.z));
    if ay >= ax && ay >= az {
        [p.x, p.z]
    } else if ax >= az {
        [p.z, p.y]
    } else {
        [p.x, p.y]
    }
}

/// Appends a flat triangle; the winding is flipped if needed so it faces along `facing`
/// (pass `V3::ZERO` to keep the given order). Degenerate triangles are skipped.
pub fn tri(m: &mut WorldMesh, a: V3, b: V3, c: V3, facing: V3, paint: Paint) {
    let n = (b - a).cross(c - a);
    let len = n.length();
    if len < 1e-9 {
        return;
    }
    let (b, c, n) = if n.dot(facing) < 0.0 {
        (c, b, -n)
    } else {
        (b, c, n)
    };
    let n = n * (1.0 / len);
    let base = m.vertices.len() as u32;
    for p in [a, b, c] {
        m.vertices.push(vertex(p, n, planar_uv(p, n), paint));
    }
    m.indices.extend_from_slice(&[base, base + 1, base + 2]);
}

/// Appends a flat quad `a, b, c, d` (a closed loop), facing along `facing` (or as wound if
/// `facing` is zero), with planar UVs. Degenerate quads are skipped.
pub fn quad(m: &mut WorldMesh, q: [V3; 4], facing: V3, paint: Paint) {
    let n = (q[2] - q[0]).cross(q[3] - q[1]);
    quad_uv(m, q, facing, paint, q.map(|p| planar_uv(p, n)));
}

/// [`quad`] with explicit UVs per corner.
pub fn quad_uv(m: &mut WorldMesh, q: [V3; 4], facing: V3, paint: Paint, uv: [[f32; 2]; 4]) {
    let n = (q[2] - q[0]).cross(q[3] - q[1]);
    let len = n.length();
    if len < 1e-9 {
        return;
    }
    let (order, n) = if n.dot(facing) < 0.0 {
        ([0, 3, 2, 1], -n)
    } else {
        ([0, 1, 2, 3], n)
    };
    let n = n * (1.0 / len);
    let base = m.vertices.len() as u32;
    for i in order {
        m.vertices.push(vertex(q[i], n, uv[i], paint));
    }
    m.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// A flat quad with a color per corner (for vertex-color noise on ground and asphalt);
/// UVs are planar.
pub fn quad_vc(m: &mut WorldMesh, q: [V3; 4], facing: V3, paint: Paint, colors: [Rgb; 4]) {
    let n = (q[2] - q[0]).cross(q[3] - q[1]);
    let len = n.length();
    if len < 1e-9 {
        return;
    }
    let (order, n) = if n.dot(facing) < 0.0 {
        ([0, 3, 2, 1], -n)
    } else {
        ([0, 1, 2, 3], n)
    };
    let n = n * (1.0 / len);
    let base = m.vertices.len() as u32;
    for i in order {
        m.vertices.push(vertex(
            q[i],
            n,
            planar_uv(q[i], n),
            paint.with_rgb(colors[i]),
        ));
    }
    m.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// A flat triangle with a color per corner, facing along `facing`.
pub fn tri_vc(m: &mut WorldMesh, t: [V3; 3], facing: V3, paint: Paint, colors: [Rgb; 3]) {
    let n = (t[1] - t[0]).cross(t[2] - t[0]);
    let len = n.length();
    if len < 1e-9 {
        return;
    }
    let (order, n) = if n.dot(facing) < 0.0 {
        ([0, 2, 1], -n)
    } else {
        ([0, 1, 2], n)
    };
    let n = n * (1.0 / len);
    let base = m.vertices.len() as u32;
    for i in order {
        m.vertices.push(vertex(
            t[i],
            n,
            planar_uv(t[i], n),
            paint.with_rgb(colors[i]),
        ));
    }
    m.indices.extend_from_slice(&[base, base + 1, base + 2]);
}

/// A band around a counter-clockwise polygon pushed out by `offset` (mitered), from `y0`
/// to `y1`: outward walls, plus optional top and bottom ledges back to the polygon.
#[allow(clippy::too_many_arguments)]
pub fn ring_band(
    m: &mut WorldMesh,
    poly: &[V2],
    offset: f32,
    y0: f32,
    y1: f32,
    side: Paint,
    top: Option<Paint>,
    bottom: Option<Paint>,
) {
    let outer = crate::math::offset_polygon(poly, |_| offset);
    let n = poly.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b) = (outer[i], outer[j]);
        let out = (b - a).right().at(0.0);
        let len = (b - a).length();
        quad_uv(
            m,
            [a.at(y0), b.at(y0), b.at(y1), a.at(y1)],
            out,
            side,
            [[0.0, y0], [len, y0], [len, y1], [0.0, y1]],
        );
        if let Some(p) = top {
            quad(
                m,
                [poly[i].at(y1), poly[j].at(y1), b.at(y1), a.at(y1)],
                V3::Y,
                p,
            );
        }
        if let Some(p) = bottom {
            quad(
                m,
                [poly[i].at(y0), poly[j].at(y0), b.at(y0), a.at(y0)],
                -V3::Y,
                p,
            );
        }
    }
}

/// A rectangle in the frame's XY plane at depth `z`, facing +Z (or -Z if `back`), with
/// UVs in local meters.
#[allow(clippy::too_many_arguments)]
pub fn rect(
    m: &mut WorldMesh,
    f: &Frame,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    z: f32,
    back: bool,
    paint: Paint,
) {
    if x1 - x0 <= 1e-4 || y1 - y0 <= 1e-4 {
        return;
    }
    let q = [
        f.p(x0, y0, z),
        f.p(x1, y0, z),
        f.p(x1, y1, z),
        f.p(x0, y1, z),
    ];
    let facing = if back { -f.z } else { f.z };
    quad_uv(
        m,
        q,
        facing,
        paint,
        [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
    );
}

/// Faces of a box, for [`boxf`]'s `faces` mask.
pub mod face {
    pub const NEG_X: u8 = 1;
    pub const POS_X: u8 = 2;
    pub const NEG_Y: u8 = 4;
    pub const POS_Y: u8 = 8;
    pub const NEG_Z: u8 = 16;
    pub const POS_Z: u8 = 32;
    pub const ALL: u8 = 63;
    /// Everything but the bottom, for boxes standing on something.
    pub const NO_BOTTOM: u8 = ALL & !NEG_Y;
}

/// A box spanning `lo..hi` in frame coordinates, with the faces in `faces`.
pub fn boxf(m: &mut WorldMesh, f: &Frame, lo: [f32; 3], hi: [f32; 3], faces: u8, paint: Paint) {
    let p = |x: usize, y: usize, z: usize| {
        f.p(
            if x == 0 { lo[0] } else { hi[0] },
            if y == 0 { lo[1] } else { hi[1] },
            if z == 0 { lo[2] } else { hi[2] },
        )
    };
    let sides: [(u8, V3, [V3; 4]); 6] = [
        (
            face::NEG_X,
            -f.x,
            [p(0, 0, 0), p(0, 0, 1), p(0, 1, 1), p(0, 1, 0)],
        ),
        (
            face::POS_X,
            f.x,
            [p(1, 0, 0), p(1, 1, 0), p(1, 1, 1), p(1, 0, 1)],
        ),
        (
            face::NEG_Y,
            -f.y,
            [p(0, 0, 0), p(1, 0, 0), p(1, 0, 1), p(0, 0, 1)],
        ),
        (
            face::POS_Y,
            f.y,
            [p(0, 1, 0), p(0, 1, 1), p(1, 1, 1), p(1, 1, 0)],
        ),
        (
            face::NEG_Z,
            -f.z,
            [p(0, 0, 0), p(0, 1, 0), p(1, 1, 0), p(1, 0, 0)],
        ),
        (
            face::POS_Z,
            f.z,
            [p(0, 0, 1), p(1, 0, 1), p(1, 1, 1), p(0, 1, 1)],
        ),
    ];
    for (bit, n, q) in sides {
        if faces & bit != 0 {
            quad(m, q, n, paint);
        }
    }
}

/// A frustum along the frame's Y axis from radius `r0` at `y = 0` to `r1` at `y = h`, with
/// `sides` faces. `smooth` gives radial normals (wheels, trunks seen up close); caps are
/// added where the radius is non-zero and `caps` is set.
#[allow(clippy::too_many_arguments)]
pub fn frustum(
    m: &mut WorldMesh,
    f: &Frame,
    r0: f32,
    r1: f32,
    h: f32,
    sides: u32,
    caps: bool,
    smooth: bool,
    paint: Paint,
) {
    lathe(m, f, &[[r0, 0.0], [r1, h]], sides, caps, smooth, paint);
}

/// A cylinder of radius `r` and height `h` along the frame's Y axis with `sides` faces.
#[allow(clippy::too_many_arguments)]
pub fn cylinder(
    m: &mut WorldMesh,
    f: &Frame,
    r: f32,
    h: f32,
    sides: u32,
    caps: bool,
    smooth: bool,
    paint: Paint,
) {
    lathe(m, f, &[[r, 0.0], [r, h]], sides, caps, smooth, paint);
}

/// A surface of revolution around the frame's Y axis through the profile points
/// `[radius, y]` (bottom to top), with `sides` faces around.
#[allow(clippy::too_many_arguments)]
pub fn lathe(
    m: &mut WorldMesh,
    f: &Frame,
    profile: &[[f32; 2]],
    sides: u32,
    caps: bool,
    smooth: bool,
    paint: Paint,
) {
    let ring = |k: u32| {
        let (s, c) = sin_cos(TAU * k as f32 / sides as f32);
        f.x * c + f.z * s
    };
    for seg in profile.windows(2) {
        let ([ra, ya], [rb, yb]) = (seg[0], seg[1]);
        for k in 0..sides {
            let (d0, d1) = (ring(k), ring(k + 1));
            let q = [
                f.o + d0 * ra + f.y * ya,
                f.o + d1 * ra + f.y * ya,
                f.o + d1 * rb + f.y * yb,
                f.o + d0 * rb + f.y * yb,
            ];
            let mid = (d0 + d1) * 0.5;
            if !smooth {
                if ra == 0.0 {
                    tri(m, q[0], q[2], q[3], mid, paint);
                } else if rb == 0.0 {
                    tri(m, q[0], q[1], q[2], mid, paint);
                } else {
                    quad(m, q, mid, paint);
                }
                continue;
            }
            // Radial normals tilted by the profile slope.
            let slope = (ra - rb) / (yb - ya).max(1e-6);
            let (n0, n1) = (
                (d0 + f.y * slope).normalize(),
                (d1 + f.y * slope).normalize(),
            );
            let normals = [n0, n1, n1, n0];
            let (u0, u1) = (k as f32 / sides as f32, (k + 1) as f32 / sides as f32);
            let uv = [[u0, ya], [u1, ya], [u1, yb], [u0, yb]];
            let flat = (q[2] - q[0]).cross(q[3] - q[1]);
            let order = if flat.dot(mid) >= 0.0 {
                [0, 1, 2, 3]
            } else {
                [0, 3, 2, 1]
            };
            let base = m.vertices.len() as u32;
            for i in order {
                m.vertices.push(vertex(q[i], normals[i], uv[i], paint));
            }
            for t in [[0, 1, 2], [0, 2, 3]] {
                let [a, b, c] = t.map(|i| q[order[i]]);
                if (b - a).cross(c - a).length() > 1e-9 {
                    m.indices.extend(t.map(|i| base + i as u32));
                }
            }
        }
    }
    if caps {
        let (first, last) = (profile[0], profile[profile.len() - 1]);
        for ([r, y], up) in [(first, -1.0f32), (last, 1.0)] {
            if r <= 0.0 {
                continue;
            }
            let c = f.o + f.y * y;
            for k in 1..sides.saturating_sub(1) {
                let (d0, d1, d2) = (ring(0), ring(k), ring(k + 1));
                tri(m, c + d0 * r, c + d1 * r, c + d2 * r, f.y * up, paint);
            }
        }
    }
}

/// Fills a simple polygon (any winding) at height `y`, facing up or down.
pub fn polygon_cap(m: &mut WorldMesh, poly: &[V2], y: f32, up: bool, paint: Paint) {
    let facing = if up { V3::Y } else { -V3::Y };
    let mut tris = Vec::new();
    triangulate(poly, &mut tris);
    for [a, b, c] in tris {
        tri(
            m,
            poly[a].at(y),
            poly[b].at(y),
            poly[c].at(y),
            facing,
            paint,
        );
    }
}

/// Extrudes a counter-clockwise polygon from `y0` to `y1`: outward walls plus an optional
/// top cap.
pub fn extrude_polygon(
    m: &mut WorldMesh,
    poly: &[V2],
    y0: f32,
    y1: f32,
    walls: Paint,
    top: Option<Paint>,
) {
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let out = (b - a).right();
        let q = [a.at(y0), b.at(y0), b.at(y1), a.at(y1)];
        let len = (b - a).length();
        quad_uv(
            m,
            q,
            out.at(0.0),
            walls,
            [[0.0, y0], [len, y0], [len, y1], [0.0, y1]],
        );
    }
    if let Some(paint) = top {
        polygon_cap(m, poly, y1, true, paint);
    }
}

/// The triangles of a unit icosphere: an icosahedron subdivided `subdiv` times
/// (20 * 4^subdiv triangles), built once and reused by [`icosphere`].
pub struct UnitSphere {
    pub tris: Vec<[V3; 3]>,
}

impl UnitSphere {
    pub fn new(subdiv: u32) -> UnitSphere {
        const T: f32 = 1.618_034;
        let base: [V3; 12] = [
            V3::new(-1.0, T, 0.0),
            V3::new(1.0, T, 0.0),
            V3::new(-1.0, -T, 0.0),
            V3::new(1.0, -T, 0.0),
            V3::new(0.0, -1.0, T),
            V3::new(0.0, 1.0, T),
            V3::new(0.0, -1.0, -T),
            V3::new(0.0, 1.0, -T),
            V3::new(T, 0.0, -1.0),
            V3::new(T, 0.0, 1.0),
            V3::new(-T, 0.0, -1.0),
            V3::new(-T, 0.0, 1.0),
        ];
        // Counter-clockwise seen from outside.
        const FACES: [[usize; 3]; 20] = [
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];
        let mut tris: Vec<[V3; 3]> = FACES
            .iter()
            .map(|f| {
                [
                    base[f[0]].normalize(),
                    base[f[1]].normalize(),
                    base[f[2]].normalize(),
                ]
            })
            .collect();
        for _ in 0..subdiv {
            let mut next = Vec::with_capacity(tris.len() * 4);
            for [a, b, c] in tris {
                let ab = (a + b).normalize();
                let bc = (b + c).normalize();
                let ca = (c + a).normalize();
                next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
            }
            tris = next;
        }
        UnitSphere { tris }
    }
}

/// An ellipsoid from a [`UnitSphere`]. `smooth` uses radial normals. `shade` maps a unit
/// normal to the vertex color, so canopies can be darker underneath.
pub fn icosphere(
    m: &mut WorldMesh,
    sphere: &UnitSphere,
    center: V3,
    radii: V3,
    smooth: bool,
    paint: Paint,
    shade: impl Fn(V3) -> Rgb,
) {
    let place = |u: V3| center + V3::new(u.x * radii.x, u.y * radii.y, u.z * radii.z);
    let inv = V3::new(1.0 / radii.x, 1.0 / radii.y, 1.0 / radii.z);
    for &[a, b, c] in &sphere.tris {
        let (pa, pb, pc) = (place(a), place(b), place(c));
        if smooth {
            let base = m.vertices.len() as u32;
            for (u, p) in [(a, pa), (b, pb), (c, pc)] {
                let n = V3::new(u.x * inv.x, u.y * inv.y, u.z * inv.z).normalize();
                let pt = paint.with_rgb(shade(n));
                m.vertices.push(vertex(p, n, [u.x, u.y], pt));
            }
            m.indices.extend_from_slice(&[base, base + 1, base + 2]);
        } else {
            let n = (a + b + c).normalize();
            tri(m, pa, pb, pc, n, paint.with_rgb(shade(n)));
        }
    }
}

/// Ear-clipping triangulation of a simple polygon (either winding). Pushes index triples
/// into `out`, each counter-clockwise seen from above.
pub fn triangulate(poly: &[V2], out: &mut Vec<[usize; 3]>) {
    let n = poly.len();
    if n < 3 {
        return;
    }
    let ccw = signed_area(poly) >= 0.0;
    let mut idx: Vec<usize> = if ccw {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    let mut guard = 0;
    while idx.len() > 3 && guard < 4 * n * n {
        guard += 1;
        let k = idx.len();
        let mut clipped = false;
        for i in 0..k {
            let (ia, ib, ic) = (idx[(i + k - 1) % k], idx[i], idx[(i + 1) % k]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            // Convex corner (counter-clockwise seen from above means a left turn).
            if (b - a).turn(c - b) <= 0.0 {
                continue;
            }
            let inside = idx
                .iter()
                .any(|&j| j != ia && j != ib && j != ic && in_triangle(poly[j], a, b, c));
            if inside {
                continue;
            }
            out.push([ia, ib, ic]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break; // Not simple; emit what is left as a fan.
        }
    }
    for i in 1..idx.len().saturating_sub(1) {
        out.push([idx[0], idx[i], idx[i + 1]]);
    }
}

fn in_triangle(p: V2, a: V2, b: V2, c: V2) -> bool {
    let s1 = (b - a).turn(p - a);
    let s2 = (c - b).turn(p - b);
    let s3 = (a - c).turn(p - c);
    s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Every triangle's winding agrees with its vertex normals (or, for smooth meshes,
    /// roughly agrees).
    pub fn assert_well_formed(m: &WorldMesh, min_dot: f32) {
        assert_eq!(m.indices.len() % 3, 0);
        for tri in m.indices.chunks_exact(3) {
            let v = [tri[0], tri[1], tri[2]].map(|i| m.vertices[i as usize]);
            let [a, b, c] = v.map(|v| V3::new(v.pos[0], v.pos[1], v.pos[2]));
            let face = (b - a).cross(c - a).normalize();
            for vert in v {
                let n = V3::new(vert.normal[0], vert.normal[1], vert.normal[2]);
                assert!((n.length() - 1.0).abs() < 1e-4, "{n:?}");
                assert!(
                    face.dot(n) > min_dot,
                    "winding disagrees: {face:?} vs {n:?}"
                );
            }
        }
    }

    const P: Paint = Paint::new([200, 100, 50], Pattern::Plain);

    #[test]
    fn box_is_closed_and_outward() {
        let mut m = WorldMesh::default();
        let f = Frame::upright(V3::new(1.0, 0.0, 2.0), V2::new(0.6, 0.8));
        boxf(&mut m, &f, [-1.0, 0.0, -0.5], [1.0, 2.0, 0.5], face::ALL, P);
        assert_eq!(m.triangle_count(), 12);
        assert_well_formed(&m, 0.999);
        let center = f.p(0.0, 1.0, 0.0);
        for v in &m.vertices {
            let p = V3::new(v.pos[0], v.pos[1], v.pos[2]);
            assert!((p - center).dot(V3::new(v.normal[0], v.normal[1], v.normal[2])) > 0.0);
        }
    }

    #[test]
    fn frustum_lathe_and_sphere_are_well_formed() {
        let mut m = WorldMesh::default();
        let f = Frame::at(V3::new(0.0, 1.0, 0.0));
        frustum(&mut m, &f, 1.0, 0.5, 2.0, 8, true, false, P);
        assert_eq!(m.triangle_count(), 8 * 2 + 6 * 2);
        assert_well_formed(&m, 0.999);
        let mut cyl = WorldMesh::default();
        cylinder(&mut cyl, &f, 0.5, 2.0, 6, true, true, P);
        assert_eq!(cyl.triangle_count(), 6 * 2 + 4 * 2);
        assert_well_formed(&cyl, 0.5);
        let mut cone = WorldMesh::default();
        frustum(&mut cone, &f, 1.0, 0.0, 1.0, 6, true, false, P);
        assert_eq!(cone.triangle_count(), 6 + 4);
        assert_well_formed(&cone, 0.999);
        let mut s = WorldMesh::default();
        icosphere(
            &mut s,
            &UnitSphere::new(1),
            V3::ZERO,
            V3::new(1.0, 0.8, 1.0),
            true,
            P,
            |_| [1, 2, 3],
        );
        assert_eq!(s.triangle_count(), 80);
        assert_well_formed(&s, 0.7);
        let mut smooth = WorldMesh::default();
        lathe(
            &mut smooth,
            &f,
            &[[0.5, 0.0], [0.4, 1.0], [0.0, 1.5]],
            6,
            false,
            true,
            P,
        );
        assert_well_formed(&smooth, 0.3);
    }

    #[test]
    fn slopes_get_uvs_along_the_eave_and_up_the_slope() {
        // A roof plane rising 2 m over 4 m toward +X, eave along Z.
        let mut m = WorldMesh::default();
        let q = [
            V3::new(0.0, 3.0, 5.0),
            V3::new(0.0, 3.0, 0.0),
            V3::new(4.0, 5.0, 0.0),
            V3::new(4.0, 5.0, 5.0),
        ];
        quad(&mut m, q, V3::new(-1.0, 2.0, 0.0), P);
        let at = |p: V3| {
            let v = m.vertices.iter().find(|v| v.pos == p.to_array()).unwrap();
            v.uv
        };
        let (eave, ridge) = (at(q[1]), at(q[2]));
        // Up the slope is +v by the slope's true length; the eave is constant v.
        assert!((ridge[1] - eave[1] - 20f32.sqrt()).abs() < 1e-4);
        assert!((at(q[0])[1] - eave[1]).abs() < 1e-5);
        assert!(((at(q[0])[0] - eave[0]).abs() - 5.0).abs() < 1e-4);
        // Walls keep their projection.
        let mut w = WorldMesh::default();
        let wall = [[0.0, 0.0], [1.0, 0.0], [1.0, 2.0], [0.0, 2.0]];
        quad(&mut w, wall.map(|[x, y]| V3::new(x, y, 1.0)), V3::Z, P);
        assert!(w.vertices.iter().all(|v| v.uv == [v.pos[0], v.pos[1]]));
    }

    #[test]
    fn triangulate_concave_polygon() {
        // An L shape, clockwise and counter-clockwise.
        let l = [
            V2::new(0.0, 0.0),
            V2::new(0.0, 2.0),
            V2::new(2.0, 2.0),
            V2::new(2.0, 1.0),
            V2::new(1.0, 1.0),
            V2::new(1.0, 0.0),
        ];
        for poly in [l.to_vec(), l.iter().rev().copied().collect()] {
            let mut out = Vec::new();
            triangulate(&poly, &mut out);
            assert_eq!(out.len(), 4);
            let area: f32 = out
                .iter()
                .map(|t| signed_area(&[poly[t[0]], poly[t[1]], poly[t[2]]]))
                .sum();
            assert!((area - 3.0).abs() < 1e-5, "{area}");
        }
        let mut m = WorldMesh::default();
        extrude_polygon(&mut m, &l, 0.0, 3.0, P, Some(P));
        assert_eq!(m.triangle_count(), 12 + 4);
        assert_well_formed(&m, 0.999);
    }
}
