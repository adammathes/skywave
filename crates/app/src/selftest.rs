//! Determinism self-test: the hashes the native test suite pins, computed by whichever
//! build runs this. The web build runs it with `?selftest=1` and prints
//! [`SelfTest::console_line`]; `tools/screenshot/shoot.mjs --selftest` compares that with
//! `tools/screenshot/selftest.json`, which a test here keeps equal to the native values.
//! A mismatch means wasm and native floats diverged.
//!
//! After a deliberate change to any of these hashes, rewrite the file with
//! `cargo test -p app -- --ignored write_selftest`.

use serde_json::{Map, Value};
use skywave_core::Seed;
use vehicle::sim::DT;
use vehicle::{Assists, FlatGround, VehicleInput, VehicleState};
use world::Palette;

use crate::input::{Action, InputState};
use crate::map::MapImage;
use crate::{vehicles, App};

/// The values, in the order they are printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelfTest {
    /// `Place::test_grid().content_hash()`.
    pub place_content_hash: u64,
    /// `world::mesh_hash` of the test town's chunk meshes.
    pub town_mesh_hash: u64,
    /// [`MapImage::hash`] of the test town's map.
    pub map_image_hash: u64,
    /// [`App::state_hash`] after [`run_drive`] in the default car, seed 1.
    pub drive_hash: u64,
    /// `VehicleState::hash` after the vehicle crate's 10 s replay (corsair, palisade).
    pub replay_hashes: [u64; 2],
}

impl SelfTest {
    pub fn run() -> SelfTest {
        let place = place::Place::test_grid();
        let meshes = world::build_chunk_meshes(&place, &Palette::socal_1985());
        let mut app = App::new(Seed(1));
        run_drive(&mut app);
        SelfTest {
            place_content_hash: place.content_hash(),
            town_mesh_hash: world::mesh_hash(&meshes),
            map_image_hash: MapImage::rasterize(&place).hash(),
            drive_hash: app.state_hash(),
            replay_hashes: [replay("corsair_gtx"), replay("palisade_wagon")],
        }
    }

    /// Name and value of every hash, values as `0x`-prefixed hex strings (a u64 does not
    /// fit a JSON number).
    pub fn fields(&self) -> Map<String, Value> {
        let hex = |v: u64| Value::String(format!("{v:#018x}"));
        let mut m = Map::new();
        m.insert("place_content_hash".into(), hex(self.place_content_hash));
        m.insert("town_mesh_hash".into(), hex(self.town_mesh_hash));
        m.insert("map_image_hash".into(), hex(self.map_image_hash));
        m.insert("drive_hash".into(), hex(self.drive_hash));
        m.insert("replay_corsair_gtx".into(), hex(self.replay_hashes[0]));
        m.insert("replay_palisade_wagon".into(), hex(self.replay_hashes[1]));
        m
    }

    /// The one console line the web self-test prints: `{"skywave_selftest": {...}}`.
    pub fn console_line(&self) -> String {
        let mut outer = Map::new();
        outer.insert("skywave_selftest".into(), Value::Object(self.fields()));
        Value::Object(outer).to_string()
    }
}

/// Length of the scripted drive, s.
pub const DRIVE_S: f64 = 9.0;

/// Frame time of frame `frame` of the scripted drive: jittery around 60 Hz.
pub fn drive_frame_dt(frame: u32) -> f64 {
    1.0 / 60.0 + f64::from(frame % 7) * 0.0013 - 0.003
}

/// The scripted drive's input at `t` s: 5 s full throttle, 2 s steering right, 2 s braking.
pub fn drive_input(t: f64, input: &mut InputState) {
    input.clear();
    if t < 5.0 {
        input.set(Action::Throttle, 1.0);
    } else if t < 7.0 {
        input.set(Action::Steer, 0.6);
    } else {
        input.set(Action::Brake, 1.0);
    }
}

/// Runs the scripted drive through the whole [`App`].
pub fn run_drive(app: &mut App) {
    let mut input = InputState::default();
    let (mut t, mut frame) = (0.0, 0);
    while t < DRIVE_S {
        drive_input(t, &mut input);
        let dt = drive_frame_dt(frame);
        app.update(&input, dt);
        t += dt;
        frame += 1;
    }
}

/// The vehicle crate's replay (`crates/vehicle/tests/sim.rs`, `replay`): 10 s on flat
/// ground from the origin facing north, full throttle, a steering sweep, a stop and a
/// handbrake flick. Kept step for step the same; a test checks the hashes agree with the
/// ones pinned there.
fn replay(id: &str) -> u64 {
    const GROUND: FlatGround = FlatGround {
        y: 0.0,
        friction: 1.0,
    };
    let spec = vehicles::builtin_spec(id).expect("built-in car");
    let mut s = VehicleState::spawn(&spec, [0.0, 0.0, 0.0], 0.0);
    for i in 0..10 * 120 {
        vehicle::step(
            &spec,
            &mut s,
            &replay_input(i),
            &Assists::default(),
            &GROUND,
        );
    }
    s.hash()
}

fn replay_input(i: usize) -> VehicleInput {
    let t = i as f32 * DT;
    // A triangle wave keeps the script free of platform `sin`.
    let tri = |t: f32, period: f32| {
        let p = (t / period).fract();
        if p < 0.5 {
            4.0 * p - 1.0
        } else {
            3.0 - 4.0 * p
        }
    };
    let pedals = |throttle, brake, steer| VehicleInput {
        throttle,
        brake,
        steer,
        handbrake: false,
    };
    if t < 3.0 {
        pedals(1.0, 0.0, 0.0)
    } else if t < 6.5 {
        pedals(0.6, 0.0, 0.8 * tri(t - 3.0, 1.75))
    } else if t < 8.5 {
        pedals(0.0, 1.0, 0.0)
    } else {
        VehicleInput {
            throttle: 0.4,
            brake: 0.0,
            steer: -0.5,
            handbrake: t < 9.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMITTED: &str = include_str!("../../../tools/screenshot/selftest.json");

    fn pretty(t: &SelfTest) -> String {
        let mut s = serde_json::to_string_pretty(&Value::Object(t.fields())).unwrap();
        s.push('\n');
        s
    }

    #[test]
    fn selftest_matches_the_pins_and_the_committed_json() {
        let t = SelfTest::run();
        // The same values the crates' own golden tests pin.
        assert_eq!(t.place_content_hash, 0x324D_158F_EC2A_D198);
        assert_eq!(t.town_mesh_hash, 0xF908_E7B3_4AA8_73B6);
        assert_eq!(t.map_image_hash, 0x8a97_cf1e_db61_56ae);
        assert_eq!(t.drive_hash, 0x3bd9_240f_f5b3_05d0);
        assert_eq!(
            t.replay_hashes,
            [0x90e4_6e3d_5a39_5dc1, 0xaa95_1912_809c_e63b]
        );
        let committed: Value = serde_json::from_str(COMMITTED).expect("selftest.json parses");
        assert_eq!(
            committed,
            Value::Object(t.fields()),
            "tools/screenshot/selftest.json is stale; run \
             `cargo test -p app -- --ignored write_selftest`. It should read:\n{}",
            pretty(&t)
        );
        let line: Value = serde_json::from_str(&t.console_line()).unwrap();
        assert_eq!(line["skywave_selftest"], committed);
    }

    /// Rewrites `tools/screenshot/selftest.json` from this build's values.
    #[test]
    #[ignore = "writes tools/screenshot/selftest.json"]
    fn write_selftest() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tools/screenshot/selftest.json"
        );
        std::fs::write(path, pretty(&SelfTest::run())).expect("write selftest.json");
    }
}
