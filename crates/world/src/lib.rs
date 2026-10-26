//! Turns a [`place::Place`] into chunked, vertex-colored low-poly meshes at the fidelity
//! bar of `docs/RENDERING.md`: roads with curbs, gutters, slabbed sidewalks, crosswalks and
//! lane paint; buildings with plinths, inset windows, storefronts, awnings, signs and
//! parapets; street lights, trees, palms, hydrants, benches, signals and parked cars.
//!
//! Everything is a pure function of the Place (and its `header.seed` for detail
//! variation): same input, bit-identical vertices on every platform. [`collide`] builds the
//! static shapes the car collides with from the same layout.
//!
//! [`procgen`] is the procedural producer (macro map so far), driven by a
//! [`template::RegionTemplate`].
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod buildings;
pub mod collide;
pub mod ground;
pub mod math;
pub mod mesh;
pub mod palette;
pub mod procgen;
pub mod props;
pub mod roads;
pub mod template;

use alloc::vec::Vec;

use place::{Place, ZoneId, SIDEWALK_WIDTH_M};

pub use collide::{resolve, Impact, WorldCollider};
pub use mesh::{Glow, Pattern, WorldMesh, WorldVertex};
pub use palette::Palette;

use buildings::FacadeEnv;
use math::{floor, point_in_polygon, segment_distance, V2};

/// Height of curb tops, sidewalks and lots above the asphalt (which is at `y = 0`).
pub const CURB_M: f32 = 0.15;
/// Ground level inside blocks.
pub const LOT_Y: f32 = CURB_M;
/// The terrain apron around the town, just below the asphalt so it never shows through.
pub const TERRAIN_Y: f32 = -0.06;
/// Lift of paint and overlays above the surface they sit on. Larger than the 1 mm a real
/// stripe is so 32-bit depth (near plane 0.25 m) separates them out to ~150 m.
pub const PAINT_LIFT: f32 = 0.005;

/// What a chunk mesh contains; one draw per (chunk, kind).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MeshKind {
    Ground = 0,
    Road = 1,
    Marking = 2,
    Building = 3,
    Prop = 4,
}

/// The mesh of one kind of content in one chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkMesh {
    /// Chunk coordinates: `floor(x / chunk_size_m)`, `floor(z / chunk_size_m)`.
    pub chunk: [i32; 2],
    pub kind: MeshKind,
    pub mesh: WorldMesh,
}

/// Collects meshes by chunk and kind while building.
pub struct MeshSet {
    chunk_size: f32,
    meshes: Vec<ChunkMesh>,
}

impl MeshSet {
    fn new(chunk_size: f32) -> MeshSet {
        MeshSet {
            chunk_size: if chunk_size > 0.0 { chunk_size } else { 128.0 },
            meshes: Vec::new(),
        }
    }

    /// The mesh of `kind` in the chunk containing `p`.
    pub fn at(&mut self, p: V2, kind: MeshKind) -> &mut WorldMesh {
        let chunk = [
            floor(p.x / self.chunk_size) as i32,
            floor(p.y / self.chunk_size) as i32,
        ];
        let found = self
            .meshes
            .iter()
            .position(|c| c.chunk == chunk && c.kind == kind);
        let i = match found {
            Some(i) => i,
            None => {
                self.meshes.push(ChunkMesh {
                    chunk,
                    kind,
                    mesh: WorldMesh::default(),
                });
                self.meshes.len() - 1
            }
        };
        &mut self.meshes[i].mesh
    }
}

/// Totals from a build, for budgets and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BuildStats {
    pub triangles: usize,
    pub vertices: usize,
    /// Triangles per [`MeshKind`].
    pub triangles_by_kind: [usize; 5],
    pub buildings: usize,
    pub max_building_triangles: usize,
    pub windows: u32,
    pub lit_windows: u32,
    pub props: usize,
}

/// Builds all meshes of `place`, grouped by chunk and [`MeshKind`], sorted by kind then
/// chunk.
pub fn build_chunk_meshes(place: &Place, palette: &Palette) -> Vec<ChunkMesh> {
    build_with_stats(place, palette).0
}

