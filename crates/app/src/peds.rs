//! Pedestrians in the game (`docs/NPCS.md`, "In the game"): the sidewalk graph and a
//! [`traffic::peds::Pedestrians`] for the place, stepped with the 120 Hz sim beside the
//! traffic and drawn as instanced figure parts. Owned by [`crate::npc::Npcs`], which feeds
//! it the signal tick and the lane graph and gives it the traffic's density setting.
//!
//! - **Setup.** [`Crowd::new`] builds the sidewalk graph and the pool
//!   ([`PedestrianBudget::SMALL`]) at scene setup. The first time the density is on,
//!   [`Crowd::build_meshes`] builds [`VARIANTS`] figures of the place's era (every outfit
//!   among them), 13 part meshes each, and keeps them; every person is drawn as the
//!   variant its figure seed picks ([`variant_of`]), scaled to the height the sim gave it
//!   so its stride matches its pace.
//! - **Step.** Each sim step after the traffic's: the signal tick, the density, the
//!   camera's view cone and the player's car, then `step`. Every slot's pose before the
//!   step is kept, so drawing interpolates by person id.
//! - **Draw.** One instance per part of every live person within [`DRAW_RADIUS_M`] of the
//!   eye, posed by the figure's pose functions from the sim's phase and blend weights,
//!   ordered by mesh so each part of a variant is one instanced draw; within
//!   [`SHADOW_RADIUS_M`] they cast shadows. Pedestrians add no lights; the scene's lights
//!   light them.
//!
//! Every per-frame path writes into buffers sized at setup.

use glam::{Mat3, Mat4, Quat, Vec3};
use place::Place;
use render::{LightSet, Mesh, MeshId, MeshInstance};
use skywave_core::{hash64, Seed};
use traffic::figure::{figure, FigureSpec, Outfit, Skeleton, Xform, PARTS};
use traffic::peds::{PedInput, PedPose, PedestrianBudget, Pedestrians};
use traffic::walks::WalkGraph;
use traffic::{Density, LaneGraph, Player, View};

use crate::car;
use crate::lighting::{Bounds, FrameLights};

/// Figures built per place: every person is drawn as one of them.
pub const VARIANTS: usize = 8;
/// People are drawn this far from the eye, m (as far as they live out of view).
pub const DRAW_RADIUS_M: f32 = traffic::peds::DESPAWN_M;
/// People nearer than this cast shadows, m: farther, a figure's shadow is a few texels
/// of the far cascade, not worth its draws.
pub const SHADOW_RADIUS_M: f32 = 40.0;
/// People nearer than this are lit by the frame's lights, m.
const LIT_RADIUS_M: f32 = crate::propinst::LIT_RADIUS_M;
/// How far a figure's parts reach from where it stands, m (a hand on a 2.1 m person).
pub const REACH_M: f32 = 1.0;
/// Light-culling half size round a person and the height of its middle, m.
const FIGURE_HALF_M: f32 = 1.0;
/// The outfits the variants wear in turn.
const OUTFITS: [Outfit; 5] = [
    Outfit::ShortsTee,
    Outfit::JeansJacket,
    Outfit::Sundress,
    Outfit::Suit,
    Outfit::Beach,
];
/// Hash key of the variant seeds.
const KEY_VARIANT: u64 = 0x4649_4755_5245_0001;

/// The variant person `seed` is drawn as.
pub fn variant_of(seed: u64) -> usize {
    (seed % VARIANTS as u64) as usize
}

/// A slot's person at the start of the step, to interpolate from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Snap {
    alive: bool,
    id: u32,
    pos: [f32; 3],
    heading: f32,
    pose: PedPose,
}

/// One built figure: its skeleton, height and part meshes.
struct Variant {
    skeleton: Skeleton,
    height_m: f32,
    ids: [MeshId; PARTS],
}

/// The pedestrians of one place (see the module docs).
pub struct Crowd {
    walks: WalkGraph,
    peds: Pedestrians,
    /// A pool with nobody in it, to start over from when the density is switched off.
    empty: Pedestrians,
    place_seed: u64,
    density: Density,
    prev: Vec<Snap>,
    /// Per slot: the person id its scale is for, and the scale (sim height over the
    /// variant's).
    scale: Vec<(u32, f32)>,
    variants: Vec<Variant>,
    built_bytes: usize,
    /// Scratch for drawing in mesh order: variant and slot.
    order: Vec<(u16, u16)>,
    /// Per-step sim time, ms, smoothed, for the overlay.
    step_ms: f32,
}

