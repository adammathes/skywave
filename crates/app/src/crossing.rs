//! The basin's railway in the game (`docs/NPCS.md`, "The train"): the freight train and the
//! grade crossings' gates and lamps ([`traffic::train::Railway`] runs them). Each step it
//! tells the traffic which gate lines hold and the people which crossings are closed,
//! weighs everyone near the track, and answers the player's car: a train body or a lowered
//! arm is a heavy box at rest (`world::collide::hit_car`), and anything that touches the
//! train stops it, so it never pushes anyone. The bodies, arms and flashing lamps are
//! instances.

use glam::{Mat4, Quat, Vec3};
use place::Place;
use render::{Mesh, MeshId, MeshInstance};
use traffic::train::{Phase, Railway, CARS, CAR_M, HALF_W_M, LOCO_M};
use world::setpieces::{TRAIN_BOXCAR, TRAIN_HOPPER, TRAIN_LOCO, TRAIN_TANK};
use world::{MeshKind, Palette, WorldMesh};

use crate::worldmesh::atlas_mesh;
use crate::{convert, lighttiles, Part};

/// The train and the gates are drawn within this of the eye, m.
pub const DRAW_RADIUS_M: f32 = 700.0;
/// A train body or a gate is weighed against the player's car within this of it, m.
const CONTACT_REACH_M: f32 = 30.0;
/// What a train body and a gate arm weigh against the player's car: immovable.
const HEAVY_KG: f32 = 1.0e7;
/// An arm holds the player's car out once it is this far down (1 up, 0 down).
const ARM_BLOCKS_BELOW: f32 = 0.5;
/// How far beyond the train's side a reset keeps a car, m.
const SWEEP_CLEAR_M: f32 = 1.5;
/// The gate lamps alternate this many times a second.
const FLASH_HZ: f32 = 1.0;
/// How far the bells carry, m.
pub const BELL_REACH_M: f32 = 160.0;

/// The kind of train body `k` (0 the locomotive).
fn body_kind(k: usize) -> u8 {
    match k {
        0 => TRAIN_LOCO,
        _ => [TRAIN_BOXCAR, TRAIN_TANK, TRAIN_HOPPER, TRAIN_BOXCAR][(k * 7 + 3) % 4],
    }
}

/// The place's railway as the game runs and draws it.
pub struct Rail {
    pub railway: Railway,
    /// Train mesh per body kind, arm mesh and lit lamp meshes per gate, registered ids.
    body_mesh: [u16; 4],
    arm_mesh: Vec<u16>,
    lamp_mesh: [u16; 2],
    ids: Vec<MeshId>,
    /// The train's head and each gate's arm at the last step, for drawing between steps.
    prev_head: f32,
    prev_arm: Vec<f32>,
    /// Seconds run, for the lamps' flashing.
    clock: f32,
}

impl Rail {
    /// The place's railway, its train first coming after `rest_s`; `None` without one.
    pub fn of(place: &Place, rest_s: f32) -> Option<Rail> {
        let railway = Railway::of(place, rest_s)?;
        let n = railway.gates.len();
        Some(Rail {
            prev_head: railway.train.head,
            prev_arm: vec![1.0; railway.train.gates.len()],
            arm_mesh: vec![0; n],
            railway,
            body_mesh: [0; 4],
            lamp_mesh: [0; 2],
            ids: Vec::new(),
            clock: 0.0,
        })
    }

    /// The meshes its instances draw; [`Rail::set_ids`] takes their ids in this order.
    pub fn meshes(&mut self, pal: &Palette) -> Vec<Mesh> {
        let mut out = Vec::new();
        let mut add = |m: WorldMesh| {
            let m = atlas_mesh(&m, MeshKind::Prop);
            let mut mesh = convert(&m, Part::Base);
            lighttiles::append(&mut mesh, &convert(&m, Part::Lamps));
            out.push(mesh);
            (out.len() - 1) as u16
        };
        for kind in [TRAIN_LOCO, TRAIN_BOXCAR, TRAIN_TANK, TRAIN_HOPPER] {
            let mut m = WorldMesh::default();
            let len = if kind == TRAIN_LOCO { LOCO_M } else { CAR_M };
            world::setpieces::train_body(&mut m, kind, len, pal);
            self.body_mesh[usize::from(kind)] = add(m);
        }
        for (k, g) in self.railway.gates.iter().enumerate() {
            let mut m = WorldMesh::default();
            world::setpieces::gate_arm(&mut m, g.arm_m, pal);
            self.arm_mesh[k] = add(m);
        }
        for k in 0..2 {
            let mut m = WorldMesh::default();
            world::setpieces::gate_lamp(&mut m, k);
            self.lamp_mesh[k] = add(m);
        }
        out
    }

