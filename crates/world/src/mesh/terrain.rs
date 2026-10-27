//! Terrain and water meshes of a terrain place (`docs/WORLD_GENERATION.md` stage 8).
//!
//! [`TerrainGrid::new`] is the place's heightfield with the roads stamped in: within a
//! road's width plus a [`SHOULDER_M`] shoulder (and [`GRID_COVER_M`] more, so every 4 m
//! triangle that reaches under a road's edge is flattened) each sample is pulled to just
//! under the road's profile (the nearest road's, or the lowest one's where they are a deck
//! apart: ground goes under bridges); beyond that, 1:2 cut and fill slopes blend back into
//! the terrain over [`BLEND_M`]; inside junctions the ground stays under the junction's
//! asphalt fan. The meshes and the collider read the same grid, triangulated the same way
//! ([`TerrainGrid::height_at`]).
//!
//! [`build`] meshes it per chunk: one indexed mesh of two triangles per cell with smooth
//! normals, vertex colors by zone (sand on the beach, wet near the water, lawn and dry
//! grass in town, scrub on the hills, soil on farmland), blended over neighboring cells so
//! the beach band fades into the grass, darkened and turned to bare soil on steep cuts, and
//! mottled by seeded noise; plus a flat water plane at `water_level` in every chunk that
//! has water, reaching far out to sea from the place's edges.
//!
//! [`build_far`] is the whole place's terrain and water in one coarse mesh (a sample every
//! [`FAR_STEP_M`]), drawn behind the streamed chunks out to the horizon. It sits under the
//! ground everywhere: [`FAR_DROP_M`] lower, and each of its samples lowered further by
//! whatever the coarse surface would otherwise poke above the fine one in the cells around
//! it (valleys and road cuts; on ridges the coarse surface lies under the fine one anyway),
//! so a resident chunk always wins the depth test.

use alloc::vec;
use alloc::vec::Vec;

use place::{Place, WaterKind, ZoneId, SIDEWALK_WIDTH_M};

use crate::math::{clamp, floor, lerp, segment_distance, sqrt, value_noise, V2, V3};
use crate::mesh::{quad, Paint, Pattern, WorldMesh, WorldVertex};
use crate::palette::{mix, shade, Palette, Rgb};
use crate::roads::{profile_height, Network, DECK_CLEARANCE_M};
use crate::{MeshKind, MeshSet};

/// Shoulder beside a road (past its curb or sidewalk) that is graded to the road.
pub const SHOULDER_M: f32 = 4.0;
/// Extra graded width so no terrain triangle under a road's edge keeps an ungraded corner.
pub const GRID_COVER_M: f32 = 2.0;
/// Most a sample under a road goes lower where the road surface bends up round it
/// ([`bend_up`]): tight hairpins bend it more than any 4 m grid follows.
const MAX_BEND_DROP_M: f32 = 0.05;
/// Width over which the 1:2 cut and fill slopes blend back into the terrain.
pub const BLEND_M: f32 = 8.0;
/// Rise over run of road cut and fill slopes.
const SLOPE: f32 = 0.5;
/// Graded ground sits this far under the road surface, so it never shows through.
pub const UNDER_ROAD_M: f32 = 0.04;
/// Curb top strip of a road without sidewalks (its edge, as far as grading goes).
const CURB_TOP_M: f32 = 0.2;
/// How far the water plane reaches out from the place's edges.
pub const OCEAN_SKIRT_M: f32 = 3000.0;
/// Water quads per chunk side (vertices for a future wave shader and for fog).
const WATER_SPLIT: u32 = 4;
const KEY_GROUND: u64 = 0x4752_4F55;
const KEY_GROUND_FINE: u64 = 0x4752_4646;
const KEY_STANDS: u64 = 0x5354_4E44;
/// Distance between the samples of the far terrain ([`build_far`]).
pub const FAR_STEP_M: f32 = 32.0;
/// How far the far terrain and its water sit under the ground and the water.
pub const FAR_DROP_M: f32 = 0.75;

/// The place's heights with the roads graded in, on the place's terrain grid.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainGrid {
    /// World `[x, z]` of sample `(0, 0)`.
    pub origin: [f32; 2],
    /// Distance between samples.
    pub cell: f32,
    /// Samples along x and z.
    pub w: usize,
    pub h: usize,
    /// Row-major (z major) heights.
    pub heights: Vec<f32>,
    /// Per cell (`(w - 1) × (h - 1)`, row-major): 1 where the cell is split along its
    /// `(1, 0)`–`(0, 1)` diagonal instead of `(0, 0)`–`(1, 1)`. Under roads each cell takes
    /// the diagonal that better fits the road surface at its center.
    pub flip: Vec<u8>,
    pub water_level: f32,
}

/// Best road candidates for one sample while grading: edges and their heights there.
#[derive(Clone, Copy)]
struct Pull {
    /// Nearest road (by distance past its curb) whose graded core holds the sample.
    core_key: f32,
    core: (u32, u32),
    core_y: f32,
    /// Lowest road whose core holds the sample.
    min: (u32, u32),
    min_y: f32,
    /// Nearest road whose blend band holds the sample, and the distance past its core.
    blend_s: f32,
    blend: (u32, u32),
}

/// No road: the `(edge, segment)` of an empty [`Pull`] slot.
const NONE: (u32, u32) = (u32::MAX, 0);

/// Surface height of `edge` at `q` from the profile around segment `seg` (so the legs of a
/// tight switchback, closer than a road's graded width, each grade their own ground).
fn local_height(net: &Network, (edge, seg): (u32, u32), q: V2) -> f32 {
    let path = &net.paths[edge as usize];
    if let Some(y) = path.flat {
        return y;
    }
    let n = path.raw.len();
    let k0 = (seg as usize).saturating_sub(2);
    let k1 = (seg as usize + 3).min(n - 1);
    profile_height(&path.raw[k0..=k1], &path.raw_y[k0..=k1], q)
}

