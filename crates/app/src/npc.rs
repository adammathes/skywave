//! NPC traffic in the game (`docs/NPCS.md`, "In the game"): the lane graph and a
//! [`traffic::Traffic`] for the place, stepped with the 120 Hz sim, drawn as instances of
//! the parked-car meshes, lit after dusk and heard as it passes.
//!
//! - **Setup.** [`Npcs::new`] builds the lane graph and the pool (sized for
//!   [`Density::Busy`]) at scene setup. The first time traffic is on, [`Npcs::build_meshes`]
//!   makes the meshes: every built-in shape in each of [`PAINTS`] street colors at levels
//!   of detail 1 and 2 ([`car::parked_mesh`], lamps lit so the instance's glow switches
//!   them); a session with traffic off never builds them. A scene with traffic is warmed
//!   up for [`WARMUP_S`] before its first frame, so the streets round the start already
//!   have cars.
//! - **Step.** Each sim step feeds the player's car (the obstacle every NPC car sees), the
//!   camera's view cone, the clock and the density, then steps and populates. The pose of
//!   every car before the step is kept, so drawing interpolates like the player's car.
//!   With traffic off and no car left the sim does not run.
//! - **Draw.** One instance per live car within [`DRAW_RADIUS_M`], level 1 within
//!   [`DETAIL_RADIUS_M`] and level 2 beyond, ordered by mesh so each mesh is one instanced
//!   draw, lit by the frame's lights round it. Its lamps glow with the lights on.
//! - **Lights.** The nearest [`LIT_CARS`] cars with their lights on within
//!   [`LIGHT_RADIUS_M`] of the eye each add their two low beams (spot lights with coronas)
//!   to the frame's dynamic lights, after the player's: at most [`NPC_LIGHTS`].
//! - **Sound.** The nearest [`TRAFFIC_VOICES`] moving cars within [`HEAR_RADIUS_M`] are
//!   the world synth's traffic voices, panned by where they are, attenuated by distance,
//!   pitched by speed and the Doppler shift of the relative speed.
//!
//! Every per-frame path writes into buffers sized at setup: nothing allocates after it
//! but switching traffic off, which drops the cars.

use audio::synth::{TrafficVoice, TRAFFIC_VOICES};
use core::f32::consts::{PI, TAU};
use glam::{Mat4, Quat, Vec3};
use place::Place;
use render::{LightSet, Mesh, MeshId, MeshInstance, PointLight};
use skywave_core::{hash64, Seed};
use traffic::{
    Clock, Density, Inputs, LaneGraph, Player, SignalClock, Traffic, TrafficBudget, View,
};
use vehicle::shape;

use crate::lighting::{light_set, Bounds};
use crate::{car, drive, vehicles};

/// Every density, in the order the World page offers them.
pub const DENSITIES: [Density; 4] = [Density::Off, Density::Low, Density::Normal, Density::Busy];
/// Names of [`DENSITIES`] on the World page.
pub const DENSITY_NAMES: [&str; 4] = ["Off", "Low", "Normal", "Busy"];

/// Stable name of a density (settings JSON, the `traffic=` scene key).
pub fn density_name(d: Density) -> &'static str {
    match d {
        Density::Off => "off",
        Density::Low => "low",
        Density::Normal => "normal",
        Density::Busy => "busy",
    }
}

pub fn density_from_name(name: &str) -> Option<Density> {
    DENSITIES.into_iter().find(|&d| density_name(d) == name)
}

/// NPC paints: the most common of the era's street colors (`shape::ERA_1980S_PAINT`, 86%
/// of its weight), each car's picked by its paint seed with their weights.
pub const PAINTS: usize = 8;
/// Seconds of traffic run before the first frame of a scene with traffic.
pub const WARMUP_S: f32 = 20.0;
/// NPC cars are drawn this far from the eye, m (as far as parked cars)...
pub const DRAW_RADIUS_M: f32 = crate::propinst::CAR_DRAW_RADIUS_M;
/// ...at level of detail 1 this near, level 2 beyond, m.
pub const DETAIL_RADIUS_M: f32 = crate::propinst::CAR_DETAIL_RADIUS_M;
/// Cars nearer than this are lit by the frame's lights, m.
const LIT_RADIUS_M: f32 = crate::propinst::LIT_RADIUS_M;
/// Most NPC cars whose low beams light the scene, and how near the eye they must be, m.
pub const LIT_CARS: usize = 8;
pub const LIGHT_RADIUS_M: f32 = 150.0;
/// Most lights the traffic adds to a frame: two low beams per lit car.
pub const NPC_LIGHTS: usize = 2 * LIT_CARS;
/// Cars are heard this far away, m.
pub const HEAR_RADIUS_M: f32 = 120.0;
/// Speed of sound, m/s.
const SOUND_MPS: f32 = 343.0;
/// Head lights come on this long after sunset (and off this long before sunrise), s:
/// about when the street lamps' photocell switches (civil twilight).
const TWILIGHT_S: f32 = 1500.0;
/// Light-culling half size round a car and the height of its middle, m.
const CAR_HALF_M: f32 = 2.8;

