//! The map audit: reads a place's road graph for the things a real street map never has,
//! and says where each one is.
//!
//! - **Unreachable**: a road you cannot drive to from the main network.
//! - **Loop**: a road that leaves a junction and comes back to it having met nothing (a
//!   teardrop), or one whose two ends are the same node.
//! - **Dead end**: a road that ends at nothing, in a bulb or without one. The caller says
//!   where one may be (a cul-de-sac in a suburb, a pier road, an authored lookout); a road
//!   that runs off the edge of the place is not one.
//! - **Stub**: a dead end shorter than [`STUB_M`], wherever it is: too short to be a street.
//! - **Unjoined**: a dead end whose tip lies on another road at the same height without
//!   sharing a node with it: it looks like a junction and nothing can turn there.
//!
//! `crates/app/tests/maps.rs` runs it over every built-in world.

use alloc::vec;
use alloc::vec::Vec;

use place::{Place, RoadClass};

use crate::math::{segment_distance, V2};

/// A dead end shorter than this (from its last junction, bulb included) is a stub.
pub const STUB_M: f32 = 40.0;
/// A road ending within this of the place's edge runs off the map: not a dead end.
pub const EDGE_M: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Unreachable,
    Loop,
    DeadEnd,
    Stub,
    Unjoined,
}

/// A tip this near another road's height is at grade with it.
const GRADE_M: f32 = 2.0;

/// One thing wrong with the map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Finding {
    pub fault: Fault,
    /// Where: the dead end's tip, the loop's junction, a node of the unreachable roads.
    pub at: [f32; 2],
    pub class: RoadClass,
    /// Length of the road in question, m.
    pub len_m: f32,
}

fn length(profile: &[[f32; 3]]) -> f32 {
    profile
        .windows(2)
        .map(|w| (V2::new(w[1][0], w[1][2]) - V2::new(w[0][0], w[0][2])).length())
        .sum()
}

