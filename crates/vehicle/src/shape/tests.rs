//! Mesh acceptance tests for the shipped shapes (`docs/VEHICLES.md`, "Acceptance tests").

extern crate std;

use super::math::{cross, dot, sub};
use super::params::parse_paint;
use super::*;
use crate::VehicleSpec;
use alloc::string::ToString;
use alloc::vec::Vec;

const GANNET: &str = include_str!("../../../../assets/vehicles/gannet_gt.shape.ron");
const HOMESTEAD: &str = include_str!("../../../../assets/vehicles/homestead_wagon.shape.ron");
const GANNET_SPEC: &str = include_str!("../../../../assets/vehicles/gannet_gt.ron");
const HOMESTEAD_SPEC: &str = include_str!("../../../../assets/vehicles/homestead_wagon.ron");
const SEABREEZE: &str = include_str!("../../../../assets/vehicles/seabreeze_convertible.shape.ron");
const REGENT: &str = include_str!("../../../../assets/vehicles/regent_coupe_cabrio.shape.ron");
const SEABREEZE_SPEC: &str = include_str!("../../../../assets/vehicles/seabreeze_convertible.ron");
const REGENT_SPEC: &str = include_str!("../../../../assets/vehicles/regent_coupe_cabrio.ron");

fn load(text: &str) -> ShapeSpec {
    let s: ShapeSpec = ron::from_str(text).unwrap();
    assert_eq!(s.validate(), Ok(()), "{}", s.id);
    s
}

/// The rest of the roster (the 1980s cars, the 2001 heroes, the 2001 traffic cars, then the
/// second four 2001 heroes), as `(shape, spec)` texts.
macro_rules! car {
    ($id:literal) => {
        (
            include_str!(concat!("../../../../assets/vehicles/", $id, ".shape.ron")),
            include_str!(concat!("../../../../assets/vehicles/", $id, ".ron")),
        )
    };
}
const ROSTER: [(&str, &str); 32] = [
    car!("pipit_hatch"),
    car!("halcyon_turbo"),
    car!("bancroft_sedan"),
    car!("prescott_landau"),
    car!("meadow_minivan"),
    car!("buckboard_pickup"),
    car!("drayman_van"),
    car!("burro_4x4"),
    car!("basilisk_v12"),
    car!("basilisk_roadster"),
    car!("avocet_v8"),
    car!("avocet_spider"),
    car!("longbow_coupe"),
    car!("longbow_convertible"),
    car!("appaloosa_gt"),
    car!("appaloosa_convertible"),
    car!("halberd_coupe"),
    car!("halberd_targa"),
    car!("tanager_awd"),
    car!("destrier_coupe"),
    car!("siskin_coupe"),
    car!("plover_sedan"),
    car!("sorrel_utility"),
    car!("wren_minivan"),
    car!("percheron_pickup"),
    car!("linnet_hatch"),
    car!("sledge_box_truck"),
    car!("anvil_semi"),
    car!("shrike_coupe"),
    car!("glaive_turbo"),
    car!("falchion_sedan"),
    car!("courser_v8"),
];
const PICKUP: &str = ROSTER[5].0;
const UTILITY: &str = ROSTER[7].0;
const WEDGE: &str = ROSTER[1].0;
const GT: &str = ROSTER[16].0;
const TARGA: &str = ROSTER[17].0;
const MUSCLE: &str = ROSTER[19].0;
const TUNER: &str = ROSTER[20].0;
const SEMI: &str = ROSTER[27].0;

/// Every built-in shape with its vehicle spec: the first four cars, then the roster.
fn pairs() -> Vec<(&'static str, &'static str)> {
    let mut v = alloc::vec![
        (GANNET, GANNET_SPEC),
        (HOMESTEAD, HOMESTEAD_SPEC),
        (SEABREEZE, SEABREEZE_SPEC),
        (REGENT, REGENT_SPEC),
    ];
    v.extend(ROSTER);
    v
}

fn shapes() -> Vec<ShapeSpec> {
    pairs().iter().map(|p| load(p.0)).collect()
}