/// [`build_chunk_meshes`] plus totals.
pub fn build_with_stats(place: &Place, palette: &Palette) -> (Vec<ChunkMesh>, BuildStats) {
    let seed = place.header.seed;
    let mut out = MeshSet::new(place.header.chunk_size_m);
    let mut stats = BuildStats::default();
    let net = roads::Network::new(place);
    let side_zone = side_zones(place, &net);
    let blocks = ground::blocks(&net);

    // Plan every building first: the ground needs to know where strip malls face.
    let lookup = Lookup::new(place, &net);
    let plans: Vec<(Vec<FacadeEnv>, buildings::BuildingPlan)> = place
        .buildings
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let env = lookup.facade_envs(i);
            let door_pick = skywave_core::hash64(seed, &[0x444F_4F52, i as u64]) as u32;
            let plan = buildings::plan_building(b, &env, door_pick);
            (env, plan)
        })
        .collect();
    let fronts: Vec<usize> = plans.iter().map(|(_, p)| p.frontage).collect();

    let mut props = ground::build(place, &blocks, &fronts, palette, seed, &mut out);
    roads::build(place, &net, &side_zone, palette, seed, &mut out);

    for (i, (b, (env, plan))) in place.buildings.iter().zip(&plans).enumerate() {
        let fp = ground::to_v2(&b.footprint);
        let m = out.at(ground::centroid(&fp), MeshKind::Building);
        let s = buildings::build(m, b, plan, palette, seed, i as u32);
        props.extend(buildings::house_site(
            m, b, plan, env, palette, seed, i as u32,
        ));
        stats.buildings += 1;
        stats.windows += s.windows;
        stats.lit_windows += s.lit_windows;
        stats.max_building_triangles = stats.max_building_triangles.max(s.triangles);
    }

    let parks: Vec<Vec<V2>> = blocks
        .iter()
        .filter(|b| ground::is_park(place, b))
        .map(|b| b.poly.clone())
        .collect();
    props.extend(props::place(place, &net, &side_zone, &parks, seed));
    let kit = props::PropKit::new();
    for p in &props {
        props::build(out.at(p.pos, MeshKind::Prop), p, palette, &kit);
    }
    stats.props = props.len();

    let mut meshes = out.meshes;
    meshes.retain(|c| !c.mesh.indices.is_empty());
    meshes.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.chunk.cmp(&b.chunk)));
    for c in &meshes {
        let t = c.mesh.triangle_count();
        stats.triangles += t;
        stats.vertices += c.mesh.vertices.len();
        stats.triangles_by_kind[c.kind as usize] += t;
    }
    (meshes, stats)
}

/// Determinism fingerprint of built meshes: every vertex and index, in order.
pub fn mesh_hash(meshes: &[ChunkMesh]) -> u64 {
    let mut h = 0u64;
    let mut buf: Vec<u8> = Vec::new();
    for c in meshes {
        buf.clear();
        buf.extend_from_slice(&c.chunk[0].to_le_bytes());
        buf.extend_from_slice(&c.chunk[1].to_le_bytes());
        buf.push(c.kind as u8);
        for v in &c.mesh.vertices {
            for x in v.pos.iter().chain(&v.normal).chain(&v.uv) {
                buf.extend_from_slice(&x.to_bits().to_le_bytes());
            }
            buf.extend_from_slice(&v.color);
            buf.extend_from_slice(&v.emissive);
        }
        for i in &c.mesh.indices {
            buf.extend_from_slice(&i.to_le_bytes());
        }
        h = skywave_core::hash_bytes(h, &buf);
    }
    h
}

/// Whether two edges lie on one straight street (parallel and collinear).
fn same_street(place: &Place, a: usize, b: usize) -> bool {
    let nodes = &place.roads.nodes;
    let line = |e: usize| {
        let e = &place.roads.edges[e];
        (
            V2::from_array(nodes[e.a as usize].pos),
            V2::from_array(nodes[e.b as usize].pos),
        )
    };
    let ((a0, a1), (b0, b1)) = (line(a), line(b));
    let (da, db) = ((a1 - a0).normalize(), (b1 - b0).normalize());
    a == b || (math::abs(da.turn(db)) < 0.02 && math::abs(da.turn(b0 - a0)) < 0.5)
}

