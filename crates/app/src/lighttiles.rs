//! Light tiles: the town's chunk meshes cut into pieces small enough that the lights
//! reaching each piece fit in one draw's light set.
//!
//! The renderer shades an instance with at most [`LIGHTS_PER_DRAW`] lights, and a 128 m
//! chunk on the strip is reached by twenty-odd street lamps. So the lit meshes are cut
//! along the chunk grid ([`bin_by_cells`]: a stripe of lane paint running past its chunk
//! goes to the next one) and each chunk's meshes are drawn as one (ground, roads, paint,
//! buildings and props: one draw instead of five); a chunk reached by more static lights
//! than one draw takes ([`STATIC_LIGHTS_PER_TILE`]) is quartered, and its quarters again,
//! down to [`MIN_TILE_M`]. Triangles crossing a cut are clipped along it
//! (one line at a time, Sutherland–Hodgman); a new vertex interpolates every attribute and
//! is computed from the edge's endpoints in a fixed order, so the triangles either side of
//! a shared edge get the same one and the pieces meet without cracks.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use glam::Vec3;
use render::{Mesh, PointLight, Vertex, LIGHTS_PER_DRAW};

use crate::lighting::Bounds;

/// Smallest tile edge, m: a quarter of a chunk's edge (on the test town's busiest corner
/// eight lamps reach a 32 m tile).
pub const MIN_TILE_M: f32 = 32.0;
/// Static lights a tile may be reached by before it is split: a whole draw's set. Near the
/// car its headlights then push the farthest one or two out of the set (those that only
/// graze the tile, at the edge of their reach); leaving room for them instead would cut
/// 150 more tiles in the test town for little to see.
pub const STATIC_LIGHTS_PER_TILE: usize = LIGHTS_PER_DRAW;

/// One drawn piece of the town and what it covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub mesh: Mesh,
    pub bounds: Bounds,
}

/// The box round every vertex of `mesh`.
pub fn mesh_bounds(mesh: &Mesh) -> Option<Bounds> {
    Bounds::around(mesh.vertices.iter().map(|v| Vec3::from(v.pos)))
}

/// Cuts `mesh` along the grid of `cell`-sized squares (x and z, lines at multiples of
/// `cell`) and appends each piece to the mesh of its cell in `cells` (keyed by cell `[z,
/// x]`, so iteration runs in rows).
pub fn bin_by_cells(mesh: Mesh, cell: f32, cells: &mut BTreeMap<[i32; 2], Mesh>) {
    let Some(b) = mesh_bounds(&mesh) else {
        return;
    };
    let index = |v: f32| (v / cell).floor() as i32;
    let peel = |mesh: Mesh, axis: usize, lo: f32, hi: f32| {
        let mut parts = Vec::new();
        let mut rest = mesh;
        for k in index(lo) + 1..=index(hi) {
            let (below, above) = split(&rest, axis, k as f32 * cell);
            parts.push((k - 1, below));
            rest = above;
        }
        parts.push((index(hi), rest));
        parts
    };
    for (x, column) in peel(mesh, 0, b.min.x, b.max.x) {
        for (z, piece) in peel(column, 2, b.min.z, b.max.z) {
            if !piece.indices.is_empty() {
                append(cells.entry([z, x]).or_default(), &piece);
            }
        }
    }
}

/// Appends `part`'s triangles to `all`.
pub fn append(all: &mut Mesh, part: &Mesh) {
    let base = all.vertices.len() as u32;
    all.vertices.extend_from_slice(&part.vertices);
    all.indices.extend(part.indices.iter().map(|i| i + base));
}

