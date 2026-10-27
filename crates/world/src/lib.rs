//! Turns a [`place::Place`] into chunked, vertex-colored low-poly meshes at the fidelity
//! bar of `docs/RENDERING.md`: roads with curbs, gutters, slabbed sidewalks, crosswalks and
//! lane paint; buildings with plinths, inset windows, storefronts, awnings, signs and
//! parapets; street lights, trees, palms, hydrants, benches, signals and parked cars.
//!
//! Everything is a pure function of the Place (and its `header.seed` for detail
//! variation): same input, bit-identical vertices on every platform. [`collide`] builds the
//! static shapes the car collides with from the same layout.
//!
//! [`procgen`] is the procedural producer, driven by a [`template::RegionTemplate`]; its
//! last stage ([`procgen::detail`]) lists a place's props and lights in the Place, which
//! the meshes, the collider and the app read from there.
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
pub mod signs;
pub mod template;

use alloc::vec::Vec;

use place::{Place, RegionId, ZoneId, SIDEWALK_WIDTH_M};

pub use collide::{resolve, Impact, WorldCollider};
pub use mesh::terrain::TerrainGrid;
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

/// Which chunks a [`MeshSet`] keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keep {
    All,
    Chunk([i32; 2]),
    Nothing,
}

/// Collects meshes by chunk and kind while building. A set may keep only one chunk's
/// meshes ([`build_chunk`]); writes to the others go to a scratch mesh that is cleared on
/// each use, and builders may skip work [`MeshSet::wants`] says nobody keeps. Either way a
/// kept mesh receives the same writes in the same order, so it comes out bit-identical.
pub struct MeshSet {
    chunk_size: f32,
    keep: Keep,
    meshes: Vec<ChunkMesh>,
    sink: WorldMesh,
}

impl MeshSet {
    fn new(chunk_size: f32, keep: Keep) -> MeshSet {
        MeshSet {
            chunk_size: if chunk_size > 0.0 { chunk_size } else { 128.0 },
            keep,
            meshes: Vec::new(),
            sink: WorldMesh::default(),
        }
    }

    /// Chunk coordinates of the ground point `p`.
    pub fn chunk_of(&self, p: V2) -> [i32; 2] {
        [
            floor(p.x / self.chunk_size) as i32,
            floor(p.y / self.chunk_size) as i32,
        ]
    }

    /// Whether writes for the chunk containing `p` are kept.
    pub fn wants(&self, p: V2) -> bool {
        match self.keep {
            Keep::All => true,
            Keep::Chunk(c) => self.chunk_of(p) == c,
            Keep::Nothing => false,
        }
    }

    /// The mesh of `kind` in the chunk containing `p`.
    pub fn at(&mut self, p: V2, kind: MeshKind) -> &mut WorldMesh {
        if !self.wants(p) {
            self.sink.vertices.clear();
            self.sink.indices.clear();
            return &mut self.sink;
        }
        let chunk = self.chunk_of(p);
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

    /// Vertices in the meshes kept so far.
    pub fn kept_vertices(&self) -> usize {
        self.meshes.iter().map(|c| c.mesh.vertices.len()).sum()
    }

    /// The non-empty meshes, sorted by kind then chunk.
    fn finish(self) -> Vec<ChunkMesh> {
        let mut meshes = self.meshes;
        meshes.retain(|c| !c.mesh.indices.is_empty());
        meshes.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.chunk.cmp(&b.chunk)));
        meshes
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

/// A step of building a chunk, in the order a whole build runs them; each writes only its
/// own kinds of mesh, so a streamer can stop between them.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// A terrain place's graded terrain and water; the test town's apron, lots, parks and
    /// parking lots ([`MeshKind::Ground`]).
    Ground = 0,
    /// Roads, junctions and lane paint ([`MeshKind::Road`], [`MeshKind::Marking`]).
    Roads = 1,
    /// Buildings with their walks and drives ([`MeshKind::Building`]).
    Buildings = 2,
    /// Street furniture, trees and parked cars ([`MeshKind::Prop`]).
    Props = 3,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Ground, Stage::Roads, Stage::Buildings, Stage::Props];
}

