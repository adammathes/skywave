//! The drive-thru (`docs/WORLD_GENERATION.md`, "Set pieces"): stopping is the order. Pull
//! up with the driver's door at a landmark's service window (a kit's `Window` part, the
//! donut shop's) and hold still a moment, and the person inside leans out over the sill and
//! hands over a box, which lands on the passenger seat and rides there for the rest of the
//! drive (up to [`MAX_BOXES`] stacked). No menu, no money, no UI; the radio plays on.
//! Driving [`LEAVE_M`] off lets the window serve again.
//!
//! [`DriveThru::of`] reads the windows off the place's landmark props; [`DriveThru::step`]
//! runs with the 120 Hz sim; the server, the box in hand or in the air are drawn near the
//! eye ([`DriveThru::push`]), the boxes on the seat with the car
//! ([`DriveThru::push_seat`]). Nothing here allocates per frame.

use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use place::{Place, PropKind};
use render::{Mesh, MeshId, MeshInstance};
use skywave_core::{hash64, Seed};
use traffic::figure::{figure, Era, Figure, MeshData, Xform, PARTS};
use world::landmarks::Kit;
use world::math::V3;

use crate::car;
use crate::drive::Pose;
use crate::lighting::{Bounds, FrameLights};

/// How long the car holds still at the window before the order is in, s.
pub const HOLD_S: f32 = 0.6;
/// The serve, s from the order: the server leans out and holds the box out, lets go of it
/// at [`HANDOFF_S`], it lands on the seat at [`LAND_S`], and they are back in at
/// [`SERVE_S`].
pub const HANDOFF_S: f32 = 1.3;
pub const LAND_S: f32 = 2.0;
pub const SERVE_S: f32 = 2.8;
/// Below this the car is holding still, m/s.
const STILL_MPS: f32 = 0.4;
/// The driver's door is at the window from this near the wall's face to this far, and
/// within this of the window along the wall, m.
const GAP_M: (f32, f32) = (0.2, 2.4);
const ALONG_M: f32 = 1.5;
/// The driver's side faces the window within this cosine.
const SQUARE_COS: f32 = 0.85;
/// How far from the wall a driver pulls up (the `donut=` scene's car), m.
pub const PARK_GAP_M: f32 = 0.8;
/// The window serves again once the door is this far from it, m.
pub const LEAVE_M: f32 = 6.0;
/// The most boxes the seat holds.
pub const MAX_BOXES: u8 = 3;
/// The server stands this far in from the window's face, m.
const STAND_IN_M: f32 = 0.22;
/// A box: width across, height and depth, m.
const BOX_M: [f32; 3] = [0.34, 0.09, 0.3];
/// The box rises this high over its path on the way in, m.
const ARC_M: f32 = 0.25;
/// The server and a box in their hands or the air are drawn within this of the eye, m.
pub const DRAW_RADIUS_M: f32 = 200.0;
/// Most instances [`DriveThru::push`] adds a frame: the server and a box.
pub const MAX_INSTANCES: usize = PARTS + 1;
/// Hash key of the server's figure seed.
const KEY_SERVER: u64 = 0x5345_5256_4552;

/// A service window: the sill's middle on the wall's face, the way it faces out and the
/// height of the floor inside (its landmark's lot).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub sill: Vec3,
    pub out: Vec2,
    pub floor_y: f32,
}

/// Where the window stands with the player's car.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Nobody at a window.
    Idle,
    /// At window `at` and still for `held` seconds.
    Waiting { at: u16, held: f32 },
    /// Window `at` serving, `t` seconds in.
    Serving { at: u16, t: f32 },
    /// Served at window `at`: it serves again once the car has left.
    Served { at: u16 },
}

/// The car as the window sees it each step.
#[derive(Clone, Copy, Debug)]
pub struct Car {
    /// Ground point of the driver's door (the driver's side at the driver's hip) and the
    /// way out of it, `(x, z)`.
    pub door: Vec2,
    pub side: Vec2,
    /// Signed speed, m/s.
    pub speed: f32,
}

