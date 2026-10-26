//! `place::validate`: the invariants of `docs/PLACES.md`, "Validation", checked on load and
//! in tests. Each failure has its own [`PlaceError`] variant naming the offending item.
//!
//! In order: header (version, chunk size, place size, geo origin), terrain and zone grid
//! sizes, finite numbers, water polygons; road edges (indices, lanes, width, speed,
//! profile from `a` to `b` within ±8 m of the terrain), graph connectivity (one component
//! holds ≥ 95% of the edges); parcels (simple counter-clockwise polygons touching their
//! frontage edge's property line); buildings (simple counter-clockwise footprints, height,
//! floors, ≤ 5% overlap with any other building of the same chunk); at least one
//! `PlayerStart`; every object inside the place and every table sorted by chunk; detail
//! references and shapes. Per-chunk hashes are checked when decoding ([`crate::codec`]).
//!
//! All arithmetic is plain `f32`/`f64` in a fixed order, so a verdict is the same on every
//! platform.

use alloc::vec;
use alloc::vec::Vec;

use crate::{
    Place, PlaceError, SpawnKind, Table, Terrain, Zones, MAX_SIZE_CHUNKS, SIDEWALK_WIDTH_M,
};

/// How far an edge profile sample may be above or below the terrain, meters.
pub const PROFILE_TERRAIN_TOLERANCE_M: f32 = 8.0;

/// How far a profile's ends may be from its edge's nodes, meters.
pub const PROFILE_END_TOLERANCE_M: f32 = 0.05;

/// Slack beyond the property line (half the road width plus the sidewalk) within which a
/// parcel counts as touching its frontage edge, meters.
pub const FRONTAGE_TOLERANCE_M: f32 = 1.0;

/// Largest share of the smaller footprint two buildings may have in common.
pub const MAX_BUILDING_OVERLAP: f64 = 0.05;

/// Share of the edges the largest connected component must hold, as `num / den`.
const CONNECTED_SHARE: (usize, usize) = (19, 20);

pub fn validate(place: &Place) -> Result<(), PlaceError> {
    header(place)?;
    grids(place)?;
    roads(place)?;
    parcels(place)?;
    buildings(place)?;
    if !place
        .spawns
        .iter()
        .any(|s| s.kind == SpawnKind::PlayerStart)
    {
        return Err(PlaceError::NoPlayerStart);
    }
    chunking(place)?;
    detail(place)
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|c| c.is_finite())
}