/// What every chunk of a place shares, worked out once per place: the road network, the
/// blocks, every building's plan and every prop's placement. Building a chunk from it
/// ([`ChunkPlan::build_stage`]) only meshes what lies in that chunk.
pub struct ChunkPlan {
    seed: u64,
    chunk_size: f32,
    net: roads::Network,
    side_zone: Vec<[Option<ZoneId>; 2]>,
    /// The test town's blocks (none on a terrain place).
    blocks: Vec<ground::Block>,
    /// A terrain place's ground with the roads graded in, and its cell colors.
    terrain: Option<(TerrainGrid, mesh::terrain::CellColors)>,
    plans: Vec<(Vec<FacadeEnv>, buildings::BuildingPlan)>,
    /// Each building's footprint centroid: its meshes are filed under its chunk.
    centroids: Vec<V2>,
    /// Per building: the ground height its floor stands at and how far the ground falls
    /// below it ([`buildings::footing`]); zeros on the flat test town.
    footings: Vec<(f32, f32)>,
    fronts: Vec<usize>,
    /// Every prop meshed into the chunks, in build order: on the test town the lots'
    /// parked cars, the house sites', the streets'; on a terrain place its `Props` table
    /// but for the kinds the app instances ([`props::instanced`]).
    props: Vec<props::Prop>,
    kit: props::PropKit,
    /// Chunks that may hold a mesh, `[min, max]` inclusive.
    chunk_range: [[i32; 2]; 2],
}

