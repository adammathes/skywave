//! Grade-separated highways and channels in authored layouts (`docs/LAYOUTS.md`), on a small
//! test town: the avenues pass over the freeway without meeting it, one of them joined to it
//! by a diamond of ramps; roads bridge the dry wash and the creek; the whole Place validates.

use place::{RoadClass, RoadGraph, ZoneId};
use skywave_core::Seed;
use world::layout::{Layout, LayoutError};
use world::procgen::authored;
use world::template::RegionTemplate;

const GRAMMAR: &str = include_str!("../../../assets/grammar/socal_basin_2001.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon_2001.ron");
const REGION: &str = include_str!("../../../assets/regions/socal_bowl.ron");
const TOWN: &str = include_str!("fixtures/freeway_town.ron");

fn layout() -> Layout {
    let l: Layout = ron::from_str(TOWN).expect("parse layout");
    l.validate().expect("validate layout");
    l
}

/// The height of the edge of `g` passing nearest `(x, z)` that `pick` accepts.
fn road_y(g: &RoadGraph, x: f32, z: f32, pick: impl Fn(&place::Edge) -> bool) -> f32 {
    let mut best = (f32::INFINITY, 0.0);
    for e in g.edges.iter().filter(|e| pick(e)) {
        for p in &e.profile {
            let d = (p[0] - x).hypot(p[2] - z);
            if d < best.0 {
                best = (d, p[1]);
            }
        }
    }
    assert!(best.0 < 8.0, "a road at ({x}, {z})");
    best.1
}

#[test]
fn avenues_pass_over_the_freeway_and_ramps_join_one() {
    let l = layout();
    let (m, g, _) = authored::skeleton(Seed(1), &l);
    let highway = |e: &place::Edge| e.class == RoadClass::Highway;
    // The freeway meets a road only at its ends and where its four ramps leave it.
    let mut on_freeway = vec![0; g.nodes.len()];
    for e in g.edges.iter().filter(|e| highway(e)) {
        on_freeway[e.a as usize] += 1;
        on_freeway[e.b as usize] += 1;
    }
    let ramps: Vec<&place::Edge> = g.edges.iter().filter(|e| e.oneway).collect();
    assert_eq!(ramps.len(), 4);
    for r in &ramps {
        let ends = [on_freeway[r.a as usize] > 0, on_freeway[r.b as usize] > 0];
        assert!(ends[0] != ends[1], "a ramp joins the freeway to a road");
        assert!((r.lanes_fwd, r.lanes_back) == (1, 0) && !r.sidewalk);
    }
    // Two off-ramps (from the freeway) and two on-ramps, one of each either side.
    assert_eq!(
        ramps
            .iter()
            .filter(|r| on_freeway[r.a as usize] > 0)
            .count(),
        2
    );
    assert_eq!(on_freeway.iter().filter(|&&n| n > 0).count(), 4);
    // Each avenue rides a deck over the freeway; the freeway stays on the ground.
    for x in [500.0, 800.0, 1150.0] {
        let z = if x == 800.0 { 780.0 } else { 760.0 };
        let under = road_y(&g, x, z, highway);
        let over = road_y(&g, x, z, |e| !highway(e) && !e.oneway);
        assert!(
            over - under > authored::OVERPASS_LIFT_M - 0.5,
            "the avenue at x {x} clears the freeway: {over} over {under}"
        );
        assert!(
            (under - m.height_at(x, z)).abs() < 0.5,
            "freeway on the ground"
        );
        assert_eq!(m.zone_at(x, z), ZoneId::Highway);
    }
    // Nothing is steeper than its limit, the ramps a little more.
    for e in &g.edges {
        let limit = if e.oneway { 0.09 } else { 0.121 };
        for s in e.profile.windows(2) {
            let run = (s[1][0] - s[0][0]).hypot(s[1][2] - s[0][2]);
            assert!(
                (s[1][1] - s[0][1]).abs() <= limit * run + 1e-3,
                "grade at {:?}",
                s[0]
            );
        }
    }
}

