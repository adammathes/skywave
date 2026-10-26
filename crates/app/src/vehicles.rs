//! Loading [`VehicleSpec`]s from the RON files in `assets/vehicles/`.

use vehicle::VehicleSpec;

/// Parses a RON `VehicleSpec` and validates it. The error names the spec and, for a parse
/// failure, the line and column.
pub fn load_spec(text: &str) -> Result<VehicleSpec, String> {
    let spec: VehicleSpec = ron::from_str(text).map_err(|e| format!("vehicle spec: {e}"))?;
    spec.validate()
        .map_err(|e| format!("vehicle spec `{}`: {e:?}", spec.id))?;
    Ok(spec)
}

/// The vehicle specs compiled into the game, as `(id, RON text)`.
pub const BUILTIN: [(&str, &str); 2] = [
    (
        "corsair_gtx",
        include_str!("../../../assets/vehicles/corsair_gtx.ron"),
    ),
    (
        "palisade_wagon",
        include_str!("../../../assets/vehicles/palisade_wagon.ron"),
    ),
];

/// Loads the built-in spec with this id.
pub fn builtin_spec(id: &str) -> Result<VehicleSpec, String> {
    let (_, text) = BUILTIN
        .iter()
        .find(|(name, _)| *name == id)
        .ok_or_else(|| format!("no vehicle `{id}`"))?;
    load_spec(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vehicle::{Drive, Era, VehicleClass};

    const CORSAIR: &str = include_str!("../../../assets/vehicles/corsair_gtx.ron");
    const PALISADE: &str = include_str!("../../../assets/vehicles/palisade_wagon.ron");

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
            (CORSAIR, "corsair_gtx", "corsair_gtx.shape.ron"),
            (PALISADE, "palisade_wagon", "palisade_wagon.shape.ron"),
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
        let sports = load_spec(CORSAIR).unwrap();
        let wagon = load_spec(PALISADE).unwrap();

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
        let sports = load_spec(CORSAIR).unwrap();
        let wagon = load_spec(PALISADE).unwrap();

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
    fn rejects_bad_text() {
        let err = load_spec("VehicleSpec(id: ").unwrap_err();
        assert!(err.starts_with("vehicle spec: "), "{err}");

        let typo = CORSAIR.replace("wheelbase_m:", "wheelbse_m:");
        assert!(load_spec(&typo).is_err());

        let negative = CORSAIR.replace("mass_kg: 1250", "mass_kg: -1250");
        assert_eq!(
            load_spec(&negative).unwrap_err(),
            "vehicle spec `corsair_gtx`: NonPositiveMass"
        );
    }
}