impl ChunkPlan {
    pub fn new(place: &Place, palette: &Palette) -> ChunkPlan {
        let seed = place.header.seed;
        let net = roads::Network::new(place);
        let side_zone = side_zones(place, &net);
        let blocks = if is_terrain_place(place) {
            Vec::new()
        } else {
            ground::blocks(&net)
        };

        // Plan every building first: the ground needs to know where strip malls face. A
        // terrain place's buildings get the detailing's signage.
        let lookup = Lookup::new(place, &net);
        let terrain_place = is_terrain_place(place);
        let plans: Vec<(Vec<FacadeEnv>, buildings::BuildingPlan)> = place
            .buildings
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let env = lookup.facade_envs(i);
                let door_pick = skywave_core::hash64(seed, &[0x444F_4F52, i as u64]) as u32;
                let mut plan = buildings::plan_building(b, &env, door_pick);
                if terrain_place {
                    plan.signage = Some(buildings::signage(seed, i as u32));
                }
                (env, plan)
            })
            .collect();
        let fronts: Vec<usize> = plans.iter().map(|(_, p)| p.frontage).collect();
        let centroids = place
            .buildings
            .iter()
            .map(|b| ground::centroid(&ground::to_v2(&b.footprint)))
            .collect();

        // The test town lies on flat lot pads in an apron; a terrain place on its heightfield
        // with the roads graded in.
        let terrain = is_terrain_place(place).then(|| {
            let g = TerrainGrid::new(place, &net);
            let colors = mesh::terrain::CellColors::new(place, &g, palette, seed);
            (g, colors)
        });

        let footings: Vec<(f32, f32)> = match &terrain {
            Some((g, _)) => place
                .buildings
                .iter()
                .map(|b| buildings::footing(&b.footprint, |x, z| g.height_at(x, z)))
                .collect(),
            None => alloc::vec![(0.0, 0.0); place.buildings.len()],
        };

        // The props, meshing nothing. A terrain place's are in its `Props` table. The test
        // town's: the lots' parked cars, the house sites' props (their walks go to a
        // scratch mesh), then the streets' and the parks'.
        let mut none = MeshSet::new(place.header.chunk_size_m, Keep::Nothing);
        let mut props = if terrain.is_none() {
            ground::build(place, &blocks, &fronts, palette, seed, &mut none)
        } else {
            place
                .props
                .items
                .iter()
                .filter(|p| !props::instanced(p.kind))
                .map(|p| props::from_place(p, seed))
                .collect()
        };
        let mut scratch = WorldMesh::default();
        for (i, (b, (env, plan))) in place.buildings.iter().zip(&plans).enumerate() {
            if terrain.is_some() {
                break;
            }
            scratch.vertices.clear();
            scratch.indices.clear();
            props.extend(buildings::house_site(
                &mut scratch,
                b,
                plan,
                env,
                palette,
                seed,
                i as u32,
            ));
        }
        if terrain.is_none() {
            let parks: Vec<Vec<V2>> = blocks
                .iter()
                .filter(|b| ground::is_park(place, b))
                .map(|b| b.poly.clone())
                .collect();
            props.extend(props::place(place, &net, &side_zone, &parks, seed));
        }

        // The place's chunks and the terrain apron's.
        let o = place.header.chunk_origin;
        let n = place.header.size_chunks;
        let mut lo = [i32::from(o[0]), i32::from(o[1])];
        let mut hi = [
            lo[0] + i32::from(n[0].max(1)) - 1,
            lo[1] + i32::from(n[1].max(1)) - 1,
        ];
        let apron = match terrain {
            None => ground::apron_bounds(place),
            Some(_) => None,
        };
        if let Some((a, b)) = apron {
            let (a, b) = (none.chunk_of(a), none.chunk_of(b));
            for i in 0..2 {
                lo[i] = lo[i].min(a[i]);
                hi[i] = hi[i].max(b[i]);
            }
        }
        ChunkPlan {
            seed,
            chunk_size: none.chunk_size,
            net,
            side_zone,
            blocks,
            terrain,
            plans,
            centroids,
            footings,
            fronts,
            props,
            kit: props::PropKit::new(),
            chunk_range: [lo, hi],
        }
    }

    /// Every prop of the place, in build order.
    pub fn props(&self) -> &[props::Prop] {
        &self.props
    }

    /// Chunks that may hold a mesh, `[min, max]` inclusive; every other chunk is empty.
    pub fn chunk_range(&self) -> [[i32; 2]; 2] {
        self.chunk_range
    }

    /// Chunk edge, meters.
    pub fn chunk_size(&self) -> f32 {
        self.chunk_size
    }

    /// The chunk holding ground point `p` (`[x, z]`), as the builders file meshes.
    pub fn chunk_of(&self, p: [f32; 2]) -> [i32; 2] {
        [
            floor(p[0] / self.chunk_size) as i32,
            floor(p[1] / self.chunk_size) as i32,
        ]
    }

    /// A terrain place's far terrain ([`mesh::terrain::build_far`]): the whole place's
    /// ground and water as one coarse mesh under the chunks' own; `None` for the test town.
    pub fn far_terrain(&self, place: &Place, palette: &Palette) -> Option<WorldMesh> {
        let (g, colors) = self.terrain.as_ref()?;
        Some(mesh::terrain::build_far(
            place, g, colors, palette, self.seed,
        ))
    }

    /// The meshes `stage` writes in `chunk`, sorted by kind (empty ones left out): the
    /// same as [`build_chunk_meshes`] gives for that chunk and those kinds.
    pub fn build_stage(
        &self,
        place: &Place,
        palette: &Palette,
        chunk: [i32; 2],
        stage: Stage,
    ) -> Vec<ChunkMesh> {
        let mut out = MeshSet::new(self.chunk_size, Keep::Chunk(chunk));
        self.run(place, palette, stage, &mut out, None);
        out.finish()
    }

    /// One piece of [`ChunkPlan::build_stage`], so a streamer can stop inside a long stage:
    /// the roads, buildings or props of `chunk` from item `*cursor` on (0 starts the stage),
    /// up to the item that brings the piece to `max_vertices` vertices or more; the ground
    /// is always one piece. Returns the piece's meshes, sorted by kind, and whether the
    /// stage is finished; otherwise `*cursor` is where the next piece starts. The pieces'
    /// meshes of each kind, appended in order, are that kind's mesh from
    /// [`ChunkPlan::build_stage`], vertex for vertex.
    pub fn build_stage_piece(
        &self,
        place: &Place,
        palette: &Palette,
        chunk: [i32; 2],
        stage: Stage,
        cursor: &mut usize,
        max_vertices: usize,
    ) -> (Vec<ChunkMesh>, bool) {
        let mut out = MeshSet::new(self.chunk_size, Keep::Chunk(chunk));
        let next = self.run_from(place, palette, stage, &mut out, None, *cursor, max_vertices);
        *cursor = next.unwrap_or(0);
        (out.finish(), next.is_none())
    }

    fn run(
        &self,
        place: &Place,
        palette: &Palette,
        stage: Stage,
        out: &mut MeshSet,
        stats: Option<&mut BuildStats>,
    ) {
        self.run_from(place, palette, stage, out, stats, 0, usize::MAX);
    }

    /// Runs `stage` from item `from` on until what `out` keeps reaches `max_vertices`;
    /// returns the item to resume from, `None` when the stage is done.
    #[allow(clippy::too_many_arguments)]
    fn run_from(
        &self,
        place: &Place,
        palette: &Palette,
        stage: Stage,
        out: &mut MeshSet,
        mut stats: Option<&mut BuildStats>,
        from: usize,
        max_vertices: usize,
    ) -> Option<usize> {
        let seed = self.seed;
        match stage {
            Stage::Ground => match &self.terrain {
                // Its parked cars are in `props` already.
                None => {
                    ground::build(place, &self.blocks, &self.fronts, palette, seed, out);
                }
                Some((g, colors)) => mesh::terrain::build(place, g, colors, palette, seed, out),
            },
            Stage::Roads => {
                let terrain = self.terrain.as_ref().map(|(g, _)| g);
                let (net, side_zone) = (&self.net, &self.side_zone);
                return roads::build_from(
                    place,
                    net,
                    side_zone,
                    palette,
                    seed,
                    terrain,
                    out,
                    from,
                    max_vertices,
                );
            }
            Stage::Buildings => {
                let all = place.buildings.iter().zip(&self.plans).enumerate();
                for (i, (b, (env, plan))) in all.skip(from) {
                    let c = self.centroids[i];
                    if !out.wants(c) {
                        continue;
                    }
                    let m = out.at(c, MeshKind::Building);
                    let (base, drop) = self.footings[i];
                    let mark = m.vertices.len();
                    let s = buildings::build_on(m, b, plan, palette, seed, i as u32, drop);
                    // Its props are in `props` already.
                    buildings::house_site(m, b, plan, env, palette, seed, i as u32);
                    if base != 0.0 {
                        for v in &mut m.vertices[mark..] {
                            v.pos[1] += base;
                        }
                    }
                    if let Some(stats) = stats.as_deref_mut() {
                        stats.buildings += 1;
                        stats.windows += s.windows;
                        stats.lit_windows += s.lit_windows;
                        stats.max_building_triangles =
                            stats.max_building_triangles.max(s.triangles);
                    }
                    if out.kept_vertices() >= max_vertices {
                        return Some(i + 1);
                    }
                }
            }
            Stage::Props => {
                for (i, p) in self.props.iter().enumerate().skip(from) {
                    if out.wants(p.pos) {
                        props::build(out.at(p.pos, MeshKind::Prop), p, palette, &self.kit);
                        if out.kept_vertices() >= max_vertices {
                            return Some(i + 1);
                        }
                    }
                }
                if let Some(stats) = stats {
                    stats.props = self.props.len();
                }
            }
        }
        None
    }
}

