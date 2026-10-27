//! Mesh acceptance tests for the shipped shapes (`docs/VEHICLES.md`, "Acceptance tests").

extern crate std;

use super::math::{cross, dot, sub};
use super::params::parse_paint;
use super::*;
use crate::VehicleSpec;
use alloc::string::ToString;

const CORSAIR: &str = include_str!("../../../../assets/vehicles/corsair_gtx.shape.ron");
const PALISADE: &str = include_str!("../../../../assets/vehicles/palisade_wagon.shape.ron");
const CORSAIR_SPEC: &str = include_str!("../../../../assets/vehicles/corsair_gtx.ron");
const PALISADE_SPEC: &str = include_str!("../../../../assets/vehicles/palisade_wagon.ron");
const SEABREEZE: &str = include_str!("../../../../assets/vehicles/seabreeze_convertible.shape.ron");
const REGENT: &str = include_str!("../../../../assets/vehicles/regent_coupe_cabrio.shape.ron");
const SEABREEZE_SPEC: &str = include_str!("../../../../assets/vehicles/seabreeze_convertible.ron");
const REGENT_SPEC: &str = include_str!("../../../../assets/vehicles/regent_coupe_cabrio.ron");

fn load(text: &str) -> ShapeSpec {
    let s: ShapeSpec = ron::from_str(text).unwrap();
    assert_eq!(s.validate(), Ok(()), "{}", s.id);
    s
}

fn shapes() -> [ShapeSpec; 4] {
    [load(CORSAIR), load(PALISADE), load(SEABREEZE), load(REGENT)]
}

fn convertibles() -> [ShapeSpec; 2] {
    [load(SEABREEZE), load(REGENT)]
}

/// `spec` with its convertible top raised.
fn top_up(mut spec: ShapeSpec) -> ShapeSpec {
    spec.roof.kind = RoofKind::Convertible { top: Top::Up };
    spec
}

/// FNV-1a over every vertex attribute and index of every mesh, plus the cockpit.
fn hash(car: &CarMesh) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for m in car.meshes() {
        for p in &m.positions {
            for v in p {
                eat(&v.to_bits().to_le_bytes());
            }
        }
        for n in &m.normals {
            for v in n {
                eat(&v.to_bits().to_le_bytes());
            }
        }
        for c in &m.colors {
            eat(c);
        }
        for e in &m.emissive {
            eat(e);
        }
        for i in &m.indices {
            eat(&i.to_le_bytes());
        }
    }
    for v in car.cockpit.eye.iter().chain(&car.cockpit.wheel_pivot) {
        eat(&v.to_bits().to_le_bytes());
    }
    h
}

