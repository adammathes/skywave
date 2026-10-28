//! Loading [`VehicleSpec`]s and their [`ShapeSpec`]s from the RON files in
//! `assets/vehicles/`.

use vehicle::shape::ShapeSpec;
use vehicle::VehicleSpec;

/// Parses a RON `VehicleSpec` and validates it. The error names the spec and, for a parse
/// failure, the line and column.
pub fn load_spec(text: &str) -> Result<VehicleSpec, String> {
    let spec: VehicleSpec = ron::from_str(text).map_err(|e| format!("vehicle spec: {e}"))?;
    spec.validate()
        .map_err(|e| format!("vehicle spec `{}`: {e:?}", spec.id))?;
    Ok(spec)
}

/// The vehicle specs compiled into the game, as `(id, RON text)`, in the order the Vehicle
/// page lists them: the 1980s roster (the everyday cars, then the sports cars), then the
/// 2001 hero cars, then the 2001 traffic cars (the semi last: traffic only, never listed).
pub const BUILTIN: [(&str, &str); 36] = [
    (
        "gannet_gt",
        include_str!("../../../assets/vehicles/gannet_gt.ron"),
    ),
    (
        "homestead_wagon",
        include_str!("../../../assets/vehicles/homestead_wagon.ron"),
    ),
    (
        "seabreeze_convertible",
        include_str!("../../../assets/vehicles/seabreeze_convertible.ron"),
    ),
    (
        "regent_coupe_cabrio",
        include_str!("../../../assets/vehicles/regent_coupe_cabrio.ron"),
    ),
    (
        "halcyon_turbo",
        include_str!("../../../assets/vehicles/halcyon_turbo.ron"),
    ),
    (
        "pipit_hatch",
        include_str!("../../../assets/vehicles/pipit_hatch.ron"),
    ),
    (
        "bancroft_sedan",
        include_str!("../../../assets/vehicles/bancroft_sedan.ron"),
    ),
    (
        "prescott_landau",
        include_str!("../../../assets/vehicles/prescott_landau.ron"),
    ),
    (
        "meadow_minivan",
        include_str!("../../../assets/vehicles/meadow_minivan.ron"),
    ),
    (
        "buckboard_pickup",
        include_str!("../../../assets/vehicles/buckboard_pickup.ron"),
    ),
    (
        "drayman_van",
        include_str!("../../../assets/vehicles/drayman_van.ron"),
    ),
    (
        "burro_4x4",
        include_str!("../../../assets/vehicles/burro_4x4.ron"),
    ),
    (
        "basilisk_v12",
        include_str!("../../../assets/vehicles/basilisk_v12.ron"),
    ),
    (
        "basilisk_roadster",
        include_str!("../../../assets/vehicles/basilisk_roadster.ron"),
    ),
    (
        "avocet_v8",
        include_str!("../../../assets/vehicles/avocet_v8.ron"),
    ),
    (
        "avocet_spider",
        include_str!("../../../assets/vehicles/avocet_spider.ron"),
    ),
    (
        "longbow_coupe",
        include_str!("../../../assets/vehicles/longbow_coupe.ron"),
    ),
    (
        "longbow_convertible",
        include_str!("../../../assets/vehicles/longbow_convertible.ron"),
    ),
    (
        "appaloosa_gt",
        include_str!("../../../assets/vehicles/appaloosa_gt.ron"),
    ),
    (
        "appaloosa_convertible",
        include_str!("../../../assets/vehicles/appaloosa_convertible.ron"),
    ),
    (
        "halberd_coupe",
        include_str!("../../../assets/vehicles/halberd_coupe.ron"),
    ),
    (
        "halberd_targa",
        include_str!("../../../assets/vehicles/halberd_targa.ron"),
    ),
    (
        "tanager_awd",
        include_str!("../../../assets/vehicles/tanager_awd.ron"),
    ),
    (
        "destrier_coupe",
        include_str!("../../../assets/vehicles/destrier_coupe.ron"),
    ),
    (
        "siskin_coupe",
        include_str!("../../../assets/vehicles/siskin_coupe.ron"),
    ),
    (
        "shrike_coupe",
        include_str!("../../../assets/vehicles/shrike_coupe.ron"),
    ),
    (
        "glaive_turbo",
        include_str!("../../../assets/vehicles/glaive_turbo.ron"),
    ),
    (
        "falchion_sedan",
        include_str!("../../../assets/vehicles/falchion_sedan.ron"),
    ),
    (
        "courser_v8",
        include_str!("../../../assets/vehicles/courser_v8.ron"),
    ),
    (
        "plover_sedan",
        include_str!("../../../assets/vehicles/plover_sedan.ron"),
    ),
    (
        "sorrel_utility",
        include_str!("../../../assets/vehicles/sorrel_utility.ron"),
    ),
    (
        "wren_minivan",
        include_str!("../../../assets/vehicles/wren_minivan.ron"),
    ),
    (
        "percheron_pickup",
        include_str!("../../../assets/vehicles/percheron_pickup.ron"),
    ),
    (
        "linnet_hatch",
        include_str!("../../../assets/vehicles/linnet_hatch.ron"),
    ),
    (
        "sledge_box_truck",
        include_str!("../../../assets/vehicles/sledge_box_truck.ron"),
    ),
    (
        "anvil_semi",
        include_str!("../../../assets/vehicles/anvil_semi.ron"),
    ),
];

