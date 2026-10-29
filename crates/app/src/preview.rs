//! The Vehicle page's turntable: the car being browsed, slowly rotating on a plain disc in
//! the page's stage (a screen rectangle beside the menu panel), drawn in the main pass from
//! its own camera.
//!
//! **Its own camera, in the main pass.** The renderer has one camera per frame, so the
//! preview is a scale model placed in the world just in front of the frame's camera, where
//! that camera sees it exactly as the turntable camera would see the full-size car: the
//! stage rectangle is turned into a view-space box at [`DISTANCE_M`], the car (on its disc,
//! tilted [`TILT_RAD`] toward the eye) scaled to fill it, and the result carried to the
//! world by the inverse view. At that distance it is in front of everything else the chase
//! cameras see (the first-person ones would put the cabin round it, and two of the same car
//! read oddly anyway, so the app draws no player car, nor its lamps, under the page). It is
//! lit like everything else (the current sun, sky ambient and exposure) plus, after dark,
//! the cabin's fill light (`Sky::cabin_light`) so a car is never a silhouette; it takes no
//! street lamps and neither casts nor receives a sun shadow (the world's shadow at the eye
//! is not the turntable's). Traffic and props within [`CLEAR_M`] of the eye are left out
//! while it shows. No render pass or uniform; the one renderer change is the per-instance
//! `receives_shadow` bit. The cost is [`MAX_INSTANCES`] instances while the page is open
//! and none otherwise.

use glam::{Mat4, Vec3};
use render::{Mesh, MeshId, MeshInstance, Vertex};

use crate::car::{CarLayout, CarMeshes};
use crate::drive::CarGlow;

/// How far in front of the eye the model's center sits, m: as near as keeps its near edge
/// past the cameras' 0.25 m near plane.
pub const DISTANCE_M: f32 = 0.5;
/// While it shows, the app draws no traffic or prop instance whose origin is this close to
/// the eye (m), so nothing that moves or stands nearby can cut into the model.
pub const CLEAR_M: f32 = 4.0;
/// The turntable tilts its top this far toward the eye: the car is seen from a little above.
pub const TILT_RAD: f32 = 0.33;
/// Turn rate, rad/s: a lap in about 18 s.
pub const SPIN_RAD_PER_S: f32 = 0.35;
/// The turn a still scene shows: a three-quarter view from the front left.
pub const STILL_SPIN_RAD: f32 = 2.45;
/// Share of the stage the car's turning circle may fill, across and up (the disc's rim
/// may reach a little past it).
const FILL: [f32; 2] = [0.96, 0.86];
/// Disc radius past the car's corners, and its thickness, m (full scale).
const DISC_MARGIN_M: f32 = 0.2;
const DISC_THICKNESS_M: f32 = 0.12;
/// Disc color, linear: a plain light grey with a darker rim.
const DISC_TOP: [u8; 4] = [120, 122, 126, 255];
const DISC_RIM: [u8; 4] = [46, 48, 52, 255];
const DISC_SEGMENTS: u32 = 64;

/// Instances [`push`] adds at most: body, glass, lamps, four wheels, the interior and
/// steering wheel, the disc and the driver's parts.
pub const MAX_INSTANCES: usize = 10 + traffic::figure::PARTS;

/// The disc: radius 1, its top at `y = 0`, [`DISC_THICKNESS_M`] thick; scaled across to
/// each car.
pub fn turntable_mesh() -> Mesh {
    let v = Vertex::new;
    let mut m = Mesh::default();
    let n = DISC_SEGMENTS;
    let dir = |k: u32| {
        let a = k as f32 * core::f32::consts::TAU / n as f32;
        (a.cos(), a.sin())
    };
    // Top: a fan round the center.
    m.vertices
        .push(v([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], DISC_TOP));
    for k in 0..n {
        let (c, s) = dir(k);
        m.vertices.push(v([c, 0.0, s], [0.0, 1.0, 0.0], DISC_TOP));
    }
    for k in 0..n {
        // Counter-clockwise seen from above.
        m.indices.extend_from_slice(&[0, 1 + (k + 1) % n, 1 + k]);
    }
    // Rim: a band of quads facing out.
    let base = m.vertices.len() as u32;
    for k in 0..n {
        let (c, s) = dir(k);
        m.vertices.push(v([c, 0.0, s], [c, 0.0, s], DISC_RIM));
        m.vertices
            .push(v([c, -DISC_THICKNESS_M, s], [c, 0.0, s], DISC_RIM));
    }
    for k in 0..n {
        let (a, b) = (base + 2 * k, base + 2 * ((k + 1) % n));
        m.indices.extend_from_slice(&[a, b, a + 1, b, b + 1, a + 1]);
    }
    m
}