fn header(place: &Place) -> Result<(), PlaceError> {
    let h = &place.header;
    if h.version != crate::PLACE_VERSION {
        return Err(PlaceError::Version);
    }
    if !(h.chunk_size_m.is_finite() && h.chunk_size_m > 0.0) {
        return Err(PlaceError::ChunkSize);
    }
    let fits = |n: u16, o: i16| (1..=MAX_SIZE_CHUNKS).contains(&n) && i32::from(o) + 256 >= 0;
    if !(fits(h.size_chunks[0], h.chunk_origin[0]) && fits(h.size_chunks[1], h.chunk_origin[1])) {
        return Err(PlaceError::PlaceSize);
    }
    if let Some([lat, lon]) = h.origin_geo {
        if !((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
            return Err(PlaceError::OriginGeo);
        }
    }
    Ok(())
}

fn grids(place: &Place) -> Result<(), PlaceError> {
    let (h, t) = (&place.header, &place.terrain);
    let [w, d] = Terrain::samples(h, t.res_per_chunk);
    if t.res_per_chunk == 0 || t.heights.len() != w * d {
        return Err(PlaceError::TerrainSize);
    }
    let [zw, zd] = Zones::cells_per_side(h, t.res_per_chunk);
    if place.zones.cells.len() != zw * zd {
        return Err(PlaceError::ZoneSize);
    }
    let nodes_finite = place
        .roads
        .nodes
        .iter()
        .all(|n| finite(&n.pos) && n.elevation.is_finite());
    let points_finite = place.pois.iter().all(|p| finite(&p.pos))
        && place
            .spawns
            .iter()
            .all(|s| finite(&s.pos) && s.heading.is_finite())
        && place
            .props
            .items
            .iter()
            .all(|p| finite(&p.pos) && p.yaw.is_finite())
        && place
            .lights
            .items
            .iter()
            .all(|l| finite(&l.pos) && finite(&l.color));
    if !(finite(&t.heights) && t.water_level.is_finite() && nodes_finite && points_finite) {
        return Err(PlaceError::NonFinite);
    }
    for (i, wb) in t.water.iter().enumerate() {
        if !is_simple_ccw(&wb.polygon) {
            return Err(PlaceError::Water { body: i as u32 });
        }
    }
    Ok(())
}

fn roads(place: &Place) -> Result<(), PlaceError> {
    let nodes = &place.roads.nodes;
    let n = nodes.len();
    for (i, e) in place.roads.edges.iter().enumerate() {
        let edge = i as u32;
        if e.a as usize >= n || e.b as usize >= n {
            return Err(PlaceError::EdgeIndex { edge });
        }
        if e.a == e.b {
            return Err(PlaceError::DegenerateEdge { edge });
        }
        if e.lanes_fwd == 0 && e.lanes_back == 0 {
            return Err(PlaceError::NoLanes { edge });
        }
        if e.oneway && e.lanes_back != 0 {
            return Err(PlaceError::OneWay { edge });
        }
        if !(e.width_m.is_finite() && e.width_m > 0.0) {
            return Err(PlaceError::RoadWidth { edge });
        }
        if e.speed_kph == 0 {
            return Err(PlaceError::Speed { edge });
        }
        let ends = (e.profile.first(), e.profile.last());
        let (Some(first), Some(last)) = ends else {
            return Err(PlaceError::Profile { edge });
        };
        let near = |p: &[f32; 3], node: usize| {
            let q = nodes[node].pos;
            let tol = PROFILE_END_TOLERANCE_M;
            dist2([p[0], p[2]], q) <= tol * tol
        };
        let shaped = e.profile.len() >= 2 && e.profile.iter().all(|p| finite(p));
        if !(shaped && near(first, e.a as usize) && near(last, e.b as usize)) {
            return Err(PlaceError::Profile { edge });
        }
        for p in &e.profile {
            let ground = place.terrain.height_at(&place.header, [p[0], p[2]]);
            match ground {
                Some(g) if abs(p[1] - g) <= PROFILE_TERRAIN_TOLERANCE_M => {}
                _ => return Err(PlaceError::ProfileOffTerrain { edge }),
            }
        }
    }
    connected(place)
}

/// One component (by union-find over nodes) holds `CONNECTED_SHARE` of the edges.
fn connected(place: &Place) -> Result<(), PlaceError> {
    let edges = &place.roads.edges;
    if edges.is_empty() {
        return Ok(());
    }
    let mut parent: Vec<u32> = (0..place.roads.nodes.len() as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            let up = parent[parent[i as usize] as usize];
            parent[i as usize] = up;
            i = up;
        }
        i
    }
    for e in edges {
        let (ra, rb) = (root(&mut parent, e.a), root(&mut parent, e.b));
        if ra != rb {
            parent[ra.max(rb) as usize] = ra.min(rb);
        }
    }
    let mut count = vec![0usize; parent.len()];
    for e in edges {
        count[root(&mut parent, e.a) as usize] += 1;
    }
    let largest = count.iter().copied().max().unwrap_or(0);
    if largest * CONNECTED_SHARE.1 >= edges.len() * CONNECTED_SHARE.0 {
        Ok(())
    } else {
        Err(PlaceError::Disconnected)
    }
}