/// How far the surface of `road` bends up round `q` (where it is at `y`): the largest
/// second difference over `h` along x, z and the diagonals, 0 where it is straight or bends
/// down. Ground interpolated between samples `2h` apart rises above such a surface by about
/// half of it (sags, and the inside of tight bends on a grade, where the height lines of
/// the segments either side blend), so the samples there go that much lower.
fn bend_up(net: &Network, road: (u32, u32), q: V2, y: f32, h: f32) -> f32 {
    if net.paths[road.0 as usize].flat.is_some() {
        return 0.0;
    }
    let mut most = 0.0f32;
    for d in [
        V2::new(h, 0.0),
        V2::new(0.0, h),
        V2::new(h, h),
        V2::new(h, -h),
    ] {
        let second = local_height(net, road, q + d) + local_height(net, road, q - d) - 2.0 * y;
        most = most.max(second);
    }
    most
}

impl TerrainGrid {
    /// The place's terrain as it is (no roads graded in).
    pub fn natural(place: &Place) -> TerrainGrid {
        let t = &place.terrain;
        let [w, h] = place::Terrain::samples(&place.header, t.res_per_chunk);
        let res = f32::from(t.res_per_chunk.max(1));
        let mut heights = t.heights.clone();
        heights.resize(w * h, t.water_level);
        TerrainGrid {
            origin: place.header.origin_m(),
            cell: place.header.chunk_size_m / res,
            w,
            h,
            heights,
            flip: vec![0; (w - 1) * (h - 1)],
            water_level: t.water_level,
        }
    }

    /// The place's terrain with every road of `net` graded in (see the module docs).
    pub fn new(place: &Place, net: &Network) -> TerrainGrid {
        TerrainGrid::graded(place, net).0
    }

    /// [`TerrainGrid::new`], plus the road each sample was graded to: `(edge, profile
    /// segment)`, `(u32::MAX, 0)` where none was (blend bands and junction fans aside).
    pub fn graded(place: &Place, net: &Network) -> (TerrainGrid, Vec<(u32, u32)>) {
        let mut g = TerrainGrid::natural(place);
        let none = Pull {
            core_key: f32::INFINITY,
            core: NONE,
            core_y: 0.0,
            min: NONE,
            min_y: f32::INFINITY,
            blend_s: f32::INFINITY,
            blend: NONE,
        };
        // Pass 1: which roads reach each sample (segment by segment, heights linear).
        let mut pull = vec![none; g.w * g.h];
        for (i, e) in place.roads.edges.iter().enumerate() {
            let path = &net.paths[i];
            let hw = e.width_m * 0.5;
            let side = if e.sidewalk {
                SIDEWALK_WIDTH_M
            } else {
                CURB_TOP_M
            };
            let core = hw + side + SHOULDER_M + GRID_COVER_M;
            let reach = core + BLEND_M;
            for k in 0..path.raw.len() - 1 {
                let (a, b) = (path.raw[k], path.raw[k + 1]);
                let (ya, yb) = (path.raw_y[k], path.raw_y[k + 1]);
                let [x0, x1, z0, z1] = g.span(a, b, reach);
                for iz in z0..=z1 {
                    for ix in x0..=x1 {
                        let q = g.pos(ix, iz);
                        let (d, t) = segment_distance(q, a, b);
                        if d >= reach {
                            continue;
                        }
                        let c = &mut pull[iz * g.w + ix];
                        if d < core {
                            let key = d - hw;
                            let y = lerp(ya, yb, t);
                            if key < c.core_key {
                                c.core_key = key;
                                c.core = (i as u32, k as u32);
                                c.core_y = y;
                            }
                            if y < c.min_y {
                                c.min_y = y;
                                c.min = (i as u32, k as u32);
                            }
                        } else if d - core < c.blend_s {
                            c.blend_s = d - core;
                            c.blend = (i as u32, k as u32);
                        }
                    }
                }
            }
        }
        // Pass 2: the chosen road's surface height there, just under it.
        let mut roads = vec![NONE; g.w * g.h];
        for iz in 0..g.h {
            for ix in 0..g.w {
                let c = pull[iz * g.w + ix];
                let q = g.pos(ix, iz);
                let hp = &mut g.heights[iz * g.w + ix];
                if c.core != NONE {
                    let road = if c.min_y < c.core_y - DECK_CLEARANCE_M {
                        c.min
                    } else {
                        c.core
                    };
                    roads[iz * g.w + ix] = road;
                    let y = local_height(net, road, q);
                    let bend = 0.5 * bend_up(net, road, q, y, 0.5 * g.cell);
                    *hp = y - UNDER_ROAD_M - bend.min(MAX_BEND_DROP_M);
                } else if c.blend != NONE {
                    let y = local_height(net, c.blend, q) - UNDER_ROAD_M;
                    let (h0, s) = (*hp, c.blend_s);
                    let sloped = clamp(h0, y - SLOPE * s, y + SLOPE * s);
                    let t = clamp(s / BLEND_M, 0.0, 1.0);
                    *hp = sloped + (h0 - sloped) * (t * t * (3.0 - 2.0 * t));
                }
            }
        }
        // Junctions: the ground stays under the asphalt fan.
        for j in &net.junctions {
            let ring = net.junction_ring(j);
            if ring.is_empty() {
                continue;
            }
            let c = j.pos.at(j.y);
            let (mut lo, mut hi) = (j.pos, j.pos);
            for p in &ring {
                lo = V2::new(lo.x.min(p.x), lo.y.min(p.z));
                hi = V2::new(hi.x.max(p.x), hi.y.max(p.z));
            }
            let [x0, x1, z0, z1] = g.span(lo, hi, g.cell);
            for iz in z0..=z1 {
                for ix in x0..=x1 {
                    let q = g.pos(ix, iz);
                    let n = ring.len();
                    for k in 0..n {
                        if let Some(y) = in_triangle(q, c, ring[k], ring[(k + 1) % n]) {
                            let hp = &mut g.heights[iz * g.w + ix];
                            *hp = hp.min(y - UNDER_ROAD_M);
                            break;
                        }
                    }
                }
            }
        }
        // Diagonals under roads: whichever fits the road surface at the cell's center.
        for iz in 0..g.h - 1 {
            for ix in 0..g.w - 1 {
                let corners = [(ix, iz), (ix + 1, iz), (ix, iz + 1), (ix + 1, iz + 1)];
                let mut best = (f32::INFINITY, NONE, 0.0f32);
                for (x, z) in corners {
                    let c = pull[z * g.w + x];
                    if c.core != NONE && c.core_key < best.0 {
                        let road = if c.min_y < c.core_y - DECK_CLEARANCE_M {
                            c.min
                        } else {
                            c.core
                        };
                        best = (c.core_key, road, 0.0);
                    }
                }
                if best.1 == NONE {
                    continue;
                }
                let q = g.pos(ix, iz) + V2::new(0.5 * g.cell, 0.5 * g.cell);
                let want = local_height(net, best.1, q) - UNDER_ROAD_M;
                let [h00, h10, h01, h11] = corners.map(|(x, z)| g.sample(x, z));
                let main = 0.5 * (h00 + h11) - want;
                let other = 0.5 * (h10 + h01) - want;
                if other.abs() + 1e-4 < main.abs() {
                    g.flip[iz * (g.w - 1) + ix] = 1;
                }
            }
        }
        (g, roads)
    }

