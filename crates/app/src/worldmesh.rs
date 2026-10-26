//! Puts the pattern atlas on the town: maps each world [`Pattern`] to an atlas [`Tile`] at
//! a real-world scale and turns the world's meter uvs into atlas uvs.
//!
//! The atlas cannot wrap (its tiles share one texture), so a repeating pattern repeats by
//! geometry: each triangle (or each quad, when the world emitted one) is cut along the
//! integer lines of its pattern's grid (`uv / period`), and every piece maps its grid cell
//! onto one copy of the tile. Cutting works on any planar polygon, so walls, roofs, roads
//! and lot polygons all tile without stretching. Points on an edge are interpolated from
//! the edge's ends in a fixed order, so faces that share an edge and a grid (the two
//! triangles of a quad) cut it at bit-identical points.
//!
//! Periods are chosen for the town's triangle budget: patterns with a recognisable size
//! (brick, shingles, windows) are close to real-world scale; noise patterns (stucco,
//! concrete, asphalt, lawn, gravel), whose scale the eye cannot judge, use large cells.
//!
//! Colors: tintable tiles multiply the vertex color, scaled by the inverse of the tile's
//! mean brightness so a surface keeps the brightness it had untextured. Pre-colored tiles
//! (brick, window glass) take only the vertex color's brightness.

use render::Tile;
use world::{MeshKind, Pattern, WorldMesh, WorldVertex};

/// How a pattern lays its tile onto a surface.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fit {
    /// The white tile: no pattern, no cutting.
    Plain,
    /// One copy of the tile every `period` meters (u, v). Each face starts its grid at its
    /// own corner, moved back to a multiple of `step` (in tiles; 0 for anywhere), so faces
    /// are cut as little as possible. Where `step` is the pattern's own repeat (a brick and
    /// two courses), the pattern still lines up exactly across neighbouring faces; noise
    /// patterns just get a seam at face edges, which their noise hides.
    Repeat { period: [f32; 2], step: [f32; 2] },
    /// One window per face: the whole tile on window-sized faces, one pane of it stretched
    /// over plate glass and small lights.
    Window,
}

/// How the vertex color combines with the tile.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tint {
    /// rgb × gain.
    Color(f32),
    /// Luminance × gain, as gray.
    Gray(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    tile: Tile,
    fit: Fit,
    tint: Tint,
}

/// Inverse mean brightness (linear luminance) of the tintable tiles, so tinted surfaces
/// average to their vertex color. Checked against the generated atlas in the tests.
const GAIN_STUCCO: f32 = 1.16;
const GAIN_CONCRETE: f32 = 1.19;
const GAIN_ASPHALT: f32 = 1.36;
const GAIN_ROOF_GRAVEL: f32 = 1.54;
const GAIN_LAWN: f32 = 1.35;
const GAIN_WOOD: f32 = 1.28;
const GAIN_SHINGLES: f32 = 1.55;
/// Inverse mean luminance of the brick tiles (pre-colored).
const GAIN_BRICK_RED: f32 = 4.22;
const GAIN_BRICK_TAN: f32 = 2.40;
/// Inverse mean luminance of a window pane (frame excluded), so the glass keeps the
/// brightness of its vertex color and the frame comes out lighter.
const GAIN_GLASS: f32 = 4.12;

/// Window-sized faces (both sides within this range, meters) show the whole window tile.
const WINDOW_SIDE_M: (f32, f32) = (0.6, 2.6);
/// One pane of [`Tile::WindowGlass`], as fractions of the tile: inside the frame, left of
/// the mullion and above the transom.
const PANE: [f32; 4] = [0.07, 0.07, 0.46, 0.47];

/// A face narrower than this many cells counts as a thin strip, and a noise pattern on it
/// repeats `THIN_SCALE` times larger.
const THIN_CELLS: f32 = 0.25;
const THIN_SCALE: f32 = 3.0;

/// A brick color counts as red (rather than brown or tan) when red exceeds this many
/// times green.
const RED_BRICK_RATIO: u32 = 4;