/// Appends `mesh` to `out` as tiles: whole when at most [`STATIC_LIGHTS_PER_TILE`] of
/// `lights` reach it or `size` is down to [`MIN_TILE_M`], else split into the four
/// quarters of its square cell (corner `origin` in x and z, edge `size`) and each of those
/// the same way. Empty pieces are dropped.
pub fn split_for_lights(
    mesh: Mesh,
    origin: [f32; 2],
    size: f32,
    lights: &[PointLight],
    out: &mut Vec<Tile>,
) {
    if mesh.indices.is_empty() {
        return;
    }
    let Some(bounds) = mesh_bounds(&mesh) else {
        return;
    };
    let reached = lights.iter().filter(|l| bounds.reached_by(l)).count();
    if reached <= STATIC_LIGHTS_PER_TILE || size <= MIN_TILE_M {
        out.push(Tile { mesh, bounds });
        return;
    }
    let half = size * 0.5;
    let (west, east) = split(&mesh, 0, origin[0] + half);
    for (part, x) in [(west, origin[0]), (east, origin[0] + half)] {
        let (north, south) = split(&part, 2, origin[1] + half);
        split_for_lights(north, [x, origin[1]], half, lights, out);
        split_for_lights(south, [x, origin[1] + half], half, lights, out);
    }
}

/// One side of a cut being built: its mesh and where the source's vertices went in it.
struct Side {
    mesh: Mesh,
    remap: Vec<u32>,
}

impl Side {
    fn new(vertices: usize) -> Side {
        Side {
            mesh: Mesh::default(),
            remap: vec![u32::MAX; vertices],
        }
    }

    fn source(&mut self, src: &Mesh, i: u32) -> u32 {
        let slot = &mut self.remap[i as usize];
        if *slot == u32::MAX {
            *slot = self.mesh.vertices.len() as u32;
            self.mesh.vertices.push(src.vertices[i as usize]);
        }
        *slot
    }

    fn new_vertex(&mut self, v: Vertex) -> u32 {
        self.mesh.vertices.push(v);
        self.mesh.vertices.len() as u32 - 1
    }
}

/// A corner of a clipped triangle: a source vertex or one made on the cut.
#[derive(Clone, Copy)]
enum Corner {
    Source(u32),
    Cut(Vertex),
}

/// `mesh` cut along the plane where coordinate `axis` (0 x, 2 z) is `at`: the part at or
/// below it, then the part at or above it. A triangle lying in the plane goes below.
pub fn split(mesh: &Mesh, axis: usize, at: f32) -> (Mesh, Mesh) {
    let mut sides = [
        Side::new(mesh.vertices.len()),
        Side::new(mesh.vertices.len()),
    ];
    for tri in mesh.indices.chunks_exact(3) {
        let d = [0, 1, 2].map(|k| mesh.vertices[tri[k] as usize].pos[axis] - at);
        if d.iter().all(|&x| x <= 0.0) || d.iter().all(|&x| x >= 0.0) {
            let side = &mut sides[usize::from(d.iter().any(|&x| x > 0.0))];
            for &i in tri {
                let j = side.source(mesh, i);
                side.mesh.indices.push(j);
            }
            continue;
        }
        for (k, side) in sides.iter_mut().enumerate() {
            // Inside: at or below the plane for the first side, at or above for the second.
            let s = if k == 0 { 1.0 } else { -1.0 };
            let mut poly = [Corner::Source(0); 4];
            let mut n = 0;
            for e in 0..3 {
                let (a, b) = (tri[e], tri[(e + 1) % 3]);
                let (da, db) = (s * d[e], s * d[(e + 1) % 3]);
                if da <= 0.0 {
                    poly[n] = Corner::Source(a);
                    n += 1;
                }
                if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
                    let (va, vb) = (&mesh.vertices[a as usize], &mesh.vertices[b as usize]);
                    poly[n] = Corner::Cut(cut(va, vb, axis, at));
                    n += 1;
                }
            }
            let mut ids = [0u32; 4];
            for (id, c) in ids.iter_mut().zip(&poly[..n]) {
                *id = match *c {
                    Corner::Source(i) => side.source(mesh, i),
                    Corner::Cut(v) => side.new_vertex(v),
                };
            }
            // A triangle or a quad, fanned from its first corner (winding kept).
            for f in 1..n.saturating_sub(1) {
                side.mesh
                    .indices
                    .extend_from_slice(&[ids[0], ids[f], ids[f + 1]]);
            }
        }
    }
    let [below, above] = sides;
    (below.mesh, above.mesh)
}