fn bounds(poly: &[V2]) -> (V2, V2) {
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for p in poly {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo, hi)
}

/// The zone of the parcels lining each side of each edge (`[left, right]` of `a -> b`).
fn side_zones(place: &Place, net: &roads::Network) -> Vec<[Option<ZoneId>; 2]> {
    let nodes = &place.roads.nodes;
    let parcels: Vec<(Vec<V2>, (V2, V2))> = place
        .parcels
        .iter()
        .map(|p| {
            let poly = ground::to_v2(&p.polygon);
            let bb = bounds(&poly);
            (poly, bb)
        })
        .collect();
    place
        .roads
        .edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let a = V2::from_array(nodes[e.a as usize].pos);
            let b = V2::from_array(nodes[e.b as usize].pos);
            let (ea, _) = net.ends(i);
            let d = (b - a).normalize();
            let mut zones = [None, None];
            for (k, sigma) in [1.0f32, -1.0].into_iter().enumerate() {
                for t in [0.5, 0.3, 0.7, 0.15, 0.85] {
                    let p = a.lerp(b, t) + d.left() * (sigma * (ea.hw + ea.sw + 3.0));
                    let hit = parcels.iter().position(|(poly, (lo, hi))| {
                        p.x >= lo.x
                            && p.x <= hi.x
                            && p.y >= lo.y
                            && p.y <= hi.y
                            && point_in_polygon(p, poly)
                    });
                    if let Some(j) = hit {
                        zones[k] = Some(place.parcels[j].zone);
                        break;
                    }
                }
            }
            zones
        })
        .collect()
}

/// Precomputed geometry for asking what is in front of each facade.
struct Lookup<'a> {
    place: &'a Place,
    footprints: Vec<Vec<V2>>,
    boxes: Vec<(V2, V2)>,
    parcels: Vec<(Vec<V2>, (V2, V2))>,
    /// Road segments: ends, bounds, half width plus sidewalk.
    roads: Vec<(V2, V2, (V2, V2), f32)>,
}