const PLAIN: Look = Look {
    tile: Tile::White,
    fit: Fit::Plain,
    tint: Tint::Color(1.0),
};

/// The atlas look of a surface with `pattern` and color `rgb` in a mesh of `kind`.
fn look(pattern: u8, rgb: [u8; 3], kind: MeshKind) -> Look {
    let repeat = |tile, period, step, gain| Look {
        tile,
        fit: Fit::Repeat { period, step },
        tint: Tint::Color(gain),
    };
    match pattern {
        p if p == Pattern::Stucco as u8 => repeat(Tile::Stucco, [4.0; 2], [0.0; 2], GAIN_STUCCO),
        p if p == Pattern::Brick as u8 => {
            // Running bond, 25 cm bricks in 8.75 cm courses.
            let (tile, gain) = if u32::from(rgb[0]) > RED_BRICK_RATIO * u32::from(rgb[1]) {
                (Tile::BrickRed, GAIN_BRICK_RED)
            } else {
                (Tile::BrickTan, GAIN_BRICK_TAN)
            };
            Look {
                tile,
                fit: Fit::Repeat {
                    period: [1.0, 0.7],
                    step: [0.25, 0.25],
                },
                tint: Tint::Gray(gain),
            }
        }
        p if p == Pattern::Concrete as u8 => {
            repeat(Tile::Concrete, [8.0; 2], [0.0; 2], GAIN_CONCRETE)
        }
        p if p == Pattern::Asphalt as u8 => repeat(Tile::Asphalt, [8.0; 2], [0.0; 2], GAIN_ASPHALT),
        p if p == Pattern::RoofGravel as u8 => {
            repeat(Tile::RoofGravel, [6.0; 2], [0.0; 2], GAIN_ROOF_GRAVEL)
        }
        p if p == Pattern::Lawn as u8 => repeat(Tile::Lawn, [8.0; 2], [0.0; 2], GAIN_LAWN),
        // Shingle tabs 30 cm wide with 20 cm exposure, rows along the eaves.
        p if p == Pattern::RoofTile as u8 => {
            repeat(Tile::RoofShingles, [2.4, 1.6], [0.125, 0.25], GAIN_SHINGLES)
        }
        // Board-like rings on palm trunks, staves on water tanks.
        p if p == Pattern::Bark as u8 => {
            repeat(Tile::WoodSiding, [1.6, 2.4], [0.0, 0.125], GAIN_WOOD)
        }
        // Windows on buildings; car glass, lenses and water stay plain.
        p if p == Pattern::Glass as u8 && kind == MeshKind::Building => Look {
            tile: Tile::WindowGlass,
            fit: Fit::Window,
            tint: Tint::Gray(GAIN_GLASS),
        },
        // Awning stripes are geometry already; foliage, metal, paint and plain surfaces
        // are flat color.
        _ => PLAIN,
    }
}

/// `m` with atlas uvs in `[0, 1]` (instead of meters) and colors adjusted to the tiles,
/// its triangles cut so every repeating pattern repeats at its real-world size.
pub fn atlas_mesh(m: &WorldMesh, kind: MeshKind) -> WorldMesh {
    let mut out = WorldMesh {
        vertices: Vec::with_capacity(m.vertices.len() * 3 / 2),
        indices: Vec::with_capacity(m.indices.len() * 3 / 2),
    };
    let tris = &m.indices;
    let vert = |i: u32| m.vertices[i as usize];
    let mut k = 0;
    while k + 3 <= tris.len() {
        let (a, b, c) = (tris[k], tris[k + 1], tris[k + 2]);
        // A quad as the world builders emit it, (a, b, c) then (a, c, d): cut as one
        // polygon when it is a flat parallelogram, which halves the pieces along its
        // diagonal.
        if k + 6 <= tris.len() && tris[k + 3] == a && tris[k + 4] == c {
            let q = [vert(a), vert(b), vert(c), vert(tris[k + 5])];
            if mergeable(&q) {
                emit(&mut out, &q, kind);
                k += 6;
                continue;
            }
        }
        emit(&mut out, &[vert(a), vert(b), vert(c)], kind);
        k += 3;
    }
    out
}

