//! `VehicleSpec`: everything that makes one car feel unlike another, as plain data.
//!
//! Field names and nesting follow the RON layout in `docs/VEHICLES.md`; RON struct names
//! (`Engine(..)`, `Susp(..)`, ...) are the Rust type names. Units are SI unless the field
//! name says otherwise (`_deg`, `_ms`, `_rpm`).

use alloc::string::String;
use alloc::vec::Vec;

/// The decade a car belongs to. Selects rosters, interiors and radio presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Era {
    Eighties,
    Nineties,
    Aughts,
    Tens,
    Twenties,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum VehicleClass {
    Sports,
    Sedan,
    Wagon,
    Coupe,
    Pickup,
    Van,
    Hatch,
    Convertible,
    Luxury,
    Muscle,
    /// A small off-roader: short, tall, four-wheel drive.
    Utility,
}

/// Which axle the engine drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Drive {
    Front,
    Rear,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum SpeedUnit {
    Mph,
    Kmh,
}

/// One dial in the instrument cluster.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Gauge {
    /// Speedometer: `(min, max, unit)` printed on the face.
    Speedo(f32, f32, SpeedUnit),
    /// Tachometer: `(min, max)` in rpm.
    Tach(f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Engine {
    pub idle_rpm: f32,
    /// Rev-limiter rpm.
    pub redline_rpm: f32,
    /// Full-throttle torque curve as `(rpm, Nm)` points, rpm strictly increasing. Linearly
    /// interpolated between points and held flat beyond the ends; see [`Engine::torque_at`].
    pub torque_nm: Vec<(f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Tires {
    /// Peak lateral friction coefficient.
    pub grip_lat: f32,
    /// Peak longitudinal friction coefficient.
    pub grip_long: f32,
    /// Fraction of grip lost per unit of load above static load, in `[0, 1)`.
    pub load_sens: f32,
    /// Slip angle at peak lateral force; force falls off beyond it.
    pub slip_peak_deg: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Susp {
    /// Spring rate per wheel, N/m.
    pub spring_n_m: f32,
    /// Damping per wheel, N·s/m.
    pub damper: f32,
    /// Suspension travel, m.
    pub travel_m: f32,
    /// Anti-roll bar stiffness as a fraction of the spring rate, in `[0, 1]`.
    pub antiroll: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Steer {
    /// Road-wheel lock at standstill, degrees, in `(0, 60]`.
    pub max_deg: f32,
    /// Fraction of lock removed at 60 m/s, in `[0, 1]`.
    pub speed_sens: f32,
    /// How fast the road-wheel angle chases its target, 1/s. Low is lazy, high is darty.
    pub return_rate: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Brakes {
    /// Total brake torque over all four wheels at full pedal, N·m.
    pub max_nm: f32,
    /// Share of `max_nm` on the front axle, in `[0, 1]`.
    pub bias_front: f32,
    pub abs: bool,
    /// Total handbrake torque on the rear axle, N·m.
    pub handbrake_nm: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Aero {
    /// Drag coefficient.
    pub cd: f32,
    pub frontal_m2: f32,
    /// Downforce coefficient (same reference area as drag), `>= 0`.
    pub downforce_coef: f32,
}

/// Knobs that exist for feel rather than physics.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Feel {
    /// In `[-1, 1]`. Positive scales rear lateral grip down (tail-happy); negative scales it
    /// up relative to the front (understeer by design).
    pub oversteer_bias: f32,
    /// Visible body and camera roll, in `[0, 1]`: 0 is a go-kart, 1 a sofa.
    pub body_roll: f32,
    /// Engine sound preset id.
    pub engine_sound: String,
    /// How hard a gear change kicks the cabin, in `[0, 1]`.
    pub shift_jerk: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Interior {
    /// Dashboard preset id.
    pub dash: String,
    pub gauges: Vec<Gauge>,
    /// Radio head-unit preset id.
    pub radio: String,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct VehicleSpec {
    /// Stable snake_case id, also the file stem.
    pub id: String,
    /// Display name.
    pub name: String,
    pub era: Era,
    pub class: VehicleClass,
    pub mass_kg: f32,
    /// Body box `(length, width, height)`, m.
    pub dims_m: [f32; 3],
    pub wheelbase_m: f32,
    /// Distance between left and right wheel centers (same front and rear), m.
    pub track_m: f32,
    /// Center of mass above the ground at rest, m.
    pub cg_height_m: f32,
    /// Center of mass behind the front axle as a fraction of the wheelbase, in `(0, 1)`.
    /// Static front weight share is `1 - cg_fore_aft`.
    pub cg_fore_aft: f32,
    pub engine: Engine,
    /// Forward gear ratios, first to top, strictly decreasing.
    pub gears: Vec<f32>,
    pub final_drive: f32,
    /// Reverse ratio magnitude.
    pub reverse: f32,
    /// Torque cut per shift, ms.
    pub shift_ms: f32,
    /// Automatic gearbox.
    pub auto: bool,
    pub drive: Drive,
    pub tires: Tires,
    pub suspension: Susp,
    pub steering: Steer,
    pub brakes: Brakes,
    pub aero: Aero,
    pub feel: Feel,
    pub interior: Interior,
    /// `ShapeSpec` file name, relative to `assets/vehicles/`.
    pub shape: String,
}

/// Why [`VehicleSpec::validate`] rejected a spec. Checks run in declaration order and the
/// first failure is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecError {
    EmptyId,
    EmptyName,
    EmptyShape,
    /// Some number is NaN or infinite.
    NonFinite,
    NonPositiveMass,
    NonPositiveDims,
    NonPositiveWheelbase,
    NonPositiveTrack,
    NonPositiveCgHeight,
    WheelbaseExceedsLength,
    TrackExceedsWidth,
    CgAboveRoof,
    CgForeAftOutOfRange,
    NonPositiveIdleRpm,
    RedlineNotAboveIdle,
    EmptyTorqueCurve,
    /// Point `index` does not have a higher rpm than the point before it.
    TorqueRpmNotIncreasing {
        index: u32,
    },
    NegativeTorque {
        index: u32,
    },
    NoGears,
    NonPositiveGear {
        index: u32,
    },
    /// Gear `index` is not a lower ratio than the gear before it.
    GearsNotDecreasing {
        index: u32,
    },
    NonPositiveFinalDrive,
    NonPositiveReverse,
    NegativeShiftTime,
    NonPositiveGrip,
    LoadSensOutOfRange,
    NonPositiveSlipPeak,
    NonPositiveSuspension,
    AntirollOutOfRange,
    SteerLockOutOfRange,
    SpeedSensOutOfRange,
    NonPositiveReturnRate,
    NonPositiveBrakeTorque,
    NegativeHandbrake,
    BrakeBiasOutOfRange,
    NegativeAero,
    FeelOutOfRange,
    /// Gauge `index` has `min >= max` or a negative `min`.
    GaugeRange {
        index: u32,
    },
}

/// `lo <= v <= hi`; false for NaN.
fn within(v: f32, lo: f32, hi: f32) -> bool {
    v >= lo && v <= hi
}

impl Engine {
    /// Full-throttle torque at `rpm`, N·m: linear between curve points, clamped to the first
    /// and last point outside them. `0.0` for an empty curve.
    pub fn torque_at(&self, rpm: f32) -> f32 {
        let curve = &self.torque_nm;
        let Some(&(mut r0, mut t0)) = curve.first() else {
            return 0.0;
        };
        if rpm <= r0 {
            return t0;
        }
        for &(r1, t1) in &curve[1..] {
            if rpm <= r1 {
                return t0 + (t1 - t0) * ((rpm - r0) / (r1 - r0));
            }
            r0 = r1;
            t0 = t1;
        }
        t0
    }
}

impl VehicleSpec {
    /// Checks that the spec is physically sane. Does not judge whether it is fun.
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.id.is_empty() {
            return Err(SpecError::EmptyId);
        }
        if self.name.is_empty() {
            return Err(SpecError::EmptyName);
        }
        if self.shape.is_empty() {
            return Err(SpecError::EmptyShape);
        }
        if !self.all_finite() {
            return Err(SpecError::NonFinite);
        }

        let [length, width, height] = self.dims_m;
        if self.mass_kg <= 0.0 {
            return Err(SpecError::NonPositiveMass);
        }
        if length <= 0.0 || width <= 0.0 || height <= 0.0 {
            return Err(SpecError::NonPositiveDims);
        }
        if self.wheelbase_m <= 0.0 {
            return Err(SpecError::NonPositiveWheelbase);
        }
        if self.track_m <= 0.0 {
            return Err(SpecError::NonPositiveTrack);
        }
        if self.cg_height_m <= 0.0 {
            return Err(SpecError::NonPositiveCgHeight);
        }
        if self.wheelbase_m > length {
            return Err(SpecError::WheelbaseExceedsLength);
        }
        if self.track_m > width {
            return Err(SpecError::TrackExceedsWidth);
        }
        if self.cg_height_m >= height {
            return Err(SpecError::CgAboveRoof);
        }
        if self.cg_fore_aft <= 0.0 || self.cg_fore_aft >= 1.0 {
            return Err(SpecError::CgForeAftOutOfRange);
        }

        let e = &self.engine;
        if e.idle_rpm <= 0.0 {
            return Err(SpecError::NonPositiveIdleRpm);
        }
        if e.redline_rpm <= e.idle_rpm {
            return Err(SpecError::RedlineNotAboveIdle);
        }
        if e.torque_nm.is_empty() {
            return Err(SpecError::EmptyTorqueCurve);
        }
        for (i, &(rpm, nm)) in e.torque_nm.iter().enumerate() {
            let index = i as u32;
            if i > 0 && rpm <= e.torque_nm[i - 1].0 {
                return Err(SpecError::TorqueRpmNotIncreasing { index });
            }
            if nm < 0.0 {
                return Err(SpecError::NegativeTorque { index });
            }
        }

        if self.gears.is_empty() {
            return Err(SpecError::NoGears);
        }
        for (i, &ratio) in self.gears.iter().enumerate() {
            let index = i as u32;
            if ratio <= 0.0 {
                return Err(SpecError::NonPositiveGear { index });
            }
            if i > 0 && ratio >= self.gears[i - 1] {
                return Err(SpecError::GearsNotDecreasing { index });
            }
        }
        if self.final_drive <= 0.0 {
            return Err(SpecError::NonPositiveFinalDrive);
        }
        if self.reverse <= 0.0 {
            return Err(SpecError::NonPositiveReverse);
        }
        if self.shift_ms < 0.0 {
            return Err(SpecError::NegativeShiftTime);
        }

        let t = &self.tires;
        if t.grip_lat <= 0.0 || t.grip_long <= 0.0 {
            return Err(SpecError::NonPositiveGrip);
        }
        if !(t.load_sens >= 0.0 && t.load_sens < 1.0) {
            return Err(SpecError::LoadSensOutOfRange);
        }
        if t.slip_peak_deg <= 0.0 {
            return Err(SpecError::NonPositiveSlipPeak);
        }

        let s = &self.suspension;
        if s.spring_n_m <= 0.0 || s.damper <= 0.0 || s.travel_m <= 0.0 {
            return Err(SpecError::NonPositiveSuspension);
        }
        if !within(s.antiroll, 0.0, 1.0) {
            return Err(SpecError::AntirollOutOfRange);
        }

        let st = &self.steering;
        if st.max_deg <= 0.0 || st.max_deg > 60.0 {
            return Err(SpecError::SteerLockOutOfRange);
        }
        if !within(st.speed_sens, 0.0, 1.0) {
            return Err(SpecError::SpeedSensOutOfRange);
        }
        if st.return_rate <= 0.0 {
            return Err(SpecError::NonPositiveReturnRate);
        }

        let b = &self.brakes;
        if b.max_nm <= 0.0 {
            return Err(SpecError::NonPositiveBrakeTorque);
        }
        if b.handbrake_nm < 0.0 {
            return Err(SpecError::NegativeHandbrake);
        }
        if !within(b.bias_front, 0.0, 1.0) {
            return Err(SpecError::BrakeBiasOutOfRange);
        }

        let a = &self.aero;
        if a.cd < 0.0 || a.frontal_m2 < 0.0 || a.downforce_coef < 0.0 {
            return Err(SpecError::NegativeAero);
        }

        let f = &self.feel;
        if !within(f.oversteer_bias, -1.0, 1.0)
            || !within(f.body_roll, 0.0, 1.0)
            || !within(f.shift_jerk, 0.0, 1.0)
        {
            return Err(SpecError::FeelOutOfRange);
        }

        for (i, g) in self.interior.gauges.iter().enumerate() {
            let (min, max) = match *g {
                Gauge::Speedo(min, max, _) => (min, max),
                Gauge::Tach(min, max) => (min, max),
            };
            if min < 0.0 || min >= max {
                return Err(SpecError::GaugeRange { index: i as u32 });
            }
        }
        Ok(())
    }

    fn all_finite(&self) -> bool {
        let t = &self.tires;
        let s = &self.suspension;
        let st = &self.steering;
        let b = &self.brakes;
        let a = &self.aero;
        let f = &self.feel;
        let scalars = [
            self.mass_kg,
            self.dims_m[0],
            self.dims_m[1],
            self.dims_m[2],
            self.wheelbase_m,
            self.track_m,
            self.cg_height_m,
            self.cg_fore_aft,
            self.engine.idle_rpm,
            self.engine.redline_rpm,
            self.final_drive,
            self.reverse,
            self.shift_ms,
            t.grip_lat,
            t.grip_long,
            t.load_sens,
            t.slip_peak_deg,
            s.spring_n_m,
            s.damper,
            s.travel_m,
            s.antiroll,
            st.max_deg,
            st.speed_sens,
            st.return_rate,
            b.max_nm,
            b.bias_front,
            b.handbrake_nm,
            a.cd,
            a.frontal_m2,
            a.downforce_coef,
            f.oversteer_bias,
            f.body_roll,
            f.shift_jerk,
        ];
        let gauges_finite = self.interior.gauges.iter().all(|g| match *g {
            Gauge::Speedo(min, max, _) => min.is_finite() && max.is_finite(),
            Gauge::Tach(min, max) => min.is_finite() && max.is_finite(),
        });
        scalars.iter().all(|v| v.is_finite())
            && self.gears.iter().all(|v| v.is_finite())
            && self
                .engine
                .torque_nm
                .iter()
                .all(|&(rpm, nm)| rpm.is_finite() && nm.is_finite())
            && gauges_finite
    }

    /// Diagonal inertia tensor `[Ixx, Iyy, Izz]` (pitch, yaw, roll) in body space, kg·m²,
    /// of a solid box of `dims_m` and `mass_kg` centered on the center of mass.
    pub fn inertia_box(&self) -> [f32; 3] {
        let [l, w, h] = self.dims_m;
        let k = self.mass_kg / 12.0;
        [
            k * (h * h + l * l),
            k * (w * w + l * l),
            k * (w * w + h * h),
        ]
    }

    /// Tire contact points at rest in body space, m: front-left, front-right, rear-left,
    /// rear-right. Ground is `cg_height_m` below the center of mass.
    pub fn wheel_positions(&self) -> [[f32; 3]; 4] {
        let x = self.track_m * 0.5;
        let y = -self.cg_height_m;
        let front_z = -self.cg_fore_aft * self.wheelbase_m;
        let rear_z = (1.0 - self.cg_fore_aft) * self.wheelbase_m;
        [
            [-x, y, front_z],
            [x, y, front_z],
            [-x, y, rear_z],
            [x, y, rear_z],
        ]
    }

    /// Ratio of the highest forward gear (excluding the final drive); `0.0` with no gears.
    pub fn top_gear_ratio(&self) -> f32 {
        self.gears.last().copied().unwrap_or(0.0)
    }

    /// Number of forward gears.
    pub fn gear_count(&self) -> usize {
        self.gears.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    /// A valid spec to mutate; the numbers match the sketch in `docs/VEHICLES.md`.
    fn sample() -> VehicleSpec {
        VehicleSpec {
            id: "test_car".to_string(),
            name: "Test Car".to_string(),
            era: Era::Eighties,
            class: VehicleClass::Sports,
            mass_kg: 1250.0,
            dims_m: [4.3, 1.75, 1.15],
            wheelbase_m: 2.45,
            track_m: 1.5,
            cg_height_m: 0.45,
            cg_fore_aft: 0.48,
            engine: Engine {
                idle_rpm: 900.0,
                redline_rpm: 7200.0,
                torque_nm: vec![
                    (1000.0, 180.0),
                    (3000.0, 260.0),
                    (5500.0, 290.0),
                    (7000.0, 230.0),
                ],
            },
            gears: vec![3.1, 2.0, 1.4, 1.05, 0.85],
            final_drive: 3.7,
            reverse: 3.0,
            shift_ms: 250.0,
            auto: true,
            drive: Drive::Rear,
            tires: Tires {
                grip_lat: 1.25,
                grip_long: 1.15,
                load_sens: 0.08,
                slip_peak_deg: 7.0,
            },
            suspension: Susp {
                spring_n_m: 32000.0,
                damper: 3200.0,
                travel_m: 0.16,
                antiroll: 0.3,
            },
            steering: Steer {
                max_deg: 32.0,
                speed_sens: 0.6,
                return_rate: 8.0,
            },
            brakes: Brakes {
                max_nm: 3200.0,
                bias_front: 0.62,
                abs: false,
                handbrake_nm: 1800.0,
            },
            aero: Aero {
                cd: 0.34,
                frontal_m2: 1.9,
                downforce_coef: 0.02,
            },
            feel: Feel {
                oversteer_bias: 0.2,
                body_roll: 0.6,
                engine_sound: "v8_flatplane".to_string(),
                shift_jerk: 0.3,
            },
            interior: Interior {
                dash: "sport_80s".to_string(),
                gauges: vec![
                    Gauge::Speedo(0.0, 160.0, SpeedUnit::Mph),
                    Gauge::Tach(0.0, 8000.0),
                ],
                radio: "digital_green".to_string(),
            },
            shape: "test_car.shape.ron".to_string(),
        }
    }

    /// Applies `f` to a valid spec and asserts the validator reports `expected`.
    fn rejects(f: impl FnOnce(&mut VehicleSpec), expected: SpecError) {
        let mut s = sample();
        f(&mut s);
        assert_eq!(s.validate(), Err(expected));
    }

    #[test]
    fn sample_is_valid() {
        assert_eq!(sample().validate(), Ok(()));
    }

    #[test]
    fn rejects_empty_strings() {
        rejects(|s| s.id.clear(), SpecError::EmptyId);
        rejects(|s| s.name.clear(), SpecError::EmptyName);
        rejects(|s| s.shape.clear(), SpecError::EmptyShape);
    }

    #[test]
    fn rejects_non_finite_numbers() {
        rejects(|s| s.mass_kg = f32::NAN, SpecError::NonFinite);
        rejects(|s| s.dims_m[2] = f32::INFINITY, SpecError::NonFinite);
        rejects(|s| s.gears[1] = f32::NAN, SpecError::NonFinite);
        rejects(|s| s.engine.torque_nm[2].1 = f32::NAN, SpecError::NonFinite);
        rejects(
            |s| s.feel.body_roll = f32::NEG_INFINITY,
            SpecError::NonFinite,
        );
        rejects(
            |s| s.interior.gauges[0] = Gauge::Speedo(0.0, f32::NAN, SpeedUnit::Kmh),
            SpecError::NonFinite,
        );
    }

    #[test]
    fn rejects_non_positive_mass() {
        rejects(|s| s.mass_kg = 0.0, SpecError::NonPositiveMass);
        rejects(|s| s.mass_kg = -1250.0, SpecError::NonPositiveMass);
    }

    #[test]
    fn rejects_bad_geometry() {
        rejects(|s| s.dims_m[0] = 0.0, SpecError::NonPositiveDims);
        rejects(|s| s.dims_m[1] = -1.0, SpecError::NonPositiveDims);
        rejects(|s| s.dims_m[2] = 0.0, SpecError::NonPositiveDims);
        rejects(|s| s.wheelbase_m = 0.0, SpecError::NonPositiveWheelbase);
        rejects(|s| s.track_m = -1.5, SpecError::NonPositiveTrack);
        rejects(|s| s.cg_height_m = 0.0, SpecError::NonPositiveCgHeight);
        rejects(|s| s.wheelbase_m = 4.5, SpecError::WheelbaseExceedsLength);
        rejects(|s| s.track_m = 1.8, SpecError::TrackExceedsWidth);
        rejects(|s| s.cg_height_m = 1.15, SpecError::CgAboveRoof);
        rejects(|s| s.cg_fore_aft = 0.0, SpecError::CgForeAftOutOfRange);
        rejects(|s| s.cg_fore_aft = 1.0, SpecError::CgForeAftOutOfRange);
        rejects(|s| s.cg_fore_aft = -0.2, SpecError::CgForeAftOutOfRange);
    }

    #[test]
    fn rejects_bad_engine() {
        rejects(|s| s.engine.idle_rpm = 0.0, SpecError::NonPositiveIdleRpm);
        rejects(
            |s| s.engine.redline_rpm = 900.0,
            SpecError::RedlineNotAboveIdle,
        );
        rejects(
            |s| s.engine.redline_rpm = 500.0,
            SpecError::RedlineNotAboveIdle,
        );
        rejects(|s| s.engine.torque_nm.clear(), SpecError::EmptyTorqueCurve);
        rejects(
            |s| s.engine.torque_nm[2].0 = 3000.0,
            SpecError::TorqueRpmNotIncreasing { index: 2 },
        );
        rejects(
            |s| s.engine.torque_nm.swap(0, 1),
            SpecError::TorqueRpmNotIncreasing { index: 1 },
        );
        rejects(
            |s| s.engine.torque_nm[3].1 = -1.0,
            SpecError::NegativeTorque { index: 3 },
        );
    }

    #[test]
    fn rejects_bad_gearing() {
        rejects(|s| s.gears.clear(), SpecError::NoGears);
        rejects(
            |s| s.gears[0] = 0.0,
            SpecError::NonPositiveGear { index: 0 },
        );
        rejects(
            |s| s.gears[2] = 2.0,
            SpecError::GearsNotDecreasing { index: 2 },
        );
        rejects(
            |s| s.gears.reverse(),
            SpecError::GearsNotDecreasing { index: 1 },
        );
        rejects(
            |s| s.gears.push(0.85),
            SpecError::GearsNotDecreasing { index: 5 },
        );
        rejects(|s| s.final_drive = 0.0, SpecError::NonPositiveFinalDrive);
        rejects(|s| s.reverse = -3.0, SpecError::NonPositiveReverse);
        rejects(|s| s.shift_ms = -1.0, SpecError::NegativeShiftTime);
    }

    #[test]
    fn rejects_bad_tires() {
        rejects(|s| s.tires.grip_lat = 0.0, SpecError::NonPositiveGrip);
        rejects(|s| s.tires.grip_long = -1.0, SpecError::NonPositiveGrip);
        rejects(|s| s.tires.load_sens = 1.0, SpecError::LoadSensOutOfRange);
        rejects(|s| s.tires.load_sens = -0.1, SpecError::LoadSensOutOfRange);
        rejects(
            |s| s.tires.slip_peak_deg = 0.0,
            SpecError::NonPositiveSlipPeak,
        );
    }

    #[test]
    fn rejects_bad_suspension() {
        rejects(
            |s| s.suspension.spring_n_m = 0.0,
            SpecError::NonPositiveSuspension,
        );
        rejects(
            |s| s.suspension.damper = -1.0,
            SpecError::NonPositiveSuspension,
        );
        rejects(
            |s| s.suspension.travel_m = 0.0,
            SpecError::NonPositiveSuspension,
        );
        rejects(
            |s| s.suspension.antiroll = 1.5,
            SpecError::AntirollOutOfRange,
        );
        rejects(
            |s| s.suspension.antiroll = -0.1,
            SpecError::AntirollOutOfRange,
        );
    }

    #[test]
    fn rejects_bad_steering() {
        rejects(|s| s.steering.max_deg = 0.0, SpecError::SteerLockOutOfRange);
        rejects(
            |s| s.steering.max_deg = 75.0,
            SpecError::SteerLockOutOfRange,
        );
        rejects(
            |s| s.steering.speed_sens = 1.2,
            SpecError::SpeedSensOutOfRange,
        );
        rejects(
            |s| s.steering.speed_sens = -0.1,
            SpecError::SpeedSensOutOfRange,
        );
        rejects(
            |s| s.steering.return_rate = 0.0,
            SpecError::NonPositiveReturnRate,
        );
    }

    #[test]
    fn rejects_bad_brakes() {
        rejects(|s| s.brakes.max_nm = 0.0, SpecError::NonPositiveBrakeTorque);
        rejects(
            |s| s.brakes.handbrake_nm = -1.0,
            SpecError::NegativeHandbrake,
        );
        rejects(
            |s| s.brakes.bias_front = 1.1,
            SpecError::BrakeBiasOutOfRange,
        );
        rejects(
            |s| s.brakes.bias_front = -0.1,
            SpecError::BrakeBiasOutOfRange,
        );
    }

    #[test]
    fn brake_bias_bounds_are_inclusive() {
        for bias in [0.0, 1.0] {
            let mut s = sample();
            s.brakes.bias_front = bias;
            assert_eq!(s.validate(), Ok(()));
        }
    }

    #[test]
    fn rejects_bad_aero_and_feel() {
        rejects(|s| s.aero.cd = -0.3, SpecError::NegativeAero);
        rejects(|s| s.aero.frontal_m2 = -1.0, SpecError::NegativeAero);
        rejects(|s| s.aero.downforce_coef = -0.1, SpecError::NegativeAero);
        rejects(|s| s.feel.oversteer_bias = 1.5, SpecError::FeelOutOfRange);
        rejects(|s| s.feel.oversteer_bias = -1.5, SpecError::FeelOutOfRange);
        rejects(|s| s.feel.body_roll = 2.0, SpecError::FeelOutOfRange);
        rejects(|s| s.feel.shift_jerk = -0.5, SpecError::FeelOutOfRange);
    }

    #[test]
    fn rejects_bad_gauges() {
        rejects(
            |s| s.interior.gauges[1] = Gauge::Tach(8000.0, 0.0),
            SpecError::GaugeRange { index: 1 },
        );
        rejects(
            |s| s.interior.gauges[0] = Gauge::Speedo(-10.0, 160.0, SpeedUnit::Mph),
            SpecError::GaugeRange { index: 0 },
        );
    }

    #[test]
    fn torque_interpolates_and_clamps() {
        let e = sample().engine;
        assert_eq!(e.torque_at(0.0), 180.0);
        assert_eq!(e.torque_at(1000.0), 180.0);
        assert_eq!(e.torque_at(2000.0), 220.0);
        assert_eq!(e.torque_at(3000.0), 260.0);
        assert_eq!(e.torque_at(4250.0), 275.0);
        assert_eq!(e.torque_at(6250.0), 260.0);
        assert_eq!(e.torque_at(7000.0), 230.0);
        assert_eq!(e.torque_at(9000.0), 230.0);

        let single = Engine {
            torque_nm: vec![(2000.0, 100.0)],
            ..e.clone()
        };
        assert_eq!(single.torque_at(500.0), 100.0);
        assert_eq!(single.torque_at(5000.0), 100.0);

        let empty = Engine {
            torque_nm: Vec::new(),
            ..e
        };
        assert_eq!(empty.torque_at(3000.0), 0.0);
    }

    #[test]
    fn box_inertia() {
        let mut s = sample();
        s.mass_kg = 12.0;
        s.dims_m = [4.0, 2.0, 1.0];
        assert_eq!(s.inertia_box(), [17.0, 20.0, 5.0]);

        // Yaw inertia is the largest for any car-shaped box.
        let [pitch, yaw, roll] = sample().inertia_box();
        assert!(yaw > pitch && pitch > roll && roll > 0.0);
    }

    #[test]
    fn wheels_sit_around_the_center_of_mass() {
        let s = sample();
        let w = s.wheel_positions();
        let (fl, fr, rl, rr) = (w[0], w[1], w[2], w[3]);
        assert_eq!(fl[0], -0.75);
        assert_eq!(fr[0], 0.75);
        assert_eq!(rl[0], -0.75);
        assert_eq!(rr[0], 0.75);
        for p in w {
            assert_eq!(p[1], -0.45);
        }
        // Front is -z; the axles straddle the center of mass and span the wheelbase.
        assert!(fl[2] < 0.0 && rl[2] > 0.0);
        assert_eq!(fl[2], fr[2]);
        assert_eq!(rl[2], rr[2]);
        assert!((rl[2] - fl[2] - s.wheelbase_m).abs() < 1e-6);
        assert!((-fl[2] - 0.48 * 2.45).abs() < 1e-6);
    }

    #[test]
    fn gear_helpers() {
        let mut s = sample();
        assert_eq!(s.gear_count(), 5);
        assert_eq!(s.top_gear_ratio(), 0.85);
        s.gears.clear();
        assert_eq!(s.gear_count(), 0);
        assert_eq!(s.top_gear_ratio(), 0.0);
    }
}
