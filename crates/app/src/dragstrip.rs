//! The drag strip's start tree and timing board (`docs/WORLD_GENERATION.md`, "Set pieces"):
//! a place, not a race. Stop square on the start line and hold still a second, and the tree
//! counts down (the staging lamps, three ambers half a second apart, the green); the clock
//! runs from the moment the car's nose leaves the line (a red lamp if it leaves before the
//! green, and the run is still timed) to the finish, and the board shows the elapsed time
//! and the speed over the line, with a toast. No target, no ranking, nothing kept but the
//! last run; leaving the strip, turning back or idling on it ends a run quietly.
//!
//! The place carries the strip as props (`place::PropKind::StartTree` beside the start
//! line, `StripLine`s at the start and the finish, the `TimingBoard`); [`DragStrip::of`]
//! reads them back. The lit lamps and the digits are instances over those props
//! ([`world::setpieces::tree_lamps`], [`world::setpieces::digit`]).

use glam::{Mat4, Quat, Vec2, Vec3};
use place::{Place, PropKind};
use render::{Mesh, MeshId, MeshInstance};
use world::setpieces::{BOARD_SLOTS, TREE_LAMPS};
use world::{MeshKind, WorldMesh};

use crate::worldmesh::atlas_mesh;
use crate::{convert, lighttiles, Part};

/// How long the car must hold still on the line before the tree counts down, s.
pub const STAGE_HOLD_S: f32 = 1.0;
/// Each amber burns this long before the next; the green follows the third, s.
pub const AMBER_S: f32 = 0.5;
pub const GREEN_AT_S: f32 = 3.0 * AMBER_S;
/// A staged car that has not left this long after the green is unstaged, s.
const IDLE_AFTER_GREEN_S: f32 = 10.0;
/// A run longer than this ends quietly, s.
const MAX_RUN_S: f32 = 60.0;
/// The nose counts as on the line from this far behind it to this far past it, m.
const STAGE_BACK_M: f32 = 1.2;
const STAGE_PAST_M: f32 = 0.3;
/// A staged car points down the strip within this cosine.
const SQUARE_COS: f32 = 0.94;
/// Below this the car is holding still, m/s.
const STILL_MPS: f32 = 0.4;
/// The tree stands on the strip's middle; a car this close to it laterally is not in a
/// lane, m.
const TREE_CLEAR_M: f32 = 1.2;
/// The tree's lamps and the board's digits are drawn within this of the eye, m.
pub const DRAW_RADIUS_M: f32 = 600.0;
/// Seconds a finished run's toast stays up.
pub const TOAST_S: f32 = 4.0;
/// Most instances a frame adds: the tree's lamps and the board's digits.
pub const MAX_INSTANCES: usize = TREE_LAMPS.len() + BOARD_SLOTS;

/// Miles an hour in a meter a second.
const MPH_PER_MPS: f32 = 2.236_936;

/// Lamp indices into [`TREE_LAMPS`].
const PRESTAGE: usize = 0;
const STAGE: usize = 1;
const GREEN: usize = 5;
const RED: usize = 6;

/// Where a run stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Nobody on the line.
    Idle,
    /// On the line and still, for `held` seconds.
    Staged { held: f32 },
    /// The tree counting down, `t` seconds in (green at [`GREEN_AT_S`]).
    Countdown { t: f32 },
    /// Off the line: `t` seconds since the tree started, `et` since the nose left,
    /// `jumped` when it left before the green.
    Run { t: f32, et: f32, jumped: bool },
}

/// A finished run: elapsed time from the line, s, and the speed over the finish, m/s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunTime {
    pub et_s: f32,
    pub speed_mps: f32,
    pub jumped: bool,
}

/// The car as the strip sees it each step.
#[derive(Clone, Copy, Debug)]
pub struct Car {
    /// Ground point of its middle and its nose's direction, `(x, z)`.
    pub pos: Vec2,
    pub forward: Vec2,
    /// Half its length, m.
    pub half_len: f32,
    /// Signed speed, m/s.
    pub speed: f32,
}

