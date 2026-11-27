//! What the suites build: Pacific Shore, the drawn beach town (`docs/LAYOUTS.md`), with its
//! zone rules. The seed varies the fine grain (lots, buildings, small relief), not the map.
#![allow(dead_code)]

use place::{Place, RoadGraph};
use skywave_core::Seed;
use world::layout::Layout;
use world::procgen::{authored, MacroMap};
use world::template::RegionTemplate;

pub const REGION: &str = include_str!("../../../../assets/regions/pacific_shore.ron");
pub const LAYOUT: &str = include_str!("../../../../assets/layouts/pacific_shore.ron");
/// The Shore's size in chunks.
pub const SIZE: [u16; 2] = [16, 16];

pub fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(REGION).expect("parse");
    t.validate().expect("validate");
    t
}

pub fn layout() -> Layout {
    let l: Layout = ron::from_str(LAYOUT).expect("parse layout");
    l.validate().expect("validate layout");
    l
}

/// The Shore for `seed`, dressed by `t` and `g`.
pub fn build_place(seed: Seed, t: &RegionTemplate, g: &world::Grammar) -> Place {
    authored::build_place(seed, &layout(), t, g)
}

/// The Shore's macro map and main roads for `seed`.
pub fn skeleton(seed: u64) -> (MacroMap, RoadGraph) {
    let (m, g, _) = authored::skeleton(Seed(seed), &layout());
    (m, g)
}

/// [`build_place`] with the plan its detail pass read, for `palette`.
pub fn build_place_planned(
    seed: Seed,
    t: &RegionTemplate,
    g: &world::Grammar,
    palette: &world::Palette,
) -> (Place, world::ChunkPlan) {
    authored::build_place_planned(seed, &layout(), t, g, palette)
}

/// The Mirage Bowl at seed 1, dressed by `g`: a freeway with overpasses, a rail line, a
/// downtown with office lots, hills all round.
pub fn bowl(g: &world::Grammar) -> Place {
    let t: RegionTemplate =
        ron::from_str(include_str!("../../../../assets/regions/socal_bowl.ron")).expect("parse");
    let l: Layout = ron::from_str(include_str!("../../../../assets/layouts/mirage_bowl.ron"))
        .expect("parse layout");
    authored::build_place(Seed(1), &l, &t, g)
}
