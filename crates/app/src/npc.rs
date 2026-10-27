//! NPC traffic in the game (`docs/NPCS.md`, "In the game"): the lane graph and a
//! [`traffic::Traffic`] for the place, stepped with the 120 Hz sim, drawn as instances of
//! the parked-car meshes, lit after dusk and heard as it passes.
//!
//! - **Setup.** [`Npcs::new`] builds the lane graph and the pool (sized for
//!   [`Density::Busy`]) at scene setup. The first time traffic is on, [`Npcs::build_meshes`]
//!   finds the meshes: every traffic shape (`vehicles::TRAFFIC_SHAPES`) in the place's four
//!   era street colors for it (`propinst::place_paints`, the parked cars' own) at levels
//!   of detail 1 and 2, the
//!   parked cars' meshes where the place parks that shape and paint, built
//!   ([`car::parked_mesh`]) only where it does not; a session with traffic off never
//!   builds any. Lamps are built lit so the instance's glow switches them.
//! - **Warm-up.** Whenever traffic is switched on, [`WARMUP_S`] of it runs with the player
//!   and the view held still, so the streets round the car already have cars: all at once
//!   while the scene loads, else [`WARMUP_BATCH`] steps a frame until done.
//! - **Step.** Each sim step feeds the player's car (the obstacle every NPC car sees), the
//!   camera's view cone, the clock and the density, then steps and populates. The pose of
//!   every car before the step is kept, so drawing interpolates like the player's car.
//!   With traffic off and no car left the sim does not run.
//! - **Collision.** Before each traffic step, [`Npcs::collide`] meets the player's car
//!   with every NPC car near it, each as its shape's body box ([`CarBody`]) and mass, through
//!   `world::collide::hit_car`: the player's car bumps and slows, the NPC car is shoved
//!   ([`traffic::Traffic::shove`]), brakes, stands a moment and steers back to its lane.
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
//! - **Pedestrians.** [`Npcs`] also owns the place's people ([`crate::peds`]): built at
//!   setup, at the same density, warmed up with the cars (and while a scene loads,
//!   [`PEDS_PREROLL_S`] before them), stepped after the cars at the signal tick they
//!   reached.
//!
//! Every per-frame path writes into buffers sized at setup: nothing allocates after it
//! but switching traffic off, which drops the cars and the people.

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

use crate::lighting::{Bounds, FrameLights};
use crate::{car, drive, peds, vehicles};

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

/// NPC paints per shape: the place's era paints for it (the parked cars'), each car's
/// picked by its paint seed.
pub const PAINTS: usize = crate::propinst::ERA_PAINTS;
/// Seconds of traffic run when it is switched on...
pub const WARMUP_S: f32 = 20.0;
const WARMUP_STEPS: u32 = (WARMUP_S as u32) * skywave_core::SIM_HZ;
/// Seconds the pedestrians alone walk before that while a scene loads, so people have
/// spread from where they appear (40–120 m out) toward the camera before the first frame.
pub const PEDS_PREROLL_S: f32 = 40.0;
const PEDS_PREROLL_STEPS: u32 = (PEDS_PREROLL_S as u32) * skywave_core::SIM_HZ;
/// ...at most this many steps a frame once the game is running: a second of sim, about
/// 10 ms of CPU, for 20 frames.
pub const WARMUP_BATCH: u32 = 120;
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
/// Marks a car slot in the draw order as its brake lamps' overlay.
const BRAKE_SLOT: u16 = 0x8000;

/// An NPC car's body as the player's car hits it: its shape's box (the body from nose to
/// tail and its widest point) and its car's mass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarBody {
    /// How far the box's center sits behind the car's ground point (the drawn mesh's
    /// origin, midway between the axles), m.
    pub back: f32,
    /// Half length and half width, m.
    pub half: [f32; 2],
    /// Roof height above the ground, m.
    pub height: f32,
    pub mass_kg: f32,
}

impl CarBody {
    /// The body of the built-in shape `shape`, with its car's mass.
    pub fn of(shape: &shape::ShapeSpec) -> Result<CarBody, String> {
        let spec = vehicles::builtin_spec(&shape.id)?;
        let len = shape.length();
        let mid = 0.5 * (shape.wheels.front_x + shape.wheels.rear_x);
        let half_w = shape.plan.iter().fold(0.0f32, |m, p| m.max(p.1));
        let height = shape.side.iter().fold(0.0f32, |m, p| m.max(p.1));
        Ok(CarBody {
            back: 0.5 * len - mid,
            half: [0.5 * len, half_w],
            height,
            mass_kg: spec.mass_kg,
        })
    }