/// Built-in vehicle ids that were renamed, `(old, new)`: a saved setting or a scene URL
/// naming the old id gets the car under its new one.
pub const RENAMED: [(&str, &str); 2] = [
    ("corsair_gtx", "gannet_gt"),
    ("palisade_wagon", "homestead_wagon"),
];

/// `id`, or the id it was renamed to.
pub fn current_id(id: &str) -> &str {
    RENAMED
        .iter()
        .find(|(old, _)| *old == id)
        .map_or(id, |(_, new)| new)
}

/// Loads the built-in spec with this id.
pub fn builtin_spec(id: &str) -> Result<VehicleSpec, String> {
    let (_, text) = BUILTIN
        .iter()
        .find(|(name, _)| *name == id)
        .ok_or_else(|| format!("no vehicle `{id}`"))?;
    load_spec(text)
}

/// Parses a RON `ShapeSpec` and validates it.
pub fn load_shape(text: &str) -> Result<ShapeSpec, String> {
    let shape: ShapeSpec = ron::from_str(text).map_err(|e| format!("shape spec: {e}"))?;
    shape
        .validate()
        .map_err(|e| format!("shape spec `{}`: {e:?}", shape.id))?;
    Ok(shape)
}

/// The car body shapes compiled into the game, as `(file name, RON text)`; a
/// `VehicleSpec::shape` names one. [`TRAFFIC_SHAPES`] are the everyday cars NPC traffic
/// drives; new shapes go at the end, so the place's parked-car paints (seeded by index)
/// keep theirs.
pub const BUILTIN_SHAPES: [(&str, &str); 36] = [
    (
        "gannet_gt.shape.ron",
        include_str!("../../../assets/vehicles/gannet_gt.shape.ron"),
    ),
    (
        "homestead_wagon.shape.ron",
        include_str!("../../../assets/vehicles/homestead_wagon.shape.ron"),
    ),
    (
        "seabreeze_convertible.shape.ron",
        include_str!("../../../assets/vehicles/seabreeze_convertible.shape.ron"),
    ),
    (
        "regent_coupe_cabrio.shape.ron",
        include_str!("../../../assets/vehicles/regent_coupe_cabrio.shape.ron"),
    ),
    (
        "halcyon_turbo.shape.ron",
        include_str!("../../../assets/vehicles/halcyon_turbo.shape.ron"),
    ),
    (
        "pipit_hatch.shape.ron",
        include_str!("../../../assets/vehicles/pipit_hatch.shape.ron"),
    ),
    (
        "bancroft_sedan.shape.ron",
        include_str!("../../../assets/vehicles/bancroft_sedan.shape.ron"),
    ),
    (
        "prescott_landau.shape.ron",
        include_str!("../../../assets/vehicles/prescott_landau.shape.ron"),
    ),
    (
        "meadow_minivan.shape.ron",
        include_str!("../../../assets/vehicles/meadow_minivan.shape.ron"),
    ),
    (
        "buckboard_pickup.shape.ron",
        include_str!("../../../assets/vehicles/buckboard_pickup.shape.ron"),
    ),
    (
        "drayman_van.shape.ron",
        include_str!("../../../assets/vehicles/drayman_van.shape.ron"),
    ),
    (
        "burro_4x4.shape.ron",
        include_str!("../../../assets/vehicles/burro_4x4.shape.ron"),
    ),
    (
        "longbow_coupe.shape.ron",
        include_str!("../../../assets/vehicles/longbow_coupe.shape.ron"),
    ),
    (
        "appaloosa_gt.shape.ron",
        include_str!("../../../assets/vehicles/appaloosa_gt.shape.ron"),
    ),
    (
        "basilisk_v12.shape.ron",
        include_str!("../../../assets/vehicles/basilisk_v12.shape.ron"),
    ),
    (
        "basilisk_roadster.shape.ron",
        include_str!("../../../assets/vehicles/basilisk_roadster.shape.ron"),
    ),
    (
        "avocet_v8.shape.ron",
        include_str!("../../../assets/vehicles/avocet_v8.shape.ron"),
    ),
    (
        "avocet_spider.shape.ron",
        include_str!("../../../assets/vehicles/avocet_spider.shape.ron"),
    ),
    (
        "longbow_convertible.shape.ron",
        include_str!("../../../assets/vehicles/longbow_convertible.shape.ron"),
    ),
    (
        "appaloosa_convertible.shape.ron",
        include_str!("../../../assets/vehicles/appaloosa_convertible.shape.ron"),
    ),
    (
        "halberd_coupe.shape.ron",
        include_str!("../../../assets/vehicles/halberd_coupe.shape.ron"),
    ),
    (
        "halberd_targa.shape.ron",
        include_str!("../../../assets/vehicles/halberd_targa.shape.ron"),
    ),
    (
        "tanager_awd.shape.ron",
        include_str!("../../../assets/vehicles/tanager_awd.shape.ron"),
    ),
    (
        "destrier_coupe.shape.ron",
        include_str!("../../../assets/vehicles/destrier_coupe.shape.ron"),
    ),
    (
        "siskin_coupe.shape.ron",
        include_str!("../../../assets/vehicles/siskin_coupe.shape.ron"),
    ),
    (
        "plover_sedan.shape.ron",
        include_str!("../../../assets/vehicles/plover_sedan.shape.ron"),
    ),
    (
        "sorrel_utility.shape.ron",
        include_str!("../../../assets/vehicles/sorrel_utility.shape.ron"),
    ),
    (
        "wren_minivan.shape.ron",
        include_str!("../../../assets/vehicles/wren_minivan.shape.ron"),
    ),
    (
        "percheron_pickup.shape.ron",
        include_str!("../../../assets/vehicles/percheron_pickup.shape.ron"),
    ),
    (
        "linnet_hatch.shape.ron",
        include_str!("../../../assets/vehicles/linnet_hatch.shape.ron"),
    ),
    (
        "sledge_box_truck.shape.ron",
        include_str!("../../../assets/vehicles/sledge_box_truck.shape.ron"),
    ),
    (
        "anvil_semi.shape.ron",
        include_str!("../../../assets/vehicles/anvil_semi.shape.ron"),
    ),
    (
        "shrike_coupe.shape.ron",
        include_str!("../../../assets/vehicles/shrike_coupe.shape.ron"),
    ),
    (
        "glaive_turbo.shape.ron",
        include_str!("../../../assets/vehicles/glaive_turbo.shape.ron"),
    ),
    (
        "falchion_sedan.shape.ron",
        include_str!("../../../assets/vehicles/falchion_sedan.shape.ron"),
    ),
    (
        "courser_v8.shape.ron",
        include_str!("../../../assets/vehicles/courser_v8.shape.ron"),
    ),
];

