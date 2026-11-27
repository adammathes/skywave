//! `audio-preview places [OUT_DIR] [SCENARIO...]`: what the places sound like. For each
//! world, sits the car idling at a road junction in each kind of district (downtown, the
//! strip, a residential street, industry, a park or the hills, beside the highway, by the
//! water) for [`SPOT_S`] seconds, by day and at night, and renders the whole game's sound
//! (engine, world and traffic, through the mixer) to `<scenario>_<day|night>.wav`, the
//! spots one after another with a short crossfade. Prints each spot's zone and position.

use std::path::Path;

use app::{InputMapper, SceneSetup};
use audio::mixer::{Mixer, SAMPLE_RATE};
use place::{Place, ZoneId, Zones};
use skywave_core::Seed;

/// The worlds heard by default.
pub const WORLDS: [&str; 3] = ["mirage_bowl_2001", "bay_loop_2005", "lake_city_1986"];
/// Seconds at each spot, and of the crossfade between spots.
const SPOT_S: f32 = 8.0;
const FADE_S: f32 = 0.6;
/// The day and night clocks.
const TIMES: [(&str, &str); 2] = [("day", "13:00"), ("night", "22:30")];
const FPS: usize = 60;

/// The district kinds visited, in order; each lists the zones that count.
const KINDS: [(&str, &[ZoneId]); 7] = [
    ("downtown", &[ZoneId::Downtown]),
    ("strip", &[ZoneId::Strip, ZoneId::Commercial]),
    ("residential", &[ZoneId::Residential, ZoneId::Suburb]),
    ("industrial", &[ZoneId::Industrial]),
    ("green", &[ZoneId::Park, ZoneId::Hills, ZoneId::Farmland]),
    ("highway", &[ZoneId::Highway]),
    ("water", &[ZoneId::Beach, ZoneId::Water]),
];

fn zone_at(place: &Place, p: [f32; 2]) -> Option<ZoneId> {
    let res = place.terrain.res_per_chunk.max(1);
    let [nx, nz] = Zones::cells_per_side(&place.header, res);
    let cell_m = place.header.chunk_size_m / f32::from(res);
    let o = place.header.origin_m();
    let x = ((p[0] - o[0]) / cell_m).floor();
    let z = ((p[1] - o[1]) / cell_m).floor();
    if x < 0.0 || z < 0.0 || x as usize >= nx || z as usize >= nz {
        return None;
    }
    place.zones.cells.get(z as usize * nx + x as usize).copied()
}

/// The junction most surrounded by `zones` (the zones 40 m and 80 m round it, in eight
/// directions), if any junction is in one.
fn spot(place: &Place, zones: &[ZoneId]) -> Option<[f32; 2]> {
    let is = |p: [f32; 2]| zone_at(place, p).is_some_and(|z| zones.contains(&z));
    let mut best = None;
    for n in &place.roads.nodes {
        if !is(n.pos) {
            continue;
        }
        let mut score = 0;
        for r in [40.0, 80.0] {
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                if is([n.pos[0] + r * a.cos(), n.pos[1] + r * a.sin()]) {
                    score += 1;
                }
            }
        }
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, n.pos));
        }
    }
    best.map(|(_, p)| p)
}

/// `seconds` of the game's sound with the car idling at `pos` in `scenario` at `clock`.
fn sit(scenario: &str, clock: &str, pos: [f32; 2], seconds: f32) -> Result<Vec<f32>, String> {
    let q = format!("scenario={scenario}&t={clock}&pos={},{}", pos[0], pos[1]);
    let mut app = app::App::from_setup(&SceneSetup::from_query(&q)?)?;
    let mut input = InputMapper::default();
    let mut mixer = Mixer::new();
    let per = SAMPLE_RATE as usize / FPS;
    let dt = 1.0 / FPS as f64;
    let clock = std::cell::Cell::new(0.0);
    let now = || clock.get();
    // Settle the car and the traffic before listening.
    for _ in 0..FPS * 3 {
        clock.set(clock.get() + dt);
        app.frame(&mut input, dt, &now);
    }
    let frames = (seconds * FPS as f32) as usize;
    let mut out = vec![0.0; frames * per * 2];
    for chunk in out.chunks_exact_mut(per * 2) {
        clock.set(clock.get() + dt);
        app.frame(&mut input, dt, &now);
        let mut p = *app.audio_params();
        p.paused = false;
        mixer.set_params(&p);
        mixer.render(chunk);
    }
    Ok(out)
}

/// Appends `x` to `out` with a [`FADE_S`] crossfade.
fn splice(out: &mut Vec<f32>, x: &[f32]) {
    let fade = (FADE_S * SAMPLE_RATE as f32) as usize * 2;
    let start = out.len().saturating_sub(fade);
    for (i, &v) in x.iter().enumerate() {
        let j = start + i;
        if j < out.len() {
            let t = (i / 2) as f32 / (fade / 2) as f32;
            out[j] = out[j] * (1.0 - t) + v * t;
        } else {
            out.push(v);
        }
    }
}

pub fn run(dir: &Path, worlds: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let worlds: Vec<&str> = if worlds.is_empty() {
        WORLDS.to_vec()
    } else {
        worlds.iter().map(String::as_str).collect()
    };
    for world in worlds {
        let setup = SceneSetup::from_query(&format!("scenario={world}")).expect("a world");
        let scenario = app::scenario::get(setup.scenario);
        let (place, _) =
            app::places::load(setup.place, Seed(setup.seed), scenario).expect("the place builds");
        let spots: Vec<(&str, [f32; 2])> = KINDS
            .iter()
            .filter_map(|(name, zones)| spot(&place, zones).map(|p| (*name, p)))
            .collect();
        for (time, clock) in TIMES {
            let mut out = Vec::new();
            for (i, (name, pos)) in spots.iter().enumerate() {
                let at = i as f32 * (SPOT_S - FADE_S);
                println!(
                    "{world} {time} {at:5.1}s {name:12} at {:.0},{:.0}",
                    pos[0], pos[1]
                );
                let x = sit(world, clock, *pos, SPOT_S).expect("the scene loads");
                splice(&mut out, &x);
            }
            let name = format!("{world}_{time}.wav");
            crate::write_wav(&dir.join(&name), &out)?;
            crate::levels(&name, &out);
        }
    }
    Ok(())
}