/// What [`push`] draws: a car's registered meshes and layout, and the disc.
#[derive(Clone, Copy, Debug)]
pub struct Model<'a> {
    pub car: &'a CarMeshes,
    pub layout: &'a CarLayout,
    pub turntable: MeshId,
    /// The player's driver, at the wheel.
    pub driver: Option<&'a crate::driver::Driver>,
}

/// The disc's frame (its top's center, `+y` up, scaled from meters of car) to world, so the
/// turntable carrying a car of `layout` fills `stage` (normalized device coordinates
/// `[x0, y0, x1, y1]`, y up) as the camera `view`/`proj` sees it, turned `spin` radians.
/// `None` for an empty stage.
pub fn table_to_world(
    view: Mat4,
    proj: Mat4,
    stage: [f32; 4],
    layout: &CarLayout,
    spin: f32,
) -> Option<Mat4> {
    let [x0, y0, x1, y1] = stage;
    let (sx, sy) = (proj.x_axis.x, proj.y_axis.y);
    if !(x1 > x0 && y1 > y0 && sx > 0.0 && sy > 0.0) {
        return None;
    }
    let d = DISTANCE_M;
    // The stage at distance d, view space.
    let half = [0.5 * (x1 - x0) * d / sx, 0.5 * (y1 - y0) * d / sy];
    let center = [0.5 * (x0 + x1) * d / sx, 0.5 * (y0 + y1) * d / sy];
    let radius = car_radius(layout);
    let height = layout.dims[2];
    let (st, ct) = TILT_RAD.sin_cos();
    // The car's turning circle's near edge is a·cos(tilt) nearer the eye than its center
    // (a its radius there) and looks d / (d - a·cos(tilt)) times bigger. Across, the
    // circle's diameter fits exactly; up, its far and near edges and the roof, nearly.
    let across = FILL[0] * half[0];
    let scale_x = across * d / (radius * (d + across * ct));
    let scale_y = FILL[1] * 2.0 * half[1] / (2.0 * radius * st + height * ct);
    let scale_y = scale_y * (d - radius * scale_y * ct) / d;
    let scale = scale_x.min(scale_y);
    // Facing down the ray from the eye to the stage's center (as a camera aimed at it
    // would see it, not skewed by the stage being off to one side), centered between the
    // turning circle's near edge and the roof.
    let center = Vec3::new(center[0], center[1], -d);
    let ray = center.normalize();
    let aim = Mat4::from_rotation_y((-ray.x).atan2(-ray.z))
        * Mat4::from_rotation_x(ray.y.clamp(-1.0, 1.0).asin());
    let lift = 0.5 * scale * height * ct;
    let at = center - aim.transform_vector3(Vec3::Y * lift);
    let model = Mat4::from_translation(at)
        * aim
        * Mat4::from_rotation_x(TILT_RAD)
        * Mat4::from_rotation_y(spin)
        * Mat4::from_scale(Vec3::splat(scale));
    Some(view.inverse() * model)
}

/// The circle a car of `layout` sweeps on the disc (half its diagonal), m.
fn car_radius(layout: &CarLayout) -> f32 {
    let [l, w, _] = layout.dims;
    0.5 * (l * l + w * w).sqrt()
}

/// The disc under a car of `layout`: past its corners by [`DISC_MARGIN_M`], m.
fn disc_radius(layout: &CarLayout) -> f32 {
    car_radius(layout) + DISC_MARGIN_M
}

/// Body space to the disc's frame: the car stands on the disc's top, the middle of its
/// wheelbase over the center.
fn car_on_table(layout: &CarLayout) -> Mat4 {
    let c = &layout.wheel_centers;
    let mid = 0.25 * (c[0] + c[1] + c[2] + c[3]);
    let ground = mid.y - layout.wheel_radius;
    Mat4::from_translation(-Vec3::new(mid.x, ground, mid.z))
}