    /// The box around car `c`, as the world collider takes it.
    pub fn solid(&self, c: &traffic::Car) -> world::collide::BoxSolid {
        world::collide::BoxSolid {
            c: [
                c.pos[0] - c.dir[0] * self.back,
                c.pos[2] - c.dir[1] * self.back,
            ],
            u: c.dir,
            half: self.half,
            y0: c.pos[1] + BODY_CLEARANCE_M,
            y1: c.pos[1] + self.height,
            kind: world::props::PropKind::ParkedCar { color: 0 },
        }
    }
}

/// Height of an NPC car's underside above the ground, m (the player's car's body box
/// starts as high).
const BODY_CLEARANCE_M: f32 = 0.25;

/// A car's pose at the start of the step, to interpolate from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Snap {
    id: u32,
    pos: [f32; 3],
    heading: f32,
}

/// The traffic of one place (see the module docs).
pub struct Npcs {
    pub(crate) graph: LaneGraph,
    /// The place's seed, which picks its parked-car paints.
    place_seed: u64,
    traffic: Traffic,
    /// A pool with no car, to start over from when traffic is switched off.
    empty: Traffic,
    signals: SignalClock,
    density: Density,
    prev: Vec<Snap>,
    /// Mesh ids, `(shape * PAINTS + paint) * 2 + lod - 1`, once found or built.
    ids: Vec<MeshId>,
    /// Brake lamp overlay mesh ids ([`car::brake_overlay_mesh`]), `shape * 2 + lod - 1`.
    brakes: Vec<MeshId>,
    /// Bytes of the meshes built for the traffic alone (not shared with parked cars).
    built_bytes: usize,
    /// Per shape, its head lamps' centers in the mesh's frame.
    heads: Vec<[Vec3; 2]>,
    /// Per shape, its body as the player's car hits it.
    bodies: Vec<CarBody>,
    /// Scratch for drawing in mesh order: mesh id and car slot.
    order: Vec<(u32, u16)>,
    /// Per-step sim time, ms, smoothed, for the overlay.
    step_ms: f32,
    /// Warm-up steps still to run, and pedestrian steps to run before them when the scene
    /// loads.
    warm: u32,
    preroll: u32,
    /// The car id each traffic voice follows ([`traffic::cars::Car::id`]), or `u32::MAX`.
    voice_ids: [u32; TRAFFIC_VOICES],
    /// The place's pedestrians, at the same density.
    pub(crate) crowd: peds::Crowd,
}

/// What a step needs from the game.
#[derive(Clone, Copy, Debug)]
pub struct StepInputs {
    pub player: Player,
    pub view: View,
    pub clock: Clock,
}

impl Npcs {
    /// The lane graph and an empty pool for `place`, seeded by `seed`, its cars picked by
    /// the world's era weights `era` (per built-in shape: [`crate::scenario::Lineup::shapes`];
    /// reduced, so a world whose cars all weigh alike picks as an unweighted pool).
    pub fn new(place: &Place, seed: Seed, era: &[u32]) -> Npcs {
        let graph = LaneGraph::new(place);
        let budget = TrafficBudget {
            shapes: vehicles::TRAFFIC_SHAPES as u8,
            ..TrafficBudget::default()
        };
        let mut traffic = Traffic::new(&graph, budget, seed);
        let mut weights = era.to_vec();
        crate::scenario::reduced(&mut weights);
        traffic.set_shape_weights(&weights);
        let cap = budget.cars;
        let crowd = peds::Crowd::new(place, &graph, seed);
        let bodies = vehicles::BUILTIN_SHAPES[..vehicles::TRAFFIC_SHAPES]
            .iter()
            .map(|(_, text)| vehicles::load_shape(text).and_then(|sh| CarBody::of(&sh)))
            .collect::<Result<_, _>>()
            .expect("the built-in traffic shapes load");
        Npcs {
            graph,
            place_seed: place.header.seed,
            empty: traffic.clone(),
            traffic,
            signals: SignalClock::default(),
            density: Density::Off,
            prev: Vec::with_capacity(cap),
            ids: Vec::new(),
            brakes: Vec::new(),
            built_bytes: 0,
            heads: Vec::new(),
            bodies,
            // A car and its brake lamps.
            order: Vec::with_capacity(2 * cap),
            step_ms: 0.0,
            warm: 0,
            preroll: 0,
            voice_ids: [u32::MAX; TRAFFIC_VOICES],
            crowd,
        }
    }

