//! What the pause menu's Vehicle page shows: every built-in car
//! ([`crate::vehicles::BUILTIN`], in its order, so a car added there shows up here) with its
//! name, a one-line stat strip, a short spec summary and its paints, read from the specs
//! and shapes once. The 0-60 mph times and top speeds are [`MEASURED`] with the real sim,
//! flat out from rest on flat ground with the default assists (as `vehicle/tests/roster.rs`
//! does); a test runs it again, so the page never disagrees with the car. (Measuring at
//! runtime takes half a second.)

use std::sync::OnceLock;

use vehicle::{Drive, Era, VehicleSpec};

use crate::vehicles::{self, BUILTIN};

/// Most paints a built-in car's palette has (the Vehicle page's paint chooser has a slot
/// for each).
pub const MAX_PAINTS: usize = 12;

/// `(id, seconds from rest to 60 mph, top speed in mph)` of every built-in car, as the sim
/// measures them (see the module docs).
const MEASURED: [(&str, f32, f32); 12] = [
    ("corsair_gtx", 5.60, 155.0),
    ("palisade_wagon", 12.60, 110.0),
    ("seabreeze_convertible", 12.51, 87.0),
    ("regent_coupe_cabrio", 9.88, 122.0),
    ("halcyon_turbo", 6.83, 146.0),
    ("pipit_hatch", 8.78, 108.0),
    ("bancroft_sedan", 10.49, 117.0),
    ("prescott_landau", 11.41, 115.0),
    ("meadow_minivan", 14.98, 98.0),
    ("buckboard_pickup", 9.06, 104.0),
    ("drayman_van", 13.52, 83.0),
    ("burro_4x4", 12.13, 85.0),
];

/// One row of the Vehicle page.
#[derive(Clone, Debug, PartialEq)]
pub struct RosterEntry {
    /// The [`BUILTIN`] id.
    pub id: &'static str,
    /// The spec's name ("Palisade Wagon").
    pub name: String,
    /// Class, power to weight, driven wheels and gearbox: [`stat_strip`].
    pub stats: String,
    /// Power to weight and driven wheels only, for a narrow menu (a large UI scale).
    pub short: String,
    /// "Convertible".
    pub class: String,
    /// "1980s".
    pub era: &'static str,
    /// Peak power and torque: "146 kW  350 Nm".
    pub engine: String,
    /// Driven wheels and gearbox: "FWD  4-spd auto".
    pub drive: String,
    /// Seconds from rest to 60 mph, flat out.
    pub zero_to_60_s: f32,
    /// Top speed, mph.
    pub top_mph: f32,
    /// The body palette as sRGB swatches, upper and lower (the same for one-tone paint), in
    /// palette order: the paints the Vehicle page offers.
    pub paints: Vec<[[u8; 3]; 2]>,
}

/// Every built-in car, in [`BUILTIN`] order (index `i` is `BUILTIN[i]`).
pub fn roster() -> &'static [RosterEntry] {
    static ROSTER: OnceLock<Vec<RosterEntry>> = OnceLock::new();
    ROSTER.get_or_init(|| {
        BUILTIN
            .iter()
            .map(|&(id, _)| match vehicles::builtin_spec(id) {
                Ok(spec) => entry(id, spec),
                Err(_) => RosterEntry {
                    id,
                    name: id.to_owned(),
                    stats: String::new(),
                    short: String::new(),
                    class: String::new(),
                    era: "",
                    engine: String::new(),
                    drive: String::new(),
                    zero_to_60_s: 0.0,
                    top_mph: 0.0,
                    paints: vec![[[128; 3]; 2]],
                },
            })
            .collect()
    })
}

/// The cars' names in [`BUILTIN`] order, for the Vehicle page's car chooser.
pub fn names() -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| roster().iter().map(|c| c.name.as_str()).collect())
}

