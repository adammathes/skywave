//! The main road network: TIGER lines rasterized at the frame's scale (so a divided highway's
//! two carriageways, a few meters apart once squeezed, become one road), thinned to a
//! skeleton, traced into chains between junctions, trimmed to its 2-core (every chain ends
//! at a junction: no roads to nowhere), and joined into long roads that run straight on
//! through junctions.

use crate::sources::Line;
use crate::Warp;

/// A road ready to write out.
pub struct Road {
    pub name: String,
    pub highway: bool,
    pub points: Vec<(f32, f32)>,
    pub ramps: Vec<String>,
    pub suspension: bool,
}

/// Chains shorter than this between junctions are collapsed into one junction.
const MERGE_JUNCTION_M: f32 = 45.0;
/// Small loops (an interchange, a frontage road beside its highway) under this are cut.
const SMALL_LOOP_M: f32 = 700.0;
/// Two chain ends at a junction continue as one road when they turn less than this.
const STRAIGHT_DEG: f32 = 50.0;
/// Road simplification tolerance, meters.
const TOLERANCE_M: f32 = 6.0;
/// No bend in a road turns more than this at one point: cars follow the polyline.
const MAX_BEND_DEG: f32 = 12.0;
/// A road end meeting another road this close to one of its points joins at that point.
const JOIN_AT_BEND_M: f32 = 15.0;
/// Roads run straight this far into a joint.
const STRAIGHT_IN_M: f32 = 25.0;
/// Ramps sit at least this far along the highway from its ends and from each other.
const RAMP_CLEAR_M: f32 = 200.0;
const RAMP_SPACING_M: f32 = 500.0;

type P = (f32, f32);

fn dist(a: P, b: P) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn length(l: &[P]) -> f32 {
    l.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Douglas-Peucker on an open polyline; keeps both ends.
pub fn simplify(pts: &[P], tol: f32) -> Vec<P> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let (mut worst, mut at) = (0.0, 0);
    for (i, &p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let d = seg_dist(p, a, b);
        if d > worst {
            worst = d;
            at = i;
        }
    }
    if worst <= tol {
        return vec![a, b];
    }
    let mut l = simplify(&pts[..=at], tol);
    let r = simplify(&pts[at..], tol);
    l.pop();
    l.extend(r);
    l
}

/// Each inner point moved to the mean of itself and two neighbours either side: irons out
/// the skeleton's staircase. Keeps both ends.
fn average(pts: &[P]) -> Vec<P> {
    let n = pts.len();
    (0..n)
        .map(|i| {
            if i == 0 || i == n - 1 {
                return pts[i];
            }
            let k = 2.min(i).min(n - 1 - i);
            let (mut x, mut z) = (0.0, 0.0);
            for p in &pts[i - k..=i + k] {
                x += p.0;
                z += p.1;
            }
            let c = (2 * k + 1) as f32;
            (x / c, z / c)
        })
        .collect()
}

/// One round of corner cutting; keeps both ends.
/// Cuts every bend sharper than `MAX_BEND_DEG` into two gentler ones until none is left.
/// Points in `fixed` (junctions) stay as they are.
fn ease(pts: &[P], fixed: &[P]) -> Vec<P> {
    let mut pts = pts.to_vec();
    loop {
        let bend = (1..pts.len().saturating_sub(1)).find(|&i| {
            let (a, b, c) = (pts[i - 1], pts[i], pts[i + 1]);
            if fixed.contains(&b) {
                return false;
            }
            let d = (c.1 - b.1).atan2(c.0 - b.0) - (b.1 - a.1).atan2(b.0 - a.0);
            let d = (d + core::f32::consts::PI).rem_euclid(core::f32::consts::TAU)
                - core::f32::consts::PI;
            d.abs() > MAX_BEND_DEG.to_radians()
        });
        let Some(i) = bend else { return pts };
        let (a, b, c) = (pts[i - 1], pts[i], pts[i + 1]);
        let cut = |to: P| {
            let k = (12.0 / dist(b, to)).min(0.25);
            (b.0 + (to.0 - b.0) * k, b.1 + (to.1 - b.1) * k)
        };
        pts.splice(i..=i, [cut(a), cut(c)]);
    }
}

fn chaikin(pts: &[P]) -> Vec<P> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        out.push((a.0 * 0.75 + b.0 * 0.25, a.1 * 0.75 + b.1 * 0.25));
        out.push((a.0 * 0.25 + b.0 * 0.75, a.1 * 0.25 + b.1 * 0.75));
    }
    out.push(pts[pts.len() - 1]);
    out
}