fn parcels(place: &Place) -> Result<(), PlaceError> {
    for (i, p) in place.parcels.iter().enumerate() {
        let parcel = i as u32;
        if !is_simple_ccw(&p.polygon) {
            return Err(PlaceError::Parcel { parcel });
        }
        let Some(e) = place.roads.edges.get(p.frontage_edge as usize) else {
            return Err(PlaceError::FrontageEdge { parcel });
        };
        let walk = if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
        let reach = e.width_m * 0.5 + walk + FRONTAGE_TOLERANCE_M;
        let line: Vec<[f32; 2]> = e.profile.iter().map(|q| [q[0], q[2]]).collect();
        if polyline_polygon_dist2(&line, &p.polygon) > f64::from(reach) * f64::from(reach) {
            return Err(PlaceError::FrontageGap { parcel });
        }
    }
    Ok(())
}

fn buildings(place: &Place) -> Result<(), PlaceError> {
    for (i, b) in place.buildings.iter().enumerate() {
        let building = i as u32;
        if !is_simple_ccw(&b.footprint) {
            return Err(PlaceError::Footprint { building });
        }
        if !(b.height_m.is_finite() && b.height_m > 0.0) {
            return Err(PlaceError::BuildingHeight { building });
        }
        if b.floors == 0 {
            return Err(PlaceError::Floors { building });
        }
    }
    // Pairwise within each chunk's run (the table is sorted by chunk; an unsorted table
    // fails `chunking` later, so the runs here only need to be maximal).
    let keys: Vec<u32> = (0..place.buildings.len())
        .map(|i| place.chunk_key(Table::Buildings, i))
        .collect();
    let mut start = 0;
    while start < keys.len() {
        let mut end = start + 1;
        while end < keys.len() && keys[end] == keys[start] {
            end += 1;
        }
        for i in start..end {
            for j in i + 1..end {
                let (a, b) = (&place.buildings[i].footprint, &place.buildings[j].footprint);
                let smaller = f64::from(crate::signed_area(a).min(crate::signed_area(b)));
                if overlap_area(a, b) > smaller * MAX_BUILDING_OVERLAP {
                    return Err(PlaceError::BuildingOverlap {
                        a: i as u32,
                        b: j as u32,
                    });
                }
            }
        }
        start = end;
    }
    Ok(())
}

fn chunking(place: &Place) -> Result<(), PlaceError> {
    for table in Table::ALL {
        let mut prev = 0;
        for item in 0..place.table_len(table) {
            if dangling(place, table, item) {
                return Err(PlaceError::DetailIndex {
                    table,
                    item: item as u32,
                });
            }
            let key = place.chunk_key(table, item);
            if key == u32::MAX {
                return Err(PlaceError::OutOfBounds {
                    table,
                    item: item as u32,
                });
            }
            if key < prev {
                return Err(PlaceError::Unsorted { table });
            }
            prev = key;
        }
    }
    Ok(())
}

/// Whether a detail record refers to something that does not exist.
fn dangling(place: &Place, table: Table, item: usize) -> bool {
    let (nodes, edges) = (place.roads.nodes.len(), place.roads.edges.len());
    let lanes = place.lanes.lanes.len();
    let has = |i: u32, n: usize| (i as usize) < n;
    match table {
        Table::Lanes => !has(place.lanes.lanes[item].edge, edges),
        Table::StopLines => {
            let s = &place.lanes.stop_lines[item];
            !(has(s.edge, edges) && has(s.node, nodes))
        }
        Table::Turns => {
            let t = &place.lanes.turns[item];
            !(has(t.node, nodes) && has(t.from, lanes) && has(t.to, lanes))
        }
        Table::Crossings => !has(place.sidewalks.crossings[item].node, nodes),
        Table::Facades => {
            let f = &place.facades.items[item];
            match place.buildings.get(f.building as usize) {
                Some(b) => usize::from(f.side) >= b.footprint.len(),
                None => true,
            }
        }
        _ => false,
    }
}