impl Crowd {
    /// The sidewalk graph of `place` over its lane graph `lanes`, and an empty pool seeded
    /// by `seed`.
    pub fn new(place: &Place, lanes: &LaneGraph, seed: Seed) -> Crowd {
        let walks = WalkGraph::new(place, lanes);
        let budget = PedestrianBudget::SMALL;
        let peds = Pedestrians::new(&walks, seed, place.header.era, budget);
        let cap = budget.pedestrians;
        let rest = Snap {
            alive: false,
            id: 0,
            pos: [0.0; 3],
            heading: 0.0,
            pose: peds.pedestrians()[0].pose,
        };
        Crowd {
            walks,
            empty: peds.clone(),
            peds,
            place_seed: place.header.seed,
            density: Density::Off,
            prev: vec![rest; cap],
            scale: vec![(u32::MAX, 1.0); cap],
            variants: Vec::new(),
            built_bytes: 0,
            order: Vec::with_capacity(cap),
            step_ms: 0.0,
        }
    }

    /// Builds the [`VARIANTS`] figures' part meshes, registering each with `register`.
    pub fn build_meshes(&mut self, mut register: impl FnMut(Mesh) -> MeshId) {
        let era = self.peds.era();
        let mut bytes = 0;
        self.variants = (0..VARIANTS)
            .map(|k| {
                // Each outfit in turn, so few figures still dress the crowd every way: the
                // first of the place's seeds for this variant that wears it.
                let want = OUTFITS[k % OUTFITS.len()];
                let seed_at = |t: u64| hash64(self.place_seed, &[KEY_VARIANT, k as u64, t]);
                let seed = (0..64)
                    .map(seed_at)
                    .find(|&s| FigureSpec::from_seed(s, era).outfit == want)
                    .unwrap_or(seed_at(0));
                let f = figure(seed, era);
                let ids = core::array::from_fn(|p| {
                    let m = car::plain_mesh(&f.parts[p]);
                    bytes += m.vertices.len() * core::mem::size_of::<render::Vertex>()
                        + m.indices.len() * core::mem::size_of::<u32>();
                    register(m)
                });
                Variant {
                    skeleton: f.skeleton,
                    height_m: f.spec.height_m,
                    ids,
                }
            })
            .collect();
        self.built_bytes = bytes;
    }

    /// Most instances a frame draws: a part for every person the pool holds.
    pub fn capacity(&self) -> usize {
        self.prev.len() * PARTS
    }

    /// Live people.
    pub fn count(&self) -> usize {
        self.peds.count()
    }

    /// The pedestrian sim's time per step, ms (smoothed).
    pub fn step_ms(&self) -> f32 {
        self.step_ms
    }

    /// Keeps `density` of people from the next step; off sends everyone home at once.
    pub fn set_density(&mut self, density: Density) {
        if density == Density::Off && self.density != Density::Off {
            self.peds = self.empty.clone();
            for p in &mut self.prev {
                p.alive = false;
            }
            // Ids count up again from the empty pool's: a new person may reuse an old id.
            self.scale.fill((u32::MAX, 1.0));
        }
        self.density = density;
    }

    /// Whether the crowd runs: on, or people still to clear.
    pub fn active(&self) -> bool {
        self.density != Density::Off || self.count() > 0
    }

    /// One sim step at signal `tick` over `lanes`, round the `player`'s car seen from
    /// `view` (nothing when not [`Crowd::active`]).
    pub fn step(&mut self, lanes: &LaneGraph, tick: u64, player: Player, view: View) {
        if !self.active() {
            return;
        }
        for (s, p) in self.prev.iter_mut().zip(self.peds.pedestrians()) {
            *s = Snap {
                alive: p.alive,
                id: p.id,
                pos: p.pos,
                heading: p.heading,
                pose: p.pose,
            };
        }
        let input = PedInput {
            tick,
            density: self.density,
            view,
            player,
        };
        self.peds.step(&self.walks, lanes, &input);
    }

    /// Notes `ms` of pedestrian sim for `steps` steps, for the overlay.
    pub fn note_time(&mut self, ms: f64, steps: u32) {
        if steps > 0 && ms.is_finite() && ms >= 0.0 {
            let per = (ms / f64::from(steps)) as f32;
            self.step_ms += (per - self.step_ms) * 0.1;
        }
    }

