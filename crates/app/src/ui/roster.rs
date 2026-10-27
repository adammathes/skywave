//! What the pause menu's Vehicle page lists: every built-in car
//! ([`crate::vehicles::BUILTIN`], in its order, so a car added there shows up here) with its
//! name and a one-line stat strip, read from the specs once.

use std::sync::OnceLock;

use vehicle::{Drive, VehicleSpec};

use crate::vehicles::{self, BUILTIN};

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
}

/// Every built-in car, in [`BUILTIN`] order (index `i` is `BUILTIN[i]`).
pub fn roster() -> &'static [RosterEntry] {
    static ROSTER: OnceLock<Vec<RosterEntry>> = OnceLock::new();
    ROSTER.get_or_init(|| {
        BUILTIN
            .iter()
            .map(|&(id, _)| match vehicles::builtin_spec(id) {
                Ok(spec) => RosterEntry {
                    id,
                    stats: stat_strip(&spec),
                    short: format!("{:.0} kW/t  {}", kw_per_t(&spec), drive_name(&spec)),
                    name: spec.name,
                },
                Err(_) => RosterEntry {
                    id,
                    name: id.to_owned(),
                    stats: String::new(),
                    short: String::new(),
                },
            })
            .collect()
    })
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
}