fn detail(place: &Place) -> Result<(), PlaceError> {
    let bad = |table: Table, item: usize| PlaceError::DetailGeometry {
        table,
        item: item as u32,
    };
    let line_ok = |pts: &[[f32; 3]]| pts.len() >= 2 && pts.iter().all(|p| finite(p));
    for (i, l) in place.lanes.lanes.iter().enumerate() {
        if !line_ok(&l.centerline) {
            return Err(bad(Table::Lanes, i));
        }
    }
    for (i, s) in place.lanes.stop_lines.iter().enumerate() {
        if !line_ok(&[s.a, s.b]) {
            return Err(bad(Table::StopLines, i));
        }
    }
    for (i, s) in place.sidewalks.walks.iter().enumerate() {
        if !line_ok(&s.polyline) {
            return Err(bad(Table::Sidewalks, i));
        }
    }
    for (i, c) in place.sidewalks.crossings.iter().enumerate() {
        if !line_ok(&[c.a, c.b]) {
            return Err(bad(Table::Crossings, i));
        }
    }
    for (i, f) in place.facades.items.iter().enumerate() {
        let windows = usize::from(f.cols) * usize::from(f.rows);
        if f.lit.len() != windows.div_ceil(8) {
            return Err(bad(Table::Facades, i));
        }
    }
    for (i, l) in place.lights.items.iter().enumerate() {
        if !(l.radius.is_finite() && l.radius > 0.0) {
            return Err(bad(Table::Lights, i));
        }
    }
    Ok(())
}

fn abs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dz) = (a[0] - b[0], a[1] - b[1]);
    dx * dx + dz * dz
}

type P = [f64; 2];

fn p64(p: [f32; 2]) -> P {
    [f64::from(p[0]), f64::from(p[1])]
}

/// Cross product of `b - a` and `c - a`: positive when `a, b, c` turn counter-clockwise
/// seen from +Y (the same sign as [`crate::signed_area`]).
fn orient(a: P, b: P, c: P) -> f64 {
    (b[1] - a[1]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[1] - a[1])
}

/// Whether the closed segments `p1 p2` and `q1 q2` share a point.
fn segments_touch(p1: P, p2: P, q1: P, q2: P) -> bool {
    let (d1, d2) = (orient(q1, q2, p1), orient(q1, q2, p2));
    let (d3, d4) = (orient(p1, p2, q1), orient(p1, p2, q2));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    let on = |a: P, b: P, c: P, d: f64| {
        d == 0.0
            && c[0] >= a[0].min(b[0])
            && c[0] <= a[0].max(b[0])
            && c[1] >= a[1].min(b[1])
            && c[1] <= a[1].max(b[1])
    };
    on(q1, q2, p1, d1) || on(q1, q2, p2, d2) || on(p1, p2, q1, d3) || on(p1, p2, q2, d4)
}

/// At least three finite vertices, positive (counter-clockwise) area, no zero-length edge
/// and no two non-adjacent edges touching.
pub fn is_simple_ccw(poly: &[[f32; 2]]) -> bool {
    let n = poly.len();
    if n < 3 || !poly.iter().all(|p| finite(p)) || crate::signed_area(poly) <= 0.0 {
        return false;
    }
    let v: Vec<P> = poly.iter().map(|&p| p64(p)).collect();
    for i in 0..n {
        if v[i] == v[(i + 1) % n] {
            return false;
        }
    }
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue; // Adjacent through the closing edge.
            }
            if segments_touch(v[i], v[(i + 1) % n], v[j], v[(j + 1) % n]) {
                return false;
            }
        }
    }
    true
}

/// Even-odd point-in-polygon test.
pub fn point_in_polygon(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Squared distance from point `p` to segment `a b`.
fn point_segment_dist2(p: P, a: P, b: P) -> f64 {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dz * dz;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dz) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ez) = (a[0] + dx * t - p[0], a[1] + dz * t - p[1]);
    ex * ex + ez * ez
}

/// Squared distance between a polyline and a polygon (0 when they touch or the line
/// starts inside).
fn polyline_polygon_dist2(line: &[[f32; 2]], poly: &[[f32; 2]]) -> f64 {
    if line.first().is_some_and(|&p| point_in_polygon(p, poly)) {
        return 0.0;
    }
    let mut best = f64::MAX;
    let n = poly.len();
    for w in line.windows(2) {
        let (a, b) = (p64(w[0]), p64(w[1]));
        for i in 0..n {
            let (c, d) = (p64(poly[i]), p64(poly[(i + 1) % n]));
            if segments_touch(a, b, c, d) {
                return 0.0;
            }
            let d2 = point_segment_dist2(a, c, d)
                .min(point_segment_dist2(b, c, d))
                .min(point_segment_dist2(c, a, b))
                .min(point_segment_dist2(d, a, b));
            best = best.min(d2);
        }
    }
    best
}

