//! Plain-data description of a Skywave place.
//!
//! A [`Place`] is what world generation (or the real-data compiler) produces and what the
//! world, traffic and renderer consume. Coordinates are meters in a right-handed, Y-up world:
//! 2D positions are `[x, z]` on the ground plane.
#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use skywave_core::hash_bytes;

/// Format version of the [`Place`] layout and of [`Place::write_bytes`].
pub const PLACE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct PlaceHeader {
    pub version: u32,
    pub seed: u64,
    /// Edge length of a streaming chunk, in meters.
    pub chunk_size_m: f32,
}

/// A regular heightfield of `width * height` samples, row-major (`z` major, `x` minor).
#[derive(Clone, Debug, PartialEq)]
pub struct Terrain {
    pub width: u32,
    pub height: u32,
    /// Distance between neighbouring samples, in meters.
    pub cell_m: f32,
    /// Elevations in meters; `heights.len() == width * height`.
    pub heights: Vec<f32>,
}

impl Terrain {
    pub fn flat(width: u32, height: u32, cell_m: f32, elevation: f32) -> Terrain {
        let mut heights = Vec::new();
        heights.resize(width as usize * height as usize, elevation);
        Terrain {
            width,
            height,
            cell_m,
            heights,
        }
    }

    /// Elevation at sample `(x, z)`, or `None` outside the grid.
    pub fn sample(&self, x: u32, z: u32) -> Option<f32> {
        if x < self.width && z < self.height {
            self.heights
                .get(z as usize * self.width as usize + x as usize)
                .copied()
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub pos: [f32; 2],
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoadClass {
    Local = 0,
    Collector = 1,
    Arterial = 2,
    Highway = 3,
}

/// An undirected road segment between nodes `a` and `b` (indices into `RoadGraph::nodes`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub class: RoadClass,
    /// Total lanes, both directions.
    pub lanes: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoadGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// An extruded building: a closed, counter-clockwise (seen from above) footprint polygon.
#[derive(Clone, Debug, PartialEq)]
pub struct Building {
    pub footprint: Vec<[f32; 2]>,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub header: PlaceHeader,
    pub terrain: Terrain,
    pub roads: RoadGraph,
    pub buildings: Vec<Building>,
}

/// Why [`Place::validate`] rejected a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceError {
    Version,
    TerrainSize,
    EdgeIndex { edge: u32 },
    DegenerateEdge { edge: u32 },
    NoLanes { edge: u32 },
    Footprint { building: u32 },
    BuildingHeight { building: u32 },
    NonFinite,
}

impl Place {
    /// A tiny hand-laid town used by the engine skeleton: a 4 x 4 street grid on 80 m
    /// blocks with an arterial through the middle and one or two boxes per block.
    pub fn test_grid() -> Place {
        const N: u32 = 4;
        const BLOCK: f32 = 80.0;
        let origin = (N - 1) as f32 * BLOCK * -0.5;

        let mut nodes = Vec::new();
        for z in 0..N {
            for x in 0..N {
                nodes.push(Node {
                    pos: [origin + x as f32 * BLOCK, origin + z as f32 * BLOCK],
                });
            }
        }
        let mut edges = Vec::new();
        for z in 0..N {
            for x in 0..N {
                let i = z * N + x;
                // Row z == 1 is the east-west arterial; column x == 1 the north-south collector.
                if x + 1 < N {
                    let (class, lanes) = if z == 1 {
                        (RoadClass::Arterial, 4)
                    } else {
                        (RoadClass::Local, 2)
                    };
                    edges.push(Edge {
                        a: i,
                        b: i + 1,
                        class,
                        lanes,
                    });
                }
                if z + 1 < N {
                    let (class, lanes) = if x == 1 {
                        (RoadClass::Collector, 2)
                    } else {
                        (RoadClass::Local, 2)
                    };
                    edges.push(Edge {
                        a: i,
                        b: i + N,
                        class,
                        lanes,
                    });
                }
            }
        }

        // Per block: (inset from block corner, width, depth, height). Hand-picked so the
        // skyline has variety without needing randomness.
        const LOTS: [[f32; 4]; 9] = [
            [14.0, 22.0, 30.0, 9.0],
            [12.0, 50.0, 24.0, 6.0],
            [16.0, 26.0, 26.0, 24.0],
            [12.0, 30.0, 18.0, 4.5],
            [18.0, 40.0, 40.0, 36.0],
            [14.0, 20.0, 46.0, 12.0],
            [12.0, 52.0, 20.0, 7.5],
            [20.0, 24.0, 24.0, 15.0],
            [13.0, 34.0, 28.0, 5.0],
        ];
        let mut buildings = Vec::new();
        for bz in 0..N - 1 {
            for bx in 0..N - 1 {
                let [inset, w, d, h] = LOTS[(bz * (N - 1) + bx) as usize];
                let x0 = origin + bx as f32 * BLOCK + inset;
                let z0 = origin + bz as f32 * BLOCK + inset;
                buildings.push(Building {
                    footprint: rect(x0, z0, w, d),
                    height: h,
                });
                // A small second building in the far corner of the shorter lots.
                if h < 10.0 {
                    let x1 = origin + (bx + 1) as f32 * BLOCK - 12.0 - 14.0;
                    let z1 = origin + (bz + 1) as f32 * BLOCK - 12.0 - 14.0;
                    buildings.push(Building {
                        footprint: rect(x1, z1, 14.0, 14.0),
                        height: h + 3.0,
                    });
                }
            }
        }

        let extent = (N - 1) as f32 * BLOCK + 2.0 * BLOCK;
        Place {
            header: PlaceHeader {
                version: PLACE_VERSION,
                seed: 0,
                chunk_size_m: 256.0,
            },
            terrain: Terrain::flat(9, 9, extent / 8.0, 0.0),
            roads: RoadGraph { nodes, edges },
            buildings,
        }
    }

    /// Checks the structural invariants every consumer relies on.
    pub fn validate(&self) -> Result<(), PlaceError> {
        if self.header.version != PLACE_VERSION {
            return Err(PlaceError::Version);
        }
        let t = &self.terrain;
        if t.heights.len() != t.width as usize * t.height as usize {
            return Err(PlaceError::TerrainSize);
        }
        let finite = self.header.chunk_size_m.is_finite()
            && t.cell_m.is_finite()
            && t.heights.iter().all(|h| h.is_finite())
            && self
                .roads
                .nodes
                .iter()
                .all(|n| n.pos.iter().all(|c| c.is_finite()));
        if !finite {
            return Err(PlaceError::NonFinite);
        }
        let n = self.roads.nodes.len();
        for (i, e) in self.roads.edges.iter().enumerate() {
            let edge = i as u32;
            if e.a as usize >= n || e.b as usize >= n {
                return Err(PlaceError::EdgeIndex { edge });
            }
            if e.a == e.b {
                return Err(PlaceError::DegenerateEdge { edge });
            }
            if e.lanes == 0 {
                return Err(PlaceError::NoLanes { edge });
            }
        }
        for (i, b) in self.buildings.iter().enumerate() {
            let building = i as u32;
            let pts_ok = b.footprint.iter().all(|p| p.iter().all(|c| c.is_finite()));
            if b.footprint.len() < 3 || !pts_ok || signed_area(&b.footprint) <= 0.0 {
                return Err(PlaceError::Footprint { building });
            }
            if !(b.height.is_finite() && b.height > 0.0) {
                return Err(PlaceError::BuildingHeight { building });
            }
        }
        Ok(())
    }

    /// Canonical little-endian serialization, in field declaration order. Vectors are
    /// prefixed by their `u32` length. Used for hashing and, later, the on-disk format.
    pub fn write_bytes(&self, out: &mut Vec<u8>) {
        let h = &self.header;
        put_u32(out, h.version);
        out.extend_from_slice(&h.seed.to_le_bytes());
        put_f32(out, h.chunk_size_m);

        let t = &self.terrain;
        put_u32(out, t.width);
        put_u32(out, t.height);
        put_f32(out, t.cell_m);
        put_u32(out, t.heights.len() as u32);
        for &v in &t.heights {
            put_f32(out, v);
        }

        put_u32(out, self.roads.nodes.len() as u32);
        for node in &self.roads.nodes {
            put_f32(out, node.pos[0]);
            put_f32(out, node.pos[1]);
        }
        put_u32(out, self.roads.edges.len() as u32);
        for e in &self.roads.edges {
            put_u32(out, e.a);
            put_u32(out, e.b);
            out.push(e.class as u8);
            out.push(e.lanes);
        }

        put_u32(out, self.buildings.len() as u32);
        for b in &self.buildings {
            put_u32(out, b.footprint.len() as u32);
            for p in &b.footprint {
                put_f32(out, p[0]);
                put_f32(out, p[1]);
            }
            put_f32(out, b.height);
        }
    }

    /// Hash of [`Place::write_bytes`]: the determinism fingerprint of a place.
    pub fn content_hash(&self) -> u64 {
        let mut bytes = Vec::new();
        self.write_bytes(&mut bytes);
        hash_bytes(0, &bytes)
    }
}

fn rect(x0: f32, z0: f32, w: f32, d: f32) -> Vec<[f32; 2]> {
    // Counter-clockwise seen from above (+Y), i.e. positive area in the (x, z) plane
    // with the shoelace formula below.
    vec![[x0, z0], [x0, z0 + d], [x0 + w, z0 + d], [x0 + w, z0]]
}

/// Shoelace area, positive for counter-clockwise footprints viewed from +Y.
fn signed_area(poly: &[[f32; 2]]) -> f32 {
    let mut acc = 0.0;
    for (i, p) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()];
        acc += p[1] * q[0] - p[0] * q[1];
    }
    acc * 0.5
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_is_valid() {
        let p = Place::test_grid();
        assert_eq!(p.validate(), Ok(()));
        assert_eq!(p.roads.nodes.len(), 16);
        assert_eq!(p.roads.edges.len(), 24);
        assert!(p.buildings.len() >= 9);
    }

