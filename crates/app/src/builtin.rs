//! The game's built-in content as text, and the pure parsers that check it: region
//! templates, facade grammars, sign lexicons, landmark kits and scenarios. It knows nothing
//! else of the app, so the build script (`build.rs`) compiles it too, to bake each world's
//! place at build time ([`crate::places`]).

use std::sync::OnceLock;

use place::Place;
use skywave_core::Seed;

use world::landmarks::Kit;
use world::layout::Layout;
use world::template::RegionTemplate;
use world::Grammar;

pub const CALIFORNIA_COAST: &str = include_str!("../../../assets/regions/california_coast.ron");
pub const SOCAL_BASIN: &str = include_str!("../../../assets/regions/socal_basin.ron");
pub const SOCAL_BOWL: &str = include_str!("../../../assets/regions/socal_bowl.ron");
pub const BAY_AREA: &str = include_str!("../../../assets/regions/bay_area.ron");
pub const PACIFIC_SHORE_REGION: &str = include_str!("../../../assets/regions/pacific_shore.ron");
pub const PACIFIC_SHORE: &str = include_str!("../../../assets/layouts/pacific_shore.ron");
pub const BAY_LOOP: &str = include_str!("../../../assets/layouts/bay_loop.ron");
pub const MIRAGE_BOWL: &str = include_str!("../../../assets/layouts/mirage_bowl.ron");
pub const LAKE_CITY: &str = include_str!("../../../assets/layouts/lake_city.ron");
pub const GREAT_LAKES_CITY: &str = include_str!("../../../assets/regions/great_lakes_city.ron");
pub const CALIFORNIA_COAST_80S: &str =
    include_str!("../../../assets/grammar/california_coast_80s.ron");
pub const SOCAL_BASIN_2001: &str = include_str!("../../../assets/grammar/socal_basin_2001.ron");
pub const GREAT_LAKES_80S: &str = include_str!("../../../assets/grammar/great_lakes_80s.ron");
pub const LEXICON: &str = include_str!("../../../assets/grammar/lexicon.ron");
pub const LEXICON_2001: &str = include_str!("../../../assets/grammar/lexicon_2001.ron");

/// The landmark kits compiled into the game: a `place::PropKind::Landmark`'s `kit` is an
/// index into this list.
pub const LANDMARKS: [(&str, &str); 10] = world::landmarks::BUILTIN;

/// The sign lexicons compiled into the game, as `(id, RON text)`: a grammar's
/// `signs.lexicon` names its own.
pub const LEXICONS: [(&str, &str); 2] = [("us_roadside", LEXICON), ("us_basin", LEXICON_2001)];

/// The region templates compiled into the game, as `(id, RON text)`.
pub const REGIONS: [(&str, &str); 6] = [
    ("california_coast", CALIFORNIA_COAST),
    ("pacific_shore", PACIFIC_SHORE_REGION),
    ("socal_basin", SOCAL_BASIN),
    ("socal_bowl", SOCAL_BOWL),
    ("bay_area", BAY_AREA),
    ("great_lakes_city", GREAT_LAKES_CITY),
];
/// The authored layouts compiled into the game, as `(id, RON text)`.
pub const LAYOUTS: [(&str, &str); 4] = [
    ("pacific_shore", PACIFIC_SHORE),
    ("bay_loop", BAY_LOOP),
    ("mirage_bowl", MIRAGE_BOWL),
    ("lake_city", LAKE_CITY),
];
/// The facade grammars compiled into the game, as `(file name, RON text)`.
pub const GRAMMARS: [(&str, &str); 3] = [
    ("california_coast_80s.ron", CALIFORNIA_COAST_80S),
    ("socal_basin_2001.ron", SOCAL_BASIN_2001),
    ("great_lakes_80s.ron", GREAT_LAKES_80S),
];

/// The text of the built-in region template `id`.
pub fn region_text(id: &str) -> Option<&'static str> {
    REGIONS.iter().find(|(r, _)| *r == id).map(|(_, t)| *t)
}

/// The text of the built-in authored layout `id`.
pub fn layout_text(id: &str) -> Option<&'static str> {
    LAYOUTS.iter().find(|(l, _)| *l == id).map(|(_, t)| *t)
}

/// The text of the built-in grammar file `file`.
pub fn grammar_text(file: &str) -> Option<&'static str> {
    GRAMMARS.iter().find(|(g, _)| *g == file).map(|(_, t)| *t)
}

