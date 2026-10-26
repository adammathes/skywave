//! The procedural pipeline end to end ([`build_place`]): every reference seed gives a
//! valid, deterministic skeleton Place with its start on a road. Run with `--nocapture`
//! to see the numbers: `cargo test -p world --test place -- --nocapture`.

use std::time::Instant;

use place::{RegionId, SpawnKind};
use skywave_core::Seed;
use world::procgen::build_place;
use world::template::RegionTemplate;

const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");

fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(CALIFORNIA).expect("parse");
    t.validate().expect("validate");
    t
}

/// Pinned `Place::content_hash` of `build_place(seed, california_coast)` for seeds 1..=8.
///
/// Update rule: these move whenever any procedural stage (macro map, arterials, and later
/// streets, parcels and buildings) or the skeleton assembly changes. Regenerate with
/// `cargo test -p world --test place -- --nocapture` (it prints the table), check the
/// `?place=coast` screenshots, and commit the new values in their own commit whose
/// message says why they moved.
const GOLDEN: [u64; 8] = [
    0xF0F5_29C1_FA9F_F10D,
    0x70F1_11DC_466B_4DA3,
    0x6978_ED1D_3273_4FF9,
    0x72D1_A67B_5886_D9C5,
    0x2439_A551_6499_3527,
    0x7066_E28E_EDB7_7D89,
    0x4951_21EE_9380_5C29,
    0xD866_ED10_E167_084F,
];

#[test]
fn places_validate_and_are_deterministic() {
    let t = template();
    let mut got = Vec::new();
    for seed in 1..=8u64 {
        let start = Instant::now();
        let p = build_place(Seed(seed), &t);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(p.validate(), Ok(()), "seed {seed}");
        assert_eq!(p.header.region, RegionId::CaliforniaCoast);
        assert!(!p.buildings.is_empty() && !p.parcels.is_empty());
        let s = p.player_start().expect("a PlayerStart");
        assert_eq!(s.kind, SpawnKind::PlayerStart);
        let ground = p
            .terrain
            .height_at(&p.header, [s.pos[0], s.pos[2]])
            .unwrap();
        println!(
            "seed {seed}: {} nodes, {} edges, start ({:.0}, {:.1}, {:.0}) heading {:.2} over ground {ground:.1} m, {ms:.0} ms",
            p.roads.nodes.len(),
            p.roads.edges.len(),
            s.pos[0],
            s.pos[1],
            s.pos[2],
            s.heading
        );
        assert!((s.pos[1] - ground).abs() < 8.0, "seed {seed}");
        got.push(p.content_hash());
    }
    assert_eq!(build_place(Seed(1), &t).content_hash(), got[0]);
    let table = got
        .iter()
        .map(|h| {
            let l = format!("    0x{h:016X},");
            format!("{}_{}_{}_{},", &l[..10], &l[10..14], &l[14..18], &l[18..22])
        })
        .collect::<Vec<_>>()
        .join("\n");
    println!("place hashes:\n{table}");
    assert_eq!(
        got, GOLDEN,
        "place hashes moved; see the update rule on GOLDEN"
    );
}

/// Where a terrain point under a road is judged against that road: on its asphalt, away
/// from junctions (whose fans blend between roads) and from where its graded band overlaps
/// another stretch of road (crossings, the legs of a tight switchback).
struct Roads<'a> {
    place: &'a place::Place,
    net: &'a world::roads::Network,
    /// Junction centers and the radius their fans and trims cover.
    zones: Vec<(world::math::V2, f32)>,
}