/// The strip, its run and its drawn instances.
pub struct DragStrip {
    start: Vec2,
    dir: Vec2,
    run_m: f32,
    half_w: f32,
    tree: Mat4,
    board: Mat4,
    pub phase: Phase,
    pub last: Option<RunTime>,
    /// A run finished: the toast's seconds left, and what it says.
    toast: f32,
    toast_text: String,
    lamp_mesh: [u16; TREE_LAMPS.len()],
    digit_mesh: [u16; 10],
    ids: Vec<MeshId>,
}

fn xz(p: [f32; 3]) -> Vec2 {
    Vec2::new(p[0], p[2])
}

/// A Place prop's facing `(x, z)` (`world::props::dir_of_yaw`).
fn facing(yaw: f32) -> Vec2 {
    let (s, c) = yaw.sin_cos();
    Vec2::new(-s, -c)
}

impl DragStrip {
    /// The place's drag strip, from its start tree, the painted line farthest down the
    /// strip from it and its timing board; `None` without one.
    pub fn of(place: &Place) -> Option<DragStrip> {
        let items = &place.props.items;
        let tree = items.iter().find(|p| p.kind == PropKind::StartTree)?;
        let board = items.iter().find(|p| p.kind == PropKind::TimingBoard)?;
        let dir = facing(tree.yaw);
        let at = xz(tree.pos);
        // The start line is the painted line just behind the tree; the finish the farthest
        // one down the strip.
        let lines: Vec<(f32, Vec2, u16)> = items
            .iter()
            .filter_map(|p| match p.kind {
                PropKind::StripLine { len_dm } => {
                    let q = xz(p.pos);
                    let along = (q - at).dot(dir);
                    ((q - at).perp_dot(dir).abs() < 2.0).then_some((along, q, len_dm))
                }
                _ => None,
            })
            .collect();
        let (_, start, len_dm) = lines
            .iter()
            .copied()
            .filter(|l| l.0 <= 0.0)
            .max_by(|a, b| a.0.total_cmp(&b.0))?;
        let finish = lines.iter().map(|l| l.0).fold(f32::MIN, f32::max);
        let run_m = finish - (start - at).dot(dir);
        if run_m < 50.0 {
            return None;
        }
        let place_at = |p: &place::Prop| {
            Mat4::from_rotation_translation(Quat::from_rotation_y(p.yaw), Vec3::from(p.pos))
        };
        Some(DragStrip {
            start,
            dir,
            run_m,
            half_w: 0.05 * f32::from(len_dm),
            tree: place_at(tree),
            board: place_at(board),
            phase: Phase::Idle,
            last: None,
            toast: 0.0,
            toast_text: String::new(),
            lamp_mesh: [0; TREE_LAMPS.len()],
            digit_mesh: [0; 10],
            ids: Vec::new(),
        })
    }

    /// The meshes its instances draw, in order; [`DragStrip::set_ids`] takes their ids.
    pub fn meshes(&mut self) -> Vec<Mesh> {
        let mut out = Vec::new();
        let mut add = |m: WorldMesh| {
            let m = atlas_mesh(&m, MeshKind::Prop);
            let mut mesh = convert(&m, Part::Base);
            lighttiles::append(&mut mesh, &convert(&m, Part::Lamps));
            out.push(mesh);
            (out.len() - 1) as u16
        };
        for k in 0..TREE_LAMPS.len() {
            let mut m = WorldMesh::default();
            world::setpieces::tree_lamps(&mut m, k);
            self.lamp_mesh[k] = add(m);
        }
        for d in 0..10 {
            let mut m = WorldMesh::default();
            world::setpieces::digit(&mut m, d);
            self.digit_mesh[d] = add(m);
        }
        out
    }

