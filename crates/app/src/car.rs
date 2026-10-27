//! The player's car as the renderer draws it: the `vehicle::shape` meshes for its
//! [`ShapeSpec`] converted to [`render::Mesh`]es in body space, and the [`CarLayout`] the
//! cameras and [`crate::drive`] animate (eye, steering wheel, dials, wheel hubs, lamps).
//!
//! Everything is built once at startup: LOD0 (drawn) and LOD1 (registered for distant
//! cars later), each with the pop-up lamps down and up when the car has them; the
//! headlight switch picks the variant.
//!
//! **Lamps.** The shape's `lights` mesh holds every lens (head, tail, amber markers,
//! reverse) and bezel with the emissive set as if all were lit, and the shape does not
//! say which triangles are which. The vertex emissive is not scaled by the instance color,
//! so lamp states cannot be instance tints: each lens vertex is classified by its emissive
//! hue (warm white head, red tail, amber marker) and the mesh is baked four times,
//! headlights off/on × brake off/on, with each group's intensity ([`lamp_scale`]). Drawing
//! picks one of the four.
//!
//! **Glass** is single-sided, facing out. The forward pass culls back faces and has no
//! blended path yet, so windows are opaque tinted panes seen from outside and vanish from
//! inside: the cockpit sees the street through the windshield and side glass. Glass uvs
//! (meters along the car and up) map onto one pane of [`Tile::WindowGlass`], so the panes
//! carry its sky-reflection gradient and highlight streak.
//!
//! **Cabin walls.** The interior mesh covers the dash, door cards, pillars and seats but
//! not every gap down to the floor, and the shell's own faces point out, so from the seat
//! they are culled and the street shows through the footwells and under the door cards.
//! The cockpit therefore also draws [`CarLod::shell_inside`]: the body with its triangles
//! turned inside out (reversed winding, negated normals). From outside it is always behind
//! the shell's front faces, so it costs only triangles, and only in the first-person
//! cameras.
//!
//! **Convertibles** ([`CarLayout::open_top`]) have no roof or side glass, so their cabin
//! shows from outside: every camera draws the interior and steering wheel. The body has
//! nothing above the belt over the cabin, so neither has its inside-out copy, and the
//! cockpit sees the sky.

use glam::{Mat4, Quat, Vec3};
use render::{Mesh, MeshId, Tile, Vertex};
use vehicle::shape::{self, BuildOptions, CarMesh, HeadLights, MeshData, ShapeSpec};
use vehicle::{Gauge, SpeedUnit, VehicleSpec};

/// Steering wheel angle at full road-wheel lock: 2.5 turns lock to lock, degrees.
pub const STEERING_WHEEL_LOCK_DEG: f32 = 450.0;
/// Needle sweep from the bottom of a dial's scale to its top, degrees, centered on 12
/// o'clock (the scale starts at about 7:30 and ends at 4:30).
pub const DIAL_SWEEP_DEG: f32 = 270.0;
/// How far the needles and scale ticks stand off the dial face, toward the driver, m.
const NEEDLE_LIFT_M: f32 = 0.003;
const TICK_LIFT_M: f32 = 0.0025;
/// Needle glow: a white needle with a red tip, intensity scale.
const NEEDLE_RGB: [u8; 3] = [240, 238, 230];
const NEEDLE_TIP_RGB: [u8; 3] = [255, 40, 24];
const NEEDLE_GLOW: f32 = 3.0;
/// Where the red tip starts, as a share of the needle's reach.
const NEEDLE_TIP_AT: f32 = 0.62;
/// Scale ticks per dial and their glow.
const DIAL_TICKS: usize = 9;
const TICK_GLOW: f32 = 1.6;
/// Headlight tell-tales on the first dial's face below the hub (where the needle never
/// sweeps): low beams green on the left, high beams blue on the right, their glow, and
/// their half size and distance from the hub as shares of the dial's radius.
const TELLTALE_RGB: [[u8; 3]; 2] = [[40, 255, 70], [40, 110, 255]];
const TELLTALE_GLOW: f32 = 4.0;
const TELLTALE_HALF: f32 = 0.1;
const TELLTALE_AT: [f32; 2] = [0.22, 0.55];
/// Vertex tint for the grille's egg-crate atlas pattern (the shape's grille color is near
/// black, which would hide the tintable pattern), linear unorm8.
const GRILLE_TINT: [u8; 4] = [96, 96, 98, 255];
/// Most a glass tint channel may be, so the pre-colored window tile keeps its own colors.
const GLASS_TINT_MAX: f32 = 210.0;
/// m/s to the dial's unit.
const MPH_PER_MPS: f32 = 2.236_936;
const KMH_PER_MPS: f32 = 3.6;

/// Lamp groups, by emissive hue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LampGroup {
    /// Unlit parts of the lamp mesh: bezels, reverse lenses.
    None,
    Head,
    Tail,
    /// Amber parking and side-marker lamps (lit with the headlights).
    Marker,
}

/// Which group a lamp vertex with emissive `e` belongs to.
pub fn lamp_group(e: [u8; 4]) -> LampGroup {
    if e[3] == 0 {
        LampGroup::None
    } else if e[1] < 64 {
        LampGroup::Tail
    } else if e[2] < 96 {
        LampGroup::Marker
    } else {
        LampGroup::Head
    }
}

/// Emissive intensity scale of `group` with the headlights `on` and the brake `braking`;
/// `None` keeps the shape's own (the lit look it was built with).
pub fn lamp_scale(group: LampGroup, on: bool, braking: bool) -> Option<f32> {
    match group {
        LampGroup::None => None,
        LampGroup::Head | LampGroup::Marker => (!on).then_some(0.0),
        LampGroup::Tail => Some(if braking {
            BRAKE_GLOW
        } else if on {
            TAIL_GLOW
        } else {
            0.0
        }),
    }
}

/// Tail lamp intensity with the headlights on, and under braking.
pub const TAIL_GLOW: f32 = 1.0;
pub const BRAKE_GLOW: f32 = 3.5;