fn entry(id: &'static str, spec: VehicleSpec) -> RosterEntry {
    let paints = vehicles::builtin_shape(&spec)
        .map(|shape| shape.palette.iter().map(|p| swatch(p)).collect())
        .unwrap_or_default();
    let (zero_to_60_s, top_mph) = MEASURED
        .iter()
        .find(|m| m.0 == id)
        .map_or((f32::NAN, f32::NAN), |m| (m.1, m.2));
    let gearbox = if spec.auto { "auto" } else { "manual" };
    RosterEntry {
        id,
        stats: stat_strip(&spec),
        short: format!("{:.0} kW/t  {}", kw_per_t(&spec), drive_name(&spec)),
        class: format!("{:?}", spec.class),
        era: era_name(spec.era),
        engine: format!(
            "{:.0} kW  {:.0} Nm",
            peak_power_w(&spec) * 1e-3,
            spec.engine
                .torque_nm
                .iter()
                .fold(0.0f32, |m, &(_, nm)| m.max(nm))
        ),
        drive: format!("{}  {}-spd {gearbox}", drive_name(&spec), spec.gear_count()),
        zero_to_60_s,
        top_mph,
        paints,
        name: spec.name,
    }
}

fn era_name(era: Era) -> &'static str {
    match era {
        Era::Eighties => "1980s",
        Era::Nineties => "1990s",
        Era::Aughts => "2000s",
        Era::Tens => "2010s",
        Era::Twenties => "2020s",
    }
}

/// A palette entry (`#rrggbb` or `#upper/#lower`) as `[upper, lower]` sRGB; grey where
/// it does not parse (the shape's validation rejects that, so never for a built-in car).
fn swatch(paint: &str) -> [[u8; 3]; 2] {
    let hex = |s: &str| -> Option<[u8; 3]> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let byte = |k: usize| u8::from_str_radix(s.get(k..k + 2)?, 16).ok();
        Some([byte(0)?, byte(2)?, byte(4)?])
    };
    let (upper, lower) = paint.split_once('/').unwrap_or((paint, paint));
    match (hex(upper), hex(lower)) {
        (Some(u), Some(l)) => [u, l],
        _ => [[128; 3]; 2],
    }
}

/// Index of the built-in car `id` in [`BUILTIN`] (and [`roster`]).
pub fn index_of(id: &str) -> Option<usize> {
    BUILTIN.iter().position(|(v, _)| *v == id)
}

/// Peak full-throttle power between idle and redline, W, sampled every 10 rpm.
pub fn peak_power_w(spec: &VehicleSpec) -> f32 {
    let e = &spec.engine;
    let mut best = 0.0f32;
    let mut rpm = e.idle_rpm;
    while rpm <= e.redline_rpm {
        best = best.max(e.torque_at(rpm) * rpm * (core::f32::consts::TAU / 60.0));
        rpm += 10.0;
    }
    best
}

/// Peak power per tonne of the car, kW/t.
fn kw_per_t(spec: &VehicleSpec) -> f32 {
    peak_power_w(spec) / spec.mass_kg.max(1.0)
}

fn drive_name(spec: &VehicleSpec) -> &'static str {
    match spec.drive {
        Drive::Front => "FWD",
        Drive::Rear => "RWD",
        Drive::All => "AWD",
    }
}

