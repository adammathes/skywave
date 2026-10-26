//! The macro map: stages 1–2 of `docs/WORLD_GENERATION.md` for a whole place.
//!
//! [`MacroMap::generate`] runs, in order:
//! 1. coast: shoreline with bays and beach widths ([`Shore`]);
//! 2. terrain parameters: plain width and rise, ridges;
//! 3. layout: strip extent, downtown seed, highway centerline toward the pass, valleys,
//!    the canal pocket;
//! 4. heightfield at 4 m ([`terrain::heights`]), flattened where the canal pocket goes;
//! 5. zones on 16 m cells: rules ([`zones::score`]), two majority passes, then the
//!    structural zones on top (beach, strip, highway, canals, parks). The beach band is
//!    measured from the true shoreline; the strip, downtown and canal pocket from the
//!    smoothed one ([`Shore::road`]);
//! 6. terrain smoothing by zone (urban zones blend toward a box-blurred copy, so roads sit
//!    well), canals dug, and the highway corridor carved: a bed of at most 4.8% from the
//!    coast inland ([`terrain::corridor_bed`]), through a valley cut into the hills before
//!    the zones are painted ([`terrain::valley`]), so the highway climbs to a real pass in
//!    cuts of tens of meters ([`terrain::carve_corridor`]);
//! 7. pass (the highway's highest point) and points of interest.
//!
//! Grids: heights are `width × height` samples, 4 m apart, sample `(ix, iz)` at world
//! `(4 ix, 4 iz)`; zones are `zone_width × zone_height` cells of 16 m (4×4 samples), cell
//! `(cx, cz)` covering world `[16 cx, 16 cx + 16) × [16 cz, 16 cz + 16)`. Both row-major,
//! `z` major. A zone cell is `Water` iff the mean of its 16 samples is below
//! `water_level` (or a canal covers it).

use alloc::vec;
use alloc::vec::Vec;

use place::ZoneId;
use skywave_core::{hash64, hash_bytes, Rng, Seed};

use super::terrain::{self, smoothstep, Ridge, Shore, TerrainParams, Valley};
use super::zones::{self, cell_center, ZoneInputs};
use super::{stage, CELL_M, RES_PER_CHUNK, ZONE_CELL_M, ZONE_SAMPLES};
use crate::math::{clamp, floor, lerp, segment_distance, signed_area, sqrt, value_noise, V2};
use crate::template::{CoastSide, CoastWater, RegionTemplate};

/// Maps between world coordinates and the canonical frame terrain is built in: `u` inland
/// from the place edge the water lies along, `v` along that edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub side: CoastSide,
    /// Height samples in x and z.
    pub samples: [u32; 2],
}

impl Frame {
    pub fn new(side: CoastSide, nx: u32, nz: u32) -> Frame {
        Frame {
            side,
            samples: [nx, nz],
        }
    }

    fn across(&self) -> bool {
        matches!(self.side, CoastSide::West | CoastSide::East)
    }

    /// Samples along `u`.
    pub fn n_u(&self) -> u32 {
        if self.across() {
            self.samples[0]
        } else {
            self.samples[1]
        }
    }

    /// Samples along `v`.
    pub fn n_v(&self) -> u32 {
        if self.across() {
            self.samples[1]
        } else {
            self.samples[0]
        }
    }

    pub fn len_u(&self) -> f32 {
        (self.n_u() - 1) as f32 * CELL_M
    }

    pub fn len_v(&self) -> f32 {
        (self.n_v() - 1) as f32 * CELL_M
    }

    fn extent(&self) -> [f32; 2] {
        [
            (self.samples[0] - 1) as f32 * CELL_M,
            (self.samples[1] - 1) as f32 * CELL_M,
        ]
    }

    /// World sample index of canonical sample `(iu, iv)`.
    pub fn index(&self, iu: u32, iv: u32) -> usize {
        let [nx, nz] = self.samples;
        let (ix, iz) = match self.side {
            CoastSide::West => (iu, iv),
            CoastSide::East => (nx - 1 - iu, iv),
            CoastSide::North => (iv, iu),
            CoastSide::South => (iv, nz - 1 - iu),
        };
        iz as usize * nx as usize + ix as usize
    }

    pub fn to_world(&self, u: f32, v: f32) -> [f32; 2] {
        let [ex, ez] = self.extent();
        match self.side {
            CoastSide::West => [u, v],
            CoastSide::East => [ex - u, v],
            CoastSide::North => [v, u],
            CoastSide::South => [v, ez - u],
        }
    }

    pub fn to_canon(&self, p: [f32; 2]) -> (f32, f32) {
        let [ex, ez] = self.extent();
        match self.side {
            CoastSide::West => (p[0], p[1]),
            CoastSide::East => (ex - p[0], p[1]),
            CoastSide::North => (p[1], p[0]),
            CoastSide::South => (ez - p[1], p[0]),
        }
    }

