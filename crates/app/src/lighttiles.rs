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
#[derive(Debug)]
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
    cut_triangles(mesh, axis, at, 0..mesh.indices.len() / 3, &mut sides);
    let [below, above] = sides;
    (below.mesh, above.mesh)
}

/// [`split`] a run of triangles at a time, so a long cut can stop between runs.
#[derive(Debug)]
pub struct Cut {
    src: Mesh,
    axis: usize,
    at: f32,
    /// The next triangle to cut.
    next: usize,
    sides: [Side; 2],
}

impl Cut {
    pub fn new(src: Mesh, axis: usize, at: f32) -> Cut {
        let n = src.vertices.len();
        Cut {
            src,
            axis,
            at,
            next: 0,
            sides: [Side::new(n), Side::new(n)],
        }
    }

    /// Cuts up to `max` more triangles; returns how many it cut and whether every
    /// triangle is cut now.
    pub fn run(&mut self, max: usize) -> (usize, bool) {
        let total = self.src.indices.len() / 3;
        let end = total.min(self.next.saturating_add(max));
        let tris = self.next..end;
        cut_triangles(&self.src, self.axis, self.at, tris, &mut self.sides);
        let done = end - self.next;
        self.next = end;
        (done, end == total)
    }

    /// The two parts, as [`split`] gives them, once [`Cut::run`] says it is done.
    pub fn halves(self) -> (Mesh, Mesh) {
        let [below, above] = self.sides;
        (below.mesh, above.mesh)
    }
}

