//! The sea: low ground the open water at the frame's edge reaches, traced into polygons.

use crate::net::simplify;
use crate::sources::Dem;
use crate::Warp;

/// Mask cell size, meters.
const CELL_M: f32 = 16.0;
/// Water bodies smaller than this many cells are dropped.
const MIN_CELLS: usize = 64;
/// Shoreline simplification tolerance, meters.
const TOLERANCE_M: f32 = 10.0;

/// Polygons of the sea, counter-clockwise in (x, z), each closed implicitly.
pub fn sea(dem: &Dem, warp: &Warp, ext: [f32; 2], below_m: f32) -> Vec<Vec<(f32, f32)>> {
    let w = (ext[0] / CELL_M).round() as usize;
    let h = (ext[1] / CELL_M).round() as usize;
    let low: Vec<bool> = (0..w * h)
        .map(|i| {
            let (x, z) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let (lon, lat) = warp.inv(x * CELL_M, z * CELL_M);
            dem.at(lon, lat) <= below_m
        })
        .collect();

    // Water: low cells the frame's edge reaches, 4-connected.
    let mut water = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h)
        .filter(|&i| {
            let (x, z) = (i % w, i / w);
            low[i] && (x == 0 || z == 0 || x == w - 1 || z == h - 1)
        })
        .collect();
    flood(&mut stack, &mut water, w, h, |i| low[i]);
    // Land the frame's edge does not reach (islands) goes under too: polygons have no holes.
    let mut land = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h)
        .filter(|&i| {
            let (x, z) = (i % w, i / w);
            !water[i] && (x == 0 || z == 0 || x == w - 1 || z == h - 1)
        })
        .collect();
    flood(&mut stack, &mut land, w, h, |i| !water[i]);
    let water: Vec<bool> = land.iter().map(|&l| !l).collect();

    // One polygon per 4-connected body.
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    for s in 0..w * h {
        if !water[s] || seen[s] {
            continue;
        }
        let mut body = vec![false; w * h];
        let mut stack = vec![s];
        flood(&mut stack, &mut body, w, h, |i| water[i]);
        let cells = body.iter().filter(|&&b| b).count();
        for (i, b) in body.iter().enumerate() {
            seen[i] |= *b;
        }
        if cells < MIN_CELLS {
            continue;
        }
        let ring = trace(&body, w, h);
        let pts: Vec<(f32, f32)> = ring
            .iter()
            .map(|&(x, z)| (x as f32 * CELL_M, z as f32 * CELL_M))
            .collect();
        out.push(simplify_ring(&pts, TOLERANCE_M));
    }
    out
}

fn flood(
    stack: &mut Vec<usize>,
    into: &mut [bool],
    w: usize,
    h: usize,
    ok: impl Fn(usize) -> bool,
) {
    for &i in stack.iter() {
        into[i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, z) = (i % w, i / w);
        let mut push = |j: usize| {
            if !into[j] && ok(j) {
                into[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if z > 0 {
            push(i - w);
        }
        if z + 1 < h {
            push(i + w);
        }
    }
}

/// The outer boundary of a 4-connected, hole-free body of cells, as grid-corner points.
/// Cells outside the grid count as not in the body.
fn trace(body: &[bool], w: usize, h: usize) -> Vec<(i32, i32)> {
    let at = |x: i32, z: i32| {
        x >= 0
            && z >= 0
            && (x as usize) < w
            && (z as usize) < h
            && body[z as usize * w + x as usize]
    };
    // Directed cell edges with the body on the left (x east, z south: "left" of a step
    // east is north). Each starts at a corner; at most two leave any corner.
    let mut next: Vec<[(i32, i32, i32, i32); 2]> = vec![[(0, 0, 0, 0); 2]; (w + 1) * (h + 1)];
    let mut count = vec![0u8; (w + 1) * (h + 1)];
    let mut first = None;
    let corner = |x: i32, z: i32| z as usize * (w + 1) + x as usize;
    for z in 0..h as i32 {
        for x in 0..w as i32 {
            if !at(x, z) {
                continue;
            }
            // Edges round the cell counter-clockwise on screen (z down): body on the left.
            let mut add = |a: (i32, i32), b: (i32, i32)| {
                let k = corner(a.0, a.1);
                next[k][count[k] as usize] = (a.0, a.1, b.0, b.1);
                count[k] += 1;
                first.get_or_insert(k);
            };
            if !at(x, z - 1) {
                add((x + 1, z), (x, z)); // north side, walking west
            }
            if !at(x - 1, z) {
                add((x, z), (x, z + 1)); // west side, walking south
            }
            if !at(x, z + 1) {
                add((x, z + 1), (x + 1, z + 1)); // south side, walking east
            }
            if !at(x + 1, z) {
                add((x + 1, z + 1), (x + 1, z)); // east side, walking north
            }
        }
    }
    let Some(k0) = first else {
        return Vec::new();
    };
    let start = next[k0][0];
    let mut used = vec![[false; 2]; (w + 1) * (h + 1)];
    used[k0][0] = true;
    let mut ring = vec![(start.0, start.1)];
    let mut cur = start;
    loop {
        let k = corner(cur.2, cur.3);
        let d = (cur.2 - cur.0, cur.3 - cur.1);
        // Two ways out of a pinch corner: take the left turn, so diagonal cells stay apart.
        let mut pick = None;
        for j in 0..count[k] as usize {
            if used[k][j] {
                continue;
            }
            let e = next[k][j];
            let o = (e.2 - e.0, e.3 - e.1);
            // Left of (dx, dz) with z down is (dz, -dx).
            let left = o == (d.1, -d.0);
            if pick.is_none() || left {
                pick = Some(j);
            }
        }
        let Some(j) = pick else { break };
        used[k][j] = true;
        cur = next[k][j];
        if (cur.0, cur.1) == (start.0, start.1) {
            break;
        }
        ring.push((cur.0, cur.1));
    }
    ring
}

/// Douglas-Peucker on a closed ring, split at its first point and the point farthest from it.
fn simplify_ring(pts: &[(f32, f32)], tol: f32) -> Vec<(f32, f32)> {
    if pts.len() < 4 {
        return pts.to_vec();
    }
    let d = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).hypot(a.1 - b.1);
    let far = (0..pts.len())
        .max_by(|&a, &b| d(pts[0], pts[a]).total_cmp(&d(pts[0], pts[b])))
        .unwrap();
    let mut a = simplify(&pts[..=far], tol);
    let mut tail: Vec<(f32, f32)> = pts[far..].to_vec();
    tail.push(pts[0]);
    let b = simplify(&tail, tol);
    a.pop();
    a.extend_from_slice(&b[..b.len() - 1]);
    a
}

#[cfg(test)]
mod tests {
    use super::trace;

    #[test]
    fn trace_walks_round_a_block() {
        // A 2 x 1 body in a 3 x 2 grid.
        let body = [true, true, false, false, false, false];
        let mut ring = trace(&body, 3, 2);
        ring.sort();
        assert_eq!(ring, vec![(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]);
    }
}