impl Roads<'_> {
    /// The edge, profile segment and surface height of the road whose asphalt (plus `slack`)
    /// holds `q` with the height closest to `y`, if any, outside junctions.
    fn under(&self, q: world::math::V2, y: f32, slack: f32) -> Option<(usize, usize, f32)> {
        use world::math::segment_distance;
        if self.zones.iter().any(|&(c, r)| (q - c).length() < r) {
            return None;
        }
        let mut best: Option<(usize, usize, f32)> = None;
        for (i, e) in self.place.roads.edges.iter().enumerate() {
            let path = &self.net.paths[i];
            let (d, k) = (0..path.raw.len() - 1)
                .map(|k| (segment_distance(q, path.raw[k], path.raw[k + 1]).0, k))
                .fold((f32::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a });
            if d <= e.width_m * 0.5 + slack {
                let h = world::roads::profile_height(&path.raw, &path.raw_y, q);
                if best.is_none_or(|b| (h - y).abs() < (b.2 - y).abs()) {
                    best = Some((i, k, h));
                }
            }
        }
        best
    }

    /// Whether another stretch of `edge` far along it lies near `q` (a folded switchback).
    fn folded(&self, q: world::math::V2, edge: usize, seg: usize) -> bool {
        use world::math::segment_distance;
        let path = &self.net.paths[edge];
        let arc = |k: usize, t: f32| {
            let mut s = 0.0;
            for w in path.raw[..=k].windows(2) {
                s += (w[1] - w[0]).length();
            }
            s + t * (path.raw[k + 1] - path.raw[k]).length()
        };
        let s_q = arc(seg, segment_distance(q, path.raw[seg], path.raw[seg + 1]).1);
        (0..path.raw.len() - 1).any(|k| {
            let (d, t) = segment_distance(q, path.raw[k], path.raw[k + 1]);
            d < 20.0 && (arc(k, t) - s_q).abs() > 2.0 * d + 8.0
        })
    }
}

/// Meshes of a procedural place: well formed, within the chunk budget, roads on their
/// profiles, the ground graded under them, water planes only where there is water.
#[test]
fn coast_meshes_sit_roads_on_the_terrain() {
    use world::math::V2;
    use world::mesh::terrain::{TerrainGrid, UNDER_ROAD_M};
    use world::roads::{Network, DECK_CLEARANCE_M};
    use world::{build_with_stats, MeshKind, Palette, Pattern};
    let t = template();
    for seed in 1..=2u64 {
        let p = build_place(Seed(seed), &t);
        let start = Instant::now();
        let (meshes, stats) = build_with_stats(&p, &Palette::socal_1985());
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let net = Network::new(&p);
        let (grid, graded) = TerrainGrid::graded(&p, &net);
        let max_vertices = meshes.iter().map(|c| c.mesh.vertices.len()).max().unwrap();
        let count = |k: MeshKind| meshes.iter().filter(|c| c.kind == k).count();
        println!(
            "seed {seed}: {} meshes ({} ground, {} road, {} marking), {} triangles, {} vertices, max {max_vertices} per mesh, {ms:.0} ms",
            meshes.len(),
            count(MeshKind::Ground),
            count(MeshKind::Road),
            count(MeshKind::Marking),
            stats.triangles,
            stats.vertices,
        );
        assert_eq!(count(MeshKind::Ground), p.header.chunk_count());
        assert!(max_vertices <= 256 * 1024);
        let roads = Roads {
            place: &p,
            net: &net,
            zones: net
                .junctions
                .iter()
                .map(|j| {
                    let merges = j.merges.iter().map(|m| &m.end);
                    let r = j
                        .ends
                        .iter()
                        .chain(merges)
                        .map(|e| e.trim + e.hw)
                        .fold(0.0, f32::max);
                    (j.pos, r)
                })
                .collect(),
        };

        // Every asphalt vertex of a ribbon on its profile; the ground there never through
        // it (interpolated over 4 m cells it may dip a little under it at grade breaks).
        let (mut checked, mut worst_road) = (0usize, 0.0f32);
        let (mut worst_under, mut worst_over) = (0.0f32, f32::MIN);
        for c in &meshes {
            let m = &c.mesh;
            assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
            for v in &m.vertices {
                assert!(v.pos.iter().chain(&v.normal).all(|x| x.is_finite()));
            }
            if c.kind != MeshKind::Road {
                continue;
            }
            let asphalt = m
                .vertices
                .iter()
                .filter(|v| v.color[3] == Pattern::Asphalt as u8);
            for v in asphalt {
                let q = V2::new(v.pos[0], v.pos[2]);
                // Miter offsets put ribbon edges up to a little past the half width.
                let Some((edge, seg, y)) = roads.under(q, v.pos[1], 0.25) else {
                    continue;
                };
                checked += 1;
                let off = (v.pos[1] - y).abs();
                worst_road = worst_road.max(off);
                assert!(
                    off <= 0.05,
                    "seed {seed}: road vertex {:?} {off} m off its profile",
                    v.pos
                );
                let own = grid.triangle_at(q.x, q.y).iter().all(|&(ix, iz)| {
                    let (e, k) = graded[iz * grid.w + ix];
                    e as usize == edge && (k as usize).abs_diff(seg) <= 1
                });
                let g = grid.height_at(q.x, q.y);
                if !own || roads.folded(q, edge, seg) || y - g > DECK_CLEARANCE_M {
                    continue;
                }
                worst_under = worst_under.max(y - g);
                worst_over = worst_over.max(g - y);
                assert!(
                    g - y <= UNDER_ROAD_M,
                    "seed {seed}: ground {g} pokes through the road at {:?}",
                    v.pos
                );
                assert!(
                    y - g <= 0.25,
                    "seed {seed}: ground {g} far under the road at {:?}",
                    v.pos
                );
            }
        }
        assert!(checked > 1000);

        // Every terrain sample under a road's asphalt within 0.1 m of its profile.
        let (mut samples, mut worst_sample) = (0usize, 0.0f32);
        for iz in 0..grid.h {
            for ix in 0..grid.w {
                let (e, k) = graded[iz * grid.w + ix];
                if e == u32::MAX {
                    continue;
                }
                let q = grid.pos(ix, iz);
                let g = grid.sample(ix, iz);
                let Some((edge, seg, y)) = roads.under(q, g, 0.0) else {
                    continue;
                };
                if edge != e as usize
                    || (k as usize).abs_diff(seg) > 1
                    || roads.folded(q, edge, seg)
                {
                    continue;
                }
                samples += 1;
                worst_sample = worst_sample.max((g - y).abs());
                assert!(
                    (g - y).abs() <= 0.1,
                    "seed {seed}: terrain sample {g} at {q:?} under a road at {y}"
                );
            }
        }
        println!(
            "seed {seed}: {checked} asphalt vertices, worst {worst_road:.3} m off profile; ground \
             under them {worst_under:.3} m under to {worst_over:.3} m over; {samples} terrain \
             samples under roads, worst {worst_sample:.3} m off"
        );
        assert!(samples > 1000);

        // Water planes only in chunks with water, and in every one of them.
        let res = usize::from(p.terrain.res_per_chunk);
        let cw = usize::from(p.header.size_chunks[0]) * res;
        for c in meshes.iter().filter(|c| c.kind == MeshKind::Ground) {
            let wet = c
                .mesh
                .vertices
                .iter()
                .any(|v| v.color[3] == Pattern::Water as u8);
            let [cx, cz] = [c.chunk[0] as usize, c.chunk[1] as usize];
            let has = (0..res).any(|j| {
                (0..res).any(|i| {
                    p.zones.cells[(cz * res + j) * cw + cx * res + i] == place::ZoneId::Water
                })
            });
            assert_eq!(wet, has, "seed {seed}: chunk {:?}", c.chunk);
        }
    }
}

