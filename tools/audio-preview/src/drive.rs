//! `audio-preview drive [OUT_DIR] [CAR_ID...]`: drives real cars from `assets/vehicles/`
//! through the vehicle sim on flat ground with a scripted driver, and renders what the
//! engine bus hears: the sim's own rpm, throttle, gear and shifts, so the clip is what the
//! game plays. The script: idle, a full-throttle pull through the gears, a lift, a light
//! cruise, a brake to a stop and idle again (21.5 s per car).

use std::path::Path;

use audio::mixer::{Mixer, SAMPLE_RATE};
use audio::synth::{AudioParams, EngineProfile};
use vehicle::sim::{self, Assists, FlatGround, VehicleInput, VehicleState};
use vehicle::VehicleSpec;

/// One car per engine sound, the default set.
pub const CARS: [&str; 13] = [
    "courser_v8",
    "destrier_coupe",
    "avocet_v8",
    "basilisk_v12",
    "falchion_sedan",
    "halberd_coupe",
    "prescott_landau",
    "tanager_awd",
    "pipit_hatch",
    "halcyon_turbo",
    "seabreeze_convertible",
    "shrike_coupe",
    "anvil_semi",
];

const FPS: usize = 60;
const SECONDS: f32 = 21.5;

fn input(t: f32) -> VehicleInput {
    let (throttle, brake) = match t {
        t if t < 2.0 => (0.0, 0.0),
        t if t < 11.0 => (1.0, 0.0),
        t if t < 13.5 => (0.0, 0.0),
        t if t < 17.0 => (0.35, 0.0),
        t if t < 20.0 => (0.0, 0.8),
        _ => (0.0, 0.0),
    };
    VehicleInput {
        throttle,
        brake,
        steer: 0.0,
        handbrake: false,
    }
}

/// Renders one car's drive (interleaved stereo, engine bus only).
pub fn render(spec: &VehicleSpec) -> Vec<f32> {
    let ground = FlatGround {
        y: 0.0,
        friction: 1.0,
    };
    let assists = Assists::default();
    let mut s = VehicleState::spawn(spec, [0.0, 0.0, 0.0], 0.0);
    let mut m = Mixer::new();
    let per = SAMPLE_RATE as usize / FPS;
    let frames = (SECONDS * FPS as f32) as usize;
    let steps = (1.0 / sim::DT).round() as usize / FPS;
    let mut out = vec![0.0; frames * per * 2];
    let profile = EngineProfile::from_name(&spec.feel.engine_sound);
    for (i, chunk) in out.chunks_exact_mut(per * 2).enumerate() {
        let t = i as f32 / FPS as f32;
        for _ in 0..steps {
            sim::step(spec, &mut s, &input(t), &assists, &ground);
        }
        let mut p = AudioParams {
            world_volume: 0.0,
            ..AudioParams::default()
        };
        p.engine.profile = profile;
        p.engine.rpm = s.engine_rpm;
        p.engine.idle_rpm = spec.engine.idle_rpm;
        p.engine.redline_rpm = spec.engine.redline_rpm;
        p.engine.throttle = if s.gear < 0 { s.brake } else { s.throttle };
        p.engine.gear = s.gear;
        p.engine.shift_ms = spec.shift_ms;
        p.speed_mps = s.speed_mps;
        m.set_params(&p);
        m.render(chunk);
    }
    out
}

pub fn run(out_dir: &Path, ids: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let default: Vec<String> = CARS.iter().map(|s| s.to_string()).collect();
    let ids = if ids.is_empty() { &default[..] } else { ids };
    for id in ids {
        let path = format!("assets/vehicles/{id}.ron");
        let text = std::fs::read_to_string(&path)?;
        let spec: VehicleSpec = ron::from_str(&text).expect("a valid spec");
        let x = render(&spec);
        let name = format!("drive_{id}.wav");
        crate::write_wav(&out_dir.join(&name), &x)?;
        crate::levels(&format!("{name} ({})", spec.feel.engine_sound), &x);
    }
    Ok(())
}
