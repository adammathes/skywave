//! The macro map: a whole place's heightfield and zone map, with the structure the chunk
//! stages read (shore, strip, grid lines, water bodies, points of interest). An authored
//! layout fills one ([`super::authored::skeleton`], `docs/LAYOUTS.md`); this module holds
//! the type and the terrain work every layout shares (smoothing by zone, the edge walls).
//!
//! Grids: heights are `width × height` samples, 4 m apart, sample `(ix, iz)` at world
//! `(4 ix, 4 iz)`; zones are `zone_width × zone_height` cells of 16 m (4×4 samples), cell
//! `(cx, cz)` covering world `[16 cx, 16 cx + 16) × [16 cz, 16 cz + 16)`. Both row-major,
//! `z` major. A zone cell is `Water` iff the mean of its 16 samples is below
//! `water_level` (or a canal covers it).

use alloc::vec;
use alloc::vec::Vec;

use place::{EdgeKind, ZoneId, EAST, NORTH, SOUTH, WEST};
use skywave_core::Seed;

use super::terrain::{self, Shore};
use super::zones::cell_center;
use super::{CELL_M, ZONE_CELL_M, ZONE_SAMPLES};
use crate::edges;
use crate::math::{clamp, floor, lerp, signed_area, V2};
use crate::template::CoastSide;

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

    /// Samples along `v`.
    pub fn n_v(&self) -> u32 {
        if self.across() {
            self.samples[1]
        } else {
            self.samples[0]
        }
    }

    pub fn extent(&self) -> [f32; 2] {
        [
            (self.samples[0] - 1) as f32 * CELL_M,
            (self.samples[1] - 1) as f32 * CELL_M,
        ]
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
    /// A dry concrete flood channel (an inland basin's): no water in it.
    Channel = 3,
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
    /// The corridor bed's height at every point of `highway` :
    /// at most 4.8% steep, carved into `heights`.
    pub highway_bed: Vec<f32>,
    /// How far carving the corridor moved each height sample (carved minus before;
    /// negative in cuts, zero away from the corridor). The arterial stage keeps roads off
    /// its cut and fill slopes.
    pub corridor: Vec<f32>,
    /// The highway's highest point; `None` on flat places.
    pub pass: Option<[f32; 2]>,
    pub pois: Vec<MacroPoi>,
    /// What bounds the place on each side, `[north, east, south, west]`.
    pub edges: [EdgeKind; 4],
    /// The roadless band inside each side ([`edges::band_m`]), same order.
    pub bands: [f32; 4],
    /// An inland basin's grid, channel, rail line and airfield, as a layout
    /// draws them; `None` where there are none.
    pub basin: Option<BasinPlan>,
    /// An authored town's one street grid, `[origin, block]` ([`crate::layout::Grid`]).
    pub grid: Option<[[f32; 2]; 2]>,
}

/// One arterial of a basin's grid: a straight line, world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct GridLine {
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// Two one-way carriageways either side of a planted median.
    pub boulevard: bool,
}