    /// Fingerprint of the people, for determinism checks.
    pub fn hash(&self) -> u64 {
        self.peds.hash()
    }

    /// Slot `i`'s person `alpha` of the way through the last step: ground point, heading
    /// and pose.
    fn pose(&self, i: usize, alpha: f32) -> (Vec3, f32, PedPose) {
        let p = &self.peds.pedestrians()[i];
        let now = (Vec3::from(p.pos), p.heading, p.pose);
        let s = &self.prev[i];
        let (from, to) = (Vec3::from(s.pos), now.0);
        // A new person, a new action (sitting moves the origin to the seat) or a jump.
        if !s.alive || s.id != p.id || s.pose.action != p.pose.action || from.distance(to) > 1.0 {
            return now;
        }
        let wrap = |d: f32| d - d.round();
        let turn = (p.heading - s.heading + core::f32::consts::PI)
            .rem_euclid(core::f32::consts::TAU)
            - core::f32::consts::PI;
        let lerp = |a: f32, b: f32| a + (b - a) * alpha;
        let pose = PedPose {
            phase: s.pose.phase + wrap(p.pose.phase - s.pose.phase) * alpha,
            speed: lerp(s.pose.speed, p.pose.speed),
            t: lerp(s.pose.t, p.pose.t),
            look: lerp(s.pose.look, p.pose.look),
            weight: lerp(s.pose.weight, p.pose.weight),
            ..p.pose
        };
        (from.lerp(to, alpha), s.heading + turn * alpha, pose)
    }

    /// Slot `i`'s figure scale: its height in the sim over its variant's.
    fn scale(&mut self, i: usize) -> f32 {
        let p = &self.peds.pedestrians()[i];
        if self.scale[i].0 != p.id {
            let v = &self.variants[variant_of(p.seed)];
            let h = FigureSpec::from_seed(p.seed, self.peds.era()).height_m;
            // As the sim clamps it.
            self.scale[i] = (p.id, h.clamp(1.4, 2.1) / v.height_m);
        }
        self.scale[i].1
    }

    /// Slot `i`'s person `alpha` of the way through the step: each part's model-to-world
    /// transform into `out`; returns the figure's ground point.
    fn parts(&mut self, i: usize, alpha: f32, out: &mut [Mat4; PARTS]) -> Vec3 {
        let (pos, heading, pose) = self.pose(i, alpha);
        let scale = self.scale(i);
        let variant = &self.variants[variant_of(self.peds.pedestrians()[i].seed)];
        let mut xf = [Xform::IDENTITY; PARTS];
        variant
            .skeleton
            .pose(&pose.joints(&variant.skeleton), &mut xf);
        let root = Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_y(heading),
            pos,
        );
        for (m, x) in out.iter_mut().zip(&xf) {
            *m = root * part_matrix(x);
        }
        pos
    }

    /// Appends one instance per part of every live person within [`DRAW_RADIUS_M`] of
    /// `eye` and not within `clear_m` of it (whole figures, never some parts),
    /// `alpha` of the way through the step, in mesh order, lit by the frame's `lights`
    /// when close. Returns how many instances.
    pub fn push(
        &mut self,
        alpha: f32,
        eye: Vec3,
        clear_m: f32,
        lights: &FrameLights,
        out: &mut Vec<MeshInstance>,
    ) -> usize {
        if self.variants.is_empty() {
            return 0;
        }
        let (draw2, clear2) = (DRAW_RADIUS_M * DRAW_RADIUS_M, clear_m * clear_m);
        let mut order = core::mem::take(&mut self.order);
        order.clear();
        for (i, p) in self.peds.pedestrians().iter().enumerate() {
            let d2 = Vec3::from(p.pos).distance_squared(eye);
            if p.alive && d2 <= draw2 && d2 >= clear2 {
                order.push((variant_of(p.seed) as u16, i as u16));
            }
        }
        order.sort_unstable();
        let lit2 = LIT_RADIUS_M * LIT_RADIUS_M;
        let shadow2 = SHADOW_RADIUS_M * SHADOW_RADIUS_M;
        let start = out.len();
        let mut parts = [Mat4::IDENTITY; PARTS];
        let mut g = 0;
        while g < order.len() {
            let v = usize::from(order[g].0);
            let n = order[g..]
                .iter()
                .take_while(|o| usize::from(o.0) == v)
                .count();
            // Part-major within the variant: each part mesh's instances in one run.
            let base = out.len();
            let blank =
                MeshInstance::new(self.variants[v].ids[0], Mat4::IDENTITY.to_cols_array_2d());
            out.resize(base + n * PARTS, blank);
            for j in 0..n {
                let i = usize::from(order[g + j].1);
                let pos = self.parts(i, alpha, &mut parts);
                let bounds = Bounds::cube(pos + Vec3::Y * FIGURE_HALF_M, FIGURE_HALF_M);
                let d2 = bounds.distance_squared(eye);
                let set = if d2 < lit2 {
                    lights.set_for(&bounds)
                } else {
                    LightSet::EMPTY
                };
                for (k, m) in parts.iter().enumerate() {
                    out[base + k * n + j] = MeshInstance {
                        lights: set,
                        casts_shadow: d2 < shadow2,
                        ..MeshInstance::new(self.variants[v].ids[k], m.to_cols_array_2d())
                    };
                }
            }
            g += n;
        }
        self.order = order;
        out.len() - start
    }
}

