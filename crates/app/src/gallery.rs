//! The landmark gallery's contact sheets: where `tools/screenshot/shoot.mjs --gallery <kit>`
//! puts the car and the camera to shoot each variant of a landmark kit
//! (`world::landmarks::gallery`, `place=gallery`) from a few fixed cameras at noon, golden
//! hour, dusk and night, before it tiles the shots into one sheet per kit.
//!
//! The shots are written to `tools/screenshot/gallery.json` by
//! `cargo test -p app -- --ignored write_gallery`; a test fails while that file is stale.
//! Every camera stands south of its lot on open ground and faces the landmark's middle:
//! - `front`: the hood camera straight on, far enough back that the top fits the frame;
//! - `quarter`: the hood camera from 35° east of straight on, as far back;
//! - `street`: the far chase camera from 20° west, a little farther, the car in the frame
//!   for scale.

use serde_json::{json, Map, Value};
use world::landmarks::{gallery_lots, letter, Kit, GALLERY_SETBACK_M};

/// The times of day every shot is taken at: `(name, clock, headlights)`. Dusk is after the
/// street lamps strike, with blue still in the sky. Night leaves the headlights off: their
/// pool on the lawn outshines the landmark's own lamps.
pub const TIMES: [(&str, &str, bool); 4] = [
    ("noon", "12:00", false),
    ("golden", "17:40", false),
    ("dusk", "18:45", false),
    ("night", "22:30", false),
];

/// A camera: `(name, scene camera, bearing from straight on in degrees east, distance as a
/// share of the fitting distance)`.
const CAMERAS: [(&str, &str, f32, f32); 3] = [
    ("front", "hood", 0.0, 1.0),
    ("quarter", "hood", 35.0, 1.0),
    ("street", "chase-far", -20.0, 1.2),
];

/// The hood camera's eye over the ground, m, and the share of its half field of view the
/// landmark's top may take.
const EYE_M: f32 = 1.25;
const TOP_SHARE: f32 = 0.8;
/// The nearest a camera stands to a landmark's middle, m.
const MIN_DISTANCE_M: f32 = 30.0;
/// The least a camera stands south of its row's road, m.
const ROAD_CLEAR_M: f32 = 12.0;

/// One shot of the sheet, before the time of day.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub variant: char,
    pub camera: &'static str,
    pub cam: &'static str,
    /// Where the car stands, `[x, z]`, and its compass heading, degrees clockwise from north.
    pub pos: [f32; 2],
    pub yaw_deg: f32,
}