/// Every built-in convertible, top down.
fn convertibles() -> Vec<ShapeSpec> {
    shapes()
        .into_iter()
        .filter(|s| s.roof.convertible())
        .map(|mut s| {
            s.roof.kind = RoofKind::Convertible { top: Top::Down };
            s
        })
        .collect()
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
    for m in car.meshes().into_iter().chain([&car.underglow]) {
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
        for lod in 0..3 {
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
        // Parked cars far away.
        let far = build_lod(&spec, 0, 2).unwrap().exterior_triangles();
        assert!(
            (1500..=2500).contains(&far),
            "{} lod2 exterior {far}",
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
        for lod in 0..3 {
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
        let hard = load(HOMESTEAD);
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
        let w = 2.0 * spec.plan.iter().fold(0.0f32, |m, p| m.max(p.1)) as f64;
        let h = spec.side.iter().fold(0.0f32, |m, p| m.max(p.1)) as f64;
        assert!(vol > l * 1.6 * 0.9 * 0.5, "{}: body volume {vol}", spec.id);
        assert!(vol < l * w * h, "{}: body volume {vol}", spec.id);
    }
}

#[test]
fn shapes_match_their_vehicle_specs() {
    for (shape, text) in pairs() {
        let shape = load(shape);
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
    let expected: [u64; 36] = [
        0x423b_5062_5fd3_cf8a,
        0x3029_8378_2176_2c55,
        0xc45e_2768_5549_171f,
        0x4152_4f72_8755_e48b,
        0x935e_8299_8414_b56e,
        0xf385_c1f0_248b_6010,
        0x5b7d_750e_8323_4664,
        0xab19_d617_825e_8cab,
        0x022b_e7f5_0d8d_21a6,
        0x02ee_91a1_62eb_8235,
        0xd025_366a_19f6_af9f,
        0x4676_d964_214b_3d24,
        0xc131_bba0_9f7c_56b8,
        0xc702_8a79_c709_5d1e,
        0xcf76_5b98_a053_581a,
        0x6064_4e35_c578_88f3,
        0x43f0_f6a5_f780_2511,
        0xd82a_13ee_60f4_895e,
        0xb97f_4162_95ea_886b,
        0xd559_ed48_912b_e8e0,
        0x9e9d_b87d_199a_d69d,
        0x131b_6f94_c796_e5ca,
        0x545d_1564_b330_58d7,
        0xdef6_898f_f892_e447,
        0xde04_5bd2_8495_65cb,
        0x452a_a1dc_f15e_9898,
        0x1f36_9ab3_4ecd_57bc,
        0x4ee2_52ce_fdff_5e52,
        0xcfa0_bd81_967d_c962,
        0x3af4_d4e6_9fbc_6082,
        0xb41b_0a5c_9c13_ad1f,
        0x6eab_1dc0_5663_cdea,
        0x9297_9ead_c044_7c3f,
        0x2830_b234_e936_bfea,
        0x5c96_5cb3_a235_afeb,
        0xebbe_b1b8_7826_cd46,
    ];
    let mut got = [0u64; 36];
    for (k, spec) in shapes().iter().enumerate() {
        let a = build(spec, 1).unwrap();
        let b = build(spec, 1).unwrap();
        assert_eq!(a, b);
        got[k] = hash(&a);
        std::println!("{}: {:#018x}", spec.id, got[k]);
    }
    assert_eq!(got, expected, "mesh hashes changed");
}

/// Body vertices over the middle of a pickup's bed, between the bulkhead and the tailgate,
/// and above the rail.
fn over_the_bed(spec: &ShapeSpec, m: &MeshData) -> usize {
    let (s0, s1) = spec.bed().unwrap();
    let rail = super::math::polyline_at(&spec.belt, (s0 + s1) * 0.5);
    let ax = spec.wheels.front_x;
    m.positions
        .iter()
        .filter(|p| {
            let s = p[2] + ax;
            s > s0 + 0.02 && s < s1 - 0.02 && p[0].abs() < 0.6 && p[1] > rail - 0.02
        })
        .count()
}

#[test]
fn a_pickup_bed_is_open_and_lined() {
    let spec = load(PICKUP);
    let (s0, s1) = spec.bed().unwrap();
    assert!(s1 - s0 > 2.0, "a long bed: {s0}..{s1}");
    for lod in 0..3 {
        let car = build_lod(&spec, 0, lod).unwrap();
        assert_eq!(over_the_bed(&spec, &car.body), 0, "lod{lod}: bed covered");
        // The floor is down in the bed, well under the rail (and over the underside).
        let ax = spec.wheels.front_x;
        let rail = super::math::polyline_at(&spec.belt, (s0 + s1) * 0.5);
        let floor = car
            .body
            .positions
            .iter()
            .filter(|p| p[2] + ax > s0 - 0.01 && p[2] + ax < s1 + 0.01 && p[0].abs() < 0.7)
            .filter(|p| p[1] > 0.5)
            .fold(f32::MAX, |m, p| m.min(p[1]));
        assert!(floor < rail - 0.35, "lod{lod}: floor {floor}, rail {rail}");
    }
    // Without the detail the same top is a closed deck.
    let mut closed = spec.clone();
    closed.details.retain(|&d| d != Detail::PickupBed);
    assert!(over_the_bed(&spec, &build(&closed, 0).unwrap().body) > 20);
}

#[test]
fn a_spare_tire_hangs_on_the_tail() {
    let spec = load(UTILITY);
    let tail = spec.length() - spec.wheels.front_x;
    for lod in 0..3 {
        let car = build_lod(&spec, 0, lod).unwrap();
        let behind = car
            .body
            .positions
            .iter()
            .filter(|p| p[2] > tail + 0.15)
            .count();
        assert!(behind > 20, "lod{lod}: {behind} vertices behind the tail");
        let mut bare = spec.clone();
        bare.details.retain(|&d| d != Detail::SpareTire);
        let car = build_lod(&bare, 0, lod).unwrap();
        assert!(car.body.positions.iter().all(|p| p[2] < tail + 0.15));
    }
}

#[test]
fn a_t_top_puts_glass_in_the_roof() {
    let spec = load(WEDGE);
    let g = &spec.glass;
    let ax = spec.wheels.front_x;
    let roof_glass = |car: &CarMesh| {
        car.glass
            .positions
            .iter()
            .filter(|p| p[2] + ax > g.windshield.1 + 0.02 && p[2] + ax < g.backlight.0 - 0.02)
            .filter(|p| p[1] > 1.2)
            .count()
    };
    for lod in 0..2 {
        assert!(
            roof_glass(&build_lod(&spec, 0, lod).unwrap()) > 8,
            "lod{lod}"
        );
    }
    let mut solid = spec.clone();
    solid.details.retain(|&d| d != Detail::TTop);
    assert_eq!(roof_glass(&build(&solid, 0).unwrap()), 0);
}

#[test]
fn palette_choice_only_changes_paint() {
    let spec = load(GANNET);
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
    let spec = load(GANNET);
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

/// Body, interior and glass vertices over the middle of a targa's opening, above the side
/// glass's top edge less 5 cm, as `(body, interior)`: where the roof and the headliner were.
fn over_the_targa(spec: &ShapeSpec, car: &CarMesh) -> (usize, usize) {
    let (s0, s1) = (spec.glass.windshield.1 + 0.1, spec.glass.side[0].1 - 0.1);
    let ax = spec.wheels.front_x;
    let b = body::Body::new(spec, 0, 0);
    let f = b.frame((s0 + s1) * 0.5);
    let roofline = f.y_edge - 0.05;
    let count = |m: &MeshData| {
        m.positions
            .iter()
            .filter(|p| p[2] + ax > s0 && p[2] + ax < s1)
            .filter(|p| p[0].abs() < f.z_re - 0.12 && p[1] > roofline - 0.06)
            .count()
    };
    (count(&car.body) + count(&car.glass), count(&car.interior))
}

#[test]
fn a_targa_opens_the_roof_over_the_front_seats() {
    let targa = load(TARGA);
    let coupe = load(GT);
    for lod in 0..2 {
        let open = over_the_targa(&targa, &build_lod(&targa, 0, lod).unwrap());
        assert_eq!(open, (0, 0), "lod{lod}");
    }
    let (roof, headliner) = over_the_targa(&coupe, &build(&coupe, 0).unwrap());
    assert!(roof > 8 && headliner > 8, "{roof} {headliner}");
    // The same shell otherwise: the side glass stays up.
    let side_glass = |spec: &ShapeSpec| {
        let w = spec.glass.side[0];
        let ax = spec.wheels.front_x;
        let car = build(spec, 0).unwrap();
        car.glass
            .positions
            .iter()
            .filter(|p| p[2] + ax > w.0 + 0.05 && p[2] + ax < w.1 - 0.05 && p[0].abs() > 0.5)
            .count()
    };
    assert!(side_glass(&targa) > 4);
    assert_eq!(side_glass(&targa), side_glass(&coupe));
    // A targa needs a hardtop with a door window to end at.
    rejects(TARGA, |s| s.glass.side.clear(), ShapeError::Targa);
    rejects(
        TARGA,
        |s| s.roof.kind = RoofKind::Convertible { top: Top::Down },
        ShapeError::Targa,
    );
}

/// The lowest body vertex and the highest one behind the rear axle, model space.
fn low_and_tail_top(car: &CarMesh, spec: &ShapeSpec) -> (f32, f32) {
    let rear = spec.wheels.rear_x - spec.wheels.front_x;
    let low = car.body.positions.iter().fold(f32::MAX, |m, p| m.min(p[1]));
    let top = car
        .body
        .positions
        .iter()
        .filter(|p| p[2] > rear)
        .fold(0.0f32, |m, p| m.max(p[1]));
    (low, top)
}

#[test]
fn the_tuner_wears_a_kit_a_tall_wing_rims_and_underglow() {
    let tuner = load(TUNER);
    let mut plain = tuner.clone();
    plain
        .details
        .retain(|d| !matches!(d, Detail::BodyKit | Detail::Underglow(_)));
    plain.rear.spoiler = Spoiler::Wing;
    plain.parts.rims = Rims::Alloy5;
    for lod in 0..3 {
        let (kit, bare) = (
            build_lod(&tuner, 0, lod).unwrap(),
            build_lod(&plain, 0, lod).unwrap(),
        );
        let ((kit_low, tall), (bare_low, low_wing)) = (
            low_and_tail_top(&kit, &tuner),
            low_and_tail_top(&bare, &plain),
        );
        // The kit hangs lower than the sills; the tall wing stands well over a plain one.
        assert!(
            kit_low < bare_low - 0.04,
            "lod{lod}: {kit_low} vs {bare_low}"
        );
        assert!(kit_low > 0.05, "lod{lod}: the kit scrapes at {kit_low}");
        assert!(tall > low_wing + 0.15, "lod{lod}: {tall} vs {low_wing}");
        // Underglow: lit tubes under the car at the near levels, none far away, none on
        // a car without it; never in the lamp mesh.
        assert!(bare.underglow.positions.is_empty());
        assert_eq!(kit.lights, bare.lights, "lod{lod}");
        let tubes = &kit.underglow;
        if lod == 2 {
            assert!(tubes.positions.is_empty());
            continue;
        }
        assert!(tubes.triangle_count() > 20, "lod{lod}");
        let blue = emissive(TubeColor::Blue.rgb(), 3.0);
        assert!(tubes.emissive.iter().all(|e| *e == blue));
        assert!(tubes.positions.iter().all(|p| p[1] > 0.05 && p[1] < 0.2));
    }
    // The aftermarket rim is bigger, on a lower-profile tire.
    let rim = |spec: &ShapeSpec| {
        let w = build(spec, 0).unwrap().wheels[0].clone();
        // The rim's bright lip is the widest chrome ring on the outer face.
        let chrome = Materials::new(spec, 0, None).chrome.color;
        w.positions
            .iter()
            .zip(&w.colors)
            .filter(|(_, c)| **c == chrome)
            .fold(0.0f32, |m, (p, _)| m.max(p[1].hypot(p[2])))
    };
    let r = tuner.wheels.radius;
    assert!(rim(&tuner) > 0.7 * r && rim(&tuner) < r, "{}", rim(&tuner));
    // Every car but the tuner is dark underneath.
    for spec in shapes() {
        let has = spec
            .details
            .iter()
            .any(|d| matches!(d, Detail::Underglow(_)));
        assert_eq!(has, spec.id == tuner.id, "{}", spec.id);
        assert_eq!(
            build(&spec, 0).unwrap().underglow.positions.is_empty(),
            !has
        );
    }
}

#[test]
fn a_blower_stands_through_the_hood() {
    let muscle = load(MUSCLE);
    let s = muscle.glass.windshield.0 * 0.55;
    let z = s - muscle.wheels.front_x;
    let hood = body::Body::new(&muscle, 0, 0).frame(s).yt;
    let over = |spec: &ShapeSpec, lod: u8| {
        build_lod(spec, 0, lod)
            .unwrap()
            .body
            .positions
            .iter()
            .filter(|p| (p[2] - z).abs() < 0.3 && p[0].abs() < 0.25)
            .fold(0.0f32, |m, p| m.max(p[1]))
    };
    let mut bare = muscle.clone();
    bare.details.retain(|&d| d != Detail::Blower);
    for lod in 0..3 {
        assert!(over(&muscle, lod) > hood + 0.3, "lod{lod}");
        assert!(over(&bare, lod) < hood + 0.05, "lod{lod}");
    }
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
        GANNET,
        |s| s.plan.swap(2, 3),
        ShapeError::PlanNotIncreasing { index: 3 },
    );
    rejects(
        GANNET,
        |s| s.plan[1].0 = 0.0,
        ShapeError::PlanNotIncreasing { index: 1 },
    );
    rejects(GANNET, |s| s.plan[0].0 = 0.05, ShapeError::PlanSpan);
    rejects(
        GANNET,
        |s| s.plan[2].1 = 0.0,
        ShapeError::NonPositiveWidth { index: 2 },
    );
    rejects(
        GANNET,
        |s| s.side[3].0 = 0.3,
        ShapeError::SideTopNotIncreasing { index: 3 },
    );
    rejects(
        GANNET,
        |s| s.side[13].0 = 3.9,
        ShapeError::SideBottomNotDecreasing { index: 13 },
    );
    rejects(GANNET, |s| s.side[0].0 = 0.01, ShapeError::SideNotAtNose);
    rejects(GANNET, |s| s.side.truncate(4), ShapeError::SidePointCount);
    rejects(GANNET, |s| s.side[11].1 = 1.5, ShapeError::SideInverted);
    rejects(
        GANNET,
        |s| s.belt.push((1.0, 0.8)),
        ShapeError::BeltNotIncreasing { index: 2 },
    );
    rejects(GANNET, |s| s.side[4].1 = f32::NAN, ShapeError::NonFinite);
}

/// Body vertices within a tire's radius of hub `c` (model space) and its width, on its side.
fn on_the_hub(m: &MeshData, c: [f32; 3], w: &Wheels) -> usize {
    m.positions
        .iter()
        .filter(|p| {
            let (dy, dz) = (p[1] - c[1], p[2] - c[2]);
            p[0] * c[0] > 0.0
                && (p[0].abs() - c[0].abs()).abs() <= w.width * 0.5 + 0.01
                && dy * dy + dz * dz <= w.radius * w.radius * 1.01
        })
        .count()
}

#[test]
fn fixed_axles_stand_wheels_under_the_body() {
    let semi = load(SEMI);
    assert_eq!(semi.axles.len(), 3);
    let w = semi.wheels;
    for lod in 0..3 {
        let car = build_lod(&semi, 0, lod).unwrap();
        for &ax in &semi.axles {
            for side in [-1.0, 1.0] {
                let hub = [side * w.track * 0.5, w.radius, ax - w.front_x];
                assert!(
                    on_the_hub(&car.body, hub, &w) >= 12,
                    "lod{lod}: no wheel at {hub:?}"
                );
            }
        }
        // The road wheels stay four, at the two road axles.
        assert_eq!(car.wheel_centers[2][2], w.rear_x - w.front_x);
    }
    // A car without fixed axles builds as before (the mesh hashes above).
    assert!(shapes().iter().filter(|s| !s.axles.is_empty()).count() == 1);
    // An axle in a low body, on a road axle, past the tail or one of five is refused.
    for bad in [
        alloc::vec![2.5],
        alloc::vec![1.5],
        alloc::vec![17.4],
        alloc::vec![5.3, 5.9],
        alloc::vec![5.3, 6.45, 7.6, 14.1, 15.25],
    ] {
        rejects(SEMI, |s| s.axles = bad, ShapeError::WheelPlacement);
    }
}

#[test]
fn validate_rejects_bad_parts() {
    rejects(GANNET, |s| s.seams.doors = 3, ShapeError::DoorCount);
    rejects(
        GANNET,
        |s| s.palette[2] = "red".to_string(),
        ShapeError::BadColor { index: 2 },
    );
    rejects(GANNET, |s| s.palette.clear(), ShapeError::EmptyPalette);
    rejects(
        GANNET,
        |s| s.glass.windshield = (2.0, 1.5),
        ShapeError::GlassRange,
    );
    rejects(
        HOMESTEAD,
        |s| s.glass.side[1].0 = 2.5,
        ShapeError::SideWindow { index: 1 },
    );
    rejects(
        GANNET,
        |s| s.wheels.front_x = 0.2,
        ShapeError::WheelPlacement,
    );
    rejects(GANNET, |s| s.wheels.track = 1.9, ShapeError::TrackTooWide);
    rejects(
        GANNET,
        |s| s.section.shoulder = 1.2,
        ShapeError::SectionOutOfRange,
    );
    rejects(GANNET, |s| s.interior.seat.0 = 1.9, ShapeError::Interior);
    rejects(
        GANNET,
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
        HOMESTEAD,
        |s| s.roof.kind = RoofKind::Convertible { top: Top::Down },
        ShapeError::OpenTop,
    );
    rejects(
        SEABREEZE,
        |s| s.color_details.push((9, Detail::WoodPanel)),
        ShapeError::ColorDetail,
    );
    rejects(
        GANNET,
        |s| s.palette[0] = "#ffffff/".to_string(),
        ShapeError::BadColor { index: 0 },
    );
    rejects(
        GANNET,
        |s| s.interior.insert = "red".to_string(),
        ShapeError::Interior,
    );
    // A bed needs a top at the belt behind the cab, a meter of it, and a hardtop.
    rejects(
        GANNET,
        |s| s.details.push(Detail::PickupBed),
        ShapeError::Bed,
    );
    rejects(PICKUP, |s| s.glass.backlight = (4.0, 4.2), ShapeError::Bed);
    rejects(
        PICKUP,
        |s| s.roof.kind = RoofKind::Convertible { top: Top::Down },
        ShapeError::OpenTop,
    );
}

#[test]
fn two_tone_paint_splits_at_the_crease() {
    for (text, k) in [(GANNET, 6), (REGENT, 0), (HOMESTEAD, 0)] {
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
    let spec = load(HOMESTEAD);
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
    // Same geometry (a two-tone welds into a few more vertices, never more triangles).
    assert_eq!(own.body.triangle_count(), era.body.triangle_count());
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
    let spoilers = [
        Spoiler::None,
        Spoiler::Lip,
        Spoiler::Wing,
        Spoiler::TallWing,
    ];
    let rims = [
        Rims::Steel,
        Rims::Alloy5,
        Rims::Mesh,
        Rims::Turbine,
        Rims::Dish,
        Rims::Slotted,
        Rims::Twin,
    ];
    let details = [
        Detail::Louvers,
        Detail::HoodScoop,
        Detail::SideStripe,
        Detail::WoodPanel,
        Detail::ChromeTrim,
        Detail::VinylRoof,
        Detail::LandauRoof,
        Detail::RallyStripes,
        Detail::MudFlaps,
        Detail::RoofRack,
        Detail::BlackTrim,
        Detail::Badge(BadgeShape::Diamond),
        Detail::Badge(BadgeShape::Circle),
        Detail::Badge(BadgeShape::Bar),
        Detail::Badge(BadgeShape::Chevron),
        Detail::WoodApplique,
        Detail::TTop,
        Detail::SpareTire,
        Detail::CorneringLamps,
        Detail::SideStrakes,
        Detail::NacaDucts,
        Detail::EngineLouvers,
        Detail::BodyKit,
        Detail::Blower,
        Detail::Underglow(TubeColor::Blue),
        Detail::Underglow(TubeColor::Violet),
        Detail::Underglow(TubeColor::Green),
    ];
    // Each shape starts the option lists somewhere else, so every rim and detail is built.
    for (i, base) in shapes().into_iter().enumerate() {
        for k in 0..5 {
            let mut s = base.clone();
            s.front.lights = heads[k % heads.len()];
            s.front.grille = grilles[k % grilles.len()];
            s.front.bumper = bumpers[k % bumpers.len()];
            s.rear.lights = tails[k % tails.len()];
            s.rear.spoiler = spoilers[k % spoilers.len()];
            s.rear.bumper = bumpers[(k + 1) % bumpers.len()];
            s.parts.rims = rims[(i + k) % rims.len()];
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
            s.details = details
                .iter()
                .copied()
                .cycle()
                .skip((i + k * 3) % details.len())
                .take(4)
                .collect();
            for lod in 0..3 {
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