/// "Sports   134 kW/t   RWD   5-spd auto": the class, peak power per tonne, the driven
/// wheels and the gearbox.
pub fn stat_strip(spec: &VehicleSpec) -> String {
    let kw_per_t = kw_per_t(spec);
    let drive = drive_name(spec);
    let gearbox = if spec.auto { "auto" } else { "manual" };
    format!(
        "{:?}   {kw_per_t:.0} kW/t   {drive}   {}-spd {gearbox}",
        spec.class,
        spec.gear_count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use vehicle::sim::DT;
    use vehicle::{VehicleInput, VehicleState};

    /// Meters per second in one mile per hour.
    const MPS_PER_MPH: f32 = 0.447_04;

    #[test]
    fn the_roster_is_every_builtin_in_order() {
        let r = roster();
        assert_eq!(r.len(), BUILTIN.len());
        for (i, (id, _)) in BUILTIN.iter().enumerate() {
            assert_eq!(r[i].id, *id);
            assert_eq!(index_of(id), Some(i));
            assert!(!r[i].stats.is_empty(), "{id}");
            assert!(r[i].name.is_ascii(), "the HUD font is ASCII");
        }
        assert_eq!(index_of("hovercar"), None);
        let wagon = &r[index_of("palisade_wagon").unwrap()];
        assert_eq!(wagon.name, "Palisade Wagon");
        assert!(wagon.short.ends_with(" kW/t  RWD"), "{}", wagon.short);
        assert!(wagon.stats.starts_with("Wagon   "), "{}", wagon.stats);
        assert!(wagon.stats.ends_with("RWD   3-spd auto"), "{}", wagon.stats);
        let sports = &r[index_of("corsair_gtx").unwrap()];
        assert!(sports.stats.starts_with("Sports   "), "{}", sports.stats);
        assert!(
            sports.stats.ends_with("RWD   5-spd auto"),
            "{}",
            sports.stats
        );
        let mut manual = vehicles::builtin_spec("corsair_gtx").unwrap();
        manual.auto = false;
        assert!(stat_strip(&manual).ends_with("5-spd manual"));
    }

    /// Full throttle from rest on flat ground: seconds to 60 mph (`NaN` if never) and the
    /// speed it settles at, m/s (when a second adds under 0.01 m/s, or after 150 s).
    fn straight_line(spec: &VehicleSpec) -> (f32, f32) {
        const STEPS_PER_S: usize = 120;
        let ground = vehicle::FlatGround {
            y: 0.0,
            friction: 1.0,
        };
        let input = VehicleInput {
            throttle: 1.0,
            brake: 0.0,
            steer: 0.0,
            handbrake: false,
        };
        let assists = vehicle::Assists::default();
        let mut s = VehicleState::spawn(spec, [0.0; 3], 0.0);
        let mut t60 = f32::NAN;
        let mut second_ago = 0.0;
        for i in 0..150 * STEPS_PER_S {
            vehicle::step(spec, &mut s, &input, &assists, &ground);
            if t60.is_nan() && s.speed_mps >= 60.0 * MPS_PER_MPH {
                t60 = (i + 1) as f32 * DT;
            }
            if (i + 1) % STEPS_PER_S == 0 {
                if !t60.is_nan() && s.speed_mps - second_ago < 0.01 {
                    break;
                }
                second_ago = s.speed_mps;
            }
        }
        (t60, s.speed_mps)
    }

    #[test]
    fn the_measured_figures_match_the_sim() {
        // Run with `--nocapture` after retuning a car and paste the rows into MEASURED.
        let mut ok = true;
        for &(id, t60, top) in &MEASURED {
            let (t, v) = straight_line(&vehicles::builtin_spec(id).unwrap());
            let mph = v / MPS_PER_MPH;
            eprintln!("    (\"{id}\", {t:.2}, {mph:.0}),");
            ok &= (t - t60).abs() <= 0.05 && (mph - top).abs() <= 1.0;
        }
        assert!(ok, "MEASURED is out of date");
        for (id, _) in BUILTIN {
            assert!(MEASURED.iter().any(|m| m.0 == id), "{id}");
        }
    }

    #[test]
    fn the_spec_summary_matches_the_tuned_roster() {
        let r = roster();
        for car in r {
            assert!((4.0..20.0).contains(&car.zero_to_60_s), "{}", car.id);
            assert!((70.0..170.0).contains(&car.top_mph), "{}", car.id);
            assert!(
                !car.paints.is_empty() && car.paints.len() <= MAX_PAINTS,
                "{}",
                car.id
            );
            assert_eq!(car.era, "1980s");
        }
        // docs/VEHICLES.md: the Corsair 5.6 s and 70 m/s, the Regent 9.9 s and 55 m/s.
        let corsair = &r[index_of("corsair_gtx").unwrap()];
        assert!(
            (corsair.zero_to_60_s - 5.6).abs() < 0.2,
            "{}",
            corsair.zero_to_60_s
        );
        assert!((corsair.top_mph * MPS_PER_MPH - 70.0).abs() < 1.5);
        let regent = &r[index_of("regent_coupe_cabrio").unwrap()];
        assert!(
            (regent.zero_to_60_s - 9.9).abs() < 0.2,
            "{}",
            regent.zero_to_60_s
        );
        assert_eq!(regent.class, "Convertible");
        assert_eq!(regent.drive, "FWD  4-spd auto");
        assert!(regent.engine.ends_with(" kW  350 Nm"), "{}", regent.engine);
        // Ivory over burgundy first.
        assert_eq!(regent.paints[0], [[0xe8, 0xe1, 0xcf], [0x6e, 0x1a, 0x22]]);
        assert_eq!(regent.paints[1], [[0x6e, 0x1a, 0x22]; 2]);
        assert_eq!(names().len(), BUILTIN.len());
        assert_eq!(names()[3], "Regent Coupe Cabrio");
    }
}
