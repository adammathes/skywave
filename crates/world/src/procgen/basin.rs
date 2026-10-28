//! Stages 1–2 for an inland basin (`socal_basin`; `docs/WORLD_GENERATION.md`, "Inland
//! basins"): no coast, a flat floor tilting up to hills along one side, a desert fringe
//! along a side beside it, and the structure the arterial stage builds its roads from
//! ([`BasinPlan`]).
//!
//! [`generate`] works in the canonical frame of the hills' side (`u` in from it, `v` along
//! it), with `w` along it measured from the side opposite the desert, and runs, in order:
//! 1. **Layout** inside the edge bands: the foot of the hills; the arterial grid at the
//!    template's spacing, its first line along the foot of the hills ("cross" lines run
//!    parallel to the hills, "out" lines out from them), one boulevard each way; the
//!    freeway between two cross lines near the middle, from the first out line (where it
//!    ends at grade) out into the desert, where it turns along the desert edge and ends at
//!    grade on the last cross line; the rail line between the first two cross lines, edge
//!    to edge, with spurs beside it; the flood channel between the first two out lines,
//!    from the hills to the far edge, with a ramp at a bridge every `ramp_every_m`; the
//!    airfield between the last cross line and the far edge, past the channel; downtown.
//! 2. **Heights** at 4 m: the floor (tilted toward the hills, rolling a little), the hills
//!    rising from their foot to the band, ridged.
//! 3. **Zones** on 16 m cells: the template's scores (its `hills`, `rail`, `arterial` and
//!    `boulevard` curves as well as the coast's `slope`, `downtown` and `highway`), two
//!    majority passes, then by structure: the hills, the desert fringe (`Farmland`), the
//!    rights of way (`Highway`: the freeway corridor, the rail line, the channel), the
//!    airfield and the parks (`Park`).
//! 4. **Grading:** urban zones smoothed (as on the coast), the freeway corridor carved,
//!    the walls of the edges raised; then the channel cut (its bed `depth_m` under the
//!    ground beside it, through the walls too), the bands painted and points of interest
//!    (overlooks) found.
//!
//! Every draw comes from `Rng::new(seed, [stage, BASIN, ..])`, so a basin's draws never
//! touch a coast's stage streams.

use alloc::vec;
use alloc::vec::Vec;

use place::ZoneId;
use skywave_core::{Rng, Seed};

use super::macro_map::{
    cell_means, fillet, flatten_by_zone, near_polyline, paint_bands, raise_walls, resample_c,
    BasinPlan, Frame, GridLine, MacroMap, MacroPoi, MacroPoiKind, Strip, WaterBody, WaterKind,
};
use super::terrain::{self, smoothstep, Noise, Shore};
use super::zones::{self, cell_center, ZoneInputs};
use super::{stage, CELL_M, RES_PER_CHUNK, ZONE_CELL_M, ZONE_SAMPLES};
use crate::edges;
use crate::math::{abs, clamp, lerp, segment_distance, sqrt, V2};
use crate::template::{Basin, CoastSide, RegionTemplate};

/// Second word of every basin stream's key.
const BASIN: u64 = 0x0042_4153_494E;
/// The first cross line runs this far out from the foot of the hills.
const FOOT_ROAD_M: f32 = 24.0;
/// Out lines start this far in from their band.
const OUT_MARGIN_M: f32 = 40.0;
/// The freeway's desert leg runs this far inside the desert's band...
const LEG_M: f32 = 60.0;
/// ...and cross lines it would cross stop this far short of it.
const LEG_CLEAR_M: f32 = 150.0;
/// Radius of the freeway's turn into the desert leg.
const TURN_RADIUS_M: f32 = 220.0;
/// Track centers of a spur beside the main line.
const SPUR_GAP_M: f32 = 4.5;
/// Length of a spur's turnout off the main line.
const TURNOUT_M: f32 = 30.0;
/// A spur keeps this clear of the roads and the channel it runs between.
const SPUR_CLEAR_M: f32 = 24.0;
/// Width of a channel ramp.
pub const RAMP_W_M: f32 = 8.0;
/// A ramp starts this far from the middle of the bridge it is reached from.
const RAMP_START_M: f32 = 16.0;
/// The channel's head lies this far into the hills from their foot.
const CHANNEL_HEAD_M: f32 = 80.0;
/// Over this length at its head the channel deepens from nothing.
const CHANNEL_HEADWALL_M: f32 = 40.0;
/// The airfield keeps this clear of the last cross line, the far band and the channel.
const AIRFIELD_CLEAR_M: f32 = 40.0;