/// The place's service windows, the serve and the boxes on the seat.
pub struct DriveThru {
    pub windows: Vec<Window>,
    pub phase: Phase,
    /// Boxes on the passenger seat.
    pub boxes: u8,
    /// The server (their part meshes until [`DriveThru::meshes`] takes them).
    server: Figure,
    ids: Vec<MeshId>,
}

/// The body-space point the bottom box's middle sits at on the passenger seat of a car
/// whose driver's hip is `hip`: across the car, on the cushion just ahead of the hip.
pub fn seat_point(hip: Vec3) -> Vec3 {
    Vec3::new(-hip.x, hip.y + 0.5 * BOX_M[1] + 0.01, hip.z - 0.2)
}

impl DriveThru {
    /// The service windows on `place`'s landmarks built from `kits`; `None` without one.
    /// The server is a figure of the place's era picked by `seed`.
    pub fn of(place: &Place, kits: &[Kit], seed: Seed) -> Option<DriveThru> {
        let mut windows = Vec::new();
        for p in &place.props.items {
            if let PropKind::Landmark { kit, variant } = p.kind {
                let Some(k) = kits.get(usize::from(kit)) else {
                    continue;
                };
                let base = V3::new(p.pos[0], p.pos[1], p.pos[2]);
                let facing = world::props::dir_of_yaw(p.yaw);
                for (sill, out) in world::landmarks::windows(k, usize::from(variant), base, facing)
                {
                    windows.push(Window {
                        sill: Vec3::new(sill.x, sill.y, sill.z),
                        out: Vec2::new(out.x, out.y),
                        floor_y: p.pos[1],
                    });
                }
            }
        }
        if windows.is_empty() {
            return None;
        }
        let era = if place.header.era == place::Era::Seventies {
            Era::Seventies
        } else {
            Era::Eighties
        };
        Some(DriveThru {
            windows,
            phase: Phase::Idle,
            boxes: 0,
            server: figure(hash64(seed.0, &[KEY_SERVER]), era),
            ids: Vec::new(),
        })
    }

    /// The meshes its instances draw, in order (the server's parts, then the box);
    /// [`DriveThru::set_ids`] takes their ids.
    pub fn meshes(&mut self) -> Vec<Mesh> {
        let parts = core::mem::take(&mut self.server.parts);
        let mut out: Vec<Mesh> = parts.iter().map(car::plain_mesh).collect();
        out.push(box_mesh());
        out
    }

    pub fn set_ids(&mut self, ids: Vec<MeshId>) {
        self.ids = ids;
    }

    /// The window `car`'s door is at, if it is at one.
    fn at_window(&self, car: &Car) -> Option<usize> {
        self.windows.iter().position(|w| {
            let d = car.door - Vec2::new(w.sill.x, w.sill.z);
            let gap = d.dot(w.out);
            let along = d.perp_dot(w.out).abs();
            (GAP_M.0..=GAP_M.1).contains(&gap)
                && along <= ALONG_M
                && car.side.dot(w.out) <= -SQUARE_COS
        })
    }

    /// One simulation step of `dt` seconds with the player's car.
    pub fn step(&mut self, car: &Car, dt: f32) {
        let still = car.speed.abs() < STILL_MPS;
        self.phase = match self.phase {
            Phase::Idle => match self.at_window(car) {
                Some(at) if still => Phase::Waiting {
                    at: at as u16,
                    held: dt,
                },
                _ => Phase::Idle,
            },
            Phase::Waiting { at, held } => {
                if !still || self.at_window(car) != Some(usize::from(at)) {
                    Phase::Idle
                } else if held + dt >= HOLD_S {
                    Phase::Serving { at, t: 0.0 }
                } else {
                    Phase::Waiting {
                        at,
                        held: held + dt,
                    }
                }
            }
            // Ordered is ordered: the serve plays out even if the car rolls on.
            Phase::Serving { at, t } => {
                let next = t + dt;
                if t < LAND_S && next >= LAND_S {
                    self.boxes = (self.boxes + 1).min(MAX_BOXES);
                }
                if next >= SERVE_S {
                    Phase::Served { at }
                } else {
                    Phase::Serving { at, t: next }
                }
            }
            Phase::Served { at } => {
                let w = &self.windows[usize::from(at)];
                if car.door.distance(Vec2::new(w.sill.x, w.sill.z)) > LEAVE_M {
                    Phase::Idle
                } else {
                    Phase::Served { at }
                }
            }
        };
    }