#[test]
fn roads_bridge_the_wash_and_the_creek() {
    let l = layout();
    let (m, g, _) = authored::skeleton(Seed(1), &l);
    // The wash: a dry bed 5 m down, the boulevards and the freeway level across it.
    for (x, z) in [(1280.0, 150.0), (1262.0, 730.0), (1287.0, 1380.0)] {
        let bed = m.height_at(x, z);
        let y = road_y(&g, x, z, |_| true);
        assert!(
            y - bed > 4.5 && y - bed < 7.8,
            "a bridge at ({x}, {z}): {y} over {bed}"
        );
        assert_eq!(m.zone_at(x, z), ZoneId::Highway);
    }
    assert!(
        m.height_at(1270.0, 400.0) > m.water_level,
        "the wash is dry"
    );
    // The creek: water in the park, first avenue across it.
    assert!(m.height_at(330.0, 300.0) < m.water_level - 1.0);
    assert_eq!(m.zone_at(330.0, 300.0), ZoneId::Water);
    assert!(road_y(&g, 500.0, 322.0, |_| true) > m.water_level + 2.0);
    assert_eq!(m.water.len(), 2);
}

#[test]
fn the_town_builds_a_valid_deterministic_place() {
    let l = layout();
    let t: RegionTemplate = ron::from_str(REGION).expect("parse template");
    t.validate().expect("validate template");
    let mut g: world::Grammar = ron::from_str(GRAMMAR).expect("parse grammar");
    g.lexicon = ron::from_str(LEXICON).expect("parse lexicon");
    g.validate().expect("validate grammar");
    let p = authored::build_place(Seed(1), &l, &t, &g);
    place::validate::validate(&p).expect("the town validates");
    let again = authored::build_place(Seed(1), &l, &t, &g);
    assert_eq!(p.content_hash(), again.content_hash(), "deterministic");
    // No building on the freeway, its ramps or the wash.
    let (m, g, _) = authored::skeleton(Seed(1), &l);
    for b in &p.buildings {
        let n = b.footprint.len() as f32;
        let c = b
            .footprint
            .iter()
            .fold([0.0, 0.0], |a, q| [a[0] + q[0] / n, a[1] + q[1] / n]);
        for e in g
            .edges
            .iter()
            .filter(|e| e.class == RoadClass::Highway || e.oneway)
        {
            for q in &e.profile {
                let d = (q[0] - c[0]).hypot(q[2] - c[1]);
                assert!(d > 0.5 * e.width_m + 4.0, "a building at {c:?}");
            }
        }
        assert!(
            m.height_at(c[0], c[1]) > 4.0,
            "a building in the wash at {c:?}"
        );
    }
    assert!(p.buildings.len() > 500);
}

#[test]
fn bad_ramps_and_channels_are_refused() {
    let road = |l: &mut Layout, name: &str| l.roads.iter().position(|r| r.name == name).unwrap();
    let mut l = layout();
    let i = road(&mut l, "first avenue");
    l.roads[i].ramps = vec!["north boulevard".into()];
    assert_eq!(
        l.validate(),
        Err(LayoutError::BadRamp("first avenue".into()))
    );
    // A road the freeway does not cross, and one too near its end for the ramps.
    for (name, at) in [("north boulevard", None), ("first avenue", Some(250.0))] {
        let mut l = layout();
        if let Some(x) = at {
            let i = road(&mut l, name);
            l.roads[i].points = vec![(x, 150.0), (x, 1380.0)];
        }
        let i = road(&mut l, "freeway");
        l.roads[i].ramps = vec![name.into()];
        assert_eq!(l.validate(), Err(LayoutError::BadRamp("freeway".into())));
    }
    // An airfield's taxiway must reach a road.
    let mut l = layout();
    l.airfield = Some(world::layout::Airfield {
        runway: ((600.0, 900.0), (600.0, 1300.0)),
        runway_m: 30.0,
        width_m: 100.0,
        taxiway: (650.0, 1380.0),
    });
    assert_eq!(l.validate(), Ok(()));
    l.airfield.as_mut().unwrap().taxiway = (650.0, 1200.0);
    assert_eq!(
        l.validate(),
        Err(LayoutError::RoadToNowhere("airfield taxiway".into()))
    );
    let mut l = layout();
    l.channels[0].depth_m = 9.0;
    assert_eq!(l.validate(), Err(LayoutError::Value("wash".into())));
}