/// What a dial reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DialReading {
    /// Speed in the dial's unit: `(min, max, unit per m/s)`.
    Speed(f32, f32, f32),
    /// Engine rpm: `(min, max)`.
    Rpm(f32, f32),
    /// No gauge in the spec for this dial: the needle rests mid-scale (a fuel gauge).
    Fixed,
}

impl DialReading {
    /// Needle position in `[0, 1]` along the scale.
    pub fn fraction(self, speed_mps: f32, rpm: f32) -> f32 {
        let f = match self {
            DialReading::Speed(lo, hi, k) => (speed_mps.abs() * k - lo) / (hi - lo),
            DialReading::Rpm(lo, hi) => (rpm - lo) / (hi - lo),
            DialReading::Fixed => 0.5,
        };
        if f.is_finite() {
            f.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// One dial in body space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DialLayout {
    pub center: Vec3,
    /// Toward the driver.
    pub normal: Vec3,
    /// 12 o'clock.
    pub up: Vec3,
    pub radius: f32,
    pub reading: DialReading,
}

impl DialLayout {
    /// Needle-to-body transform for a needle at `fraction` of the scale: the needle mesh
    /// points along local `+y` from the hub in the local `xy` plane, facing `+z`.
    pub fn needle(&self, fraction: f32) -> Mat4 {
        let right = self.up.cross(self.normal);
        let basis = Mat4::from_cols(
            right.extend(0.0),
            self.up.extend(0.0),
            self.normal.extend(0.0),
            (self.center + self.normal * NEEDLE_LIFT_M).extend(1.0),
        );
        // Clockwise from the bottom of the scale, as the driver sees it.
        let cw = (fraction - 0.5) * DIAL_SWEEP_DEG.to_radians();
        basis * Mat4::from_rotation_z(-cw)
    }
}

/// Where things are on the car, body space (origin at the center of mass), at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarLayout {
    /// Model space (the shape's: ground, centerline, front axle) to body space.
    pub offset: Vec3,
    /// Driver's eye.
    pub eye: Vec3,
    /// Beltline height at the eye's station.
    pub belt_y: f32,
    /// Dash top height.
    pub dash_y: f32,
    /// Hub centers, front-left, front-right, rear-left, rear-right.
    pub wheel_centers: [Vec3; 4],
    pub wheel_radius: f32,
    /// Steering wheel hub and its axis (toward the driver; positive turns are
    /// counter-clockwise as the driver sees them).
    pub wheel_pivot: Vec3,
    pub wheel_axis: Vec3,
    /// Steering wheel turn per radian of road-wheel angle.
    pub steering_ratio: f32,
    pub dials: [DialLayout; 3],
    pub dial_count: usize,
    /// The radio's display quad (bottom-left, bottom-right, top-right, top-left as the
    /// driver sees it), for the radio's station text.
    pub radio_display: [Vec3; 4],
    /// The radio display's glow, sRGB (the shape's interior `display` color).
    pub radio_glow: [u8; 3],
    /// Headlamp centers, left then right (lit and, for pop-ups, raised).
    pub headlamps: [Vec3; 2],
    /// Tail lamp centers, left then right.
    pub taillamps: [Vec3; 2],
    /// Overall body extent: length, width, height, m (the spec's `dims_m`).
    pub dims: [f32; 3],
    /// A convertible: the cabin is open above the belt, so the interior and steering wheel
    /// are drawn from every camera, not only the first-person ones.
    pub open_top: bool,
}

impl CarLayout {
    /// The layout of `car` (built from `shape` for `spec`) with its lamps lit by `lit`
    /// (pop-ups raised).
    fn new(spec: &VehicleSpec, shape: &ShapeSpec, car: &CarMesh, lit: &CarMesh) -> CarLayout {
        let offset = Vec3::from(shape::body_offset(spec));
        let at = |p: [f32; 3]| Vec3::from(p) + offset;
        let c = &car.cockpit;
        let eye_x = c.eye[2] + shape.wheels.front_x;
        let gauges = &spec.interior.gauges;
        let dials = core::array::from_fn(|k| {
            let d = &c.dials[k];
            let reading = match gauges.get(k) {
                Some(Gauge::Speedo(lo, hi, unit)) => DialReading::Speed(
                    *lo,
                    *hi,
                    match unit {
                        SpeedUnit::Mph => MPH_PER_MPS,
                        SpeedUnit::Kmh => KMH_PER_MPS,
                    },
                ),
                Some(Gauge::Tach(lo, hi)) => DialReading::Rpm(*lo, *hi),
                None => DialReading::Fixed,
            };
            DialLayout {
                center: at(d.center),
                normal: Vec3::from(d.normal),
                up: Vec3::from(d.up),
                radius: d.radius,
                reading,
            }
        });
        CarLayout {
            offset,
            eye: at(c.eye),
            belt_y: polyline_at(&shape.belt, eye_x) + offset.y,
            dash_y: shape.interior.dash_height + offset.y,
            wheel_centers: car.wheel_centers.map(at),
            wheel_radius: car.wheel_radius,
            wheel_pivot: at(c.wheel_pivot),
            wheel_axis: Vec3::from(c.wheel_axis),
            steering_ratio: STEERING_WHEEL_LOCK_DEG / spec.steering.max_deg,
            dials,
            dial_count: usize::from(c.dial_count).min(3),
            radio_display: c.radio_display.map(at),
            radio_glow: hex_rgb(&shape.interior.display).unwrap_or(DEFAULT_RADIO_GLOW),
            headlamps: lamp_centers(&lit.lights, LampGroup::Head).map(|p| p + offset),
            taillamps: lamp_centers(&lit.lights, LampGroup::Tail).map(|p| p + offset),
            dims: spec.dims_m,
            open_top: shape.roof.convertible(),
        }
    }

