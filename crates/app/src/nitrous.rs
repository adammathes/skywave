//! Nitrous: a donut box and a burger bag on the passenger seat ([`crate::drivethru`]) make
//! one charge, up to [`MAX_CHARGES`]. The nitrous button burns one: for [`BURN_S`] the
//! engine pulls [`BOOST`] times as hard in the forward gears
//! (`vehicle::Assists::power_boost`). A toast says what is in the tank.

/// How long a charge burns, s.
pub const BURN_S: f32 = 3.0;
/// The engine torque multiplier while it burns.
pub const BOOST: f32 = 1.8;
/// The most charges the tank holds.
pub const MAX_CHARGES: u8 = 3;
/// How long a toast shows, s.
const TOAST_S: f32 = 2.5;
/// The toast for each count of charges after one is made, and while one burns.
const READY: [&str; MAX_CHARGES as usize + 1] =
    ["", "NITROUS READY", "NITROUS READY x2", "NITROUS READY x3"];
const BURNING: &str = "NITROUS!";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Nitrous {
    pub charges: u8,
    /// Seconds left of the charge burning.
    pub burn_s: f32,
    toast_s: f32,
    toast: &'static str,
}

impl Nitrous {
    /// `n` charges made (a donut and a burger each).
    pub fn add(&mut self, n: u8) {
        if n == 0 {
            return;
        }
        self.charges = (self.charges + n).min(MAX_CHARGES);
        self.show(READY[usize::from(self.charges)]);
    }

    /// The button pressed: burns a charge unless one is burning or the tank is empty.
    pub fn fire(&mut self) {
        if self.burn_s > 0.0 || self.charges == 0 {
            return;
        }
        self.charges -= 1;
        self.burn_s = BURN_S;
        self.show(BURNING);
    }

    /// One simulation step of `dt` seconds: the engine's torque multiplier for it.
    pub fn step(&mut self, dt: f32) -> f32 {
        let boost = if self.burn_s > 0.0 { BOOST } else { 1.0 };
        self.burn_s = (self.burn_s - dt).max(0.0);
        boost
    }

    /// The toast's clock, by real time.
    pub fn frame(&mut self, dt: f32) {
        self.toast_s = (self.toast_s - dt).max(0.0);
    }

    /// The toast showing, if any.
    pub fn toast(&self) -> Option<&'static str> {
        (self.toast_s > 0.0).then_some(self.toast)
    }

    fn show(&mut self, text: &'static str) {
        self.toast = text;
        self.toast_s = TOAST_S;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = skywave_core::SIM_DT as f32;

    #[test]
    fn a_charge_burns_once_for_its_time() {
        let mut n = Nitrous::default();
        n.fire();
        assert_eq!(
            (n.charges, n.step(DT)),
            (0, 1.0),
            "an empty tank does nothing"
        );
        n.add(1);
        assert_eq!(n.toast(), Some("NITROUS READY"));
        n.fire();
        n.fire();
        assert_eq!(
            n.charges, 0,
            "a second press while it burns is not a second charge"
        );
        assert_eq!(n.toast(), Some(BURNING));
        let mut boosted = 0u32;
        for _ in 0..(2.0 * BURN_S / DT) as u32 {
            if n.step(DT) > 1.0 {
                boosted += 1;
            }
        }
        assert!(
            boosted.abs_diff((BURN_S / DT).round() as u32) <= 1,
            "{boosted}"
        );
        n.frame(TOAST_S);
        assert_eq!(n.toast(), None);
    }

    #[test]
    fn the_tank_holds_three() {
        let mut n = Nitrous::default();
        n.add(2);
        n.add(2);
        assert_eq!(n.charges, MAX_CHARGES);
        assert_eq!(n.toast(), Some("NITROUS READY x3"));
    }
}
