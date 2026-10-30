//! A pickup's open cargo bed (`Detail::PickupBed`): the loft leaves the top out over
//! `Body::bed` (`Body::open_cut`), and this lines the hole with painted rail caps, inner
//! walls, a front bulkhead, the tailgate's inner face, a ribbed floor and wheel tubs over
//! the rear tires.

use super::body::Body;
use super::math::*;
use super::mesh::Soup;
use super::Materials;
use alloc::vec::Vec;

const UP: V3 = [0.0, 1.0, 0.0];
const FWD: V3 = [0.0, 0.0, -1.0];
const BACK: V3 = [0.0, 0.0, 1.0];

/// Width of the rail cap inboard of the belt ledge, m.
const CAP_W: f32 = 0.05;
/// Floor depth under the rail, m.
const DEPTH: f32 = 0.46;

/// The lower paint's shades on the bed's walls, floor and ribs.
pub(crate) const SHADES: [f32; 3] = [0.8, 0.62, 0.74];

/// Lines the bed into `out`, if the body has one.
pub fn build(b: &Body, m: &Materials, out: &mut Soup) {
    let Some((s0, s1)) = b.bed else {
        return;
    };
    let lod0 = b.lod == 0;
    let wh = &b.spec.wheels;
    let ax = b.axle[1];
    // Tubs cover the rear tires where they rise through the floor.
    let (t0, t1) = (ax - b.ra - 0.02, ax + b.ra + 0.02);
    let tub_top = b.wy + b.ra + 0.03;
    let zt = wh.track * 0.5 - wh.width * 0.5 - 0.04;
    let mid = b.frame((s0 + s1) * 0.5);
    let fl = (mid.y_ledge - DEPTH).max(mid.yb + 0.08);

    // Stations: the loft's within the bed, plus the bed and tub ends.
    let gap = if lod0 { 0.02 } else { 0.12 };
    let mut sts: Vec<f32> = alloc::vec![s0, s1, t0, t1];
    sts.extend(b.stations.iter().map(|t| t.s).filter(|&s| s > s0 && s < s1));
    sts.retain(|&s| s >= s0 && s <= s1);
    sts.sort_by(|a, c| a.partial_cmp(c).unwrap_or(core::cmp::Ordering::Equal));
    let mut kept: Vec<f32> = Vec::new();
    for s in sts {
        let keep = [s0, s1, t0, t1].contains(&s);
        match kept.last_mut() {
            Some(last) if s - *last < gap => {
                if keep {
                    *last = s;
                }
            }
            _ => kept.push(s),
        }
    }
    let over_tub = |s: f32| s > t0 + 1e-4 && s < t1 - 1e-4;
    let paint = m.paint_low;
    let wall = paint.shade(SHADES[0]);
    let p = |s: f32, z: f32, y: f32| b.p(s, z, y);

    // Rail caps and inner walls, down to the floor or the tub top.
    for k in 0..kept.len() - 1 {
        let (sa, sb) = (kept[k], kept[k + 1]);
        let (fa, fb) = (b.frame(sa), b.frame(sb));
        let low = if over_tub((sa + sb) * 0.5) {
            tub_top
        } else {
            fl
        };
        for sx in [-1.0f32, 1.0] {
            let (za, zb) = (fa.z_ledge, fb.z_ledge);
            out.quad_dir(
                p(sa, sx * za, fa.y_ledge),
                p(sb, sx * zb, fb.y_ledge),
                p(sb, sx * (zb - CAP_W), fb.y_ledge - 0.004),
                p(sa, sx * (za - CAP_W), fa.y_ledge - 0.004),
                paint,
                UP,
            );
            out.quad_dir(
                p(sa, sx * (za - CAP_W), fa.y_ledge - 0.004),
                p(sb, sx * (zb - CAP_W), fb.y_ledge - 0.004),
                p(sb, sx * (zb - CAP_W), low),
                p(sa, sx * (za - CAP_W), low),
                wall,
                [-sx, 0.0, 0.0],
            );
        }
    }

    // Front bulkhead and the tailgate's inner face.
    for (s, dir) in [(s0, BACK), (s1, FWD)] {
        let f = b.frame(s);
        let z = f.z_ledge - CAP_W;
        out.quad_dir(
            p(s, -z, fl),
            p(s, z, fl),
            p(s, z, f.y_ledge - 0.004),
            p(s, -z, f.y_ledge - 0.004),
            wall,
            dir,
        );
    }

    // Floor: full width ahead of and behind the tubs, between them in the middle.
    let floor = paint.shade(SHADES[1]);
    let zi = |s: f32| b.frame(s).z_ledge - CAP_W;
    for (a, c, za, zc) in [
        (s0, t0, zi(s0), zi(t0)),
        (t0, t1, zt, zt),
        (t1, s1, zi(t1), zi(s1)),
    ] {
        out.quad_dir(
            p(a, -za, fl),
            p(a, za, fl),
            p(c, zc, fl),
            p(c, -zc, fl),
            floor,
            UP,
        );
    }
    // Tubs: boxes from the floor to over the tire, against the walls.
    for sx in [-1.0f32, 1.0] {
        let zw = zi(ax);
        let (x0, x1) = if sx > 0.0 { (zt, zw) } else { (-zw, -zt) };
        out.aabb(
            [x0, fl, t0 - b.axle[0]],
            [x1, tub_top, t1 - b.axle[0]],
            wall,
        );
    }
    if !lod0 {
        return;
    }
    // Raised ribs along the floor, clear of the tubs.
    let rib = paint.shade(SHADES[2]);
    let n = 5;
    for k in 0..n {
        let z = lerp(-zt + 0.08, zt - 0.08, k as f32 / (n - 1) as f32);
        out.aabb(
            [z - 0.02, fl, s0 + 0.03 - b.axle[0]],
            [z + 0.02, fl + 0.012, s1 - 0.03 - b.axle[0]],
            rib,
        );
    }
}
