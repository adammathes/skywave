//! Region templates: the shipped RON files load and validate, and `validate` rejects bad
//! data with a specific error.

use place::ZoneId;
use world::template::{RegionTemplate, TemplateError};

const SHORE: &str = include_str!("../../../assets/regions/pacific_shore.ron");
const BOWL: &str = include_str!("../../../assets/regions/socal_bowl.ron");
const BAY: &str = include_str!("../../../assets/regions/bay_area.ron");
const GREAT_LAKES: &str = include_str!("../../../assets/regions/great_lakes_city.ron");
const NORTH_SHORE: &str = include_str!("../../../assets/regions/north_shore_suburb.ron");

fn parse(text: &str) -> RegionTemplate {
    ron::from_str(text).unwrap_or_else(|e| panic!("parse: {e}"))
}

#[test]
fn shipped_templates_load_and_validate() {
    for (text, name) in [
        (SHORE, "pacific_shore"),
        (BOWL, "socal_bowl"),
        (BAY, "bay_area"),
        (GREAT_LAKES, "great_lakes_city"),
        (NORTH_SHORE, "north_shore_suburb"),
    ] {
        let t = parse(text);
        assert_eq!(t.name, name);
        assert_eq!(t.validate(), Ok(()), "{name}");
    }
}

#[test]
fn other_bad_fields_are_rejected() {
    let mut t = parse(SHORE);
    t.zones.park.trees[0].1 = 0.0;
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadTreeWeight {
            zone: ZoneId::Park,
            index: 0
        })
    );

    let mut t = parse(SHORE);
    t.zones.strip.props.neon = 1.5;
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadProps {
            zone: ZoneId::Strip,
            field: "neon"
        })
    );

    for field in [
        "street_light_m",
        "street_tree_m",
        "hydrant_m",
        "power_pole_m",
    ] {
        let mut t = parse(SHORE);
        let p = &mut t.zones.residential.props;
        let x = match field {
            "street_light_m" => &mut p.street_light_m,
            "street_tree_m" => &mut p.street_tree_m,
            "hydrant_m" => &mut p.hydrant_m,
            _ => &mut p.power_pole_m,
        };
        *x = 1e-5;
        assert_eq!(
            t.validate(),
            Err(TemplateError::BadProps {
                zone: ZoneId::Residential,
                field
            }),
            "{field}"
        );
    }
    let mut t = parse(SHORE);
    t.zones.residential.props.hydrant_m = 0.0;
    assert_eq!(t.validate(), Ok(()), "a zero spacing means none");

    let mut t = parse(SHORE);
    t.zones.beach.block_m.1 = (0.0, 10.0);
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadBlockSize {
            zone: ZoneId::Beach
        })
    );

    let mut t = parse(SHORE);
    t.zones.residential.lots.width_m = (20.0, 15.0);
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadLots {
            zone: ZoneId::Residential,
            field: "width_m"
        })
    );

    let mut t = parse(SHORE);
    t.zones.downtown.lots.depth_m = 2.0;
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadLots {
            zone: ZoneId::Downtown,
            field: "depth_m"
        })
    );

    let mut t = parse(SHORE);
    t.zones.suburb.lots.setback_m.1 = -1.0;
    assert_eq!(
        t.validate(),
        Err(TemplateError::BadLots {
            zone: ZoneId::Suburb,
            field: "setback_m"
        })
    );

    let mut t = parse(SHORE);
    t.era.year = 1850;
    assert_eq!(t.validate(), Err(TemplateError::EraYear));

    let bad_massing =
        |zone: ZoneId, field: &'static str| Err::<(), _>(TemplateError::BadMassing { zone, field });
    let mut t = parse(SHORE);
    t.zones.downtown.massing.as_mut().unwrap().coverage = 1.2;
    assert_eq!(t.validate(), bad_massing(ZoneId::Downtown, "coverage"));
    let mut t = parse(SHORE);
    t.zones.strip.massing.as_mut().unwrap().floors = (3, 1);
    assert_eq!(t.validate(), bad_massing(ZoneId::Strip, "floors"));
    let mut t = parse(SHORE);
    t.zones.suburb.massing.as_mut().unwrap().floors_mean = 9.0;
    assert_eq!(t.validate(), bad_massing(ZoneId::Suburb, "floors_mean"));
    let mut t = parse(SHORE);
    t.zones.residential.massing.as_mut().unwrap().style_weights = vec![1.0];
    assert_eq!(
        t.validate(),
        bad_massing(ZoneId::Residential, "style_weights")
    );
}

#[test]
fn unknown_and_missing_fields_fail_to_parse() {
    let typo = SHORE.replace("alleys:", "aleys:");
    assert!(ron::from_str::<RegionTemplate>(&typo).is_err());
    let start = SHORE.find("        farmland: ZoneRule(").unwrap();
    let end = SHORE.find("        hills: ZoneRule(").unwrap();
    let missing = format!("{}{}", &SHORE[..start], &SHORE[end..]);
    assert!(ron::from_str::<RegionTemplate>(&missing).is_err());
}