/// Index of a side in `place::Terrain::edges` order.
fn side_index(s: CoastSide) -> usize {
    match s {
        CoastSide::North => place::NORTH,
        CoastSide::East => place::EAST,
        CoastSide::South => place::SOUTH,
        CoastSide::West => place::WEST,
    }
}

/// The canonical frame of a basin and its `w` coordinate (along the hills, from the side
/// opposite the desert).
struct Canon {
    frame: Frame,
    /// The desert is at `v = len_v`.
    desert_hi: bool,
    len_v: f32,
}

impl Canon {
    fn v(&self, w: f32) -> f32 {
        if self.desert_hi {
            w
        } else {
            self.len_v - w
        }
    }

    fn world(&self, u: f32, w: f32) -> [f32; 2] {
        self.frame.to_world(u, self.v(w))
    }

    fn canon(&self, p: [f32; 2]) -> (f32, f32) {
        let (u, v) = self.frame.to_canon(p);
        (u, self.v(v))
    }
}

/// Generates a basin's macro map; see the module docs. `t` must have passed
/// [`RegionTemplate::validate`] with `b` its basin.
pub fn generate(seed: Seed, t: &RegionTemplate, b: &Basin, size_chunks: [u16; 2]) -> MacroMap {
    let size_chunks = [size_chunks[0].max(1), size_chunks[1].max(1)];
    let nx = u32::from(size_chunks[0]) * RES_PER_CHUNK;
    let nz = u32::from(size_chunks[1]) * RES_PER_CHUNK;
    let frame = Frame::new(b.hills.side, nx, nz);
    let (len_u, len_v) = (frame.len_u(), frame.len_v());
    let edges = t.edges.sides();
    let bands = edges::bands(&edges);
    let [side_far, side_v0, side_v1] = frame.sides();
    let desert_hi = side_index(b.desert.side) == side_v1;
    let c = Canon {
        frame,
        desert_hi,
        len_v,
    };
    let band_h = bands[side_index(b.hills.side)];
    let band_far = bands[side_far];
    let (band_w0, band_d) = if desert_hi {
        (bands[side_v0], bands[side_v1])
    } else {
        (bands[side_v1], bands[side_v0])
    };
    let wl = b.water_level_m;

    // 1. Layout.
    let mut rng = Rng::new(seed, &[stage::LAYOUT, BASIN]);
    let floor = rng.range_f32(b.floor_m.0, b.floor_m.1);
    let u_foot = band_h + rng.range_f32(b.hills.depth_m.0, b.hills.depth_m.1);
    let hills_h = rng.range_f32(b.hills.height_m.0, b.hills.height_m.1);
    let desert_depth = rng.range_f32(b.desert.depth_m.0, b.desert.depth_m.1);
    let spacing = b.grid.spacing_m;
    let w_lo = band_w0 + OUT_MARGIN_M;
    let w_hi = len_v - band_d;
    let w_desert = w_hi - desert_depth;
    let w_leg = w_hi - LEG_M;
    // Cross lines, the first along the foot of the hills, leaving room for the airfield
    // between the last and the far band; at least three.
    let u_first = u_foot + FOOT_ROAD_M;
    let u_max = len_u - band_far - b.airfield.width_m - 2.0 * AIRFIELD_CLEAR_M;
    let mut cross_u: Vec<f32> = Vec::new();
    let mut u = u_first;
    while u <= u_max || cross_u.len() < 3 {
        cross_u.push(u);
        u += spacing;
    }
    let nc = cross_u.len();
    // Out lines, from the far side of the desert in, short of the desert; at least three.
    let w_first = w_lo + rng.range_f32(0.0, 0.3 * spacing);
    let mut out_w: Vec<f32> = Vec::new();
    let mut w = w_first;
    while w <= w_desert || out_w.len() < 3 {
        out_w.push(w);
        w += spacing;
    }
    let no = out_w.len();
    let u_last = cross_u[nc - 1];
    // The freeway between two cross lines near the middle, the rail between the first two.
    let kf = ((nc - 1) / 2).max(1);
    let u_fw = 0.5 * (cross_u[kf] + cross_u[kf + 1]);
    let u_rail = 0.5 * (cross_u[0] + cross_u[1]);
    // One boulevard each way, on interior lines (not the freeway's west end).
    let cross_blvd = 1 + rng.range(0, (nc - 2) as u32) as usize;
    let out_blvd = 1 + rng.range(0, (no - 2) as u32) as usize;
    // The channel between the first two out lines.
    let w_channel = 0.5 * (out_w[0] + out_w[1]) + rng.range_f32(-0.1, 0.1) * spacing;
    let u_dt = u_foot + rng.range_f32(b.downtown_m.0, b.downtown_m.1);
    let w_dt = lerp(
        w_lo,
        w_desert,
        rng.range_f32(b.downtown_along.0, b.downtown_along.1),
    );

    let mut lines: Vec<GridLine> = Vec::new();
    for (k, &u) in cross_u.iter().enumerate() {
        // North of the freeway the line runs to the desert's band; between the freeway and
        // the last line it stops short of the desert leg; the last meets the leg's end.
        let w_end = if u < u_fw {
            w_hi - 12.0
        } else if k + 1 < nc {
            w_leg - LEG_CLEAR_M
        } else {
            w_leg
        };
        lines.push(GridLine {
            a: c.world(u, out_w[0]),
            b: c.world(u, w_end),
            boulevard: k == cross_blvd,
        });
    }
    for (k, &w) in out_w.iter().enumerate() {
        lines.push(GridLine {
            a: c.world(cross_u[0], w),
            b: c.world(u_last, w),
            boulevard: k == out_blvd,
        });
    }

    // The freeway: from the first out line out into the desert, round along its edge to
    // the last cross line.
    let leg = u_last - u_fw;
    let corners = [(u_fw, out_w[0]), (u_fw, w_leg), (u_last, w_leg)];
    let radius = TURN_RADIUS_M.min(0.45 * leg).min(0.45 * (w_leg - out_w[0]));
    let highway: Vec<[f32; 2]> = resample_c(&fillet(&corners, radius), ZONE_CELL_M)
        .iter()
        .map(|&(u, w)| c.world(u, w))
        .collect();

    // The rail line edge to edge (into the walls of the bands), spurs beside it between
    // the out lines it crosses.
    let rail: Vec<[f32; 2]> = resample_c(
        &[(u_rail, band_w0 * 0.5), (u_rail, len_v - 0.5 * band_d)],
        ZONE_CELL_M,
    )
    .iter()
    .map(|&(u, w)| c.world(u, w))
    .collect();
    let mut gaps: Vec<(f32, f32)> = Vec::new();
    let mut stops: Vec<f32> = out_w.clone();
    stops.push(w_channel);
    stops.push(w_hi);
    stops.sort_by(f32::total_cmp);
    let mut prev = w_lo;
    for &s in &stops {
        if s - prev > 2.0 * SPUR_CLEAR_M + TURNOUT_M + b.rail.spur_m.0 {
            gaps.push((prev + SPUR_CLEAR_M, s - SPUR_CLEAR_M));
        }
        prev = s;
    }
    let n_spurs = rng.range(u32::from(b.rail.spurs.0), u32::from(b.rail.spurs.1) + 1);
    let mut spurs: Vec<Vec<[f32; 2]>> = Vec::new();
    for k in 0..n_spurs as usize {
        if gaps.is_empty() {
            break;
        }
        let g = gaps.remove(rng.range(0, gaps.len() as u32) as usize);
        let len = rng
            .range_f32(b.rail.spur_m.0, b.rail.spur_m.1)
            .min(g.1 - g.0 - TURNOUT_M);
        let side = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
        let off = side * SPUR_GAP_M;
        let (w0, w1) = if rng.next_f32() < 0.5 {
            (g.0, g.0 + TURNOUT_M + len)
        } else {
            (g.1, g.1 - TURNOUT_M - len)
        };
        let dir = if w1 > w0 { 1.0 } else { -1.0 };
        let pts = [
            (u_rail, w0),
            (u_rail + off, w0 + dir * TURNOUT_M),
            (u_rail + off, w1),
        ];
        spurs.push(
            resample_c(&pts, 8.0)
                .iter()
                .map(|&(u, w)| c.world(u, w))
                .collect(),
        );
    }

    // The channel, from its head in the hills to the far edge (into the wall of the
    // band), and its ramps: at the first bridge past every `ramp_every_m` of it.
    let ch = &b.channel;
    let (bed_half, lip_half) = (0.5 * ch.bed_m, 0.5 * ch.bed_m + ch.wall_m);
    let u_head = u_foot - CHANNEL_HEAD_M;
    let u_tail = len_u - 0.5 * band_far;
    let channel_c = resample_c(&[(u_head, w_channel), (u_tail, w_channel)], ZONE_CELL_M);
    let channel: Vec<[f32; 2]> = channel_c.iter().map(|&(u, w)| c.world(u, w)).collect();
    // A ramp: (top u, foot u, side), in canonical coordinates.
    let ramp_len = ch.depth_m / ch.ramp_grade;
    let mut ramps_c: Vec<(f32, f32, f32)> = Vec::new();
    let mut next = 0.0;
    for &u in &cross_u {
        if u - u_head < next {
            continue;
        }
        next = (((u - u_head) / ch.ramp_every_m) as u32 + 1) as f32 * ch.ramp_every_m;
        // Down away from the bridge, toward the far side unless that runs past the edge;
        // on alternate walls.
        let dir = if u + RAMP_START_M + ramp_len < u_tail - band_far {
            1.0
        } else {
            -1.0
        };
        let side = if ramps_c.len().is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        let top = u + dir * RAMP_START_M;
        ramps_c.push((top, top + dir * ramp_len, side));
    }
    let ramp_w = |side: f32| w_channel + side * (bed_half + 0.5 * RAMP_W_M);
    let ramps: Vec<[[f32; 2]; 2]> = ramps_c
        .iter()
        .map(|&(u0, u1, side)| [c.world(u0, ramp_w(side)), c.world(u1, ramp_w(side))])
        .collect();

    // The airfield between the last cross line and the far band, east of the channel.
    let af = &b.airfield;
    let u_af0 = u_last + AIRFIELD_CLEAR_M;
    let u_af1 = (u_af0 + af.width_m).min(len_u - band_far - 16.0);
    let u_af = 0.5 * (u_af0 + u_af1);
    let w_af0 = w_channel + 0.5 * ch.row_m + AIRFIELD_CLEAR_M;
    let length = rng.range_f32(af.length_m.0, af.length_m.1);
    let w_af1 = (w_af0 + length).min(w_leg - AIRFIELD_CLEAR_M);
    let airfield: Vec<[f32; 2]> = [
        (u_af0, w_af0),
        (u_af0, w_af1),
        (u_af1, w_af1),
        (u_af1, w_af0),
    ]
    .iter()
    .map(|&(u, w)| c.world(u, w))
    .collect();
    let airfield = super::macro_map::ccw(airfield);
    let (r0, r1) = (w_af0 + 30.0, w_af1 - 30.0);
    let runway = [c.world(u_af, r0), c.world(u_af, r1)];
    // The taxiway from the runway's first end straight to the last cross line.
    let taxiway = [c.world(u_last, r0), c.world(u_af, r0)];

    // 2. Heights.
    let bounds = t.height_bounds();
    let relief = Noise::fbm(seed, &[stage::TERRAIN, BASIN, 1], 300.0, 2);
    let swell = Noise::fbm(seed, &[stage::TERRAIN, BASIN, 2], 520.0, 2);
    let gullies = Noise::fbm(seed, &[stage::TERRAIN, BASIN, 3], 220.0, 2);
    let foot_wander = Noise::fbm(seed, &[stage::TERRAIN, BASIN, 4], 360.0, 2);
    let (w, h) = (nx as usize, nz as usize);
    let mut heights = vec![0.0f32; w * h];
    for iz in 0..h {
        for ix in 0..w {
            let p = [ix as f32 * CELL_M, iz as f32 * CELL_M];
            let (u, _) = c.canon(p);
            let mut y = floor + b.tilt * (len_u - u) + b.relief_m * relief.at(p[0], p[1]);
            let foot = u_foot + 20.0 * foot_wander.at(p[0], p[1]);
            let tt = clamp((foot - u) / (foot - band_h * 0.5).max(1.0), 0.0, 1.0);
            if tt > 0.0 {
                let s = tt * tt * (3.0 - 2.0 * tt);
                y += hills_h * s * (0.85 + 0.3 * swell.at(p[0], p[1]))
                    + b.hills.relief_m * tt * gullies.at(p[0], p[1]);
            }
            heights[iz * w + ix] = clamp(y, bounds.0, bounds.1);
        }
    }

    // 3. Zones.
    let (zw, zh) = ((nx / ZONE_SAMPLES) as usize, (nz / ZONE_SAMPLES) as usize);
    let mean_h = cell_means(&heights, w, zw, zh);
    let downtown = c.world(u_dt, w_dt);
    let blvd: Vec<&GridLine> = lines.iter().filter(|l| l.boulevard).collect();
    let mut inputs = ZoneInputs {
        w: zw,
        h: zh,
        coast: vec![terrain::FAR_SHORE_M; zw * zh],
        slope: vec![0.0; zw * zh],
        downtown: vec![0.0; zw * zh],
        highway: vec![0.0; zw * zh],
        water: vec![false; zw * zh],
        basin: vec![[0.0; 4]; zw * zh],
    };
    for cz in 0..zh {
        for cx in 0..zw {
            let i = cz * zw + cx;
            let (x, z) = cell_center(cx, cz);
            let q = V2::new(x, z);
            let at = |x: usize, z: usize| mean_h[z * zw + x];
            let (x0, x1) = (cx.saturating_sub(1), (cx + 1).min(zw - 1));
            let (z0, z1) = (cz.saturating_sub(1), (cz + 1).min(zh - 1));
            let gx = (at(x1, cz) - at(x0, cz)) / ((x1 - x0).max(1) as f32 * ZONE_CELL_M);
            let gz = (at(cx, z1) - at(cx, z0)) / ((z1 - z0).max(1) as f32 * ZONE_CELL_M);
            inputs.slope[i] = sqrt(gx * gx + gz * gz);
            inputs.downtown[i] = (q - V2::from_array(downtown)).length();
            inputs.highway[i] = polyline_distance(&highway, q);
            let (u, _) = c.canon([x, z]);
            let line_d =
                |l: &GridLine| segment_distance(q, V2::from_array(l.a), V2::from_array(l.b)).0;
            let arterial = lines.iter().map(line_d).fold(f32::INFINITY, f32::min);
            let boulevard = blvd.iter().map(|l| line_d(l)).fold(f32::INFINITY, f32::min);
            inputs.basin[i] = [u - u_foot, polyline_distance(&rail, q), arterial, boulevard];
        }
    }
    let mut zone = zones::score(seed, t, &inputs);
    for _ in 0..2 {
        zone = zones::majority(&zone, zw, zh, |q| q != ZoneId::Water);
    }
    let corridor = 0.5 * b.freeway.corridor_m;
    let in_airfield = |u: f32, w: f32| (u_af0..u_af1).contains(&u) && (w_af0..w_af1).contains(&w);
    for cz in 0..zh {
        for cx in 0..zw {
            let i = cz * zw + cx;
            let (x, z) = cell_center(cx, cz);
            let (u, w) = c.canon([x, z]);
            let foot = u_foot + 20.0 * foot_wander.at(x, z);
            if u < foot {
                zone[i] = ZoneId::Hills;
            } else if w > w_desert + 0.5 * ZONE_CELL_M {
                zone[i] = ZoneId::Farmland;
            }
            let q = V2::new(x, z);
            let on_rail = polyline_distance(&rail, q) <= 0.5 * b.rail.row_m;
            let on_channel = u >= u_head - 0.5 * ZONE_CELL_M
                && abs(w - w_channel) <= 0.5 * ch.row_m
                && u <= u_tail;
            if on_rail || on_channel || near_polyline(&highway, [x, z], corridor) {
                zone[i] = ZoneId::Highway;
            } else if in_airfield(u, w) {
                zone[i] = ZoneId::Park;
            }
        }
    }
    place_parks(seed, b, &mut zone, zw, zh, downtown);

    // 4. Grading: smoothing, the freeway's corridor, the walls, then the channel.
    flatten_by_zone(&mut heights, w, h, &zone, zw, zh);
    let bed = terrain::corridor_bed(&heights, w, h, &highway, wl);
    let corridor_moved = terrain::carve_corridor(&mut heights, w, h, &highway, &bed, wl);
    let before_walls = heights.clone();
    raise_walls(seed, &mut heights, w, h, &edges, bounds);
    let bridge_heights = heights.clone();
    let geo = ChannelGeo {
        c: &c,
        w_channel,
        u_head,
        u_tail,
        bed_half,
        lip_half,
        depth: ch.depth_m,
        ramps: &ramps_c,
    };
    for iz in 0..h {
        for ix in 0..w {
            let i = iz * w + ix;
            let cut = geo.cut([ix as f32 * CELL_M, iz as f32 * CELL_M]);
            if cut > 0.0 {
                heights[i] = heights[i].min(before_walls[i] - cut);
            }
        }
    }
    paint_bands(&heights, w, &edges, &bands, wl, &mut zone, zw, zh);
    // The yards' industry stops at the airfield.
    for cz in 0..zh {
        for cx in 0..zw {
            let (x, z) = cell_center(cx, cz);
            let (u, w) = c.canon([x, z]);
            let i = cz * zw + cx;
            if in_airfield(u, w) && zone[i] == ZoneId::Industrial {
                zone[i] = ZoneId::Park;
            }
        }
    }

    // The channel and its ramps as dry "rivers": the concrete the ground mesh paints.
    let mut water = Vec::new();
    let lip = |side: f32| w_channel + side * lip_half;
    water.push(WaterBody {
        kind: WaterKind::Channel,
        polygon: super::macro_map::ccw(vec![
            c.world(u_head, lip(-1.0)),
            c.world(u_tail, lip(-1.0)),
            c.world(u_tail, lip(1.0)),
            c.world(u_head, lip(1.0)),
        ]),
    });
    for &(u0, u1, side) in &ramps_c {
        let (a, b2) = (
            w_channel + side * bed_half,
            w_channel + side * (lip_half + RAMP_W_M),
        );
        water.push(WaterBody {
            kind: WaterKind::Channel,
            polygon: super::macro_map::ccw(vec![
                c.world(u0, a),
                c.world(u1, a),
                c.world(u1, b2),
                c.world(u0, b2),
            ]),
        });
    }

    let blvd_line = lines[cross_blvd].clone();
    let strip_len = (V2::from_array(blvd_line.b) - V2::from_array(blvd_line.a)).length();
    let n_strip = (strip_len / ZONE_CELL_M) as usize;
    let centerline = (0..=n_strip)
        .map(|k| {
            let t = k as f32 / n_strip.max(1) as f32;
            let q = V2::from_array(blvd_line.a).lerp(V2::from_array(blvd_line.b), t);
            [q.x, q.y]
        })
        .collect();
    let mut map = MacroMap {
        seed,
        size_chunks,
        frame,
        width: nx,
        height: nz,
        heights,
        water_level: wl,
        water,
        zone_width: zw as u32,
        zone_height: zh as u32,
        zones: zone,
        shore: Shore::far(frame.n_v()),
        downtown,
        strip: Strip {
            centerline,
            inland_m: 0.0,
            width_m: 0.0,
            length_m: strip_len,
        },
        highway,
        highway_bed: bed,
        corridor: corridor_moved,
        pass: None,
        pois: Vec::new(),
        edges,
        bands,
        basin: Some(BasinPlan {
            lines,
            cross: nc,
            freeway_ends: [nc, nc - 1],
            boulevard_lanes: b.grid.boulevard_lanes,
            median_m: b.grid.median_m,
            channel,
            channel_half_m: (bed_half, lip_half),
            ramps,
            rail,
            spurs,
            airfield,
            runway,
            runway_m: af.runway_m,
            taxiway,
            bridge_heights,
        }),
    };
    // The overlooks face out over the basin, on benches the overlook roads can reach.
    let out = c.frame.dir_to_world(1.0, 0.0);
    let floor_at_foot = floor + b.tilt * (len_u - u_foot);
    for p in basin_overlooks(seed, b.overlooks, &map, floor_at_foot, hills_h) {
        map.pois.push(MacroPoi {
            kind: MacroPoiKind::Overlook,
            pos: p,
            facing: out,
        });
    }
    map
}