#[test]
fn shapes_build_with_sane_meshes() {
    for spec in shapes() {
        for lod in 0..2 {
            let car = build_lod(&spec, 0, lod).unwrap();
            for (k, m) in car.meshes().iter().enumerate() {
                let n = m.positions.len();
                assert!(n > 0 && !m.indices.is_empty(), "{} mesh {k} empty", spec.id);
                assert_eq!(m.normals.len(), n);
                assert_eq!(m.uvs.len(), n);
                assert_eq!(m.colors.len(), n);
                assert_eq!(m.emissive.len(), n);
                assert_eq!(m.indices.len() % 3, 0);
                assert!(
                    m.indices.iter().all(|&i| (i as usize) < n),
                    "{} mesh {k} index out of range",
                    spec.id
                );
                for (p, nr) in m.positions.iter().zip(&m.normals) {
                    assert!(
                        p.iter().all(|v| v.is_finite()),
                        "{} mesh {k} bad position",
                        spec.id
                    );
                    assert!(
                        nr.iter().all(|v| v.is_finite()),
                        "{} mesh {k} NaN normal",
                        spec.id
                    );
                    let l = dot(*nr, *nr);
                    assert!(
                        (l - 1.0).abs() < 1e-3,
                        "{} mesh {k} normal length {l}",
                        spec.id
                    );
                }
                for t in m.indices.chunks(3) {
                    assert!(t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
                }
            }
            // Glass is see-through, lamps glow.
            assert!(car.glass.colors.iter().all(|c| c[3] < 255));
            assert!(car.lights.emissive.iter().any(|e| e[3] > 0));
        }
    }
}

#[test]
fn triangle_budgets() {
    for spec in shapes() {
        let car = build(&spec, 0).unwrap();
        let ext = car.exterior_triangles();
        let int = car.interior_triangles();
        std::println!("{}: exterior {ext}, interior {int}", spec.id);
        assert!((8000..=14000).contains(&ext), "{} exterior {ext}", spec.id);
        assert!((4000..=8000).contains(&int), "{} interior {int}", spec.id);
        // Parked and distant cars.
        let low = build_lod(&spec, 0, 1).unwrap().exterior_triangles();
        assert!(
            (3000..=5000).contains(&low),
            "{} lod1 exterior {low}",
            spec.id
        );
    }
}

#[test]
fn lod1_is_at_most_40_percent() {
    for spec in shapes() {
        let full = build_lod(&spec, 0, 0).unwrap();
        let low = build_lod(&spec, 0, 1).unwrap();
        let (fe, le) = (full.exterior_triangles(), low.exterior_triangles());
        let (ft, lt) = (
            fe + full.interior_triangles(),
            le + low.interior_triangles(),
        );
        std::println!("{}: lod1 exterior {le}/{fe}, total {lt}/{ft}", spec.id);
        assert!(le * 10 <= fe * 4, "{} exterior lod1 {le} of {fe}", spec.id);
        assert!(lt * 10 <= ft * 4, "{} total lod1 {lt} of {ft}", spec.id);
        // The cockpit stays usable at LOD1.
        assert_eq!(low.cockpit.eye, full.cockpit.eye);
    }
}

#[test]
fn wheels_fit_inside_the_arches() {
    for spec in shapes() {
        for lod in 0..2 {
            let car = build_lod(&spec, 0, lod).unwrap();
            let w = &spec.wheels;
            for c in car.wheel_centers {
                let (inner, outer) = (c[0].abs() - w.width * 0.5, c[0].abs() + w.width * 0.5);
                for p in &car.body.positions {
                    if p[0] * c[0] <= 0.0 || p[0].abs() < inner || p[0].abs() > outer {
                        continue;
                    }
                    let (dy, dz) = (p[1] - c[1], p[2] - c[2]);
                    let r = (dy * dy + dz * dz).sqrt();
                    assert!(
                        r >= w.radius,
                        "{} lod{lod}: body vertex {p:?} inside the tire at {c:?} (r {r})",
                        spec.id
                    );
                }
            }
        }
    }
}

#[test]
fn eye_is_inside_the_greenhouse() {
    for spec in shapes() {
        if spec.roof.convertible() {
            continue;
        }
        let car = build(&spec, 0).unwrap();
        let eye = car.cockpit.eye;
        let b = body::Body::new(&spec, 0, 0);
        let s = eye[2] + spec.wheels.front_x;
        let f = b.frame(s);
        assert!(f.h > 0.2, "{}: no greenhouse at the eye", spec.id);
        assert!(
            eye[1] > f.y_ledge + 0.05 && eye[1] < f.top_y(eye[0].abs()) - 0.1,
            "{}: eye height {} (belt {}, roof {})",
            spec.id,
            eye[1],
            f.y_ledge,
            f.yt
        );
        assert!(
            eye[0] < 0.0 && eye[0].abs() < f.z_re - 0.1,
            "{}: eye lateral {}",
            spec.id,
            eye[0]
        );
        // The steering wheel sits ahead of the eye, below it, on the driver's side.
        let p = car.cockpit.wheel_pivot;
        assert!(p[2] < eye[2] - 0.3 && p[1] < eye[1] && (p[0] - eye[0]).abs() < 1e-6);
        let a = car.cockpit.wheel_axis;
        assert!((dot(a, a) - 1.0).abs() < 1e-5 && a[2] > 0.5);
        // The radio display faces the driver.
        let d = car.cockpit.radio_display;
        let n = cross(sub(d[1], d[0]), sub(d[2], d[0]));
        assert!(dot(n, sub(eye, d[0])) > 0.0);
        assert!(car.cockpit.dial_count >= 1);
    }
}

#[test]
fn convertible_eye_sits_over_the_belt_inside_the_body() {
    for spec in convertibles() {
        let car = build(&spec, 0).unwrap();
        let eye = car.cockpit.eye;
        let b = body::Body::new(&spec, 0, 0);
        let s = eye[2] + spec.wheels.front_x;
        let f = b.frame(s);
        let g = &spec.glass;
        let header = b.frame(g.windshield.1).yt;
        // Behind the header, ahead of the boot, as high over the belt as in a hardtop and
        // under the header line, well inside the doors.
        assert!(
            s > g.windshield.1 + 0.3 && s < g.backlight.0 - 0.5,
            "{}",
            spec.id
        );
        assert!(
            eye[1] > f.y_ledge + 0.05 && eye[1] < header - 0.15,
            "{}: eye height {} (belt {}, header {header})",
            spec.id,
            eye[1],
            f.y_ledge
        );
        assert!(
            eye[0] < 0.0 && eye[0].abs() < f.z_ledge - 0.2,
            "{}",
            spec.id
        );
        let p = car.cockpit.wheel_pivot;
        assert!(p[2] < eye[2] - 0.3 && p[1] < eye[1] && (p[0] - eye[0]).abs() < 1e-6);
        let d = car.cockpit.radio_display;
        let n = cross(sub(d[1], d[0]), sub(d[2], d[0]));
        assert!(dot(n, sub(eye, d[0])) > 0.0);
    }
}

/// Triangles of `m` whose centroid is over the open cabin: between the windshield header
/// and the boot, inside the door caps, above the belt (plus `margin`).
fn over_the_cabin(spec: &ShapeSpec, m: &MeshData, margin: f32) -> usize {
    let b = body::Body::new(spec, 0, 0);
    let g = &spec.glass;
    let (s0, s1) = (g.windshield.1 + 0.06, g.backlight.0 - 0.04);
    m.indices
        .chunks(3)
        .filter(|t| {
            let c = [0, 1, 2].map(|k| m.positions[t[k] as usize]);
            let (x, y, z) = (
                (c[0][0] + c[1][0] + c[2][0]) / 3.0,
                (c[0][1] + c[1][1] + c[2][1]) / 3.0,
                (c[0][2] + c[1][2] + c[2][2]) / 3.0,
            );
            let s = z + spec.wheels.front_x;
            if s <= s0 || s >= s1 {
                return false;
            }
            let f = b.frame(s);
            x.abs() < f.z_ledge - roof::CAP_W - 0.01 && y > f.y_ledge + margin
        })
        .count()
}

#[test]
fn convertibles_have_no_roof_over_the_cabin() {
    for spec in convertibles() {
        for lod in 0..2 {
            let car = build_lod(&spec, 0, lod).unwrap();
            for (what, m) in [("body", &car.body), ("glass", &car.glass)] {
                let n = over_the_cabin(&spec, m, 0.02);
                assert_eq!(
                    n, 0,
                    "{} lod{lod}: {n} {what} triangles roof the cabin",
                    spec.id
                );
            }
            // The seats show over the doors.
            let seats = over_the_cabin(&spec, &car.interior, 0.02);
            assert!(
                seats > 8,
                "{} lod{lod}: {seats} seat triangles over the belt",
                spec.id
            );
        }
        // A hardtop's roof does count.
        let hard = load(PALISADE);
        let car = build(&hard, 0).unwrap();
        assert!(over_the_cabin(&hard, &car.body, 0.02) > 50);
    }
}

#[test]
fn convertibles_have_a_framed_windshield_and_a_boot() {
    for spec in convertibles() {
        let car = build(&spec, 0).unwrap();
        let g = &spec.glass;
        let header = body::Body::new(&spec, 0, 0).frame(g.windshield.1).yt;
        let z = |s: f32| s - spec.wheels.front_x;
        // The header rail stands at the windshield's top edge.
        assert!(car.body.positions.iter().any(|p| {
            (p[2] - z(g.windshield.1)).abs() < 0.06 && p[1] > header && p[0].abs() < 0.1
        }));
        // No glass behind the windshield: no side, quarter or rear windows.
        assert!(car
            .glass
            .positions
            .iter()
            .all(|p| p[2] <= z(g.windshield.1) + 1e-3));
        // The boot rises over the deck behind the cabin.
        let deck = body::Body::new(&spec, 0, 0).frame((g.backlight.0 + g.backlight.1) * 0.5);
        let boot = car
            .body
            .positions
            .iter()
            .filter(|p| p[2] > z(g.backlight.0) && p[2] < z(g.backlight.1) && p[0].abs() < 0.3)
            .fold(0.0f32, |m, p| m.max(p[1]));
        assert!(
            boot > deck.yt + 0.08,
            "{}: boot {boot}, deck {}",
            spec.id,
            deck.yt
        );
        // LOD1 keeps the boot.
        let low = build_lod(&spec, 0, 1).unwrap();
        assert!(low
            .body
            .positions
            .iter()
            .any(|p| p[2] > z(g.backlight.0) && p[1] > deck.yt + 0.08));
    }
}

#[test]
fn a_raised_top_covers_the_cabin_with_a_rear_window() {
    for spec in convertibles() {
        let spec = top_up(spec);
        assert_eq!(spec.validate(), Ok(()));
        for lod in 0..2 {
            let car = build_lod(&spec, 0, lod).unwrap();
            assert_sane(&car, &spec, &alloc::format!("{} top up lod{lod}", spec.id));
            assert!(over_the_cabin(&spec, &car.body, 0.2) > 8, "{}", spec.id);
            let z0 = spec.glass.backlight.0 - spec.wheels.front_x;
            assert!(
                car.glass.positions.iter().any(|p| p[2] > z0),
                "{} rear window",
                spec.id
            );
        }
    }
}

#[test]
fn color_details_apply_to_their_palette_entry_only() {
    let spec = load(SEABREEZE);
    assert!(!spec.details.contains(&Detail::WoodApplique));
    let plain = build(&spec, 0).unwrap();
    let woody = build(&spec, 1).unwrap();
    let wood = rgb([120, 74, 40]);
    let count = |m: &MeshData| m.colors.iter().filter(|&&c| c == wood).count();
    assert_eq!(count(&plain.body), 0);
    assert!(count(&woody.body) > 8);
    assert_eq!(build(&spec, 1 + spec.palette.len()).unwrap(), woody);
}

#[test]
fn body_faces_point_outward() {
    // Signed volume of the body mesh: closed parts add their volume, open decals add
    // little. Inside-out winding would make it negative.
    for spec in shapes() {
        let car = build(&spec, 0).unwrap();
        let m = &car.body;
        let mut vol = 0.0f64;
        for t in m.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| m.positions[t[k] as usize]);
            vol += dot(a, cross(b, c)) as f64 / 6.0;
        }
        let l = spec.length() as f64;
        assert!(vol > l * 1.6 * 0.9 * 0.5, "{}: body volume {vol}", spec.id);
        assert!(vol < l * 2.0 * 1.5, "{}: body volume {vol}", spec.id);
    }
}

