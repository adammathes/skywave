//! The world answers the player (`docs/NPCS.md`, "Replies"): small, friendly replies that
//! show the place noticed you, never annoyed or scared.
//!
//! - **Horn:** a short honk near people: some of those within [`HONK_REACH_M`] look over,
//!   some look over and wave ([`crate::figure::Skeleton::greet`]), the rest carry on.
//! - **Brights:** flashing the high beams at an oncoming car with its lights on: some flash
//!   back, twice.
//! - **Throttle:** a blip of the throttle at a standstill beside a stopped car: some drivers
//!   rev back.
//!
//! Who answers is seeded by the person's or the car's id ([`Replies::new`]'s seed), so the
//! same person always answers the same way: honking again does not re-roll a stranger into
//! a waver. It is rate-limited twice over: a press within [`HONK_REST_S`] (and the like) of
//! the last one that counted does nothing, and whoever answered does not answer again
//! within [`ANSWER_REST_S`]. Nothing here feeds back into the sim: replies are a pose, two
//! lamps and an engine note the game lays over people and cars as they are, so nobody stops,
//! steps, flinches or runs, and the traffic and crowd hashes do not move.
//!
//! Fixed slots, no allocation: [`GREETINGS`] people and [`CAR_REPLIES`] cars at a time.

use skywave_core::{hash64, SIM_HZ};

use crate::cars::{Car, Player};
use crate::math::{dot, sin_cos, sqrt};
use crate::peds::{Action, Pedestrian};

/// People this near the player's car hear a honk, m.
pub const HONK_REACH_M: f32 = 25.0;
/// A press of the horn this soon after the last one that counted is not heard anew, s.
pub const HONK_REST_S: f32 = 3.0;
/// Shares of people who wave and who only look over (the rest carry on).
pub const WAVE_SHARE: f32 = 0.4;
pub const LOOK_SHARE: f32 = 0.3;
/// How long a wave and a look last, s, easing in over [`EASE_IN_S`] and out over
/// [`EASE_OUT_S`].
pub const WAVE_S: f32 = 2.6;
pub const LOOK_S: f32 = 1.8;
const EASE_IN_S: f32 = 0.35;
const EASE_OUT_S: f32 = 0.5;

/// The brights reach oncoming cars this far ahead, m, within this cosine of straight
/// ahead, heading within this cosine of straight at the player.
pub const FLASH_REACH_M: f32 = 120.0;
const FLASH_CONE_COS: f32 = 0.9;
const ONCOMING_COS: f32 = 0.8;
/// A flash this soon after the last that counted does nothing, s.
pub const FLASH_REST_S: f32 = 3.0;
/// Share of drivers who flash back.
pub const FLASH_SHARE: f32 = 0.45;
/// A flash back: lit from 0 to 0.2 s and from 0.35 to [`FLASH_BACK_S`].
pub const FLASH_BACK_S: f32 = 0.55;

/// The player's car is standing below this speed, m/s, and has stood this long before a
/// blip counts, s (the blip may roll it on a little).
pub const STAND_MPS: f32 = 0.5;
const STAND_S: f32 = 0.5;
/// A blip: the throttle past [`BLIP_ON`] and back under [`BLIP_OFF`] within [`BLIP_S`].
const BLIP_ON: f32 = 0.5;
const BLIP_OFF: f32 = 0.25;
const BLIP_S: f32 = 0.6;
/// A blip this soon after the last that counted does nothing, s.
pub const BLIP_REST_S: f32 = 2.5;
/// A car beside: stopped, at most this far ahead or behind, m, this far to the side, m,
/// pointing the same way within this cosine.
const BESIDE_ALONG_M: f32 = 4.5;
const BESIDE_SIDE_M: (f32, f32) = (1.6, 6.0);
const BESIDE_COS: f32 = 0.85;
/// Share of drivers who rev back.
pub const REV_SHARE: f32 = 0.5;
/// A rev back, s.
pub const REV_S: f32 = 1.0;

/// Whoever answered does not answer again for this long, s.
pub const ANSWER_REST_S: f32 = 20.0;
/// Each answer starts this long after the press, s: the low end plus up to the span.
const DELAY_S: (f32, f32) = (0.25, 0.6);