    /// Steering-wheel-to-body transform for road-wheel angle `steer_rad` (positive right).
    pub fn steering_wheel(&self, steer_rad: f32) -> Mat4 {
        // Steering left (negative) turns the wheel counter-clockwise: a positive turn.
        let turn = -steer_rad * self.steering_ratio;
        Mat4::from_translation(self.wheel_pivot)
            * Mat4::from_quat(Quat::from_axis_angle(self.wheel_axis, turn))
    }
}

/// The radio display's glow when the shape's color does not parse (a validated shape's
/// always does).
const DEFAULT_RADIO_GLOW: [u8; 3] = [60, 255, 122];

/// `#rrggbb` as bytes.
fn hex_rgb(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// `y` of the polyline `pts` (`x` increasing) at `x`, held flat beyond its ends.
fn polyline_at(pts: &[(f32, f32)], x: f32) -> f32 {
    let Some(&(x0, y0)) = pts.first() else {
        return 0.0;
    };
    if x <= x0 {
        return y0;
    }
    for w in pts.windows(2) {
        let ((xa, ya), (xb, yb)) = (w[0], w[1]);
        if x <= xb {
            return ya + (yb - ya) * (x - xa) / (xb - xa);
        }
    }
    pts[pts.len() - 1].1
}

/// Centers of the left (`x < 0`) and right lenses of `group` in `lights`, model space.
fn lamp_centers(lights: &MeshData, group: LampGroup) -> [Vec3; 2] {
    let mut sum = [Vec3::ZERO; 2];
    let mut n = [0u32; 2];
    for (p, e) in lights.positions.iter().zip(&lights.emissive) {
        if lamp_group(*e) == group {
            let side = usize::from(p[0] > 0.0);
            sum[side] += Vec3::from(*p);
            n[side] += 1;
        }
    }
    core::array::from_fn(|k| {
        if n[k] > 0 {
            sum[k] / n[k] as f32
        } else {
            Vec3::ZERO
        }
    })
}

/// One level of detail, converted.
#[derive(Clone, Debug, PartialEq)]
pub struct CarLod {
    /// Pop-ups down, up (the same mesh twice for a car without pop-ups).
    pub body: [Mesh; 2],
    /// The pop-ups-down body turned inside out: the cabin's walls seen from the seat.
    pub shell_inside: Mesh,
    pub glass: Mesh,
    pub interior: Mesh,
    /// Relative to the steering wheel pivot.
    pub steering_wheel: Mesh,
    /// The right-side wheel; the left ones are drawn turned 180° about `y`.
    pub wheel: Mesh,
    /// `lights[headlights][braking]`.
    pub lights: [[Mesh; 2]; 2],
}

/// Mesh ids of one level of detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarLodIds {
    pub body: [MeshId; 2],
    pub shell_inside: MeshId,
    pub glass: MeshId,
    pub interior: MeshId,
    pub steering_wheel: MeshId,
    pub wheel: MeshId,
    pub lights: [[MeshId; 2]; 2],
}

/// Mesh ids of the player's car, as registered with the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarMeshes {
    pub lods: [CarLodIds; 2],
    /// One needle per dial.
    pub needles: [MeshId; 3],
    /// Every dial's scale ticks, body space.
    pub dial_ticks: MeshId,
    /// The headlight tell-tales, low and high beams, body space.
    pub telltales: [MeshId; 2],
}

/// The player's car, built.
#[derive(Clone, Debug, PartialEq)]
pub struct CarModel {
    pub layout: CarLayout,
    pub lods: [CarLod; 2],
    pub needles: [Mesh; 3],
    pub dial_ticks: Mesh,
    pub telltales: [Mesh; 2],
    /// Triangles of LOD0 as built: exterior (body, parts, glass, lamps, four wheels) and
    /// interior (cabin and steering wheel).
    pub exterior_triangles: usize,
    pub interior_triangles: usize,
}

impl CarModel {
    /// Builds the car `shape` describes for `spec` in palette color 0.
    pub fn build(spec: &VehicleSpec, shape: &ShapeSpec) -> Result<CarModel, String> {
        CarModel::build_painted(spec, shape, 0)
    }

    /// Builds the car in palette entry `paint` (which wraps). Only the body (and so the
    /// inside-out shell) depends on the paint.
    pub fn build_painted(
        spec: &VehicleSpec,
        shape: &ShapeSpec,
        paint: usize,
    ) -> Result<CarModel, String> {
        let popups = shape.front.lights == HeadLights::PopUp;
        let make = |lod: u8, up: bool| {
            shape::build_with(
                shape,
                &BuildOptions {
                    color_index: paint,
                    lod,
                    popups_up: up,
                    paint: None,
                },
            )
            .map_err(|e| format!("shape `{}`: {e:?}", shape.id))
        };
        let offset = Vec3::from(shape::body_offset(spec));
        let mut built = Vec::with_capacity(2);
        for lod in 0..2u8 {
            let down = make(lod, false)?;
            let up = if popups { Some(make(lod, true)?) } else { None };
            built.push((down, up));
        }
        let (car0, up0) = &built[0];
        let layout = CarLayout::new(spec, shape, car0, up0.as_ref().unwrap_or(car0));
        let lods = core::array::from_fn(|k| {
            let (down, up) = &built[k];
            let lit = up.as_ref().unwrap_or(down);
            let body_down = convert(&down.body, offset, Surface::Body);
            let body_up = if up.is_some() {
                convert(&lit.body, offset, Surface::Body)
            } else {
                body_down.clone()
            };
            let lamps = |car: &CarMesh, on: bool, braking: bool| {
                let mut m = convert(&car.lights, offset, Surface::Plain);
                for v in &mut m.vertices {
                    if let Some(s) = lamp_scale(lamp_group(v.emissive), on, braking) {
                        let e = v.emissive;
                        v.emissive = render::emissive([e[0], e[1], e[2]], s);
                    }
                }
                m
            };
            let pivot = Vec3::from(down.cockpit.wheel_pivot);
            CarLod {
                shell_inside: inside_out(&body_down),
                body: [body_down, body_up],
                glass: convert(&down.glass, offset, Surface::Glass),
                interior: convert(&down.interior, offset, Surface::Plain),
                steering_wheel: convert(&down.steering_wheel, -pivot, Surface::Plain),
                wheel: convert(&down.wheels[1], Vec3::ZERO, Surface::Plain),
                lights: [
                    [lamps(down, false, false), lamps(down, false, true)],
                    [lamps(lit, true, false), lamps(lit, true, true)],
                ],
            }
        });
        Ok(CarModel {
            needles: core::array::from_fn(|k| needle_mesh(layout.dials[k].radius)),
            dial_ticks: dial_ticks_mesh(&layout.dials[..layout.dial_count]),
            telltales: core::array::from_fn(|k| {
                telltale_mesh(&layout.dials[..layout.dial_count.min(1)], k)
            }),
            exterior_triangles: car0.exterior_triangles(),
            interior_triangles: car0.interior_triangles(),
            layout,
            lods,
        })
    }