    pub fn set_ids(&mut self, ids: Vec<MeshId>) {
        self.ids = ids;
    }

    /// Most instances a frame adds: every body, every arm and a lit lamp per gate.
    pub fn capacity(&self) -> usize {
        CARS + 1 + 2 * self.railway.gates.len()
    }

    /// One step of `dt` seconds, after the traffic's and the people's.
    pub fn step(
        &mut self,
        player: &traffic::Player,
        cars: &[traffic::Car],
        peds: &traffic::peds::Pedestrians,
        dt: f32,
    ) {
        self.prev_head = self.railway.train.head;
        for (p, g) in self.prev_arm.iter_mut().zip(&self.railway.train.gates) {
            *p = g.arm;
        }
        self.railway.step(player, cars, peds, dt);
        // A run that turned round starts from the other end: nothing to draw between.
        if self.railway.train.head < self.prev_head {
            self.prev_head = self.railway.train.head;
        }
        self.clock += dt;
    }

    /// The gate lines the traffic must not cross, and which crossings people keep off.
    pub fn barriers(&self) -> &[traffic::Barrier] {
        self.railway.barriers()
    }

    pub fn closed(&self) -> &[bool] {
        self.railway.closed()
    }

    /// Whether something reaching `reach` from `p` is clear of the train and of the track
    /// it runs on (and its swept margin), while the train is out on the line.
    pub fn clear_of_track(&self, p: [f32; 2], reach: f32) -> bool {
        let t = &self.railway.train;
        if matches!(t.phase, Phase::Resting { .. }) {
            return true;
        }
        t.line.project(p).1 > HALF_W_M + SWEEP_CLEAR_M + reach
    }