    pub fn set_ids(&mut self, ids: Vec<MeshId>) {
        self.ids = ids;
    }

    /// Where `car`'s nose is: along the strip from the start line, and across it.
    fn nose(&self, car: &Car) -> (f32, f32) {
        let nose = car.pos + car.forward * car.half_len;
        let d = nose - self.start;
        (d.dot(self.dir), self.dir.perp_dot(d))
    }

    /// One simulation step of `dt` seconds with the player's car.
    pub fn step(&mut self, car: &Car, dt: f32) {
        self.toast = (self.toast - dt).max(0.0);
        let (along, across) = self.nose(car);
        let square = car.forward.dot(self.dir) >= SQUARE_COS;
        let in_lane = across.abs() >= TREE_CLEAR_M && across.abs() <= self.half_w;
        let on_line = (-STAGE_BACK_M..=STAGE_PAST_M).contains(&along) && square && in_lane;
        let still = car.speed.abs() < STILL_MPS;
        self.phase = match self.phase {
            Phase::Idle => {
                if on_line && still {
                    Phase::Staged { held: dt }
                } else {
                    Phase::Idle
                }
            }
            Phase::Staged { held } => {
                if !(on_line && still) {
                    Phase::Idle
                } else if held + dt >= STAGE_HOLD_S {
                    Phase::Countdown { t: 0.0 }
                } else {
                    Phase::Staged { held: held + dt }
                }
            }
            Phase::Countdown { t } => {
                let t = t + dt;
                if along > STAGE_PAST_M && in_lane {
                    Phase::Run {
                        t,
                        et: 0.0,
                        jumped: t < GREEN_AT_S,
                    }
                } else if !on_line || t > GREEN_AT_S + IDLE_AFTER_GREEN_S {
                    Phase::Idle
                } else {
                    Phase::Countdown { t }
                }
            }
            Phase::Run { t, et, jumped } => {
                let (t, et) = (t + dt, et + dt);
                let off = across.abs() > self.half_w + 2.0 || car.forward.dot(self.dir) < 0.0;
                if along >= self.run_m {
                    let r = RunTime {
                        et_s: et,
                        speed_mps: car.speed.abs(),
                        jumped,
                    };
                    self.last = Some(r);
                    self.toast = TOAST_S;
                    let red = if jumped { "  (red light)" } else { "" };
                    let mph = r.speed_mps * MPH_PER_MPS;
                    self.toast_text = format!("{:.2} s  {mph:.0} mph{red}", r.et_s);
                    Phase::Idle
                } else if off || along < -STAGE_BACK_M - 2.0 || et > MAX_RUN_S {
                    Phase::Idle
                } else {
                    Phase::Run { t, et, jumped }
                }
            }
        };
    }

    /// Which of the tree's [`TREE_LAMPS`] are lit now, as a bit mask.
    pub fn lamps(&self) -> u8 {
        let bit = |k: usize| 1u8 << k;
        match self.phase {
            Phase::Idle => 0,
            Phase::Staged { .. } => bit(PRESTAGE) | bit(STAGE),
            Phase::Countdown { t } => {
                let mut m = bit(PRESTAGE) | bit(STAGE);
                if t >= GREEN_AT_S {
                    m |= bit(GREEN);
                } else {
                    m |= bit(2 + ((t / AMBER_S) as usize).min(2));
                }
                m
            }
            Phase::Run { jumped, t, .. } => {
                if jumped {
                    bit(RED)
                } else if t >= GREEN_AT_S {
                    bit(GREEN)
                } else {
                    0
                }
            }
        }
    }

