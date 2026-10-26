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
    0xA0F0_BBEB_2D7A_A686,
    0x66CE_8A10_8A94_E6E9,
    0x9E12_34CF_3A08_5D04,
    0x5B8F_D268_4165_4779,
    0x5CFE_5034_66FC_0643,
    0x6E81_583E_7862_CEC0,
    0x1982_A601_BC31_3A89,
    0x942B_77E7_007D_49D4,
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
        assert!(p.buildings.is_empty() && p.parcels.is_empty());
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
                    let r = j.ends.iter().map(|e| e.trim + e.hw).fold(0.0, f32::max);
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
    // Road centerlines on grades of 3% and more.
    let (mut roads, mut worst_road, mut decks) = (0usize, 0.0f32, 0usize);
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
    // Terrain away from roads, on slopes of 20% and more.
    let (mut slopes, mut worst_ground) = (0usize, 0.0f32);
    for iz in (2..grid.h - 2).step_by(5) {
        for ix in (2..grid.w - 2).step_by(5) {
            let q = grid.pos(ix, iz) + world::math::V2::new(1.3, 2.1);
            let g = grid.height_at(q.x, q.y);
            let gx = grid.height_at(q.x + 2.0, q.y) - grid.height_at(q.x - 2.0, q.y);
            if gx.abs() < 0.8 || g < p.terrain.water_level {
                continue;
            }
            let y = probe(q.x, g + 20.0, q.y);
            if (y - g).abs() > 0.3 {
                continue; // a road or a deck above the terrain here
            }
            slopes += 1;
            worst_ground = worst_ground.max((y - g).abs());
            assert!((y - g).abs() <= 0.1, "terrain at {q:?}: {g}, collider {y}");
        }
    }
    println!(
        "{roads} sloped road points (worst {worst_road:.3} m, {decks} on decks), {slopes} hillside points (worst {worst_ground:.3} m)"
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