    /// The server's pose `t` seconds into a serve: how far out they lean and hold the box
    /// out, each 0–1.
    fn lean_reach(t: f32) -> (f32, f32) {
        let ramp = |a: f32, b: f32| ((t - a) / (b - a)).clamp(0.0, 1.0);
        let lean = ramp(0.0, 0.5) - ramp(LAND_S, SERVE_S);
        let reach = ramp(0.3, 0.9) - ramp(HANDOFF_S, LAND_S);
        (lean, reach)
    }

    /// The server's figure frame at window `w`: on the floor [`STAND_IN_M`] inside the
    /// sill, facing out.
    fn server_frame(w: &Window) -> Mat4 {
        let feet = w.sill - Vec3::new(w.out.x, 0.0, w.out.y) * STAND_IN_M;
        // The figure faces its -z.
        let yaw = (-w.out.x).atan2(-w.out.y);
        Mat4::from_rotation_translation(
            Quat::from_rotation_y(yaw),
            Vec3::new(feet.x, w.floor_y, feet.z),
        )
    }

    /// Appends the server leaning out of the window serving and the box in their hand or
    /// on its way to the car (`pose` and its seat point, body space) for a serve within
    /// [`DRAW_RADIUS_M`] of `eye`, lit by the frame's `lights`.
    pub fn push(
        &self,
        eye: Vec3,
        pose: &Pose,
        seat: Vec3,
        lights: &FrameLights,
        out: &mut Vec<MeshInstance>,
    ) {
        let Phase::Serving { at, t } = self.phase else {
            return;
        };
        if self.ids.len() < PARTS + 1 {
            return;
        }
        let w = &self.windows[usize::from(at)];
        if w.sill.distance(eye) > DRAW_RADIUS_M {
            return;
        }
        let frame = Self::server_frame(w);
        let (lean, reach) = Self::lean_reach(t);
        let j = self.server.skeleton.serve(lean, reach);
        let mut xf = [Xform::IDENTITY; PARTS];
        self.server.skeleton.pose(&j, &mut xf);
        let bounds = Bounds::cube(w.sill, 1.5);
        let set = lights.set_for(&bounds);
        for (k, x) in xf.iter().enumerate() {
            out.push(MeshInstance {
                lights: set,
                ..MeshInstance::new(self.ids[k], (frame * part_matrix(x)).to_cols_array_2d())
            });
        }
        // The box: in the hand once the arm is half out, then on its way to the seat.
        let held = |t: f32| {
            let (lean, reach) = Self::lean_reach(t);
            let hand = Vec3::from(
                self.server
                    .skeleton
                    .hand(&self.server.skeleton.serve(lean, reach)),
            );
            frame.transform_point3(hand - Vec3::Y * 0.5 * BOX_M[1])
        };
        let server_rot = Quat::from_mat4(&frame);
        let transform = if !(0.6..LAND_S).contains(&t) {
            return;
        } else if t < HANDOFF_S {
            Mat4::from_rotation_translation(server_rot, held(t))
        } else {
            let u = (t - HANDOFF_S) / (LAND_S - HANDOFF_S);
            let s = u * u * (3.0 - 2.0 * u);
            let to = pose.point(seat);
            let p =
                held(HANDOFF_S).lerp(to, s) + Vec3::Y * (ARC_M * (core::f32::consts::PI * u).sin());
            Mat4::from_rotation_translation(server_rot.slerp(pose.rot, s), p)
        };
        out.push(MeshInstance {
            lights: set,
            ..MeshInstance::new(self.ids[PARTS], transform.to_cols_array_2d())
        });
    }