/// A car's pose at the start of the step, to interpolate from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Snap {
    id: u32,
    pos: [f32; 3],
    heading: f32,
}

/// The traffic of one place (see the module docs).
pub struct Npcs {
    graph: LaneGraph,
    traffic: Traffic,
    /// A pool with no car, to start over from when traffic is switched off.
    empty: Traffic,
    signals: SignalClock,
    density: Density,
    prev: Vec<Snap>,
    /// Mesh ids, `(shape * PAINTS + paint) * 2 + lod - 1`, once built and registered.
    ids: Vec<MeshId>,
    /// Per shape, its head lamps' centers in the mesh's frame.
    heads: Vec<[Vec3; 2]>,
    /// Scratch for drawing in mesh order: mesh index and car slot.
    order: Vec<(u16, u16)>,
    /// Per-step sim time, ms, smoothed, for the overlay.
    step_ms: f32,
    warmed: bool,
}

/// What a step needs from the game.
#[derive(Clone, Copy, Debug)]
pub struct StepInputs {
    pub player: Player,
    pub view: View,
    pub clock: Clock,
}

impl Npcs {
    /// The lane graph and an empty pool for `place`, seeded by `seed`.
    pub fn new(place: &Place, seed: Seed) -> Npcs {
        let graph = LaneGraph::new(place);
        let budget = TrafficBudget {
            shapes: vehicles::BUILTIN_SHAPES.len() as u8,
            ..TrafficBudget::default()
        };
        let traffic = Traffic::new(&graph, budget, seed);
        let cap = budget.cars;
        Npcs {
            graph,
            empty: traffic.clone(),
            traffic,
            signals: SignalClock::default(),
            density: Density::Off,
            prev: Vec::with_capacity(cap),
            ids: Vec::new(),
            heads: Vec::new(),
            order: Vec::with_capacity(cap),
            step_ms: 0.0,
            warmed: false,
        }
    }

    /// Whether the cars' meshes are built and registered.
    pub fn has_meshes(&self) -> bool {
        !self.ids.is_empty()
    }

    /// Builds the cars' meshes and registers each with `register`, which returns its id.
    pub fn build_meshes(&mut self, mut register: impl FnMut(Mesh) -> MeshId) -> Result<(), String> {
        let shapes: Vec<shape::ShapeSpec> = vehicles::BUILTIN_SHAPES
            .iter()
            .map(|(_, text)| vehicles::load_shape(text))
            .collect::<Result<_, _>>()?;
        let mut ids = Vec::with_capacity(shapes.len() * PAINTS * 2);
        let mut heads = Vec::with_capacity(shapes.len());
        for s in &shapes {
            for (p, &(paint, _)) in shape::ERA_1980S_PAINT[..PAINTS].iter().enumerate() {
                for lod in [1, 2] {
                    let (m, h) = car::parked_mesh_and_lamps(s, 0, Some(paint), lod)?;
                    if p == 0 && lod == 1 {
                        heads.push(h);
                    }
                    ids.push(register(m));
                }
            }
        }
        self.ids = ids;
        self.heads = heads;
        Ok(())
    }

    /// Most cars ever live, so most instances a frame draws.
    pub fn capacity(&self) -> usize {
        self.traffic.budget.cars
    }

    pub fn density(&self) -> Density {
        self.density
    }

    /// Live cars.
    pub fn count(&self) -> usize {
        self.traffic.cars().len()
    }

    /// The live cars.
    pub fn cars(&self) -> &[traffic::Car] {
        self.traffic.cars()
    }

