//! `vehicle::shape`: a parametric car mesh generator.
//!
//! A [`ShapeSpec`] (RON file `assets/vehicles/*.shape.ron`, layout in `docs/VEHICLES.md`)
//! is a side silhouette, a plan outline, a beltline and a few cross-section numbers, plus
//! choices for lamps, grille, bumpers, rims and trim. [`build`] lofts it into an
//! early-2000s-console-density body (3–6k triangles with wheels and parts, 1.5–3k for the
//! cabin) as plain arrays the renderer can upload.
//!
//! Model space is the body-space axes (`+x` right, `+y` up, `-z` forward) with the origin
//! on the ground, on the centerline, under the front axle. [`body_offset`] moves it to
//! the center-of-mass origin the sim uses.
//!
//! Everything here is deterministic: no platform math, no hash maps, the same spec gives
//! bit-identical meshes everywhere.

mod body;
mod interior;
mod math;
mod mesh;
mod params;
mod parts;
mod roof;
mod wheel;

#[cfg(all(test, feature = "serde"))]
mod tests;

pub use mesh::MeshData;
pub use params::{
    Antenna, BadgeShape, Bumper, Detail, Exhaust, Front, Glass, Grille, Handles, HeadLights,
    InteriorShape, Mirrors, Parts, Rear, Rims, Roof, RoofKind, Seams, Section, ShapeError,
    ShapeSpec, Spoiler, TailLights, Top, Wheels,
};

use crate::VehicleSpec;
use mesh::{emissive, Mat, Uv};
use params::parse_hex;

/// One instrument dial: the app draws its face markings and needle at runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dial {
    pub center: [f32; 3],
    /// Unit normal of the dial face, toward the driver.
    pub normal: [f32; 3],
    /// Unit "12 o'clock" direction in the face plane.
    pub up: [f32; 3],
    pub radius: f32,
}

/// Where the app puts the cockpit camera and what it animates, in model space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cockpit {
    /// Driver eye position.
    pub eye: [f32; 3],
    /// Steering wheel hub center; rotate `CarMesh::steering_wheel` about it.
    pub wheel_pivot: [f32; 3],
    /// Unit steering axis (wheel face normal, toward the driver). A positive rotation about
    /// it turns the wheel counter-clockwise as the driver sees it.
    pub wheel_axis: [f32; 3],
    /// Radio display quad for the station text: bottom-left, bottom-right, top-right,
    /// top-left as the driver sees it.
    pub radio_display: [[f32; 3]; 4],
    /// The first `dial_count` entries are real.
    pub dials: [Dial; 3],
    pub dial_count: u8,
}

/// A built car. All meshes are in model space except `wheels`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CarMesh {
    /// Painted shell plus every opaque exterior part (bumpers, grille, mirrors, trim).
    pub body: MeshData,
    /// One wheel each for front-left, front-right, rear-left, rear-right (identical).
    /// Local frame: hub center at the origin, axle along `x`, outer face toward `+x`; left
    /// wheels are turned 180° about `y`. The sim positions and spins them.
    pub wheels: [MeshData; 4],
    /// Windows; colors carry alpha for blending. Single-sided, facing out.
    pub glass: MeshData,
    /// Lamp lenses with the emissive channel set (head, tail, turn, badges).
    pub lights: MeshData,
    /// The cabin except the steering wheel.
    pub interior: MeshData,
    /// Rim, spokes and hub; rotates about `cockpit.wheel_pivot` / `wheel_axis`.
    pub steering_wheel: MeshData,
    /// Wheel hub centers, model space, in the order of `wheels`.
    pub wheel_centers: [[f32; 3]; 4],
    pub wheel_radius: f32,
    pub cockpit: Cockpit,
}

impl CarMesh {
    /// Body, parts, glass, lights and all four wheels.
    pub fn exterior_triangles(&self) -> usize {
        self.body.triangle_count()
            + self.glass.triangle_count()
            + self.lights.triangle_count()
            + self
                .wheels
                .iter()
                .map(MeshData::triangle_count)
                .sum::<usize>()
    }