/// Builds all meshes of `place`, grouped by chunk and [`MeshKind`], sorted by kind then
/// chunk.
pub fn build_chunk_meshes(place: &Place, palette: &Palette) -> Vec<ChunkMesh> {
    build_with_stats(place, palette).0
}

/// The meshes of one chunk, sorted by kind: exactly the ones [`build_chunk_meshes`] gives
/// for it. To build many chunks, make one [`ChunkPlan`] and use
/// [`ChunkPlan::build_stage`].
pub fn build_chunk(place: &Place, palette: &Palette, chunk: [i32; 2]) -> Vec<ChunkMesh> {
    let plan = ChunkPlan::new(place, palette);
    let mut out = MeshSet::new(plan.chunk_size, Keep::Chunk(chunk));
    for stage in Stage::ALL {
        plan.run(place, palette, stage, &mut out, None);
    }
    out.finish()
}

/// [`build_chunk_meshes`] plus totals.
pub fn build_with_stats(place: &Place, palette: &Palette) -> (Vec<ChunkMesh>, BuildStats) {
    let plan = ChunkPlan::new(place, palette);
    let mut out = MeshSet::new(plan.chunk_size, Keep::All);
    let mut stats = BuildStats::default();
    for stage in Stage::ALL {
        plan.run(place, palette, stage, &mut out, Some(&mut stats));
    }
    let meshes = out.finish();
    for c in &meshes {
        let t = c.mesh.triangle_count();
        stats.triangles += t;
        stats.vertices += c.mesh.vertices.len();
        stats.triangles_by_kind[c.kind as usize] += t;
    }
    (meshes, stats)
}