    /// The traffic sim's time per step, ms (smoothed).
    pub fn step_ms(&self) -> f32 {
        self.step_ms
    }

    /// Keeps `density` of cars from the next step; off drops every car at once.
    pub fn set_density(&mut self, density: Density) {
        if density == Density::Off && self.density != Density::Off {
            self.traffic = self.empty.clone();
            self.prev.clear();
        }
        self.density = density;
    }

    /// Whether the traffic runs: on, or cars still to clear.
    pub fn active(&self) -> bool {
        self.density != Density::Off || self.count() > 0
    }

    /// One sim step (the signals always tick; the cars only when [`Npcs::active`]).
    pub fn step(&mut self, inp: &StepInputs) {
        self.signals.step();
        if !self.active() {
            return;
        }
        self.prev.clear();
        self.prev.extend(self.traffic.cars().iter().map(|c| Snap {
            id: c.id,
            pos: c.pos,
            heading: c.heading,
        }));
        let inputs = Inputs {
            tick: self.signals.tick,
            player: inp.player,
            view: inp.view,
            clock: inp.clock,
            density: self.density,
        };
        self.traffic.step(&self.graph, &inputs);
        self.traffic.populate(&self.graph, &inputs);
    }

    /// Runs [`WARMUP_S`] of steps the first time it is called with traffic on, the player
    /// and the view held still.
    pub fn warm_up(&mut self, inp: &StepInputs) {
        if self.warmed || self.density == Density::Off {
            return;
        }
        self.warmed = true;
        for _ in 0..(WARMUP_S * skywave_core::SIM_HZ as f32) as u32 {
            self.step(inp);
        }
    }

    /// Notes `ms` of traffic sim for `steps` steps, for the overlay.
    pub fn note_time(&mut self, ms: f64, steps: u32) {
        if steps > 0 && ms.is_finite() && ms >= 0.0 {
            let per = (ms / f64::from(steps)) as f32;
            self.step_ms += (per - self.step_ms) * 0.1;
        }
    }

    /// Fingerprint of the traffic, for determinism checks.
    pub fn hash(&self) -> u64 {
        hash64(self.traffic.hash(), &[self.signals.tick])
    }

    /// Car `i`'s ground point and heading `alpha` of the way through the last step.
    fn pose(&self, i: usize, alpha: f32) -> (Vec3, f32) {
        let c = &self.traffic.cars()[i];
        let now = (Vec3::from(c.pos), c.heading);
        let prev = match self.prev.get(i) {
            Some(p) if p.id == c.id => Some(p),
            _ => self.prev.iter().find(|p| p.id == c.id),
        };
        let Some(p) = prev else {
            return now;
        };
        let turn = (c.heading - p.heading + PI).rem_euclid(TAU) - PI;
        (
            Vec3::from(p.pos).lerp(now.0, alpha),
            p.heading + turn * alpha,
        )
    }

    /// The mesh index of car `c` at the near or far level.
    fn mesh_index(c: &traffic::Car, far: bool) -> usize {
        (usize::from(c.shape) * PAINTS + paint_of(c.paint)) * 2 + usize::from(far)
    }

    /// Appends an instance for every car within [`DRAW_RADIUS_M`] of `eye`, `alpha` of the
    /// way through the step, in mesh order, lit by the nearest of `lights` when close;
    /// lamps glow by `lamp_glow` on cars with their lights on. Returns how many.
    pub fn push(
        &mut self,
        alpha: f32,
        eye: Vec3,
        lights: &[PointLight],
        lamp_glow: f32,
        out: &mut Vec<MeshInstance>,
    ) -> usize {
        if self.ids.is_empty() {
            return 0;
        }
        let (draw2, near2) = (
            DRAW_RADIUS_M * DRAW_RADIUS_M,
            DETAIL_RADIUS_M * DETAIL_RADIUS_M,
        );
        let mut order = core::mem::take(&mut self.order);
        order.clear();
        for (i, c) in self.traffic.cars().iter().enumerate() {
            let d2 = Vec3::from(c.pos).distance_squared(eye);
            if d2 <= draw2 {
                order.push((Self::mesh_index(c, d2 > near2) as u16, i as u16));
            }
        }
        order.sort_unstable();
        let lit2 = LIT_RADIUS_M * LIT_RADIUS_M;
        for &(mesh, i) in &order {
            let c = &self.traffic.cars()[usize::from(i)];
            let (pos, heading) = self.pose(usize::from(i), alpha);
            let transform = Mat4::from_rotation_translation(Quat::from_rotation_y(heading), pos);
            let bounds = Bounds::cube(pos + Vec3::Y, CAR_HALF_M);
            let lights = if bounds.distance_squared(eye) < lit2 {
                light_set(lights, &bounds)
            } else {
                LightSet::EMPTY
            };
            out.push(MeshInstance {
                lights,
                glow: if c.lights { lamp_glow } else { 0.0 },
                ..MeshInstance::new(self.ids[usize::from(mesh)], transform.to_cols_array_2d())
            });
        }
        let n = order.len();
        self.order = order;
        n
    }

