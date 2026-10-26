//! Region templates: the shipped RON files load and validate, and `validate` rejects bad
//! data with a specific error.

use place::ZoneId;
use world::template::{CoastSide, CoastWater, RegionTemplate, TemplateError};

const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");
const GREAT_LAKES: &str = include_str!("../../../assets/regions/great_lakes_city.ron");
const NORTH_SHORE: &str = include_str!("../../../assets/regions/north_shore_suburb.ron");

fn parse(text: &str) -> RegionTemplate {
    ron::from_str(text).unwrap_or_else(|e| panic!("parse: {e}"))
}

#[test]
fn shipped_templates_load_and_validate() {
    for (text, name) in [
        (CALIFORNIA, "california_coast"),
        (GREAT_LAKES, "great_lakes_city"),
        (NORTH_SHORE, "north_shore_suburb"),
    ] {
        let t = parse(text);
        assert_eq!(t.name, name);
        assert_eq!(t.validate(), Ok(()), "{name}");
    }
}

#[test]
fn california_is_the_west_coast_template() {
    let t = parse(CALIFORNIA);
    assert_eq!(t.coast.side, CoastSide::West);
    assert_eq!(t.coast.water, CoastWater::Ocean);
    assert!(t.coast.canals.is_some());
    assert!(t.elevation.ridge_count.0 >= 2);
    for zone in [
        ZoneId::Downtown,
        ZoneId::Residential,
        ZoneId::Suburb,
        ZoneId::Farmland,
    ] {
        assert!(t.zones.get(zone).unwrap().score.is_some(), "{zone:?}");
    }
    assert!(t.zones.get(ZoneId::Water).is_none());
}

#[test]
fn stubs_are_flat_lake_grids() {
    for text in [GREAT_LAKES, NORTH_SHORE] {
        let t = parse(text);
        assert_eq!(t.coast.water, CoastWater::Lake);
        assert_eq!(t.elevation.ridge_count, (0, 0));
        assert_eq!(
            t.zones.residential.streets,
            world::template::StreetPattern::Grid
        );
        assert_eq!(t.zones.suburb.streets, world::template::StreetPattern::Grid);
        assert!(t
            .zones
            .residential
            .styles
            .contains(&world::template::BuildingStyle::Bungalow));
    }
}

#[test]
fn bad_weight_tables_are_rejected() {
    // A curve whose x does not increase.
    let mut t = parse(CALIFORNIA);
    t.zones.residential.score.as_mut().unwrap().coast =
        vec![(0.0, 1.0), (500.0, 0.5), (400.0, 0.0)];
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadCurve {
            zone: ZoneId::Residential,
            curve: "coast",
            index: 2
        })
    );

    // A NaN weight.
    let mut t = parse(CALIFORNIA);
    t.zones.hills.score.as_mut().unwrap().slope[1].1 = f32::NAN;
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadCurve {
            zone: ZoneId::Hills,
            curve: "slope",
            index: 1
        })
    );

    // Negative noise.
    let mut t = parse(CALIFORNIA);
    t.zones.farmland.score.as_mut().unwrap().noise = -0.5;
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadScore {
            zone: ZoneId::Farmland,
            field: "noise"
        })
    );

    // A structural zone given a score.
    let mut t = parse(CALIFORNIA);
    t.zones.strip.score = t.zones.downtown.score.clone();
    assert_eq!(
        t.validate(),
        Err(TemplateError::StructuralZoneScored {
            zone: ZoneId::Strip
        })
    );

    // Nothing scored at all.
    let mut t = parse(GREAT_LAKES);
    for z in [
        &mut t.zones.downtown,
        &mut t.zones.residential,
        &mut t.zones.suburb,
        &mut t.zones.industrial,
        &mut t.zones.commercial,
    ] {
        z.score = None;
    }
    assert_eq!(t.validate(), Err(TemplateError::NoScoredZones));
}

#[test]
fn other_bad_fields_are_rejected() {
    let mut t = parse(CALIFORNIA);
    t.elevation.beach_width_m = (120.0, 60.0);
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadRange {
            field: "elevation.beach_width_m"
        })
    );

    let mut t = parse(CALIFORNIA);
    t.zones.park.trees[0].1 = 0.0;
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadTreeWeight {
            zone: ZoneId::Park,
            index: 0
        })
    );

    let mut t = parse(CALIFORNIA);
    t.zones.strip.props.neon = 1.5;
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadProps {
            zone: ZoneId::Strip,
            field: "neon"
        })
    );

    let mut t = parse(CALIFORNIA);
    t.zones.beach.block_m.1 = (0.0, 10.0);
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadBlockSize {
            zone: ZoneId::Beach
        })
    );

    let mut t = parse(CALIFORNIA);
    t.era.year = 1850;
    assert_eq!(t.validate(), Err(TemplateError::EraYear));
}

#[test]
fn unknown_and_missing_fields_fail_to_parse() {
    let typo = CALIFORNIA.replace("wiggle_m:", "wigle_m:");
    assert!(ron::from_str::<RegionTemplate>(&typo).is_err());
    let start = CALIFORNIA.find("        farmland: ZoneRule(").unwrap();
    let end = CALIFORNIA.find("        hills: ZoneRule(").unwrap();
    let missing = format!("{}{}", &CALIFORNIA[..start], &CALIFORNIA[end..]);
    assert!(ron::from_str::<RegionTemplate>(&missing).is_err());
}