/// What a basin's macro stage lays out besides terrain and zones (from a layout); the
/// arterial stage builds its roads from it and the detailing its track. World coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct BasinPlan {
    /// The arterial grid: the lines parallel to the hills first, from the foot of the hills
    /// out, then the lines running out from the hills, from the far side of the desert in.
    pub lines: Vec<GridLine>,
    /// Lines parallel to the hills (the first `cross` of `lines`).
    pub cross: usize,
    /// The grid lines the freeway's ends meet at grade (indices into `lines`).
    pub freeway_ends: [usize; 2],
    /// Lanes each way of a boulevard and its median.
    pub boulevard_lanes: u8,
    pub median_m: f32,
    /// The flood channel's centerline from its head in the hills to the far edge, every
    /// 16 m, its bed's half-width and the half-width at its lips.
    pub channel: Vec<[f32; 2]>,
    pub channel_half_m: (f32, f32),
    /// The ramps down into it: from the top (at a bridge) to the foot on the bed, each 8 m
    /// wide along the wall.
    pub ramps: Vec<[[f32; 2]; 2]>,
    /// The rail line's centerline from edge to edge, and the spurs beside it (each from
    /// the turnout off the main line to its bumper).
    pub rail: Vec<[f32; 2]>,
    pub spurs: Vec<Vec<[f32; 2]>>,
    /// The airfield's corners (counter-clockwise), its runway's ends, and the taxiway from
    /// the runway's first end to the grid.
    pub airfield: Vec<[f32; 2]>,
    pub runway: [[f32; 2]; 2],
    pub runway_m: f32,
    pub taxiway: [[f32; 2]; 2],
    /// The heights with the channel filled: what the roads and the track over it bridge.
    pub bridge_heights: Vec<f32>,
    /// The local streets' grid is square to the axes whatever the roads beside it do (a
    /// generated basin's); else it follows the main roads (an authored layout's).
    pub square: bool,
    /// The way out from the foot of the hills (`lines[0]`) onto the floor.
    pub out: [f32; 2],
    /// Whether the set pieces are sited on this plan (`super::setpieces`).
    pub set_pieces: bool,
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

/// Along a `RailYards` edge the land this far inside the band is industry.
const YARDS_M: f32 = 160.0;

impl MacroMap {
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

    /// The playable rectangle `[x0, z0, x1, z1]`: the place less the edge bands. Roads,
    /// lots and points of interest stay inside it.
    pub fn playable(&self) -> [f32; 4] {
        let ext = self.frame.extent();
        let b = self.bands;
        [b[WEST], b[NORTH], ext[0] - b[EAST], ext[1] - b[SOUTH]]
    }

    /// Whether world `(x, z)` lies in the playable rectangle, `margin` meters in from it.
    pub fn in_playable(&self, x: f32, z: f32, margin: f32) -> bool {
        let r = self.playable();
        x >= r[0] + margin && z >= r[1] + margin && x <= r[2] - margin && z <= r[3] - margin
    }

    /// Signed distance inland from the shoreline at world `(x, z)`: negative in the big
    /// water, measured perpendicular to the local shore.
    pub fn coast_distance(&self, x: f32, z: f32) -> f32 {
        let (u, v) = self.frame.to_canon([x, z]);
        self.shore.distance(u, v)
    }
}

/// Raises the wall of every land edge ([`edges::rise`]) over the heights (`w × h` samples
/// [`CELL_M`] apart), from whichever is higher, the ground or the ground at the foot of the
/// band (so the crest clears the foot by the wall's height even where the land falls
/// toward the edge); where two walls meet the higher wins.
pub(super) fn raise_walls(
    seed: Seed,
    heights: &mut [f32],
    w: usize,
    h: usize,
    sides: &[EdgeKind; 4],
    bounds: (f32, f32),
) {
    let ext = [(w - 1) as f32 * CELL_M, (h - 1) as f32 * CELL_M];
    let reach = sides.map(|k| edges::wall(k).map_or(0.0, |x| x.ramp_m + edges::CREST_WANDER_M));
    let foot_at = sides.map(|k| ((edges::band_m(k) / CELL_M) as usize).min(w.min(h) - 1));
    let base = heights.to_vec();
    // Each wall's height and crest along its side, once per column (north, south) or row
    // (east, west): they depend only on the position along it.
    let profiles: [Vec<(f32, f32)>; 4] = core::array::from_fn(|k| {
        let n = if k == NORTH || k == SOUTH { w } else { h };
        (0..n)
            .map(|i| edges::profile(seed.0, k, sides[k], i as f32 * CELL_M).unwrap_or((0.0, 0.0)))
            .collect()
    });
    // The highest wall over each cell, side by side over just the strip each can reach.
    let mut top = vec![f32::NEG_INFINITY; w * h];
    for k in 0..4 {
        if reach[k] <= 0.0 {
            continue;
        }
        let n = (reach[k] / CELL_M) as usize + 2;
        let (zs, xs) = match k {
            NORTH => (0..n.min(h), 0..w),
            SOUTH => (h.saturating_sub(n)..h, 0..w),
            WEST => (0..h, 0..n.min(w)),
            _ => (0..h, w.saturating_sub(n)..w),
        };
        for iz in zs {
            for ix in xs.clone() {
                let p = [ix as f32 * CELL_M, iz as f32 * CELL_M];
                let d = edges::inside(p, ext)[k];
                if d >= reach[k] {
                    continue;
                }
                let at = if k == NORTH || k == SOUTH { ix } else { iz };
                let rise = edges::rise_at(sides[k], profiles[k][at], d);
                let (tx, tz) = match k {
                    NORTH => (ix, foot_at[k]),
                    SOUTH => (ix, h - 1 - foot_at[k]),
                    WEST => (foot_at[k], iz),
                    _ => (w - 1 - foot_at[k], iz),
                };
                let foot = base[iz * w + ix].max(base[tz * w + tx]);
                let t = &mut top[iz * w + ix];
                *t = t.max(foot + rise);
            }
        }
    }
    for (hp, &t) in heights.iter_mut().zip(&top) {
        if t > *hp {
            *hp = clamp(t, bounds.0, bounds.1);
        }
    }
}