    /// World ground point of sample `(ix, iz)`.
    pub fn pos(&self, ix: usize, iz: usize) -> V2 {
        V2::new(
            self.origin[0] + ix as f32 * self.cell,
            self.origin[1] + iz as f32 * self.cell,
        )
    }

    pub fn sample(&self, ix: usize, iz: usize) -> f32 {
        self.heights[iz * self.w + ix]
    }

    /// Sample index ranges `[x0, x1, z0, z1]` (inclusive, clamped) covering the box of
    /// `a` and `b` grown by `r`.
    fn span(&self, a: V2, b: V2, r: f32) -> [usize; 4] {
        let idx = |v: f32, o: f32, n: usize| clamp((v - o) / self.cell, 0.0, (n - 1) as f32);
        let x0 = floor(idx(a.x.min(b.x) - r, self.origin[0], self.w)) as usize;
        let x1 = idx(a.x.max(b.x) + r, self.origin[0], self.w) as usize + 1;
        let z0 = floor(idx(a.y.min(b.y) - r, self.origin[1], self.h)) as usize;
        let z1 = idx(a.y.max(b.y) + r, self.origin[1], self.h) as usize + 1;
        [x0, x1.min(self.w - 1), z0, z1.min(self.h - 1)]
    }

    /// Ground height at world `(x, z)` on the mesh's triangles (each cell split along the
    /// diagonal [`TerrainGrid::flip`] gives it), clamped to the grid.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let fx = clamp((x - self.origin[0]) / self.cell, 0.0, (self.w - 1) as f32);
        let fz = clamp((z - self.origin[1]) / self.cell, 0.0, (self.h - 1) as f32);
        let (ix, iz) = ((fx as usize).min(self.w - 2), (fz as usize).min(self.h - 2));
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let h00 = self.sample(ix, iz);
        let h11 = self.sample(ix + 1, iz + 1);
        if self.flip[iz * (self.w - 1) + ix] != 0 {
            let (h10, h01) = (self.sample(ix + 1, iz), self.sample(ix, iz + 1));
            return if tx + tz <= 1.0 {
                h00 + (h10 - h00) * tx + (h01 - h00) * tz
            } else {
                h11 + (h01 - h11) * (1.0 - tx) + (h10 - h11) * (1.0 - tz)
            };
        }
        if tx >= tz {
            let h10 = self.sample(ix + 1, iz);
            h00 + (h10 - h00) * tx + (h11 - h10) * tz
        } else {
            let h01 = self.sample(ix, iz + 1);
            h00 + (h01 - h00) * tz + (h11 - h01) * tx
        }
    }

    /// The samples `(ix, iz)` of the mesh triangle under world `(x, z)` (clamped).
    pub fn triangle_at(&self, x: f32, z: f32) -> [(usize, usize); 3] {
        let fx = clamp((x - self.origin[0]) / self.cell, 0.0, (self.w - 1) as f32);
        let fz = clamp((z - self.origin[1]) / self.cell, 0.0, (self.h - 1) as f32);
        let (ix, iz) = ((fx as usize).min(self.w - 2), (fz as usize).min(self.h - 2));
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let (a, b, c, d) = ((ix, iz), (ix + 1, iz), (ix, iz + 1), (ix + 1, iz + 1));
        if self.flip[iz * (self.w - 1) + ix] != 0 {
            if tx + tz <= 1.0 {
                [a, b, c]
            } else {
                [d, c, b]
            }
        } else if tx >= tz {
            [a, b, d]
        } else {
            [a, c, d]
        }
    }

    /// Smooth normal at sample `(ix, iz)` from central differences.
    fn normal(&self, ix: usize, iz: usize) -> V3 {
        let (x0, x1) = (ix.saturating_sub(1), (ix + 1).min(self.w - 1));
        let (z0, z1) = (iz.saturating_sub(1), (iz + 1).min(self.h - 1));
        let dx = (self.sample(x1, iz) - self.sample(x0, iz)) / ((x1 - x0) as f32 * self.cell);
        let dz = (self.sample(ix, z1) - self.sample(ix, z0)) / ((z1 - z0) as f32 * self.cell);
        V3::new(-dx, 1.0, -dz).normalize()
    }
}

