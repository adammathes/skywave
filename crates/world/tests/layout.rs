//! Authored layouts end to end (`docs/LAYOUTS.md`): the Bay Loop builds a valid,
//! deterministic Place whose main roads are one network, whose start is on a road, and
//! whose bridges stand over the water; the layout checks refuse a road to nowhere.
//! `cargo test -p world --release --test layout -- --nocapture` prints the numbers.

use std::time::Instant;

use place::{RoadClass, SpawnKind, ZoneId};
use skywave_core::Seed;
use world::layout::{Layout, LayoutError};
use world::procgen::authored;
use world::template::RegionTemplate;

const GRAMMAR: &str = include_str!("../../../assets/grammar/bay_area_2005.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon_2001.ron");
const BAY_AREA: &str = include_str!("../../../assets/regions/bay_area.ron");
const BAY_LOOP: &str = include_str!("../../../assets/layouts/bay_loop.ron");

fn grammar() -> world::Grammar {
    let mut g: world::Grammar = ron::from_str(GRAMMAR).expect("parse grammar");
    g.lexicon = ron::from_str(LEXICON).expect("parse lexicon");
    g.validate().expect("validate grammar");
    g.landmarks = world::landmarks::BUILTIN
        .iter()
        .map(|(_, text)| ron::from_str(text).expect("parse landmark kit"))
        .collect();
    g
}

fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(BAY_AREA).expect("parse template");
    t.validate().expect("validate template");
    t
}

fn layout() -> Layout {
    let l: Layout = ron::from_str(BAY_LOOP).expect("parse layout");
    l.validate().expect("validate layout");
    l
}

/// Pinned `Place::content_hash` of the Bay Loop for seed 1. It moves whenever the layout,
/// the zone rules or any shared stage changes: regenerate with `--nocapture` and commit it
/// on its own with the reason.
const GOLDEN: u64 = 0x68ccc6a592d20329;

#[test]
fn the_bay_loop_validates_and_is_deterministic() {
    let (l, t, g) = (layout(), template(), grammar());
    let t0 = Instant::now();
    let p = authored::build_place(Seed(1), &l, &t, &g);
    println!(
        "bay loop: {} roads, {} parcels, {} buildings, {} props in {:.0} ms, hash {:#018x}",
        p.roads.edges.len(),
        p.parcels.len(),
        p.buildings.len(),
        p.props.items.len(),
        t0.elapsed().as_secs_f64() * 1000.0,
        p.content_hash()
    );
    place::validate::validate(&p).expect("the Bay Loop validates");
    let again = authored::build_place(Seed(1), &l, &t, &g);
    assert_eq!(p.content_hash(), again.content_hash(), "deterministic");
    if GOLDEN != 0 {
        assert_eq!(
            p.content_hash(),
            GOLDEN,
            "golden moved; see the rule above GOLDEN"
        );
    }
    // The start is on the ground of a road, in the city.
    let s = p
        .spawns
        .iter()
        .find(|s| s.kind == SpawnKind::PlayerStart)
        .expect("a start");
    assert!(s.pos[0] > 400.0 && s.pos[0] < 1800.0 && s.pos[2] > 600.0 && s.pos[2] < 1500.0);
}

#[test]
fn main_roads_are_one_network_with_bridges_over_water() {
    let l = layout();
    let mut m = authored::macro_map(Seed(1), &l);
    let (g, start) = authored::arterials(&l, &mut m);
    assert!(start.is_some());
    // One component.
    let n = g.nodes.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for e in &g.edges {
        let (a, b) = (
            find(&mut parent, e.a as usize),
            find(&mut parent, e.b as usize),
        );
        parent[a] = b;
    }
    let roots: std::collections::BTreeSet<usize> = (0..n).map(|i| find(&mut parent, i)).collect();
    assert_eq!(roots.len(), 1, "the main roads are one network");
    // Every node with one edge is a lookout's end.
    let mut degree = vec![0; n];
    for e in &g.edges {
        degree[e.a as usize] += 1;
        degree[e.b as usize] += 1;
    }
    let dead = degree.iter().filter(|&&d| d == 1).count();
    let lookouts = l.roads.iter().filter(|r| r.dead_end).count();
    assert_eq!(dead, lookouts, "only the lookouts dead-end");
    // Over the water the roads ride a deck above it; on land they sit on the ground.
    // On a steep hill the grade limit can lift or sink a road a meter or two off the carved
    // ground; only a bridge's approach, where the deck meets the shore, stands further off.
    let (mut deck, mut on, mut all) = (0, 0, 0);
    for e in &g.edges {
        for p in &e.profile {
            let ground = m.height_at(p[0], p[2]);
            assert!(
                (p[1] - ground).abs() < 7.8,
                "within the profile band at {p:?}"
            );
            all += 1;
            if m.zone_at(p[0], p[2]) == ZoneId::Water {
                assert!(p[1] > m.water_level + 1.0, "a deck over the water at {p:?}");
                deck += 1;
            } else if (p[1] - ground).abs() < 2.5 {
                on += 1;
            }
        }
    }
    assert!(
        deck > 100,
        "the bridges cross the water ({deck} deck points)"
    );
    assert!(
        on + deck > all * 97 / 100,
        "roads sit on the ground ({on} of {all})"
    );
    // The freeways round the bay, and the bayshore's two interchanges' ramps.
    assert!(g.edges.iter().any(|e| e.class == RoadClass::Highway));
    assert!(
        g.edges.iter().filter(|e| e.oneway).count() >= 8,
        "two diamonds of four ramps"
    );
}

#[test]
fn a_road_to_nowhere_is_refused() {
    let mut l = layout();
    let r = l
        .roads
        .iter_mut()
        .find(|r| r.name == "old highway")
        .unwrap();
    r.points = vec![(1450.0, 1000.0), (1450.0, 2000.0)];
    assert_eq!(
        l.validate(),
        Err(LayoutError::RoadToNowhere("old highway".into()))
    );
}
/// Steepest grade a Bay Loop street may have over any 2.5 m or more (about a wheelbase):
/// about the steepest real city streets, so every car can climb it.
const MAX_GRADE: f32 = 0.30;

#[test]
fn no_bay_loop_street_is_steeper_than_a_real_one() {
    let (l, t, g) = (layout(), template(), grammar());
    let p = authored::build_place(Seed(1), &l, &t, &g);
    let mut worst = (0.0f32, RoadClass::Local, [0.0f32; 2]);
    for e in &p.roads.edges {
        // Grade over runs of at least a wheelbase, so a profile's tiny steps do not count alone.
        let pts = &e.profile;
        for i in 0..pts.len() {
            let mut run = 0.0;
            for j in i + 1..pts.len() {
                let (a, b) = (pts[j - 1], pts[j]);
                run += ((b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2)).sqrt();
                if run >= 2.5 {
                    let grade = (pts[j][1] - pts[i][1]).abs() / run;
                    if grade > worst.0 {
                        worst = (grade, e.class, [pts[i][0], pts[i][2]]);
                    }
                    break;
                }
            }
        }
    }
    println!(
        "steepest street: {:.1}% ({:?} at {:?})",
        worst.0 * 100.0,
        worst.1,
        worst.2
    );
    assert!(
        worst.0 <= MAX_GRADE,
        "{:.1}% {:?} at {:?}",
        worst.0 * 100.0,
        worst.1,
        worst.2
    );
}
