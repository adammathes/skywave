//! Lane centerlines in driving direction, for putting a car back on the road.
//!
//! A place's `Lanes` table is used when it has one; otherwise each edge's lanes are laid
//! out from its cross section the way [`crate::roads`] paints them: two-way roads drive on
//! the right with their lanes from the centerline (the highway's from its median) out, a
//! one-way road spreads its lanes across the whole width.

use alloc::vec::Vec;

use place::{Edge, Place, RoadClass};

use crate::math::{sqrt, V2};
use crate::roads::{GUTTER_M, HIGHWAY_LANE_M, MEDIAN_HALF_M};

/// Every lane's centerline: `points[ends[i - 1]..ends[i]]` is lane `i`, in its direction of
/// travel, `[x, y, z]` with `y` the road surface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaneLines {
    pub points: Vec<[f32; 3]>,
    pub ends: Vec<u32>,
}

/// The point of a lane nearest a query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneHit {
    /// On the centerline, `[x, y, z]`.
    pub pos: [f32; 3],
    /// Unit direction of travel there, `[x, z]`.
    pub dir: [f32; 2],
    /// Horizontal distance from the query, m.
    pub dist: f32,
}

impl LaneLines {
    /// The lanes of `place`.
    pub fn of(place: &Place) -> LaneLines {
        let mut out = LaneLines::default();
        if !place.lanes.lanes.is_empty() {
            for lane in &place.lanes.lanes {
                out.points.extend_from_slice(&lane.centerline);
                out.ends.push(out.points.len() as u32);
            }
            return out;
        }
        for e in &place.roads.edges {
            if e.profile.len() < 2 {
                continue;
            }
            for (forward, offset) in lane_offsets(e) {
                out.push_offset(&e.profile, forward, offset);
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Lane `i`'s centerline.
    pub fn lane(&self, i: usize) -> &[[f32; 3]] {
        let start = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        &self.points[start..self.ends[i] as usize]
    }

    /// The `profile` polyline moved `offset` m to the right of its direction of travel
    /// (reversed first when not `forward`).
    fn push_offset(&mut self, profile: &[[f32; 3]], forward: bool, offset: f32) {
        let n = profile.len();
        let at = |i: usize| {
            if forward {
                profile[i]
            } else {
                profile[n - 1 - i]
            }
        };
        for i in 0..n {
            let (a, b) = (at(i.saturating_sub(1)), at((i + 1).min(n - 1)));
            let dir = V2::new(b[0] - a[0], b[2] - a[2]).normalize();
            let r = dir.right();
            let p = at(i);
            self.points
                .push([p[0] + r.x * offset, p[1], p[2] + r.y * offset]);
        }
        self.ends.push(self.points.len() as u32);
    }

    /// The nearest point to `[x, z]` on any lane no farther than `max_m`, with its lane's
    /// direction of travel; the first lane wins a tie.
    pub fn nearest(&self, p: [f32; 2], max_m: f32) -> Option<LaneHit> {
        let q = V2::from_array(p);
        let mut best: Option<(f32, LaneHit)> = None;
        for i in 0..self.len() {
            for w in self.lane(i).windows(2) {
                let (a, b) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
                let ab = b - a;
                let len2 = ab.dot(ab);
                if len2 <= 0.0 {
                    continue;
                }
                let t = ((q - a).dot(ab) / len2).clamp(0.0, 1.0);
                let c = a + ab * t;
                let d = q - c;
                let d2 = d.dot(d);
                if d2 > max_m * max_m || best.is_some_and(|(b2, _)| d2 >= b2) {
                    continue;
                }
                let y = w[0][1] + (w[1][1] - w[0][1]) * t;
                let dir = ab * (1.0 / sqrt(len2));
                let hit = LaneHit {
                    pos: [c.x, y, c.y],
                    dir: [dir.x, dir.y],
                    dist: sqrt(d2),
                };
                best = Some((d2, hit));
            }
        }
        best.map(|(_, h)| h)
    }
}

/// `(forward, offset to the right of travel)` of each lane of `e`.
fn lane_offsets(e: &Edge) -> Vec<(bool, f32)> {
    let inner = (0.5 * e.width_m - GUTTER_M).max(0.5);
    let mut out = Vec::new();
    if e.oneway {
        let n = e.lanes_fwd.max(1);
        let w = 2.0 * inner / f32::from(n);
        for k in 0..n {
            out.push((true, -inner + (f32::from(k) + 0.5) * w));
        }
        return out;
    }
    for (forward, n) in [(true, e.lanes_fwd), (false, e.lanes_back)] {
        let n = n.max(1);
        for k in 0..n {
            let k = f32::from(k) + 0.5;
            let offset = if e.class == RoadClass::Highway {
                MEDIAN_HALF_M + HIGHWAY_LANE_M * k
            } else {
                inner / f32::from(n) * k
            };
            out.push((forward, offset));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_towns_start_is_on_a_lane_heading_east() {
        let place = Place::test_grid();
        let lanes = LaneLines::of(&place);
        assert!(!lanes.is_empty());
        let start = place.player_start().unwrap();
        let hit = lanes.nearest([start.pos[0], start.pos[2]], 200.0).unwrap();
        // The spawn is in the eastbound outer lane of the strip, facing east.
        assert!(hit.dist < 0.1, "{hit:?}");
        assert!(hit.dir[0] > 0.99, "{hit:?}");
    }

    #[test]
    fn nearest_picks_the_lane_on_your_side_and_respects_the_limit() {
        let lanes = LaneLines::of(&Place::test_grid());
        let start = Place::test_grid().player_start().unwrap().pos;
        // Just north of the strip's centerline: the westbound lanes.
        let hit = lanes.nearest([start[0] + 10.0, -2.0], 200.0).unwrap();
        assert!(hit.dir[0] < -0.99 && hit.pos[2] < 0.0, "{hit:?}");
        assert_eq!(lanes.nearest([1.0e5, 1.0e5], 200.0), None);
    }

    #[test]
    fn one_way_lanes_spread_across_the_road() {
        let e = Edge {
            a: 0,
            b: 1,
            class: RoadClass::Highway,
            lanes_fwd: 2,
            lanes_back: 0,
            width_m: 9.0,
            sidewalk: false,
            speed_kph: 90,
            oneway: true,
            profile: alloc::vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0]],
        };
        let offsets = lane_offsets(&e);
        assert_eq!(offsets.len(), 2);
        assert!((offsets[0].1 + offsets[1].1).abs() < 1e-6);
        let mut lanes = LaneLines::default();
        lanes.push_offset(&e.profile, false, 2.0);
        // Driving west (reversed), the right-hand side is north (-z).
        assert_eq!(lanes.lane(0), &[[100.0, 0.0, -2.0], [0.0, 0.0, -2.0]]);
    }
}