/// Height of the triangle `a b c` (3D) at ground point `p` if `p` lies in it.
fn in_triangle(p: V2, a: V3, b: V3, c: V3) -> Option<f32> {
    let (ax, az, bx, bz, cx, cz) = (a.x, a.z, b.x, b.z, c.x, c.z);
    let den = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
    if den.abs() < 1e-9 {
        return None;
    }
    let l1 = ((bz - cz) * (p.x - cx) + (cx - bx) * (p.y - cz)) / den;
    let l2 = ((cz - az) * (p.x - cx) + (ax - cx) * (p.y - cz)) / den;
    let l3 = 1.0 - l1 - l2;
    let eps = -1e-4;
    (l1 >= eps && l2 >= eps && l3 >= eps).then_some(l1 * a.y + l2 * b.y + l3 * c.y)
}

/// Ground color of a zone before blending; `patch` in `[0, 1)` is seeded noise that puts
/// stands of dark chaparral on the hills' golden grass and varies the open ground.
fn zone_color(pal: &Palette, zone: ZoneId, patch: f32) -> Rgb {
    let golden = mix(pal.lawn_dry, pal.sand, 70);
    let stand = clamp((patch - 0.55) * 5.0, 0.0, 1.0);
    match zone {
        ZoneId::Beach => pal.sand,
        ZoneId::Water => shade(pal.sand, 170),
        ZoneId::Park => pal.lawn,
        ZoneId::Hills => mix(golden, shade(pal.scrub, 175), (stand * 230.0) as u32),
        ZoneId::Farmland => mix(pal.soil, pal.lawn_dry, (40.0 + 120.0 * patch) as u32),
        ZoneId::Suburb => mix(pal.lawn, golden, 130),
        _ => mix(golden, pal.scrub, (40.0 + 100.0 * patch) as u32),
    }
}

/// The zone color of every terrain cell, worked out once per place so each chunk's mesh
/// ([`build`]) only blends its own neighborhood.
#[derive(Clone, Debug, PartialEq)]
pub struct CellColors {
    /// Cells along x and z.
    w: usize,
    h: usize,
    /// Row-major (z major) colors.
    rgb: Vec<[u32; 3]>,
    /// Row-major zones, empty when the place has no zone map.
    zones: Vec<ZoneId>,
}

impl CellColors {
    pub fn new(place: &Place, grid: &TerrainGrid, pal: &Palette, seed: u64) -> CellColors {
        let [cw, ch] = place::Zones::cells_per_side(&place.header, place.terrain.res_per_chunk);
        let zones = if place.zones.cells.len() == cw * ch {
            place.zones.cells.clone()
        } else {
            Vec::new()
        };
        let mut colors = CellColors {
            w: cw,
            h: ch,
            rgb: Vec::new(),
            zones,
        };
        colors.rgb = (0..cw * ch)
            .map(|i| {
                let p = grid.pos(i % cw, i / cw);
                let patch = value_noise(seed ^ KEY_STANDS, p.x, p.y, 46.0);
                zone_color(pal, colors.zone_at(i % cw, i / cw), patch).map(u32::from)
            })
            .collect();
        colors
    }

    /// The zone of cell `(cx, cz)`, clamped to the map.
    fn zone_at(&self, cx: usize, cz: usize) -> ZoneId {
        if self.zones.is_empty() {
            ZoneId::Residential
        } else {
            self.zones[cz.min(self.h - 1) * self.w + cx.min(self.w - 1)]
        }
    }
}

/// Builds the terrain mesh of every chunk of `place` that `out` keeps from `grid`, and the
/// water planes.
pub fn build(
    place: &Place,
    grid: &TerrainGrid,
    colors: &CellColors,
    pal: &Palette,
    seed: u64,
    out: &mut MeshSet,
) {
    let res = usize::from(place.terrain.res_per_chunk.max(1));
    let (cw, ch) = (colors.w, colors.h);
    let zone_at = |cx: usize, cz: usize| colors.zone_at(cx, cz);
    // Each vertex blends the 4×4 cell colors around it (16 m).
    let cell_rgb = &colors.rgb;
    let paint = Paint::new(pal.lawn, Pattern::Terrain);
    let wl = grid.water_level;
    let [nx, nz] = place.header.size_chunks.map(usize::from);
    for cz in 0..nz {
        for cx in 0..nx {
            let center = grid.pos(cx * res + res / 2, cz * res + res / 2);
            if !out.wants(center) {
                continue;
            }
            let mut m = WorldMesh::default();
            let n = res + 1;
            for j in 0..n {
                for i in 0..n {
                    let (ix, iz) = (cx * res + i, cz * res + j);
                    let p = grid.pos(ix, iz);
                    let y = grid.sample(ix, iz);
                    let nrm = grid.normal(ix, iz);
                    let mut acc = [0u32; 3];
                    for dz in 0..4 {
                        for dx in 0..4 {
                            let qx = clamp(ix as f32 + dx as f32 - 2.0, 0.0, (cw - 1) as f32);
                            let qz = clamp(iz as f32 + dz as f32 - 2.0, 0.0, (ch - 1) as f32);
                            let c = cell_rgb[qz as usize * cw + qx as usize];
                            for k in 0..3 {
                                acc[k] += c[k];
                            }
                        }
                    }
                    let rgb = ground_rgb(acc.map(|a| (a / 16) as u8), y, wl, nrm, p, pal, seed);
                    m.vertices.push(WorldVertex {
                        pos: [p.x, y, p.y],
                        normal: nrm.to_array(),
                        uv: [p.x, p.y],
                        color: paint.with_rgb(rgb).color,
                        emissive: paint.emissive,
                    });
                }
            }
            for j in 0..res {
                for i in 0..res {
                    let v00 = (j * n + i) as u32;
                    let (v10, v01, v11) = (v00 + 1, v00 + n as u32, v00 + n as u32 + 1);
                    let cell = (cz * res + j) * (grid.w - 1) + cx * res + i;
                    if grid.flip[cell] != 0 {
                        m.indices.extend_from_slice(&[v00, v01, v10, v10, v01, v11]);
                    } else {
                        m.indices.extend_from_slice(&[v00, v11, v10, v00, v01, v11]);
                    }
                }
            }
            water(
                place,
                grid,
                pal,
                [cx, cz],
                res,
                &zone_at,
                WATER_SPLIT,
                &mut m,
            );
            surf(place, grid, pal, [cx, cz], res, seed, &mut m);
            let dst = out.at(center, MeshKind::Ground);
            let base = dst.vertices.len() as u32;
            dst.vertices.extend_from_slice(&m.vertices);
            dst.indices.extend(m.indices.iter().map(|i| i + base));
        }
    }
}

