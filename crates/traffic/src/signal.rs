//! Fixed-cycle traffic signals.
//!
//! Every signalized junction runs a fixed cycle of its phases (two at a plain crossing,
//! more at a skewed or many-road junction), counted in 120 Hz sim ticks: phase 0 green,
//! yellow, all red, then phase 1, and so on. A junction's cycle starts at its own offset,
//! a hash of the place seed and its node, so neighboring lights do not change in lockstep.
//! The light a lane faces is a pure function of the tick, so the whole state of every
//! signal in the place is one [`SignalClock`].

use skywave_core::{hash64, SIM_HZ};

/// Green time per phase, ticks (25 s).
pub const GREEN_TICKS: u32 = 25 * SIM_HZ;
/// Yellow time per phase, ticks (3.5 s).
pub const YELLOW_TICKS: u32 = 7 * SIM_HZ / 2;
/// All-red clearance after each yellow, ticks (1.5 s).
pub const ALL_RED_TICKS: u32 = 3 * SIM_HZ / 2;
/// One phase: green, yellow, all red (30 s).
pub const PHASE_TICKS: u32 = GREEN_TICKS + YELLOW_TICKS + ALL_RED_TICKS;

/// The full cycle of a junction with `phases` phases, ticks.
pub const fn cycle_ticks(phases: u8) -> u32 {
    phases as u32 * PHASE_TICKS
}

/// Hash key of signal offsets.
const KEY_SIGNAL: u64 = 0x5167_4e41_4c00_0001;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Light {
    Green = 0,
    Yellow = 1,
    Red = 2,
}

/// The sim tick every signal reads; step it once per 120 Hz sim step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SignalClock {
    pub tick: u64,
}

impl SignalClock {
    pub fn step(&mut self) {
        self.tick += 1;
    }
}

/// Where in its cycle the junction at `node` of a place with `seed`, with `phases` phases,
/// starts, ticks.
pub fn offset(seed: u64, node: u32, phases: u8) -> u32 {
    let cycle = u64::from(cycle_ticks(phases.max(1)));
    (hash64(seed, &[KEY_SIGNAL, u64::from(node)]) % cycle) as u32
}

/// The light shown to `phase` (below `phases`) of a junction with cycle `offset` at `tick`.
pub fn light(offset: u32, phase: u8, phases: u8, tick: u64) -> Light {
    let t = phase_time(offset, phase, phases, tick);
    if t < u64::from(GREEN_TICKS) {
        Light::Green
    } else if t < u64::from(GREEN_TICKS + YELLOW_TICKS) {
        Light::Yellow
    } else {
        Light::Red
    }
}

/// Ticks until the light shown to `phase` turns red (0 while red).
pub fn until_red(offset: u32, phase: u8, phases: u8, tick: u64) -> u32 {
    let t = phase_time(offset, phase, phases, tick);
    u64::from(GREEN_TICKS + YELLOW_TICKS).saturating_sub(t) as u32
}

/// Ticks since `phase`'s green began, within its cycle.
fn phase_time(offset: u32, phase: u8, phases: u8, tick: u64) -> u64 {
    let cycle = u64::from(cycle_ticks(phases.max(1)));
    let shift = u64::from(phase % phases.max(1)) * u64::from(PHASE_TICKS);
    ((tick % cycle) + u64::from(offset) + cycle - shift) % cycle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_phase_goes_at_a_time_and_the_cycle_repeats() {
        for phases in 1..=4u8 {
            let off = offset(1, 7, phases);
            let cycle = cycle_ticks(phases);
            let mut greens = [0u32; 4];
            let mut clock = SignalClock::default();
            for _ in 0..cycle {
                let mut going = 0;
                for p in 0..phases {
                    let l = light(off, p, phases, clock.tick);
                    going += u32::from(l != Light::Red);
                    greens[p as usize] += u32::from(l == Light::Green);
                    assert_eq!(l, light(off, p, phases, clock.tick + u64::from(cycle)));
                    let left = until_red(off, p, phases, clock.tick);
                    assert_eq!(left == 0, l == Light::Red);
                    assert_eq!(
                        light(off, p, phases, clock.tick + u64::from(left)),
                        Light::Red
                    );
                }
                assert!(going <= 1, "{phases} phases, tick {}", clock.tick);
                clock.step();
            }
            assert!(greens[..phases as usize].iter().all(|&g| g == GREEN_TICKS));
        }
        assert_eq!(cycle_ticks(2), 60 * SIM_HZ);
    }

    #[test]
    fn offsets_follow_seed_and_node() {
        assert_eq!(offset(1, 3, 2), offset(1, 3, 2));
        assert_ne!(offset(1, 3, 2), offset(2, 3, 2));
        assert_ne!(offset(1, 3, 2), offset(1, 4, 2));
        assert!(offset(9, 9, 3) < cycle_ticks(3));
    }
}