    /// A canonical direction `(du, dv)` in world axes.
    pub fn dir_to_world(&self, du: f32, dv: f32) -> [f32; 2] {
        match self.side {
            CoastSide::West => [du, dv],
            CoastSide::East => [-du, dv],
            CoastSide::North => [dv, du],
            CoastSide::South => [dv, -du],
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterKind {
    Ocean = 0,
    Lake = 1,
    Canal = 2,
}

/// A body of water as a simple polygon, counter-clockwise seen from above. Its surface is
/// at `MacroMap::water_level`; the terrain under it is below that.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterBody {
    pub kind: WaterKind,
    pub polygon: Vec<[f32; 2]>,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroPoiKind {
    /// On the shoreline at the strip's center, the pier runs along `facing`.
    Pier = 0,
    /// Where the highway meets the coast.
    Harbor = 1,
    /// High ground with a view of the water along `facing`.
    Overlook = 2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacroPoi {
    pub kind: MacroPoiKind,
    pub pos: [f32; 2],
    /// Unit direction toward the water.
    pub facing: [f32; 2],
}

/// The main drag: a band parallel to the beach.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    /// Centerline of the band, every 16 m along the coast, world coordinates.
    pub centerline: Vec<[f32; 2]>,
    /// Gap between the beach's inland edge and the band.
    pub inland_m: f32,
    /// Depth of the band.
    pub width_m: f32,
    /// Length along the coast.
    pub length_m: f32,
}

/// Stages 1–2 for a whole place. See the module docs for the grids.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroMap {
    pub seed: Seed,
    pub size_chunks: [u16; 2],
    pub frame: Frame,
    /// Height samples in x and z.
    pub width: u32,
    pub height: u32,
    pub heights: Vec<f32>,
    pub water_level: f32,
    pub water: Vec<WaterBody>,
    pub zone_width: u32,
    pub zone_height: u32,
    pub zones: Vec<ZoneId>,
    /// The shoreline in the canonical frame; see [`MacroMap::coast_distance`].
    pub shore: Shore,
    /// Downtown seed point.
    pub downtown: [f32; 2],
    pub strip: Strip,
    /// Highway centerline from the coast to the inland edge, every ~16 m.
    pub highway: Vec<[f32; 2]>,
    /// The corridor bed's height at every point of `highway` ([`terrain::corridor_bed`]):
    /// at most 4.8% steep, carved into `heights`.
    pub highway_bed: Vec<f32>,
    /// How far carving the corridor moved each height sample (carved minus before;
    /// negative in cuts, zero away from the corridor). The arterial stage keeps roads off
    /// its cut and fill slopes.
    pub corridor: Vec<f32>,
    /// The highway's highest point where it crosses the ridges; `None` on flat places.
    pub pass: Option<[f32; 2]>,
    pub pois: Vec<MacroPoi>,
}

/// Terrain smoothing strength per zone (by `ZoneId` discriminant).
const FLATTEN: [f32; 12] = [
    0.85, // Downtown
    0.85, // Strip
    0.7,  // Residential
    0.35, // Suburb
    0.9,  // Industrial
    0.8,  // Commercial
    0.5,  // Park
    0.0,  // Beach
    0.6,  // Farmland
    0.0,  // Hills
    0.8,  // Highway
    0.0,  // Water
];

/// Along-coast distances from downtown count this much: downtowns stretch along the shore.
const DOWNTOWN_ALONG: f32 = 0.75;

/// Zones a canal may be dug through.
const CANAL_ZONES: [ZoneId; 5] = [
    ZoneId::Residential,
    ZoneId::Suburb,
    ZoneId::Commercial,
    ZoneId::Industrial,
    ZoneId::Park,
];

/// An axis-aligned world rectangle `[x0, x1) × [z0, z1)`.
type Rect = [f32; 4];

impl MacroMap {
    /// Generates the macro map. `template` must have passed
    /// [`RegionTemplate::validate`]; sizes below one chunk are raised to one.
    pub fn generate(seed: Seed, template: &RegionTemplate, size_chunks: [u16; 2]) -> MacroMap {
        let t = template;
        let e = &t.elevation;
        let l = &t.layout;
        let wl = e.water_level_m;
        let size_chunks = [size_chunks[0].max(1), size_chunks[1].max(1)];
        let nx = u32::from(size_chunks[0]) * RES_PER_CHUNK;
        let nz = u32::from(size_chunks[1]) * RES_PER_CHUNK;
        let frame = Frame::new(t.coast.side, nx, nz);
        let (len_u, len_v) = (frame.len_u(), frame.len_v());

        // 1. Coast.
        let shore = Shore::generate(seed, t, frame.n_v(), len_v);
        let bay_v = shore.bays.first().map_or(len_v * 0.5, |b| b.v);
        let shore_mean = mean(&shore.u);
        let beach_mean = mean(&shore.beach);

        // 2. Plain and ridges.
        let mut rng = Rng::new(seed, &[stage::TERRAIN]);
        let plain_width = rng.range_f32(e.plain_width_m.0, e.plain_width_m.1);
        let plain_rise = rng.range_f32(e.plain_rise_m.0, e.plain_rise_m.1);
        let hills_extent = len_u - shore_mean - beach_mean - plain_width;
        let mut n_ridges = rng.range(u32::from(e.ridge_count.0), u32::from(e.ridge_count.1) + 1);
        if hills_extent < 200.0 || e.ridge_height_m.1 <= 0.0 {
            n_ridges = 0;
        }
        let mut crest: Vec<f32> = (0..n_ridges)
            .map(|_| rng.range_f32(e.ridge_height_m.0, e.ridge_height_m.1))
            .collect();
        crest.sort_by(|a, b| a.total_cmp(b));
        let ridges: Vec<Ridge> = crest
            .iter()
            .enumerate()
            .map(|(k, &height)| Ridge {
                at: hills_extent * (0.4 + 0.6 * (k as f32 + 0.5) / n_ridges as f32)
                    + rng.range_f32(-40.0, 40.0),
                height,
                half_width: rng.range_f32(e.ridge_width_m.0, e.ridge_width_m.1),
            })
            .collect();
        let plain_top = wl + e.beach_height_m + plain_rise;

        // 3. Layout.
        let mut rng = Rng::new(seed, &[stage::LAYOUT]);
        let strip_len = rng
            .range_f32(l.strip.length_m.0, l.strip.length_m.1)
            .min(len_v - 128.0)
            .max(ZONE_CELL_M);
        let strip_inland = rng.range_f32(l.strip.inland_m.0, l.strip.inland_m.1);
        let strip_width = rng.range_f32(l.strip.width_m.0, l.strip.width_m.1);
        let strip_v0 = clamp(bay_v - 0.5 * strip_len, 64.0, len_v - 64.0);
        let strip_v1 = clamp(bay_v + 0.5 * strip_len, 64.0, len_v - 64.0);
        // The highway leaves past one end of the strip (toward +v or -v); the canal pocket
        // and shore park take the other end, the first valley the other side of the hills.
        let toward = if rng.next_f32() < 0.5 { 1.0 } else { -1.0 };
        let pass_v = len_v
            * if toward > 0.0 {
                rng.range_f32(0.7, 0.84)
            } else {
                rng.range_f32(0.16, 0.3)
            };
        let start_v = clamp(
            if toward > 0.0 {
                strip_v1 + 70.0
            } else {
                strip_v0 - 70.0
            },
            48.0,
            len_v - 48.0,
        );
        let p0 = shore.point(start_v, shore.beach_at(start_v) + 24.0);
        let crest_u = shore_mean + beach_mean + plain_width + ridges.first().map_or(0.0, |r| r.at);
        let p2 = (crest_u.min(len_u * 0.82).max(p0.0 + 200.0), pass_v);
        let p1 = (0.5 * (p0.0 + p2.0), lerp(start_v, pass_v, 0.5));
        let end_v = clamp(pass_v + rng.range_f32(-120.0, 120.0), 48.0, len_v - 48.0);
        let p3 = (len_u + CELL_M, end_v);
        let highway_c = catmull_rom(&[p0, p1, p2, p3], 16.0);

        let downtown_d = shore.beach_at(bay_v)
            + strip_inland
            + strip_width
            + rng.range_f32(l.downtown_inland_m.0, l.downtown_inland_m.1);
        let downtown_c = (shore.road_u(bay_v) + downtown_d, bay_v);

        let mut valleys: Vec<Valley> = Vec::new();
        if !ridges.is_empty() {
            let n = rng.range(u32::from(e.valley_count.0), u32::from(e.valley_count.1) + 1);
            for k in 0..n {
                let length = rng.range_f32(e.valley_length_m.0, e.valley_length_m.1);
                let width = rng.range_f32(e.valley_width_m.0, e.valley_width_m.1);
                let rise = rng.range_f32(e.valley_floor_m.0, e.valley_floor_m.1);
                let (v, d0, length, floor0) = if k == 0 {
                    // Opens off the plain, on the far side from the pass.
                    let f = if toward > 0.0 {
                        rng.range_f32(0.08, 0.22)
                    } else {
                        rng.range_f32(0.78, 0.92)
                    };
                    (
                        len_v * f,
                        beach_mean + plain_width - 60.0,
                        length,
                        plain_top + 1.5,
                    )
                } else {
                    // Perched between ridges.
                    let v = len_v * rng.range_f32(0.4, 0.6);
                    let d0 = beach_mean + plain_width + 0.35 * hills_extent;
                    let perch = plain_top + 0.15 * ridges[0].height;
                    (v, d0, length.min(0.6 * hills_extent), perch)
                };
                let half = 0.5 * length;
                valleys.push(Valley {
                    u: shore.shore_u(v) + d0 + half,
                    v,
                    half_length: half,
                    half_width: 0.5 * width,
                    floor: (floor0, floor0 + rise),
                });
            }
        }

        let mut canals: Vec<Rect> = Vec::new();
        if let Some(c) = &t.coast.canals {
            if rng.next_f32() < c.chance {
                let n = rng.range(u32::from(c.fingers.0), u32::from(c.fingers.1) + 1);
                let length = rng.range_f32(c.length_m.0, c.length_m.1);
                let span = (n.max(1) - 1) as f32 * c.spacing_m;
                let vc = clamp(
                    bay_v - toward * 0.5 * strip_len,
                    0.5 * span + 48.0,
                    len_v - 0.5 * span - 48.0,
                );
                let d0 = shore.beach_at(vc) + strip_inland + strip_width + 48.0;
                let u0 = shore.road_u(vc) + d0;
                let hw = 0.5 * c.width_m;
                let first = vc - 0.5 * span;
                let mut canon: Vec<[f32; 4]> = (0..n)
                    .map(|i| {
                        let v = first + i as f32 * c.spacing_m;
                        [u0, u0 + length, v - hw, v + hw]
                    })
                    .collect();
                if n > 1 {
                    canon.push([u0, u0 + c.width_m, first - hw, first + span + hw]);
                    canon.push([
                        u0 + length - c.width_m,
                        u0 + length,
                        first - hw,
                        first + span + hw,
                    ]);
                }
                canals = canon.iter().map(|r| snap_rect(&frame, *r)).collect();
            }
        }

        // 4. Heightfield.
        let highway_v = if ridges.is_empty() {
            Vec::new()
        } else {
            v_per_u(&highway_c, frame.n_u() as usize)
        };
        let bounds = t.height_bounds();
        let params = TerrainParams {
            shore: &shore,
            water_level: wl,
            shelf_depth: e.shelf_depth_m,
            beach_height: e.beach_height_m,
            plain_width,
            plain_rise,
            relief: e.relief_m,
            warp: e.warp_m,
            ridges: &ridges,
            valleys: &valleys,
            highway_v: &highway_v,
            pass_depth: l.highway.pass_depth,
            pass_half_width: l.highway.pass_width_m,
            bounds,
        };
        let mut heights = terrain::heights(seed, &frame, &params);
        let (w, h) = (nx as usize, nz as usize);
        if !canals.is_empty() {
            flatten_pocket(&mut heights, w, h, &canals, wl);
        }
        // The pass: the highway's bed profile, and the hills cut down along it.
        let to_world = |c: (f32, f32)| frame.to_world(c.0, c.1);
        let highway: Vec<[f32; 2]> = highway_c.iter().map(|&c| to_world(c)).collect();
        let bed = terrain::corridor_bed(&heights, w, h, &highway, wl);
        terrain::valley(&mut heights, &frame, &highway_c, &bed);

        // 5. Zones.
        let downtown = to_world(downtown_c);
        let (zw, zh) = ((nx / ZONE_SAMPLES) as usize, (nz / ZONE_SAMPLES) as usize);
        let mean_h = cell_means(&heights, w, zw, zh);
        let inputs = zone_inputs(&frame, &shore, &mean_h, zw, zh, downtown, &highway, wl);
        let mut zone = zones::score(seed, t, &inputs);
        for _ in 0..2 {
            zone = zones::majority(&zone, zw, zh, |q| q != ZoneId::Water);
        }
        let corridor = 0.5 * l.highway.corridor_m;
        for cz in 0..zh {
            for cx in 0..zw {
                let i = cz * zw + cx;
                if zone[i] == ZoneId::Water {
                    continue;
                }
                let (x, z) = cell_center(cx, cz);
                let (u, v) = frame.to_canon([x, z]);
                let beach = shore.beach_at(v);
                let behind = shore.road_distance(u, v) - beach;
                if inputs.coast[i] < beach {
                    zone[i] = ZoneId::Beach;
                } else if (strip_v0..=strip_v1).contains(&v)
                    && (strip_inland..strip_inland + strip_width).contains(&behind)
                {
                    zone[i] = ZoneId::Strip;
                }
                if zone[i] != ZoneId::Beach
                    && inputs.highway[i] <= corridor + ZONE_CELL_M * 2.0
                    && polyline_distance(&highway, [x, z]) <= corridor
                {
                    zone[i] = ZoneId::Highway;
                }
            }
        }
        // Canals: all or nothing, only through zones they belong in.
        let canal_cells: Vec<usize> = (0..zw * zh)
            .filter(|&i| {
                let (x, z) = cell_center(i % zw, i / zw);
                canals.iter().any(|r| in_rect(r, x, z))
            })
            .collect();
        if canal_cells.iter().all(|&i| CANAL_ZONES.contains(&zone[i])) {
            for &i in &canal_cells {
                zone[i] = ZoneId::Water;
            }
        } else {
            canals.clear();
        }
        let geo = ParkGeo {
            frame: &frame,
            shore: &shore,
            coast: &inputs.coast,
            highway: &inputs.highway,
            downtown,
            zw,
            zh,
        };
        let strip_band = (strip_v0, strip_v1, strip_inland + strip_width);
        place_parks(seed, t, &geo, strip_band, toward, &mut zone);

        // 6. Smooth by zone, dig canals.
        flatten_by_zone(&mut heights, w, h, &zone, zw, zh);
        for r in &canals {
            let (x0, x1) = (sample_span(r[0], w), sample_span(r[1], w));
            let (z0, z1) = (sample_span(r[2], h), sample_span(r[3], h));
            for row in heights.chunks_exact_mut(w).take(z1).skip(z0) {
                for hp in &mut row[x0..x1] {
                    *hp = wl - 1.8;
                }
            }
        }

        // The highway corridor, last, so nothing smooths its bed.
        let corridor = terrain::carve_corridor(&mut heights, w, h, &highway, &bed, wl);

        // 7. Pass, water bodies, points of interest.
        let mut map = MacroMap {
            seed,
            size_chunks,
            frame,
            width: nx,
            height: nz,
            heights,
            water_level: wl,
            water: Vec::new(),
            zone_width: zw as u32,
            zone_height: zh as u32,
            zones: zone,
            shore,
            downtown,
            strip: Strip {
                centerline: Vec::new(),
                inland_m: strip_inland,
                width_m: strip_width,
                length_m: strip_v1 - strip_v0,
            },
            highway,
            highway_bed: bed,
            corridor,
            pass: None,
            pois: Vec::new(),
        };
        if !ridges.is_empty() {
            let mut best = (f32::NEG_INFINITY, None);
            for &p in &map.highway {
                if frame.to_canon(p).0 > len_u - 160.0 {
                    continue;
                }
                let hp = map.height_at(p[0], p[1]);
                if hp > best.0 {
                    best = (hp, Some(p));
                }
            }
            map.pass = best.1;
        }
        let mut v = strip_v0;
        while v <= strip_v1 {
            let d = map.shore.beach_at(v) + strip_inland + 0.5 * strip_width;
            map.strip
                .centerline
                .push(to_world(map.shore.road_point(v, d)));
            v += ZONE_CELL_M;
        }
        let kind = match t.coast.water {
            CoastWater::Ocean => WaterKind::Ocean,
            CoastWater::Lake => WaterKind::Lake,
        };
        let mut coast = vec![to_world((0.0, 0.0))];
        let mut v = 0.0;
        loop {
            coast.push(to_world((map.shore.shore_u(v), v)));
            if v >= len_v {
                break;
            }
            v = (v + ZONE_CELL_M).min(len_v);
        }
        coast.push(to_world((0.0, len_v)));
        map.water.push(WaterBody {
            kind,
            polygon: ccw(coast),
        });
        for r in &canals {
            map.water.push(WaterBody {
                kind: WaterKind::Canal,
                polygon: ccw(vec![[r[0], r[2]], [r[1], r[2]], [r[1], r[3]], [r[0], r[3]]]),
            });
        }
        let seaward = |v: f32| {
            let (du, dv) = map.shore.normal(v);
            frame.dir_to_world(-du, -dv)
        };
        if l.pier {
            map.pois.push(MacroPoi {
                kind: MacroPoiKind::Pier,
                pos: to_world(map.shore.point(bay_v, 0.0)),
                facing: seaward(bay_v),
            });
        }
        map.pois.push(MacroPoi {
            kind: MacroPoiKind::Harbor,
            pos: to_world(map.shore.point(start_v, 0.0)),
            facing: seaward(start_v),
        });
        let overlooks = overlooks(seed, t, &map);
        for p in overlooks {
            let (_, v) = frame.to_canon(p);
            map.pois.push(MacroPoi {
                kind: MacroPoiKind::Overlook,
                pos: p,
                facing: seaward(v),
            });
        }
        map
    }

    /// Bilinear height at world `(x, z)`, clamped to the grid.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let (w, h) = (self.width as usize, self.height as usize);
        let fx = clamp(x / CELL_M, 0.0, (w - 1) as f32);
        let fz = clamp(z / CELL_M, 0.0, (h - 1) as f32);
        let (ix, iz) = ((fx as usize).min(w - 2), (fz as usize).min(h - 2));
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let at = |i: usize, j: usize| self.heights[j * w + i];
        let a = lerp(at(ix, iz), at(ix + 1, iz), tx);
        let b = lerp(at(ix, iz + 1), at(ix + 1, iz + 1), tx);
        lerp(a, b, tz)
    }

    /// The zone of the cell containing world `(x, z)`, clamped to the grid.
    pub fn zone_at(&self, x: f32, z: f32) -> ZoneId {
        let cx = clamp(floor(x / ZONE_CELL_M), 0.0, (self.zone_width - 1) as f32) as usize;
        let cz = clamp(floor(z / ZONE_CELL_M), 0.0, (self.zone_height - 1) as f32) as usize;
        self.zones[cz * self.zone_width as usize + cx]
    }

    /// Signed distance inland from the shoreline at world `(x, z)`: negative in the big
    /// water, measured perpendicular to the local shore.
    pub fn coast_distance(&self, x: f32, z: f32) -> f32 {
        let (u, v) = self.frame.to_canon([x, z]);
        self.shore.distance(u, v)
    }

    /// Content hash over every field, for determinism tests.
    pub fn hash(&self) -> u64 {
        let mut b: Vec<u8> = Vec::with_capacity(self.heights.len() * 4 + self.zones.len() + 4096);
        let f = |b: &mut Vec<u8>, x: f32| b.extend_from_slice(&x.to_bits().to_le_bytes());
        b.extend_from_slice(&self.seed.0.to_le_bytes());
        b.extend_from_slice(&self.width.to_le_bytes());
        b.extend_from_slice(&self.height.to_le_bytes());
        for &x in &self.heights {
            f(&mut b, x);
        }
        f(&mut b, self.water_level);
        for &x in self.highway_bed.iter().chain(&self.corridor) {
            f(&mut b, x);
        }
        for body in &self.water {
            b.push(body.kind as u8);
            for p in &body.polygon {
                f(&mut b, p[0]);
                f(&mut b, p[1]);
            }
        }
        b.extend(self.zones.iter().map(|&z| z as u8));
        for p in [self.downtown]
            .iter()
            .chain(&self.strip.centerline)
            .chain(&self.highway)
            .chain(&self.pass)
        {
            f(&mut b, p[0]);
            f(&mut b, p[1]);
        }
        for p in &self.pois {
            b.push(p.kind as u8);
            f(&mut b, p.pos[0]);
            f(&mut b, p.pos[1]);
        }
        hash_bytes(0x004D_4143_524F, &b)
    }
}

fn mean(xs: &[f32]) -> f32 {
    let mut acc = 0.0;
    for &x in xs {
        acc += x;
    }
    acc / xs.len().max(1) as f32
}

/// Uniform Catmull-Rom through `pts` (end tangents mirrored), sampled about every `step`.
fn catmull_rom(pts: &[(f32, f32)], step: f32) -> Vec<(f32, f32)> {
    let n = pts.len();
    let at = |i: isize| -> (f32, f32) {
        if i < 0 {
            (2.0 * pts[0].0 - pts[1].0, 2.0 * pts[0].1 - pts[1].1)
        } else if i as usize >= n {
            (
                2.0 * pts[n - 1].0 - pts[n - 2].0,
                2.0 * pts[n - 1].1 - pts[n - 2].1,
            )
        } else {
            pts[i as usize]
        }
    };
    let mut out = Vec::new();
    for i in 0..n as isize - 1 {
        let (a, b, c, d) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let (dx, dz) = (c.0 - b.0, c.1 - b.1);
        let steps = ((sqrt(dx * dx + dz * dz) / step) as usize).max(1);
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            let q = |p0: f32, p1: f32, p2: f32, p3: f32| {
                0.5 * (2.0 * p1
                    + (p2 - p0) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
            };
            out.push((q(a.0, b.0, c.0, d.0), q(a.1, b.1, c.1, d.1)));
        }
    }
    out.push(pts[n - 1]);
    out
}

/// The polyline's `v` at every `u` sample (held beyond its ends).
pub(crate) fn v_per_u(line: &[(f32, f32)], nu: usize) -> Vec<f32> {
    let mut out = vec![f32::NAN; nu];
    for s in line.windows(2) {
        let (a, b) = if s[0].0 <= s[1].0 {
            (s[0], s[1])
        } else {
            (s[1], s[0])
        };
        let i0 = clamp(a.0 / CELL_M, 0.0, nu as f32) as usize;
        let i1 = clamp(b.0 / CELL_M + 1.0, 0.0, nu as f32) as usize;
        for (i, o) in out.iter_mut().enumerate().take(i1).skip(i0) {
            let u = i as f32 * CELL_M;
            let span = b.0 - a.0;
            let t = if span > 0.0 {
                clamp((u - a.0) / span, 0.0, 1.0)
            } else {
                0.0
            };
            *o = lerp(a.1, b.1, t);
        }
    }
    let first = line.first().map_or(0.0, |p| p.1);
    let mut last = first;
    for o in out.iter_mut() {
        if o.is_nan() {
            *o = last;
        } else {
            last = *o;
        }
    }
    out
}

/// A canonical rectangle `[u0, u1, v0, v1]` in world axes, edges snapped to zone cells.
fn snap_rect(frame: &Frame, r: [f32; 4]) -> Rect {
    let a = frame.to_world(r[0], r[2]);
    let b = frame.to_world(r[1], r[3]);
    let snap = |x: f32| floor(x / ZONE_CELL_M + 0.5) * ZONE_CELL_M;
    let (mut x0, mut x1) = (snap(a[0].min(b[0])), snap(a[0].max(b[0])));
    let (mut z0, mut z1) = (snap(a[1].min(b[1])), snap(a[1].max(b[1])));
    if x1 <= x0 {
        x1 = x0 + ZONE_CELL_M;
    }
    if z1 <= z0 {
        z1 = z0 + ZONE_CELL_M;
    }
    x0 = x0.max(0.0);
    z0 = z0.max(0.0);
    [x0, x1, z0, z1]
}

/// First sample index at or after world coordinate `x`, clamped to `n`.
fn sample_span(x: f32, n: usize) -> usize {
    let i = (x / CELL_M) as usize;
    let i = if (i as f32) * CELL_M < x { i + 1 } else { i };
    i.min(n)
}

fn in_rect(r: &Rect, x: f32, z: f32) -> bool {
    x >= r[0] && x < r[1] && z >= r[2] && z < r[3]
}

/// Lowers the land around the canals to just above the water, fading out over 60 m.
fn flatten_pocket(heights: &mut [f32], w: usize, h: usize, canals: &[Rect], wl: f32) {
    let mut bb = [
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    for r in canals {
        bb = [
            bb[0].min(r[0]),
            bb[1].max(r[1]),
            bb[2].min(r[2]),
            bb[3].max(r[3]),
        ];
    }
    let pad = 40.0;
    let reach = pad + 60.0;
    let (x0, x1) = (sample_span(bb[0] - reach, w), sample_span(bb[1] + reach, w));
    let (z0, z1) = (sample_span(bb[2] - reach, h), sample_span(bb[3] + reach, h));
    for iz in z0..z1 {
        for ix in x0..x1 {
            let (x, z) = (ix as f32 * CELL_M, iz as f32 * CELL_M);
            let dx = (bb[0] - pad - x).max(x - bb[1] - pad).max(0.0);
            let dz = (bb[2] - pad - z).max(z - bb[3] - pad).max(0.0);
            let m = 1.0 - smoothstep(0.0, 60.0, sqrt(dx * dx + dz * dz));
            let hp = &mut heights[iz * w + ix];
            if m > 0.0 && *hp > wl {
                *hp = lerp(*hp, wl + 1.8, m);
            }
        }
    }
}

/// Mean height of each zone cell's 4×4 samples.
fn cell_means(heights: &[f32], w: usize, zw: usize, zh: usize) -> Vec<f32> {
    let n = ZONE_SAMPLES as usize;
    let mut out = vec![0.0; zw * zh];
    for cz in 0..zh {
        for cx in 0..zw {
            let mut acc = 0.0;
            for j in 0..n {
                for i in 0..n {
                    acc += heights[(cz * n + j) * w + cx * n + i];
                }
            }
            out[cz * zw + cx] = acc / (n * n) as f32;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn zone_inputs(
    frame: &Frame,
    shore: &Shore,
    mean_h: &[f32],
    zw: usize,
    zh: usize,
    downtown: [f32; 2],
    highway: &[[f32; 2]],
    wl: f32,
) -> ZoneInputs {
    let n = zw * zh;
    let mut coast = vec![0.0; n];
    let mut slope = vec![0.0; n];
    let mut down = vec![0.0; n];
    let mut water = vec![false; n];
    let dt = frame.to_canon(downtown);
    for cz in 0..zh {
        for cx in 0..zw {
            let i = cz * zw + cx;
            let (x, z) = cell_center(cx, cz);
            let (u, v) = frame.to_canon([x, z]);
            coast[i] = shore.distance(u, v);
            let at = |x: usize, z: usize| mean_h[z * zw + x];
            let (x0, x1) = (cx.saturating_sub(1), (cx + 1).min(zw - 1));
            let (z0, z1) = (cz.saturating_sub(1), (cz + 1).min(zh - 1));
            let gx = (at(x1, cz) - at(x0, cz)) / ((x1 - x0).max(1) as f32 * ZONE_CELL_M);
            let gz = (at(cx, z1) - at(cx, z0)) / ((z1 - z0).max(1) as f32 * ZONE_CELL_M);
            slope[i] = sqrt(gx * gx + gz * gz);
            let (du, dv) = (u - dt.0, v - dt.1);
            down[i] = sqrt(du * du + DOWNTOWN_ALONG * DOWNTOWN_ALONG * dv * dv);
            water[i] = mean_h[i] < wl;
        }
    }
    // Highway: rasterize the centerline every 4 m, then a distance transform.
    let mut hw = vec![f32::INFINITY; n];
    for s in highway.windows(2) {
        let (dx, dz) = (s[1][0] - s[0][0], s[1][1] - s[0][1]);
        let steps = ((sqrt(dx * dx + dz * dz) / CELL_M) as usize).max(1);
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let (x, z) = (s[0][0] + dx * t, s[0][1] + dz * t);
            if x < 0.0 || z < 0.0 {
                continue;
            }
            let (cx, cz) = ((x / ZONE_CELL_M) as usize, (z / ZONE_CELL_M) as usize);
            if cx < zw && cz < zh {
                hw[cz * zw + cx] = 0.0;
            }
        }
    }
    zones::chamfer(&mut hw, zw, zh, ZONE_CELL_M);
    ZoneInputs {
        w: zw,
        h: zh,
        coast,
        slope,
        downtown: down,
        highway: hw,
        water,
    }
}

fn polyline_distance(line: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let p = V2::from_array(p);
    let mut best = f32::INFINITY;
    for s in line.windows(2) {
        let (d, _) = segment_distance(p, V2::from_array(s[0]), V2::from_array(s[1]));
        best = best.min(d);
    }
    best
}

fn ccw(mut poly: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    let v: Vec<V2> = poly.iter().map(|&p| V2::from_array(p)).collect();
    if signed_area(&v) < 0.0 {
        poly.reverse();
    }
    poly
}

/// Geometry the park placement reads.
struct ParkGeo<'a> {
    frame: &'a Frame,
    shore: &'a Shore,
    coast: &'a [f32],
    highway: &'a [f32],
    downtown: [f32; 2],
    zw: usize,
    zh: usize,
}

/// One park on the shore past the strip's far end from the highway, the rest as round
/// patches in residential and suburban land.
fn place_parks(
    seed: Seed,
    t: &RegionTemplate,
    g: &ParkGeo<'_>,
    strip: (f32, f32, f32),
    toward: f32,
    zone: &mut [ZoneId],
) {
    let rule = &t.layout.parks;
    let mut rng = Rng::new(seed, &[stage::ZONES, 100]);
    let n = rng.range(u32::from(rule.count.0), u32::from(rule.count.1) + 1);
    if n == 0 {
        return;
    }
    let len_v = g.frame.len_v();
    let (strip_v0, strip_v1, strip_depth) = strip;
    let length = rng.range_f32(120.0, 220.0);
    let depth = rng.range_f32(70.0, 120.0).min(strip_depth + 40.0);
    let v_park = clamp(
        if toward > 0.0 {
            strip_v0 - 40.0 - 0.5 * length
        } else {
            strip_v1 + 40.0 + 0.5 * length
        },
        0.5 * length,
        len_v - 0.5 * length,
    );
    let fixed = |z: ZoneId| {
        matches!(
            z,
            ZoneId::Water | ZoneId::Beach | ZoneId::Strip | ZoneId::Highway
        )
    };
    for (i, (zi, &coast)) in zone.iter_mut().zip(g.coast).enumerate() {
        let (x, z) = cell_center(i % g.zw, i / g.zw);
        let (_, v) = g.frame.to_canon([x, z]);
        let past = coast - g.shore.beach_at(v);
        let along = v - v_park;
        if !fixed(*zi)
            && (-0.5 * length..0.5 * length).contains(&along)
            && (0.0..depth).contains(&past)
        {
            *zi = ZoneId::Park;
        }
    }
    let shore_park = g
        .shore
        .point(v_park, g.shore.beach_at(v_park) + 0.5 * depth);
    let mut centers = vec![g.frame.to_world(shore_park.0, shore_park.1)];
    let edge_seed = hash64(seed.0, &[stage::ZONES, 101]);
    let mut tries = 0;
    while (centers.len() as u32) < n && tries < 400 {
        tries += 1;
        let i = rng.range(0, (g.zw * g.zh) as u32) as usize;
        let radius = rng.range_f32(rule.radius_m.0, rule.radius_m.1);
        if !matches!(zone[i], ZoneId::Residential | ZoneId::Suburb) || g.highway[i] < 100.0 {
            continue;
        }
        let (x, z) = cell_center(i % g.zw, i / g.zw);
        let dist = |p: [f32; 2]| sqrt((p[0] - x) * (p[0] - x) + (p[1] - z) * (p[1] - z));
        if dist(g.downtown) < 250.0 || centers.iter().any(|&c| dist(c) < 350.0) {
            continue;
        }
        centers.push([x, z]);
        let reach = (radius * 1.2 / ZONE_CELL_M) as isize + 1;
        let (cx, cz) = ((i % g.zw) as isize, (i / g.zw) as isize);
        for qz in (cz - reach).max(0)..(cz + reach + 1).min(g.zh as isize) {
            for qx in (cx - reach).max(0)..(cx + reach + 1).min(g.zw as isize) {
                let j = qz as usize * g.zw + qx as usize;
                let (px, pz) = cell_center(qx as usize, qz as usize);
                let edge = radius * (0.85 + 0.3 * value_noise(edge_seed, px, pz, 48.0));
                if dist([px, pz]) <= edge
                    && matches!(
                        zone[j],
                        ZoneId::Residential | ZoneId::Suburb | ZoneId::Commercial
                    )
                {
                    zone[j] = ZoneId::Park;
                }
            }
        }
    }
}

/// Blends each sample toward a blurred copy by its zone's [`FLATTEN`] strength (itself
/// blurred across cells so there are no steps at cell edges). Water and beach stay as made.
fn flatten_by_zone(heights: &mut [f32], w: usize, h: usize, zone: &[ZoneId], zw: usize, zh: usize) {
    let mut blurred = terrain::box_blur(heights, w, h, 3);
    for _ in 0..2 {
        blurred = terrain::box_blur(&blurred, w, h, 3);
    }
    let mut strength: Vec<f32> = zone.iter().map(|&z| FLATTEN[z as usize]).collect();
    for _ in 0..2 {
        strength = terrain::box_blur(&strength, zw, zh, 1);
    }
    // Bilinear in cell units; cell centers sit at sample 1.5 + 4 c.
    let n = ZONE_SAMPLES as usize;
    let taps = |i: usize, cells: usize| {
        let f = clamp((i as f32 - 1.5) / n as f32, 0.0, (cells - 1) as f32);
        let i0 = (f as usize).min(cells.saturating_sub(2));
        (i0, (i0 + 1).min(cells - 1), f - i0 as f32)
    };
    let xt: Vec<(usize, usize, f32)> = (0..w).map(|ix| taps(ix, zw)).collect();
    for iz in 0..h {
        let (j0, j1, tz) = taps(iz, zh);
        let (r0, r1) = (
            &strength[j0 * zw..(j0 + 1) * zw],
            &strength[j1 * zw..(j1 + 1) * zw],
        );
        let zrow = &zone[(iz / n) * zw..(iz / n + 1) * zw];
        let row = &mut heights[iz * w..(iz + 1) * w];
        let brow = &blurred[iz * w..(iz + 1) * w];
        for (ix, ((hp, b), &(i0, i1, tx))) in row.iter_mut().zip(brow).zip(&xt).enumerate() {
            if matches!(zrow[ix / n], ZoneId::Water | ZoneId::Beach) {
                continue;
            }
            let s = lerp(lerp(r0[i0], r0[i1], tx), lerp(r1[i0], r1[i1], tx), tz);
            *hp += (b - *hp) * s;
        }
    }
}

/// The highest `Hills` cells, at least 400 m apart and 250 m from the pass.
fn overlooks(seed: Seed, t: &RegionTemplate, map: &MacroMap) -> Vec<[f32; 2]> {
    let mut rng = Rng::new(seed, &[stage::POIS]);
    let n = rng.range(
        u32::from(t.layout.overlooks.0),
        u32::from(t.layout.overlooks.1) + 1,
    );
    let zw = map.zone_width as usize;
    let mut cand: Vec<(f32, usize)> = map
        .zones
        .iter()
        .enumerate()
        .filter(|(_, &z)| z == ZoneId::Hills)
        .map(|(i, _)| {
            let (x, z) = cell_center(i % zw, i / zw);
            (map.height_at(x, z), i)
        })
        .collect();
    cand.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<[f32; 2]> = Vec::new();
    let far = |a: [f32; 2], b: [f32; 2], m: f32| {
        let (dx, dz) = (a[0] - b[0], a[1] - b[1]);
        dx * dx + dz * dz >= m * m
    };
    for (_, i) in cand {
        if out.len() as u32 >= n {
            break;
        }
        let (x, z) = cell_center(i % zw, i / zw);
        let p = [x, z];
        if map.pass.is_some_and(|q| !far(p, q, 250.0)) || out.iter().any(|&q| !far(p, q, 400.0)) {
            continue;
        }
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        for side in [
            CoastSide::West,
            CoastSide::East,
            CoastSide::North,
            CoastSide::South,
        ] {
            let f = Frame::new(side, 64, 32);
            let p = [100.0, 40.0];
            let (u, v) = f.to_canon(p);
            assert_eq!(f.to_world(u, v), p, "{side:?}");
            // Sample (iu, iv) sits at canonical (4 iu, 4 iv).
            let i = f.index(3, 5);
            let (x, z) = ((i % 64) as f32 * CELL_M, (i / 64) as f32 * CELL_M);
            assert_eq!(f.to_canon([x, z]), (12.0, 20.0), "{side:?}");
        }
    }

    #[test]
    fn catmull_rom_hits_control_points() {
        let pts = [(0.0, 0.0), (100.0, 10.0), (200.0, 0.0)];
        let line = catmull_rom(&pts, 16.0);
        assert_eq!(line[0], pts[0]);
        assert_eq!(*line.last().unwrap(), pts[2]);
        assert!(line.contains(&pts[1]));
        let v = v_per_u(&line, 60);
        assert_eq!(v[0], 0.0);
        assert!((v[25] - 10.0).abs() < 0.5);
        assert_eq!(v[59], 0.0);
    }
}