/// A terrain vertex's color: `rgb`, its zones' blended color, wet at the waterline, bare
/// soil on steep ground, shaded by slope and mottled by noise at ground point `p`.
fn ground_rgb(rgb: Rgb, y: f32, wl: f32, nrm: V3, p: V2, pal: &Palette, seed: u64) -> Rgb {
    let mut rgb = rgb;
    // Wet sand at the waterline, the sea floor darker still.
    if y < wl + 0.6 {
        let wet = clamp((wl + 0.6 - y) * 120.0, 0.0, 150.0);
        rgb = shade(rgb, 256 - wet as u32);
    }
    // Bare soil on steep cuts and fills, then slope darkening.
    let steep = clamp((0.93 - nrm.y) / 0.2, 0.0, 1.0);
    rgb = mix(rgb, shade(pal.soil, 200), (steep * 200.0) as u32);
    let coarse = value_noise(seed ^ KEY_GROUND, p.x, p.y, 37.0);
    let fine = value_noise(seed ^ KEY_GROUND_FINE, p.x, p.y, 7.0);
    let tone = 200.0 + 40.0 * nrm.y * nrm.y + 16.0 * fine;
    shade(rgb, (tone - 10.0 + 20.0 * coarse) as u32)
}

/// Height at `(tx, tz)` (each in `[0, 1]`) in a cell with corner heights `h` (`00`, `10`,
/// `01`, `11`), split along its `10`–`01` diagonal when `flip`, as
/// [`TerrainGrid::height_at`] interpolates.
fn cell_height(h: [f32; 4], flip: bool, tx: f32, tz: f32) -> f32 {
    let [h00, h10, h01, h11] = h;
    if flip {
        if tx + tz <= 1.0 {
            h00 + (h10 - h00) * tx + (h01 - h00) * tz
        } else {
            h11 + (h01 - h11) * (1.0 - tx) + (h10 - h11) * (1.0 - tz)
        }
    } else if tx >= tz {
        h00 + (h10 - h00) * tx + (h11 - h10) * tz
    } else {
        h00 + (h01 - h00) * tz + (h11 - h01) * tx
    }
}

/// The far terrain's coarse grid: the fine sample columns and rows it keeps (every
/// [`FAR_STEP_M`] and the last), its heights (each lowered under the fine ground around
/// it, see the module docs) and each cell's diagonal.
struct FarGrid {
    /// Fine samples per coarse step.
    step: usize,
    xs: Vec<usize>,
    zs: Vec<usize>,
    heights: Vec<f32>,
    flip: Vec<bool>,
}

fn far_grid(grid: &TerrainGrid) -> FarGrid {
    let step = floor(FAR_STEP_M / grid.cell + 0.5).max(1.0) as usize;
    let axis = |n: usize| {
        let mut v: Vec<usize> = (0..n).step_by(step).collect();
        if v.last() != Some(&(n - 1)) {
            v.push(n - 1);
        }
        v
    };
    let (xs, zs) = (axis(grid.w), axis(grid.h));
    let (nx, nz) = (xs.len(), zs.len());
    let plain: Vec<f32> = zs
        .iter()
        .flat_map(|&z| xs.iter().map(move |&x| grid.sample(x, z)))
        .collect();
    // Each cell's diagonal (the one nearer the fine ground at its center) and how far its
    // plain interpolation rises above the fine samples it covers.
    let cells = (nx - 1) * (nz - 1);
    let (mut flip, mut poke) = (vec![false; cells], vec![0.0f32; cells]);
    for cz in 0..nz - 1 {
        for cx in 0..nx - 1 {
            let (x0, x1, z0, z1) = (xs[cx], xs[cx + 1], zs[cz], zs[cz + 1]);
            let v = cz * nx + cx;
            let h = [plain[v], plain[v + 1], plain[v + nx], plain[v + nx + 1]];
            let mid = grid.sample((x0 + x1) / 2, (z0 + z1) / 2);
            let f = (0.5 * (h[1] + h[2]) - mid).abs() + 1e-4 < (0.5 * (h[0] + h[3]) - mid).abs();
            let mut worst = 0.0f32;
            for iz in z0..=z1 {
                for ix in x0..=x1 {
                    let tx = (ix - x0) as f32 / (x1 - x0) as f32;
                    let tz = (iz - z0) as f32 / (z1 - z0) as f32;
                    worst = worst.max(cell_height(h, f, tx, tz) - grid.sample(ix, iz));
                }
            }
            flip[cz * (nx - 1) + cx] = f;
            poke[cz * (nx - 1) + cx] = worst;
        }
    }
    // A sample drops by the most any cell round it pokes up: every point of a cell then
    // interpolates corners lowered at least as far as the cell pokes there.
    let mut heights = plain.clone();
    for j in 0..nz {
        for i in 0..nx {
            let mut drop = 0.0f32;
            for cz in j.saturating_sub(1)..j.min(nz - 2) + 1 {
                for cx in i.saturating_sub(1)..i.min(nx - 2) + 1 {
                    drop = drop.max(poke[cz * (nx - 1) + cx]);
                }
            }
            heights[j * nx + i] -= drop + FAR_DROP_M;
        }
    }
    FarGrid {
        step,
        xs,
        zs,
        heights,
        flip,
    }
}

