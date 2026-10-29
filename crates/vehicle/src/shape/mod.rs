//! `vehicle::shape`: a parametric car mesh generator.
//!
//! A [`ShapeSpec`] (RON file `assets/vehicles/*.shape.ron`, layout in `docs/VEHICLES.md`)
//! is a side silhouette, a plan outline, a beltline and a few cross-section numbers, plus
//! choices for lamps, grille, bumpers, rims and trim. [`build`] lofts it into a body of a
//! little more than early-2000s-console density (8–14k triangles with wheels and parts,
//! 4–8k for the cabin; 3–5k at LOD1) as plain arrays the renderer can upload.
//!
//! Model space is the body-space axes (`+x` right, `+y` up, `-z` forward) with the origin
//! on the ground, on the centerline, under the front axle. [`body_offset`] moves it to
//! the center-of-mass origin the sim uses.
//!
//! Everything here is deterministic: no platform math, no hash maps, the same spec gives
//! bit-identical meshes everywhere.

mod bed;
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
    Antenna, Applique, BadgeShape, Bumper, Detail, Exhaust, Front, Glass, Grille, Handles,
    HeadLights, InteriorShape, Mirrors, Parts, Rear, Rims, Roof, RoofKind, Seams, Section,
    ShapeError, ShapeSpec, Spoiler, TailLights, Top, TubeColor, Wheels,
};

use crate::VehicleSpec;
use mesh::{emissive, Mat, Uv};
use params::{parse_hex, parse_paint};

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
    /// The driver's hip point (`InteriorShape::seat`).
    pub hip: [f32; 3],
    /// Where the driver's heels rest: the floor behind the pedals, on the driver's line.
    pub heel: [f32; 3],
    /// The steering wheel rim's radius (to the middle of its tube).
    pub wheel_radius: f32,
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
    /// Underglow tubes, lit; empty without `Detail::Underglow`. Their own lamp group: the
    /// app draws them only while the underglow is switched on.
    pub underglow: MeshData,
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
    /// Body, parts, glass, lights, underglow and all four wheels.
    pub fn exterior_triangles(&self) -> usize {
        self.body.triangle_count()
            + self.glass.triangle_count()
            + self.lights.triangle_count()
            + self.underglow.triangle_count()
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
    /// discs (≤ 40% of the triangles); 2 is a coarser loft with plainer wheels and no arch
    /// lips (~2k exterior triangles) for parked cars far away.
    pub lod: u8,
    /// Pop-up headlamps raised.
    pub popups_up: bool,
    /// A paint (`#rrggbb` or `#upper/#lower`) in place of the palette entry's, which still
    /// picks the `color_details`: parked cars in an era's colors ([`ERA_1980S_PAINT`]).
    pub paint: Option<&'static str>,
}

/// Body colors seen on the street in the 1980s and how common each is (weights): lots of
/// tan, white, silver and burgundy, some browns, blues and two-tones.
pub const ERA_1980S_PAINT: [(&str, u32); 14] = [
    ("#c9b48e", 14),        // tan
    ("#ebe8df", 14),        // white
    ("#b9bbbd", 11),        // silver
    ("#6e1f2a", 10),        // burgundy
    ("#6a4a32", 8),         // brown
    ("#41597e", 7),         // medium blue
    ("#bfc2c4/#3b4d72", 6), // silver over blue
    ("#1e1f22", 6),         // black
    ("#a8262a", 6),         // red
    ("#344a3a", 4),         // dark green
    ("#e3d9bd", 4),         // cream
    ("#8fa7bd", 4),         // light blue
    ("#b89a52", 3),         // gold
    ("#d4c3a0/#5e4430", 3), // champagne over brown
];

/// The era paint `roll` (any number; taken modulo the weights' sum) lands on.
pub fn era_1980s_paint(roll: u32) -> &'static str {
    let total: u32 = ERA_1980S_PAINT.iter().map(|p| p.1).sum();
    let mut r = roll % total;
    for &(paint, w) in &ERA_1980S_PAINT {
        if r < w {
            return paint;
        }
        r -= w;
    }
    ERA_1980S_PAINT[0].0
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

/// Builds level of detail `level` (0, 1 or 2).
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
    let lod = opts.lod.min(2);
    if opts.paint.is_some_and(|p| params::parse_paint(p).is_none()) {
        return Err(ShapeError::BadColor { index: u32::MAX });
    }
    let mats = Materials::new(spec, opts.color_index, opts.paint);
    let b = body::Body::new(spec, lod, opts.color_index);

    let mut shell = mesh::Soup::new(38.0);
    let mut glass = mesh::Soup::new(30.0);
    b.emit(&mats, &mut shell, &mut glass);
    bed::build(&b, &mats, &mut shell);

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
    let wheel = wheel::build(w, &spec.parts, lod, &mats).finish();
    let (x, y, wb) = (w.track * 0.5, w.radius, w.rear_x - w.front_x);
    let mut body = shell.finish();
    if !spec.axles.is_empty() {
        let fixed = wheel::fixed(w, &spec.parts, lod, &mats).finish();
        for &ax in &spec.axles {
            for side in [-1.0f32, 1.0] {
                append_wheel(&mut body, &fixed, [side * x, y, ax - w.front_x], side < 0.0);
            }
        }
    }
    Ok(CarMesh {
        body,
        wheels: [wheel.clone(), wheel.clone(), wheel.clone(), wheel],
        glass: glass.finish(),
        lights: p.lights.finish(),
        underglow: p.underglow.finish(),
        interior: int.soup.finish(),
        steering_wheel: int.wheel.finish(),
        wheel_centers: [[-x, y, 0.0], [x, y, 0.0], [-x, y, wb], [x, y, wb]],
        wheel_radius: w.radius,
        cockpit: int.cockpit,
    })
}

