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

use alloc::vec;
use alloc::vec::Vec;

use place::{Place, ZoneId, SIDEWALK_WIDTH_M};

use crate::math::{clamp, floor, lerp, segment_distance, value_noise, V2, V3};
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
                    let mut rgb: Rgb = acc.map(|a| (a / 16) as u8);
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
                    rgb = shade(rgb, (tone - 10.0 + 20.0 * coarse) as u32);
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
            water(place, grid, pal, [cx, cz], res, &zone_at, &mut m);
            let dst = out.at(center, MeshKind::Ground);
            let base = dst.vertices.len() as u32;
            dst.vertices.extend_from_slice(&m.vertices);
            dst.indices.extend(m.indices.iter().map(|i| i + base));
        }
    }
}

/// The water plane of chunk `c` if any of its zone cells is water: a grid of flat quads at
/// the water level, reaching [`OCEAN_SKIRT_M`] past each place edge the chunk lies on
/// where that edge is water.
fn water(
    place: &Place,
    grid: &TerrainGrid,
    pal: &Palette,
    c: [usize; 2],
    res: usize,
    zone_at: &dyn Fn(usize, usize) -> ZoneId,
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
    let k = WATER_SPLIT as f32;
    for j in 0..WATER_SPLIT {
        for i in 0..WATER_SPLIT {
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