/// Everything wrong with `place`'s roads, in node order. `dead_end_ok(p)` says whether a
/// road may end at `p` (and a loop hang there).
pub fn roads(place: &Place, dead_end_ok: impl Fn([f32; 2]) -> bool) -> Vec<Finding> {
    let g = &place.roads;
    let n = g.nodes.len();
    let lens: Vec<f32> = g.edges.iter().map(|e| length(&e.profile)).collect();
    // Edges at each node (a self-loop once).
    let mut at: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (k, e) in g.edges.iter().enumerate() {
        at[e.a as usize].push(k as u32);
        if e.b != e.a {
            at[e.b as usize].push(k as u32);
        }
    }
    let other = |k: u32, node: u32| {
        let e = &g.edges[k as usize];
        if e.a == node {
            e.b
        } else {
            e.a
        }
    };
    let mut out = Vec::new();

    // Unreachable: every component but the longest.
    let mut comp = vec![u32::MAX; n];
    let mut comp_len: Vec<f32> = Vec::new();
    for start in 0..n {
        if comp[start] != u32::MAX || at[start].is_empty() {
            continue;
        }
        let c = comp_len.len() as u32;
        let (mut stack, mut total) = (vec![start as u32], 0.0);
        comp[start] = c;
        while let Some(u) = stack.pop() {
            for &k in &at[u as usize] {
                // Each edge counted from its `a` end.
                if g.edges[k as usize].a == u {
                    total += lens[k as usize];
                }
                let v = other(k, u);
                if comp[v as usize] == u32::MAX {
                    comp[v as usize] = c;
                    stack.push(v);
                }
            }
        }
        comp_len.push(total);
    }
    let main = (0..comp_len.len())
        .max_by(|&a, &b| comp_len[a].total_cmp(&comp_len[b]))
        .unwrap_or(0) as u32;
    let mut seen = vec![false; comp_len.len()];
    for u in 0..n {
        let c = comp[u];
        if c == u32::MAX || c == main || seen[c as usize] {
            continue;
        }
        seen[c as usize] = true;
        out.push(Finding {
            fault: Fault::Unreachable,
            at: g.nodes[u].pos,
            class: g.edges[at[u][0] as usize].class,
            len_m: comp_len[c as usize],
        });
    }

    let o = place.header.origin_m();
    let size = [
        f32::from(place.header.size_chunks[0]) * place.header.chunk_size_m,
        f32::from(place.header.size_chunks[1]) * place.header.chunk_size_m,
    ];
    let off_the_map = |p: [f32; 2]| {
        p[0] < o[0] + EDGE_M
            || p[1] < o[1] + EDGE_M
            || p[0] > o[0] + size[0] - EDGE_M
            || p[1] > o[1] + size[1] - EDGE_M
    };
    // A bulb: two edges between the same two nodes, the far one having nothing else. Its
    // far node is the tip of a dead end; the bulb's own length is half its two edges.
    let bulb_tip = |u: usize| -> bool {
        at[u].len() == 2 && at[u][0] != at[u][1] && {
            let (j, k) = (at[u][0], at[u][1]);
            let v = other(j, u as u32);
            v != u as u32 && other(k, u as u32) == v
        }
    };
    for u in 0..n {
        // Self-loops.
        for &k in &at[u] {
            let e = &g.edges[k as usize];
            if e.a == e.b && !dead_end_ok(g.nodes[u].pos) {
                out.push(Finding {
                    fault: Fault::Loop,
                    at: g.nodes[u].pos,
                    class: e.class,
                    len_m: lens[k as usize],
                });
            }
        }
        let tip = bulb_tip(u);
        if at[u].len() == 1 || tip {
            // A dead end: walk back to the last junction.
            let first = at[u][0];
            if g.edges[first as usize].a == g.edges[first as usize].b {
                continue;
            }
            let (mut node, mut via, mut len) =
                (other(first, u as u32), first, lens[first as usize]);
            if tip {
                len = 0.5 * (lens[at[u][0] as usize] + lens[at[u][1] as usize]);
                // Off the bulb's near node by its one other edge.
                let near = node as usize;
                let Some(&next) = at[near].iter().find(|&&k| k != at[u][0] && k != at[u][1]) else {
                    continue;
                };
                if at[near].len() != 3 {
                    // A bulb on a junction: the junction is where it hangs.
                    node = near as u32;
                    via = u32::MAX;
                } else {
                    len += lens[next as usize];
                    node = other(next, near as u32);
                    via = next;
                }
            }
            let mut steps = 0;
            while via != u32::MAX && at[node as usize].len() == 2 && steps < g.edges.len() {
                let next = if at[node as usize][0] == via {
                    at[node as usize][1]
                } else {
                    at[node as usize][0]
                };
                len += lens[next as usize];
                node = other(next, node);
                via = next;
                steps += 1;
            }
            let p = g.nodes[u].pos;
            if off_the_map(p) {
                continue;
            }
            // On another road's asphalt, at its height, with no node there.
            let (tip, y) = (V2::new(p[0], p[1]), g.nodes[u].elevation);
            let touches = g.edges.iter().any(|e| {
                e.a != u as u32
                    && e.b != u as u32
                    && e.profile.windows(2).any(|w| {
                        let (a, b) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
                        segment_distance(tip, a, b).0 < 0.5 * e.width_m
                            && (0.5 * (w[0][1] + w[1][1]) - y).abs() < GRADE_M
                    })
            });
            let fault = if touches {
                Fault::Unjoined
            } else if len < STUB_M {
                Fault::Stub
            } else if dead_end_ok(p) {
                continue;
            } else {
                Fault::DeadEnd
            };
            out.push(Finding {
                fault,
                at: p,
                class: g.edges[first as usize].class,
                len_m: len,
            });
        } else if at[u].len() >= 3 {
            // Teardrops: out of this junction by one edge and back by another, through
            // nodes that meet nothing. Found once, from the lower edge.
            for &k in &at[u] {
                if g.edges[k as usize].a == g.edges[k as usize].b {
                    continue;
                }
                let (mut node, mut via, mut len) = (other(k, u as u32), k, lens[k as usize]);
                let mut steps = 0;
                while node != u as u32 && at[node as usize].len() == 2 && steps < g.edges.len() {
                    let next = if at[node as usize][0] == via {
                        at[node as usize][1]
                    } else {
                        at[node as usize][0]
                    };
                    len += lens[next as usize];
                    node = other(next, node);
                    via = next;
                    steps += 1;
                }
                // A bulb on the junction itself is the dead end above, not a teardrop.
                let bulb = steps == 1 && len < 4.0 * core::f32::consts::PI * 14.0;
                if node == u as u32 && k < via && !bulb && !dead_end_ok(g.nodes[u].pos) {
                    out.push(Finding {
                        fault: Fault::Loop,
                        at: g.nodes[u].pos,
                        class: g.edges[k as usize].class,
                        len_m: len,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use place::{Edge, Node};

    fn place(nodes: &[[f32; 2]], edges: &[(u32, u32)]) -> Place {
        let mut p = Place::test_grid();
        p.header.chunk_origin = [0, 0];
        p.header.size_chunks = [8, 8];
        p.roads.nodes = nodes
            .iter()
            .map(|&pos| Node {
                pos,
                elevation: 0.0,
            })
            .collect();
        let proto = p.roads.edges[0].clone();
        p.roads.edges = edges
            .iter()
            .map(|&(a, b)| {
                let (pa, pb) = (nodes[a as usize], nodes[b as usize]);
                Edge {
                    a,
                    b,
                    class: RoadClass::Local,
                    // A loop's profile goes out and back so it has a length.
                    profile: if a == b {
                        vec![
                            [pa[0], 0.0, pa[1]],
                            [pa[0] + 20.0, 0.0, pa[1]],
                            [pa[0], 0.0, pa[1]],
                        ]
                    } else {
                        vec![[pa[0], 0.0, pa[1]], [pb[0], 0.0, pb[1]]]
                    },
                    ..proto.clone()
                }
            })
            .collect();
        p
    }

    /// Two 200 m blocks side by side: six nodes, the middle street between them.
    const BLOCK: [[f32; 2]; 6] = [
        [200.0, 200.0],
        [400.0, 200.0],
        [600.0, 200.0],
        [600.0, 400.0],
        [400.0, 400.0],
        [200.0, 400.0],
    ];
    const RING: [(u32, u32); 7] = [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0), (1, 4)];

    fn faults(nodes: &[[f32; 2]], edges: &[(u32, u32)], ok: bool) -> Vec<(Fault, [f32; 2])> {
        roads(&place(nodes, edges), |_| ok)
            .iter()
            .map(|f| (f.fault, f.at))
            .collect()
    }

    #[test]
    fn two_blocks_are_clean() {
        assert_eq!(faults(&BLOCK, &RING, false), []);
    }

    #[test]
    fn a_dead_end_is_found_unless_allowed_and_a_stub_always() {
        let mut nodes = BLOCK.to_vec();
        nodes.push([700.0, 200.0]);
        let mut edges = RING.to_vec();
        edges.push((2, 6));
        assert_eq!(
            faults(&nodes, &edges, false),
            [(Fault::DeadEnd, [700.0, 200.0])]
        );
        assert_eq!(faults(&nodes, &edges, true), []);
        nodes[6] = [620.0, 200.0];
        assert_eq!(
            faults(&nodes, &edges, true),
            [(Fault::Stub, [620.0, 200.0])]
        );
    }

    #[test]
    fn a_dead_end_on_another_road_is_unjoined() {
        // A street from the corner to the middle of the far side, with no node there.
        let mut nodes = BLOCK.to_vec();
        nodes.push([500.0, 400.0]);
        let mut edges = RING.to_vec();
        edges.push((2, 6));
        assert_eq!(
            faults(&nodes, &edges, true),
            [(Fault::Unjoined, [500.0, 400.0])]
        );
    }

    #[test]
    fn a_road_off_the_edge_of_the_place_is_not_a_dead_end() {
        let mut nodes = BLOCK.to_vec();
        nodes.push([1010.0, 200.0]);
        let mut edges = RING.to_vec();
        edges.push((2, 6));
        assert_eq!(faults(&nodes, &edges, false), []);
    }

    #[test]
    fn a_bulb_is_a_dead_end_at_its_tip() {
        // A street off the blocks ending in a bulb: two edges from its end to the tip.
        let mut nodes = BLOCK.to_vec();
        nodes.extend([[700.0, 200.0], [724.0, 200.0]]);
        let mut edges = RING.to_vec();
        edges.extend([(2, 6), (6, 7), (6, 7)]);
        assert_eq!(
            faults(&nodes, &edges, false),
            [(Fault::DeadEnd, [724.0, 200.0])]
        );
        assert_eq!(faults(&nodes, &edges, true), []);
    }

    #[test]
    fn a_teardrop_and_a_self_loop_are_loops() {
        // Out of a corner, round two nodes and back to it.
        let mut nodes = BLOCK.to_vec();
        nodes.extend([[700.0, 150.0], [700.0, 250.0]]);
        let mut edges = RING.to_vec();
        edges.extend([(2, 6), (6, 7), (7, 2)]);
        assert_eq!(
            faults(&nodes, &edges, false),
            [(Fault::Loop, [600.0, 200.0])]
        );
        let mut edges = RING.to_vec();
        edges.push((3, 3));
        assert_eq!(
            faults(&BLOCK, &edges, false),
            [(Fault::Loop, [600.0, 400.0])]
        );
    }

    #[test]
    fn roads_off_the_network_are_unreachable() {
        let mut nodes = BLOCK.to_vec();
        nodes.extend([[700.0, 600.0], [800.0, 600.0], [800.0, 700.0]]);
        let mut edges = RING.to_vec();
        edges.extend([(6, 7), (7, 8), (8, 6)]);
        assert_eq!(
            faults(&nodes, &edges, true),
            [(Fault::Unreachable, [700.0, 600.0])]
        );
    }
}