/// The overlooks: the highest benches of the hills (a cell no steeper than
/// [`OVERLOOK_SLOPE`] over its neighbours, up between [`OVERLOOK_RISE`] of the hills'
/// height above the floor at their foot; on hills without one, gentler ground), at least
/// 400 m apart.
fn basin_overlooks(
    seed: Seed,
    count: (u8, u8),
    map: &MacroMap,
    floor: f32,
    hills_h: f32,
) -> Vec<[f32; 2]> {
    let mut rng = Rng::new(seed, &[stage::POIS, BASIN]);
    let n = rng.range(u32::from(count.0), u32::from(count.1) + 1);
    let (zw, zh) = (map.zone_width as usize, map.zone_height as usize);
    let h = |cx: usize, cz: usize| {
        let (x, z) = cell_center(cx, cz);
        map.height_at(x, z)
    };
    let mut out: Vec<[f32; 2]> = Vec::new();
    // The benches first; where the hills have too few, gentler ground in a wider band.
    for (slope_max, (lo, hi)) in [(OVERLOOK_SLOPE, OVERLOOK_RISE), (0.3, (0.2, 0.9))] {
        let mut cand: Vec<(f32, usize)> = Vec::new();
        for cz in 1..zh - 1 {
            for cx in 1..zw - 1 {
                let i = cz * zw + cx;
                let (x, z) = cell_center(cx, cz);
                if map.zones[i] != ZoneId::Hills || !map.in_playable(x, z, 60.0) {
                    continue;
                }
                let y = h(cx, cz);
                let rise = (y - floor) / hills_h.max(1.0);
                if !(lo..=hi).contains(&rise) {
                    continue;
                }
                let gx = (h(cx + 1, cz) - h(cx - 1, cz)) / (2.0 * ZONE_CELL_M);
                let gz = (h(cx, cz + 1) - h(cx, cz - 1)) / (2.0 * ZONE_CELL_M);
                if sqrt(gx * gx + gz * gz) <= slope_max {
                    cand.push((y, i));
                }
            }
        }
        cand.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, i) in cand {
            if out.len() as u32 >= n {
                break;
            }
            let (x, z) = cell_center(i % zw, i / zw);
            if out
                .iter()
                .all(|q| (V2::from_array(*q) - V2::new(x, z)).length() >= 400.0)
            {
                out.push([x, z]);
            }
        }
        if out.len() as u32 >= n.max(1) {
            break;
        }
    }
    out
}