/// Appends the turntable's instances with the disc's frame at `table` (from
/// [`table_to_world`]): the car at rest (body with the lamps down and off, glass, lamps,
/// four wheels, the interior and steering wheel seen through the glass, the driver at the
/// wheel) and the disc, with `fill`
/// (0 to 1) of the cabin light. None cast a shadow or take street lamps. At most
/// [`MAX_INSTANCES`].
pub fn push(table: Mat4, model: &Model, glow: CarGlow, fill: f32, out: &mut Vec<MeshInstance>) {
    let instance = |mesh: MeshId, m: Mat4, glow: f32| MeshInstance {
        glow,
        cabin: fill,
        casts_shadow: false,
        receives_shadow: false,
        ..MeshInstance::new(mesh, m.to_cols_array_2d())
    };
    let lod = &model.car.lods[0];
    let layout = model.layout;
    let body = table * car_on_table(layout);
    out.push(instance(lod.body[0], body, 1.0));
    out.push(MeshInstance {
        glass: true,
        ..instance(lod.glass, body, 1.0)
    });
    out.push(instance(lod.lights[0][0], body, glow.lamps));
    for (k, hub) in layout.wheel_centers.iter().enumerate() {
        let side = if k % 2 == 0 {
            Mat4::from_rotation_y(core::f32::consts::PI)
        } else {
            Mat4::IDENTITY
        };
        out.push(instance(
            lod.wheel,
            body * Mat4::from_translation(*hub) * side,
            1.0,
        ));
    }
    out.push(instance(lod.interior, body, glow.instruments));
    let wheel = body * layout.steering_wheel(0.0);
    out.push(instance(lod.steering_wheel, wheel, glow.instruments));
    if let Some(d) = model.driver {
        let mut parts = [Mat4::IDENTITY; traffic::figure::PARTS];
        crate::driver::pose(&d.skeleton, &layout.seating, 0.0, 0.0, 0.0, &mut parts);
        for (k, m) in parts.iter().enumerate() {
            out.push(instance(d.ids[k], body * *m, 1.0));
        }
    }
    let r = disc_radius(layout);
    let disc = table * Mat4::from_scale(Vec3::new(r, 1.0, r));
    out.push(instance(model.turntable, disc, 1.0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::camera::rh::proj::directx;

    fn regent() -> CarLayout {
        let spec = crate::vehicles::builtin_spec("regent_coupe_cabrio").unwrap();
        let shape = crate::vehicles::builtin_shape(&spec).unwrap();
        crate::car::CarModel::build(&spec, &shape).unwrap().layout
    }

    #[test]
    fn the_turntable_fills_its_stage_in_front_of_everything() {
        let layout = regent();
        let view = glam::camera::rh::view::look_to_mat4(
            Vec3::new(10.0, 3.0, -4.0),
            Vec3::new(0.3, -0.2, -1.0),
            Vec3::Y,
        );
        let proj = directx::perspective(62f32.to_radians(), 16.0 / 9.0, 0.25, 3000.0);
        let stage = [0.1, -0.6, 0.9, 0.5];
        for spin in [0.0, 1.0, STILL_SPIN_RAD, 4.0] {
            let table = table_to_world(view, proj, stage, &layout, spin).unwrap();
            let clip = proj * view * table;
            let r = car_radius(&layout);
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            // The car's turning circle and its roof corners.
            let roof = layout.dims[2];
            let mut points = Vec::new();
            for k in 0..32 {
                let a = k as f32 * core::f32::consts::TAU / 32.0;
                points.push(Vec3::new(r * a.cos(), 0.0, r * a.sin()));
            }
            for (x, z) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let [l, w, _] = layout.dims;
                points.push(Vec3::new(0.5 * w * x, roof, 0.5 * l * z));
            }
            for p in points {
                let c = clip * p.extend(1.0);
                let ndc = c.truncate() / c.w;
                for k in 0..3 {
                    lo[k] = lo[k].min(ndc[k]);
                    hi[k] = hi[k].max(ndc[k]);
                }
                // Well within CLEAR_M and past the 0.25 m near plane.
                let eye_dist = c.w;
                assert!((0.26..0.8).contains(&eye_dist), "{eye_dist}");
            }
            assert!(
                lo[0] >= stage[0] - 1e-3 && hi[0] <= stage[2] + 1e-3,
                "{lo:?} {hi:?}"
            );
            assert!(
                lo[1] >= stage[1] - 0.02 && hi[1] <= stage[3] + 0.02,
                "{lo:?} {hi:?}"
            );
            // It fills most of the stage one way or the other.
            let fill = ((hi[0] - lo[0]) / (stage[2] - stage[0]))
                .max((hi[1] - lo[1]) / (stage[3] - stage[1]));
            assert!(fill > 0.65, "{fill}");
        }
        assert!(table_to_world(view, proj, [0.5, 0.0, 0.5, 1.0], &layout, 0.0).is_none());
    }

    #[test]
    fn the_disc_is_closed_and_faces_out() {
        let m = turntable_mesh();
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        // Every triangle's winding agrees with its normal (counter-clockwise from outside).
        for t in m.indices.chunks(3) {
            let p = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].pos));
            let n = Vec3::from(m.vertices[t[0] as usize].normal);
            let face = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(face.dot(n) > 0.0, "{p:?}");
        }
    }
}