    /// Cabin including the steering wheel.
    pub fn interior_triangles(&self) -> usize {
        self.interior.triangle_count() + self.steering_wheel.triangle_count()
    }

    /// Every mesh, for iteration (wheels once each).
    pub fn meshes(&self) -> [&MeshData; 9] {
        [
            &self.body,
            &self.glass,
            &self.lights,
            &self.interior,
            &self.steering_wheel,
            &self.wheels[0],
            &self.wheels[1],
            &self.wheels[2],
            &self.wheels[3],
        ]
    }
}

/// Offset to add to a model-space point to get the sim's body space (origin at the
/// center of mass) for `spec`.
pub fn body_offset(spec: &VehicleSpec) -> [f32; 3] {
    [0.0, -spec.cg_height_m, -spec.cg_fore_aft * spec.wheelbase_m]
}

/// Build choices beyond the spec.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildOptions {
    /// Palette entry for the paint (wraps).
    pub color_index: usize,
    /// 0 is full detail; 1 drops seams, interior detail, small parts and turns rims into
    /// discs (≤ 40% of the triangles).
    pub lod: u8,
    /// Pop-up headlamps raised.
    pub popups_up: bool,
}

/// Builds the full-detail car in palette color `color_index`.
pub fn build(spec: &ShapeSpec, color_index: usize) -> Result<CarMesh, ShapeError> {
    build_with(
        spec,
        &BuildOptions {
            color_index,
            ..BuildOptions::default()
        },
    )
}

/// Builds level of detail `level` (0 or 1).
pub fn build_lod(spec: &ShapeSpec, color_index: usize, level: u8) -> Result<CarMesh, ShapeError> {
    build_with(
        spec,
        &BuildOptions {
            color_index,
            lod: level,
            ..BuildOptions::default()
        },
    )
}

/// Validates `spec` and builds it.
pub fn build_with(spec: &ShapeSpec, opts: &BuildOptions) -> Result<CarMesh, ShapeError> {
    spec.validate()?;
    let lod = opts.lod.min(1);
    let mats = Materials::new(spec, opts.color_index);
    let b = body::Body::new(spec, lod, opts.color_index);

    let mut shell = mesh::Soup::new(38.0);
    let mut glass = mesh::Soup::new(30.0);
    b.emit(&mats, &mut shell, &mut glass);

    let mut p = parts::Parts::new(&b, &mats, opts.popups_up);
    p.build();
    shell.extend(p.body);

    let mut open = roof::OpenTop::new(&b, &mats);
    open.build();
    shell.extend(open.body);
    glass.extend(open.glass);

    let mut int = interior::Interior::new(&b, &mats);
    int.build();

    let w = &spec.wheels;
    let wheel = wheel::build(w, spec.parts.rims, spec.parts.hubcaps, lod, &mats).finish();
    let (x, y, wb) = (w.track * 0.5, w.radius, w.rear_x - w.front_x);
    Ok(CarMesh {
        body: shell.finish(),
        wheels: [wheel.clone(), wheel.clone(), wheel.clone(), wheel],
        glass: glass.finish(),
        lights: p.lights.finish(),
        interior: int.soup.finish(),
        steering_wheel: int.wheel.finish(),
        wheel_centers: [[-x, y, 0.0], [x, y, 0.0], [-x, y, wb], [x, y, wb]],
        wheel_radius: w.radius,
        cockpit: int.cockpit,
    })
}

/// sRGB byte to linear byte (polynomial fit of the sRGB curve, deterministic).
fn lin(c: u8) -> u8 {
    let x = c as f32 / 255.0;
    let y = x * (x * (x * 0.305_306_01 + 0.682_171_1) + 0.012_522_878);
    math::clamp(y * 255.0 + 0.5, 0.0, 255.0) as u8
}