/// Cuts triangles `tris` of `mesh` along the plane `pos[axis] = at` into `sides` (below,
/// above), in order.
fn cut_triangles(
    mesh: &Mesh,
    axis: usize,
    at: f32,
    tris: std::ops::Range<usize>,
    sides: &mut [Side; 2],
) {
    for tri in mesh.indices[tris.start * 3..tris.end * 3].chunks_exact(3) {
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

/// The light tiles of one streamed chunk's geometry (all its kinds as one mesh), built
/// without its neighbours, a piece at a time so a streamer can stop between pieces: whole
/// when none of the lights reaches it (the terrain apron), else cut along the chunk grid
/// and each piece split for the lights as [`split_for_lights`] does (the same tiles, in
/// the same order). Pieces past the chunk's own cell (lane paint running on into the next
/// one) become tiles of their own instead of joining the neighbour's, so the chunk can
/// come and go alone. At most `max_tiles` (at least 1): the last ones are merged when
/// there would be more.
///
/// [`Tiler::start`], then [`Tiler::step`] until it returns the tiles: each step does about
/// [`TILE_PIECE_TRIANGLES`] triangles' worth of cutting (a cut stops part way and goes on
/// in the next step). Reused from chunk to chunk.
#[derive(Debug, Default)]
pub struct Tiler {
    own: [i32; 2],
    cell: f32,
    max_tiles: usize,
    lights: Vec<PointLight>,
    /// Grid lines across z the chunk's mesh spans: `[first, last]`.
    z_lines: [i32; 2],
    /// Work left, the next on top.
    todo: Vec<Job>,
    /// The chunk's mesh cut along the grid, by cell `[z, x]`.
    cells: Vec<([i32; 2], Mesh)>,
    /// Tiles of the chunk's own cell, and pieces past it, so far.
    tiles: Vec<Tile>,
    over: Vec<Tile>,
}

/// Triangles' worth of work in one [`Tiler::step`] (cutting a triangle, or looking at a
/// vertex's bounds): about 2 ms in a release build.
pub const TILE_PIECE_TRIANGLES: usize = 6_000;

/// A piece of [`Tiler`]'s work.
#[derive(Debug)]
enum Job {
    /// Cut the chunk's mesh along the chunk grid.
    Bin(Mesh),
    /// Peel `mesh` along `axis` at grid lines `k..=last` (`x`: the column, peeling z).
    Peel {
        mesh: Mesh,
        axis: usize,
        k: i32,
        last: i32,
        x: i32,
    },
    /// A cut under way, and where its parts go.
    Cutting { cut: Cut, then: Then },
    /// Every cell is cut out: split them in turn, in rows.
    Cells,
    /// Keep a mesh whole as a tile if its lights fit, or quarter it (its square cell's
    /// corner `origin`, edge `size`; `own`: in the chunk's own cell).
    Split {
        mesh: Mesh,
        origin: [f32; 2],
        size: f32,
        own: bool,
    },
    /// Merge the pieces past the cell into the tiles and cap their number.
    Finish,
}

/// Where the parts of a [`Job::Cutting`] go.
#[derive(Debug)]
enum Then {
    /// Peeling: below is the piece before line `k`, above goes on from line `k + 1`.
    Peel {
        axis: usize,
        k: i32,
        last: i32,
        x: i32,
    },
    /// Quartering, cut across x: each half is cut across z next, the west one first.
    Halves {
        origin: [f32; 2],
        half: f32,
        own: bool,
    },
    /// Quartering, a half cut across z: its north and south quarters are split in turn.
    Quarters {
        origin: [f32; 2],
        half: f32,
        own: bool,
    },
}

impl Tiler {
    /// Starts on `base`, chunk `own` of the `cell`-sized grid, for the static `lights`
    /// (those that cannot reach it change nothing).
    pub fn start(
        &mut self,
        base: Mesh,
        own: [i32; 2],
        cell: f32,
        lights: impl IntoIterator<Item = PointLight>,
        max_tiles: usize,
    ) {
        self.own = own;
        self.cell = cell;
        self.max_tiles = max_tiles;
        self.lights.clear();
        self.lights.extend(lights);
        self.todo.clear();
        self.cells.clear();
        self.tiles.clear();
        self.over.clear();
        self.todo.push(Job::Finish);
        self.todo.push(Job::Bin(base));
    }

    /// Runs the next piece of work; returns the tiles after the last.
    pub fn step(&mut self) -> Option<Vec<Tile>> {
        let mut left = TILE_PIECE_TRIANGLES;
        while left > 0 {
            // A pass over a whole mesh (its bounds) that does not fit what is left of this
            // step waits for the next one.
            let whole = match self.todo.last() {
                Some(Job::Bin(m)) | Some(Job::Split { mesh: m, .. }) => m.vertices.len(),
                _ => 0,
            };
            if whole > left && left < TILE_PIECE_TRIANGLES {
                break;
            }
            let job = self.todo.pop().unwrap_or(Job::Finish);
            let cost = match job {
                Job::Finish => return Some(self.finish()),
                Job::Bin(base) => self.bin(base),
                Job::Peel {
                    mesh,
                    axis,
                    k,
                    last,
                    x,
                } => {
                    if k <= last {
                        let at = k as f32 * self.cell;
                        let cut = Cut::new(mesh, axis, at);
                        let then = Then::Peel { axis, k, last, x };
                        self.todo.push(Job::Cutting { cut, then });
                    } else {
                        self.peeled(axis, last, x, mesh);
                    }
                    0
                }
                Job::Cutting { mut cut, then } => {
                    let (n, done) = cut.run(left);
                    if done {
                        self.cut_done(cut.halves(), then);
                    } else {
                        self.todo.push(Job::Cutting { cut, then });
                    }
                    n.max(1)
                }
                Job::Cells => {
                    self.cells.sort_by_key(|c| c.0);
                    // Pushed last first, so they run in the cells' (row) order.
                    while let Some(([z, x], mesh)) = self.cells.pop() {
                        self.todo.push(Job::Split {
                            mesh,
                            origin: [x as f32 * self.cell, z as f32 * self.cell],
                            size: self.cell,
                            own: [x, z] == self.own,
                        });
                    }
                    0
                }
                Job::Split {
                    mesh,
                    origin,
                    size,
                    own,
                } => self.split_job(mesh, origin, size, own),
            };
            left = left.saturating_sub(cost);
        }
        None
    }

    /// Starts on the chunk's mesh: whole when no light reaches it, else peeled along the
    /// grid lines across x, then each column across z (as [`bin_by_cells`] cuts).
    fn bin(&mut self, base: Mesh) -> usize {
        let cost = base.vertices.len();
        let Some(b) = mesh_bounds(&base).filter(|_| !base.indices.is_empty()) else {
            return cost;
        };
        if !self.lights.iter().any(|l| b.reached_by(l)) {
            self.tiles.push(Tile {
                mesh: base,
                bounds: b,
            });
            return cost;
        }
        let index = |v: f32| (v / self.cell).floor() as i32;
        self.z_lines = [index(b.min.z) + 1, index(b.max.z)];
        self.todo.push(Job::Cells);
        self.todo.push(Job::Peel {
            mesh: base,
            axis: 0,
            k: index(b.min.x) + 1,
            last: index(b.max.x),
            x: 0,
        });
        cost
    }

    /// A peeled piece: the one before grid line `k + 1` along `axis` (a column, peeled
    /// across z next, or a cell).
    fn peeled(&mut self, axis: usize, k: i32, x: i32, mesh: Mesh) {
        if mesh.indices.is_empty() {
            return;
        }
        if axis == 0 {
            let [first, last] = self.z_lines;
            self.todo.push(Job::Peel {
                mesh,
                axis: 2,
                k: first,
                last,
                x: k,
            });
        } else {
            self.cells.push(([k, x], mesh));
        }
    }

    fn cut_done(&mut self, (below, above): (Mesh, Mesh), then: Then) {
        match then {
            Then::Peel { axis, k, last, x } => {
                self.todo.push(Job::Peel {
                    mesh: above,
                    axis,
                    k: k + 1,
                    last,
                    x,
                });
                self.peeled(axis, k - 1, x, below);
            }
            Then::Halves { origin, half, own } => {
                let mid = origin[1] + half;
                for (mesh, x) in [(above, origin[0] + half), (below, origin[0])] {
                    self.todo.push(Job::Cutting {
                        cut: Cut::new(mesh, 2, mid),
                        then: Then::Quarters {
                            origin: [x, origin[1]],
                            half,
                            own,
                        },
                    });
                }
            }
            Then::Quarters { origin, half, own } => {
                let south = [origin[0], origin[1] + half];
                for (mesh, origin) in [(above, south), (below, origin)] {
                    self.todo.push(Job::Split {
                        mesh,
                        origin,
                        size: half,
                        own,
                    });
                }
            }
        }
    }

    /// One level of [`split_for_lights`]: a tile, or its quarters to come.
    fn split_job(&mut self, mesh: Mesh, origin: [f32; 2], size: f32, own: bool) -> usize {
        let cost = mesh.vertices.len();
        if mesh.indices.is_empty() {
            return cost;
        }
        let Some(bounds) = mesh_bounds(&mesh) else {
            return cost;
        };
        let reached = self.lights.iter().filter(|l| bounds.reached_by(l)).count();
        if reached <= STATIC_LIGHTS_PER_TILE || size <= MIN_TILE_M {
            let into = if own { &mut self.tiles } else { &mut self.over };
            into.push(Tile { mesh, bounds });
            return cost;
        }
        let half = size * 0.5;
        self.todo.push(Job::Cutting {
            cut: Cut::new(mesh, 0, origin[0] + half),
            then: Then::Halves { origin, half, own },
        });
        cost
    }

    fn finish(&mut self) -> Vec<Tile> {
        let mut tiles = std::mem::take(&mut self.tiles);
        let lights = &self.lights;
        // A piece past the cell joins the first tile it can share a light set with: the two
        // together are still reached by no more lights than one draw takes.
        for piece in self.over.drain(..) {
            let union = |b: &Bounds| Bounds {
                min: b.min.min(piece.bounds.min),
                max: b.max.max(piece.bounds.max),
            };
            let fits = |t: &Tile| {
                let u = union(&t.bounds);
                lights.iter().filter(|l| u.reached_by(l)).count() <= STATIC_LIGHTS_PER_TILE
            };
            match tiles.iter_mut().find(|t| fits(t)) {
                Some(t) => {
                    append(&mut t.mesh, &piece.mesh);
                    t.bounds = union(&t.bounds);
                }
                None => tiles.push(piece),
            }
        }
        while tiles.len() > self.max_tiles.max(1) {
            let last = tiles.pop().expect("more than one tile");
            let into = tiles.last_mut().expect("at least one tile");
            append(&mut into.mesh, &last.mesh);
            into.bounds = Bounds {
                min: into.bounds.min.min(last.bounds.min),
                max: into.bounds.max.max(last.bounds.max),
            };
        }
        tiles
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

    /// The tiler gives a chunk's tiles as [`split_for_lights`] gives them, in the same
    /// order, a cut at a time; a chunk no light reaches stays whole.
    #[test]
    fn the_tiler_cuts_as_split_for_lights_does() {
        // Chunk (1, 0) on the 128 m grid, lamps down x = 160 every 8 m.
        let mut chunk = grid(32, 128.0);
        for v in &mut chunk.vertices {
            v.pos[0] += 128.0;
        }
        let lamps: Vec<PointLight> = (0..16)
            .map(|i| streetlamp(Vec3::new(160.0, 7.5, i as f32 * 8.0)))
            .collect();
        let mut tiler = Tiler::default();
        tiler.start(chunk.clone(), [1, 0], 128.0, lamps.iter().copied(), 30);
        let mut steps = 1;
        let tiles = loop {
            if let Some(t) = tiler.step() {
                break t;
            }
            steps += 1;
        };
        let mut want = Vec::new();
        split_for_lights(chunk.clone(), [128.0, 0.0], 128.0, &lamps, &mut want);
        assert!(
            want.len() > 4 && steps > 1,
            "{} tiles, {steps} steps",
            want.len()
        );
        assert!(tiles == want);
        // No light: one tile, the whole mesh.
        tiler.start(chunk.clone(), [1, 0], 128.0, [], 30);
        let tiles = loop {
            if let Some(t) = tiler.step() {
                break t;
            }
        };
        assert_eq!(tiles.len(), 1);
        assert!(tiles[0].mesh == chunk);
    }

    #[test]
    fn a_cut_in_runs_is_the_split() {
        let m = grid(9, 100.0);
        let whole = split(&m, 2, 41.0);
        let mut cut = Cut::new(m.clone(), 2, 41.0);
        let mut runs = 0;
        while !cut.run(7).1 {
            runs += 1;
        }
        assert!(runs > 10);
        assert!(cut.halves() == whole);
    }
}