/// Whether quad `q` is one flat parallelogram (in position and uv) of one surface.
fn mergeable(q: &[WorldVertex; 4]) -> bool {
    let same = q[1..].iter().all(|v| {
        v.normal == q[0].normal && v.emissive == q[0].emissive && v.color[3] == q[0].color[3]
    });
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-3 * (1.0 + a.abs().max(b.abs()));
    let pos = (0..3).all(|i| close(q[0].pos[i] + q[2].pos[i], q[1].pos[i] + q[3].pos[i]));
    let uv = (0..2).all(|i| close(q[0].uv[i] + q[2].uv[i], q[1].uv[i] + q[3].uv[i]));
    same && pos && uv
}

/// A polygon vertex while cutting: grid coordinates `s, t`, position, normal, color and
/// emissive rgb, all interpolated linearly.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cv([f32; 14]);

const S: usize = 0;
const T: usize = 1;
const POS: usize = 2;
const NORMAL: usize = 5;
const COLOR: usize = 8;
const EMISSIVE: usize = 11;

/// Most vertices a cut polygon can have: a quad clipped by four lines.
const MAX_POLY: usize = 8;

#[derive(Clone, Copy)]
struct Poly {
    v: [Cv; MAX_POLY],
    n: usize,
}

impl Poly {
    fn new() -> Poly {
        Poly {
            v: [Cv([0.0; 14]); MAX_POLY],
            n: 0,
        }
    }

    fn push(&mut self, v: Cv) {
        // Drop repeats (a vertex lying on a cut line comes out once).
        if self.n > 0 && self.v[self.n - 1].0[S] == v.0[S] && self.v[self.n - 1].0[T] == v.0[T] {
            return;
        }
        if self.n < MAX_POLY {
            self.v[self.n] = v;
            self.n += 1;
        }
    }

    /// Keeps the part where `sign * (coordinate axis - line) >= 0` (Sutherland–Hodgman).
    fn clip(&self, axis: usize, line: f32, sign: f32) -> Poly {
        let mut out = Poly::new();
        for i in 0..self.n {
            let (a, b) = (self.v[i], self.v[(i + 1) % self.n]);
            let (da, db) = (sign * (a.0[axis] - line), sign * (b.0[axis] - line));
            if da >= 0.0 {
                out.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                out.push(cut(a, b, axis, line));
            }
        }
        if out.n > 1
            && out.v[0].0[S] == out.v[out.n - 1].0[S]
            && out.v[0].0[T] == out.v[out.n - 1].0[T]
        {
            out.n -= 1;
        }
        out
    }
}

/// The point where edge `a`–`b` crosses `axis = line`, computed from the edge's ends in a
/// canonical order so both faces sharing the edge get the same bits.
fn cut(a: Cv, b: Cv, axis: usize, line: f32) -> Cv {
    let (p, q) = if before(&a, &b) { (a, b) } else { (b, a) };
    let f = (line - p.0[axis]) / (q.0[axis] - p.0[axis]);
    let mut r = Cv([0.0; 14]);
    for i in 0..14 {
        r.0[i] = p.0[i] + (q.0[i] - p.0[i]) * f;
    }
    r.0[axis] = line;
    r
}

fn before(a: &Cv, b: &Cv) -> bool {
    for i in 0..5 {
        if a.0[i] != b.0[i] {
            return a.0[i] < b.0[i];
        }
    }
    false
}

/// Linear luminance of a linear rgb in unorm8 units.
fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Grid cells a span `lo..hi` covers, ignoring slivers thinner than `EPS` cell past a line.
fn cells(lo: f32, hi: f32) -> (i32, i32) {
    const EPS: f32 = 1e-3;
    let i0 = (lo + EPS).floor() as i32;
    let i1 = ((hi - EPS).ceil() as i32).max(i0 + 1);
    (i0, i1)
}