/// The far terrain: the whole place's ground and water as one coarse mesh, a sample every
/// [`FAR_STEP_M`] (65×65 samples, 8192 triangles, for a 2 km place), colored by zone as
/// [`build`] colors it, under the fine ground everywhere (see the module docs), and a
/// water plane [`FAR_DROP_M`] under the water level in every chunk with water (one quad
/// apiece, and the skirts out to sea).
pub fn build_far(
    place: &Place,
    grid: &TerrainGrid,
    colors: &CellColors,
    pal: &Palette,
    seed: u64,
) -> WorldMesh {
    let FarGrid {
        step,
        xs,
        zs,
        heights,
        flip,
    } = far_grid(grid);
    let (nx, nz) = (xs.len(), zs.len());
    let pos = |i: usize, j: usize| grid.pos(xs[i], zs[j]);
    let normal = |i: usize, j: usize| {
        let (i0, i1) = (i.saturating_sub(1), (i + 1).min(nx - 1));
        let (j0, j1) = (j.saturating_sub(1), (j + 1).min(nz - 1));
        let dx = (heights[j * nx + i1] - heights[j * nx + i0]) / (pos(i1, j).x - pos(i0, j).x);
        let dz = (heights[j1 * nx + i] - heights[j0 * nx + i]) / (pos(i, j1).y - pos(i, j0).y);
        V3::new(-dx, 1.0, -dz).normalize()
    };
    let (cw, ch) = (colors.w, colors.h);
    let half = step / 2;
    let paint = Paint::new(pal.lawn, Pattern::Terrain);
    let wl = grid.water_level;
    let mut m = WorldMesh::default();
    for j in 0..nz {
        for i in 0..nx {
            let (ix, iz) = (xs[i], zs[j]);
            let p = pos(i, j);
            let nrm = normal(i, j);
            // The zone colors of the cells within half a step.
            let mut acc = [0u32; 3];
            let mut n = 0;
            for qz in iz.saturating_sub(half)..(iz + half).min(ch) {
                for qx in ix.saturating_sub(half)..(ix + half).min(cw) {
                    let c = colors.rgb[qz * cw + qx];
                    for k in 0..3 {
                        acc[k] += c[k];
                    }
                    n += 1;
                }
            }
            let avg = acc.map(|a| (a / n.max(1)) as u8);
            let rgb = ground_rgb(avg, grid.sample(ix, iz), wl, nrm, p, pal, seed);
            m.vertices.push(WorldVertex {
                pos: [p.x, heights[j * nx + i], p.y],
                normal: nrm.to_array(),
                uv: [p.x, p.y],
                color: paint.with_rgb(rgb).color,
                emissive: paint.emissive,
            });
        }
    }
    for cz in 0..nz - 1 {
        for cx in 0..nx - 1 {
            let v00 = (cz * nx + cx) as u32;
            let (v10, v01, v11) = (v00 + 1, v00 + nx as u32, v00 + nx as u32 + 1);
            if flip[cz * (nx - 1) + cx] {
                m.indices.extend_from_slice(&[v00, v01, v10, v10, v01, v11]);
            } else {
                m.indices.extend_from_slice(&[v00, v11, v10, v00, v01, v11]);
            }
        }
    }
    let ground = m.vertices.len();
    let res = usize::from(place.terrain.res_per_chunk.max(1));
    let zone_at = |cx: usize, cz: usize| colors.zone_at(cx, cz);
    let [cnx, cnz] = place.header.size_chunks.map(usize::from);
    for cz in 0..cnz {
        for cx in 0..cnx {
            water(place, grid, pal, [cx, cz], res, &zone_at, 1, &mut m);
        }
    }
    for v in &mut m.vertices[ground..] {
        v.pos[1] -= FAR_DROP_M;
    }
    m
}

/// The surf on the shorelines of chunk `c`: along every segment of an ocean's or a lake's
/// shoreline (its polygon less the place's sides) whose middle is in the chunk, with dry
/// ground on one side and water on the other, a band of foam just seaward of the waterline
/// and, on the ocean, a broken line of breakers further out; flat on the water, a hair
/// above it.
fn surf(
    place: &Place,
    grid: &TerrainGrid,
    pal: &Palette,
    c: [usize; 2],
    res: usize,
    seed: u64,
    m: &mut WorldMesh,
) {
    let o = grid.pos(c[0] * res, c[1] * res);
    let size = grid.cell * res as f32;
    let (lo, hi) = (grid.pos(0, 0), grid.pos(grid.w - 1, grid.h - 1));
    let on_side = |p: V2| {
        let e = 0.5;
        p.x < lo.x + e || p.y < lo.y + e || p.x > hi.x - e || p.y > hi.y - e
    };
    let wl = grid.water_level;
    let y = wl + SURF_LIFT_M;
    for body in &place.terrain.water {
        let ocean = match body.kind {
            WaterKind::Ocean => true,
            WaterKind::Lake => false,
            _ => continue,
        };
        let foam = Paint::new(mix(pal.water, pal.paint_white, 210), Pattern::Water);
        let breakers = Paint::new(mix(pal.water, pal.paint_white, 130), Pattern::Water);
        let n = body.polygon.len();
        for k in 0..n {
            let (a, b) = (
                V2::from_array(body.polygon[k]),
                V2::from_array(body.polygon[(k + 1) % n]),
            );
            let mid = a.lerp(b, 0.5);
            let inside = mid.x >= o.x && mid.y >= o.y && mid.x < o.x + size && mid.y < o.y + size;
            if !inside || (on_side(a) && on_side(b)) || (b - a).length() < 1e-3 {
                continue;
            }
            // Seaward: the side of the segment under the water, the other side dry.
            let t = (b - a).normalize();
            let mut nrm = V2::new(-t.y, t.x);
            let wet = |q: V2| grid.height_at(q.x, q.y) < wl;
            let (l, r) = (wet(mid + nrm * 3.0), wet(mid - nrm * 3.0));
            if l == r {
                continue;
            }
            if r {
                nrm = -nrm;
            }
            let band = |m: &mut WorldMesh, d0: f32, d1: f32, paint: Paint| {
                let q = [a + nrm * d0, b + nrm * d0, b + nrm * d1, a + nrm * d1];
                quad(m, q.map(|p| V3::new(p.x, y, p.y)), V3::Y, paint);
            };
            if ocean {
                band(m, SURF_FOAM_M.0, SURF_FOAM_M.1, foam);
                // Breakers only along some of the shore, never the same stretch twice.
                if value_noise(seed ^ KEY_SURF, mid.x, mid.y, 40.0) > 0.45 {
                    band(m, SURF_BREAK_M.0, SURF_BREAK_M.1, breakers);
                }
            } else {
                band(
                    m,
                    SURF_FOAM_M.0,
                    0.5 * (SURF_FOAM_M.0 + SURF_FOAM_M.1),
                    foam,
                );
            }
        }
    }
}