/// People and cars answering (or resting after it) at a time.
pub const GREETINGS: usize = 16;
pub const CAR_REPLIES: usize = 4;

const KEY_GREET: u64 = 0x6EE7;
const KEY_FLASH: u64 = 0xF1A5;
const KEY_REV: u64 = 0x2E7;

/// How a person answers the horn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Greet {
    Look,
    Wave,
}

/// One person's answer: who, how, and the step it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Greeting {
    pub id: u32,
    pub greet: Greet,
    pub start: u64,
}

/// One car's answer: which car and the step it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarReply {
    pub id: u32,
    pub start: u64,
}

/// What the player did this step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplyInput {
    /// The horn held.
    pub horn: bool,
    /// The brights flashed (held).
    pub flash: bool,
    /// The throttle, 0–1.
    pub throttle: f32,
    /// The player's car, its speed signed.
    pub player: Player,
}

/// The replies under way (see the module docs).
#[derive(Clone, Debug)]
pub struct Replies {
    seed: u64,
    tick: u64,
    horn: bool,
    flash: bool,
    last_honk: Option<u64>,
    last_flash: Option<u64>,
    last_blip: Option<u64>,
    /// Steps the car has been standing.
    stood: u32,
    /// The step the throttle went past [`BLIP_ON`] from a stand, while it stays there.
    blip: Option<u64>,
    throttle_up: bool,
    /// Where the player's car was at the last step (people look at it).
    pub player: [f32; 3],
    greetings: [Option<Greeting>; GREETINGS],
    flashes: [Option<CarReply>; CAR_REPLIES],
    revs: [Option<CarReply>; CAR_REPLIES],
}

fn ticks(s: f32) -> u64 {
    (s * SIM_HZ as f32) as u64
}

/// A uniform 0–1 from a hash.
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 * (1.0 / 16_777_216.0)
}

/// Whether `slot` still holds an answer (playing or resting) at `tick`.
fn busy(start: u64, tick: u64) -> bool {
    tick < start + ticks(ANSWER_REST_S)
}

/// Seconds into an answer that started at `start`, at `alpha` of the way through `tick`'s
/// step (negative before it starts).
fn since(start: u64, tick: u64, alpha: f32) -> f32 {
    (tick as f32 - start as f32 + alpha) / SIM_HZ as f32
}

/// Ease in and out over an answer `len` s long, `s` s in: 0–1.
pub fn envelope(s: f32, len: f32) -> f32 {
    if s <= 0.0 || s >= len {
        return 0.0;
    }
    let e = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    e(s / EASE_IN_S).min(e((len - s) / EASE_OUT_S))
}

impl Replies {
    /// Nobody answering yet; who answers is seeded by `seed`.
    pub fn new(seed: u64) -> Replies {
        Replies {
            seed,
            tick: 0,
            horn: false,
            flash: false,
            last_honk: None,
            last_flash: None,
            last_blip: None,
            stood: 0,
            blip: None,
            throttle_up: false,
            player: [0.0; 3],
            greetings: [None; GREETINGS],
            flashes: [None; CAR_REPLIES],
            revs: [None; CAR_REPLIES],
        }
    }