    /// Registers every mesh through `add` (which returns the id it was given) and returns
    /// the ids. A mesh shared by two variants is registered once.
    pub fn register(&self, mut add: impl FnMut(&Mesh) -> MeshId) -> CarMeshes {
        let mut lod_ids = |l: &CarLod| {
            let down = add(&l.body[0]);
            let up = if l.body[1] == l.body[0] {
                down
            } else {
                add(&l.body[1])
            };
            CarLodIds {
                body: [down, up],
                shell_inside: add(&l.shell_inside),
                glass: add(&l.glass),
                interior: add(&l.interior),
                steering_wheel: add(&l.steering_wheel),
                wheel: add(&l.wheel),
                lights: [
                    [add(&l.lights[0][0]), add(&l.lights[0][1])],
                    [add(&l.lights[1][0]), add(&l.lights[1][1])],
                ],
            }
        };
        let lods = [lod_ids(&self.lods[0]), lod_ids(&self.lods[1])];
        CarMeshes {
            lods,
            needles: [
                add(&self.needles[0]),
                add(&self.needles[1]),
                add(&self.needles[2]),
            ],
            dial_ticks: add(&self.dial_ticks),
            telltales: [add(&self.telltales[0]), add(&self.telltales[1])],
        }
    }

    /// Registers only what depends on the paint (each level's bodies and inside-out shell)
    /// through `add`, and returns `base` (the ids of the same car in another paint) with
    /// those replaced.
    pub fn register_paint(
        &self,
        base: &CarMeshes,
        mut add: impl FnMut(&Mesh) -> MeshId,
    ) -> CarMeshes {
        let mut out = *base;
        for (ids, l) in out.lods.iter_mut().zip(&self.lods) {
            let down = add(&l.body[0]);
            let up = if l.body[1] == l.body[0] {
                down
            } else {
                add(&l.body[1])
            };
            ids.body = [down, up];
            ids.shell_inside = add(&l.shell_inside);
        }
        out
    }
}

/// A parked car: `shape` in palette color `paint` (or the `era` paint over it) at level of
/// detail `lod` (1 near, 2 far), as one mesh (body, glass, lamps and four wheels), its origin
/// on the ground midway between the axles, nose toward `-z`. The lamps are built lit (head
/// lamps as the shape has them, tail lamps at [`TAIL_GLOW`]): an instance's `glow` switches
/// them, 0 for a parked car, the lamp glow for an NPC car driving with its lights on.
pub fn parked_mesh(
    shape: &ShapeSpec,
    paint: usize,
    era: Option<&'static str>,
    lod: u8,
) -> Result<Mesh, String> {
    parked_mesh_and_lamps(shape, paint, era, lod).map(|(m, _)| m)
}

/// The head lamps' centers of `shape`'s [`parked_mesh`], left and right, in its frame.
pub fn parked_headlamps(shape: &ShapeSpec) -> Result<[Vec3; 2], String> {
    parked_mesh_and_lamps(shape, 0, None, 2).map(|(_, h)| h)
}

/// [`parked_mesh`] and its head lamps' centers, left and right, in the mesh's frame.
fn parked_mesh_and_lamps(
    shape: &ShapeSpec,
    paint: usize,
    era: Option<&'static str>,
    lod: u8,
) -> Result<(Mesh, [Vec3; 2]), String> {
    let car = shape::build_with(
        shape,
        &BuildOptions {
            color_index: paint,
            lod,
            popups_up: false,
            paint: era,
        },
    )
    .map_err(|e| format!("shape `{}`: {e:?}", shape.id))?;
    let mid = 0.5 * (car.wheel_centers[0][2] + car.wheel_centers[2][2]);
    let offset = Vec3::new(0.0, 0.0, -mid);
    let mut out = convert(&car.body, offset, Surface::Body);
    let mut add = |m: &Mesh| {
        let base = out.vertices.len() as u32;
        out.vertices.extend_from_slice(&m.vertices);
        out.indices.extend(m.indices.iter().map(|i| i + base));
    };
    add(&convert(&car.glass, offset, Surface::Glass));
    let heads = lamp_centers(&car.lights, LampGroup::Head).map(|p| p + offset);
    let mut lamps = convert(&car.lights, offset, Surface::Plain);
    for v in &mut lamps.vertices {
        if let Some(s) = lamp_scale(lamp_group(v.emissive), true, false) {
            let e = v.emissive;
            v.emissive = render::emissive([e[0], e[1], e[2]], s);
        }
    }
    add(&lamps);
    // The right wheel at every hub, turned about `y` on the left.
    let wheel = convert(&car.wheels[1], Vec3::ZERO, Surface::Plain);
    for (k, hub) in car.wheel_centers.iter().enumerate() {
        let left = k % 2 == 0;
        let at = Vec3::from(*hub) + offset;
        let mut w = wheel.clone();
        for v in &mut w.vertices {
            let (p, n) = (Vec3::from(v.pos), Vec3::from(v.normal));
            let (p, n) = if left {
                (Vec3::new(-p.x, p.y, -p.z), Vec3::new(-n.x, n.y, -n.z))
            } else {
                (p, n)
            };
            v.pos = (p + at).to_array();
            v.normal = n.to_array();
        }
        add(&w);
    }
    Ok((out, heads))
}

/// How far a brake overlay's lenses stand off the parked mesh's, at levels of detail 1 and
/// 2, m: enough to win the depth test over the lens under them at the distance each level
/// is drawn.
const BRAKE_OVERLAY_LIFT_M: [f32; 2] = [0.006, 0.06];