/// The collider of a procedural place stands on the same ground the meshes draw: sloped
/// roads at their profile, the graded terrain on its triangles, and decks above the roads
/// under them.
#[test]
fn collider_follows_slopes_and_decks() {
    use vehicle::Ground;
    use world::mesh::terrain::TerrainGrid;
    use world::roads::{Network, DECK_CLEARANCE_M};
    use world::WorldCollider;
    let t = template();
    let p = build_place(Seed(1), &t);
    let net = Network::new(&p);
    let grid = TerrainGrid::new(&p, &net);
    let c = WorldCollider::build(&p);
    let down = [0.0, -1.0, 0.0];
    let probe = |x: f32, y: f32, z: f32| {
        let hit = c
            .probe([x, y, z], down, 50.0)
            .expect("ground under the probe");
        y - hit.t
    };
    // Where two streets that do not meet overlap at different heights (closer than an
    // overpass clears), the probe stands on whichever slab it meets first (an overlap from
    // stage 4: skipped and counted here).
    let boxes: Vec<[f32; 4]> = p
        .roads
        .edges
        .iter()
        .map(|e| {
            let r = 0.5 * e.width_m + 3.5;
            e.profile
                .iter()
                .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, q| {
                    [
                        b[0].min(q[0] - r),
                        b[1].min(q[2] - r),
                        b[2].max(q[0] + r),
                        b[3].max(q[2] + r),
                    ]
                })
        })
        .collect();
    let overlaps = |i: usize, q: world::math::V3| {
        use world::math::{segment_distance, V2};
        use world::procgen::arterials::OVERPASS_LIFT_M;
        let v = V2::new(q.x, q.z);
        let own = &p.roads.edges[i];
        p.roads.edges.iter().enumerate().any(|(j, e)| {
            let b = boxes[j];
            let meets = [e.a, e.b].iter().any(|n| *n == own.a || *n == own.b);
            j != i
                && !meets
                && v.x >= b[0]
                && v.x <= b[2]
                && v.y >= b[1]
                && v.y <= b[3]
                && e.profile.windows(2).any(|w| {
                    let (a, c) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
                    let (d, u) = segment_distance(v, a, c);
                    let dy = (w[0][1] + (w[1][1] - w[0][1]) * u - q.y).abs();
                    d < 0.5 * e.width_m + 3.5 && dy > 0.1 && dy < OVERPASS_LIFT_M - 2.0
                })
        })
    };
    // Road centerlines on grades of 3% and more.
    let (mut roads, mut worst_road, mut decks, mut overlapped) = (0usize, 0.0f32, 0usize, 0usize);
    for (i, path) in net.paths.iter().enumerate() {
        let (ea, eb) = net.ends(i);
        let mut s = ea.trim + 2.0;
        while s < path.len() - eb.trim - 2.0 {
            let q = path.p(s, 0.0, 0.0);
            let ahead = path.p(s + 1.0, 0.0, 0.0);
            s += 7.0;
            if (ahead.y - q.y).abs() < 0.03 {
                continue;
            }
            if overlaps(i, q) {
                overlapped += 1;
                continue;
            }
            if q.y - grid.height_at(q.x, q.z) > DECK_CLEARANCE_M {
                decks += 1;
            }
            let y = probe(q.x, q.y + 1.5, q.z);
            roads += 1;
            worst_road = worst_road.max((y - q.y).abs());
            assert!(
                (y - q.y).abs() <= 0.1,
                "road edge {i} at {q:?}: collider {y}"
            );
        }
    }
    // Terrain away from roads (their sidewalks and curbs too), on slopes of 20% and more.
    let near_road = |v: world::math::V2| {
        p.roads.edges.iter().enumerate().any(|(j, e)| {
            let b = boxes[j];
            v.x >= b[0]
                && v.x <= b[2]
                && v.y >= b[1]
                && v.y <= b[3]
                && e.profile.windows(2).any(|w| {
                    let (a, c) = (
                        world::math::V2::new(w[0][0], w[0][2]),
                        world::math::V2::new(w[1][0], w[1][2]),
                    );
                    world::math::segment_distance(v, a, c).0 < 0.5 * e.width_m + 3.5
                })
        })
    };
    let (mut slopes, mut worst_ground) = (0usize, 0.0f32);
    for iz in (2..grid.h - 2).step_by(5) {
        for ix in (2..grid.w - 2).step_by(5) {
            let q = grid.pos(ix, iz) + world::math::V2::new(1.3, 2.1);
            let g = grid.height_at(q.x, q.y);
            let gx = grid.height_at(q.x + 2.0, q.y) - grid.height_at(q.x - 2.0, q.y);
            if gx.abs() < 0.8 || g < p.terrain.water_level || near_road(q) {
                continue;
            }
            let y = probe(q.x, g + 20.0, q.y);
            if (y - g).abs() > 0.3 {
                continue; // a deck above the terrain here
            }
            slopes += 1;
            worst_ground = worst_ground.max((y - g).abs());
            assert!((y - g).abs() <= 0.1, "terrain at {q:?}: {g}, collider {y}");
        }
    }
    println!(
        "{roads} sloped road points (worst {worst_road:.3} m, {decks} on decks, {overlapped} on overlapping streets skipped), {slopes} hillside points (worst {worst_ground:.3} m)"
    );
    assert!(
        overlapped * 20 < roads,
        "{overlapped} of {roads} on overlapping streets"
    );
    assert!(roads > 200 && slopes > 200);
    // A deck: the probe from above stands on it, from under it on what is below.
    let mut deck_points = 0;
    for (i, path) in net.paths.iter().enumerate() {
        let (ea, eb) = net.ends(i);
        let mut s = ea.trim;
        while s < path.len() - eb.trim {
            let q = path.p(s, 0.0, 0.0);
            s += 4.0;
            let g = grid.height_at(q.x, q.z);
            if q.y - g > DECK_CLEARANCE_M + 2.0 {
                deck_points += 1;
                assert!((probe(q.x, q.y + 1.0, q.z) - q.y).abs() <= 0.1);
                let below = probe(q.x, g + 1.0, q.z);
                assert!(
                    below < q.y - DECK_CLEARANCE_M,
                    "edge {i}: under the deck at {q:?}: {below}"
                );
            }
        }
    }
    println!("{deck_points} deck points");
    assert!(deck_points >= 3, "seed 1 has an overpass");
}

