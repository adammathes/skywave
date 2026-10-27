//! Fixed-cycle traffic signals.
//!
//! Every signalized junction runs the same two-phase cycle, counted in 120 Hz sim ticks:
//! phase 0 green, yellow, all red, then phase 1 green, yellow, all red. A junction's cycle
//! starts at its own offset, a hash of the place seed and its node, so neighboring lights
//! do not change in lockstep. The light a lane faces is a pure function of the tick, so the
//! whole state of every signal in the place is one [`SignalClock`].

use skywave_core::{hash64, SIM_HZ};

/// Green time per phase, ticks (25 s).
pub const GREEN_TICKS: u32 = 25 * SIM_HZ;
/// Yellow time per phase, ticks (3.5 s).
pub const YELLOW_TICKS: u32 = 7 * SIM_HZ / 2;
/// All-red clearance after each yellow, ticks (1.5 s).
pub const ALL_RED_TICKS: u32 = 3 * SIM_HZ / 2;
/// One phase: green, yellow, all red.
pub const PHASE_TICKS: u32 = GREEN_TICKS + YELLOW_TICKS + ALL_RED_TICKS;
/// The full cycle of both phases (60 s).
pub const CYCLE_TICKS: u32 = 2 * PHASE_TICKS;

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

/// Where in its cycle the junction at `node` of a place with `seed` starts, ticks.
pub fn offset(seed: u64, node: u32) -> u32 {
    (hash64(seed, &[KEY_SIGNAL, u64::from(node)]) % u64::from(CYCLE_TICKS)) as u32
}

/// The light shown to `phase` (0 or 1) of a junction with cycle `offset` at `tick`.
pub fn light(offset: u32, phase: u8, tick: u64) -> Light {
    let cycle = u64::from(CYCLE_TICKS);
    let shift = u64::from(phase & 1) * u64::from(PHASE_TICKS);
    let t = ((tick % cycle) + u64::from(offset) + cycle - shift) % cycle;
    if t < u64::from(GREEN_TICKS) {
        Light::Green
    } else if t < u64::from(GREEN_TICKS + YELLOW_TICKS) {
        Light::Yellow
    } else {
        Light::Red
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_never_both_go_and_the_cycle_repeats() {
        let off = offset(1, 7);
        let mut greens = [0u32; 2];
        let mut clock = SignalClock::default();
        for _ in 0..CYCLE_TICKS {
            let (a, b) = (light(off, 0, clock.tick), light(off, 1, clock.tick));
            assert!(a == Light::Red || b == Light::Red, "tick {}", clock.tick);
            greens[0] += u32::from(a == Light::Green);
            greens[1] += u32::from(b == Light::Green);
            assert_eq!(a, light(off, 0, clock.tick + u64::from(CYCLE_TICKS)));
            clock.step();
        }
        assert_eq!(greens, [GREEN_TICKS; 2]);
        assert_eq!(CYCLE_TICKS, 60 * SIM_HZ);
    }

    #[test]
    fn offsets_follow_seed_and_node() {
        assert_eq!(offset(1, 3), offset(1, 3));
        assert_ne!(offset(1, 3), offset(2, 3));
        assert_ne!(offset(1, 3), offset(1, 4));
        assert!(offset(9, 9) < CYCLE_TICKS);
    }
}