/// The [`BUILTIN_SHAPES`] NPC traffic draws from, by index: the twelve everyday cars of the
/// 1980s and their two common sports cars, then the seven 2001 traffic cars. A world drives
/// those of its cars ([`crate::scenario::Lineup::shapes`]), so the 1984 coast drives the
/// first fourteen alone. The exotics, the new convertibles and the 2001 hero cars are the
/// player's to drive.
pub const TRAFFIC_SHAPES: [usize; 21] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 25, 26, 27, 28, 29, 30, 31,
];

/// Where each traffic shape is seen, as weights per zone (`place::ZoneId` order: downtown,
/// strip, residential, suburb, industrial, commercial, park, beach, farmland, hills,
/// highway (the freeway lanes), water), for traffic (`traffic::ShapeRule::zones`) and parked
/// cars alike. A shape not listed weighs [`ZONES_ALIKE`]: the 1980s cars weigh by their
/// age alone. SUVs and minivans in the residential and strip zones, pickups and box trucks
/// in the industrial ones, the semi on the freeway alone.
pub const ZONE_WEIGHTS: [(&str, [u8; 12]); 7] = [
    ("plover_sedan", [3, 3, 3, 3, 2, 3, 2, 2, 2, 2, 3, 2]),
    ("sorrel_utility", [2, 4, 4, 4, 1, 2, 2, 2, 2, 3, 2, 2]),
    ("wren_minivan", [1, 4, 4, 4, 1, 2, 2, 2, 1, 2, 2, 1]),
    ("percheron_pickup", [1, 1, 2, 2, 5, 1, 1, 1, 4, 3, 2, 1]),
    ("linnet_hatch", [3, 3, 2, 2, 1, 3, 2, 2, 1, 1, 2, 2]),
    ("sledge_box_truck", [2, 1, 1, 1, 5, 2, 1, 1, 1, 1, 1, 1]),
    ("anvil_semi", [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0]),
];