fn seg_dist(p: P, a: P, b: P) -> f32 {
    let (dx, dz) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dz * dz;
    let t = if l2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dz) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    dist(p, (a.0 + dx * t, a.1 + dz * t))
}

/// The skeleton grid: which cells hold a road.
struct Grid {
    w: usize,
    h: usize,
    on: Vec<bool>,
}

impl Grid {
    fn get(&self, x: i32, z: i32) -> bool {
        x >= 0
            && z >= 0
            && (x as usize) < self.w
            && (z as usize) < self.h
            && self.on[z as usize * self.w + x as usize]
    }
}

const N8: [(i32, i32); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];

/// Zhang-Suen thinning, in place.
fn thin(g: &mut Grid) {
    loop {
        let mut changed = false;
        for pass in 0..2 {
            let mut kill = Vec::new();
            for z in 0..g.h as i32 {
                for x in 0..g.w as i32 {
                    if !g.get(x, z) {
                        continue;
                    }
                    let n: Vec<bool> = N8.iter().map(|&(dx, dz)| g.get(x + dx, z + dz)).collect();
                    let b = n.iter().filter(|&&v| v).count();
                    if !(2..=6).contains(&b) {
                        continue;
                    }
                    let a = (0..8).filter(|&i| !n[i] && n[(i + 1) % 8]).count();
                    if a != 1 {
                        continue;
                    }
                    // n[0] north, n[2] east, n[4] south, n[6] west.
                    let ok = if pass == 0 {
                        !(n[0] && n[2] && n[4]) && !(n[2] && n[4] && n[6])
                    } else {
                        !(n[0] && n[2] && n[6]) && !(n[0] && n[4] && n[6])
                    };
                    if ok {
                        kill.push(z as usize * g.w + x as usize);
                    }
                }
            }
            changed |= !kill.is_empty();
            for i in kill {
                g.on[i] = false;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Neighbours under m-adjacency: a diagonal counts only when neither shared side cell is set,
/// so a staircase is a line, not a chain of triangles.
fn neighbours(g: &Grid, x: i32, z: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for &(dx, dz) in &N8 {
        if !g.get(x + dx, z + dz) {
            continue;
        }
        if dx != 0 && dz != 0 && (g.get(x + dx, z) || g.get(x, z + dz)) {
            continue;
        }
        out.push((x + dx, z + dz));
    }
    out
}

struct Chain {
    a: usize,
    b: usize,
    pts: Vec<P>,
    highway: bool,
    suspension: bool,
    alive: bool,
}

/// The road network of `lines` in the frame.
/// Chains passing within this of a `suspension` point are that bridge.
const SUSPENSION_NEAR_M: f32 = 40.0;

/// The road network of `lines` in the frame. A chain that passes one of the `suspension`
/// points is a suspension bridge and stays a road of its own.
pub fn network(
    lines: &[Line],
    warp: &Warp,
    ext: [f32; 2],
    cell: f32,
    suspension: &[P],
    separate_highways: bool,
) -> Vec<Road> {
    if !separate_highways {
        let all: Vec<&Line> = lines.iter().collect();
        let (mut nodes, mut chains) = skeleton(&all, warp, ext, cell, None, suspension);
        cleanup(&mut nodes, &mut chains);
        return join(&nodes, &chains, false);
    }
    // Highways on a skeleton of their own, so where one crosses a street there is no
    // junction (it passes over) and a street corner never drags it off its line.
    let art: Vec<&Line> = lines.iter().filter(|l| l.mtfcc != "S1100").collect();
    let hwy: Vec<&Line> = lines.iter().filter(|l| l.mtfcc == "S1100").collect();
    let (mut nodes, mut chains) = skeleton(&art, warp, ext, cell, Some(false), suspension);
    let (hn, hc) = skeleton(&hwy, warp, ext, cell, Some(true), suspension);
    let off = nodes.len();
    nodes.extend(hn);
    chains.extend(hc.into_iter().map(|mut c| {
        c.a += off;
        c.b += off;
        c
    }));
    // A highway's loose end (off the frame, or short of the streets) is cut back to where
    // it first passes a street junction, and meets the streets there.
    let street: Vec<usize> = (0..off).filter(|&m| degree(&chains, m) > 0).collect();
    for n in off..nodes.len() {
        if degree(&chains, n) != 1 {
            continue;
        }
        let Some(ci) = chains.iter().position(|c| c.a == n || c.b == n) else {
            continue;
        };
        if chains[ci].b == n {
            chains[ci].pts.reverse();
            let c = &mut chains[ci];
            core::mem::swap(&mut c.a, &mut c.b);
        }
        // Walk in from the loose end to the first point near a street junction.
        let hit = chains[ci].pts.iter().enumerate().find_map(|(k, &p)| {
            street
                .iter()
                .map(|&m| (dist(p, nodes[m]), m))
                .filter(|&(d, _)| d < ATTACH_M)
                .min_by(|x, y| x.0.total_cmp(&y.0))
                .map(|(_, m)| (k, m))
        });
        let Some((k, m)) = hit else { continue };
        let c = &mut chains[ci];
        c.pts.drain(..k);
        c.pts[0] = nodes[m];
        c.a = m;
        // No hook where the end jumped onto the junction: drop points that double back.
        while c.pts.len() > 2 {
            let (p0, p1, p2) = (c.pts[0], c.pts[1], c.pts[2]);
            let u = (p1.0 - p0.0, p1.1 - p0.1);
            let v = (p2.0 - p1.0, p2.1 - p1.1);
            if u.0 * v.0 + u.1 * v.1 >= 0.0 {
                break;
            }
            c.pts.remove(1);
        }
    }
    cleanup(&mut nodes, &mut chains);
    join(&nodes, &chains, true)
}

/// Shorter highways are left out.
const MIN_HIGHWAY_M: f32 = 300.0;

/// A highway end this close to a street junction meets it.
const ATTACH_M: f32 = 90.0;

/// The skeleton of `lines`: its junctions and the chains between them.
fn skeleton(
    lines: &[&Line],
    warp: &Warp,
    ext: [f32; 2],
    cell: f32,
    highways: Option<bool>,
    suspension: &[P],
) -> (Vec<P>, Vec<Chain>) {
    let w = (ext[0] / cell).ceil() as usize;
    let h = (ext[1] / cell).ceil() as usize;
    let mut g = Grid {
        w,
        h,
        on: vec![false; w * h],
    };
    // Cells a highway runs through (for chains whose kind is not given).
    let mut hwy = vec![false; w * h];
    // Rasterize: step a quarter cell along each segment; 4-connect diagonal steps.
    for l in lines {
        let hw = l.mtfcc == "S1100";
        let pts: Vec<P> = l.lonlat.iter().map(|&(a, b)| warp.fwd(a, b)).collect();
        for s in pts.windows(2) {
            let n = (dist(s[0], s[1]) / (cell * 0.25)).ceil().max(1.0) as usize;
            let mut last: Option<(i32, i32)> = None;
            for k in 0..=n {
                let t = k as f32 / n as f32;
                let p = (
                    s[0].0 + (s[1].0 - s[0].0) * t,
                    s[0].1 + (s[1].1 - s[0].1) * t,
                );
                // Keep a cell's margin off the frame's edge.
                if p.0 < cell || p.1 < cell || p.0 > ext[0] - cell || p.1 > ext[1] - cell {
                    last = None;
                    continue;
                }
                let c = ((p.0 / cell) as i32, (p.1 / cell) as i32);
                let mut mark = |c: (i32, i32)| {
                    let i = c.1 as usize * w + c.0 as usize;
                    g.on[i] = true;
                    hwy[i] |= hw;
                };
                if let Some(q) = last {
                    if q.0 != c.0 && q.1 != c.1 {
                        mark((c.0, q.1));
                    }
                }
                mark(c);
                last = Some(c);
            }
        }
    }
    // Close one-cell gaps between near-parallel marks so thinning gives one line.
    let before = g.on.clone();
    for z in 1..h - 1 {
        for x in 1..w - 1 {
            let i = z * w + x;
            if before[i] {
                continue;
            }
            let (n, s, e, wst) = (before[i - w], before[i + w], before[i + 1], before[i - 1]);
            if (n && s) || (e && wst) {
                g.on[i] = true;
            }
        }
    }
    thin(&mut g);

    // Nodes: clusters of junction cells (three or more neighbours) and line ends.
    let center = |x: i32, z: i32| ((x as f32 + 0.5) * cell, (z as f32 + 0.5) * cell);
    let mut node_of = vec![usize::MAX; w * h];
    let mut nodes: Vec<P> = Vec::new();
    for z in 0..h as i32 {
        for x in 0..w as i32 {
            let i = z as usize * w + x as usize;
            if !g.on[i] || node_of[i] != usize::MAX {
                continue;
            }
            let deg = neighbours(&g, x, z).len();
            if deg == 2 {
                continue;
            }
            // Flood the cluster of non-2 cells.
            let id = nodes.len();
            let mut stack = vec![(x, z)];
            node_of[i] = id;
            let (mut sx, mut sz, mut k) = (0.0, 0.0, 0.0);
            while let Some((cx, cz)) = stack.pop() {
                let c = center(cx, cz);
                sx += c.0;
                sz += c.1;
                k += 1.0;
                for (nx, nz) in neighbours(&g, cx, cz) {
                    let j = nz as usize * w + nx as usize;
                    if node_of[j] == usize::MAX && neighbours(&g, nx, nz).len() != 2 {
                        node_of[j] = id;
                        stack.push((nx, nz));
                    }
                }
            }
            nodes.push((sx / k, sz / k));
        }
    }

    // Chains: walk from each node cell out along 2-neighbour cells to the next node.
    let mut chains: Vec<Chain> = Vec::new();
    let mut walked = vec![false; w * h];
    for z in 0..h as i32 {
        for x in 0..w as i32 {
            let i = z as usize * w + x as usize;
            let a = node_of[i];
            if !g.on[i] || a == usize::MAX {
                continue;
            }
            for start in neighbours(&g, x, z) {
                let j = start.1 as usize * w + start.0 as usize;
                if node_of[j] == a || walked[j] {
                    continue;
                }
                let (mut prev, mut cur) = ((x, z), start);
                let mut pts = vec![nodes[a]];
                let mut on_hwy = 0;
                let b = loop {
                    let k = cur.1 as usize * w + cur.0 as usize;
                    if node_of[k] != usize::MAX {
                        break node_of[k];
                    }
                    walked[k] = true;
                    on_hwy += usize::from(hwy[k]);
                    pts.push(center(cur.0, cur.1));
                    let nb = neighbours(&g, cur.0, cur.1);
                    let Some(&nx) = nb.iter().find(|&&q| q != prev) else {
                        break usize::MAX;
                    };
                    prev = cur;
                    cur = nx;
                };
                if b == usize::MAX || (b == a && pts.len() < 3) {
                    continue;
                }
                // Two adjacent node cells of different clusters: walk once.
                if pts.len() == 1 && b < a {
                    continue;
                }
                // Mostly on highway cells: a highway.
                let highway = highways.unwrap_or(on_hwy * 2 > pts.len() - 1);
                pts.push(nodes[b]);
                let suspended = pts
                    .iter()
                    .any(|&p| suspension.iter().any(|&q| dist(p, q) < SUSPENSION_NEAR_M));
                chains.push(Chain {
                    a,
                    b,
                    pts,
                    highway,
                    suspension: suspended,
                    alive: true,
                });
            }
        }
    }

    (nodes, chains)
}

fn degree(chains: &[Chain], n: usize) -> usize {
    chains
        .iter()
        .filter(|c| c.alive)
        .map(|c| usize::from(c.a == n) + usize::from(c.b == n))
        .sum()
}

/// Trim to the 2-core, cut small loops, collapse near junctions; until nothing changes.
fn cleanup(nodes: &mut [P], chains: &mut [Chain]) {
    loop {
        let mut changed = false;
        // Dead ends.
        for i in 0..chains.len() {
            if chains[i].alive
                && (degree(chains, chains[i].a) == 1 || degree(chains, chains[i].b) == 1)
            {
                chains[i].alive = false;
                changed = true;
            }
        }
        // Small self-loops, and the longer of two short chains between the same nodes.
        for i in 0..chains.len() {
            if !chains[i].alive {
                continue;
            }
            let li = length(&chains[i].pts);
            if chains[i].a == chains[i].b && li < SMALL_LOOP_M {
                chains[i].alive = false;
                changed = true;
                continue;
            }
            for j in 0..chains.len() {
                if i == j || !chains[j].alive {
                    continue;
                }
                let same = (chains[i].a == chains[j].a && chains[i].b == chains[j].b)
                    || (chains[i].a == chains[j].b && chains[i].b == chains[j].a);
                let lj = length(&chains[j].pts);
                if same && li + lj < SMALL_LOOP_M && (lj > li || (lj == li && j > i)) {
                    chains[j].alive = false;
                    changed = true;
                }
            }
        }
        // Collapse a short chain: its far node moves onto its near one.
        for i in 0..chains.len() {
            if !chains[i].alive || chains[i].a == chains[i].b {
                continue;
            }
            if length(&chains[i].pts) >= MERGE_JUNCTION_M {
                continue;
            }
            let (keep, gone) = (chains[i].a.min(chains[i].b), chains[i].a.max(chains[i].b));
            let mid = (
                (nodes[keep].0 + nodes[gone].0) * 0.5,
                (nodes[keep].1 + nodes[gone].1) * 0.5,
            );
            nodes[keep] = mid;
            chains[i].alive = false;
            for c in chains.iter_mut().filter(|c| c.alive) {
                if c.a == gone {
                    c.a = keep;
                }
                if c.b == gone {
                    c.b = keep;
                }
                if c.a == keep {
                    c.pts[0] = mid;
                }
                if c.b == keep {
                    let n = c.pts.len();
                    c.pts[n - 1] = mid;
                }
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }
}

/// The direction a chain leaves node `n` by: toward its point some 60 m out.
fn leaving(c: &Chain, at_a: bool) -> P {
    let pts: Vec<P> = if at_a {
        c.pts.clone()
    } else {
        c.pts.iter().rev().copied().collect()
    };
    let mut s = 0.0;
    let mut q = pts[pts.len() - 1];
    for w in pts.windows(2) {
        s += dist(w[0], w[1]);
        if s >= 60.0 {
            q = w[1];
            break;
        }
    }
    let d = dist(pts[0], q).max(1e-3);
    ((q.0 - pts[0].0) / d, (q.1 - pts[0].1) / d)
}

/// Joins chains into roads that run straight on through junctions, names them and gives
/// each highway ramps to the arterials that cross it.
/// `drop_stubs`: a highway too short to be a freeway is left out (it ends on its own
/// skeleton's streets); else it is kept as a street (it joins the one network).
fn join(nodes: &[P], chains: &[Chain], drop_stubs: bool) -> Vec<Road> {
    // Ends: (chain, is the `a` end). Pair ends at each node, straightest first.
    let live: Vec<usize> = (0..chains.len()).filter(|&i| chains[i].alive).collect();
    let mut partner: Vec<[Option<(usize, bool)>; 2]> = vec![[None, None]; chains.len()];
    let cos_max = -(STRAIGHT_DEG.to_radians().cos());
    for n in 0..nodes.len() {
        let ends: Vec<(usize, bool)> = live
            .iter()
            .flat_map(|&i| {
                let mut v = Vec::new();
                if chains[i].a == n {
                    v.push((i, true));
                }
                if chains[i].b == n {
                    v.push((i, false));
                }
                v
            })
            .collect();
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for x in 0..ends.len() {
            for y in x + 1..ends.len() {
                let (ci, ai) = ends[x];
                let (cj, aj) = ends[y];
                if ci == cj
                    || chains[ci].highway != chains[cj].highway
                    || chains[ci].suspension != chains[cj].suspension
                {
                    continue;
                }
                let (u, v) = (leaving(&chains[ci], ai), leaving(&chains[cj], aj));
                let dot = u.0 * v.0 + u.1 * v.1;
                // Two ends alone at a node bending less than 60 degrees are a bend, not a
                // corner: one road, eased round it.
                if dot < cos_max || ends.len() == 2 && dot < -0.5 {
                    pairs.push((dot, x, y));
                }
            }
        }
        pairs.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.cmp(&q.1)).then(p.2.cmp(&q.2)));
        let mut used = vec![false; ends.len()];
        for (_, x, y) in pairs {
            if used[x] || used[y] {
                continue;
            }
            used[x] = true;
            used[y] = true;
            let (ci, ai) = ends[x];
            let (cj, aj) = ends[y];
            partner[ci][usize::from(!ai)] = Some((cj, aj));
            partner[cj][usize::from(!aj)] = Some((ci, ai));
        }
    }

    // Walk each road from an unpaired end (or anywhere on a closed loop).
    let mut done = vec![false; chains.len()];
    let mut lines: Vec<(bool, bool, Vec<P>)> = Vec::new();
    let order: Vec<usize> = live
        .iter()
        .copied()
        .filter(|&i| partner[i][0].is_none() || partner[i][1].is_none())
        .chain(live.iter().copied())
        .collect();
    for &i0 in &order {
        if done[i0] {
            continue;
        }
        // Start from the free end, if any: walk forward from `a` when `a` is unpaired.
        let mut forward = partner[i0][0].is_none() || partner[i0][1].is_some();
        let mut i = i0;
        let mut pts: Vec<P> = Vec::new();
        loop {
            done[i] = true;
            let c = &chains[i];
            let seg: Vec<P> = if forward {
                c.pts.clone()
            } else {
                c.pts.iter().rev().copied().collect()
            };
            if pts.is_empty() {
                pts.extend(seg);
            } else {
                pts.extend_from_slice(&seg[1..]);
            }
            // Leave through the far end: `b` when walking forward.
            let Some((j, at_a)) = partner[i][usize::from(forward)] else {
                break;
            };
            if done[j] {
                break;
            }
            i = j;
            forward = at_a;
        }
        let pts: Vec<P> = ease(
            &simplify(
                &chaikin(&chaikin(&average(&average(&average(&pts))))),
                TOLERANCE_M,
            ),
            &[],
        )
        .into_iter()
        .map(|(x, z)| ((x * 10.0).round() / 10.0, (z * 10.0).round() / 10.0))
        .collect();
        lines.push((chains[i0].highway, chains[i0].suspension, pts));
    }
    // A highway stub (a few hundred meters between two street corners) is no freeway.
    if drop_stubs {
        lines.retain(|l| !l.0 || length(&l.2) >= MIN_HIGHWAY_M);
    } else {
        for l in lines.iter_mut() {
            l.0 &= length(&l.2) >= MIN_HIGHWAY_M;
        }
    }
    lines.sort_by(|a, b| length(&b.2).total_cmp(&length(&a.2)));

    let (mut nh, mut na) = (0, 0);
    let mut roads: Vec<Road> = lines
        .into_iter()
        .map(|(highway, suspension, points)| {
            let name = if highway {
                nh += 1;
                format!("freeway {nh}")
            } else {
                na += 1;
                format!("road {na}")
            };
            Road {
                name,
                highway,
                points,
                ramps: Vec::new(),
                suspension,
            }
        })
        .collect();

    snap_ends(&mut roads);

    // Ramps to the arterials that cross each highway, well clear of its ends and each other.
    for h in 0..roads.len() {
        if !roads[h].highway {
            continue;
        }
        let hl: Vec<world::math::V2> = roads[h]
            .points
            .iter()
            .map(|&(x, z)| world::math::V2::new(x, z))
            .collect();
        let len = world::layout::line_length(&hl);
        let mut at: Vec<f32> = Vec::new();
        let mut ramps = Vec::new();
        for r in &roads {
            if r.highway {
                continue;
            }
            let rl: Vec<world::math::V2> = r
                .points
                .iter()
                .map(|&(x, z)| world::math::V2::new(x, z))
                .collect();
            let Some((s, sr, _)) = world::layout::crossing(&hl, &rl) else {
                continue;
            };
            // A road that only ends on the highway joins it at grade.
            let rlen = world::layout::line_length(&rl);
            if sr < 60.0 || sr > rlen - 60.0 {
                continue;
            }
            if s < RAMP_CLEAR_M || s > len - RAMP_CLEAR_M {
                continue;
            }
            if at.iter().any(|&t| (t - s).abs() < RAMP_SPACING_M) {
                continue;
            }
            at.push(s);
            ramps.push(r.name.clone());
        }
        roads[h].ramps = ramps;
    }
    roads
}

/// Moves each road end onto the nearest road it meets (as `world` snaps it, within
/// `SNAP_M`) and makes that a point of both, so `world` joins them there rather than adding
/// a kinked stub. Then every road runs straight for `STRAIGHT_IN_M` into each joint: a bend
/// inside a junction's trim puts the stop line off its painted bar.
fn snap_ends(roads: &mut [Road]) {
    let round = |q: P| ((q.0 * 10.0).round() / 10.0, (q.1 * 10.0).round() / 10.0);
    let mut joints: Vec<P> = Vec::new();
    for i in 0..roads.len() {
        for end in [0, roads[i].points.len() - 1] {
            let p = roads[i].points[end];
            // (distance, road, insert-before index or usize::MAX for an existing point, point)
            let mut best: Option<(f32, usize, usize, P)> = None;
            for (j, r) in roads.iter().enumerate() {
                if j == i {
                    continue;
                }
                for (k, s) in r.points.windows(2).enumerate() {
                    let (a, b) = (s[0], s[1]);
                    let ab = (b.0 - a.0, b.1 - a.1);
                    let l2 = ab.0 * ab.0 + ab.1 * ab.1;
                    let t = if l2 > 0.0 {
                        (((p.0 - a.0) * ab.0 + (p.1 - a.1) * ab.1) / l2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let mut q = (round((a.0 + ab.0 * t, a.1 + ab.1 * t)), k + 1);
                    // Near a bend, join at the bend: no kink inside the junction.
                    for (v, at) in [(a, k), (b, k + 1)] {
                        if dist(q.0, v) < JOIN_AT_BEND_M {
                            q = (v, at + usize::MAX / 2);
                        }
                    }
                    let d = dist(p, q.0);
                    if d <= world::layout::SNAP_M && best.is_none_or(|b| d < b.0) {
                        best = Some((d, j, q.1, q.0));
                    }
                }
            }
            if let Some((_, j, at, q)) = best {
                roads[i].points[end] = q;
                if at < usize::MAX / 2 && !roads[j].points.contains(&q) {
                    roads[j].points.insert(at, q);
                }
                joints.push(q);
            }
        }
    }
    // Where two streets cross, both get the crossing as a point.
    for i in 0..roads.len() {
        for j in i + 1..roads.len() {
            if roads[i].highway || roads[j].highway {
                continue;
            }
            let mut k = 0;
            while k + 1 < roads[i].points.len() {
                let (a, b) = (roads[i].points[k], roads[i].points[k + 1]);
                let mut m = 0;
                while m + 1 < roads[j].points.len() {
                    let (c, d) = (roads[j].points[m], roads[j].points[m + 1]);
                    if let Some(x) = cross(a, b, c, d) {
                        let x = round(x);
                        if [a, b].iter().all(|&v| dist(v, x) > 1.0) {
                            roads[i].points.insert(k + 1, x);
                        }
                        if [c, d].iter().all(|&v| dist(v, x) > 1.0) {
                            roads[j].points.insert(m + 1, x);
                        }
                        joints.push(x);
                        break;
                    }
                    m += 1;
                }
                k += 1;
            }
        }
    }
    for r in roads.iter_mut() {
        let n = r.points.len();
        let keep: Vec<bool> = (0..n)
            .map(|k| {
                let v = r.points[k];
                k == 0
                    || k == n - 1
                    || joints.contains(&v)
                    || !joints.iter().any(|&j| dist(v, j) < STRAIGHT_IN_M)
            })
            .collect();
        let mut k = 0;
        r.points.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        r.points = ease(&r.points, &joints).into_iter().map(round).collect();
    }
}

/// Where segment ab properly crosses cd, strictly inside both.
fn cross(a: P, b: P, c: P, d: P) -> Option<P> {
    let r = (b.0 - a.0, b.1 - a.1);
    let s = (d.0 - c.0, d.1 - c.1);
    let den = r.0 * s.1 - r.1 * s.0;
    if den.abs() < 1e-6 {
        return None;
    }
    let t = ((c.0 - a.0) * s.1 - (c.1 - a.1) * s.0) / den;
    let u = ((c.0 - a.0) * r.1 - (c.1 - a.1) * r.0) / den;
    (t > 1e-4 && t < 1.0 - 1e-4 && u > 1e-4 && u < 1.0 - 1e-4)
        .then_some((a.0 + r.0 * t, a.1 + r.1 * t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_rounds_a_square_corner_into_gentle_bends() {
        let l = ease(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)], &[]);
        assert_eq!((l[0], l[l.len() - 1]), ((0.0, 0.0), (100.0, 100.0)));
        for w in l.windows(3) {
            let h = |a: P, b: P| (b.1 - a.1).atan2(b.0 - a.0);
            let d = (h(w[1], w[2]) - h(w[0], w[1])).abs();
            assert!(d <= MAX_BEND_DEG.to_radians() + 1e-4, "{d}");
        }
    }

    #[test]
    fn simplify_keeps_ends_and_corners() {
        let l = [(0.0, 0.0), (5.0, 0.5), (10.0, 0.0), (10.0, 10.0)];
        assert_eq!(
            simplify(&l, 1.0),
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]
        );
    }
}