    /// Whether the cars' and the figures' meshes are built and registered.
    pub fn has_meshes(&self) -> bool {
        !self.ids.is_empty()
    }

    /// Finds the cars' meshes among `parked`'s and builds the rest, then builds the
    /// pedestrian figures', registering each with `register`, which returns its id.
    pub fn build_meshes(
        &mut self,
        parked: Option<&crate::propinst::PropInstances>,
        mut register: impl FnMut(Mesh) -> MeshId,
    ) -> Result<(), String> {
        let shapes: Vec<shape::ShapeSpec> = vehicles::BUILTIN_SHAPES[..vehicles::TRAFFIC_SHAPES]
            .iter()
            .map(|(_, text)| vehicles::load_shape(text))
            .collect::<Result<_, _>>()?;
        let mut ids = Vec::with_capacity(shapes.len() * PAINTS * 2);
        let mut heads = Vec::with_capacity(shapes.len());
        let mut brakes = Vec::with_capacity(shapes.len() * 2);
        let mut bytes = 0;
        let size = |m: &Mesh| {
            m.vertices.len() * core::mem::size_of::<render::Vertex>()
                + m.indices.len() * core::mem::size_of::<u32>()
        };
        for (k, sh) in shapes.iter().enumerate() {
            heads.push(car::parked_headlamps(sh)?);
            for lod in [1, 2] {
                let m = car::brake_overlay_mesh(sh, lod)?;
                bytes += size(&m);
                brakes.push(register(m));
            }
            let (_, era) = crate::propinst::place_paints(self.place_seed, k, sh.palette.len());
            for &e in &era {
                if let Some(pair) = parked.and_then(|p| p.car_ids(k, e)) {
                    ids.extend(pair);
                    continue;
                }
                for lod in [1, 2] {
                    let m = car::parked_mesh(sh, 0, Some(shape::ERA_1980S_PAINT[e].0), lod)?;
                    bytes += size(&m);
                    ids.push(register(m));
                }
            }
        }
        self.ids = ids;
        self.brakes = brakes;
        self.heads = heads;
        self.built_bytes = bytes;
        self.crowd.build_meshes(register);
        Ok(())
    }

    /// Bytes of mesh built for the traffic alone (the rest are the parked cars').
    pub fn built_bytes(&self) -> usize {
        self.built_bytes
    }

