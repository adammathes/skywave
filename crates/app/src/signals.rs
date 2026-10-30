//! The junction lights as the drivers see them: each signal pole's heads light the lens of
//! the light its approach shows now (`traffic::LaneGraph::heads`), the same light the
//! traffic obeys. The poles and their dark lenses are in the world's chunks; the lit lenses
//! are instances of three one-quad meshes (green, yellow, red), pushed each frame grouped by
//! color so each is one instanced draw. Nothing allocates per frame.

use glam::{Mat4, Quat, Vec3};
use place::{Place, PropKind};
use render::{Mesh, MeshId, MeshInstance};
use traffic::{LaneGraph, Light, NONE};
use world::props::{dir_of_yaw, signal_lamp, SIGNAL_HEADS};
use world::{MeshKind, Palette, WorldMesh};

use crate::worldmesh::atlas_mesh;
use crate::{convert, Part};

/// Lit lenses are drawn within this of the eye, m (farther they are under a pixel).
pub const DRAW_RADIUS_M: f32 = 500.0;

/// One signal head: where it hangs, which way the pole faces, the lane whose light it
/// shows and the chunk it is drawn with.
struct Head {
    pos: Vec3,
    yaw: f32,
    lane: u32,
    chunk: [i32; 2],
}

/// The place's signal heads and their lit lens meshes.
pub struct Signals {
    heads: Vec<Head>,
    /// Lit lens mesh per light, in [`Light`] order, once registered.
    ids: Vec<MeshId>,
}

impl Signals {
    /// The heads of `place`'s signal poles that show a light (`graph`'s), each in the chunk
    /// `chunk_of` its pole stands in; `None` without any.
    pub fn of(
        place: &Place,
        graph: &LaneGraph,
        chunk_of: impl Fn([f32; 2]) -> [i32; 2],
    ) -> Option<Signals> {
        let mut heads = Vec::new();
        for h in &graph.heads {
            let p = &place.props.items[h.prop as usize];
            let PropKind::Signal { arm_dm } = p.kind else {
                continue;
            };
            if h.lane == NONE {
                continue;
            }
            let arm = dir_of_yaw(p.yaw);
            let chunk = chunk_of([p.pos[0], p.pos[2]]);
            for t in SIGNAL_HEADS {
                let x = f32::from(arm_dm) * 0.1 * t;
                heads.push(Head {
                    pos: Vec3::from(p.pos) + Vec3::new(arm.x, 0.0, arm.y) * x,
                    yaw: p.yaw,
                    lane: h.lane,
                    chunk,
                });
            }
        }
        (!heads.is_empty()).then_some(Signals {
            heads,
            ids: Vec::new(),
        })
    }

    /// The lit lens meshes, in [`Light`] order; [`Signals::set_ids`] takes their ids.
    pub fn meshes(pal: &Palette) -> Vec<Mesh> {
        (0..3)
            .map(|light| {
                let mut m = WorldMesh::default();
                signal_lamp(&mut m, light, pal);
                convert(&atlas_mesh(&m, MeshKind::Prop), Part::Base)
            })
            .collect()
    }

    pub fn set_ids(&mut self, ids: Vec<MeshId>) {
        self.ids = ids;
    }

    /// Most instances a frame adds: one lit lens per head.
    pub fn capacity(&self) -> usize {
        self.heads.len()
    }

    /// The light head `k` shows at signal tick `tick`.
    fn light(&self, graph: &LaneGraph, k: usize, tick: u64) -> Light {
        graph.light(self.heads[k].lane, tick).unwrap_or(Light::Red)
    }

    /// Appends the lit lens of every head near `eye` in a chunk `live` says is drawn, as
    /// its lane shows at signal tick `tick`.
    pub fn push(
        &self,
        graph: &LaneGraph,
        tick: u64,
        eye: Vec3,
        live: impl Fn([i32; 2]) -> bool,
        out: &mut Vec<MeshInstance>,
    ) {
        if self.ids.len() < 3 {
            return;
        }
        let r2 = DRAW_RADIUS_M * DRAW_RADIUS_M;
        for light in [Light::Green, Light::Yellow, Light::Red] {
            let id = self.ids[light as usize];
            for (k, h) in self.heads.iter().enumerate() {
                if h.pos.distance_squared(eye) > r2
                    || !live(h.chunk)
                    || self.light(graph, k, tick) != light
                {
                    continue;
                }
                let m = Mat4::from_rotation_translation(Quat::from_rotation_y(h.yaw), h.pos);
                out.push(MeshInstance {
                    casts_shadow: false,
                    ..MeshInstance::new(id, m.to_cols_array_2d())
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::PlaceKey;
    use traffic::signal::cycle_ticks;

    /// Every head's lit lens follows `traffic::signal::light` of its lane across a whole
    /// cycle: green, then yellow, then red, one lens at a time, the same light the cars obey.
    #[test]
    fn a_heads_lit_lens_follows_its_light_across_a_cycle() {
        let place = crate::places::build(PlaceKey::Coast, skywave_core::Seed(1)).unwrap();
        let graph = &LaneGraph::new(&place);
        let mut s = Signals::of(&place, graph, |_| [0, 0]).expect("the town has signals");
        let ids: Vec<MeshId> = (0..3).map(MeshId).collect();
        s.set_ids(ids);
        let h = &s.heads[0];
        let j = &graph.junctions[graph.lanes[h.lane as usize].end_node as usize];
        let (lane, phase) = (h.lane, graph.lanes[h.lane as usize].phase);
        let mut seen = [0u32; 3];
        let mut out = Vec::with_capacity(s.capacity());
        let mut last = None;
        let mut changes = 0;
        for tick in 0..u64::from(cycle_ticks(j.phases)) {
            out.clear();
            s.push(graph, tick, h.pos, |_| true, &mut out);
            let want = traffic::signal::light(j.offset, phase, j.phases, tick);
            assert_eq!(graph.light(lane, tick), Some(want));
            // Exactly one lit lens on this head, of its light's color.
            let here: Vec<&MeshInstance> = out
                .iter()
                .filter(|i| Vec3::from_slice(&i.transform[3][..3]).distance(h.pos) < 1e-3)
                .collect();
            assert_eq!(here.len(), 1, "tick {tick}");
            assert_eq!(here[0].mesh, MeshId(want as u32), "tick {tick}");
            seen[want as usize] += 1;
            changes += usize::from(last.is_some_and(|l| l != want));
            last = Some(want);
        }
        assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
        // Around one cycle a head changes light three times (two with the wrap).
        assert!((2..=3).contains(&changes), "{changes}");
        // Every head is drawn once, wherever its chunk is drawn.
        out.clear();
        s.push(graph, 0, h.pos, |_| true, &mut out);
        let near = s
            .heads
            .iter()
            .filter(|x| x.pos.distance_squared(h.pos) <= DRAW_RADIUS_M * DRAW_RADIUS_M)
            .count();
        assert_eq!(out.len(), near);
        out.clear();
        s.push(graph, 0, h.pos, |_| false, &mut out);
        assert!(out.is_empty());
    }
}