fn is_convex_ccw(v: &[P]) -> bool {
    let n = v.len();
    (0..n).all(|i| orient(v[i], v[(i + 1) % n], v[(i + 2) % n]) >= 0.0)
}

fn area64(v: &[P]) -> f64 {
    let mut acc = 0.0;
    for i in 0..v.len() {
        let (p, q) = (v[i], v[(i + 1) % v.len()]);
        acc += p[1] * q[0] - p[0] * q[1];
    }
    acc * 0.5
}

/// Sutherland-Hodgman: `subject` clipped to the convex counter-clockwise `clip`. The area
/// of the result is exact for any simple subject (concave pieces may join along
/// zero-width bridges, which add no area).
fn clip_convex(subject: &[P], clip: &[P]) -> Vec<P> {
    let mut out: Vec<P> = subject.to_vec();
    let n = clip.len();
    for i in 0..n {
        let (a, b) = (clip[i], clip[(i + 1) % n]);
        let input = core::mem::take(&mut out);
        let m = input.len();
        for k in 0..m {
            let (p, q) = (input[k], input[(k + 1) % m]);
            let (dp, dq) = (orient(a, b, p), orient(a, b, q));
            if dp >= 0.0 {
                out.push(p);
            }
            if (dp >= 0.0) != (dq >= 0.0) {
                let t = dp / (dp - dq);
                out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// Area common to two simple counter-clockwise polygons: exact when either is convex,
/// otherwise estimated on a 32 x 32 grid of sample points over their bounds' overlap.
pub fn overlap_area(a: &[[f32; 2]], b: &[[f32; 2]]) -> f64 {
    let bounds = |p: &[[f32; 2]]| {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for v in p {
            for k in 0..2 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        (lo, hi)
    };
    let ((alo, ahi), (blo, bhi)) = (bounds(a), bounds(b));
    let lo = [alo[0].max(blo[0]), alo[1].max(blo[1])];
    let hi = [ahi[0].min(bhi[0]), ahi[1].min(bhi[1])];
    if !(lo[0] < hi[0] && lo[1] < hi[1]) {
        return 0.0;
    }
    let va: Vec<P> = a.iter().map(|&p| p64(p)).collect();
    let vb: Vec<P> = b.iter().map(|&p| p64(p)).collect();
    if is_convex_ccw(&vb) {
        return area64(&clip_convex(&va, &vb));
    }
    if is_convex_ccw(&va) {
        return area64(&clip_convex(&vb, &va));
    }
    const N: u32 = 32;
    let (w, d) = (hi[0] - lo[0], hi[1] - lo[1]);
    let mut hits = 0u32;
    for j in 0..N {
        for i in 0..N {
            let p = [
                lo[0] + (i as f32 + 0.5) / N as f32 * w,
                lo[1] + (j as f32 + 0.5) / N as f32 * d,
            ];
            if point_in_polygon(p, a) && point_in_polygon(p, b) {
                hits += 1;
            }
        }
    }
    f64::from(w) * f64::from(d) * f64::from(hits) / f64::from(N * N)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use alloc::vec;

    fn square(x: f32, z: f32, s: f32) -> Vec<[f32; 2]> {
        vec![[x, z], [x, z + s], [x + s, z + s], [x + s, z]]
    }

    #[test]
    fn polygon_checks() {
        assert!(is_simple_ccw(&square(0.0, 0.0, 1.0)));
        let mut cw = square(0.0, 0.0, 1.0);
        cw.reverse();
        assert!(!is_simple_ccw(&cw));
        // A bow tie has zero area; one with a bigger lobe has positive area but crosses.
        let bow = vec![[0.0, 0.0], [0.0, 2.0], [3.0, 0.0], [3.0, 3.0]];
        assert!(crate::signed_area(&bow) != 0.0);
        assert!(!is_simple_ccw(&bow));
        let dup = vec![[0.0, 0.0], [0.0, 1.0], [0.0, 1.0], [1.0, 0.0]];
        assert!(!is_simple_ccw(&dup));
        assert!(point_in_polygon([0.5, 0.5], &square(0.0, 0.0, 1.0)));
        assert!(!point_in_polygon([1.5, 0.5], &square(0.0, 0.0, 1.0)));
    }

    #[test]
    fn overlap_areas() {
        let a = square(0.0, 0.0, 10.0);
        assert_eq!(overlap_area(&a, &square(5.0, 5.0, 10.0)), 25.0);
        assert_eq!(overlap_area(&a, &square(10.0, 0.0, 10.0)), 0.0);
        // An L (concave) against a square: exact by clipping the L to the square.
        let l = vec![
            [0.0, 0.0],
            [0.0, 10.0],
            [10.0, 10.0],
            [10.0, 5.0],
            [5.0, 5.0],
            [5.0, 0.0],
        ];
        assert!(is_simple_ccw(&l));
        assert_eq!(overlap_area(&l, &square(4.0, 4.0, 2.0)), 3.0);
        assert_eq!(overlap_area(&square(4.0, 4.0, 2.0), &l), 3.0);
        // Two Ls: sampled.
        let est = overlap_area(&l, &l);
        assert!((est - 75.0).abs() < 2.0, "{est}");
    }

    /// Each corruption class is rejected with its own error.
    #[test]
    fn validate_catches_broken_places() {
        let ok = Place::test_grid();
        assert_eq!(ok.validate(), Ok(()));
        let broken = |f: &dyn Fn(&mut Place)| {
            let mut p = ok.clone();
            f(&mut p);
            p.validate()
        };
        use PlaceError as E;
        assert_eq!(broken(&|p| p.header.version = 2), Err(E::Version));
        assert_eq!(broken(&|p| p.header.chunk_size_m = 0.0), Err(E::ChunkSize));
        assert_eq!(broken(&|p| p.header.size_chunks[0] = 0), Err(E::PlaceSize));
        assert_eq!(
            broken(&|p| p.header.origin_geo = Some([91.0, 0.0])),
            Err(E::OriginGeo)
        );
        assert_eq!(
            broken(&|p| {
                p.terrain.heights.pop();
            }),
            Err(E::TerrainSize)
        );
        assert_eq!(
            broken(&|p| {
                p.zones.cells.pop();
            }),
            Err(E::ZoneSize)
        );
        assert_eq!(
            broken(&|p| p.roads.nodes[0].pos[0] = f32::NAN),
            Err(E::NonFinite)
        );
        assert_eq!(
            broken(&|p| p.terrain.water.push(WaterBody {
                polygon: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                kind: WaterKind::Lake,
            })),
            Err(E::Water { body: 0 })
        );
        assert_eq!(
            broken(&|p| p.roads.edges[3].b = 999),
            Err(E::EdgeIndex { edge: 3 })
        );
        assert_eq!(
            broken(&|p| p.roads.edges[0].b = p.roads.edges[0].a),
            Err(E::DegenerateEdge { edge: 0 })
        );
        assert_eq!(
            broken(&|p| {
                p.roads.edges[1].lanes_fwd = 0;
                p.roads.edges[1].lanes_back = 0;
            }),
            Err(E::NoLanes { edge: 1 })
        );
        assert_eq!(
            broken(&|p| p.roads.edges[1].oneway = true),
            Err(E::OneWay { edge: 1 })
        );
        assert_eq!(
            broken(&|p| p.roads.edges[2].width_m = -1.0),
            Err(E::RoadWidth { edge: 2 })
        );
        assert_eq!(
            broken(&|p| p.roads.edges[2].speed_kph = 0),
            Err(E::Speed { edge: 2 })
        );
        assert_eq!(
            broken(&|p| p.roads.edges[4].profile.reverse()),
            Err(E::Profile { edge: 4 })
        );
        assert_eq!(
            broken(&|p| {
                p.roads.edges[4].profile.pop();
            }),
            Err(E::Profile { edge: 4 })
        );
        assert_eq!(
            broken(&|p| p.roads.edges[5].profile[0][1] = 8.5),
            Err(E::ProfileOffTerrain { edge: 5 })
        );
        assert_eq!(
            broken(&|p| p.roads.edges[5].profile[0][1] = 7.5),
            Ok(()),
            "within tolerance"
        );
        assert_eq!(
            broken(&|p| {
                // Cut the graph west of the north-south arterial.
                let nodes = p.roads.nodes.clone();
                let west = |n: u32| nodes[n as usize].pos[0] < 0.0;
                p.roads.edges.retain(|e| west(e.a) == west(e.b));
            }),
            Err(E::Disconnected)
        );
        assert_eq!(
            broken(&|p| p.parcels[4].polygon.reverse()),
            Err(E::Parcel { parcel: 4 })
        );
        assert_eq!(
            broken(&|p| p.parcels[5].frontage_edge = 10_000),
            Err(E::FrontageEdge { parcel: 5 })
        );
        assert_eq!(
            broken(&|p| {
                // A parcel facing a road on the far side of town.
                let far = p
                    .roads
                    .edges
                    .iter()
                    .position(|e| {
                        let a = p.roads.nodes[e.a as usize].pos;
                        let q = p.parcels[6].polygon[0];
                        (a[0] - q[0]).abs() + (a[1] - q[1]).abs() > 300.0
                    })
                    .unwrap();
                p.parcels[6].frontage_edge = far as u32;
            }),
            Err(E::FrontageGap { parcel: 6 })
        );
        assert_eq!(
            broken(&|p| p.buildings[2].footprint.reverse()),
            Err(E::Footprint { building: 2 })
        );
        assert_eq!(
            broken(&|p| p.buildings[0].height_m = 0.0),
            Err(E::BuildingHeight { building: 0 })
        );
        assert_eq!(
            broken(&|p| p.buildings[7].floors = 0),
            Err(E::Floors { building: 7 })
        );
        // Two buildings of one chunk: the second moved halfway onto the first.
        let (i, j) = (0..ok.buildings.len() - 1)
            .find(|&i| ok.chunk_key(Table::Buildings, i) == ok.chunk_key(Table::Buildings, i + 1))
            .map(|i| (i, i + 1))
            .unwrap();
        assert_eq!(
            broken(&|p| {
                let shift = |v: [f32; 2]| [v[0] + 0.5, v[1] + 0.5];
                p.buildings[j].footprint =
                    p.buildings[i].footprint.iter().map(|&v| shift(v)).collect();
            }),
            Err(E::BuildingOverlap {
                a: i as u32,
                b: j as u32
            })
        );
        assert_eq!(broken(&|p| p.spawns.clear()), Err(E::NoPlayerStart));
        assert_eq!(
            broken(&|p| p.spawns[0].pos[0] = 600.0),
            Err(E::OutOfBounds {
                table: Table::Spawns,
                item: 0
            })
        );
        assert_eq!(
            broken(&|p| {
                let last = p.parcels.len() - 1;
                p.parcels.swap(0, last);
            }),
            Err(E::Unsorted {
                table: Table::Parcels
            })
        );
        assert_eq!(
            broken(&|p| p.lanes.lanes.push(Lane {
                edge: 9999,
                forward: true,
                slot: 0,
                centerline: vec![[0.0; 3], [1.0, 0.0, 0.0]],
            })),
            Err(E::DetailIndex {
                table: Table::Lanes,
                item: 0
            })
        );
        assert_eq!(
            broken(&|p| p.lights.items.push(Light {
                pos: [0.0; 3],
                color: [1.0; 3],
                radius: 0.0,
                kind: LightKind::Street,
            })),
            Err(E::DetailGeometry {
                table: Table::Lights,
                item: 0
            })
        );
    }
}