    #[test]
    fn test_grid_is_deterministic() {
        let a = Place::test_grid();
        let b = Place::test_grid();
        assert_eq!(a, b);
        assert_eq!(a.content_hash(), b.content_hash());
        // Pinned fingerprint: change deliberately, in its own commit, with the reason.
        assert_eq!(a.content_hash(), 0x6D8C_1D3E_DDFB_D18A);
    }

    #[test]
    fn validate_catches_broken_places() {
        let mut p = Place::test_grid();
        p.roads.edges[3].b = 99;
        assert_eq!(p.validate(), Err(PlaceError::EdgeIndex { edge: 3 }));

        let mut p = Place::test_grid();
        p.roads.edges[0].b = p.roads.edges[0].a;
        assert_eq!(p.validate(), Err(PlaceError::DegenerateEdge { edge: 0 }));

        let mut p = Place::test_grid();
        p.roads.edges[1].lanes = 0;
        assert_eq!(p.validate(), Err(PlaceError::NoLanes { edge: 1 }));

        let mut p = Place::test_grid();
        p.buildings[2].footprint.reverse();
        assert_eq!(p.validate(), Err(PlaceError::Footprint { building: 2 }));

        let mut p = Place::test_grid();
        p.buildings[0].height = 0.0;
        assert_eq!(
            p.validate(),
            Err(PlaceError::BuildingHeight { building: 0 })
        );

        let mut p = Place::test_grid();
        p.terrain.heights.pop();
        assert_eq!(p.validate(), Err(PlaceError::TerrainSize));

        let mut p = Place::test_grid();
        p.roads.nodes[0].pos[0] = f32::NAN;
        assert_eq!(p.validate(), Err(PlaceError::NonFinite));
    }

    #[test]
    fn terrain_sampling() {
        let t = Terrain::flat(3, 2, 10.0, 1.5);
        assert_eq!(t.sample(2, 1), Some(1.5));
        assert_eq!(t.sample(3, 0), None);
        assert_eq!(t.sample(0, 2), None);
    }

    #[test]
    fn serialization_changes_with_content() {
        let a = Place::test_grid();
        let mut b = a.clone();
        b.buildings[0].height += 1.0;
        assert_ne!(a.content_hash(), b.content_hash());
    }
}