/// The surf lies this far above the water (over it, under the beach's edge)...
const SURF_LIFT_M: f32 = 0.03;
/// ...the foam this far out from the waterline, and the breakers this far.
const SURF_FOAM_M: (f32, f32) = (0.2, 3.5);
const SURF_BREAK_M: (f32, f32) = (7.0, 9.0);
const KEY_SURF: u64 = 0x5355_5246;

/// How far out past the place's sides the horizon's rings of samples lie.
pub const HORIZON_RINGS_M: [f32; 12] = [
    24.0, 60.0, 110.0, 180.0, 270.0, 390.0, 540.0, 730.0, 970.0, 1270.0, 1650.0, 2150.0,
];

/// What lies past the place's sides, out to the horizon ([`crate::edges::beyond`]): one
/// mesh on a grid that shares the far terrain's samples along the sides (so the two meet
/// with no gap) and steps out by [`HORIZON_RINGS_M`]; past a corner the two sides blend by
/// angle. Ridges behind mountains and hills, a low forest or flat yards; under the water
/// (and left out) past the coast, where the water plane reaches out instead. Colored by
/// kind, height and slope like the ground. Empty when no side is land.
pub fn build_horizon(place: &Place, grid: &TerrainGrid, pal: &Palette, seed: u64) -> WorldMesh {
    use crate::edges::beyond;
    use place::{EdgeKind, EAST, NORTH, SOUTH, WEST};
    let mut m = WorldMesh::default();
    let kinds = place.terrain.edges;
    if kinds.iter().all(|k| k.is_water() || *k == EdgeKind::Open) {
        return m;
    }
    let fg = far_grid(grid);
    let (fx, fz) = (fg.xs.len(), fg.zs.len());
    let rings = HORIZON_RINGS_M.len();
    let o = grid.pos(0, 0);
    let end = grid.pos(grid.w - 1, grid.h - 1);
    // The axes: the rings before the place, its far-terrain samples, the rings after.
    let axis = |lo: f32, hi: f32, inner: &[f32]| {
        let mut v: Vec<f32> = HORIZON_RINGS_M.iter().rev().map(|r| lo - r).collect();
        v.extend_from_slice(inner);
        v.extend(HORIZON_RINGS_M.iter().map(|r| hi + r));
        v
    };
    let inner_x: Vec<f32> = fg.xs.iter().map(|&i| grid.pos(i, 0).x).collect();
    let inner_z: Vec<f32> = fg.zs.iter().map(|&j| grid.pos(0, j).y).collect();
    let (ax, az) = (axis(o.x, end.x, &inner_x), axis(o.y, end.y, &inner_z));
    let (nx, nz) = (ax.len(), az.len());
    let (i0, i1, j0, j1) = (rings, rings + fx - 1, rings, rings + fz - 1);
    let wl = grid.water_level;
    // Heights, and the side whose kind colors each sample.
    let mut heights = vec![0.0f32; nx * nz];
    let mut owner = vec![EdgeKind::Open; nx * nz];
    for j in 0..nz {
        for i in 0..nx {
            let (ci, cj) = (i.clamp(i0, i1), j.clamp(j0, j1));
            let base = fg.heights[(cj - j0) * fx + (ci - i0)];
            let (bx, bz) = (ax[ci] - o.x, az[cj] - o.y);
            let dx = (ax[i] - ax[ci]).abs();
            let dz = (az[j] - az[cj]).abs();
            let sx = if i < i0 { WEST } else { EAST };
            let sz = if j < j0 { NORTH } else { SOUTH };
            let (rise, kind) = match (dx > 0.0, dz > 0.0) {
                (false, false) => (0.0, kinds[sz]),
                (true, false) => (beyond(seed, sx, kinds[sx], bz, dx), kinds[sx]),
                (false, true) => (beyond(seed, sz, kinds[sz], bx, dz), kinds[sz]),
                (true, true) => {
                    let s = sqrt(dx * dx + dz * dz);
                    let w = smooth(dz / (dx + dz));
                    let a = beyond(seed, sx, kinds[sx], bz, s);
                    let b = beyond(seed, sz, kinds[sz], bx, s);
                    (lerp(a, b, w), if w < 0.5 { kinds[sx] } else { kinds[sz] })
                }
            };
            heights[j * nx + i] = base + rise;
            owner[j * nx + i] = kind;
        }
    }
    let normal = |i: usize, j: usize| {
        let (a, b) = (i.saturating_sub(1), (i + 1).min(nx - 1));
        let (c, d) = (j.saturating_sub(1), (j + 1).min(nz - 1));
        let gx = (heights[j * nx + b] - heights[j * nx + a]) / (ax[b] - ax[a]);
        let gz = (heights[d * nx + i] - heights[c * nx + i]) / (az[d] - az[c]);
        V3::new(-gx, 1.0, -gz).normalize()
    };
    let paint = Paint::new(pal.lawn, Pattern::Terrain);
    for j in 0..nz {
        for i in 0..nx {
            let (p, y) = (V2::new(ax[i], az[j]), heights[j * nx + i]);
            let nrm = normal(i, j);
            let rgb = horizon_rgb(owner[j * nx + i], y - wl, nrm, p, pal, seed);
            m.vertices.push(WorldVertex {
                pos: [p.x, y, p.y],
                normal: nrm.to_array(),
                uv: [p.x, p.y],
                color: paint.with_rgb(rgb).color,
                emissive: paint.emissive,
            });
        }
    }
    for cz in 0..nz - 1 {
        for cx in 0..nx - 1 {
            // The place's own cells are the far terrain's.
            if (i0..i1).contains(&cx) && (j0..j1).contains(&cz) {
                continue;
            }
            let v00 = (cz * nx + cx) as u32;
            let (v10, v01, v11) = (v00 + 1, v00 + nx as u32, v00 + nx as u32 + 1);
            let dry = [v00, v10, v01, v11]
                .iter()
                .any(|&v| heights[v as usize] > wl - HORIZON_WET_M);
            if dry {
                m.indices.extend_from_slice(&[v00, v11, v10, v00, v01, v11]);
            }
        }
    }
    m
}