fn rgb(hex: [u8; 3]) -> [u8; 4] {
    [lin(hex[0]), lin(hex[1]), lin(hex[2]), 255]
}

fn hex(s: &str) -> [u8; 4] {
    rgb(parse_hex(s).unwrap_or([128, 128, 128]))
}

/// Every material a car uses, resolved to linear colors.
pub(crate) struct Materials {
    pub paint: Mat,
    pub seam: Mat,
    pub chassis: Mat,
    pub liner: Mat,
    pub chrome: Mat,
    pub trim: Mat,
    pub rubber: Mat,
    pub glass: Mat,
    pub tire: Mat,
    pub rim: Mat,
    pub steel: Mat,
    pub headlamp: Mat,
    pub tail: Mat,
    pub amber: Mat,
    pub reverse: Mat,
    pub grille: Mat,
    pub mirror: Mat,
    pub plate: Mat,
    pub accent: Mat,
    pub wood: Mat,
    pub wood_trim: Mat,
    pub dash: Mat,
    pub cluster: Mat,
    pub dial: Mat,
    pub display: Mat,
    pub vent: Mat,
    pub radio: Mat,
    pub seat: Mat,
    pub seat_bolster: Mat,
    pub carpet: Mat,
    pub headliner: Mat,
    pub int_trim: Mat,
    pub accent_int: Mat,
    pub door_card: Mat,
    pub wheel_rim: Mat,
    pub wheel_spoke: Mat,
}

impl Materials {
    fn new(spec: &ShapeSpec, color_index: usize) -> Materials {
        let paint_hex = &spec.palette[color_index % spec.palette.len()];
        let paint = Mat::new(hex(paint_hex));
        let s = |h: [u8; 3]| Mat::new(rgb(h));
        let ic = &spec.interior.colors;
        let ic = |k: usize| Mat::new(hex(&ic[k]));
        let disp = parse_hex(&spec.interior.display).unwrap_or([60, 255, 120]);
        let disp_lin = rgb(disp);
        let disp_e = [disp_lin[0], disp_lin[1], disp_lin[2]];
        let trim = s([40, 40, 43]);
        Materials {
            paint,
            seam: paint.shade(0.25).group(1),
            chassis: s([30, 30, 32]),
            liner: s([18, 18, 19]),
            chrome: s([196, 200, 206]).group(2),
            trim,
            rubber: s([34, 34, 36]),
            glass: Mat::new({
                let mut c = rgb([46, 60, 70]);
                c[3] = 150;
                c
            })
            .uv(Uv::Side),
            tire: s([30, 30, 31]),
            rim: s([196, 198, 202]),
            steel: s([70, 72, 76]),
            headlamp: s([236, 238, 230]).glow(emissive([255, 238, 205], 2.0)),
            tail: s([180, 16, 14]).glow(emissive([255, 18, 8], 1.0)),
            amber: s([232, 128, 24]).glow(emissive([255, 110, 10], 0.6)),
            reverse: s([226, 226, 222]),
            grille: s([20, 20, 22]).uv(Uv::Face),
            mirror: s([170, 180, 190]),
            plate: s([228, 226, 214]),
            accent: Mat::new(hex(&spec.accent)),
            wood: s([120, 74, 40]).uv(Uv::Side),
            wood_trim: s([206, 186, 140]),
            dash: ic(0),
            cluster: s([14, 14, 16]),
            dial: s([22, 24, 26]).glow(emissive(disp_e, 0.15)),
            display: Mat::new(disp_lin).glow(emissive(disp_e, 1.5)),
            vent: s([14, 14, 15]),
            radio: s([30, 30, 33]),
            seat: ic(1),
            seat_bolster: ic(1).shade(0.82),
            carpet: ic(2),
            headliner: ic(3),
            int_trim: ic(4),
            accent_int: ic(5),
            door_card: ic(1).shade(0.9),
            wheel_rim: ic(4).shade(0.7),
            wheel_spoke: ic(4),
        }
    }
}
