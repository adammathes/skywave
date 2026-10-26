//! Ground meshes: the terrain apron around the town, raised lot pads inside every block
//! (lawn with seeded color noise, or paving), park paths and parking lots.

use alloc::vec::Vec;

use place::{BuildingUse, LotKind, Place, StyleTag};

use crate::math::{floor, point_in_polygon, signed_area, value_noise, V2, V3};
use crate::mesh::{quad, quad_vc, tri, tri_vc, Paint, Pattern};
use crate::palette::{mix, shade, Palette, Rgb};
use crate::props::{Prop, PropKind};
use crate::roads::Network;
use crate::{MeshKind, MeshSet, LOT_Y, PAINT_LIFT, TERRAIN_Y};
use skywave_core::{Rng, Seed};

/// Lot pad grid cell, meters: fine enough for visible lawn mottling.
const LOT_CELL: f32 = 7.0;
/// Terrain apron grid cell and margin around the road network, meters.
const APRON_CELL: f32 = 30.0;
const APRON_MARGIN: f32 = 300.0;
const KEY_LAWN: u64 = 0x4C41_574E;
const KEY_LAWN_FINE: u64 = 0x4649_4E45;

/// A city block: the polygon enclosed by its property lines (counter-clockwise).
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub poly: Vec<V2>,
}

/// Faces of the road graph, offset inward to the property lines. The unbounded outer face
/// is skipped.
pub fn blocks(net: &Network) -> Vec<Block> {
    let mut visited: Vec<Vec<bool>> = net
        .junctions
        .iter()
        .map(|j| alloc::vec![false; j.ends.len()])
        .collect();
    let mut out = Vec::new();
    for start_j in 0..net.junctions.len() {
        for start_s in 0..net.junctions[start_j].ends.len() {
            if visited[start_j][start_s] {
                continue;
            }
            let (mut j, mut s) = (start_j, start_s);
            let mut pts = Vec::new();
            let mut offs = Vec::new();
            let mut steps = 0;
            loop {
                visited[j][s] = true;
                let end = &net.junctions[j].ends[s];
                pts.push(net.junctions[j].pos);
                offs.push(end.hw + end.sw);
                let [ea, eb] = net.edge_ends[end.edge];
                let (oj, os) = if ea == (j, s) { eb } else { ea };
                let deg = net.junctions[oj].ends.len();
                // Turn as far right as possible: the face stays on the left.
                let next = (oj, (os + deg - 1) % deg);
                steps += 1;
                if next == (start_j, start_s) || visited[next.0][next.1] || steps > 256 {
                    break;
                }
                (j, s) = next;
            }
            if pts.len() >= 3 && signed_area(&pts) > 0.0 {
                let poly = crate::math::offset_polygon(&pts, |i| -offs[i]);
                if signed_area(&poly) > 1.0 {
                    out.push(Block { poly });
                }
            }
        }
    }
    out
}

/// How a block's open ground is surfaced.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    Lawn,
    ParkLawn,
    Paved,
}

/// Builds the ground meshes; returns the cars parked in parking lots. `fronts[i]` is the
/// frontage facade of building `i` (strip malls get their parking lot in front of it).
pub fn build(
    place: &Place,
    blocks: &[Block],
    fronts: &[usize],
    pal: &Palette,
    seed: u64,
    out: &mut MeshSet,
) -> Vec<Prop> {
    let mut cars = Vec::new();
    apron(place, pal, seed, out);
    for b in blocks {
        let surface = block_surface(place, b);
        lot_pad(b, surface, pal, seed, out);
        if surface == Surface::ParkLawn {
            park_paths(b, pal, out);
        }
    }
    for (i, bld) in place.buildings.iter().enumerate() {
        if bld.style == StyleTag::StripMall {
            if let Some(b) = blocks
                .iter()
                .find(|b| point_in_polygon(centroid(&to_v2(&bld.footprint)), &b.poly))
            {
                parking_lot(
                    &to_v2(&bld.footprint),
                    fronts[i],
                    b,
                    pal,
                    seed ^ i as u64,
                    out,
                    &mut cars,
                );
            }
        }
    }
    cars
}

pub fn to_v2(poly: &[[f32; 2]]) -> Vec<V2> {
    poly.iter().map(|&p| V2::from_array(p)).collect()
}

pub fn centroid(poly: &[V2]) -> V2 {
    let mut c = V2::new(0.0, 0.0);
    for &p in poly {
        c = c + p;
    }
    c * (1.0 / poly.len() as f32)
}