/// Horizon cells all this far under the water are left out (the water plane covers them).
const HORIZON_WET_M: f32 = 0.5;

/// Hermite step over `[0, 1]`.
fn smooth(t: f32) -> f32 {
    let t = clamp(t, 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The color of the land past a side of `kind`, `above` meters over the water: dry grass
/// in the low hills going to scrub, bare rock up on the mountains, dark canopy over a
/// forest, ballast and rust in the yards; then wet near the water, soil on steep slopes and
/// mottled, as the ground is.
fn horizon_rgb(kind: place::EdgeKind, above: f32, nrm: V3, p: V2, pal: &Palette, seed: u64) -> Rgb {
    use place::EdgeKind;
    let up = |a: f32, b: f32| (smooth((above - a) / (b - a)) * 256.0) as u32;
    let rgb = match kind {
        EdgeKind::Mountains => {
            let rock = mix(shade(pal.soil, 170), pal.concrete_dark, 110);
            mix(
                mix(pal.lawn_dry, pal.scrub, up(20.0, 140.0)),
                rock,
                up(180.0, 420.0),
            )
        }
        EdgeKind::Forest => mix(shade(pal.leaf_green, 140), shade(pal.palm_green, 120), 90),
        EdgeKind::RailYards => mix(pal.concrete_dark, shade(pal.soil, 190), 110),
        _ => mix(pal.lawn_dry, pal.scrub, up(10.0, 120.0)),
    };
    ground_rgb(rgb, above, 0.0, nrm, p, pal, seed)
}

/// The water plane of chunk `c` if any of its zone cells is water: a grid of `split` ×
/// `split` flat quads at the water level, reaching [`OCEAN_SKIRT_M`] past each place edge
/// the chunk lies on where that edge is water.
#[allow(clippy::too_many_arguments)]
fn water(
    place: &Place,
    grid: &TerrainGrid,
    pal: &Palette,
    c: [usize; 2],
    res: usize,
    zone_at: &dyn Fn(usize, usize) -> ZoneId,
    split: u32,
    m: &mut WorldMesh,
) {
    let (c0x, c0z) = (c[0] * res, c[1] * res);
    let wet = |x0: usize, x1: usize, z0: usize, z1: usize| {
        (z0..z1).any(|z| (x0..x1).any(|x| zone_at(x, z) == ZoneId::Water))
    };
    if !wet(c0x, c0x + res, c0z, c0z + res) {
        return;
    }
    let [nx, nz] = place.header.size_chunks.map(usize::from);
    let o = grid.pos(c0x, c0z);
    let size = grid.cell * res as f32;
    let (mut x0, mut x1, mut z0, mut z1) = (o.x, o.x + size, o.y, o.y + size);
    if c[0] == 0 && wet(c0x, c0x + 1, c0z, c0z + res) {
        x0 -= OCEAN_SKIRT_M;
    }
    if c[0] + 1 == nx && wet(c0x + res - 1, c0x + res, c0z, c0z + res) {
        x1 += OCEAN_SKIRT_M;
    }
    if c[1] == 0 && wet(c0x, c0x + res, c0z, c0z + 1) {
        z0 -= OCEAN_SKIRT_M;
    }
    if c[1] + 1 == nz && wet(c0x, c0x + res, c0z + res - 1, c0z + res) {
        z1 += OCEAN_SKIRT_M;
    }
    let y = grid.water_level;
    let paint = Paint::new(pal.water, Pattern::Water);
    // Inside the chunk a grid; the skirts as single wide quads around it.
    let k = split as f32;
    for j in 0..split {
        for i in 0..split {
            let (a, b) = (o.x + size * i as f32 / k, o.x + size * (i + 1) as f32 / k);
            let (cz0, cz1) = (o.y + size * j as f32 / k, o.y + size * (j + 1) as f32 / k);
            water_quad(m, [a, b, cz0, cz1], y, paint);
        }
    }
    let (ix0, ix1, iz0, iz1) = (o.x, o.x + size, o.y, o.y + size);
    for r in [
        [x0, ix0, z0, z1],
        [ix1, x1, z0, z1],
        [ix0, ix1, z0, iz0],
        [ix0, ix1, iz1, z1],
    ] {
        if r[1] - r[0] > 0.5 && r[3] - r[2] > 0.5 {
            water_quad(m, r, y, paint);
        }
    }
}

fn water_quad(m: &mut WorldMesh, r: [f32; 4], y: f32, paint: Paint) {
    let [x0, x1, z0, z1] = r;
    let q = [
        V3::new(x0, y, z0),
        V3::new(x1, y, z0),
        V3::new(x1, y, z1),
        V3::new(x0, y, z1),
    ];
    quad(m, q, V3::Y, paint);
}