    /// Writes the low beams of the nearest [`LIT_CARS`] cars with their lights on within
    /// [`LIGHT_RADIUS_M`] of `eye` into `out`, nearest first. Returns how many lights.
    pub fn lights(&self, alpha: f32, eye: Vec3, out: &mut [PointLight; NPC_LIGHTS]) -> usize {
        if self.heads.is_empty() {
            return 0;
        }
        let mut best = [(f32::INFINITY, 0usize); LIT_CARS];
        let mut n = 0;
        let reach2 = LIGHT_RADIUS_M * LIGHT_RADIUS_M;
        for (i, c) in self.traffic.cars().iter().enumerate() {
            if !c.lights {
                continue;
            }
            let d2 = Vec3::from(c.pos).distance_squared(eye);
            if d2 > reach2 || (n == LIT_CARS && d2 >= best[n - 1].0) {
                continue;
            }
            let mut k = n.min(LIT_CARS - 1);
            while k > 0 && best[k - 1].0 > d2 {
                best[k] = best[k - 1];
                k -= 1;
            }
            best[k] = (d2, i);
            n = (n + 1).min(LIT_CARS);
        }
        for (k, &(_, i)) in best[..n].iter().enumerate() {
            let (pos, heading) = self.pose(i, alpha);
            let pose = drive::Pose {
                pos,
                rot: Quat::from_rotation_y(heading),
            };
            let heads = self.heads[usize::from(self.traffic.cars()[i].shape)];
            out[2 * k] = drive::headlight(&pose, heads[0]);
            out[2 * k + 1] = drive::headlight(&pose, heads[1]);
        }
        2 * n
    }

    /// The engines of the nearest [`TRAFFIC_VOICES`] moving cars within [`HEAR_RADIUS_M`] of
    /// the listener at `ear` facing `forward` and moving at `velocity` (m/s), nearest in
    /// the first voice; the rest silent.
    pub fn voices(
        &self,
        alpha: f32,
        ear: Vec3,
        forward: Vec3,
        velocity: Vec3,
    ) -> [TrafficVoice; TRAFFIC_VOICES] {
        let mut best = [(f32::INFINITY, 0usize); TRAFFIC_VOICES];
        let mut n = 0;
        let reach2 = HEAR_RADIUS_M * HEAR_RADIUS_M;
        for (i, c) in self.traffic.cars().iter().enumerate() {
            let d2 = Vec3::from(c.pos).distance_squared(ear);
            if c.speed < 0.5 || d2 > reach2 || (n == TRAFFIC_VOICES && d2 >= best[n - 1].0) {
                continue;
            }
            let mut k = n.min(TRAFFIC_VOICES - 1);
            while k > 0 && best[k - 1].0 > d2 {
                best[k] = best[k - 1];
                k -= 1;
            }
            best[k] = (d2, i);
            n = (n + 1).min(TRAFFIC_VOICES);
        }
        let right = Vec3::new(-forward.z, 0.0, forward.x).normalize_or_zero();
        let mut out = [TrafficVoice::default(); TRAFFIC_VOICES];
        for (v, &(_, i)) in out.iter_mut().zip(&best[..n]) {
            let c = &self.traffic.cars()[i];
            let (pos, _) = self.pose(i, alpha);
            let rel = pos + Vec3::Y * 0.5 - ear;
            let d = rel.length().max(1.0);
            let u = rel / d;
            let vel = Vec3::new(c.dir[0], 0.0, c.dir[1]) * c.speed;
            // Closing speed: positive while the car and the listener draw together.
            let closing = -(vel - velocity).dot(u);
            let doppler = SOUND_MPS / (SOUND_MPS - closing.clamp(-100.0, 100.0));
            let load = 0.35 + 0.65 * (c.speed / 15.0).min(1.0);
            *v = TrafficVoice {
                gain: ((6.0 / d).min(1.0) * load).min(1.0),
                pan: u.dot(right).clamp(-1.0, 1.0),
                hz: (30.0 + 4.5 * c.speed) * doppler,
            };
        }
        out
    }
}

