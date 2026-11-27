//! The Vehicle page's studio: the car being browsed, in front of a bright
//! backdrop, in the page's stage (a screen rectangle beside the menu panel). The world is not
//! drawn under the page; the car turns only when the player turns it (the look stick, or the
//! mouse's look drag).
//!
//! **A studio, not the street.** Under the page the frame is the studio alone: a level camera
//! at the eye, a fixed noon sun from over the viewer's left shoulder, the backdrop (an
//! emissive sphere round the eye, [`backdrop_mesh`]) and the car. The stage
//! rectangle is turned into a view-space box at [`DISTANCE_M`], the car (its ground plane,
//! tilted [`TILT_RAD`] toward the eye) scaled to fill it, and the result carried to the
//! world by the inverse view. Nothing in it casts or receives a sun shadow or takes a street
//! lamp. The cost is [`MAX_INSTANCES`] instances while the page is open and none otherwise.

use glam::{Mat4, Vec3};
use render::{Mesh, MeshId, MeshInstance, Vertex};

use crate::car::{CarLayout, CarMeshes};
use crate::drive::CarGlow;

/// How far in front of the eye the model's center sits, m: as near as keeps its near edge
/// past the cameras' 0.25 m near plane.
pub const DISTANCE_M: f32 = 0.5;
/// The car's ground plane tilts this far toward the eye: the car is seen from a little above.
pub const TILT_RAD: f32 = 0.33;
/// Turn rate at full stick, rad/s (a lap in about 2.5 s); mouse look turns it as far as the
/// same motion turns the camera.
pub const TURN_RAD_PER_S: f32 = 2.5;
/// The turn the page opens on: a three-quarter view from the front left.
pub const STILL_SPIN_RAD: f32 = 2.45;
/// The studio camera's vertical field of view, degrees: long enough that the car reads as a
/// product shot, not a wide-angle one.
pub const STUDIO_FOV_DEG: f32 = 34.0;
/// Toward the studio's sun (world space, `+z` behind the level camera, which looks down
/// `-z`): high over the viewer's left shoulder.
pub const STUDIO_SUN: [f32; 3] = [-0.45, 0.78, 0.43];
/// The backdrop sphere's radius round the eye, m: past the model, inside the far plane.
pub const BACKDROP_M: f32 = 3.0;
/// Post-exposure brightness of a full-white backdrop vertex (tonemapper input, 0.18 mid
/// grey): the backdrop's colors below land near their sRGB values.
pub const BACKDROP_EXPOSED: f32 = 0.75;
/// The backdrop's colors, sRGB, by elevation: overhead, at the horizon, and underfoot. A
/// deep blue sky over a bright cyan horizon and a lighter floor.
const BACKDROP_TOP: [u8; 3] = [24, 64, 178];
const BACKDROP_HORIZON: [u8; 3] = [70, 186, 250];
const BACKDROP_FLOOR: [u8; 3] = [120, 206, 246];
const BACKDROP_RINGS: u32 = 24;
const BACKDROP_SEGMENTS: u32 = 32;
/// Share of the stage the car's turning circle may fill, across and up.
const FILL: [f32; 2] = [0.96, 0.86];
/// Instances [`push`] adds at most: body, glass, lamps, four wheels, the interior and
/// steering wheel and the driver's parts; and the backdrop.
pub const MAX_INSTANCES: usize = 10 + traffic::figure::PARTS;