/// Built chunk by chunk and stage by stage (as the app streams it), a terrain place gives
/// each chunk exactly its slice of the whole build: terrain, water, roads and decks.
#[test]
fn coast_chunks_build_as_slices_of_the_whole_build() {
    use world::{build_chunk_meshes, mesh_hash, ChunkMesh, ChunkPlan, Palette, Stage};
    let p = build_place(Seed(1), &template());
    let pal = Palette::socal_1985();
    let all = build_chunk_meshes(&p, &pal);
    let plan = ChunkPlan::new(&p, &pal);
    let [lo, hi] = plan.chunk_range();
    assert_eq!((lo, hi), ([0, 0], [15, 15]), "the place's chunks");
    let t0 = Instant::now();
    let mut total = 0;
    for z in lo[1]..=hi[1] {
        for x in lo[0]..=hi[0] {
            let mut staged: Vec<ChunkMesh> = Vec::new();
            for s in Stage::ALL {
                staged.extend(plan.build_stage(&p, &pal, [x, z], s));
            }
            staged.sort_by_key(|c| c.kind);
            let slice: Vec<ChunkMesh> = all.iter().filter(|c| c.chunk == [x, z]).cloned().collect();
            assert_eq!(mesh_hash(&staged), mesh_hash(&slice), "chunk {x}, {z}");
            total += staged.len();
        }
    }
    println!(
        "{total} chunk meshes staged one by one in {:.0} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(total, all.len());
}

/// The far terrain of a procedural place covers it in one coarse mesh within its triangle
/// budget and lies under the graded ground everywhere (by the drop at least), so the
/// streamed chunks always draw over it.
#[test]
fn far_terrain_lies_under_the_ground() {
    use world::mesh::terrain::{TerrainGrid, FAR_DROP_M};
    use world::roads::Network;
    use world::{ChunkPlan, Palette, Pattern};
    let pal = Palette::socal_1985();
    for seed in 1..=2u64 {
        let p = build_place(Seed(seed), &template());
        let far = ChunkPlan::new(&p, &pal).far_terrain(&p, &pal).unwrap();
        let grid = TerrainGrid::new(&p, &Network::new(&p));
        let tris = far.indices.len() / 3;
        assert!(
            (8192..=10_000).contains(&tris),
            "seed {seed}: {tris} triangles"
        );
        // Every fine sample under a ground triangle of the far mesh, against it.
        let (mut checked, mut closest) = (0usize, f32::INFINITY);
        for t in far.indices.chunks_exact(3) {
            let v = [0, 1, 2].map(|k| far.vertices[t[k] as usize]);
            if v.iter().any(|v| v.color[3] == Pattern::Water as u8) {
                continue;
            }
            let [a, b, c] = v.map(|v| v.pos);
            let lo = [a[0].min(b[0]).min(c[0]), a[2].min(b[2]).min(c[2])];
            let hi = [a[0].max(b[0]).max(c[0]), a[2].max(b[2]).max(c[2])];
            let den = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
            let (ix0, iz0) = ((lo[0] / grid.cell) as usize, (lo[1] / grid.cell) as usize);
            let (ix1, iz1) = ((hi[0] / grid.cell) as usize, (hi[1] / grid.cell) as usize);
            for iz in iz0..=iz1.min(grid.h - 1) {
                for ix in ix0..=ix1.min(grid.w - 1) {
                    let q = grid.pos(ix, iz);
                    let l1 = ((b[2] - c[2]) * (q.x - c[0]) + (c[0] - b[0]) * (q.y - c[2])) / den;
                    let l2 = ((c[2] - a[2]) * (q.x - c[0]) + (a[0] - c[0]) * (q.y - c[2])) / den;
                    let l3 = 1.0 - l1 - l2;
                    if l1 < -1e-4 || l2 < -1e-4 || l3 < -1e-4 {
                        continue;
                    }
                    let y = l1 * a[1] + l2 * b[1] + l3 * c[1];
                    let under = grid.sample(ix, iz) - y;
                    checked += 1;
                    closest = closest.min(under);
                    assert!(
                        under >= FAR_DROP_M - 0.01,
                        "seed {seed}: far terrain {y} only {under} m under the ground at {q:?}"
                    );
                }
            }
        }
        println!(
            "seed {seed}: far terrain {tris} triangles, {checked} samples checked, closest \
             {closest:.3} m under"
        );
        assert!(checked > 200_000, "{checked}");
    }
}