/// The point where edge `a`–`b` meets the plane `pos[axis] = at`, every attribute
/// interpolated. Computed from the endpoint that sorts first by position, so either
/// triangle sharing the edge gets the same vertex, bit for bit.
fn cut(a: &Vertex, b: &Vertex, axis: usize, at: f32) -> Vertex {
    let order = |p: &[f32; 3], q: &[f32; 3]| {
        (0..3)
            .map(|k| p[k].total_cmp(&q[k]))
            .find(|o| *o != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    };
    let (a, b) = if order(&a.pos, &b.pos) == Ordering::Greater {
        (b, a)
    } else {
        (a, b)
    };
    let t = (at - a.pos[axis]) / (b.pos[axis] - a.pos[axis]);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let mut pos = [0, 1, 2].map(|k| lerp(a.pos[k], b.pos[k]));
    pos[axis] = at;
    let n = Vec3::from([0, 1, 2].map(|k| lerp(a.normal[k], b.normal[k])));
    let normal = n.normalize_or(Vec3::from(a.normal)).to_array();
    let byte = |x: u8, y: u8| lerp(f32::from(x), f32::from(y)).round() as u8;
    let bytes = |x: [u8; 4], y: [u8; 4]| [0, 1, 2, 3].map(|k| byte(x[k], y[k]));
    let mut emissive = bytes(a.emissive, b.emissive);
    // The alpha packs an intensity with a flag bit: take the nearer end's, not a blend.
    emissive[3] = if t < 0.5 {
        a.emissive[3]
    } else {
        b.emissive[3]
    };
    Vertex {
        pos,
        normal,
        uv: [0, 1].map(|k| lerp(f32::from(a.uv[k]), f32::from(b.uv[k])).round() as u16),
        color: bytes(a.color, b.color),
        emissive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::streetlamp;

    /// A flat grid of `n`×`n` quads over `[0, size]²`, each its own two triangles.
    fn grid(n: usize, size: f32) -> Mesh {
        let mut m = Mesh::default();
        let step = size / n as f32;
        for z in 0..n {
            for x in 0..n {
                let base = m.vertices.len() as u32;
                for (dx, dz) in [(0, 0), (1, 0), (1, 1), (0, 1)] {
                    m.vertices.push(Vertex {
                        pos: [(x + dx) as f32 * step, 0.0, (z + dz) as f32 * step],
                        normal: [0.0, 1.0, 0.0],
                        uv: [(x + dx) as u16 * 100, (z + dz) as u16 * 100],
                        color: [200, 100, 50, 255],
                        emissive: [0; 4],
                    });
                }
                m.indices
                    .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            }
        }
        m
    }

    fn area(m: &Mesh) -> f32 {
        m.indices
            .chunks_exact(3)
            .map(|t| {
                let p = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].pos));
                (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5
            })
            .sum()
    }

    /// Every triangle wound as the grid's are (its normal up) and not degenerate.
    fn normals_up(m: &Mesh) -> bool {
        m.indices.chunks_exact(3).all(|t| {
            let p = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].pos));
            (p[1] - p[0]).cross(p[2] - p[0]).y > 1e-6
        })
    }

    #[test]
    fn a_cut_keeps_the_area_and_winding_and_splits_along_the_line() {
        // Three cells across 100 m: the cut at 50 runs through the middle cell.
        let m = grid(3, 100.0);
        let (west, east) = split(&m, 0, 50.0);
        assert!((area(&west) + area(&east) - area(&m)).abs() < 1e-2);
        assert!((area(&west) - 5000.0).abs() < 1e-2, "{}", area(&west));
        assert!(west.vertices.iter().all(|v| v.pos[0] <= 50.0));
        assert!(east.vertices.iter().all(|v| v.pos[0] >= 50.0));
        assert!(normals_up(&west) && normals_up(&east));
        // Cut vertices interpolate: uv 100 per cell of 33.3 m, so 150 at x = 50.
        let on = west.vertices.iter().find(|v| v.pos[0] == 50.0).unwrap();
        assert_eq!(on.uv[0], 150);
        assert_eq!(on.color, [200, 100, 50, 255]);
        // A cut along the grid's own line splits nothing: the halves keep whole cells.
        let (a, b) = split(&grid(4, 100.0), 2, 50.0);
        assert_eq!((a.indices.len(), b.indices.len()), (48, 48));
        assert_eq!(a.vertices.len(), 32);
    }

    #[test]
    fn shared_edges_get_the_same_cut_vertex_from_both_sides() {
        let m = grid(3, 100.0);
        let (west, _) = split(&m, 0, 50.0);
        // Every cut vertex on the line appears with bit-identical positions wherever the
        // triangles either side of an edge produced it.
        let mut on: Vec<[u32; 3]> = west
            .vertices
            .iter()
            .filter(|v| v.pos[0] == 50.0)
            .map(|v| v.pos.map(f32::to_bits))
            .collect();
        on.sort_unstable();
        on.dedup();
        // The cells' 4 horizontal edges and 3 diagonals each meet the line once.
        assert_eq!(on.len(), 4 + 3);
    }

    #[test]
    fn binning_cuts_a_mesh_along_the_grid() {
        // 100 m square at the origin over 64 m cells: four pieces, areas by cell.
        let mut cells = BTreeMap::new();
        bin_by_cells(grid(5, 100.0), 64.0, &mut cells);
        assert_eq!(
            cells.keys().copied().collect::<Vec<_>>(),
            [[0, 0], [0, 1], [1, 0], [1, 1]]
        );
        let areas: Vec<f32> = cells.values().map(area).collect();
        for (a, want) in areas
            .iter()
            .zip([64.0 * 64.0, 36.0 * 64.0, 64.0 * 36.0, 36.0 * 36.0])
        {
            assert!((a - want).abs() < 0.05, "{areas:?}");
        }
        for ([z, x], m) in &cells {
            let b = mesh_bounds(m).unwrap();
            assert!(b.min.x >= *x as f32 * 64.0 && b.max.x <= (*x + 1) as f32 * 64.0);
            assert!(b.min.z >= *z as f32 * 64.0 && b.max.z <= (*z + 1) as f32 * 64.0);
        }
        // A second mesh adds onto the same cells; one ending on a line adds nothing past it.
        bin_by_cells(grid(2, 64.0), 64.0, &mut cells);
        assert_eq!(cells.len(), 4);
        assert!((area(&cells[&[0, 0]]) - 2.0 * 64.0 * 64.0).abs() < 0.05);
    }

    #[test]
    fn a_chunk_is_quartered_until_its_lights_fit() {
        let chunk = grid(16, 128.0);
        // No lights: one tile. A row of lamps down x = 30 every 8 m: split to 32 m.
        let mut out = Vec::new();
        split_for_lights(chunk.clone(), [0.0, 0.0], 128.0, &[], &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bounds, mesh_bounds(&chunk).unwrap());
        let lamps: Vec<PointLight> = (0..16)
            .map(|i| streetlamp(Vec3::new(30.0, 7.5, i as f32 * 8.0)))
            .collect();
        out.clear();
        split_for_lights(chunk.clone(), [0.0, 0.0], 128.0, &lamps, &mut out);
        assert!(out.len() > 4, "{}", out.len());
        let total: f32 = out.iter().map(|t| area(&t.mesh)).sum();
        assert!((total - area(&chunk)).abs() < 0.5);
        for t in &out {
            let reached = lamps.iter().filter(|l| t.bounds.reached_by(l)).count();
            let size = (t.bounds.max - t.bounds.min).max_element();
            assert!(reached <= STATIC_LIGHTS_PER_TILE || size <= MIN_TILE_M + 1e-3);
            assert_eq!(Some(t.bounds), mesh_bounds(&t.mesh));
        }
        // Far from the lamps the quarters stay whole: 64 m tiles on the east half.
        assert!(out
            .iter()
            .any(|t| t.bounds.min.x >= 64.0 && t.bounds.max.x - t.bounds.min.x > 63.0));
    }
}