/// Whether `place` stands on its own heightfield (a procedural or compiled place) rather
/// than on the hand-laid test town's flat lot pads at `y = 0`.
pub fn is_terrain_place(place: &Place) -> bool {
    place.header.region != RegionId::TestTown
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
/// A road segment for facade lookups: ends, bounds, half width plus sidewalk, edge index.
type RoadSeg = (V2, V2, (V2, V2), f32, usize);

struct Lookup<'a> {
    place: &'a Place,
    footprints: Vec<Vec<V2>>,
    boxes: Vec<(V2, V2)>,
    parcels: Vec<(Vec<V2>, (V2, V2))>,
    /// Road segments: ends, bounds, half width plus sidewalk, edge index. One per edge
    /// (node to node) on the test town; one per profile segment on a terrain place, whose
    /// streets curve.
    roads: Vec<RoadSeg>,
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
        let terrain = is_terrain_place(place);
        let mut roads = Vec::new();
        for (k, e) in place.roads.edges.iter().enumerate() {
            let (end, _) = net.ends(k);
            let sw = if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
            let half = end.hw + sw;
            if terrain && e.profile.len() > 2 {
                for w in e.profile.windows(2) {
                    let (ra, rb) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
                    roads.push((ra, rb, bounds(&[ra, rb]), half, k));
                }
            } else {
                let ra = V2::from_array(nodes[e.a as usize].pos);
                let rb = V2::from_array(nodes[e.b as usize].pos);
                roads.push((ra, rb, bounds(&[ra, rb]), half, k));
            }
        }
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
                for &(ra, rb, (lo, hi), half, k) in &self.roads {
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

    /// Building chunk by chunk (and stage by stage) gives each chunk exactly its slice of the
    /// whole build, hash for hash, and nothing outside the plan's chunk range.
    #[test]
    fn a_chunk_build_is_the_slice_of_the_whole_build() {
        let place = Place::test_grid();
        let pal = Palette::socal_1985();
        let all = build_chunk_meshes(&place, &pal);
        let plan = ChunkPlan::new(&place, &pal);
        let [lo, hi] = plan.chunk_range();
        let mut chunks: Vec<[i32; 2]> = all.iter().map(|c| c.chunk).collect();
        chunks.sort_unstable();
        chunks.dedup();
        for c in &chunks {
            assert!((lo[0]..=hi[0]).contains(&c[0]) && (lo[1]..=hi[1]).contains(&c[1]));
        }
        let t0 = Instant::now();
        let mut total = 0;
        let (mut per_stage, mut worst) = ([0.0f64; 4], [0.0f64; 4]);
        for z in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                let mut staged: Vec<ChunkMesh> = Vec::new();
                for (k, &s) in Stage::ALL.iter().enumerate() {
                    let t = Instant::now();
                    staged.extend(plan.build_stage(&place, &pal, [x, z], s));
                    let dt = t.elapsed().as_secs_f64() * 1000.0;
                    per_stage[k] += dt;
                    worst[k] = worst[k].max(dt);
                }
                staged.sort_by_key(|c| c.kind);
                let slice: Vec<ChunkMesh> =
                    all.iter().filter(|c| c.chunk == [x, z]).cloned().collect();
                assert_eq!(mesh_hash(&staged), mesh_hash(&slice), "chunk {x}, {z}");
                assert_eq!(staged.len(), slice.len());
                total += staged.len();
            }
        }
        std::println!(
            "{} chunks with meshes in {:?}..={:?}, staged one by one in {:.1} ms; ms per stage total {:.1?}, worst chunk {:.2?}",
            chunks.len(),
            lo,
            hi,
            t0.elapsed().as_secs_f64() * 1000.0,
            per_stage,
            worst
        );
        assert_eq!(total, all.len());
        let c = chunks[chunks.len() / 2];
        let one = build_chunk(&place, &pal, c);
        let slice: Vec<ChunkMesh> = all.iter().filter(|m| m.chunk == c).cloned().collect();
        assert_eq!(one, slice);
    }

    /// A stage built in pieces gives, kind by kind, the pieces' meshes one after the other:
    /// exactly the stage's meshes.
    #[test]
    fn a_stage_in_pieces_is_the_stage() {
        let place = Place::test_grid();
        let pal = Palette::socal_1985();
        let plan = ChunkPlan::new(&place, &pal);
        for chunk in [[0, 0], [-1, 1], [0, -2], [2, 1]] {
            for stage in Stage::ALL {
                let whole = plan.build_stage(&place, &pal, chunk, stage);
                let mut joined: Vec<ChunkMesh> = Vec::new();
                let (mut cursor, mut pieces) = (0, 0);
                loop {
                    let (meshes, done) =
                        plan.build_stage_piece(&place, &pal, chunk, stage, &mut cursor, 1500);
                    pieces += 1;
                    for m in meshes {
                        match joined.iter_mut().find(|j| j.kind == m.kind) {
                            Some(j) => {
                                let base = j.mesh.vertices.len() as u32;
                                j.mesh.vertices.extend_from_slice(&m.mesh.vertices);
                                j.mesh
                                    .indices
                                    .extend(m.mesh.indices.iter().map(|i| i + base));
                            }
                            None => joined.push(m),
                        }
                    }
                    if done {
                        break;
                    }
                }
                assert_eq!(cursor, 0, "a finished stage leaves the cursor at its start");
                joined.sort_by_key(|c| c.kind);
                assert_eq!(
                    joined, whole,
                    "chunk {chunk:?} {stage:?} in {pieces} pieces"
                );
                if stage == Stage::Buildings && chunk == [0, 0] {
                    assert!(pieces > 3, "{pieces} pieces");
                }
            }
        }
    }

    #[test]
    fn meshes_are_well_formed() {
        let place = Place::test_grid();
        let meshes = build_chunk_meshes(&place, &Palette::socal_1985());
        for c in &meshes {
            let m = &c.mesh;
            assert_eq!(m.indices.len() % 3, 0);
            assert!(
                m.vertices.len() <= 384 * 1024, // render::MAX_CHUNK_VERTICES
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