/// The brake lamps of `shape`'s [`parked_mesh`] at level of detail `lod` (1 or 2), lit:
/// its tail lenses alone at [`BRAKE_GLOW`], each lifted off its lens along the normal, in
/// the same frame. Drawn over the car (with the car-lamp glow) while it brakes, so the
/// parked mesh needs no braking copy.
pub fn brake_overlay_mesh(shape: &ShapeSpec, lod: u8) -> Result<Mesh, String> {
    let car = shape::build_with(
        shape,
        &BuildOptions {
            color_index: 0,
            lod,
            popups_up: false,
            paint: None,
        },
    )
    .map_err(|e| format!("shape `{}`: {e:?}", shape.id))?;
    let mid = 0.5 * (car.wheel_centers[0][2] + car.wheel_centers[2][2]);
    let lamps = convert(&car.lights, Vec3::new(0.0, 0.0, -mid), Surface::Plain);
    let lift = BRAKE_OVERLAY_LIFT_M[usize::from(lod.clamp(1, 2) - 1)];
    let tail = |v: &Vertex| lamp_group(v.emissive) == LampGroup::Tail;
    let mut out = Mesh::default();
    let mut map = vec![u32::MAX; lamps.vertices.len()];
    for t in lamps.indices.chunks_exact(3) {
        if !t.iter().all(|&i| tail(&lamps.vertices[i as usize])) {
            continue;
        }
        for &i in t {
            if map[i as usize] == u32::MAX {
                let mut v = lamps.vertices[i as usize];
                v.pos = (Vec3::from(v.pos) + Vec3::from(v.normal) * lift).to_array();
                let e = v.emissive;
                v.emissive = render::emissive([e[0], e[1], e[2]], BRAKE_GLOW);
                map[i as usize] = out.vertices.len() as u32;
                out.vertices.push(v);
            }
            out.indices.push(map[i as usize]);
        }
    }
    Ok(out)
}

/// How a shape mesh's uvs map into the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    /// Every vertex in the white tile.
    Plain,
    /// The painted shell and its parts: the grille's back plate gets the egg-crate tile.
    Body,
    /// Windows: one pane of the window tile, tinted light.
    Glass,
}

/// The linear color `vehicle::shape` gives the grille (its sRGB `#141416` through the
/// shape's sRGB fit), which marks grille vertices in the body mesh.
fn grille_color() -> [u8; 4] {
    let lin = |c: u8| {
        let x = c as f32 / 255.0;
        let y = x * (x * (x * 0.305_306_01 + 0.682_171_1) + 0.012_522_878);
        (y * 255.0 + 0.5).clamp(0.0, 255.0) as u8
    };
    [lin(20), lin(20), lin(22), 255]
}

/// Atlas uv rectangle `[u0, v0, u1, v1]` of `tile`'s fraction `[a0, b0, a1, b1]` (of its
/// width and height, v down).
fn sub_rect(tile: Tile, f: [f32; 4]) -> [f32; 4] {
    let [u0, v0, u1, v1] = tile.rect();
    [
        u0 + (u1 - u0) * f[0],
        v0 + (v1 - v0) * f[1],
        u0 + (u1 - u0) * f[2],
        v0 + (v1 - v0) * f[3],
    ]
}

/// The top-left pane of the window tile, inside its frame and mullion.
const GLASS_PANE: [f32; 4] = [0.08, 0.08, 0.46, 0.46];
/// The part of the grille tile one grille spans: all its columns, half its rows.
const GRILLE_PART: [f32; 4] = [0.0, 0.0, 1.0, 0.5];

/// `uv` (in meters, from `lo` to `hi`) to `rect` (`lo` at its left and bottom).
fn map_uv(uv: [f32; 2], lo: [f32; 2], hi: [f32; 2], rect: [f32; 4]) -> [u16; 2] {
    let t = |k: usize| {
        let d = hi[k] - lo[k];
        if d > 1e-6 {
            ((uv[k] - lo[k]) / d).clamp(0.0, 1.0)
        } else {
            0.5
        }
    };
    render::uv_unorm([
        rect[0] + (rect[2] - rect[0]) * t(0),
        rect[3] + (rect[1] - rect[3]) * t(1),
    ])
}

/// Bounds of the uvs of the vertices `pick` selects.
fn uv_bounds(m: &MeshData, pick: impl Fn(usize) -> bool) -> ([f32; 2], [f32; 2]) {
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for (i, uv) in m.uvs.iter().enumerate() {
        if pick(i) {
            for k in 0..2 {
                lo[k] = lo[k].min(uv[k]);
                hi[k] = hi[k].max(uv[k]);
            }
        }
    }
    (lo, hi)
}

/// `m` as a render mesh in the white tile, as built (a pedestrian figure's part).
pub(crate) fn plain_mesh(m: &MeshData) -> Mesh {
    convert(m, Vec3::ZERO, Surface::Plain)
}

/// `m` as a render mesh, every position moved by `offset`.
fn convert(m: &MeshData, offset: Vec3, surface: Surface) -> Mesh {
    let grille = grille_color();
    let is_grille = |i: usize| m.colors[i] == grille && m.uvs[i] != [0.0, 0.0];
    let (glo, ghi) = uv_bounds(m, is_grille);
    let (wlo, whi) = uv_bounds(m, |_| true);
    let glass_rect = sub_rect(Tile::WindowGlass, GLASS_PANE);
    let grille_rect = sub_rect(Tile::GrilleMesh, GRILLE_PART);
    let vertices = (0..m.positions.len())
        .map(|i| {
            let pos = (Vec3::from(m.positions[i]) + offset).to_array();
            let mut v = Vertex {
                pos,
                normal: m.normals[i],
                uv: [0, 0],
                color: m.colors[i],
                emissive: m.emissive[i],
            };
            match surface {
                Surface::Plain => {}
                Surface::Body => {
                    if is_grille(i) {
                        v.uv = map_uv(m.uvs[i], glo, ghi, grille_rect);
                        v.color = GRILLE_TINT;
                    }
                }
                Surface::Glass => {
                    v.uv = map_uv(m.uvs[i], wlo, whi, glass_rect);
                    v.color = glass_tint(m.colors[i]);
                }
            }
            v
        })
        .collect();
    Mesh {
        vertices,
        indices: m.indices.clone(),
    }
}