    /// Each train body `alpha` of the way through the step: its middle on the rail top,
    /// its yaw, its length and kind.
    fn bodies(&self, alpha: f32) -> impl Iterator<Item = (Vec3, f32, f32, u8)> + '_ {
        let t = &self.railway.train;
        let head = self.prev_head + (t.head - self.prev_head) * alpha;
        (0..=CARS).map(move |k| {
            let (front, len) = if k == 0 {
                (0.0, LOCO_M)
            } else {
                (
                    LOCO_M
                        + traffic::train::COUPLER_M
                        + (k - 1) as f32 * (CAR_M + traffic::train::COUPLER_M),
                    CAR_M,
                )
            };
            let u = head - front - 0.5 * len;
            let s = if t.forward { u } else { t.line.len - u };
            let (p, d) = t.line.at(s);
            let d = if t.forward { d } else { [-d[0], -d[1]] };
            (Vec3::from(p), (-d[0]).atan2(-d[1]), len, body_kind(k))
        })
    }

    /// Gate `k`'s arm, `alpha` of the way through the step (1 up, 0 down).
    fn arm(&self, k: usize, alpha: f32) -> f32 {
        let c = self.railway.gates[k].crossing;
        let (a, b) = (self.prev_arm[c], self.railway.train.gates[c].arm);
        a + (b - a) * alpha
    }

    /// Gate `k`'s arm as a transform: up `arm` of the way (1 up, 0 down).
    fn arm_transform(&self, k: usize, arm: f32) -> Mat4 {
        let g = &self.railway.gates[k];
        let pivot = world::setpieces::gate_pivot();
        let (along, _) = world::setpieces::gate_axes();
        let along = Vec3::new(along.x, along.y, along.z);
        let axis = along.cross(Vec3::Y).normalize();
        let up = Quat::from_axis_angle(axis, arm * 0.47 * core::f32::consts::PI);
        Mat4::from_rotation_translation(Quat::from_rotation_y(g.yaw), Vec3::from(g.pos))
            * Mat4::from_translation(Vec3::new(pivot.x, pivot.y, pivot.z))
            * Mat4::from_quat(up)
    }

    /// The player's car (`state`, of `spec`) against the train's bodies and the lowered
    /// arms near it: heavy boxes at rest. The hardest hit, if any.
    pub fn collide(
        &self,
        state: &mut vehicle::VehicleState,
        spec: &vehicle::VehicleSpec,
    ) -> Option<world::Bump> {
        let at = Vec3::from(state.pos);
        let mut hardest: Option<world::Bump> = None;
        let mut hit =
            |body: world::collide::BoxSolid, vel: [f32; 2], state: &mut vehicle::VehicleState| {
                let other = world::collide::OtherCar {
                    body,
                    vel,
                    spin: 0.0,
                    mass_kg: HEAVY_KG,
                };
                if let Some(h) = world::collide::hit_car(state, spec, &other) {
                    if hardest.is_none_or(|b| h.bump.speed > b.speed) {
                        hardest = Some(h.bump);
                    }
                }
            };
        let solid =
            |c: [f32; 2], u: [f32; 2], half: [f32; 2], y0: f32, y1: f32| world::collide::BoxSolid {
                c,
                u,
                half,
                y0,
                y1,
                kind: world::props::PropKind::ParkedCar { color: 0 },
                lot: world::collide::NO_LOT,
            };
        // The train hits with its own motion, as it moves.
        let v = self.railway.train.speed;
        for (p, yaw, len, _) in self.bodies(1.0) {
            if p.distance(at) > CONTACT_REACH_M + 0.5 * len {
                continue;
            }
            let d = [-yaw.sin(), -yaw.cos()];
            hit(
                solid([p.x, p.z], d, [0.5 * len, HALF_W_M], p.y + 0.3, p.y + 4.5),
                [d[0] * v, d[1] * v],
                state,
            );
        }
        for k in 0..self.railway.gates.len() {
            let g = &self.railway.gates[k];
            if self.arm(k, 1.0) > ARM_BLOCKS_BELOW
                || Vec3::from(g.pos).distance(at) > CONTACT_REACH_M
            {
                continue;
            }
            let b = g.barrier;
            let c = [0.5 * (b.a[0] + b.b[0]), 0.5 * (b.a[1] + b.b[1])];
            let u = [b.b[0] - b.a[0], b.b[1] - b.a[1]];
            let n = (u[0] * u[0] + u[1] * u[1]).sqrt().max(1e-3);
            let y = g.pos[1];
            hit(
                solid(c, [u[0] / n, u[1] / n], [0.5 * n, 0.12], y + 0.7, y + 1.3),
                [0.0; 2],
                state,
            );
        }
        hardest
    }

    /// How loud the nearest ringing crossing's bells are at `ear`, 0 to 1.
    pub fn bell(&self, ear: Vec3) -> f32 {
        let t = &self.railway.train;
        let mut loud: f32 = 0.0;
        for (k, &cs) in t.line.crossings.iter().enumerate() {
            if !t.warning(k) {
                continue;
            }
            let (p, _) = t.line.at(cs);
            let d = Vec3::from(p).distance(ear);
            loud = loud.max((1.0 - d / BELL_REACH_M).clamp(0.0, 1.0));
        }
        loud * loud
    }

    /// Appends the train's bodies, the gates' arms and their lit lamps near `eye`, `alpha`
    /// of the way through the step.
    pub fn push(
        &self,
        alpha: f32,
        eye: Vec3,
        lights: &crate::lighting::FrameLights,
        out: &mut Vec<MeshInstance>,
    ) {
        if self.ids.is_empty() {
            return;
        }
        let r2 = DRAW_RADIUS_M * DRAW_RADIUS_M;
        for (p, yaw, _, kind) in self.bodies(alpha) {
            if p.distance_squared(eye) > r2 {
                continue;
            }
            let m = Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), p);
            let id = self.ids[usize::from(self.body_mesh[usize::from(kind)])];
            let lit = lights.set_for(&crate::lighting::Bounds::cube(
                p + Vec3::Y * 2.0,
                0.5 * LOCO_M,
            ));
            out.push(MeshInstance {
                lights: lit,
                ..MeshInstance::new(id, m.to_cols_array_2d())
            });
        }
        let flash = ((self.clock * 2.0 * FLASH_HZ) as u32 % 2) as usize;
        for k in 0..self.railway.gates.len() {
            let g = &self.railway.gates[k];
            if Vec3::from(g.pos).distance_squared(eye) > r2 {
                continue;
            }
            let arm = self.arm(k, alpha);
            let id = self.ids[usize::from(self.arm_mesh[k])];
            let lit = lights.set_for(&crate::lighting::Bounds::cube(Vec3::from(g.pos), g.arm_m));
            out.push(MeshInstance {
                lights: lit,
                ..MeshInstance::new(id, self.arm_transform(k, arm).to_cols_array_2d())
            });
            if self.railway.train.warning(g.crossing) {
                let m = Mat4::from_rotation_translation(
                    Quat::from_rotation_y(g.yaw),
                    Vec3::from(g.pos),
                );
                let id = self.ids[usize::from(self.lamp_mesh[flash])];
                out.push(MeshInstance {
                    casts_shadow: false,
                    ..MeshInstance::new(id, m.to_cols_array_2d())
                });
            }
        }
    }

    pub fn hash(&self) -> u64 {
        self.railway.hash()
    }
}