/// Rounds to one decimal, so the JSON stays short and exact.
fn tenth(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// The shots of kit `k` (an index into `kits`) in the gallery of `kits`: by variant, then
/// by camera.
pub fn shots(kits: &[Kit], k: usize) -> Vec<Shot> {
    let half_fov = (crate::camera::TUNING.hood_fov_deg * 0.5).to_radians();
    let mut out = Vec::new();
    for lot in gallery_lots(kits).iter().filter(|l| l.kit == k) {
        let kit = &kits[k];
        let (_, hi) = kit.bounds(lot.variant);
        // The landmark's middle: halfway back on its lot.
        let mid = [lot.front[0], lot.front[1] - 0.5 * kit.lot[1]];
        let fit = ((hi[1] * 1.05 - EYE_M) / (half_fov * TOP_SHARE).tan()).max(MIN_DISTANCE_M);
        for (camera, cam, bearing, share) in CAMERAS {
            let (s, c) = bearing.to_radians().sin_cos();
            // Back from the lot's middle, and always south of the row's road and walks.
            let clear = (0.5 * kit.lot[1] + GALLERY_SETBACK_M + ROAD_CLEAR_M) / c;
            let d = ((fit * share).max(MIN_DISTANCE_M) + 0.5 * kit.lot[1]).max(clear);
            let pos = [mid[0] + s * d, mid[1] + c * d];
            let (dx, dz) = (mid[0] - pos[0], mid[1] - pos[1]);
            let yaw = dx.atan2(-dz).to_degrees();
            out.push(Shot {
                variant: letter(lot.variant),
                camera,
                cam,
                pos: [tenth(pos[0]), tenth(pos[1])],
                yaw_deg: tenth(yaw),
            });
        }
    }
    out
}

/// The scene keys of `shot` but the time of day (the sheet tool adds `t` and `lights`).
fn params(shot: &Shot) -> Value {
    json!({
        "place": "gallery",
        "seed": "1",
        "car": crate::drive::DEFAULT_VEHICLE,
        "cam": shot.cam,
        "pos": format!("{},{}", shot.pos[0], shot.pos[1]),
        "yaw": format!("{}", shot.yaw_deg),
        "still": "1",
        "stream": "0",
    })
}

/// `tools/screenshot/gallery.json`: the times, and per kit its variants and shots.
pub fn sheet_json(kits: &[Kit]) -> Value {
    let mut by_kit = Map::new();
    for (k, kit) in kits.iter().enumerate() {
        let variants: Vec<Value> = kit
            .variants
            .iter()
            .enumerate()
            .map(|(i, v)| json!({ "letter": letter(i).to_string(), "name": v.name, "about": v.about }))
            .collect();
        let shots: Vec<Value> = shots(kits, k)
            .iter()
            .map(|s| {
                json!({
                    "variant": s.variant.to_string(),
                    "camera": s.camera,
                    "params": params(s),
                })
            })
            .collect();
        by_kit.insert(
            kit.id.clone(),
            json!({
                "about": kit.about,
                "tone": format!("{:?}", kit.tone),
                "variants": variants,
                "shots": shots,
            }),
        );
    }
    let times: Vec<Value> = TIMES
        .iter()
        .map(|(name, t, lights)| json!({ "name": name, "t": t, "lights": lights }))
        .collect();
    json!({
        "about": "Landmark gallery contact sheets (crates/app/src/gallery.rs); written by `cargo test -p app -- --ignored write_gallery`.",
        "times": times,
        "kits": Value::Object(by_kit),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::places::landmarks;

    const COMMITTED: &str = include_str!("../../../tools/screenshot/gallery.json");

    fn pretty() -> String {
        let mut s = serde_json::to_string_pretty(&sheet_json(landmarks())).unwrap();
        s.push('\n');
        s
    }

    #[test]
    fn the_committed_sheet_plan_is_current() {
        let committed: Value = serde_json::from_str(COMMITTED).expect("gallery.json parses");
        assert_eq!(
            committed,
            sheet_json(landmarks()),
            "tools/screenshot/gallery.json is stale; run \
             `cargo test -p app -- --ignored write_gallery`. It should read:\n{}",
            pretty()
        );
    }

    #[test]
    fn every_camera_stands_on_open_ground_in_the_gallery_facing_its_landmark() {
        let kits = landmarks();
        let s = crate::scenario::get(0);
        let place =
            crate::places::build_in(crate::PlaceKey::Gallery, skywave_core::Seed(1), s).unwrap();
        let o = place.header.origin_m();
        let e = place.header.extent_m();
        let lots = gallery_lots(kits);
        for k in 0..kits.len() {
            let shots = shots(kits, k);
            assert_eq!(shots.len(), kits[k].variants.len() * CAMERAS.len());
            for shot in &shots {
                let [x, z] = shot.pos;
                assert!(x > o[0] + 20.0 && x < o[0] + e[0] - 20.0, "{shot:?}");
                assert!(z > o[1] + 20.0 && z < o[1] + e[1] - 20.0, "{shot:?}");
                // Clear of every lot and south of its row's road and walks.
                for l in &lots {
                    let [w, d] = kits[l.kit].lot;
                    let inside = (x - l.front[0]).abs() < 0.5 * w + 3.0
                        && z < l.front[1] + 3.0
                        && z > l.front[1] - d - 3.0;
                    assert!(!inside, "{shot:?} stands on a lot");
                }
                assert!(z > world::landmarks::row_z(k) + 10.0, "{shot:?}");
                // It faces the middle of its landmark.
                let lot = lots
                    .iter()
                    .find(|l| l.kit == k && letter(l.variant) == shot.variant)
                    .unwrap();
                let mid = [lot.front[0], lot.front[1] - 0.5 * kits[k].lot[1]];
                let yaw = shot.yaw_deg.to_radians();
                let (dx, dz) = (mid[0] - x, mid[1] - z);
                let len = (dx * dx + dz * dz).sqrt();
                let ahead = (yaw.sin() * dx - yaw.cos() * dz) / len;
                assert!(ahead > 0.999, "{shot:?} looks away ({ahead})");
            }
        }
        // The water tower's front camera fits its top: about 85 m back.
        let tower = kits.iter().position(|k| k.id == "water_tower").unwrap();
        let front = &shots(kits, tower)[0];
        assert_eq!((front.variant, front.camera), ('A', "front"));
        assert!(
            (70.0..110.0).contains(&(front.pos[1] - (world::landmarks::row_z(tower) - 24.0))),
            "{front:?}"
        );
    }

    #[test]
    fn landmarks_block_a_car_and_nothing_stands_by_a_camera_or_on_a_road() {
        let kits = landmarks();
        let s = crate::scenario::get(0);
        let place =
            crate::places::build_in(crate::PlaceKey::Gallery, skywave_core::Seed(1), s).unwrap();
        let c = world::collide::WorldCollider::build(&place, crate::places::grammar_of(s));
        let marks: Vec<_> = c
            .boxes
            .iter()
            .filter(|b| matches!(b.kind, world::props::PropKind::Landmark { .. }))
            .collect();
        for l in gallery_lots(kits) {
            let [w, d] = kits[l.kit].lot;
            let n = marks
                .iter()
                .filter(|b| {
                    (b.c[0] - l.front[0]).abs() < 0.5 * w
                        && b.c[1] < l.front[1]
                        && b.c[1] > l.front[1] - d
                })
                .count();
            // The tower's legs (A, C) or its column (B), and the lamp's pole.
            assert!(n >= 2, "lot {l:?}: {n} boxes");
        }
        for k in 0..kits.len() {
            for shot in shots(kits, k) {
                for b in &marks {
                    let (dx, dz) = (b.c[0] - shot.pos[0], b.c[1] - shot.pos[1]);
                    assert!(dx * dx + dz * dz > 20.0 * 20.0, "{shot:?} by {b:?}");
                }
            }
            // The row's road is clear.
            for b in &marks {
                assert!(
                    (b.c[1] - world::landmarks::row_z(k)).abs() > 8.0,
                    "{b:?} on a road"
                );
            }
        }
    }

    /// Rewrites `tools/screenshot/gallery.json` from the built-in kits.
    #[test]
    #[ignore = "writes tools/screenshot/gallery.json"]
    fn write_gallery() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tools/screenshot/gallery.json"
        );
        std::fs::write(path, pretty()).expect("write gallery.json");
    }
}
