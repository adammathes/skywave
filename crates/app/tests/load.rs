//! A load builds each per-place structure once: the graded terrain, the parking lot plan
//! and the chunk plan are made by the detail pass and handed on to the collider and the
//! meshes, streamed or built whole. One test in its own binary, so nothing else moves the
//! counts while it reads them. `cargo test -p app --test load`.

use app::{App, SceneSetup};
use world::{build_counts, BuildCounts};

/// The counts `query`'s scene takes to load.
fn builds(query: &str) -> BuildCounts {
    let setup = SceneSetup::from_query(query).expect("a scene");
    let before = build_counts();
    let app = App::from_setup(&setup).expect("the scene loads");
    let after = build_counts();
    drop(app);
    BuildCounts {
        grids: after.grids - before.grids,
        lot_plans: after.lot_plans - before.lot_plans,
        chunk_plans: after.chunk_plans - before.chunk_plans,
    }
}

#[test]
fn a_load_builds_the_grid_the_lots_and_the_plan_once() {
    let once = BuildCounts {
        grids: 1,
        lot_plans: 1,
        chunk_plans: 1,
    };
    for query in [
        "scenario=mirage_valley_2001&seed=1&stream=1",
        "scenario=mirage_valley_2001&seed=1",
        "scenario=socal_beach_1984&seed=2&stream=1",
    ] {
        assert_eq!(builds(query), once, "{query}");
    }
    // The test town has no terrain to grade.
    let town = BuildCounts { grids: 0, ..once };
    assert_eq!(builds("place=town&stream=1"), town);
}