    /// One sim step with the player's `inp` among `cars` and `people`.
    pub fn step(&mut self, inp: &ReplyInput, cars: &[Car], people: &[Pedestrian]) {
        self.tick += 1;
        let tick = self.tick;
        let p = &inp.player;
        self.player = p.pos;
        let rested = |last: Option<u64>, rest: f32| last.is_none_or(|l| tick >= l + ticks(rest));
        let (sn, cs) = sin_cos(p.heading);
        let fwd = [-sn, -cs];
        let right = [cs, -sn];
        let here = [p.pos[0], p.pos[2]];
        let rel = |q: [f32; 3]| [q[0] - here[0], q[2] - here[1]];

        // The horn.
        if inp.horn && !self.horn && rested(self.last_honk, HONK_REST_S) {
            self.last_honk = Some(tick);
            for ped in people.iter().filter(|q| q.alive) {
                let d = rel(ped.pos);
                let still = matches!(ped.pose.action, Action::None | Action::Sit);
                if !still || dot(d, d) > HONK_REACH_M * HONK_REACH_M {
                    continue;
                }
                if self
                    .greetings
                    .iter()
                    .flatten()
                    .any(|g| g.id == ped.id && busy(g.start, tick))
                {
                    continue;
                }
                let h = hash64(self.seed, &[KEY_GREET, u64::from(ped.id)]);
                let roll = unit(h);
                let greet = if roll < WAVE_SHARE {
                    Greet::Wave
                } else if roll < WAVE_SHARE + LOOK_SHARE {
                    Greet::Look
                } else {
                    continue;
                };
                let Some(slot) = self.free_greeting() else {
                    break;
                };
                *slot = Some(Greeting {
                    id: ped.id,
                    greet,
                    start: tick + delay(h),
                });
            }
        }
        self.horn = inp.horn;

        // The brights.
        if inp.flash && !self.flash && rested(self.last_flash, FLASH_REST_S) {
            self.last_flash = Some(tick);
            let mut best: Option<(f32, &Car)> = None;
            for c in cars {
                let d = rel(c.pos);
                let d2 = dot(d, d);
                if !c.lights || !(1.0..=FLASH_REACH_M * FLASH_REACH_M).contains(&d2) {
                    continue;
                }
                let n = sqrt(d2);
                let ahead = dot(d, fwd) / n > FLASH_CONE_COS;
                let oncoming = dot(c.dir, fwd) < -ONCOMING_COS;
                if ahead && oncoming && best.is_none_or(|(b, _)| d2 < b) {
                    best = Some((d2, c));
                }
            }
            if let Some((_, c)) = best {
                let h = hash64(self.seed, &[KEY_FLASH, u64::from(c.id)]);
                if unit(h) < FLASH_SHARE {
                    answer(&mut self.flashes, c.id, tick, tick + delay(h));
                }
            }
        }
        self.flash = inp.flash;

        // The throttle.
        // A blip from a stand counts even if it rolls the car on a little (an automatic
        // creeps): what matters is that the car stood before it.
        if p.speed.abs() < STAND_MPS {
            self.stood = self.stood.saturating_add(1);
        } else {
            self.stood = 0;
        }
        if !self.throttle_up && inp.throttle > BLIP_ON {
            self.throttle_up = true;
            self.blip = (u64::from(self.stood) >= ticks(STAND_S)).then_some(tick);
        } else if self.throttle_up && inp.throttle < BLIP_OFF {
            self.throttle_up = false;
            let quick = self.blip.is_some_and(|b| tick <= b + ticks(BLIP_S));
            self.blip = None;
            if quick && rested(self.last_blip, BLIP_REST_S) {
                self.last_blip = Some(tick);
                let mut best: Option<(f32, &Car)> = None;
                for c in cars {
                    let d = rel(c.pos);
                    let (along, side) = (dot(d, fwd), dot(d, right).abs());
                    let beside = along.abs() < BESIDE_ALONG_M
                        && side > BESIDE_SIDE_M.0
                        && side < BESIDE_SIDE_M.1;
                    let d2 = dot(d, d);
                    if c.speed < STAND_MPS
                        && beside
                        && dot(c.dir, fwd) > BESIDE_COS
                        && best.is_none_or(|(b, _)| d2 < b)
                    {
                        best = Some((d2, c));
                    }
                }
                if let Some((_, c)) = best {
                    let h = hash64(self.seed, &[KEY_REV, u64::from(c.id)]);
                    if unit(h) < REV_SHARE {
                        answer(&mut self.revs, c.id, tick, tick + delay(h));
                    }
                }
            }
        }
    }

    fn free_greeting(&mut self) -> Option<&mut Option<Greeting>> {
        let tick = self.tick;
        self.greetings
            .iter_mut()
            .find(|g| g.is_none_or(|g| !busy(g.start, tick)))
    }