/// The backdrop: a sphere of radius 1 round its origin seen from inside, black (it takes no
/// light) and emissive (scale 1) from [`BACKDROP_FLOOR`] below through [`BACKDROP_HORIZON`]
/// to [`BACKDROP_TOP`] overhead.
pub fn backdrop_mesh() -> Mesh {
    let linear = |c: [u8; 3]| c.map(|v| (v as f32 / 255.0).powf(2.2));
    let (top, horizon, floor) = (
        linear(BACKDROP_TOP),
        linear(BACKDROP_HORIZON),
        linear(BACKDROP_FLOOR),
    );
    let mut m = Mesh::default();
    let (rings, segs) = (BACKDROP_RINGS, BACKDROP_SEGMENTS);
    for i in 0..=rings {
        // Elevation from straight down to straight up.
        let el = (i as f32 / rings as f32 - 0.5) * core::f32::consts::PI;
        let (sy, cy) = el.sin_cos();
        // Up: the horizon's cyan deepening to the top's blue; down, quickly to the floor.
        let (a, b, t) = if sy >= 0.0 {
            (horizon, top, sy.sqrt())
        } else {
            (horizon, floor, (-sy * 3.0).min(1.0))
        };
        let c: [f32; 3] = core::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
        let e = render::emissive(c.map(|v| (v * 255.0).round() as u8), 1.0);
        for j in 0..=segs {
            let az = j as f32 / segs as f32 * core::f32::consts::TAU;
            let p = [cy * az.cos(), sy, cy * az.sin()];
            let inward = p.map(|v| -v);
            m.vertices
                .push(Vertex::new(p, inward, [0, 0, 0, 255]).with_emissive(e));
        }
    }
    let row = segs + 1;
    for i in 0..rings {
        for j in 0..segs {
            let (a, b) = (i * row + j, i * row + j + 1);
            let (c, d) = (a + row, b + row);
            // Counter-clockwise seen from the center.
            m.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    m
}

/// What [`push`] draws: a car's registered meshes and layout.
#[derive(Clone, Copy, Debug)]
pub struct Model<'a> {
    pub car: &'a CarMeshes,
    pub layout: &'a CarLayout,
    /// The player's driver, at the wheel.
    pub driver: Option<&'a crate::driver::Driver>,
}

/// The car's ground frame (under the middle of its wheelbase, `+y` up, scaled from meters of
/// car) to world, so a car of `layout` fills `stage` (normalized device coordinates
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

/// The circle a car of `layout` sweeps as it turns (half its diagonal), m.
fn car_radius(layout: &CarLayout) -> f32 {
    let [l, w, _] = layout.dims;
    0.5 * (l * l + w * w).sqrt()
}

/// Body space to the ground frame: the car stands on its plane, the middle of its
/// wheelbase over the center.
fn car_on_table(layout: &CarLayout) -> Mat4 {
    let c = &layout.wheel_centers;
    let mid = 0.25 * (c[0] + c[1] + c[2] + c[3]);
    let ground = mid.y - layout.wheel_radius;
    Mat4::from_translation(-Vec3::new(mid.x, ground, mid.z))
}

/// Appends the car's instances with its ground frame at `table` (from
/// [`table_to_world`]): the car at rest (body with the lamps down and off, glass, lamps,
/// four wheels, the interior and steering wheel seen through the glass, the driver at the
/// wheel), with `fill`
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
}

/// Appends the backdrop round `eye`, its emissive times `glow`.
pub fn push_backdrop(backdrop: MeshId, eye: Vec3, glow: f32, out: &mut Vec<MeshInstance>) {
    let m = Mat4::from_translation(eye) * Mat4::from_scale(Vec3::splat(BACKDROP_M));
    out.push(MeshInstance {
        glow,
        casts_shadow: false,
        receives_shadow: false,
        ..MeshInstance::new(backdrop, m.to_cols_array_2d())
    });
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
    fn the_backdrop_faces_in_and_is_bright_blue() {
        let m = backdrop_mesh();
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        for t in m.indices.chunks(3) {
            let p = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].pos));
            let face = (p[1] - p[0]).cross(p[2] - p[0]);
            if face.length() < 1e-6 {
                continue; // At a pole.
            }
            // Counter-clockwise seen from the center, which every normal points to.
            assert!(face.dot(-p[0]) > 0.0, "{p:?}");
            let n = Vec3::from(m.vertices[t[0] as usize].normal);
            assert!(n.dot(-p[0]) > 0.0);
        }
        for v in &m.vertices {
            // Unlit: black, all of it emissive, blue the strongest channel above the floor.
            assert_eq!(v.color[..3], [0, 0, 0]);
            let e = v.emissive;
            assert!(e[3] > 0);
            if v.pos[1] > 0.0 {
                assert!(e[2] > e[0] && e[2] >= e[1], "{e:?}");
            }
        }
    }
}