/// Which of [`PAINTS`] a car's paint seed lands on, by the era's weights.
pub fn paint_of(seed: u32) -> usize {
    let w = &shape::ERA_1980S_PAINT[..PAINTS];
    let total: u32 = w.iter().map(|p| p.1).sum();
    let mut r = seed % total;
    for (k, &(_, weight)) in w.iter().enumerate() {
        if r < weight {
            return k;
        }
        r -= weight;
    }
    0
}

/// Head lights come on [`TWILIGHT_S`] after sunset at `latitude_deg` on `day_of_year`, and
/// go off as long before sunrise.
pub fn clock(seconds_of_day: f64, latitude_deg: f32, day_of_year: u16) -> Clock {
    let (rise, set) = crate::daylight::sun_times(latitude_deg, day_of_year);
    Clock {
        seconds_of_day: seconds_of_day as f32,
        // Whole seconds, so a last-bit difference in the platform's trig never moves them.
        dusk_s: (set + TWILIGHT_S).min(86_399.0).round(),
        dawn_s: (rise - TWILIGHT_S).max(0.0).round(),
    }
}

/// The player's car as the traffic sees it: at `pose`, `speed` m/s, `dims` m long and wide.
pub fn player(pose: &drive::Pose, speed: f32, dims: [f32; 3]) -> Player {
    let f = pose.forward();
    Player {
        pos: pose.pos.to_array(),
        // The traffic crate's own trig, so the traffic stays bit-identical everywhere.
        heading: traffic::math::atan2(-f.x, -f.z),
        speed: speed.abs(),
        half_len: 0.5 * dims[0],
        half_w: 0.5 * dims[1],
    }
}