#[test]
fn shapes_match_their_vehicle_specs() {
    for (shape, text) in [
        (load(CORSAIR), CORSAIR_SPEC),
        (load(PALISADE), PALISADE_SPEC),
        (load(SEABREEZE), SEABREEZE_SPEC),
        (load(REGENT), REGENT_SPEC),
    ] {
        let v: VehicleSpec = ron::from_str(text).unwrap();
        assert_eq!(v.shape, shape.id.to_string() + ".shape.ron");
        let w = &shape.wheels;
        assert!(
            (w.rear_x - w.front_x - v.wheelbase_m).abs() < 1e-5,
            "{} wheelbase",
            v.id
        );
        assert!((w.track - v.track_m).abs() < 1e-5, "{} track", v.id);
        assert!(
            (shape.length() - v.dims_m[0]).abs() < 0.05,
            "{} length",
            v.id
        );
        let half = shape.plan.iter().fold(0.0f32, |m, p| m.max(p.1));
        assert!((2.0 * half - v.dims_m[1]).abs() < 0.05, "{} width", v.id);
        let roof = shape.side.iter().fold(0.0f32, |m, p| m.max(p.1));
        assert!((roof - v.dims_m[2]).abs() < 0.05, "{} height", v.id);
        // The model-to-body offset puts the front axle where the sim has it.
        let off = body_offset(&v);
        let car = build_lod(&shape, 0, 1).unwrap();
        let fl = car.wheel_centers[0];
        let sim = v.wheel_positions()[0];
        assert!((fl[2] + off[2] - sim[2]).abs() < 1e-5);
        assert!((fl[0] - sim[0]).abs() < 1e-5);
        assert!((0.0 + off[1] - sim[1]).abs() < 1e-5);
    }
}