/// An overlook's bench: no steeper than this, ...
const OVERLOOK_SLOPE: f32 = 0.14;
/// ...between these fractions of the way up the hills.
const OVERLOOK_RISE: (f32, f32) = (0.35, 0.7);

/// Distance from `p` to the polyline `line`.
fn polyline_distance(line: &[[f32; 2]], p: V2) -> f32 {
    line.windows(2)
        .map(|s| segment_distance(p, V2::from_array(s[0]), V2::from_array(s[1])).0)
        .fold(f32::INFINITY, f32::min)
}

/// The channel's cross-section, for cutting it into the heights.
struct ChannelGeo<'a> {
    c: &'a Canon,
    w_channel: f32,
    u_head: f32,
    u_tail: f32,
    bed_half: f32,
    lip_half: f32,
    depth: f32,
    /// (top u, foot u, side) of each ramp.
    ramps: &'a [(f32, f32, f32)],
}

impl ChannelGeo<'_> {
    /// How deep the channel cuts below the ground beside it at world `p`: the full depth
    /// on the bed, falling to nothing up the walls to the lips; deepening from its head;
    /// along a ramp, the ramp's depth there across its width and up the wall behind it.
    fn cut(&self, p: [f32; 2]) -> f32 {
        let (u, w) = self.c.canon(p);
        if u < self.u_head || u > self.u_tail + self.lip_half {
            return 0.0;
        }
        let d = w - self.w_channel;
        let wall = self.lip_half - self.bed_half;
        let head = smoothstep(self.u_head, self.u_head + CHANNEL_HEADWALL_M, u);
        let across = clamp((self.lip_half - abs(d)) / wall, 0.0, 1.0);
        let mut cut = self.depth * head * across;
        for &(u0, u1, side) in self.ramps {
            let (lo, hi) = (u0.min(u1), u0.max(u1));
            if u < lo - wall || u > hi + wall {
                continue;
            }
            let sd = side * d;
            if sd < self.bed_half {
                continue;
            }
            // Down the ramp from its top, held at the bed past its foot for a car length.
            let along = clamp((u - u0) / (u1 - u0), 0.0, 1.0);
            let depth = self.depth * along;
            let behind = sd - self.bed_half - RAMP_W_M;
            let mut k = if behind <= 0.0 {
                1.0
            } else {
                clamp(1.0 - behind / wall, 0.0, 1.0)
            };
            // The ramp's ends meet the channel's own wall.
            let past = (lo - u).max(u - hi).max(0.0);
            k *= clamp(1.0 - past / wall, 0.0, 1.0);
            cut = cut.max(depth * k);
        }
        cut
    }
}