/// Whether a park parcel occupies the block.
pub fn is_park(place: &Place, b: &Block) -> bool {
    place.parcels.iter().any(|p| {
        p.lot_kind == LotKind::Park && point_in_polygon(centroid(&to_v2(&p.polygon)), &b.poly)
    })
}

pub(crate) fn block_surface(place: &Place, b: &Block) -> Surface {
    if is_park(place, b) {
        return Surface::ParkLawn;
    }
    let mut homes = 0;
    let mut others = 0;
    for bld in &place.buildings {
        if point_in_polygon(centroid(&to_v2(&bld.footprint)), &b.poly) {
            if bld.usage == BuildingUse::Residential {
                homes += 1;
            } else {
                others += 1;
            }
        }
    }
    if others > 0 && homes == 0 {
        Surface::Paved
    } else {
        Surface::Lawn
    }
}

/// Lawn color with two octaves of noise: patches of dry grass and fine mottling.
fn lawn_color(pal: &Palette, seed: u64, p: V2, lush: bool) -> Rgb {
    let coarse = value_noise(seed ^ KEY_LAWN, p.x, p.y, 23.0);
    let fine = value_noise(seed ^ KEY_LAWN_FINE, p.x, p.y, 3.7);
    let dry = if lush {
        coarse * 60.0
    } else {
        coarse * coarse * 300.0
    };
    let c = mix(pal.lawn, pal.lawn_dry, dry as u32);
    shade(c, 220 + (fine * 64.0) as u32)
}

fn paved_color(pal: &Palette, seed: u64, p: V2) -> Rgb {
    let coarse = value_noise(seed ^ KEY_LAWN, p.x, p.y, 17.0);
    let fine = value_noise(seed ^ KEY_LAWN_FINE, p.x, p.y, 4.1);
    let c = mix(pal.asphalt_worn, pal.concrete_dark, (coarse * 200.0) as u32);
    shade(c, 230 + (fine * 40.0) as u32)
}

fn apron(place: &Place, pal: &Palette, seed: u64, out: &mut MeshSet) {
    let nodes = &place.roads.nodes;
    if nodes.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for n in nodes {
        lo = V2::new(lo.x.min(n.pos[0]), lo.y.min(n.pos[1]));
        hi = V2::new(hi.x.max(n.pos[0]), hi.y.max(n.pos[1]));
    }
    let x0 = floor((lo.x - APRON_MARGIN) / APRON_CELL) as i32;
    let x1 = floor((hi.x + APRON_MARGIN) / APRON_CELL) as i32 + 1;
    let z0 = floor((lo.y - APRON_MARGIN) / APRON_CELL) as i32;
    let z1 = floor((hi.y + APRON_MARGIN) / APRON_CELL) as i32 + 1;
    let paint = Paint::new(pal.lawn, Pattern::Lawn);
    for iz in z0..z1 {
        for ix in x0..x1 {
            let c = |dx: i32, dz: i32| {
                V2::new((ix + dx) as f32 * APRON_CELL, (iz + dz) as f32 * APRON_CELL)
            };
            let q = [c(0, 0), c(1, 0), c(1, 1), c(0, 1)];
            let colors = q.map(|p| lawn_color(pal, seed, p, false));
            let m = out.at((q[0] + q[2]) * 0.5, MeshKind::Ground);
            quad_vc(m, q.map(|p| p.at(TERRAIN_Y)), V3::Y, paint, colors);
        }
    }
}