    /// Person `id`'s answer, if one is playing: how, and how much (0–1) and how long in, s,
    /// at `alpha` of the way through the step.
    pub fn greeting(&self, id: u32, alpha: f32) -> Option<(Greet, f32, f32)> {
        let tick = self.tick;
        let g = (self.greetings.iter().flatten()).find(|g| g.id == id && busy(g.start, tick))?;
        let s = since(g.start, self.tick, alpha);
        let len = match g.greet {
            Greet::Wave => WAVE_S,
            Greet::Look => LOOK_S,
        };
        let w = envelope(s, len);
        (w > 0.0).then_some((g.greet, w, s))
    }

    /// Whether car `id` has its brights lit, flashing back, at `alpha` of the way through
    /// the step.
    pub fn flashing(&self, id: u32, alpha: f32) -> bool {
        self.flashes.iter().flatten().any(|f| {
            let s = since(f.start, self.tick, alpha);
            f.id == id && ((0.0..0.2).contains(&s) || (0.35..FLASH_BACK_S).contains(&s))
        })
    }

    /// How hard car `id` is revving back, 0–1.
    pub fn rev(&self, id: u32, alpha: f32) -> f32 {
        self.revs
            .iter()
            .flatten()
            .find(|r| r.id == id && busy(r.start, self.tick))
            .map_or(0.0, |r| {
                let s = since(r.start, self.tick, alpha);
                if s <= 0.0 || s >= REV_S {
                    return 0.0;
                }
                // Up quick, down slower: a blip, not a hold.
                let up = (s / 0.12).min(1.0);
                let down = 1.0 - (s - 0.12).max(0.0) / (REV_S - 0.12);
                up * down * down
            })
    }
}

/// The answer's delay after the press, in steps, from its hash.
fn delay(h: u64) -> u64 {
    ticks(DELAY_S.0 + DELAY_S.1 * unit(h.rotate_left(24)))
}