/// A part's transform in the figure frame as a matrix.
fn part_matrix(x: &Xform) -> Mat4 {
    // `Xform::rot` is rows; glam takes columns.
    let r = Mat3::from_cols_array_2d(&x.rot).transpose();
    Mat4::from_cols(
        r.x_axis.extend(0.0),
        r.y_axis.extend(0.0),
        r.z_axis.extend(0.0),
        Vec3::from(x.pos).extend(1.0),
    )
}

/// What the tests look at.
#[cfg(test)]
impl Crowd {
    /// Bytes of the figures' meshes.
    pub fn built_bytes(&self) -> usize {
        self.built_bytes
    }

    /// The live people.
    pub fn people(&self) -> impl Iterator<Item = &traffic::peds::Pedestrian> {
        self.peds.iter()
    }

    /// The sidewalk graph.
    pub fn walks(&self) -> &WalkGraph {
        &self.walks
    }

    pub fn density(&self) -> Density {
        self.density
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npc::DENSITIES;
    use crate::{App, InputState, SceneSetup};
    use traffic::peds::Action;

    /// The busiest block of the strip on the coast at seed 1 (the walks with the most
    /// people per 100 m round it), the car on the road facing up it.
    pub(crate) const STRIP: &str =
        "place=coast&car=corsair_gtx&cam=chase-mid&pos=872.0,942.5&yaw=1";

    fn app(query: &str) -> App {
        App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap()
    }

    /// Every figure part mesh id.
    fn ids(app: &App) -> Vec<MeshId> {
        app.npcs.crowd.variants.iter().flat_map(|v| v.ids).collect()
    }

    /// The instances of `app`'s view drawing a pedestrian's part.
    fn part_instances(app: &mut App, alpha: f32) -> usize {
        let ids = ids(app);
        let view = app.scene_view(alpha, 16.0 / 9.0);
        view.instances
            .iter()
            .filter(|i| ids.contains(&i.mesh))
            .count()
    }

    /// Live people within [`DRAW_RADIUS_M`] of the eye.
    fn in_reach(app: &App) -> usize {
        let eye = app.camera.pose(&app.drive, 1.0).eye;
        app.npcs
            .crowd
            .people()
            .filter(|p| Vec3::from(p.pos).distance(eye) <= DRAW_RADIUS_M)
            .count()
    }

    #[test]
    fn scene_setup_builds_pedestrians_for_the_coast() {
        let mut app = app(&format!("{STRIP}&t=12:00&traffic=busy&still=1"));
        let crowd = &app.npcs.crowd;
        assert_eq!(crowd.density(), Density::Busy);
        assert!(crowd.walks().links.len() > 10_000, "a sidewalk graph");
        assert_eq!(crowd.variants.len(), VARIANTS, "figures built");
        // Warmed up with the traffic while the scene loaded.
        let n = app.pedestrian_count();
        eprintln!("{n} people after the warm-up");
        assert!(n >= 80, "{n} people");
        assert_eq!(part_instances(&mut app, 1.0), in_reach(&app) * PARTS);
        // A vehicle switch keeps them.
        let hash = app.npcs.crowd.hash();
        app.switch_vehicle("palisade_wagon").unwrap();
        assert_eq!(app.npcs.crowd.hash(), hash);
        assert_eq!(app.pedestrian_count(), n);
    }

    /// Settings › World › Traffic drives the pedestrians too, and round-trips through the
    /// settings file.
    #[test]
    fn the_traffic_setting_sets_the_pedestrian_density() {
        let mut app = app("place=town&t=12:00&stream=0");
        let mut mapper = crate::InputMapper::default();
        app.set_settings_store(Box::new(crate::MemoryStore::default()));
        for d in DENSITIES {
            app.ui.settings_mut().traffic = d;
            let json = app
                .ui
                .settings()
                .to_json(&crate::input::Bindings::default());
            let (back, _) = crate::settings::Settings::from_json(&json);
            assert_eq!(back.traffic, d);
            app.frame(&mut mapper, 1.0 / 60.0, &|| 0.0);
            assert_eq!(app.npcs.crowd.density(), d);
            assert_eq!(app.npcs.density(), d);
        }
    }

    /// Every live person in reach is drawn as all of their parts, in mesh order; off draws
    /// nobody, sends everyone home and idles the sim.
    #[test]
    fn instances_are_every_part_of_every_person_and_off_draws_none() {
        let mut app = app(&format!("{STRIP}&t=12:00&traffic=busy&still=1"));
        let inputs = app.traffic_inputs();
        let caps = (app.instances.capacity(), app.npcs.crowd.order.capacity());
        for step in 0..240 {
            app.npcs.step(&inputs);
            app.npcs.step_peds(&inputs);
            let world = app.instances.len();
            let drawn = part_instances(&mut app, (step % 4) as f32 / 4.0);
            assert_eq!(drawn, in_reach(&app) * PARTS, "step {step}");
            let ids = ids(&app);
            let meshes: Vec<MeshId> = app.instances[world.min(app.instances.len())..]
                .iter()
                .filter(|i| ids.contains(&i.mesh))
                .map(|i| i.mesh)
                .collect();
            assert!(meshes.windows(2).all(|w| w[0].0 <= w[1].0), "mesh order");
        }
        assert!(app.pedestrian_count() > 50);
        assert_eq!(
            (app.instances.capacity(), app.npcs.crowd.order.capacity()),
            caps,
            "no allocation per frame or step"
        );
        app.npcs.set_density(Density::Off);
        assert_eq!(app.pedestrian_count(), 0);
        assert_eq!(part_instances(&mut app, 1.0), 0, "off draws nothing");
        assert!(!app.npcs.crowd.active(), "and the sim is idle");
        let hash = app.npcs.crowd.hash();
        app.npcs.step_peds(&inputs);
        assert_eq!(app.npcs.crowd.hash(), hash);
        // On again: the people who reuse old ids are drawn at their own heights.
        app.npcs.set_density(Density::Busy);
        app.npcs.warm_up_unseen(&inputs);
        part_instances(&mut app, 1.0);
        let era = app.npcs.crowd.peds.era();
        let mut checked = 0;
        for (i, p) in app.npcs.crowd.peds.pedestrians().iter().enumerate() {
            // Everyone drawn has a scale under their id (the rest are out of reach).
            if p.alive && app.npcs.crowd.scale[i].0 == p.id {
                checked += 1;
                let v = app.npcs.crowd.variants[variant_of(p.seed)].height_m;
                let h = FigureSpec::from_seed(p.seed, era).height_m.clamp(1.4, 2.1);
                let drawn = app.npcs.crowd.scale[i].1 * v;
                assert!(
                    (drawn - h).abs() < 1e-4,
                    "person {} drawn {drawn} m, is {h} m",
                    p.id
                );
            }
        }
        assert!(checked > 50, "{checked} people drawn");
    }

    /// How far `p` is inside the car's footprint, m (negative outside).
    fn depth(car: &Player, p: Vec3) -> f32 {
        let (s, c) = car.heading.sin_cos();
        let f = [-s, -c];
        let d = [p.x - car.pos[0], p.z - car.pos[2]];
        let along = d[0] * f[0] + d[1] * f[1];
        let across = d[0] * -f[1] + d[1] * f[0];
        (car.half_len - along.abs()).min(car.half_w - across.abs())
    }

    /// Every part of slot `i`'s figure is within its bounds: no exploded figures.
    fn assert_attached(app: &mut App, i: usize, alpha: f32) -> [Mat4; PARTS] {
        let mut parts = [Mat4::IDENTITY; PARTS];
        let at = app.npcs.crowd.parts(i, alpha, &mut parts);
        let scale = app.npcs.crowd.scale(i);
        for (k, m) in parts.iter().enumerate() {
            let p = m.w_axis.truncate() - at;
            let flat = p.with_y(0.0).length();
            assert!(p.is_finite(), "person {i} part {k}: {p}");
            assert!(
                flat < 0.75 * scale && p.y > -0.05 && p.y < 2.0 * scale,
                "person {i} part {k} at {p} off its figure"
            );
        }
        parts
    }

    #[test]
    fn driving_through_a_crowd_never_has_a_figure_inside_the_car() {
        drive_through_a_crowd(2.0);
    }

    /// The same reversing at 5 m/s: the people behind the car are in its way.
    #[test]
    fn reversing_through_a_crowd_never_has_a_figure_inside_the_car() {
        drive_through_a_crowd(-5.0);
    }

    /// Driving at `speed` (m/s, negative reversing) for 60 s up and down 60 m of walk through the thickest crowd on the strip, the
    /// car placed on its line each step (on the ground alone: no prop or wall in the
    /// collider to push it off) and the app stepping and drawing the people round it: no
    /// figure (where it stands, its pelvis, torso and head) is ever inside its footprint
    /// (but anyone it appears on at the first step, for 1.5 s), no hand or foot more than
    /// 5 cm, people step aside and hop where it is seen, and every figure stays in one
    /// piece.
    fn drive_through_a_crowd(speed: f32) {
        let mut app = app(&format!("{STRIP}&t=12:00&traffic=busy"));
        let crowd = &app.npcs.crowd;
        let all: Vec<Vec3> = crowd
            .people()
            .filter(|p| p.pose.action != Action::Sit)
            .map(|p| Vec3::from(p.pos))
            .collect();
        let thick = *all
            .iter()
            .max_by_key(|a| all.iter().filter(|b| a.distance(**b) < 20.0).count())
            .unwrap();
        // The walk by it with the most people along 60 m of it.
        let w = crowd.walks();
        let along = |at: Vec3, d: Vec3| {
            all.iter()
                .filter(|p| {
                    let r = **p - at;
                    r.dot(d).abs() < 30.0 && (r.x * d.z - r.z * d.x).abs() < 3.0
                })
                .count()
        };
        let (at, dir) = w
            .links
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.kind == traffic::walks::LinkKind::Walk && l.road_side != 0 && l.length_m > 4.0
            })
            .map(|(i, l)| {
                let at = Vec3::from(w.point(i as u32, 0.5 * l.length_m, 0.0));
                (at, Vec3::new(l.dir[0], 0.0, l.dir[1]))
            })
            .filter(|(at, _)| at.distance(thick) < 20.0)
            .max_by_key(|&(at, d)| along(at, d))
            .unwrap();
        let none = InputState::default();
        let (mut inside_steps, mut near, mut dodged) = (0, Vec::new(), Vec::new());
        let (mut landed, mut limb) = (Vec::new(), f32::NEG_INFINITY);
        // Nothing in the collider but the ground, so no prop or wall pushes the car off
        // its line.
        let mut bare =
            crate::places::build(crate::scene::PlaceKey::Coast, skywave_core::Seed(1)).unwrap();
        bare.props.items.clear();
        bare.buildings.clear();
        app.ground = crate::drive::ground_for(&bare);
        let mut steps = 0u32;
        while steps < 60 * skywave_core::SIM_HZ {
            // Up and down 60 m of walk through the crowd, turning at each end; reversing,
            // the car faces away from where it goes.
            let u = (speed.abs() * steps as f32 / skywave_core::SIM_HZ as f32) % 120.0;
            let (p, dir) = if u < 60.0 {
                (at + dir * (u - 30.0), dir)
            } else {
                (at + dir * (90.0 - u), -dir)
            };
            let facing = dir * speed.signum();
            let heading = (-facing.x).atan2(-facing.z);
            let (y, _) = app.ground.level_at([p.x, p.z]);
            let mut s = vehicle::VehicleState::spawn(&app.drive.spec, [p.x, y, p.z], heading);
            s.vel = (dir * speed.abs()).to_array();
            s.speed_mps = speed;
            app.drive.state = s;
            app.drive.prev = s;
            steps += app.update(&none, 1.0 / skywave_core::SIM_HZ as f64).count;
            app.scene_view(1.0, 16.0 / 9.0);
            let car = app.traffic_player();
            for i in 0..app.npcs.crowd.peds.pedestrians().len() {
                let p = app.npcs.crowd.peds.pedestrians()[i];
                if !p.alive {
                    continue;
                }
                let parts = assert_attached(&mut app, i, 1.0);
                // The figure: where it stands and its body (pelvis, torso, head); a hand
                // or a foot may reach past the person's clearance.
                let hit = depth(&car, Vec3::from(p.pos)) >= 0.0
                    || parts[..3]
                        .iter()
                        .any(|m| depth(&car, m.w_axis.truncate()) >= 0.0);
                for m in &parts[3..] {
                    limb = limb.max(depth(&car, m.w_axis.truncate()));
                }
                if steps <= 1 && hit {
                    landed.push(p.id);
                }
                if hit && !(landed.contains(&p.id) && steps < 180) {
                    inside_steps += 1;
                }
                if Vec3::from(p.pos).distance(Vec3::from(car.pos)) < 5.0 && !near.contains(&p.id) {
                    near.push(p.id);
                }
                let dodging = matches!(p.pose.action, Action::Aside | Action::Hop);
                if dodging && p.pose.weight > 0.5 && !dodged.contains(&p.id) {
                    dodged.push(p.id);
                }
            }
        }
        eprintln!(
            "{speed} m/s through a crowd: {inside_steps} person-steps inside, {} came near, {} \
             stepped aside or hopped, limbs at most {limb:.2} m inside",
            near.len(),
            dodged.len()
        );
        assert_eq!(inside_steps, 0);
        assert!(limb < 0.05, "a limb {limb:.2} m inside");
        assert!(near.len() > 5, "{}", near.len());
        assert!(dodged.len() > 3, "{}", dodged.len());
    }

    /// The in-game pedestrian step (busy people on the strip) within its budget: the best
    /// of three windows' median under 0.5 ms in this build, asserted only under
    /// `SKYWAVE_ENFORCE_BUDGETS`.
    #[test]
    fn the_pedestrian_step_is_within_budget() {
        let mut app = app(&format!("{STRIP}&t=12:00&traffic=busy&still=1"));
        let inputs = app.traffic_inputs();
        let mut medians = Vec::new();
        let mut times = Vec::with_capacity(600);
        for _ in 0..3 {
            times.clear();
            for _ in 0..600 {
                app.npcs.step(&inputs);
                let start = std::time::Instant::now();
                app.npcs.step_peds(&inputs);
                times.push(start.elapsed().as_secs_f64() * 1e3);
            }
            times.sort_by(f64::total_cmp);
            medians.push(times[times.len() / 2]);
        }
        let median = medians.iter().copied().fold(f64::INFINITY, f64::min);
        eprintln!(
            "pedestrian step: {} people, median {median:.3} ms (windows {medians:.3?})",
            app.pedestrian_count()
        );
        assert!(app.pedestrian_count() >= 80, "{}", app.pedestrian_count());
        if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
            assert!(median < 0.5, "median {median:.3} ms per step");
        }
    }

    /// Most bytes of mesh the figures may take (`docs/PERFORMANCE.md`).
    const FIGURE_MESH_BYTES_MAX: usize = 3_000_000;

    #[test]
    fn figure_meshes_stay_within_budget() {
        let place =
            crate::places::build(crate::scene::PlaceKey::Coast, skywave_core::Seed(1)).unwrap();
        let graph = LaneGraph::new(&place);
        let mut crowd = Crowd::new(&place, &graph, skywave_core::Seed(1));
        let (mut n, mut tris) = (0, 0);
        crowd.build_meshes(|m| {
            n += 1;
            tris += m.indices.len() / 3;
            MeshId(n)
        });
        let bytes = crowd.built_bytes();
        eprintln!("figures: {n} meshes, {tris} triangles, {bytes} bytes");
        assert_eq!(n as usize, VARIANTS * PARTS);
        assert!(bytes <= FIGURE_MESH_BYTES_MAX, "{bytes} bytes");
    }
}