    /// Appends the boxes on the passenger seat of the car at `body` (its body-to-world
    /// matrix) whose bottom box sits at `seat`, body space; `cabin` as the car's interior.
    pub fn push_seat(&self, body: Mat4, seat: Vec3, cabin: f32, out: &mut Vec<MeshInstance>) {
        if self.ids.len() < PARTS + 1 {
            return;
        }
        // The cushion tips back a little; each box sits a little askew on the one below.
        const TURN: [f32; MAX_BOXES as usize] = [0.04, -0.09, 0.07];
        for (k, &turn) in TURN.iter().enumerate().take(usize::from(self.boxes)) {
            let local = Mat4::from_rotation_translation(
                Quat::from_rotation_x(0.1) * Quat::from_rotation_y(turn),
                seat + Vec3::Y * (k as f32 * BOX_M[1]),
            );
            out.push(MeshInstance {
                cabin,
                casts_shadow: false,
                ..MeshInstance::new(self.ids[PARTS], (body * local).to_cols_array_2d())
            });
        }
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

/// The box: pink card centered on its middle, a cream lid band round its top edge (vertex
/// colors linear, as the figures' and the cars').
fn box_mesh() -> Mesh {
    let mut m = MeshData::default();
    let [w, h, d] = BOX_M.map(|v| 0.5 * v);
    let card = [150, 30, 62, 255];
    let lid = [118, 100, 88, 255];
    let band = 0.6 * h;
    for (lo, hi, color) in [(-h, band, card), (band, h, lid)] {
        // Each face: its normal axis and sign, then its four corners counter-clockwise.
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let corner = |a: f32, b: f32| {
                    let mut p = [0.0f32; 3];
                    let ext = |i: usize, t: f32| match i {
                        0 => t * w,
                        1 => 0.5 * (lo + hi) + t * 0.5 * (hi - lo),
                        _ => t * d,
                    };
                    p[axis] = ext(axis, sign);
                    p[u] = ext(u, a);
                    p[v] = ext(v, b);
                    p
                };
                let base = m.positions.len() as u32;
                let quad = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
                for (a, b) in quad {
                    m.positions.push(corner(a * sign, b));
                    let mut n = [0.0; 3];
                    n[axis] = sign;
                    m.normals.push(n);
                    m.uvs.push([0.0; 2]);
                    m.colors.push(color);
                    m.emissive.push([0; 4]);
                }
                m.indices
                    .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
    }
    car::plain_mesh(&m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::SceneSetup;
    use crate::App;
    use place::Prop;
    use vehicle::VehicleInput;
    use world::math::V2;

    const DT: f32 = skywave_core::SIM_DT as f32;

    /// The donut shop's kit index and the window of a test lot with it in `variant`.
    fn shop(variant: u8) -> DriveThru {
        let kits = crate::places::landmarks();
        let kit = kits.iter().position(|k| k.id == "donut_shop").unwrap();
        let mut place = Place::test_grid();
        place.props.items = vec![Prop {
            pos: [0.0, 2.0, 0.0],
            yaw: 0.0,
            kind: PropKind::Landmark {
                kit: kit as u8,
                variant,
            },
        }];
        DriveThru::of(&place, kits, Seed(1)).unwrap()
    }

    /// A car with its driver's door `gap` m out from window `w` and `along` m along it,
    /// its driver's side toward it, at `speed`.
    fn car(w: &Window, gap: f32, along: f32, speed: f32) -> Car {
        Car {
            door: Vec2::new(w.sill.x, w.sill.z) + w.out * gap + w.out.perp() * along,
            side: -w.out,
            speed,
        }
    }

    fn run(d: &mut DriveThru, c: &Car, secs: f32) {
        for _ in 0..(secs / DT).ceil() as u32 {
            d.step(c, DT);
        }
    }

    #[test]
    fn every_variant_has_its_window_on_the_right_side_at_its_lot_level() {
        let kits = crate::places::landmarks();
        let kit = kits.iter().find(|k| k.id == "donut_shop").unwrap();
        for v in 0..kit.variants.len() {
            let d = shop(v as u8);
            assert_eq!(d.windows.len(), 1, "variant {v}");
            let w = d.windows[0];
            // Yaw 0 faces -z; the kit's +x (the right seen from the road) is world -x.
            assert!((w.out - Vec2::new(-1.0, 0.0)).length() < 1e-5, "{w:?}");
            assert_eq!((w.floor_y, w.sill.y), (2.0, 3.0));
            assert!((w.sill.z - 20.0).abs() < 1e-4, "{w:?}");
        }
    }

    /// The lane goes somewhere: in every variant a car 2 m wide fits the loop in past the
    /// window, round behind the shop and out up the left side, inside the lot, and a curb
    /// closes the lot's back.
    #[test]
    fn every_variant_loops_the_lane_round_the_back() {
        let kits = crate::places::landmarks();
        let kit = kits.iter().find(|k| k.id == "donut_shop").unwrap();
        let [w, d] = kit.lot;
        // Facing +z, so the kit frame is the world's.
        let (pos, facing) = (V2::new(0.0, 0.0), V2::new(0.0, 1.0));
        for v in 0..kit.variants.len() {
            let mut slabs = Vec::new();
            world::landmarks::slabs(kit, v, pos, 0.0, facing, &mut slabs);
            let sill = world::landmarks::windows(kit, v, V3::new(0.0, 0.0, 0.0), facing)[0].0;
            let (right, back, left) = (sill.x + 1.9, -29.0, -11.25);
            let legs = [
                ([right, 0.0], [right, back]),
                ([right, back], [left, back]),
                ([left, back], [left, 0.0]),
            ];
            let r = 1.0;
            for (a, b) in legs {
                for k in 0..=200 {
                    let t = k as f32 / 200.0;
                    let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                    assert!(
                        p[0].abs() + r <= 0.5 * w && p[1] - r >= -d,
                        "variant {v}: {p:?}"
                    );
                    for s in &slabs {
                        let clear = (p[0] - s.c.x).abs() >= s.half[0] + r
                            || (p[1] - s.c.y).abs() >= s.half[1] + r;
                        assert!(clear, "variant {v}: {p:?} hits {s:?}");
                    }
                }
            }
            let curb = slabs
                .iter()
                .any(|s| s.half[0] >= 0.5 * w - 0.01 && s.c.y - s.half[1] <= -d + 0.01);
            assert!(curb, "variant {v}");
        }
    }

    #[test]
    fn stopping_at_the_window_is_the_order() {
        let mut d = shop(1);
        let w = d.windows[0];
        let at = car(&w, PARK_GAP_M, 0.3, 0.0);
        run(&mut d, &at, HOLD_S + 0.05);
        assert!(matches!(d.phase, Phase::Serving { .. }), "{:?}", d.phase);
        run(&mut d, &at, LAND_S - 0.1);
        assert_eq!(d.boxes, 0, "still on its way");
        run(&mut d, &at, 0.2);
        assert_eq!(d.boxes, 1, "on the seat");
        // Waiting on at the window does not order again.
        run(&mut d, &at, 10.0);
        assert_eq!((d.phase, d.boxes), (Phase::Served { at: 0 }, 1));
        // Away and back, three times: the seat holds three.
        for n in [2, 3, 3] {
            run(&mut d, &car(&w, PARK_GAP_M, LEAVE_M + 1.0, 5.0), 0.1);
            assert_eq!(d.phase, Phase::Idle);
            run(&mut d, &at, HOLD_S + SERVE_S + 0.1);
            assert_eq!(d.boxes, n);
        }
    }

    #[test]
    fn passing_by_or_stopping_wrong_orders_nothing() {
        let mut d = shop(0);
        let w = d.windows[0];
        // Rolling past at walking pace.
        for k in 0..400 {
            let along = -4.0 + k as f32 * 2.0 * DT;
            d.step(&car(&w, PARK_GAP_M, along, 2.0), DT);
        }
        // Stopped too far out, too far along, or the passenger's side to it.
        run(&mut d, &car(&w, GAP_M.1 + 0.3, 0.0, 0.0), 5.0);
        run(&mut d, &car(&w, PARK_GAP_M, ALONG_M + 0.3, 0.0), 5.0);
        let wrong = Car {
            side: w.out,
            ..car(&w, PARK_GAP_M, 0.0, 0.0)
        };
        run(&mut d, &wrong, 5.0);
        assert_eq!((d.phase, d.boxes), (Phase::Idle, 0));
        // Pulling away inside the hold cancels it.
        run(&mut d, &car(&w, PARK_GAP_M, 0.0, 0.0), 0.5 * HOLD_S);
        run(&mut d, &car(&w, PARK_GAP_M, 0.0, 1.0), DT);
        assert_eq!(d.phase, Phase::Idle);
    }

    fn scene(query: &str) -> App {
        App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap()
    }

    /// The mesh id of the box.
    fn box_id(app: &App) -> MeshId {
        app.drivethru.as_ref().unwrap().ids[PARTS]
    }

    #[test]
    fn the_valley_serves_a_box_onto_the_seat_and_draws_the_server() {
        let q = "seed=1&scenario=mirage_valley_2001&cam=chase-mid&t=12:00&still=1";
        let mut app = scene(&format!("{q}&donut=1.0"));
        let d = app.drivethru.as_ref().unwrap();
        assert_eq!(d.windows.len(), 1, "the donut shop's");
        assert!(matches!(d.phase, Phase::Serving { .. }), "{:?}", d.phase);
        let (bx, server) = (box_id(&app), d.ids[0]);
        let view = app.scene_view(1.0, 16.0 / 9.0);
        let count = |id: MeshId| view.instances.iter().filter(|i| i.mesh == id).count();
        // The server leaning out, the box in their hand.
        assert_eq!((count(server), count(bx)), (1, 1));
        let mut later = scene(&format!("{q}&donut=5"));
        assert_eq!(later.drivethru.as_ref().unwrap().boxes, 1);
        let bx = box_id(&later);
        let server = later.drivethru.as_ref().unwrap().ids[0];
        let view = later.scene_view(1.0, 16.0 / 9.0);
        let on_seat: Vec<_> = view.instances.iter().filter(|i| i.mesh == bx).collect();
        assert_eq!(on_seat.len(), 1);
        assert_eq!(
            view.instances.iter().filter(|i| i.mesh == server).count(),
            0
        );
        // On the cushion, inside the car.
        let p = Vec3::from_slice(&on_seat[0].transform[3][..3]);
        let pose = later.drive.pose(1.0);
        let local = pose.rot.inverse() * (p - pose.pos);
        let [_, w, _] = later.drive.spec.dims_m;
        assert!(local.x > 0.1 && local.x < 0.5 * w - 0.15, "{local}");
        // A new car starts with an empty seat.
        later.switch_vehicle("homestead_wagon").unwrap();
        assert_eq!(later.drivethru.as_ref().unwrap().boxes, 0);
    }

    /// Probe: driving in off the street down the lane at a crawl and stopping with the nose
    /// at the stop bar orders, for every car; the car never touches the building.
    #[test]
    fn every_car_driven_down_the_lane_to_the_bar_is_served() {
        let q = "seed=1&scenario=mirage_valley_2001&t=12:00";
        let mut app = scene(q);
        let kits = crate::places::landmarks();
        let kit = kits.iter().position(|k| k.id == "donut_shop").unwrap() as u8;
        let place = crate::places::build_in(
            crate::scene::PlaceKey::Coast,
            Seed(1),
            crate::scenario::get(app.scenario()),
        )
        .unwrap();
        let prop = place
            .props
            .items
            .iter()
            .find(|p| matches!(p.kind, PropKind::Landmark { kit: k, .. } if k == kit))
            .unwrap();
        let f = world::props::dir_of_yaw(prop.yaw);
        let road = Vec2::new(f.x, f.y);
        let lineup = crate::scenario::lineup(app.scenario());
        let mut served = 0;
        let mut worst_gap = f32::MAX;
        let mut ids = Vec::new();
        for (i, &(id, _)) in crate::vehicles::BUILTIN.iter().enumerate() {
            if lineup.allows(i) {
                ids.push(id);
            }
        }
        for id in &ids {
            app.switch_vehicle(id).unwrap();
            let w = app.drivethru.as_ref().unwrap().windows[0];
            let hip = app.drive.layout.seating.hip;
            let [len, width, _] = app.drive.spec.dims_m;
            // 24 m out toward the street, nose in, 2.5 m wide of the lane's middle (clear of
            // the sidewalk's hydrant at its mouth at seed 1); the driver steers into it.
            let lane = 0.5 * width + 1.0;
            let start = Vec2::new(w.sill.x, w.sill.z) + w.out * (lane + 2.5) + road * 24.0;
            let (y, _) = app.ground.level_at([start.x, start.y]);
            let heading = (road.x).atan2(road.y);
            app.drive = crate::drive::Drive::with_layout(
                app.drive.spec.clone(),
                app.drive.layout,
                [start.x, y, start.y],
                heading,
            );
            let d = app.drivethru.as_mut().unwrap();
            d.phase = Phase::Idle;
            d.boxes = 0;
            // The stop bar's near edge (kit z -22.75) is 2.75 m past the window.
            let bar = Vec2::new(w.sill.x, w.sill.z) - road * 2.75;
            for _ in 0..(40.0 / DT) as u32 {
                let pose = app.drive.pose(1.0);
                let fw = pose.forward();
                let fw = Vec2::new(fw.x, fw.z);
                let nose = Vec2::new(pose.pos.x, pose.pos.z) + fw * 0.5 * len;
                let to_bar = (bar - nose).dot(-road);
                let off = (Vec2::new(pose.pos.x, pose.pos.z) - Vec2::new(w.sill.x, w.sill.z))
                    .dot(w.out)
                    - lane;
                let speed = app.drive.state.speed_mps;
                let want = (0.6 * to_bar).clamp(0.0, 4.0);
                let input = VehicleInput {
                    throttle: if speed < want { 0.35 } else { 0.0 },
                    brake: if to_bar < 0.4 || speed > want + 0.5 {
                        1.0
                    } else {
                        0.0
                    },
                    // Back to the lane's middle: positive steers right, and the window is on
                    // the left.
                    steer: (-0.8 * off - 2.0 * fw.dot(w.out)).clamp(-1.0, 1.0),
                    handbrake: false,
                };
                app.drive.step(input, &app.ground);
                let c = app.drive_thru_car();
                worst_gap = worst_gap.min((c.door - Vec2::new(w.sill.x, w.sill.z)).dot(w.out));
                app.drivethru.as_mut().unwrap().step(&c, DT);
            }
            let boxes = app.drivethru.as_ref().unwrap().boxes;
            eprintln!("{id}: {boxes} box(es), {:.2} m from the bar", {
                let pose = app.drive.pose(1.0);
                let fw = pose.forward();
                let nose = Vec2::new(pose.pos.x, pose.pos.z) + Vec2::new(fw.x, fw.z) * 0.5 * len;
                (bar - nose).dot(-road)
            });
            assert_eq!(boxes, 1, "{id}: hip {hip}");
            served += 1;
        }
        eprintln!(
            "{served} of {} cars served; nearest door to the wall {worst_gap:.2} m",
            ids.len()
        );
        assert!(worst_gap > 0.3, "{worst_gap}");
    }
}