/// Appends source polygon `src` (3 or 4 vertices, convex, counter-clockwise) with its
/// pattern applied.
fn emit(out: &mut WorldMesh, src: &[WorldVertex], kind: MeshKind) {
    let v0 = src[0];
    // The terrain apron beyond the last street is only ever seen from afar, where a pattern
    // mips to its mean anyway; patterning it would cost more triangles than the town.
    let apron = kind == MeshKind::Ground && src.iter().all(|v| v.pos[1] == world::TERRAIN_Y);
    let look = if apron {
        PLAIN
    } else {
        look(v0.color[3], [v0.color[0], v0.color[1], v0.color[2]], kind)
    };
    let mut poly = Poly::new();
    for v in src {
        let c = v.color.map(f32::from);
        let rgb = match look.tint {
            Tint::Color(g) => [c[0] * g, c[1] * g, c[2] * g],
            Tint::Gray(g) => [luminance([c[0], c[1], c[2]]) * g; 3],
        };
        let mut a = [0.0; 14];
        a[S] = v.uv[0];
        a[T] = v.uv[1];
        a[POS..POS + 3].copy_from_slice(&v.pos);
        a[NORMAL..NORMAL + 3].copy_from_slice(&v.normal);
        a[COLOR..COLOR + 3].copy_from_slice(&rgb);
        for i in 0..3 {
            a[EMISSIVE + i] = f32::from(v.emissive[i]);
        }
        poly.v[poly.n] = Cv(a);
        poly.n += 1;
    }
    let smooth = src.iter().any(|v| v.normal != v0.normal);
    let alpha = [v0.color[3], v0.emissive[3]];
    let full = look.tile.rect();
    let range = |axis: usize, p: &Poly| {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for v in &p.v[..p.n] {
            lo = lo.min(v.0[axis]);
            hi = hi.max(v.0[axis]);
        }
        (lo, hi)
    };
    match look.fit {
        Fit::Plain => {
            // Every vertex at the tile's center.
            let [u0, v0, u1, v1] = full;
            let (u, v) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
            push(out, &poly, [u, v, u, v], alpha, smooth);
        }
        Fit::Window => {
            let (s0, s1) = range(S, &poly);
            let (t0, t1) = range(T, &poly);
            let (w, h) = (s1 - s0, t1 - t0);
            let window = |x: f32| (WINDOW_SIDE_M.0..=WINDOW_SIDE_M.1).contains(&x);
            let rect = if window(w) && window(h) {
                full
            } else {
                let [u0, v0, u1, v1] = full;
                let (du, dv) = (u1 - u0, v1 - v0);
                [
                    u0 + du * PANE[0],
                    v0 + dv * PANE[1],
                    u0 + du * PANE[2],
                    v0 + dv * PANE[3],
                ]
            };
            for v in &mut poly.v[..poly.n] {
                v.0[S] = (v.0[S] - s0) / w.max(1e-6);
                v.0[T] = (v.0[T] - t0) / h.max(1e-6);
            }
            push(out, &poly, rect, alpha, smooth);
        }
        Fit::Repeat { period, step } => {
            let (a0, a1) = range(S, &poly);
            let (b0, b1) = range(T, &poly);
            // Thin strips (curbs, gutters, plinths) of a noise pattern use larger cells, so
            // their length is cut less often.
            let thin = ((a1 - a0) / period[0]).min((b1 - b0) / period[1]) < THIN_CELLS;
            let scale = if thin && step == [0.0; 2] {
                THIN_SCALE
            } else {
                1.0
            };
            for (axis, k) in [(S, 0), (T, 1)] {
                for v in &mut poly.v[..poly.n] {
                    v.0[axis] /= period[k] * scale;
                }
                let (lo, _) = range(axis, &poly);
                let origin = if step[k] > 0.0 {
                    (lo / step[k]).floor() * step[k]
                } else {
                    lo
                };
                for v in &mut poly.v[..poly.n] {
                    v.0[axis] -= origin;
                }
            }
            let (s0, s1) = range(S, &poly);
            let (t0, t1) = range(T, &poly);
            let (i0, i1) = cells(s0, s1);
            let (j0, j1) = cells(t0, t1);
            for i in i0..i1 {
                let mut col = poly;
                if i > i0 {
                    col = col.clip(S, i as f32, 1.0);
                }
                if i + 1 < i1 {
                    col = col.clip(S, (i + 1) as f32, -1.0);
                }
                if col.n < 3 {
                    continue;
                }
                for j in j0..j1 {
                    let mut cell = col;
                    if j > j0 {
                        cell = cell.clip(T, j as f32, 1.0);
                    }
                    if j + 1 < j1 {
                        cell = cell.clip(T, (j + 1) as f32, -1.0);
                    }
                    if cell.n < 3 {
                        continue;
                    }
                    for v in &mut cell.v[..cell.n] {
                        v.0[S] -= i as f32;
                        v.0[T] -= j as f32;
                    }
                    push(out, &cell, full, alpha, smooth);
                }
            }
        }
    }
}