#[cfg(test)]
mod tests {
    use crate::drive::Drive;
    use crate::input::{Action, InputState};
    use crate::{App, SceneSetup};
    use traffic::train::{Phase, HALF_W_M as HALF_W};

    /// The 2001 world, seed 1, its train held resting far off.
    fn basin() -> App {
        let q = "?seed=1&scenario=mirage_valley_2001&stream=1";
        let mut app = App::from_setup(&SceneSetup::from_query(q).unwrap()).unwrap();
        app.rail.as_mut().expect("a railway").railway.train.phase = Phase::Resting { t: 0.0 };
        app
    }

    /// The car at ground point `at` facing ground direction `dir`, standing.
    fn put(app: &mut App, at: [f32; 2], dir: [f32; 2]) {
        let (y, _) = app.ground.level_at(at);
        let heading = (-dir[0]).atan2(-dir[1]);
        app.drive = Drive::with_layout(
            app.drive.spec.clone(),
            app.drive.layout,
            [at[0], y, at[1]],
            heading,
        );
    }

    /// Runs `secs` of the game holding the throttle (or nothing: a standing car stays put),
    /// returning the fastest the car went up or down.
    fn run(app: &mut App, secs: f32, throttle: bool) -> f32 {
        let mut input = InputState::default();
        let mut up = 0.0f32;
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        while t < secs {
            input.clear();
            if throttle {
                input.set(Action::Throttle, 1.0);
            }
            app.update(&input, f64::from(dt));
            up = up.max(app.drive.state.vel[1].abs());
            t += dt;
        }
        up
    }

    /// Gate `k`'s approach: the middle of its gate line and the way the cars drive there.
    fn approach(app: &App, k: usize) -> ([f32; 2], [f32; 2], usize) {
        let g = &app.rail.as_ref().unwrap().railway.gates[k];
        let b = g.barrier;
        let mid = [0.5 * (b.a[0] + b.b[0]), 0.5 * (b.a[1] + b.b[1])];
        (mid, [-b.facing[0], -b.facing[1]], g.crossing)
    }

    /// The train's head put `ahead_m` short of crossing `c`, running at `speed`.
    fn running(app: &mut App, c: usize, ahead_m: f32, speed: f32) {
        let t = &mut app.rail.as_mut().unwrap().railway.train;
        t.forward = true;
        t.head = t.line.crossings[c] - ahead_m;
        t.speed = speed;
        t.phase = Phase::Running;
    }

    #[test]
    fn a_lowered_arm_holds_a_car_out_and_the_trains_side_is_a_thud() {
        let mut app = basin();
        let (mid, dir, c) = approach(&app, 0);
        // The train coming: the crossing closes and its arms come down.
        running(&mut app, c, 230.0, 0.0);
        put(
            &mut app,
            [mid[0] - dir[0] * 60.0, mid[1] - dir[1] * 60.0],
            dir,
        );
        run(&mut app, 14.0, false);
        let r = app.rail.as_ref().unwrap();
        assert!(r.closed()[c] && r.railway.train.gates[c].arm == 0.0);
        // Full throttle at the lowered arm from 60 m back: it stops the car at the line.
        let up = run(&mut app, 8.0, true);
        let p = app.drive.state.pos;
        let past = (p[0] - mid[0]) * dir[0] + (p[2] - mid[1]) * dir[1];
        let half = 0.5 * app.drive.spec.dims_m[0];
        assert!(
            past + half < 0.6,
            "through the arm: nose {:.2} m past it",
            past + half
        );
        assert!(up < 2.5, "launched: {up:.2} m/s up");

        // Its side: the car put on the crossing's far approach, the train standing
        // across it, driven into its side at full throttle from 25 m.
        let mut app = basin();
        let (mid, dir, c) = approach(&app, 0);
        running(&mut app, c, -60.0, 0.0);
        let start = [mid[0] - dir[0] * 20.0, mid[1] - dir[1] * 20.0];
        put(&mut app, start, dir);
        // Lift the arms out of the way to reach the train.
        for g in &mut app.rail.as_mut().unwrap().railway.train.gates {
            g.arm = 1.0;
            g.closed_s = 0.0;
        }
        let up = run(&mut app, 4.0, true);
        let s = &app.drive.state;
        let line = &app.rail.as_ref().unwrap().railway.train.line;
        let (_, off) = line.project([s.pos[0], s.pos[2]]);
        assert!(
            off > traffic::train::HALF_W_M,
            "into the train: {off:.2} m off its line"
        );
        assert!(up < 2.5, "launched: {up:.2} m/s up");
        assert!(s.speed_mps.abs() < 4.0, "bounced: {:.1} m/s", s.speed_mps);
        assert_eq!(app.rail.as_ref().unwrap().railway.train.speed, 0.0);
    }