impl Lookup<'_> {
    fn new<'a>(place: &'a Place, net: &roads::Network) -> Lookup<'a> {
        let footprints: Vec<Vec<V2>> = place
            .buildings
            .iter()
            .map(|b| ground::to_v2(&b.footprint))
            .collect();
        let boxes = footprints.iter().map(|f| bounds(f)).collect();
        let parcels = place
            .parcels
            .iter()
            .map(|p| {
                let poly = ground::to_v2(&p.polygon);
                let bb = bounds(&poly);
                (poly, bb)
            })
            .collect();
        let nodes = &place.roads.nodes;
        let roads = place
            .roads
            .edges
            .iter()
            .enumerate()
            .map(|(k, e)| {
                let ra = V2::from_array(nodes[e.a as usize].pos);
                let rb = V2::from_array(nodes[e.b as usize].pos);
                let (end, _) = net.ends(k);
                let sw = if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
                (ra, rb, bounds(&[ra, rb]), end.hw + sw)
            })
            .collect();
        Lookup {
            place,
            footprints,
            boxes,
            parcels,
            roads,
        }
    }

    /// What each facade of building `index` faces: nearest road ahead, whether that is the
    /// parcel's frontage street, and whether a neighbouring building stands right against it.
    fn facade_envs(&self, index: usize) -> Vec<FacadeEnv> {
        let fp = &self.footprints[index];
        let c = ground::centroid(fp);
        let inside =
            |p: V2, (lo, hi): (V2, V2)| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y;
        let frontage_edge = self
            .parcels
            .iter()
            .position(|(poly, bb)| inside(c, *bb) && point_in_polygon(c, poly))
            .map(|k| self.place.parcels[k].frontage_edge as usize);
        let n = fp.len();
        (0..n)
            .map(|i| {
                let (a, b) = (fp[i], fp[(i + 1) % n]);
                let normal = (b - a).normalize().right();
                let mid = (a + b) * 0.5;
                let (mut road_dist, mut yard, mut nearest) = (f32::MAX, f32::MAX, usize::MAX);
                let r = 130.0;
                for (k, &(ra, rb, (lo, hi), half)) in self.roads.iter().enumerate() {
                    if mid.x < lo.x - r || mid.x > hi.x + r || mid.y < lo.y - r || mid.y > hi.y + r
                    {
                        continue;
                    }
                    let (dist, t) = segment_distance(mid, ra, rb);
                    let ahead = (ra.lerp(rb, t) - mid).dot(normal);
                    if ahead > dist * 0.7 && dist < road_dist {
                        road_dist = dist;
                        nearest = k;
                        yard = dist - half;
                    }
                }
                let blocked = [0.2f32, 0.5, 0.8].iter().any(|&t| {
                    let p = a.lerp(b, t) + normal * 0.8;
                    self.boxes.iter().enumerate().any(|(j, bb)| {
                        j != index && inside(p, *bb) && point_in_polygon(p, &self.footprints[j])
                    })
                });
                let frontage = match frontage_edge {
                    Some(f) if nearest != usize::MAX => same_street(self.place, f, nearest),
                    _ => false,
                };
                FacadeEnv {
                    road_dist,
                    yard,
                    blocked,
                    frontage,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Whole test town budget (`docs/RENDERING.md`: one ~1 km place).
    const TOWN_TRIANGLE_BUDGET: usize = 600_000;

    #[test]
    fn test_town_is_deterministic_and_within_budgets() {
        let place = Place::test_grid();
        let pal = Palette::socal_1985();
        let t0 = Instant::now();
        let (meshes, stats) = build_with_stats(&place, &pal);
        let elapsed = t0.elapsed();
        std::println!(
            "world: {} triangles ({:?} by kind), {} vertices, {} meshes, {} buildings (max {} tris), {} windows ({} lit), {} props in {:.1} ms",
            stats.triangles,
            stats.triangles_by_kind,
            stats.vertices,
            meshes.len(),
            stats.buildings,
            stats.max_building_triangles,
            stats.windows,
            stats.lit_windows,
            stats.props,
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(
            stats.triangles <= TOWN_TRIANGLE_BUDGET,
            "{}",
            stats.triangles
        );
        // Budget 100 ms (debug); assert a generous 3x so a loaded CI machine does not flake.
        if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
            assert!(elapsed.as_secs_f64() < 0.3, "{elapsed:?}");
        }
        assert!(stats.max_building_triangles <= buildings::BUILDING_TRIANGLE_BUDGET);
        let lit = stats.lit_windows as f32 / stats.windows as f32;
        assert!((0.3..0.5).contains(&lit), "{lit}");
        let again = build_chunk_meshes(&place, &pal);
        assert_eq!(mesh_hash(&meshes), mesh_hash(&again));
        // Pinned fingerprint: change deliberately, in its own commit, with the reason.
        assert_eq!(
            mesh_hash(&meshes),
            0xF908_E7B3_4AA8_73B6,
            "{:#018X}",
            mesh_hash(&meshes)
        );
        // A different detail seed changes the meshes but not the layout.
        let mut other = place.clone();
        other.header.seed = 2;
        assert_ne!(
            mesh_hash(&meshes),
            mesh_hash(&build_chunk_meshes(&other, &pal))
        );
    }

    #[test]
    fn meshes_are_well_formed() {
        let place = Place::test_grid();
        let meshes = build_chunk_meshes(&place, &Palette::socal_1985());
        for c in &meshes {
            let m = &c.mesh;
            assert_eq!(m.indices.len() % 3, 0);
            assert!(
                m.vertices.len() <= 256 * 1024,
                "chunk {:?} {:?}: {}",
                c.chunk,
                c.kind,
                m.vertices.len()
            );
            assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
            for v in &m.vertices {
                assert!(v.pos.iter().all(|x| x.is_finite()));
                let n = math::V3::new(v.normal[0], v.normal[1], v.normal[2]);
                assert!((n.length() - 1.0).abs() < 1e-3, "{:?} normal {n:?}", c.kind);
            }
        }
        let kinds = [
            MeshKind::Ground,
            MeshKind::Road,
            MeshKind::Marking,
            MeshKind::Building,
            MeshKind::Prop,
        ];
        for k in kinds {
            assert!(meshes.iter().any(|c| c.kind == k), "{k:?}");
        }
    }
}