/// The zone the band inside a land edge of `kind` takes.
fn band_zone(kind: EdgeKind) -> ZoneId {
    match kind {
        EdgeKind::Forest => ZoneId::Park,
        EdgeKind::RailYards => ZoneId::Industrial,
        EdgeKind::Desert => ZoneId::Farmland,
        _ => ZoneId::Hills,
    }
}

/// Paints the bands' zones over the cells whose centers lie in them (`Water` where the
/// cell is still under the water level), and industry for [`YARDS_M`] inside a
/// `RailYards` band.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_bands(
    heights: &[f32],
    w: usize,
    sides: &[EdgeKind; 4],
    bands: &[f32; 4],
    wl: f32,
    zone: &mut [ZoneId],
    zw: usize,
    zh: usize,
) {
    let h = heights.len() / w;
    let ext = [(w - 1) as f32 * CELL_M, (h - 1) as f32 * CELL_M];
    let n = ZONE_SAMPLES as usize;
    for cz in 0..zh {
        for cx in 0..zw {
            let (x, z) = cell_center(cx, cz);
            let d = edges::inside([x, z], ext);
            let i = cz * zw + cx;
            for k in 0..4 {
                if bands[k] <= 0.0 {
                    continue;
                }
                if d[k] < bands[k] {
                    let mut acc = 0.0;
                    for j in 0..n {
                        for q in 0..n {
                            acc += heights[(cz * n + j) * w + cx * n + q];
                        }
                    }
                    zone[i] = if acc / ((n * n) as f32) < wl {
                        ZoneId::Water
                    } else {
                        band_zone(sides[k])
                    };
                    break;
                }
                let yards = sides[k] == EdgeKind::RailYards && d[k] < bands[k] + YARDS_M;
                let fixed = matches!(
                    zone[i],
                    ZoneId::Water | ZoneId::Beach | ZoneId::Strip | ZoneId::Highway
                );
                if yards && !fixed {
                    zone[i] = ZoneId::Industrial;
                }
            }
        }
    }
}

pub(super) fn ccw(mut poly: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    let v: Vec<V2> = poly.iter().map(|&p| V2::from_array(p)).collect();
    if signed_area(&v) < 0.0 {
        poly.reverse();
    }
    poly
}

/// Blends each sample toward a blurred copy by its zone's [`FLATTEN`] strength (itself
/// blurred across cells so there are no steps at cell edges). Water and beach stay as made.
pub(super) fn flatten_by_zone(
    heights: &mut [f32],
    w: usize,
    h: usize,
    zone: &[ZoneId],
    zw: usize,
    zh: usize,
) {
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