/// Appends `poly` (cell coordinates in `[0, 1]`) as a triangle fan, mapping the cell onto
/// atlas rect `[u0, v0, u1, v1]` with v up the surface toward `v0`.
fn push(out: &mut WorldMesh, poly: &Poly, rect: [f32; 4], alpha: [u8; 2], smooth: bool) {
    let [u0, v0, u1, v1] = rect;
    let base = out.vertices.len() as u32;
    let byte = |x: f32| x.round().clamp(0.0, 255.0) as u8;
    for v in &poly.v[..poly.n] {
        let a = &v.0;
        let (s, t) = (a[S].clamp(0.0, 1.0), a[T].clamp(0.0, 1.0));
        let mut normal = [a[NORMAL], a[NORMAL + 1], a[NORMAL + 2]];
        if smooth {
            let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
                .sqrt()
                .max(1e-12);
            normal = normal.map(|x| x / len);
        }
        out.vertices.push(WorldVertex {
            pos: [a[POS], a[POS + 1], a[POS + 2]],
            normal,
            uv: [u0 + (u1 - u0) * s, v1 - (v1 - v0) * t],
            color: [
                byte(a[COLOR]),
                byte(a[COLOR + 1]),
                byte(a[COLOR + 2]),
                alpha[0],
            ],
            emissive: [
                byte(a[EMISSIVE]),
                byte(a[EMISSIVE + 1]),
                byte(a[EMISSIVE + 2]),
                alpha[1],
            ],
        });
    }
    let p = |i: usize| {
        let a = &poly.v[i].0;
        [a[POS], a[POS + 1], a[POS + 2]]
    };
    for i in 1..poly.n.saturating_sub(1) {
        let (a, b, c) = (p(0), p(i), p(i + 1));
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cr = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        if cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2] < 1e-14 {
            continue;
        }
        out.indices
            .extend_from_slice(&[base, base + i as u32, base + i as u32 + 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render::TextureAtlas;

    /// Whole test town after patterning (the world's own budget is lower).
    const TOWN_TRIANGLE_BUDGET: usize = 900_000;

    fn srgb_to_linear(c: u8) -> f32 {
        let s = f32::from(c) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }

    /// Mean linear luminance of `tile` over the tile fraction `[x0, y0, x1, y1]`.
    fn mean_luminance(atlas: &TextureAtlas, tile: Tile, frac: [f32; 4]) -> f32 {
        let (ox, oy) = tile.origin();
        let size = render::ATLAS_SIZE / render::TILES_PER_ROW;
        let r = |f: f32| (f * size as f32) as u32;
        let (mut sum, mut n) = (0.0, 0.0);
        for y in r(frac[1])..r(frac[3]) {
            for x in r(frac[0])..r(frac[2]) {
                let p = atlas.texel(0, ox + x, oy + y).map(srgb_to_linear);
                sum += luminance([p[0], p[1], p[2]]);
                n += 1.0;
            }
        }
        sum / n
    }

    #[test]
    fn gains_match_the_atlas() {
        let atlas = TextureAtlas::generate();
        let whole = [0.0, 0.0, 1.0, 1.0];
        for (tile, gain, frac) in [
            (Tile::Stucco, GAIN_STUCCO, whole),
            (Tile::Concrete, GAIN_CONCRETE, whole),
            (Tile::Asphalt, GAIN_ASPHALT, whole),
            (Tile::RoofGravel, GAIN_ROOF_GRAVEL, whole),
            (Tile::Lawn, GAIN_LAWN, whole),
            (Tile::WoodSiding, GAIN_WOOD, whole),
            (Tile::RoofShingles, GAIN_SHINGLES, whole),
            (Tile::BrickRed, GAIN_BRICK_RED, whole),
            (Tile::BrickTan, GAIN_BRICK_TAN, whole),
            (Tile::WindowGlass, GAIN_GLASS, PANE),
        ] {
            let mean = mean_luminance(&atlas, tile, frac);
            std::println!("{tile:?}: mean {mean:.4} gain {:.3}", 1.0 / mean);
            assert!((gain * mean - 1.0).abs() < 0.01, "{tile:?}: {}", 1.0 / mean);
        }
    }

    fn wv(pos: [f32; 3], uv: [f32; 2], pattern: Pattern) -> WorldVertex {
        WorldVertex {
            pos,
            normal: [0.0, 0.0, 1.0],
            uv,
            color: [200, 150, 100, pattern as u8],
            emissive: [0; 4],
        }
    }

    /// A wall quad `w × h` meters facing +Z, as the world builders emit it.
    fn wall(w: f32, h: f32, pattern: Pattern) -> WorldMesh {
        WorldMesh {
            vertices: [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
                .map(|[x, y]| wv([x, y, 0.0], [x, y], pattern))
                .to_vec(),
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    fn area(m: &WorldMesh) -> f32 {
        m.indices
            .chunks_exact(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize].pos);
                // Signed area in the XY plane: positive when counter-clockwise seen from +Z.
                0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]))
            })
            .sum()
    }

    fn inside(uv: [f32; 2], r: [f32; 4]) -> bool {
        uv[0] >= r[0] - 1e-6 && uv[0] <= r[2] + 1e-6 && uv[1] >= r[1] - 1e-6 && uv[1] <= r[3] + 1e-6
    }

    #[test]
    fn walls_tile_at_the_pattern_period() {
        // A wall 2.5 × 1.5 stucco periods is 3 × 2 cells, the last ones partial.
        let p = match look(Pattern::Stucco as u8, [0; 3], MeshKind::Building).fit {
            Fit::Repeat { period, .. } => period[0],
            other => panic!("{other:?}"),
        };
        let m = atlas_mesh(&wall(2.5 * p, 1.5 * p, Pattern::Stucco), MeshKind::Building);
        assert!((area(&m) - 3.75 * p * p).abs() < 1e-3, "{}", area(&m));
        assert_eq!(m.triangle_count(), 3 * 2 * 2);
        let r = Tile::Stucco.rect();
        for v in &m.vertices {
            assert!(inside(v.uv, r), "{:?}", v.uv);
            assert_eq!(v.color[3], Pattern::Stucco as u8);
            // Tinted by the vertex color, brightened by the tile's gain.
            assert_eq!(v.color[0], (200.0 * GAIN_STUCCO).round().min(255.0) as u8);
        }
        // A cell boundary one period along: the point there maps to both tile edges.
        let at1: Vec<f32> = m
            .vertices
            .iter()
            .filter(|v| (v.pos[0] - p).abs() < 1e-4 && v.pos[1] == 0.0)
            .map(|v| v.uv[0])
            .collect();
        assert!(at1.contains(&r[0]) && at1.contains(&r[2]), "{at1:?}");
        // The partial last column covers half the tile.
        let last = m.vertices.iter().find(|v| v.pos[0] == 2.5 * p).unwrap();
        assert!((last.uv[0] - (r[0] + r[2]) * 0.5).abs() < 1e-6);
        // Bottom of the wall at the bottom of the tile (v down in the texture).
        let bottom = m.vertices.iter().find(|v| v.pos[1] == 0.0).unwrap();
        assert_eq!(bottom.uv[1], r[3]);
    }

    #[test]
    fn triangles_are_cut_without_cracks_or_flips() {
        // A slanted triangle over many brick cells keeps its area and winding, and every
        // piece's uv stays inside the tile.
        let tri = WorldMesh {
            vertices: vec![
                wv([0.1, 0.2, 0.0], [0.1, 0.2], Pattern::Brick),
                wv([4.3, 0.9, 0.0], [4.3, 0.9], Pattern::Brick),
                wv([1.7, 3.1, 0.0], [1.7, 3.1], Pattern::Brick),
            ],
            indices: vec![0, 1, 2],
        };
        let m = atlas_mesh(&tri, MeshKind::Building);
        assert!(
            (area(&m) - area(&tri)).abs() < 1e-4,
            "{} vs {}",
            area(&m),
            area(&tri)
        );
        assert!(m.triangle_count() > 20);
        for t in m.indices.chunks_exact(3) {
            let one = WorldMesh {
                vertices: t.iter().map(|&i| m.vertices[i as usize]).collect(),
                indices: vec![0, 1, 2],
            };
            assert!(area(&one) > 0.0);
        }
        // A tan-ish color picks the tan brick, as gray.
        let r = Tile::BrickTan.rect();
        assert!(m.vertices.iter().all(|v| inside(v.uv, r)));
        assert!(m.vertices.iter().all(|v| v.color[0] == v.color[1]));
    }

    #[test]
    fn windows_get_one_tile_per_face_and_plain_stays_white() {
        let m = atlas_mesh(&wall(1.2, 1.5, Pattern::Glass), MeshKind::Building);
        assert_eq!(m.triangle_count(), 2);
        let r = Tile::WindowGlass.rect();
        let uvs: Vec<[f32; 2]> = m.vertices.iter().map(|v| v.uv).collect();
        for corner in [[r[0], r[3]], [r[2], r[3]], [r[2], r[1]], [r[0], r[1]]] {
            assert!(uvs.contains(&corner), "{corner:?} {uvs:?}");
        }
        // Plate glass shows one pane, stretched.
        let shop = atlas_mesh(&wall(4.5, 2.4, Pattern::Glass), MeshKind::Building);
        assert_eq!(shop.triangle_count(), 2);
        assert!(shop.vertices.iter().all(|v| v.uv[0] < (r[0] + r[2]) * 0.5));
        // Glass on props (cars) and plain paint sample the white tile's center.
        let w = Tile::White.rect();
        let center = [(w[0] + w[2]) * 0.5, (w[1] + w[3]) * 0.5];
        for m in [
            atlas_mesh(&wall(4.5, 2.4, Pattern::Glass), MeshKind::Prop),
            atlas_mesh(&wall(9.0, 9.0, Pattern::Paint), MeshKind::Building),
        ] {
            assert_eq!(m.triangle_count(), 2);
            assert!(m
                .vertices
                .iter()
                .all(|v| v.uv == center && v.color[0] == 200));
        }
    }

    #[test]
    fn town_stays_within_the_triangle_budget() {
        let place = place::Place::test_grid();
        let chunks = world::build_chunk_meshes(&place, &world::Palette::socal_1985());
        let (mut before, mut after) = ([0usize; 16], [0usize; 16]);
        for c in &chunks {
            let m = atlas_mesh(&c.mesh, c.kind);
            for (mesh, count) in [(&c.mesh, &mut before), (&m, &mut after)] {
                for t in mesh.indices.chunks_exact(3) {
                    count[mesh.vertices[t[0] as usize].color[3] as usize & 15] += 1;
                }
            }
            assert!(m.vertices.len() <= 256 * 1024);
            assert!(m
                .vertices
                .iter()
                .all(|v| v.uv.iter().all(|c| (0.0..=1.0).contains(c))));
        }
        let (b, a): (usize, usize) = (before.iter().sum(), after.iter().sum());
        std::println!("town triangles: {b} -> {a}");
        for p in 0..14 {
            std::println!("  pattern {p:2}: {:7} -> {:7}", before[p], after[p]);
        }
        assert!(a <= TOWN_TRIANGLE_BUDGET, "{a}");
    }
}