/// Round parks in residential land, away from downtown and each other.
fn place_parks(
    seed: Seed,
    b: &Basin,
    zone: &mut [ZoneId],
    zw: usize,
    zh: usize,
    downtown: [f32; 2],
) {
    let mut rng = Rng::new(seed, &[stage::ZONES, BASIN]);
    let n = rng.range(u32::from(b.parks.count.0), u32::from(b.parks.count.1) + 1);
    let mut centers: Vec<V2> = vec![V2::from_array(downtown)];
    let mut tries = 0;
    while centers.len() < n as usize + 1 && tries < 400 {
        tries += 1;
        let i = rng.range(0, (zw * zh) as u32) as usize;
        let radius = rng.range_f32(b.parks.radius_m.0, b.parks.radius_m.1);
        if !matches!(zone[i], ZoneId::Residential | ZoneId::Suburb) {
            continue;
        }
        let (x, z) = cell_center(i % zw, i / zw);
        let p = V2::new(x, z);
        if centers.iter().any(|&q| (q - p).length() < 400.0) {
            continue;
        }
        centers.push(p);
        let reach = (radius / ZONE_CELL_M) as isize + 1;
        let (cx, cz) = ((i % zw) as isize, (i / zw) as isize);
        for qz in (cz - reach).max(0)..(cz + reach + 1).min(zh as isize) {
            for qx in (cx - reach).max(0)..(cx + reach + 1).min(zw as isize) {
                let j = qz as usize * zw + qx as usize;
                let (px, pz) = cell_center(qx as usize, qz as usize);
                if (V2::new(px, pz) - p).length() <= radius
                    && matches!(zone[j], ZoneId::Residential | ZoneId::Suburb)
                {
                    zone[j] = ZoneId::Park;
                }
            }
        }
    }
}