    #[test]
    fn a_glancing_hit_on_the_running_train_stops_it_and_never_launches_the_car() {
        for sigma in [-1.0f32, 1.0] {
            let mut app = basin();
            let (_, dir, c) = approach(&app, 0);
            let line = app.rail.as_ref().unwrap().railway.train.line.clone();
            let (x, _) = line.at(line.crossings[c]);
            // The train running across the crossing at its cruise, its arms lifted.
            running(&mut app, c, -60.0, traffic::train::CRUISE_MPS);
            for g in &mut app.rail.as_mut().unwrap().railway.train.gates {
                g.arm = 1.0;
                g.closed_s = 0.0;
            }
            // At 40 degrees to the road, at full throttle into its side.
            let (sn, cs) = (40.0f32.to_radians() * sigma).sin_cos();
            let aim = [dir[0] * cs - dir[1] * sn, dir[0] * sn + dir[1] * cs];
            put(&mut app, [x[0] - aim[0] * 12.0, x[2] - aim[1] * 12.0], aim);
            let up = run(&mut app, 4.0, true);
            let r = app.rail.as_ref().unwrap();
            assert_eq!(r.railway.train.speed, 0.0, "{sigma}: it stopped");
            assert!(up <= 0.3, "{sigma}: launched: {up:.2} m/s up");
            let s = &app.drive.state;
            let (_, off) = line.project([s.pos[0], s.pos[2]]);
            assert!(off > traffic::train::HALF_W_M, "{sigma}: into it: {off:.2}");
        }
    }

    #[test]
    fn a_reset_by_the_standing_train_puts_the_car_clear_of_it() {
        let mut app = basin();
        let (mid, dir, c) = approach(&app, 0);
        // The train standing across the crossing, its nose 60 m past it.
        running(&mut app, c, -60.0, 0.0);
        let line = app.rail.as_ref().unwrap().railway.train.line.clone();
        let (x, _) = line.at(line.crossings[c]);
        // From on the crossing, from its approach and from just past it.
        for at in [
            [x[0], x[2]],
            mid,
            [x[0] + dir[0] * 4.0, x[2] + dir[1] * 4.0],
        ] {
            put(&mut app, at, dir);
            app.reset_car();
            let s = &app.drive.state;
            let r = app.rail.as_ref().unwrap();
            let (_, off) = line.project([s.pos[0], s.pos[2]]);
            assert!(off > HALF_W + 2.0, "reset {off:.1} m off the track");
            let [len, wid, _] = app.drive.spec.dims_m;
            let o = traffic::train::Obstacle::car(
                [s.pos[0], s.pos[2]],
                {
                    let f = s.forward();
                    let n = (f[0] * f[0] + f[2] * f[2]).sqrt();
                    [f[0] / n, f[2] / n]
                },
                0.5 * len,
                0.5 * wid,
            );
            assert!(!r.railway.train.overlaps(&o, 1.0));
            // And it drives off: nothing holds it.
            run(&mut app, 1.0, false);
            assert!(app.drive.state.speed_mps.abs() < 0.5);
        }
    }

    #[test]
    fn the_train_stops_short_of_a_car_stopped_on_the_track() {
        let mut app = basin();
        let (_, _, c) = approach(&app, 0);
        let line = app.rail.as_ref().unwrap().railway.train.line.clone();
        let (x, d) = line.at(line.crossings[c]);
        // Stopped across the track on the crossing, square to it.
        put(&mut app, [x[0], x[2]], [-d[1], d[0]]);
        running(&mut app, c, 300.0, traffic::train::CRUISE_MPS);
        run(&mut app, 60.0, false);
        let t = &app.rail.as_ref().unwrap().railway.train;
        assert_eq!(t.speed, 0.0);
        let gap = line.crossings[c] - t.head;
        assert!(gap > 1.0 && gap < 20.0, "stood {gap:.1} m short");
        assert_eq!(t.hard_stops, 0);
        assert!(app.drive.state.speed_mps.abs() < 0.1, "never touched");
    }
}