/// The block's ground at curb height, on a grid clipped to the (convex) block polygon so
/// the lawn noise has vertices to live on.
fn lot_pad(b: &Block, surface: Surface, pal: &Palette, seed: u64, out: &mut MeshSet) {
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for p in &b.poly {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let color = |p: V2| match surface {
        Surface::Lawn => lawn_color(pal, seed, p, false),
        Surface::ParkLawn => lawn_color(pal, seed, p, true),
        Surface::Paved => paved_color(pal, seed, p),
    };
    let paint = match surface {
        Surface::Paved => Paint::new(pal.asphalt_worn, Pattern::Asphalt),
        _ => Paint::new(pal.lawn, Pattern::Lawn),
    };
    let m = out.at(centroid(&b.poly), MeshKind::Ground);
    let x0 = floor(lo.x / LOT_CELL) as i32;
    let x1 = floor(hi.x / LOT_CELL) as i32 + 1;
    let z0 = floor(lo.y / LOT_CELL) as i32;
    let z1 = floor(hi.y / LOT_CELL) as i32 + 1;
    let mut clipped: Vec<V2> = Vec::with_capacity(12);
    let mut scratch: Vec<V2> = Vec::with_capacity(12);
    for iz in z0..z1 {
        for ix in x0..x1 {
            let (cx0, cz0) = (ix as f32 * LOT_CELL, iz as f32 * LOT_CELL);
            let (cx1, cz1) = (cx0 + LOT_CELL, cz0 + LOT_CELL);
            let corners = [
                V2::new(cx0, cz0),
                V2::new(cx1, cz0),
                V2::new(cx1, cz1),
                V2::new(cx0, cz1),
            ];
            if corners.iter().all(|&c| point_in_polygon(c, &b.poly)) {
                let colors = corners.map(color);
                quad_vc(m, corners.map(|p| p.at(LOT_Y)), V3::Y, paint, colors);
                continue;
            }
            clipped.clear();
            clipped.extend_from_slice(&b.poly);
            for (axis, value, keep_greater) in [
                (0, cx0, true),
                (0, cx1, false),
                (1, cz0, true),
                (1, cz1, false),
            ] {
                clip(&clipped, axis, value, keep_greater, &mut scratch);
                core::mem::swap(&mut clipped, &mut scratch);
                if clipped.len() < 3 {
                    break;
                }
            }
            if clipped.len() < 3 {
                continue;
            }
            for k in 1..clipped.len() - 1 {
                let (a, bb, c) = (clipped[0], clipped[k], clipped[k + 1]);
                let t = [a.at(LOT_Y), bb.at(LOT_Y), c.at(LOT_Y)];
                tri_vc(m, t, V3::Y, paint, [color(a), color(bb), color(c)]);
            }
        }
    }
}

/// Sutherland-Hodgman clip of `poly` against an axis-aligned half-plane.
fn clip(poly: &[V2], axis: usize, value: f32, keep_greater: bool, out: &mut Vec<V2>) {
    out.clear();
    let get = |p: V2| if axis == 0 { p.x } else { p.y };
    let inside = |p: V2| {
        if keep_greater {
            get(p) >= value
        } else {
            get(p) <= value
        }
    };
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (ia, ib) = (inside(a), inside(b));
        if ia {
            out.push(a);
        }
        if ia != ib {
            let t = (value - get(a)) / (get(b) - get(a));
            out.push(a.lerp(b, t));
        }
    }
}

/// Concrete paths from each side's midpoint to a round plaza with a fountain.
fn park_paths(b: &Block, pal: &Palette, out: &mut MeshSet) {
    let c = centroid(&b.poly);
    let m = out.at(c, MeshKind::Ground);
    let walk = Paint::new(mix(pal.concrete, pal.sand, 100), Pattern::Concrete);
    let y = LOT_Y + PAINT_LIFT * 2.0;
    let n = b.poly.len();
    for i in 0..n {
        let mid = (b.poly[i] + b.poly[(i + 1) % n]) * 0.5;
        let d = (c - mid).normalize();
        let w = d.left() * 1.4;
        let q = [mid + w, mid - w, c - w, c + w].map(|p| p.at(y));
        quad(m, q, V3::Y, walk);
    }
    let plaza = 9.0;
    let ring = 16;
    for k in 0..ring {
        let (s0, c0) = crate::math::sin_cos(crate::math::TAU * k as f32 / ring as f32);
        let (s1, c1) = crate::math::sin_cos(crate::math::TAU * (k + 1) as f32 / ring as f32);
        let a = c + V2::new(c0, s0) * plaza;
        let bb = c + V2::new(c1, s1) * plaza;
        tri(
            m,
            c.at(y + 0.002),
            a.at(y + 0.002),
            bb.at(y + 0.002),
            V3::Y,
            walk,
        );
    }
}