    /// The board's digits, [`BOARD_SLOTS`] of them (`None`: dark): the last run's time in
    /// hundredths (to 99.99 s) and its speed in whole miles an hour.
    pub fn digits(&self) -> [Option<u8>; BOARD_SLOTS] {
        let mut out = [None; BOARD_SLOTS];
        let Some(r) = self.last else {
            return out;
        };
        let hundredths = ((r.et_s * 100.0).round() as u32).min(9999);
        for (k, slot) in out.iter_mut().take(4).enumerate() {
            let d = (hundredths / 10u32.pow(3 - k as u32) % 10) as u8;
            // No leading zero before the point's first digit.
            if !(k == 0 && d == 0) {
                *slot = Some(d);
            }
        }
        let mph = ((r.speed_mps * MPH_PER_MPS).round() as u32).min(999);
        for k in 0..3 {
            let d = (mph / 10u32.pow(2 - k as u32) % 10) as u8;
            let lead = mph < 10u32.pow(2 - k as u32) && k < 2;
            if !lead {
                out[4 + k] = Some(d);
            }
        }
        out
    }

    /// The toast a finished run shows, while it is up.
    pub fn toast(&self) -> Option<&str> {
        (self.toast > 0.0).then_some(self.toast_text.as_str())
    }

    /// Appends the lit lamps and the digits when the strip is within [`DRAW_RADIUS_M`] of
    /// `eye`.
    pub fn push(&self, eye: Vec3, out: &mut Vec<MeshInstance>) {
        if self.ids.is_empty() || self.tree.w_axis.truncate().distance(eye) > DRAW_RADIUS_M {
            return;
        }
        let lamps = self.lamps();
        for (k, &m) in self.lamp_mesh.iter().enumerate() {
            if lamps >> k & 1 == 1 {
                let id = self.ids[usize::from(m)];
                out.push(MeshInstance {
                    casts_shadow: false,
                    ..MeshInstance::new(id, self.tree.to_cols_array_2d())
                });
            }
        }
        for (k, d) in self.digits().iter().enumerate() {
            if let Some(d) = d {
                let slot = world::setpieces::board_slot(k);
                let at = self.board * Mat4::from_translation(Vec3::new(slot.x, slot.y, slot.z));
                let id = self.ids[usize::from(self.digit_mesh[usize::from(*d)])];
                out.push(MeshInstance {
                    casts_shadow: false,
                    ..MeshInstance::new(id, at.to_cols_array_2d())
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use place::Prop;

    /// A strip running east from (0, 0): the start line at x = 0, the tree a meter past it,
    /// the finish 402 m on, 30 m wide.
    fn strip() -> DragStrip {
        let east = -core::f32::consts::FRAC_PI_2;
        let mut place = Place::test_grid();
        let p = |x: f32, z: f32, kind: PropKind| Prop {
            pos: [x, 0.0, z],
            yaw: east,
            kind,
        };
        place.props.items = vec![
            p(0.0, 0.0, PropKind::StripLine { len_dm: 290 }),
            p(402.3, 0.0, PropKind::StripLine { len_dm: 290 }),
            p(1.0, 0.0, PropKind::StartTree),
            p(30.0, -19.0, PropKind::TimingBoard),
        ];
        DragStrip::of(&place).expect("a strip")
    }

    fn car(x: f32, speed: f32) -> Car {
        Car {
            pos: Vec2::new(x - 2.2, 5.0),
            forward: Vec2::new(1.0, 0.0),
            half_len: 2.2,
            speed,
        }
    }

    const DT: f32 = 1.0 / 120.0;

    /// Holds the car on the line `wait_s`, then runs it at a steady 6 m/s² until it
    /// finishes (or 30 s go by); returns the seconds it ran.
    fn launch(s: &mut DragStrip, wait_s: f32) -> f32 {
        let mut t = 0.0;
        while t < wait_s {
            s.step(&car(0.0, 0.0), DT);
            t += DT;
        }
        let (mut x, mut v) = (0.0f32, 0.0f32);
        let mut et = 0.0;
        while s.last.is_none() && et < 30.0 {
            v += 6.0 * DT;
            x += v * DT;
            et += DT;
            s.step(&car(x, v), DT);
        }
        et
    }

    #[test]
    fn a_staged_car_gets_the_tree_then_a_time() {
        let mut s = strip();
        assert!((s.run_m - 402.3).abs() < 0.01);
        s.step(&car(0.0, 0.0), DT);
        assert!(matches!(s.phase, Phase::Staged { .. }));
        assert_eq!(s.lamps(), 0b11);
        // Holding a second starts the tree; the ambers go one at a time, then the green.
        for _ in 0..121 {
            s.step(&car(0.0, 0.0), DT);
        }
        assert!(matches!(s.phase, Phase::Countdown { .. }));
        assert_eq!(s.lamps() & 0b0001_1100, 0b100);
        launch(&mut s, GREEN_AT_S);
        let r = s.last.expect("a time");
        assert!(!r.jumped);
        // A steady 6 m/s² takes the nose from 0.3 m past the line (where the clock starts)
        // to the finish 402.3 m on in 11.26 s, at 69.5 m/s (155 mph).
        assert!((r.et_s - 11.26).abs() < 0.03, "{}", r.et_s);
        assert!((r.speed_mps - 69.5).abs() < 0.3, "{}", r.speed_mps);
        assert_eq!(s.phase, Phase::Idle);
        let d = s.digits();
        let h = (r.et_s * 100.0).round() as u32;
        assert_eq!(
            &d[..4],
            &[
                Some(1),
                Some(1),
                Some((h / 10 % 10) as u8),
                Some((h % 10) as u8)
            ]
        );
        assert_eq!(&d[4..], &[Some(1), Some(5), Some(5)]);
        assert!(s.toast().unwrap().starts_with("11.2"));
        assert!(s.toast().unwrap().ends_with(" s  155 mph"));
    }

    #[test]
    fn leaving_early_lights_the_red_and_is_still_timed() {
        let mut s = strip();
        for _ in 0..125 {
            s.step(&car(0.0, 0.0), DT);
        }
        launch(&mut s, 0.0);
        let r = s.last.expect("a time");
        assert!(r.jumped);
        assert!(s.toast().unwrap().contains("red light"));
    }

    #[test]
    fn a_car_off_the_line_or_turned_never_stages_and_a_run_off_the_strip_ends() {
        let mut s = strip();
        // Rolling, crooked, or on the tree's line: no staging.
        s.step(&car(0.0, 3.0), DT);
        assert_eq!(s.phase, Phase::Idle);
        let mut crooked = car(0.0, 0.0);
        crooked.forward = Vec2::new(0.7, 0.7).normalize();
        s.step(&crooked, DT);
        assert_eq!(s.phase, Phase::Idle);
        let mut middle = car(0.0, 0.0);
        middle.pos.y = 0.0;
        s.step(&middle, DT);
        assert_eq!(s.phase, Phase::Idle);
        // A run that turns off the strip ends without a time.
        for _ in 0..125 {
            s.step(&car(0.0, 0.0), DT);
        }
        s.step(&car(1.0, 5.0), DT);
        assert!(matches!(s.phase, Phase::Run { .. }));
        let mut gone = car(60.0, 20.0);
        gone.pos.y = 40.0;
        s.step(&gone, DT);
        assert_eq!(s.phase, Phase::Idle);
        assert!(s.last.is_none());
    }

    #[test]
    fn the_basin_has_a_quarter_mile_and_the_coast_none() {
        let basin = crate::scenario::get(1);
        let place = crate::places::build_in(crate::PlaceKey::Coast, skywave_core::Seed(1), basin)
            .expect("the basin builds");
        let s = DragStrip::of(&place).expect("a strip");
        assert!((s.run_m - 402.336).abs() < 0.5, "{}", s.run_m);
        assert!(s.half_w > 10.0);
        let coast = crate::places::build_in(
            crate::PlaceKey::Coast,
            skywave_core::Seed(1),
            crate::scenario::get(0),
        )
        .expect("the coast builds");
        assert!(DragStrip::of(&coast).is_none());
    }
}