    /// Most instances a frame draws: every car the pool holds and its brake lamps, and
    /// every person's parts.
    pub fn capacity(&self) -> usize {
        2 * self.traffic.budget.cars + self.crowd.capacity()
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

    /// Keeps `density` of cars and people from the next step; off drops every car and
    /// person at once, and switching on again starts a warm-up.
    pub fn set_density(&mut self, density: Density) {
        self.crowd.set_density(density);
        if density == Density::Off && self.density != Density::Off {
            self.traffic = self.empty.clone();
            self.prev.clear();
            self.warm = 0;
            self.preroll = 0;
        } else if density != Density::Off && self.density == Density::Off {
            self.warm = WARMUP_STEPS;
            self.preroll = PEDS_PREROLL_STEPS;
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

    /// The body of car `c`, as the player's car hits it.
    pub fn body(&self, c: &traffic::Car) -> CarBody {
        self.bodies[usize::from(c.shape)]
    }

    /// The player's car (`state`, of `spec`) against every NPC car near it, after its step
    /// and before the traffic's: each contact pushes it out and exchanges an impulse
    /// (`world::collide::hit_car`), and an NPC car hit while closing is shoved and stands
    /// on its brakes ([`traffic::Traffic::shove`]). The hardest hit, if any car touched.
    pub fn collide(
        &mut self,
        state: &mut vehicle::VehicleState,
        spec: &vehicle::VehicleSpec,
        ground: &drive::DriveGround,
    ) -> Option<world::Bump> {
        let [len, wid, _] = spec.dims_m;
        let reach = 0.5 * (len + wid) + 4.0;
        let mut hardest: Option<world::Bump> = None;
        for i in 0..self.traffic.cars().len() {
            let c = self.traffic.cars()[i];
            if (c.pos[0] - state.pos[0]).abs() > reach || (c.pos[2] - state.pos[2]).abs() > reach {
                continue;
            }
            let b = self.bodies[usize::from(c.shape)];
            let other = world::collide::OtherCar {
                body: b.solid(&c),
                vel: [
                    c.dir[0] * c.speed + c.slide[0],
                    c.dir[1] * c.speed + c.slide[1],
                ],
                spin: c.spin,
                mass_kg: b.mass_kg,
            };
            let Some(hit) = world::collide::hit_car(state, spec, &other) else {
                continue;
            };
            if hit.bump.speed > 0.0 {
                // Measured once a hit, from where the car stands on its path; a car still
                // off it from the last hit keeps that hit's room.
                let fresh = c.shove == [0.0; 2] && c.yaw == 0.0 && c.slide == [0.0; 2];
                let room = if fresh {
                    road_room(ground, &c, &b)
                } else {
                    c.room
                };
                self.traffic.shove(i, hit.dv, hit.dspin, room);
            }
            if hardest.is_none_or(|h| hit.bump.speed > h.speed) {
                hardest = Some(hit.bump);
            }
        }
        hardest
    }

    /// One pedestrian step after [`Npcs::step`], at the signal tick it reached.
    pub fn step_peds(&mut self, inp: &StepInputs) {
        let tick = self.signals.tick;
        self.crowd.step(&self.graph, tick, inp.player, inp.view);
    }

    /// Runs up to `max` of the warm-up's steps (cars and people) still due, the player
    /// and the view held still. Returns how many ran.
    pub fn warm_up(&mut self, inp: &StepInputs, max: u32) -> u32 {
        self.preroll = 0;
        self.run_warm_up(inp, max, inp.view)
    }

    /// Runs the whole warm-up still due before anything is drawn (the scene loading):
    /// [`PEDS_PREROLL_S`] of pedestrians, then as [`Npcs::warm_up`]. Nobody is watching,
    /// so the people's view cone is narrowed to a line ahead of the camera (`View::sees`
    /// still covers a beam a person wide along it and a couple of meters round the eye)
    /// and they may appear and walk where the camera will look; the cars still keep out
    /// of its view. The pedestrians' first seconds run before the signal clock's tick 0:
    /// at ticks a whole number of every junction's cycle later, which show the same lights,
    /// so the signals and the cars are just as without them.
    pub fn warm_up_unseen(&mut self, inp: &StepInputs) -> u32 {
        let blind = StepInputs {
            view: View {
                tan_half: 0.0,
                ..inp.view
            },
            ..*inp
        };
        if self.preroll > 0 {
            let period = signal_period(&self.graph);
            let n = u64::from(self.preroll);
            let start = n.div_ceil(period) * period + self.signals.tick - n;
            for t in start..start + n {
                self.crowd.step(&self.graph, t, blind.player, blind.view);
            }
        }
        self.preroll = 0;
        self.run_warm_up(inp, u32::MAX, blind.view)
    }

    fn run_warm_up(&mut self, inp: &StepInputs, max: u32, peds_view: View) -> u32 {
        let n = self.warm.min(max);
        let peds = StepInputs {
            view: peds_view,
            ..*inp
        };
        for _ in 0..n {
            self.step(inp);
            self.step_peds(&peds);
        }
        self.warm -= n;
        n
    }

    /// Notes `ms` of traffic sim for `steps` steps, for the overlay.
    pub fn note_time(&mut self, ms: f64, steps: u32) {
        if steps > 0 && ms.is_finite() && ms >= 0.0 {
            let per = (ms / f64::from(steps)) as f32;
            self.step_ms += (per - self.step_ms) * 0.1;
        }
    }

    /// Fingerprint of the traffic and the pedestrians, for determinism checks.
    pub fn hash(&self) -> u64 {
        hash64(self.traffic.hash(), &[self.signals.tick, self.crowd.hash()])
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
        (usize::from(c.shape) * PAINTS + c.paint as usize % PAINTS) * 2 + usize::from(far)
    }

    /// Appends an instance for every car within [`DRAW_RADIUS_M`] of `eye`, `alpha` of the
    /// way through the step, in mesh order, lit by the frame's `lights` when close;
    /// lamps glow by `lamp_glow` on cars with their lights on, and a braking car adds its
    /// brake lamps' overlay glowing by `lamp_glow`, day or night. Returns how many.
    pub fn push(
        &mut self,
        alpha: f32,
        eye: Vec3,
        lights: &FrameLights,
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
                let far = d2 > near2;
                let mesh = self.ids[Self::mesh_index(c, far)];
                order.push((mesh.0, i as u16));
                if c.braking {
                    let brake = self.brakes[usize::from(c.shape) * 2 + usize::from(far)];
                    order.push((brake.0, i as u16 | BRAKE_SLOT));
                }
            }
        }
        order.sort_unstable();
        let lit2 = LIT_RADIUS_M * LIT_RADIUS_M;
        for &(mesh, slot) in &order {
            let i = slot & !BRAKE_SLOT;
            let c = &self.traffic.cars()[usize::from(i)];
            let (pos, heading) = self.pose(usize::from(i), alpha);
            let transform = Mat4::from_rotation_translation(Quat::from_rotation_y(heading), pos);
            let bounds = Bounds::cube(pos + Vec3::Y, CAR_HALF_M);
            let lights = if bounds.distance_squared(eye) < lit2 {
                lights.set_for(&bounds)
            } else {
                LightSet::EMPTY
            };
            let brake = slot & BRAKE_SLOT != 0;
            out.push(MeshInstance {
                lights,
                glow: if c.lights || brake { lamp_glow } else { 0.0 },
                casts_shadow: !brake,
                ..MeshInstance::new(MeshId(mesh), transform.to_cols_array_2d())
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
    /// the listener at `ear` facing `forward` and moving at `velocity` (m/s); the rest
    /// silent. A car keeps its voice while it stays among the nearest, so its pitch never
    /// jumps to another car's when two swap rank.
    pub fn voices(
        &mut self,
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
        // Each chosen car to the voice it had, the rest to voices freed this frame.
        let cars = self.traffic.cars();
        let chosen = &best[..n];
        let mut slot_of = [usize::MAX; TRAFFIC_VOICES];
        let mut taken = [false; TRAFFIC_VOICES];
        for (k, &(_, i)) in chosen.iter().enumerate() {
            if let Some(v) = self.voice_ids.iter().position(|&id| id == cars[i].id) {
                slot_of[k] = v;
                taken[v] = true;
            }
        }
        for slot in slot_of[..n].iter_mut().filter(|s| **s == usize::MAX) {
            let free = (0..TRAFFIC_VOICES).find(|&v| !taken[v]).unwrap_or(0);
            taken[free] = true;
            *slot = free;
        }
        let mut ids = [u32::MAX; TRAFFIC_VOICES];
        for (k, &(_, i)) in chosen.iter().enumerate() {
            ids[slot_of[k]] = cars[i].id;
        }
        self.voice_ids = ids;
        let right = Vec3::new(-forward.z, 0.0, forward.x).normalize_or_zero();
        let mut out = [TrafficVoice::default(); TRAFFIC_VOICES];
        for (k, &(_, i)) in chosen.iter().enumerate() {
            let v = &mut out[slot_of[k]];
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

/// Steps the room probe takes to the side, m, and how level with the road at the lane's
/// line a corner must be to be on the roadway, m (a curb is 0.15 m; crown and grade are
/// less).
const ROOM_STEP_M: f32 = 0.05;
const ROAD_LEVEL_M: f32 = 0.08;

/// How far car `c` (body `b`) may be shoved off its path to its right and left, m, and
/// turned, rad, with every corner of its body still on the roadway (asphalt, level with
/// the road beside its path): at most traffic's caps, nothing where even its unshoved body
/// is not (near a junction, where lanes and drawn roads part a little).
pub fn road_room(ground: &drive::DriveGround, c: &traffic::Car, b: &CarBody) -> [f32; 3] {
    use traffic::cars::{SHOVE_SIDE_M, SHOVE_YAW};
    // Its place on the path: the drawn car less the shove and the turn.
    let (sn, cs) = traffic::math::sin_cos(c.yaw);
    let d = [c.dir[0] * cs - c.dir[1] * sn, c.dir[1] * cs + c.dir[0] * sn];
    let side = [-d[1], d[0]];
    let at = [c.pos[0] - c.shove[0], c.pos[2] - c.shove[1]];
    let y = c.pos[1] + 0.5;
    let on_road = |off: f32, yaw: f32| {
        let (sn, cs) = traffic::math::sin_cos(yaw);
        let f = [d[0] * cs + d[1] * sn, d[1] * cs - d[0] * sn];
        let r = [-f[1], f[0]];
        let mid = [
            at[0] + side[0] * off - f[0] * b.back,
            at[1] + side[1] * off - f[1] * b.back,
        ];
        [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)]
            .iter()
            .all(|&(u, v)| {
                let q = [
                    mid[0] + f[0] * b.half[0] * u + r[0] * b.half[1] * v,
                    mid[1] + f[1] * b.half[0] * u + r[1] * b.half[1] * v,
                ];
                let (yq, surface) = ground.level_from(q, y);
                let along = (q[0] - at[0]) * d[0] + (q[1] - at[1]) * d[1];
                let (yl, _) = ground.level_from([at[0] + d[0] * along, at[1] + d[1] * along], y);
                surface == world::collide::Surface::Asphalt && (yq - yl).abs() < ROAD_LEVEL_M
            })
    };
    let Some(yaw) = [SHOVE_YAW, 0.5 * SHOVE_YAW, 0.0]
        .into_iter()
        .find(|&t| on_road(0.0, t) && on_road(0.0, -t))
    else {
        return [0.0; 3];
    };
    let mut room = [0.0f32; 2];
    for (k, sign) in [1.0f32, -1.0].into_iter().enumerate() {
        for n in 1..=(SHOVE_SIDE_M / ROOM_STEP_M).round() as u32 {
            let off = (n as f32 * ROOM_STEP_M).min(SHOVE_SIDE_M);
            if !(on_road(sign * off, yaw) && on_road(sign * off, -yaw)) {
                break;
            }
            room[k] = off;
        }
    }
    [room[0], room[1], yaw]
}

/// Ticks after which every signal of `graph` shows what it showed: the least common
/// multiple of its junctions' cycles.
fn signal_period(graph: &LaneGraph) -> u64 {
    fn gcd(a: u64, b: u64) -> u64 {
        if b == 0 {
            a
        } else {
            gcd(b, a % b)
        }
    }
    graph
        .junctions
        .iter()
        .filter(|j| j.phases > 0)
        .map(|j| u64::from(traffic::signal::cycle_ticks(j.phases)))
        .fold(1, |l, c| {
            (l / gcd(l, c))
                .checked_mul(c)
                .expect("signal cycles have a common period")
        })
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
mod gate;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{App, InputState, SceneSetup};

    /// The coast junction of the `coast-traffic-noon` golden, waiting at the stop line.
    pub(crate) const JUNCTION: &str =
        "place=coast&car=gannet_gt&cam=chase-mid&pos=477.5,1001.2&yaw=347";

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

    /// `(cars of shape `shape`, cars)` over two minutes of busy traffic in the test town
    /// with the era weights `era` (per shape), counted every step, and the pool's hash at
    /// the end.
    fn census(era: &[u32], shape: usize) -> (usize, usize, u64) {
        let (per, all, hash) = census_all(era);
        (per[shape], all, hash)
    }

    /// Cars of each shape, cars, and the hash, as [`census`].
    fn census_all(era: &[u32]) -> (Vec<usize>, usize, u64) {
        let app = app("place=town&t=12:00&still=1&stream=0");
        let place = place::Place::test_grid();
        let mut npcs = Npcs::new(&place, Seed(1), era);
        npcs.set_density(Density::Busy);
        let inputs = app.traffic_inputs();
        npcs.warm_up_unseen(&inputs);
        let (mut per, mut all) = (vec![0; vehicles::BUILTIN_SHAPES.len()], 0);
        for _ in 0..120 * skywave_core::SIM_HZ {
            npcs.step(&inputs);
            all += npcs.cars().len();
            for c in npcs.cars() {
                per[usize::from(c.shape)] += 1;
            }
        }
        (per, all, npcs.traffic.hash())
    }

    #[test]
    fn a_world_cut_inside_the_roster_drives_only_its_cars() {
        // Years (1979, 1983): every traffic shape of an allowed car drives, every shape of
        // a newer car never does.
        let l = crate::scenario::lineup_with(0, Some((1979, 1983)), |_| None);
        let (per, all, _) = census_all(&l.shapes);
        for (k, (file, _)) in vehicles::BUILTIN_SHAPES.iter().enumerate() {
            let drives = k < vehicles::TRAFFIC_SHAPES && l.shapes[k] > 0;
            assert_eq!(per[k] > 0, drives, "{file}: {} of {all}", per[k]);
        }
    }

    #[test]
    fn the_1984_world_drives_as_before_and_never_a_2001_car() {
        let k = vehicles::BUILTIN_SHAPES
            .iter()
            .position(|b| b.0 == "halcyon_turbo.shape.ron")
            .unwrap();
        // The 1984 world's weights are all alike: the same traffic as an unweighted pool.
        let (n84, all, hash84) = census(&crate::scenario::lineup(0).shapes, k);
        let (n, _, unweighted) = census(&[], k);
        assert_eq!((hash84, n84), (unweighted, n));
        assert!(n84 > 0 && all > 1000);
        // The Halcyon dated 2001 never drives in 1984, and does in 2001.
        let (n, all, _) = census(
            &crate::scenario::lineup_with_2001_car(0, "halcyon_turbo").shapes,
            k,
        );
        assert_eq!(n, 0, "of {all}");
        let (n, _, _) = census(
            &crate::scenario::lineup_with_2001_car(1, "halcyon_turbo").shapes,
            k,
        );
        assert!(n > 0);
    }

    #[test]
    fn densities_have_stable_names() {
        for d in DENSITIES {
            assert_eq!(density_from_name(density_name(d)), Some(d));
        }
        assert_eq!(density_from_name("jam"), None);
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
            // Warmed up while the scene loaded, before the first frame.
            assert_eq!(app.npcs.warm, 0);
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

    /// At noon the cars braking or queued at the junction add their brake lamps' overlay,
    /// one per braking car in reach, glowing (lights off or not) and casting no shadow;
    /// cruising cars add none.
    #[test]
    fn braking_cars_light_their_brake_lamps_by_day() {
        let mut app = app(&format!("{JUNCTION}&t=12:00&traffic=busy&still=1&stream=1"));
        app.update(&InputState::default(), 0.0);
        let eye = app.camera.pose(&app.drive, 1.0).eye;
        let braking = app
            .npcs
            .cars()
            .iter()
            .filter(|c| c.braking && Vec3::from(c.pos).distance(eye) <= DRAW_RADIUS_M)
            .count();
        assert!(braking > 0, "someone waits at the light");
        let brakes = app.npcs.brakes.clone();
        // Every traffic shape has its overlay at both levels, and it has lenses.
        assert_eq!(brakes.len(), crate::vehicles::TRAFFIC_SHAPES * 2);
        for (k, (_, text)) in crate::vehicles::BUILTIN_SHAPES[..crate::vehicles::TRAFFIC_SHAPES]
            .iter()
            .enumerate()
        {
            let sh = crate::vehicles::load_shape(text).unwrap();
            for lod in [1, 2] {
                let m = car::brake_overlay_mesh(&sh, lod).unwrap();
                assert!(
                    !m.indices.is_empty(),
                    "shape {k} level {lod} has no tail lenses"
                );
            }
        }
        let view = app.scene_view(1.0, 16.0 / 9.0);
        let overlays: Vec<_> = view
            .instances
            .iter()
            .filter(|i| brakes.contains(&i.mesh))
            .collect();
        assert_eq!(overlays.len(), braking);
        assert!(overlays.iter().all(|i| i.glow > 10.0 && !i.casts_shadow));
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
        let ear = at - right * 10.0;
        let side = app.npcs.voices(1.0, ear, fwd, Vec3::ZERO);
        let slot = app
            .npcs
            .voice_ids
            .iter()
            .position(|&id| id == c.id)
            .unwrap();
        assert!(side[slot].pan > 0.5, "{:?}", side[slot]);
        // Each car keeps its voice from frame to frame while the cars go by.
        let inputs = app.traffic_inputs();
        for _ in 0..240 {
            let before = app.npcs.voice_ids;
            app.npcs.step(&inputs);
            app.npcs.voices(1.0, ear, fwd, Vec3::ZERO);
            for (v, id) in app.npcs.voice_ids.iter().enumerate() {
                let was = before.iter().position(|b| b == id);
                assert!(
                    *id == u32::MAX || was.is_none() || was == Some(v),
                    "{id} moved"
                );
            }
        }
    }

    /// Switched on from Settings mid-game, the warm-up runs a batch a frame, never more.
    #[test]
    fn traffic_switched_on_in_game_warms_up_a_batch_a_frame() {
        let mut app = app("place=town&t=12:00&stream=0");
        let mut mapper = crate::InputMapper::default();
        app.set_settings_store(Box::new(crate::MemoryStore::default()));
        app.ui.settings_mut().traffic = Density::Off;
        app.frame(&mut mapper, 1.0 / 60.0, &|| 0.0);
        assert_eq!(app.traffic_count(), 0);
        app.ui.settings_mut().traffic = Density::Busy;
        let mut frames = 0;
        let mut left = WARMUP_STEPS;
        loop {
            let ticks = app.npcs.signals.tick;
            app.frame(&mut mapper, 1.0 / 60.0, &|| 0.0);
            let stepped = app.npcs.signals.tick - ticks;
            assert!(
                stepped <= u64::from(WARMUP_BATCH) + 4,
                "{stepped} steps in a frame"
            );
            frames += 1;
            if app.npcs.warm == 0 {
                break;
            }
            assert_eq!(app.npcs.warm, left - WARMUP_BATCH);
            left = app.npcs.warm;
        }
        assert_eq!(frames, WARMUP_STEPS.div_ceil(WARMUP_BATCH));
        assert!(app.traffic_count() > 20, "{}", app.traffic_count());
    }

    /// The in-game traffic step (step and populate, busy cars on the coast) within its
    /// budget, gated like the traffic crate's (#17): the best of three windows' median
    /// under 0.5 ms and p99 under 2 ms in this build, asserted only under
    /// `SKYWAVE_ENFORCE_BUDGETS`. The median is what the sim costs when the machine is not
    /// busy; a shared runner stealing slices moves the tail, not it.
    #[test]
    fn the_traffic_step_is_within_budget() {
        let mut app = app("place=coast&t=12:00&traffic=busy&still=1");
        let inputs = app.traffic_inputs();
        let mut windows = Vec::new();
        let mut times = Vec::with_capacity(600);
        for _ in 0..3 {
            times.clear();
            for _ in 0..600 {
                let start = std::time::Instant::now();
                app.npcs.step(&inputs);
                times.push(start.elapsed().as_secs_f64() * 1e3);
            }
            times.sort_by(f64::total_cmp);
            windows.push((times[times.len() / 2], times[times.len() * 99 / 100]));
        }
        let median = windows.iter().map(|w| w.0).fold(f64::INFINITY, f64::min);
        let p99 = windows.iter().map(|w| w.1).fold(f64::INFINITY, f64::min);
        eprintln!(
            "traffic step: {} cars, median {median:.3} ms, p99 {p99:.3} ms (windows {windows:.3?})",
            app.traffic_count()
        );
        assert!(app.traffic_count() >= 60, "{}", app.traffic_count());
        if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
            assert!(median < 0.5, "median {median:.3} ms per step");
            assert!(p99 < 2.0, "p99 {p99:.3} ms per step");
        }
    }

    /// At a busy junction at night the NPC beams never crowd out the player's headlights
    /// or a street lamp: in every light set, a beam is there only when every other light
    /// reaching the instance is there too.
    #[test]
    fn npc_beams_only_take_slots_the_other_lights_leave() {
        let mut app = app(&format!("{JUNCTION}&t=22:30&lights=1&traffic=busy&still=1"));
        let world = app.world_instances;
        let eye = app.camera.pose(&app.drive, 1.0).eye;
        let mut scratch = [PointLight::omni([0.0; 3], [0.0; 3], 0.0); NPC_LIGHTS];
        let m = app.npcs.lights(1.0, eye, &mut scratch);
        assert!(m >= 4, "{m} beams at night");
        // The player's four lamps, then the beams.
        let beams = 4..4 + m;
        let view = app.scene_view(1.0, 16.0 / 9.0);
        assert!(view.lights[beams.clone()].iter().all(|l| l.radius == 60.0));
        // The player's car is lit by its own beams first.
        for inst in &view.instances[world..world + 4] {
            let set = inst.lights.indices();
            assert!(set.contains(&0) && set.contains(&1), "{:?}", inst.lights);
        }
        // The invariant, against the ranking rule itself.
        let bounds: Vec<_> = app.bounds[..world].iter().flatten().copied().collect();
        for b in &bounds {
            let set = app.lights.set_for(b);
            let has_beam = set
                .indices()
                .iter()
                .any(|&i| beams.contains(&usize::from(i)));
            if has_beam {
                let reaching = app
                    .lights
                    .lights()
                    .iter()
                    .enumerate()
                    .filter(|(i, l)| !beams.contains(i) && b.reached_by(l));
                for (i, _) in reaching {
                    assert!(set.indices().contains(&(i as u8)), "light {i} crowded out");
                }
            }
        }
    }
}