/// Asphalt in front of a strip mall with stall lines, from its front wall to the property
/// line, leaving a lawn strip along the street.
#[allow(clippy::too_many_arguments)]
pub(crate) fn parking_lot(
    footprint: &[V2],
    front: usize,
    b: &Block,
    pal: &Palette,
    seed: u64,
    out: &mut MeshSet,
    cars: &mut Vec<Prop>,
) {
    let mut rng = Rng::new(Seed(seed), &[0x4C4F_5453]);
    let (a, bb) = (footprint[front], footprint[(front + 1) % footprint.len()]);
    let normal = (bb - a).normalize().right();
    let Some(depth) = depth_ahead((a + bb) * 0.5, normal, &b.poly) else {
        return;
    };
    let along = (bb - a).normalize();
    let len = (bb - a).length();
    let y = LOT_Y + PAINT_LIFT * 2.0;
    let lot_depth = depth - 3.0;
    let m = out.at(centroid(footprint), MeshKind::Ground);
    let asphalt = Paint::new(pal.asphalt, Pattern::Asphalt);
    let walkway = Paint::new(pal.concrete, Pattern::Concrete);
    let white = Paint::new(pal.paint_white, Pattern::Paint);
    let p = |s: f32, t: f32, yy: f32| (a + along * s + normal * t).at(yy);
    let (s0, s1) = (-6.0, len + 6.0);
    // Covered walkway in front of the shops, then asphalt.
    quad(
        m,
        [p(s0, 0.0, y), p(s1, 0.0, y), p(s1, 3.6, y), p(s0, 3.6, y)],
        V3::Y,
        walkway,
    );
    quad(
        m,
        [
            p(s0, 3.6, y),
            p(s1, 3.6, y),
            p(s1, lot_depth, y),
            p(s0, lot_depth, y),
        ],
        V3::Y,
        asphalt,
    );
    // Stall rows: single row along the walkway, then double rows separated by aisles.
    let ly = y + PAINT_LIFT;
    let mut rows: Vec<(f32, f32)> = Vec::new();
    let mut t = 3.6;
    rows.push((t, t + 5.5));
    t += 5.5 + 7.5;
    while t + 11.0 < lot_depth - 7.0 {
        rows.push((t, t + 5.5));
        rows.push((t + 5.5, t + 11.0));
        t += 11.0 + 7.5;
    }
    for (k, &(t0, t1)) in rows.iter().enumerate() {
        let mut s = s0 + 2.0;
        while s < s1 - 2.0 {
            quad(
                m,
                [
                    p(s, t0, ly),
                    p(s + 0.12, t0, ly),
                    p(s + 0.12, t1, ly),
                    p(s, t1, ly),
                ],
                V3::Y,
                white,
            );
            // Busier near the shops.
            let chance = if k < 2 { 150 } else { 80 };
            if s + 2.8 < s1 - 2.0 && rng.range(0, 256) < chance {
                let nose = if k % 2 == 0 { -normal } else { normal };
                cars.push(Prop {
                    pos: a + along * (s + 1.46) + normal * ((t0 + t1) * 0.5),
                    y: LOT_Y,
                    dir: if rng.range(0, 5) == 0 { -nose } else { nose },
                    kind: PropKind::ParkedCar {
                        color: rng.range(0, 8) as u8,
                    },
                    seed: rng.next_u32(),
                });
            }
            s += 2.8;
        }
    }
}

/// Distance from `mid` straight along `normal` to the block's boundary, if it hits.
pub fn depth_ahead(mid: V2, normal: V2, block: &[V2]) -> Option<f32> {
    let mut depth = f32::MAX;
    for k in 0..block.len() {
        let (p, q) = (block[k], block[(k + 1) % block.len()]);
        if let Some(hit) = crate::math::intersect_lines(mid, normal, p, (q - p).normalize()) {
            let t = (hit - mid).dot(normal);
            let (dist, _) = crate::math::segment_distance(hit, p, q);
            if t > 0.0 && dist < 0.01 {
                depth = depth.min(t);
            }
        }
    }
    (depth < f32::MAX).then_some(depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_blocks_match_the_grid() {
        let place = Place::test_grid();
        let net = Network::new(&place);
        let blocks = blocks(&net);
        // 64 blocks minus the two merged into the strip-mall superblock, plus that block.
        assert_eq!(blocks.len(), 63);
        let areas: Vec<f32> = blocks.iter().map(|b| signed_area(&b.poly)).collect();
        // A local-on-all-sides block is 85 m square.
        assert!(
            areas.iter().any(|&a| (a - 85.0 * 85.0).abs() < 1.0),
            "{areas:?}"
        );
        // Every building sits inside some block.
        for bld in &place.buildings {
            let c = centroid(&to_v2(&bld.footprint));
            assert!(blocks.iter().any(|b| point_in_polygon(c, &b.poly)));
        }
    }
}