/// Appends right-side, hub-local wheel `w` to `body` at hub `at`, turned 180° about `y`
/// for a `left` wheel.
fn append_wheel(body: &mut MeshData, w: &MeshData, at: [f32; 3], left: bool) {
    let base = body.positions.len() as u32;
    let turn = |v: [f32; 3]| if left { [-v[0], v[1], -v[2]] } else { v };
    for (p, n) in w.positions.iter().zip(&w.normals) {
        let p = turn(*p);
        body.positions
            .push([p[0] + at[0], p[1] + at[1], p[2] + at[2]]);
        body.normals.push(turn(*n));
    }
    body.uvs.extend_from_slice(&w.uvs);
    body.colors.extend_from_slice(&w.colors);
    body.emissive.extend_from_slice(&w.emissive);
    body.indices.extend(w.indices.iter().map(|i| i + base));
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
    /// The paint, or the upper color of a two-tone.
    pub paint: Mat,
    /// The lower color of a two-tone paint (`paint` otherwise).
    pub paint_low: Mat,
    pub two_tone: bool,
    /// The two-tone split's pinstripe.
    pub pinstripe: Mat,
    pub seam: Mat,
    pub chassis: Mat,
    pub liner: Mat,
    pub chrome: Mat,
    pub trim: Mat,
    pub rubber: Mat,
    pub glass: Mat,
    pub tire: Mat,
    pub whitewall: Mat,
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
    pub dash_top: Mat,
    pub applique: Mat,
    pub bezel: Mat,
    pub cluster: Mat,
    pub dial: Mat,
    pub display: Mat,
    pub vent: Mat,
    pub radio: Mat,
    pub seat: Mat,
    pub seat_bolster: Mat,
    pub seat_insert: Mat,
    pub piping: Mat,
    pub belt: Mat,
    pub switch: Mat,
    pub warn: [Mat; 4],
    pub carpet: Mat,
    pub headliner: Mat,
    pub int_trim: Mat,
    pub accent_int: Mat,
    pub door_card: Mat,
    pub wheel_rim: Mat,
    pub wheel_spoke: Mat,
}

impl Materials {
    fn new(spec: &ShapeSpec, color_index: usize, paint: Option<&str>) -> Materials {
        let paint_hex = paint.unwrap_or(&spec.palette[color_index % spec.palette.len()]);
        let (upper, lower) = parse_paint(paint_hex).unwrap_or(([128, 128, 128], None));
        let paint = Mat::new(rgb(upper));
        let paint_low = lower.map_or(paint, |c| Mat::new(rgb(c)));
        let s = |h: [u8; 3]| Mat::new(rgb(h));
        let ic = &spec.interior.colors;
        let ic = |k: usize| Mat::new(hex(&ic[k]));
        let disp = parse_hex(&spec.interior.display).unwrap_or([60, 255, 120]);
        let disp_lin = rgb(disp);
        let disp_e = [disp_lin[0], disp_lin[1], disp_lin[2]];
        let trim = s([40, 40, 43]);
        let opt = |h: &str, or: Mat| parse_hex(h).map_or(or, |c| Mat::new(rgb(c)));
        let i = &spec.interior;
        let seat = ic(1);
        let dash = ic(0);
        let applique = match i.applique {
            params::Applique::None => dash.shade(0.8),
            params::Applique::Wood => s([112, 66, 34]),
            params::Applique::Brushed => s([150, 152, 156]).group(3),
        };
        Materials {
            paint,
            paint_low,
            two_tone: lower.is_some(),
            pinstripe: Mat::new(hex(&spec.accent)),
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
            whitewall: s([226, 224, 216]),
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
            dash,
            dash_top: opt(&i.dash_top, dash),
            applique,
            bezel: s([170, 174, 180]).group(2),
            cluster: s([14, 14, 16]),
            // Backlit faces: the display color, dim by day, glowing with the instruments.
            dial: s([20, 22, 24]).glow(emissive(disp_e, 0.7)),
            display: Mat::new(disp_lin).glow(emissive(disp_e, 1.5)),
            vent: s([14, 14, 15]),
            radio: s([30, 30, 33]),
            seat,
            seat_bolster: seat.shade(0.82),
            seat_insert: opt(&i.insert, seat.shade(1.18)),
            piping: opt(&i.piping, seat.shade(0.55)),
            belt: s([38, 38, 40]),
            switch: s([24, 24, 26]),
            warn: [
                s([90, 20, 16]).glow(emissive([255, 30, 20], 0.25)),
                s([90, 60, 10]).glow(emissive([255, 150, 20], 0.25)),
                s([20, 70, 30]).glow(emissive([40, 255, 80], 0.25)),
                s([20, 40, 90]).glow(emissive([40, 120, 255], 0.25)),
            ],
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