/// Car `id` answers from step `start` unless it answered within [`ANSWER_REST_S`] of
/// `tick`, in a slot not busy.
fn answer(slots: &mut [Option<CarReply>; CAR_REPLIES], id: u32, tick: u64, start: u64) {
    if slots
        .iter()
        .flatten()
        .any(|r| r.id == id && busy(r.start, tick))
    {
        return;
    }
    if let Some(slot) = slots
        .iter_mut()
        .find(|r| r.is_none_or(|r| !busy(r.start, tick)))
    {
        *slot = Some(CarReply { id, start });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cars::StopState;
    use crate::lanes::NONE;
    use alloc::vec::Vec;

    fn car(id: u32, pos: [f32; 2], dir: [f32; 2], speed: f32, lights: bool) -> Car {
        Car {
            id,
            pos: [pos[0], 0.0, pos[1]],
            heading: 0.0,
            dir,
            speed,
            shape: 0,
            paint: 0,
            len: 4.6,
            half_w: 0.9,
            lights,
            braking: false,
            seg: 0,
            prev: NONE,
            s: 0.0,
            next_turn: NONE,
            held: NONE,
            clearing: NONE,
            clear_left: 0.0,
            v0: 1.0,
            headway: 1.3,
            offset: [0.0; 2],
            ghost: NONE,
            shove: [0.0; 2],
            yaw: 0.0,
            slide: [0.0; 2],
            spin: 0.0,
            hold: 0,
            room: [0.0; 3],
            stop: StopState::Approach,
            wait: 0,
            stop_tick: 0,
            hidden: 0,
            stuck: 0,
            light_delay_s: 0.0,
        }
    }

    /// The player at the origin facing -z, standing.
    fn input() -> ReplyInput {
        ReplyInput {
            horn: false,
            flash: false,
            throttle: 0.0,
            player: Player {
                speed: 0.0,
                pos: [0.0; 3],
                ..Player::ABSENT
            },
        }
    }

    fn run(r: &mut Replies, inp: &ReplyInput, cars: &[Car], people: &[Pedestrian], s: f32) {
        for _ in 0..ticks(s) {
            r.step(inp, cars, people);
        }
    }

    fn crowd(n: u32, reach: f32) -> Vec<Pedestrian> {
        (0..n)
            .map(|i| {
                let a = i as f32 * 0.7;
                let (s, c) = sin_cos(a);
                Pedestrian::standing(i, [reach * s, 0.0, reach * c])
            })
            .collect()
    }

    /// Who is answering the horn, each person's kind or none.
    fn answering(r: &Replies, people: &[Pedestrian]) -> Vec<Option<Greet>> {
        people
            .iter()
            .map(|p| r.greeting(p.id, 0.0).map(|g| g.0))
            .collect()
    }

    fn honk(r: &mut Replies, people: &[Pedestrian]) {
        let horn = ReplyInput {
            horn: true,
            ..input()
        };
        run(r, &horn, &[], people, 0.15);
        run(r, &input(), &[], people, 1.0);
    }

    #[test]
    fn a_honk_gets_some_waves_some_looks_and_some_carry_on() {
        let (mut waves, mut looks, mut none) = (0, 0, 0);
        for seed in 0..40 {
            let people = crowd(12, 10.0);
            let mut r = Replies::new(seed);
            honk(&mut r, &people);
            for a in answering(&r, &people) {
                match a {
                    Some(Greet::Wave) => waves += 1,
                    Some(Greet::Look) => looks += 1,
                    None => none += 1,
                }
            }
        }
        // 480 people: about 40 / 30 / 30 percent (the slots hold 16 of each 12).
        assert!((160..230).contains(&waves), "{waves} {looks} {none}");
        assert!((110..180).contains(&looks), "{waves} {looks} {none}");
        assert!((110..180).contains(&none), "{waves} {looks} {none}");
    }

    #[test]
    fn nobody_beyond_reach_or_dodging_answers() {
        let mut people = crowd(30, HONK_REACH_M + 1.0);
        let mut near = crowd(30, 8.0);
        for p in &mut near {
            p.id += 100;
            p.pose.action = Action::Aside;
        }
        people.extend(near);
        let mut r = Replies::new(3);
        honk(&mut r, &people);
        assert!(answering(&r, &people).iter().all(Option::is_none));
    }

    #[test]
    fn spamming_the_horn_does_nothing_new() {
        let people = crowd(12, 10.0);
        let mut r = Replies::new(7);
        honk(&mut r, &people);
        let first = answering(&r, &people);
        assert!(first.iter().any(Option::is_some));
        // Honking every 1.15 s for over two minutes: whoever answered still answers once,
        // whoever carried on still carries on.
        let mut seen = alloc::vec![0u32; people.len()];
        let mut was = answering(&r, &people);
        for _ in 0..120 {
            honk(&mut r, &people);
            let now = answering(&r, &people);
            for (k, (a, b)) in was.iter().zip(&now).enumerate() {
                if a.is_none() && b.is_some() {
                    seen[k] += 1;
                }
                assert!(
                    b.is_none() || *b == first[k],
                    "{k}: {b:?} vs {:?}",
                    first[k]
                );
            }
            was = now;
        }
        // An answer at most every ANSWER_REST_S.
        let most = 120.0 * 1.15 / ANSWER_REST_S + 1.0;
        assert!(seen.iter().all(|&n| n as f32 <= most), "{seen:?}");
        let mut ignored = first.iter().zip(&seen).filter(|(f, _)| f.is_none());
        assert!(ignored.clone().count() > 0 && ignored.all(|(_, &n)| n == 0));
    }

    #[test]
    fn holding_the_horn_is_one_honk() {
        let people = crowd(12, 10.0);
        let mut r = Replies::new(7);
        let horn = ReplyInput {
            horn: true,
            ..input()
        };
        run(&mut r, &horn, &[], &people, 60.0);
        // One honk at the start, long over.
        assert!(answering(&r, &people).iter().all(Option::is_none));
    }

    #[test]
    fn some_oncoming_cars_flash_back_and_only_oncoming_lit_ones() {
        let flash = ReplyInput {
            flash: true,
            ..input()
        };
        let mut back = 0;
        for id in 0..200 {
            let cars = [car(id, [-3.5, -60.0], [0.0, 1.0], 12.0, true)];
            let mut r = Replies::new(11);
            run(&mut r, &flash, &cars, &[], 0.1);
            let mut lit = 0;
            for _ in 0..ticks(2.0) {
                r.step(&input(), &cars, &[]);
                lit += u32::from(r.flashing(id, 0.0));
            }
            if lit > 0 {
                back += 1;
                // Two blinks, 0.2 s and 0.2 s.
                assert!(
                    (ticks(0.38) as u32..ticks(0.42) as u32).contains(&lit),
                    "{lit}"
                );
            }
        }
        assert!((70..110).contains(&back), "{back} of 200 flashed back");
        for cars in [
            [car(1, [-3.5, -60.0], [0.0, 1.0], 12.0, false)],
            [car(1, [0.0, -60.0], [0.0, -1.0], 12.0, true)],
            [car(1, [3.5, 60.0], [0.0, 1.0], 12.0, true)],
            [car(1, [-60.0, -60.0], [1.0, 0.0], 12.0, true)],
        ] {
            for seed in 0..20 {
                let mut r = Replies::new(seed);
                run(&mut r, &flash, &cars, &[], 0.1);
                run(&mut r, &input(), &cars, &[], 0.2);
                assert!(!r.flashes.iter().any(Option::is_some), "{:?}", cars[0].pos);
            }
        }
    }

    /// Blips of the throttle beside a stopped car `id`: whether it revved back.
    fn blip(r: &mut Replies, cars: &[Car], hold_s: f32) -> bool {
        let id = cars[0].id;
        run(r, &input(), cars, &[], 1.0);
        let gas = ReplyInput {
            throttle: 1.0,
            ..input()
        };
        run(r, &gas, cars, &[], hold_s);
        let mut most = 0.0f32;
        for _ in 0..ticks(2.0) {
            r.step(&input(), cars, &[]);
            most = most.max(r.rev(id, 0.0));
        }
        most > 0.5
    }

    #[test]
    fn some_drivers_beside_rev_back_to_a_blip() {
        let mut back = 0;
        for id in 0..200 {
            let cars = [car(id, [3.2, 1.0], [0.0, -1.0], 0.0, false)];
            let mut r = Replies::new(5);
            back += u32::from(blip(&mut r, &cars, 0.25));
        }
        assert!((80..120).contains(&back), "{back} of 200 revved back");
        // A driver who revs back does: but not to a long press, a moving car, one behind
        // or across the road.
        let id = (0..200)
            .find(|&id| {
                blip(
                    &mut Replies::new(5),
                    &[car(id, [3.2, 1.0], [0.0, -1.0], 0.0, false)],
                    0.25,
                )
            })
            .unwrap();
        let mut r = Replies::new(5);
        assert!(!blip(
            &mut r,
            &[car(id, [3.2, 1.0], [0.0, -1.0], 0.0, false)],
            1.0
        ));
        for c in [
            car(id, [3.2, 1.0], [0.0, -1.0], 3.0, false),
            car(id, [0.0, 7.0], [0.0, -1.0], 0.0, false),
            car(id, [-3.2, 1.0], [0.0, 1.0], 0.0, false),
        ] {
            assert!(!blip(&mut Replies::new(5), &[c], 0.25), "{:?}", c.pos);
        }
        // Blipping again and again: one rev each ANSWER_REST_S.
        let cars = [car(id, [3.2, 1.0], [0.0, -1.0], 0.0, false)];
        let mut r = Replies::new(5);
        let revs: u32 = (0..20).map(|_| u32::from(blip(&mut r, &cars, 0.25))).sum();
        // 20 blips, 3.25 s each: 65 s, so three or four.
        assert!((3..=4).contains(&revs), "{revs}");
    }

    #[test]
    fn answers_ease_in_and_out() {
        assert_eq!(envelope(0.0, WAVE_S), 0.0);
        assert_eq!(envelope(WAVE_S, WAVE_S), 0.0);
        assert_eq!(envelope(1.2, WAVE_S), 1.0);
        let mut last = 0.0;
        for i in 0..=35 {
            let e = envelope(i as f32 * 0.01, WAVE_S);
            assert!(e >= last);
            last = e;
        }
    }
}