/// The camera at `eye` looking along `forward` with vertical field of view `fov_y_rad`
/// on a target `aspect` wide, as a horizontal cone (a little wider, for the margin).
pub fn view(eye: Vec3, forward: Vec3, fov_y_rad: f32, aspect: f32) -> View {
    let flat = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
    View {
        pos: eye.to_array(),
        dir: [flat.x, flat.z],
        tan_half: {
            let (sin, cos) = traffic::math::sin_cos(0.5 * fov_y_rad);
            sin / cos * aspect * 1.15
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{App, InputState, SceneSetup};

    fn app(query: &str) -> App {
        App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap()
    }

    /// The instances of `app`'s last view drawing an NPC car.
    fn npc_instances(app: &mut App, alpha: f32) -> usize {
        let ids = app.npcs.ids.clone();
        let view = app.scene_view(alpha, 16.0 / 9.0);
        view.instances
            .iter()
            .filter(|i| ids.contains(&i.mesh))
            .count()
    }

    #[test]
    fn densities_have_stable_names() {
        for d in DENSITIES {
            assert_eq!(density_from_name(density_name(d)), Some(d));
        }
        assert_eq!(density_from_name("jam"), None);
        let w = shape::ERA_1980S_PAINT[..PAINTS].iter().map(|p| p.1);
        let total: u32 = w.sum();
        let mut seen = [0u32; PAINTS];
        for seed in 0..total {
            seen[paint_of(seed)] += 1;
        }
        for (k, n) in seen.iter().enumerate() {
            assert_eq!(*n, shape::ERA_1980S_PAINT[k].1, "paint {k} by its weight");
        }
    }

    #[test]
    fn scene_setup_builds_traffic_for_town_and_coast() {
        for query in [
            "place=town&t=12:00&traffic=busy&still=1&stream=0",
            "place=coast&t=12:00&traffic=busy&still=1",
        ] {
            let mut app = app(query);
            assert_eq!(app.npcs.density(), Density::Busy);
            assert!(app.npcs.graph.lanes.len() > 50, "{query}: a lane graph");
            assert_eq!(
                app.traffic_count(),
                0,
                "nothing runs before the first frame"
            );
            app.update(&InputState::default(), 1.0 / 60.0);
            let n = app.traffic_count();
            eprintln!("{query}: {n} cars after the warm-up");
            assert!(n >= 20, "{query}: {n} cars after the warm-up");
            // Every car is drawn at most once, and nothing is drawn beyond the radius.
            let eye = app.camera.pose(&app.drive, 1.0).eye;
            let near = app
                .npcs
                .cars()
                .iter()
                .filter(|c| Vec3::from(c.pos).distance(eye) <= DRAW_RADIUS_M)
                .count();
            assert_eq!(npc_instances(&mut app, 1.0), near, "{query}");
        }
    }

    /// One instance per live car in reach, for both levels of detail, in mesh order (one
    /// draw a mesh); traffic off draws none and drops every car; drawing allocates nothing.
    #[test]
    fn instances_follow_the_live_cars_and_off_draws_none() {
        let mut app = app("place=town&t=12:00&traffic=busy&still=1&stream=0");
        app.update(&InputState::default(), 0.0);
        app.npcs.prev.clear();
        let caps = (
            app.instances.capacity(),
            app.npcs.order.capacity(),
            app.npcs.prev.capacity(),
        );
        let (mut near, mut far) = (false, false);
        for step in 0..240 {
            let inputs = app.traffic_inputs();
            app.npcs.step(&inputs);
            let eye = app.camera.pose(&app.drive, 1.0).eye;
            let world = app.instances.len();
            let drawn = npc_instances(&mut app, (step % 4) as f32 / 4.0);
            let live = app.npcs.cars();
            let reach = live
                .iter()
                .filter(|c| Vec3::from(c.pos).distance(eye) <= DRAW_RADIUS_M)
                .count();
            assert_eq!(drawn, reach, "step {step}");
            for c in live {
                let d = Vec3::from(c.pos).distance(eye);
                near |= d < DETAIL_RADIUS_M;
                far |= d > DETAIL_RADIUS_M && d < DRAW_RADIUS_M;
            }
            let ids = &app.npcs.ids;
            let meshes: Vec<MeshId> = app.instances[world.min(app.instances.len())..]
                .iter()
                .filter(|i| ids.contains(&i.mesh))
                .map(|i| i.mesh)
                .collect();
            assert!(meshes.windows(2).all(|w| w[0].0 <= w[1].0), "mesh order");
        }
        assert!(near && far, "both levels of detail drawn");
        assert_eq!(
            (
                app.instances.capacity(),
                app.npcs.order.capacity(),
                app.npcs.prev.capacity()
            ),
            caps,
            "no allocation per frame or step"
        );
        app.npcs.set_density(Density::Off);
        assert_eq!(app.traffic_count(), 0);
        assert_eq!(npc_instances(&mut app, 1.0), 0, "off draws nothing");
        assert!(!app.npcs.active(), "and the sim is idle");
        let hash = app.npcs.traffic.hash();
        let inputs = app.traffic_inputs();
        app.npcs.step(&inputs);
        assert_eq!(app.npcs.traffic.hash(), hash);
    }

    /// At night the nearest cars with their lights on add two low beams each, never more
    /// than the traffic's share of the frame's lights, and their lamps glow; by day none.
    #[test]
    fn headlights_after_dusk_stay_within_the_light_budget() {
        let mut app = app("place=town&t=22:30&traffic=busy&still=1&stream=0");
        app.update(&InputState::default(), 0.0);
        let lit = app.npcs.cars().iter().filter(|c| c.lights).count();
        assert!(lit * 10 >= app.traffic_count() * 9, "{lit} lit");
        let eye = app.camera.pose(&app.drive, 1.0).eye;
        let mut out = [PointLight::omni([0.0; 3], [0.0; 3], 0.0); NPC_LIGHTS];
        let n = app.npcs.lights(1.0, eye, &mut out);
        assert!(n > 0 && n <= NPC_LIGHTS && n.is_multiple_of(2), "{n}");
        // Nearest first, and no lit car in reach nearer than the farthest chosen is left out.
        let dist = |l: &PointLight| Vec3::from(l.position).distance(eye);
        assert!(out[..n]
            .windows(2)
            .all(|w| dist(&w[0]) <= dist(&w[1]) + 3.0));
        let farthest = dist(&out[n - 1]) - 3.0;
        let closer = app
            .npcs
            .cars()
            .iter()
            .filter(|c| c.lights && Vec3::from(c.pos).distance(eye) < farthest)
            .count();
        assert!(closer <= n / 2, "{closer} nearer cars, {} chosen", n / 2);
        for l in &out[..n] {
            assert!(
                l.direction != [0.0; 3] && l.corona.radius > 0.0,
                "a spot with a corona"
            );
        }
        let view = app.scene_view(1.0, 16.0 / 9.0);
        assert!(view.lights.len() <= render::MAX_LIGHTS);
        let spots = view.lights.iter().filter(|l| l.radius == 60.0).count();
        assert!(spots <= NPC_LIGHTS, "{spots}");
        let ids = app.npcs.ids.clone();
        let view = app.scene_view(1.0, 16.0 / 9.0);
        assert!(view
            .instances
            .iter()
            .any(|i| ids.contains(&i.mesh) && i.glow > 0.0));

        let mut noon = self::app("place=town&t=12:00&traffic=busy&still=1&stream=0");
        noon.update(&InputState::default(), 0.0);
        let eye = noon.camera.pose(&noon.drive, 1.0).eye;
        assert_eq!(noon.npcs.lights(1.0, eye, &mut out), 0);
        let view = noon.scene_view(1.0, 16.0 / 9.0);
        assert!(view
            .instances
            .iter()
            .filter(|i| ids.contains(&i.mesh))
            .all(|i| i.glow == 0.0));
    }

    /// The traffic is heard: the nearest moving cars, louder near, pitched up approaching.
    #[test]
    fn passing_cars_are_voiced_nearest_first() {
        let mut app = app("place=town&t=12:00&traffic=busy&still=1&stream=0");
        app.update(&InputState::default(), 0.0);
        let c = *app
            .npcs
            .cars()
            .iter()
            .find(|c| c.speed > 5.0)
            .expect("a moving car");
        app.npcs.prev.clear();
        let at = Vec3::from(c.pos);
        let fwd = Vec3::new(c.dir[0], 0.0, c.dir[1]);
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);
        // Standing ahead of it facing the same way: it closes from behind.
        let ahead = at + fwd * 20.0;
        let v = app.npcs.voices(1.0, ahead, fwd, Vec3::ZERO);
        let behind = at - fwd * 20.0;
        let w = app.npcs.voices(1.0, behind, fwd, Vec3::ZERO);
        let voiced = |v: &[TrafficVoice]| v.iter().filter(|v| v.gain > 0.0).count();
        assert!(voiced(&v) >= 1 && voiced(&w) >= 1);
        let base = 30.0 + 4.5 * c.speed;
        let hz = |v: &[TrafficVoice]| {
            v.iter()
                .map(|v| v.hz)
                .find(|&h| (h / base - 1.0).abs() < 0.1)
                .unwrap()
        };
        assert!(
            hz(&v) > base && hz(&w) < base,
            "{} {} {base}",
            hz(&v),
            hz(&w)
        );
        // To the listener's right when it faces across the car's path.
        let side = app.npcs.voices(1.0, at - right * 10.0, fwd, Vec3::ZERO);
        assert!(side[0].pan > 0.5, "{:?}", side[0]);
        assert!(side[0].gain >= side[1].gain);
    }

    /// The in-game traffic step (step and populate, 96 busy cars on the coast) within its
    /// budget: best of five windows' means under 0.5 ms in this build, asserted only under
    /// `SKYWAVE_ENFORCE_BUDGETS`.
    #[test]
    fn the_traffic_step_is_within_budget() {
        let mut app = app("place=coast&t=12:00&traffic=busy&still=1");
        app.update(&InputState::default(), 0.0);
        let inputs = app.traffic_inputs();
        let mut means = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            let steps = 240;
            for _ in 0..steps {
                app.npcs.step(&inputs);
            }
            means.push(start.elapsed().as_secs_f64() * 1e3 / f64::from(steps));
        }
        let best = means.iter().copied().fold(f64::INFINITY, f64::min);
        eprintln!(
            "traffic step: {} cars, {best:.3} ms per step (windows {means:.3?})",
            app.traffic_count()
        );
        assert!(app.traffic_count() >= 60, "{}", app.traffic_count());
        if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
            assert!(best < 0.5, "{best:.3} ms per step");
        }
    }
}