/// `m` seen from the other side: every triangle reversed and every normal negated.
fn inside_out(m: &Mesh) -> Mesh {
    let mut out = m.clone();
    for v in &mut out.vertices {
        v.normal = v.normal.map(|c| -c);
    }
    for t in out.indices.chunks_exact_mut(3) {
        t.swap(1, 2);
    }
    out
}

/// The shape's dark glass color scaled up to a light tint of the same hue (the window
/// tile is pre-colored; a dark vertex color would turn it black). Alpha is kept.
fn glass_tint(c: [u8; 4]) -> [u8; 4] {
    let max = c[0].max(c[1]).max(c[2]).max(1) as f32;
    // Halfway between the hue at full strength and a neutral of the same strength.
    let f = |x: u8| (0.5 * (x as f32 / max + 1.0) * GLASS_TINT_MAX + 0.5) as u8;
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

/// A tapered needle for a dial of radius `r`: hub at the origin, pointing along `+y`,
/// facing `+z`, glowing white with a red tip.
fn needle_mesh(r: f32) -> Mesh {
    let (w_hub, w_tip) = ((0.06 * r).max(0.003), (0.02 * r).max(0.0012));
    let (tail, tip) = (-0.18 * r, 0.86 * r);
    let mid = tail + (tip - tail) * NEEDLE_TIP_AT;
    let w_mid = w_hub + (w_tip - w_hub) * NEEDLE_TIP_AT;
    let n = [0.0, 0.0, 1.0];
    let mut m = Mesh::default();
    for (rgb, y0, y1, w0, w1) in [
        (NEEDLE_RGB, tail, mid, w_hub, w_mid),
        (NEEDLE_TIP_RGB, mid, tip, w_mid, w_tip),
    ] {
        let glow = render::emissive(rgb, NEEDLE_GLOW);
        let color = [rgb[0], rgb[1], rgb[2], 255];
        let i = m.vertices.len() as u32;
        for p in [[-w0, y0, 0.0], [w0, y0, 0.0], [w1, y1, 0.0], [-w1, y1, 0.0]] {
            m.vertices
                .push(Vertex::new(p, n, color).with_emissive(glow));
        }
        m.indices
            .extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
    }
    m
}

/// Scale ticks round each dial in body space: [`DIAL_TICKS`] short glowing bars over the
/// needle's sweep, the first and last longer.
fn dial_ticks_mesh(dials: &[DialLayout]) -> Mesh {
    let mut m = Mesh::default();
    let glow = render::emissive([235, 232, 220], TICK_GLOW);
    let color = [220, 218, 210, 255];
    for d in dials {
        let right = d.up.cross(d.normal);
        let base = d.center + d.normal * TICK_LIFT_M;
        for k in 0..DIAL_TICKS {
            let f = k as f32 / (DIAL_TICKS - 1) as f32;
            let a = (f - 0.5) * DIAL_SWEEP_DEG.to_radians();
            // Clockwise from 12 o'clock as the driver sees it.
            let (s, c) = a.sin_cos();
            let dir = d.up * c + right * s;
            let side = dir.cross(d.normal);
            let inner = if k == 0 || k == DIAL_TICKS - 1 {
                0.68
            } else {
                0.78
            };
            let hw = (0.025 * d.radius).max(0.0012);
            let (p0, p1) = (
                base + dir * (inner * d.radius),
                base + dir * (0.94 * d.radius),
            );
            let i = m.vertices.len() as u32;
            for p in [
                p0 - side * hw,
                p0 + side * hw,
                p1 + side * hw,
                p1 - side * hw,
            ] {
                m.vertices.push(
                    Vertex::new(p.to_array(), d.normal.to_array(), color).with_emissive(glow),
                );
            }
            m.indices
                .extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
        }
    }
    m
}

/// Tell-tale `k` (0 low beams, 1 high beams) on the first of `dials` (none without one):
/// a glowing square on the face below the hub, body space.
fn telltale_mesh(dials: &[DialLayout], k: usize) -> Mesh {
    let mut m = Mesh::default();
    let Some(d) = dials.first() else {
        return m;
    };
    let rgb = TELLTALE_RGB[k];
    let glow = render::emissive(rgb, TELLTALE_GLOW);
    let color = [rgb[0], rgb[1], rgb[2], 255];
    let right = d.up.cross(d.normal);
    let side = if k == 0 { -1.0 } else { 1.0 };
    let c = d.center + d.normal * TICK_LIFT_M + right * (side * TELLTALE_AT[0] * d.radius)
        - d.up * (TELLTALE_AT[1] * d.radius);
    let h = (TELLTALE_HALF * d.radius).max(0.002);
    for p in [
        c - right * h - d.up * h,
        c + right * h - d.up * h,
        c + right * h + d.up * h,
        c - right * h + d.up * h,
    ] {
        m.vertices
            .push(Vertex::new(p.to_array(), d.normal.to_array(), color).with_emissive(glow));
    }
    m.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vehicles;

    fn model(id: &str) -> (VehicleSpec, CarModel) {
        let spec = vehicles::builtin_spec(id).unwrap();
        let shape = vehicles::builtin_shape(&spec).unwrap();
        let m = CarModel::build(&spec, &shape).unwrap();
        (spec, m)
    }

    /// Every triangle's winding agrees with its normals, so back-face culling keeps the
    /// side the normals face.
    fn assert_wound_like_normals(m: &Mesh, what: &str) {
        assert!(!m.indices.is_empty(), "{what} empty");
        let mut bad = 0;
        for t in m.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].pos));
            let face = (b - a).cross(c - a).normalize_or_zero();
            let n: Vec3 = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].normal))
                .sum();
            if face.dot(n) <= 0.0 {
                bad += 1;
            }
        }
        assert_eq!(
            bad, 0,
            "{what}: {bad} triangles wound against their normals"
        );
    }

    #[test]
    fn both_cars_build_in_body_space() {
        for (id, _) in vehicles::BUILTIN {
            let (spec, m) = model(id);
            let [len, wid, h] = spec.dims_m;
            let ground = -spec.cg_height_m;
            for lod in &m.lods {
                for mesh in lod.body.iter().chain([&lod.glass, &lod.interior]) {
                    assert!(mesh
                        .indices
                        .iter()
                        .all(|&i| (i as usize) < mesh.vertices.len()));
                    for v in &mesh.vertices {
                        let [x, y, z] = v.pos;
                        // Door mirrors stand out of the sides.
                        assert!(x.abs() <= 0.5 * wid + 0.2, "{id} {x}");
                        // The antenna mast stands over the roof.
                        assert!(y >= ground - 1e-3 && y <= ground + h + 0.5, "{id} {y}");
                        // The body's length centered within a share of the axle offset.
                        assert!(z.abs() <= 0.5 * len + 0.6, "{id} {z}");
                    }
                }
                assert_wound_like_normals(&lod.glass, id);
                assert_wound_like_normals(&lod.shell_inside, id);
                assert_eq!(lod.shell_inside.indices.len(), lod.body[0].indices.len());
                assert_wound_like_normals(&lod.lights[1][1], id);
            }
            assert!(
                (8000..=14000).contains(&m.exterior_triangles),
                "{id} {}",
                m.exterior_triangles
            );
            assert!(
                (4000..=8000).contains(&m.interior_triangles),
                "{id} {}",
                m.interior_triangles
            );
            // The wheels sit under the body's ends where the sim has them.
            let sim = spec.wheel_positions();
            for (c, s) in m.layout.wheel_centers.iter().zip(sim) {
                assert!((c.x - s[0]).abs() < 1e-5 && (c.z - s[2]).abs() < 1e-5);
                assert!((c.y - (s[1] + m.layout.wheel_radius)).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn pop_ups_and_lamps_follow_the_switches() {
        let (_, gannet) = model("gannet_gt");
        let (_, wagon) = model("homestead_wagon");
        // Pop-ups: the raised variant is a different body; fixed lamps share one.
        assert_ne!(gannet.lods[0].body[0], gannet.lods[0].body[1]);
        assert_eq!(wagon.lods[0].body[0], wagon.lods[0].body[1]);
        for m in [&gannet, &wagon] {
            let lights = &m.lods[0].lights;
            let peak = |mesh: &Mesh, g: LampGroup| {
                mesh.vertices
                    .iter()
                    .filter(|v| lamp_group_of_lens(v) == g)
                    .map(|v| render::emissive_scale(v.emissive).0)
                    .fold(0.0f32, f32::max)
            };
            // Off: nothing glows. On: head and tail lamps light. Brake: tails brighter.
            assert!(lights[0][0]
                .vertices
                .iter()
                .all(|v| render::emissive_scale(v.emissive).0 == 0.0));
            assert!(peak(&lights[1][0], LampGroup::Head) > 1.0);
            assert!(peak(&lights[1][0], LampGroup::Tail) > 0.5);
            assert_eq!(peak(&lights[0][0], LampGroup::Head), 0.0);
            assert!(peak(&lights[0][1], LampGroup::Tail) > peak(&lights[1][0], LampGroup::Tail));
            assert!(peak(&lights[1][1], LampGroup::Tail) > peak(&lights[1][0], LampGroup::Tail));
            assert_eq!(peak(&lights[0][1], LampGroup::Head), 0.0);
            // Headlamps at the nose, either side, low.
            let [l, r] = m.layout.headlamps;
            assert!(
                l.x < -0.2 && r.x > 0.2 && (l.x + r.x).abs() < 1e-3,
                "{l} {r}"
            );
            let nose = m.layout.wheel_centers[0].z;
            assert!(l.z < nose && l.y < m.layout.belt_y + 0.3, "{l}");
            // Tail lamps at the tail, either side.
            let [tl, tr] = m.layout.taillamps;
            let tail = m.layout.wheel_centers[2].z;
            assert!(
                tl.x < -0.2 && tr.x > 0.2 && (tl.x + tr.x).abs() < 1e-3,
                "{tl} {tr}"
            );
            assert!(tl.z > tail && tl.y < m.layout.belt_y + 0.3, "{tl}");
        }
    }

    /// Lamp group of a vertex by its lens color (the emissive is zero when unlit).
    fn lamp_group_of_lens(v: &Vertex) -> LampGroup {
        let c = v.color;
        if c[0] > 60 && c[1] < 40 {
            LampGroup::Tail
        } else if c[0] > 150 && c[1] > 150 && c[2] > 150 {
            LampGroup::Head
        } else {
            LampGroup::None
        }
    }

    #[test]
    fn glass_and_grille_map_into_their_tiles() {
        let inside = |uv: [u16; 2], tile: Tile| {
            let [u0, v0, u1, v1] = tile.rect();
            let (u, v) = (uv[0] as f32 / 65535.0, uv[1] as f32 / 65535.0);
            u >= u0 - 1e-4 && u <= u1 + 1e-4 && v >= v0 - 1e-4 && v <= v1 + 1e-4
        };
        let (_, wagon) = model("homestead_wagon");
        let lod = &wagon.lods[0];
        assert!(lod
            .glass
            .vertices
            .iter()
            .all(|v| inside(v.uv, Tile::WindowGlass)));
        let grille = lod.body[0]
            .vertices
            .iter()
            .filter(|v| inside(v.uv, Tile::GrilleMesh))
            .count();
        assert!(grille >= 8, "wide grille textured: {grille}");
        // Everything else stays white.
        assert!(lod.interior.vertices.iter().all(|v| v.uv == [0, 0]));
        let white = lod.body[0]
            .vertices
            .iter()
            .filter(|v| v.uv == [0, 0])
            .count();
        assert_eq!(white + grille, lod.body[0].vertices.len());
    }

    #[test]
    fn cockpit_layout_is_inside_the_car() {
        for (id, _) in vehicles::BUILTIN {
            let (spec, m) = model(id);
            let l = &m.layout;
            let [len, wid, h] = spec.dims_m;
            let ground = -spec.cg_height_m;
            let e = l.eye;
            assert!(e.x < 0.0 && e.x.abs() < 0.5 * wid, "{id} {e}");
            assert!(e.y > ground && e.y < ground + h, "{id} {e}");
            assert!(e.z.abs() < 0.5 * len + 0.3, "{id} {e}");
            assert!(
                e.y > l.dash_y + 0.1,
                "{id}: eye {e} over the dash {}",
                l.dash_y
            );
            assert!(l.belt_y > ground + 0.5 && l.belt_y < e.y, "{id}");
            assert!(l.wheel_pivot.z < e.z && l.wheel_pivot.y < e.y);
            assert!(l.dial_count >= 1 && l.dial_count == spec.interior.gauges.len());
        }
    }

    /// From the eye, the share of a hemisphere of view directions above the horizon that
    /// reach the sky past the body (the roof, pillars and frame), by ray casting.
    fn sky_share(m: &CarModel) -> f32 {
        let body = &m.lods[0].body[0];
        let tris: Vec<[Vec3; 3]> = body
            .indices
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]].map(|i| Vec3::from(body.vertices[i as usize].pos)))
            .collect();
        let eye = m.layout.eye;
        let (mut open, mut total) = (0, 0);
        for a in 0..24 {
            for e in 1..8 {
                let (az, el) = (a as f32 * 15f32.to_radians(), e as f32 * 10f32.to_radians());
                let dir = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
                total += 1;
                let hit = tris.iter().any(|t| {
                    // Moller-Trumbore, either side.
                    let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
                    let p = dir.cross(e2);
                    let det = e1.dot(p);
                    if det.abs() < 1e-9 {
                        return false;
                    }
                    let s = eye - t[0];
                    let u = s.dot(p) / det;
                    let q = s.cross(e1);
                    let v = dir.dot(q) / det;
                    u >= 0.0 && v >= 0.0 && u + v <= 1.0 && e2.dot(q) / det > 0.0
                });
                if !hit {
                    open += 1;
                }
            }
        }
        open as f32 / total as f32
    }

    #[test]
    fn open_tops_see_much_more_sky_from_the_seat() {
        let (_, wagon) = model("homestead_wagon");
        let hard = sky_share(&wagon);
        for id in ["seabreeze_convertible", "regent_coupe_cabrio"] {
            let (_, m) = model(id);
            assert!(m.layout.open_top);
            let open = sky_share(&m);
            assert!(open > 0.85 && open > hard + 0.4, "{id}: {open} vs {hard}");
            // The cabin walls are the body inside out: no roof over the seat either.
            assert_eq!(
                m.lods[0].shell_inside.indices.len(),
                m.lods[0].body[0].indices.len()
            );
        }
    }

    #[test]
    fn left_steer_turns_the_wheel_counter_clockwise() {
        let (spec, m) = model("gannet_gt");
        let l = &m.layout;
        // The 12 o'clock point of the rim (up, square to the axis).
        let up = (Vec3::Y - l.wheel_axis * l.wheel_axis.y).normalize();
        let top = |steer: f32| l.steering_wheel(steer).transform_vector3(up);
        let lock = spec.steering.max_deg.to_radians();
        // Left (negative) a little: the top swings left as the driver sees it.
        assert!(top(-0.05).x < -0.1, "{}", top(-0.05));
        assert!(top(0.05).x > 0.1);
        // Full lock is 450°: the rim's top ends a quarter turn round, pointing left.
        let full = top(-lock);
        assert!(full.x < -0.99, "{full}");
        assert!(
            (l.steering_wheel(0.0).transform_point3(Vec3::ZERO) - l.wheel_pivot).length() < 1e-6
        );
    }

    #[test]
    fn needles_read_speed_and_rpm() {
        let (_, m) = model("gannet_gt");
        let [speedo, tach, _] = m.layout.dials;
        assert_eq!(speedo.reading, DialReading::Speed(0.0, 160.0, MPH_PER_MPS));
        assert_eq!(tach.reading, DialReading::Rpm(0.0, 8000.0));
        let tip = |d: &DialLayout, f: f32| {
            let right = d.up.cross(d.normal);
            let p = d.needle(f).transform_vector3(Vec3::Y);
            (p.dot(right), p.dot(d.up))
        };
        // Zero at lower left, mid-scale straight up, full at lower right.
        let (x0, y0) = tip(&speedo, 0.0);
        assert!(x0 < -0.5 && y0 < 0.0, "{x0} {y0}");
        let (x5, y5) = tip(&speedo, 0.5);
        assert!(x5.abs() < 1e-5 && y5 > 0.99);
        let (x1, y1) = tip(&speedo, 1.0);
        assert!(x1 > 0.5 && y1 < 0.0);
        // 80 mph is mid-scale on a 160 mph dial; 4000 rpm on an 8000 one.
        assert!((speedo.reading.fraction(80.0 / MPH_PER_MPS, 0.0) - 0.5).abs() < 1e-5);
        assert!((tach.reading.fraction(0.0, 4000.0) - 0.5).abs() < 1e-6);
        assert_eq!(tach.reading.fraction(0.0, 99_000.0), 1.0);
        assert_eq!(
            speedo.reading.fraction(-10.0, 0.0),
            speedo.reading.fraction(10.0, 0.0)
        );
        // The needle stands off the face toward the driver.
        let hub = speedo.needle(0.3).transform_point3(Vec3::ZERO);
        assert!((hub - speedo.center).dot(speedo.normal) > 0.0);
    }

    #[test]
    fn registering_shares_identical_meshes() {
        let (_, wagon) = model("homestead_wagon");
        let mut n = 0;
        let ids = wagon.register(|_| {
            n += 1;
            MeshId(n - 1)
        });
        // Per LOD: one body (no pop-ups), its inside, glass, interior, wheel, steering
        // wheel, 4 lamp states; then 3 needles, the ticks and 2 tell-tales.
        assert_eq!(n, 2 * 10 + 6);
        assert_eq!(ids.lods[0].body[0], ids.lods[0].body[1]);
        let (_, gannet) = model("gannet_gt");
        let mut n = 0;
        let ids = gannet.register(|_| {
            n += 1;
            MeshId(n - 1)
        });
        assert_eq!(n, 2 * 11 + 6);
        assert_ne!(ids.lods[0].body[0], ids.lods[0].body[1]);
    }
}