/// The zone weights of a shape [`ZONE_WEIGHTS`] does not list.
pub const ZONES_ALIKE: [u8; 12] = [2; 12];

/// The zone weights of built-in shape `id` (its `ShapeSpec::id`).
pub fn zone_weights(id: &str) -> [u8; 12] {
    ZONE_WEIGHTS
        .iter()
        .find(|(s, _)| *s == id)
        .map_or(ZONES_ALIKE, |(_, z)| *z)
}

/// Bodies longer than this keep their own length in traffic; the rest are spaced as
/// `traffic::cars::CAR_LEN_M` (the 1980s cars always were), m.
pub const LONG_BODY_M: f32 = 6.0;

/// Loads the built-in shape `spec.shape` names.
pub fn builtin_shape(spec: &VehicleSpec) -> Result<ShapeSpec, String> {
    let (_, text) = BUILTIN_SHAPES
        .iter()
        .find(|(name, _)| *name == spec.shape)
        .ok_or_else(|| format!("no shape `{}` for `{}`", spec.shape, spec.id))?;
    load_shape(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vehicle::shape::Detail::Targa;
    use vehicle::{Drive, Era, VehicleClass};

    const GANNET: &str = include_str!("../../../assets/vehicles/gannet_gt.ron");
    const HOMESTEAD: &str = include_str!("../../../assets/vehicles/homestead_wagon.ron");

    /// Peak full-throttle power between idle and redline, W, sampled every 10 rpm.
    fn peak_power_w(spec: &VehicleSpec) -> f32 {
        let e = &spec.engine;
        let mut best = 0.0f32;
        let mut rpm = e.idle_rpm;
        while rpm <= e.redline_rpm {
            best = best.max(e.torque_at(rpm) * rpm * (core::f32::consts::TAU / 60.0));
            rpm += 10.0;
        }
        best
    }

    fn peak_torque_nm(spec: &VehicleSpec) -> f32 {
        spec.engine
            .torque_nm
            .iter()
            .fold(0.0f32, |m, &(_, nm)| m.max(nm))
    }

    #[test]
    fn both_specs_load_and_validate() {
        for (text, id, shape) in [
            (GANNET, "gannet_gt", "gannet_gt.shape.ron"),
            (HOMESTEAD, "homestead_wagon", "homestead_wagon.shape.ron"),
        ] {
            let spec = load_spec(text).unwrap();
            assert_eq!(spec.validate(), Ok(()));
            assert_eq!(spec.id, id);
            assert_eq!(spec.shape, shape);
            assert_eq!(spec.era, Era::Eighties);
        }
    }

    #[test]
    fn specs_match_the_roster() {
        let sports = load_spec(GANNET).unwrap();
        let wagon = load_spec(HOMESTEAD).unwrap();

        assert_eq!(sports.class, VehicleClass::Sports);
        assert_eq!(sports.drive, Drive::Rear);
        assert_eq!(sports.gear_count(), 5);
        assert_eq!(peak_torque_nm(&sports), 290.0);
        assert!((1200.0..=1300.0).contains(&sports.mass_kg));

        assert_eq!(wagon.class, VehicleClass::Wagon);
        assert_eq!(wagon.drive, Drive::Rear);
        assert_eq!(wagon.gear_count(), 3);
        assert!(wagon.auto);
        assert_eq!(peak_torque_nm(&wagon), 310.0);
        assert!((1700.0..=1800.0).contains(&wagon.mass_kg));
    }

    #[test]
    fn sports_car_and_wagon_feel_different() {
        let sports = load_spec(GANNET).unwrap();
        let wagon = load_spec(HOMESTEAD).unwrap();

        let ptw = |s: &VehicleSpec| peak_power_w(s) / s.mass_kg;
        assert!(ptw(&sports) > 1.5 * ptw(&wagon));
        assert!(sports.cg_height_m < wagon.cg_height_m);
        assert!(sports.mass_kg < wagon.mass_kg);

        // Wide ratios: the wagon's mean step between gears is bigger.
        let step = |s: &VehicleSpec| {
            let spread = s.gears[0] / s.top_gear_ratio();
            spread.powf(1.0 / (s.gear_count() - 1) as f32)
        };
        assert!(step(&wagon) > step(&sports));

        // Slow steering, soft and rolly suspension, understeer by design.
        assert!(wagon.steering.return_rate < sports.steering.return_rate);
        assert!(wagon.suspension.spring_n_m < sports.suspension.spring_n_m);
        assert!(wagon.suspension.antiroll < sports.suspension.antiroll);
        assert!(wagon.feel.body_roll > sports.feel.body_roll);
        assert!(wagon.feel.oversteer_bias < 0.0 && sports.feel.oversteer_bias > 0.0);
        assert!(wagon.cg_fore_aft < sports.cg_fore_aft);
        assert!(wagon.tires.grip_lat < sports.tires.grip_lat);
    }

    #[test]
    fn the_convertibles_are_built_in() {
        for id in ["seabreeze_convertible", "regent_coupe_cabrio"] {
            let spec = builtin_spec(id).unwrap();
            assert_eq!(spec.class, VehicleClass::Convertible);
            assert_eq!(spec.drive, Drive::Front);
            assert_eq!(spec.era, Era::Eighties);
            assert!(builtin_shape(&spec).unwrap().roof.convertible());
        }
        let seabreeze = builtin_spec("seabreeze_convertible").unwrap();
        let regent = builtin_spec("regent_coupe_cabrio").unwrap();
        // The big coupe: more power, more mass, slower steering, more roll.
        assert!(peak_power_w(&regent) > 1.3 * peak_power_w(&seabreeze));
        assert!((90e3..=105e3).contains(&peak_power_w(&seabreeze)));
        assert!((140e3..=160e3).contains(&peak_power_w(&regent)));
        assert!(regent.mass_kg > seabreeze.mass_kg);
        assert!(regent.steering.return_rate < seabreeze.steering.return_rate);
        assert!(regent.feel.body_roll > seabreeze.feel.body_roll);
        assert_eq!((seabreeze.gear_count(), regent.gear_count()), (3, 4));
    }

    #[test]
    fn the_sports_cars_have_open_versions() {
        for (closed, open) in [
            ("avocet_v8", "avocet_spider"),
            ("basilisk_v12", "basilisk_roadster"),
            ("longbow_coupe", "longbow_convertible"),
            ("appaloosa_gt", "appaloosa_convertible"),
            ("halberd_coupe", "halberd_targa"),
        ] {
            let (c, o) = (builtin_spec(closed).unwrap(), builtin_spec(open).unwrap());
            let (cs, os) = (builtin_shape(&c).unwrap(), builtin_shape(&o).unwrap());
            let open_top = |s: &ShapeSpec| s.roof.convertible() || s.details.contains(&Targa);
            assert!(!open_top(&cs), "{closed}");
            assert!(open_top(&os), "{open}");
            assert_eq!(o.class, VehicleClass::Convertible);
            // The same car with its roof off: the chassis and engine, a little heavier.
            assert_eq!(c.engine.torque_nm, o.engine.torque_nm, "{open}");
            assert_eq!(c.gears, o.gears, "{open}");
            assert_eq!((c.wheelbase_m, c.track_m), (o.wheelbase_m, o.track_m));
            assert!(o.mass_kg > c.mass_kg && o.aero.cd > c.aero.cd, "{open}");
        }
    }

    #[test]
    fn the_roster_covers_the_eras_body_styles() {
        use VehicleClass::*;
        let specs: Vec<VehicleSpec> = BUILTIN
            .iter()
            .map(|(id, _)| builtin_spec(id).unwrap())
            .collect();
        assert_eq!(specs.len(), 36);
        for class in [
            Sports,
            Coupe,
            Hatch,
            Sedan,
            Wagon,
            Van,
            Pickup,
            Convertible,
            Utility,
        ] {
            assert!(specs.iter().any(|s| s.class == class), "{class:?}");
        }
        for (k, s) in specs.iter().enumerate() {
            // The 1980s roster, then the 2001 heroes and traffic cars.
            let eighties = k < 20;
            assert_eq!(s.era == Era::Eighties, eighties, "{}", s.id);
            assert!(specs[k + 1..]
                .iter()
                .all(|o| o.id != s.id && o.name != s.name));
        }
    }

    #[test]
    fn builtins_load_by_id() {
        for (id, _) in BUILTIN {
            assert_eq!(builtin_spec(id).unwrap().id, id);
        }
        assert_eq!(
            builtin_spec("hovercar").unwrap_err(),
            "no vehicle `hovercar`"
        );
    }

    #[test]
    fn every_builtin_has_its_shape() {
        for (id, _) in BUILTIN {
            let spec = builtin_spec(id).unwrap();
            assert_eq!(builtin_shape(&spec).unwrap().id, id);
        }
        let mut spec = builtin_spec("gannet_gt").unwrap();
        spec.shape = "hovercar.shape.ron".into();
        assert!(builtin_shape(&spec).is_err());
        assert!(load_shape("ShapeSpec(id: ").is_err());
    }

    #[test]
    fn rejects_bad_text() {
        let err = load_spec("VehicleSpec(id: ").unwrap_err();
        assert!(err.starts_with("vehicle spec: "), "{err}");

        let typo = GANNET.replace("wheelbase_m:", "wheelbse_m:");
        assert!(load_spec(&typo).is_err());

        let negative = GANNET.replace("mass_kg: 1250", "mass_kg: -1250");
        assert_eq!(
            load_spec(&negative).unwrap_err(),
            "vehicle spec `gannet_gt`: NonPositiveMass"
        );
    }
}