/// The scenarios compiled into the game, as `(id, RON text)`, in the order the World page
/// lists them (`crate::scenario::BUILTIN`).
pub const SCENARIOS: [(&str, &str); 4] = [
    (
        "socal_beach_1984",
        include_str!("../../../assets/scenarios/socal_beach_1984.ron"),
    ),
    (
        "mirage_bowl_2001",
        include_str!("../../../assets/scenarios/mirage_bowl_2001.ron"),
    ),
    (
        "bay_loop_2005",
        include_str!("../../../assets/scenarios/bay_loop_2005.ron"),
    ),
    (
        "lake_city_1986",
        include_str!("../../../assets/scenarios/lake_city_1986.ron"),
    ),
];

/// Parses a landmark kit (`assets/landmarks/*.ron`) and checks it.
pub fn parse_kit(text: &str) -> Result<Kit, String> {
    let k: Kit = ron::from_str(text).map_err(|e| format!("landmark kit: {e}"))?;
    k.validate()
        .map_err(|e| format!("landmark kit `{}`: {e}", k.id))?;
    Ok(k)
}

/// The built-in landmark kits, parsed once, in [`LANDMARKS`] order. They are built in and
/// their tests parse them, so a failure here is a build that should not have shipped.
pub fn landmarks() -> &'static [Kit] {
    static KITS: OnceLock<Vec<Kit>> = OnceLock::new();
    KITS.get_or_init(|| {
        LANDMARKS
            .iter()
            .map(|(_, text)| parse_kit(text).expect("the built-in landmark kits"))
            .collect()
    })
}

/// Parses a facade grammar (`assets/grammar/*.ron`) and the sign lexicon it names
/// (`assets/grammar/lexicon.ron`), attaches the lexicon and validates both.
pub fn load_grammar(text: &str, lexicon: &str) -> Result<Grammar, String> {
    let mut g: Grammar = ron::from_str(text).map_err(|e| format!("grammar: {e}"))?;
    g.lexicon = ron::from_str(lexicon).map_err(|e| format!("lexicon: {e}"))?;
    g.validate().map_err(|e| format!("grammar: {e}"))?;
    g.landmarks = landmarks().to_vec();
    Ok(g)
}

/// [`load_grammar`] with the built-in lexicon the grammar names (the coast's when it names
/// none that is built in, which validation then refuses).
pub fn load_builtin_grammar(text: &str) -> Result<Grammar, String> {
    let g: Grammar = ron::from_str(text).map_err(|e| format!("grammar: {e}"))?;
    let lexicon = LEXICONS
        .iter()
        .find(|(id, _)| *id == g.signs.lexicon)
        .map_or(LEXICON, |(_, t)| *t);
    load_grammar(text, lexicon)
}

/// Parses the built-in region template `id` and checks it.
pub fn template(id: &str) -> Result<RegionTemplate, String> {
    let text = region_text(id).ok_or("no such region template")?;
    let t: RegionTemplate = ron::from_str(text).map_err(|e| format!("region template: {e}"))?;
    t.validate()
        .map_err(|e| format!("region template: {e:?}"))?;
    Ok(t)
}

/// The seed whose places a release build bakes (`build.rs`): a scene's default.
pub const BAKED_SEED: Seed = Seed(1);

/// Parses the built-in authored layout `id` and checks it.
pub fn layout(id: &str) -> Result<Layout, String> {
    let text = layout_text(id).ok_or("no such layout")?;
    let l: Layout = ron::from_str(text).map_err(|e| format!("layout: {e}"))?;
    l.validate().map_err(|e| format!("layout {id}: {e}"))?;
    Ok(l)
}

/// The procedural place for `seed` of authored layout `layout` dressed by region template
/// `region`'s zone rules, or without one of the region template alone, its buildings
/// planned by `grammar`: what the build bakes and what a load generates
/// (`crate::places`), so a new way to make a world changes both.
pub fn generate(
    seed: Seed,
    region: &str,
    layout: Option<&str>,
    grammar: &Grammar,
) -> Result<Place, String> {
    let t = template(region)?;
    Ok(match layout {
        Some(id) => world::procgen::authored::build_place(seed, &self::layout(id)?, &t, grammar),
        None => world::procgen::build_place(seed, &t, grammar),
    })
}