#[test]
fn builds_are_deterministic() {
    // Pinned on purpose: a change here means every built car changed. Update in its own
    // commit with the reason.
    let expected: [u64; 4] = [
        0x1eba_1d20_a4a5_023f,
        0x711d_386f_f001_e557,
        0xe8fa_88e7_8dba_9790,
        0x1279_2d2b_6c10_b887,
    ];
    let mut got = [0u64; 4];
    for (k, spec) in shapes().iter().enumerate() {
        let a = build(spec, 1).unwrap();
        let b = build(spec, 1).unwrap();
        assert_eq!(a, b);
        got[k] = hash(&a);
        std::println!("{}: {:#018x}", spec.id, got[k]);
    }
    assert_eq!(got, expected, "mesh hashes changed");
}

#[test]
fn palette_choice_only_changes_paint() {
    let spec = load(CORSAIR);
    let a = build(&spec, 0).unwrap();
    let b = build(&spec, 1).unwrap();
    assert_eq!(a.body.positions.len(), b.body.positions.len());
    assert_ne!(a.body.colors, b.body.colors);
    assert_eq!(a.interior, b.interior);
    // Wraps.
    assert_eq!(build(&spec, spec.palette.len()).unwrap(), a);
}

#[test]
fn popups_raise() {
    let spec = load(CORSAIR);
    let down = build(&spec, 0).unwrap();
    let up = build_with(
        &spec,
        &BuildOptions {
            popups_up: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let top = |m: &MeshData| {
        m.positions
            .iter()
            .filter(|p| p[2] < -0.6 && p[0].abs() > 0.3)
            .fold(0.0f32, |a, p| a.max(p[1]))
    };
    assert!(
        top(&up.body) > top(&down.body) + 0.08,
        "{} {}",
        top(&up.body),
        top(&down.body)
    );
}

fn rejects(text: &str, f: impl FnOnce(&mut ShapeSpec), expected: ShapeError) {
    let mut s = load(text);
    f(&mut s);
    assert_eq!(s.validate(), Err(expected));
    assert_eq!(build(&s, 0), Err(expected));
}

#[test]
fn validate_rejects_bad_curves() {
    rejects(
        CORSAIR,
        |s| s.plan.swap(2, 3),
        ShapeError::PlanNotIncreasing { index: 3 },
    );
    rejects(
        CORSAIR,
        |s| s.plan[1].0 = 0.0,
        ShapeError::PlanNotIncreasing { index: 1 },
    );
    rejects(CORSAIR, |s| s.plan[0].0 = 0.05, ShapeError::PlanSpan);
    rejects(
        CORSAIR,
        |s| s.plan[2].1 = 0.0,
        ShapeError::NonPositiveWidth { index: 2 },
    );
    rejects(
        CORSAIR,
        |s| s.side[3].0 = 0.3,
        ShapeError::SideTopNotIncreasing { index: 3 },
    );
    rejects(
        CORSAIR,
        |s| s.side[13].0 = 3.9,
        ShapeError::SideBottomNotDecreasing { index: 13 },
    );
    rejects(CORSAIR, |s| s.side[0].0 = 0.01, ShapeError::SideNotAtNose);
    rejects(CORSAIR, |s| s.side.truncate(4), ShapeError::SidePointCount);
    rejects(CORSAIR, |s| s.side[11].1 = 1.5, ShapeError::SideInverted);
    rejects(
        CORSAIR,
        |s| s.belt.push((1.0, 0.8)),
        ShapeError::BeltNotIncreasing { index: 2 },
    );
    rejects(CORSAIR, |s| s.side[4].1 = f32::NAN, ShapeError::NonFinite);
}

#[test]
fn validate_rejects_bad_parts() {
    rejects(CORSAIR, |s| s.seams.doors = 3, ShapeError::DoorCount);
    rejects(
        CORSAIR,
        |s| s.palette[2] = "red".to_string(),
        ShapeError::BadColor { index: 2 },
    );
    rejects(CORSAIR, |s| s.palette.clear(), ShapeError::EmptyPalette);
    rejects(
        CORSAIR,
        |s| s.glass.windshield = (2.0, 1.5),
        ShapeError::GlassRange,
    );
    rejects(
        PALISADE,
        |s| s.glass.side[1].0 = 2.5,
        ShapeError::SideWindow { index: 1 },
    );
    rejects(
        CORSAIR,
        |s| s.wheels.front_x = 0.2,
        ShapeError::WheelPlacement,
    );
    rejects(CORSAIR, |s| s.wheels.track = 1.9, ShapeError::TrackTooWide);
    rejects(
        CORSAIR,
        |s| s.section.shoulder = 1.2,
        ShapeError::SectionOutOfRange,
    );
    rejects(CORSAIR, |s| s.interior.seat.0 = 1.9, ShapeError::Interior);
    rejects(
        CORSAIR,
        |s| s.seams.creases.push((0.0, 0.5, 0.1, 0.5)),
        ShapeError::Crease { index: 1 },
    );
    rejects(
        SEABREEZE,
        |s| s.glass.side.push((1.5, 2.4)),
        ShapeError::OpenTop,
    );
    rejects(
        SEABREEZE,
        |s| s.glass.backlight = (2.6, 3.0),
        ShapeError::OpenTop,
    );
    // A hardtop's roof line cannot be declared open.
    rejects(
        PALISADE,
        |s| s.roof.kind = RoofKind::Convertible { top: Top::Down },
        ShapeError::OpenTop,
    );
    rejects(
        SEABREEZE,
        |s| s.color_details.push((9, Detail::WoodPanel)),
        ShapeError::ColorDetail,
    );
    rejects(
        CORSAIR,
        |s| s.palette[0] = "#ffffff/".to_string(),
        ShapeError::BadColor { index: 0 },
    );
    rejects(
        CORSAIR,
        |s| s.interior.insert = "red".to_string(),
        ShapeError::Interior,
    );
}

#[test]
fn two_tone_paint_splits_at_the_crease() {
    for (text, k) in [(CORSAIR, 6), (REGENT, 4)] {
        let spec = load(text);
        let (upper, lower) = parse_paint(&spec.palette[k]).unwrap();
        let (upper, lower) = (rgb(upper), rgb(lower.unwrap()));
        let stripe = hex(&spec.accent);
        for lod in 0..2 {
            let car = build_lod(&spec, k, lod).unwrap();
            let m = &car.body;
            let mean_y = |c: [u8; 4]| {
                let ys: alloc::vec::Vec<f32> = m
                    .colors
                    .iter()
                    .zip(&m.positions)
                    .filter(|(&cc, _)| cc == c)
                    .map(|(_, p)| p[1])
                    .collect();
                assert!(ys.len() > 20, "{} lod{lod}: {} vertices", spec.id, ys.len());
                ys.iter().sum::<f32>() / ys.len() as f32
            };
            assert!(mean_y(upper) > mean_y(lower) + 0.15, "{} lod{lod}", spec.id);
            if lod == 0 {
                // The pinstripe in the crease's groove.
                assert!(m.colors.contains(&stripe), "{}", spec.id);
            }
        }
    }
}

#[test]
fn era_paint_overrides_the_palette() {
    let spec = load(PALISADE);
    let total: u32 = ERA_1980S_PAINT.iter().map(|p| p.1).sum();
    for &(paint, _) in &ERA_1980S_PAINT {
        assert!(parse_paint(paint).is_some(), "{paint}");
    }
    // Every entry is reachable and the weights hold.
    let mut seen = [0u32; ERA_1980S_PAINT.len()];
    for r in 0..total {
        let p = era_1980s_paint(r);
        seen[ERA_1980S_PAINT.iter().position(|e| e.0 == p).unwrap()] += 1;
    }
    assert!(seen.iter().zip(&ERA_1980S_PAINT).all(|(&n, e)| n == e.1));
    let opts = |paint| BuildOptions {
        color_index: 0,
        lod: 1,
        popups_up: false,
        paint,
    };
    let own = build_with(&spec, &opts(None)).unwrap();
    let era = build_with(&spec, &opts(Some(era_1980s_paint(20)))).unwrap();
    assert_eq!(own.body.positions.len(), era.body.positions.len());
    assert_ne!(own.body.colors, era.body.colors);
    assert_eq!(own.interior, era.interior);
    assert_eq!(
        build_with(&spec, &opts(Some("blue"))),
        Err(ShapeError::BadColor { index: u32::MAX })
    );
}

#[test]
fn whitewalls_are_white_on_the_outer_sidewall() {
    let spec = load(REGENT);
    assert!(spec.parts.whitewalls);
    let white = rgb([226, 224, 216]);
    for lod in 0..2 {
        let car = build_lod(&spec, 0, lod).unwrap();
        let w = &car.wheels[1];
        let n = w.colors.iter().filter(|&&c| c == white).count();
        assert!(n > 16, "lod{lod}: {n}");
        // Outer face only.
        assert!(w
            .colors
            .iter()
            .zip(&w.positions)
            .all(|(&c, p)| c != white || p[0] > 0.0));
    }
}

/// Checks a car for broken geometry and wheel/arch fit.
fn assert_sane(car: &CarMesh, spec: &ShapeSpec, what: &str) {
    for m in car.meshes() {
        assert!(
            m.indices.iter().all(|&i| (i as usize) < m.positions.len()),
            "{what}"
        );
        assert!(
            m.normals
                .iter()
                .all(|n| n.iter().all(|v| v.is_finite()) && (dot(*n, *n) - 1.0).abs() < 1e-3),
            "{what}: bad normal"
        );
    }
    let w = &spec.wheels;
    for c in car.wheel_centers {
        for p in &car.body.positions {
            let lateral = p[0] * c[0] > 0.0 && (p[0].abs() - c[0].abs()).abs() <= w.width * 0.5;
            let (dy, dz) = (p[1] - c[1], p[2] - c[2]);
            assert!(
                !lateral || dy * dy + dz * dz >= w.radius * w.radius,
                "{what}: {p:?} in tire"
            );
        }
    }
    assert!(
        car.exterior_triangles() <= 14000,
        "{what}: {}",
        car.exterior_triangles()
    );
}

#[test]
fn every_option_builds() {
    let heads = [
        HeadLights::PopUp,
        HeadLights::Rect,
        HeadLights::Quad,
        HeadLights::Round,
        HeadLights::Hidden,
    ];
    let grilles = [Grille::Slot, Grille::Egg, Grille::Wide, Grille::None];
    let bumpers = [Bumper::Chrome, Bumper::Body, Bumper::Rubber];
    let tails = [
        TailLights::FullWidth,
        TailLights::Dual,
        TailLights::Triple,
        TailLights::Round,
    ];
    let spoilers = [Spoiler::None, Spoiler::Lip, Spoiler::Wing];
    let rims = [Rims::Steel, Rims::Alloy5, Rims::Mesh, Rims::Turbine];
    let details = [
        Detail::Louvers,
        Detail::HoodScoop,
        Detail::SideStripe,
        Detail::WoodPanel,
        Detail::ChromeTrim,
        Detail::VinylRoof,
        Detail::RallyStripes,
        Detail::MudFlaps,
        Detail::RoofRack,
        Detail::BlackTrim,
        Detail::Badge(BadgeShape::Diamond),
        Detail::Badge(BadgeShape::Circle),
        Detail::Badge(BadgeShape::Bar),
        Detail::Badge(BadgeShape::Chevron),
        Detail::WoodApplique,
        Detail::CorneringLamps,
    ];
    for base in shapes() {
        for k in 0..5 {
            let mut s = base.clone();
            s.front.lights = heads[k % heads.len()];
            s.front.grille = grilles[k % grilles.len()];
            s.front.bumper = bumpers[k % bumpers.len()];
            s.rear.lights = tails[k % tails.len()];
            s.rear.spoiler = spoilers[k % spoilers.len()];
            s.rear.bumper = bumpers[(k + 1) % bumpers.len()];
            s.parts.rims = rims[k % rims.len()];
            s.parts.hubcaps = k == 4;
            s.parts.whitewalls = k % 2 == 1;
            s.interior.manual = k % 2 == 0;
            s.interior.applique = [Applique::None, Applique::Wood, Applique::Brushed][k % 3];
            s.parts.mirrors = if k % 2 == 0 {
                Mirrors::Fender
            } else {
                Mirrors::Door
            };
            s.parts.handles = if k % 2 == 0 {
                Handles::Pull
            } else {
                Handles::Flush
            };
            s.parts.exhaust = if k % 2 == 0 {
                Exhaust::Single
            } else {
                Exhaust::Dual
            };
            s.parts.antenna = if k % 2 == 0 {
                Antenna::None
            } else {
                Antenna::Mast
            };
            s.parts.wipers = k as u8 % 4;
            s.seams.doors = if k % 2 == 0 { 4 } else { 2 };
            s.seams.creases.truncate(k % 2);
            s.details = details.iter().copied().skip(k * 3).take(4).collect();
            for lod in 0..2 {
                for popups_up in [false, true] {
                    let opts = BuildOptions {
                        color_index: k,
                        lod,
                        popups_up,
                        paint: None,
                    };
                    let car = build_with(&s, &opts).unwrap();
                    assert_sane(&car, &s, &alloc::format!("{} variant {k} lod{lod}", s.id));
                }
            }
        }
    }
}
