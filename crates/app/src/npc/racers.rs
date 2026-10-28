//! The basin's set pieces that drive (`traffic::race`) against the player's car, through
//! the app: a player stopped on the race start's line is never touched while the racers
//! stand beside it and go, and a car driven into a racer or a convoy truck is a thud, never
//! a launch or a hard shove.

use traffic::race::{RacePhase, CONVOY_MPS, CONVOY_TAG, RACE_TAG};
use traffic::{Density, Script, NONE};

use crate::drive::Drive;
use crate::input::{Action, InputState};
use crate::{App, SceneSetup};

/// The 2001 world, seed 1, at `t`, its traffic `traffic`.
fn basin(t: &str, traffic: &str) -> App {
    let q = format!("?seed=1&scenario=mirage_valley_2001&t={t}&traffic={traffic}&stream=1");
    App::from_setup(&SceneSetup::from_query(&q).unwrap()).unwrap()
}

/// The car at ground point `at` facing ground direction `dir`, standing.
fn put(app: &mut App, at: [f32; 2], dir: [f32; 2]) {
    let (y, _) = app.ground.level_at(at);
    let heading = (-dir[0]).atan2(-dir[1]);
    app.drive = Drive::with_layout(
        app.drive.spec.clone(),
        app.drive.layout,
        [at[0], y, at[1]],
        heading,
    );
}

/// What a stretch of driving did to the player's car: the fastest it went up or down,
/// the most its ground speed rose in one frame, its ground speed at the end and the
/// nearest it came to an NPC car's middle.
struct Ride {
    up: f32,
    kick: f32,
    speed: f32,
    near: f32,
}

/// Runs `secs` of the game holding the throttle (or nothing).
fn run(app: &mut App, secs: f32, throttle: bool) -> Ride {
    let mut input = InputState::default();
    let dt = 1.0 / 60.0;
    let ground = |a: &App| {
        let v = a.drive.state.vel;
        (v[0] * v[0] + v[2] * v[2]).sqrt()
    };
    let mut ride = Ride {
        up: 0.0,
        kick: 0.0,
        speed: 0.0,
        near: f32::INFINITY,
    };
    let mut t = 0.0;
    while t < secs {
        input.clear();
        if throttle {
            input.set(Action::Throttle, 1.0);
        }
        let before = ground(app);
        app.update(&input, f64::from(dt));
        ride.up = ride.up.max(app.drive.state.vel[1].abs());
        ride.kick = ride.kick.max(ground(app) - before);
        let p = app.drive.state.pos;
        for c in app.npcs.cars() {
            ride.near = ride
                .near
                .min(((c.pos[0] - p[0]).powi(2) + (c.pos[2] - p[2]).powi(2)).sqrt());
        }
        t += dt;
    }
    ride.speed = ground(app);
    ride
}

/// The traffic shape of the world's first hero car (a racer's).
fn hero_shape(app: &App) -> u8 {
    let h = crate::scenario::lineup(app.ui.scenario()).heroes[0];
    app.npcs
        .shapes
        .iter()
        .position(|&k| k == h)
        .expect("a hero shape") as u8
}

#[test]
fn a_player_stopped_on_the_line_is_never_touched_while_the_racers_go() {
    let mut app = basin("21:00", "normal");
    let c = app
        .npcs
        .drivers
        .race
        .as_ref()
        .expect("a race start")
        .course
        .clone();
    // On the line in the middle lane, stopped, facing down the course.
    let at = app.npcs.graph.pose(c.lanes[1], c.line_s[1] - 2.0).pos;
    put(&mut app, [at[0], at[2]], c.dir);
    let mut ran = false;
    let mut moved = 0.0f32;
    let start = app.drive.state.pos;
    for _ in 0..90 {
        let ride = run(&mut app, 1.0, false);
        assert!(ride.up < 0.3, "lifted: {:.2} m/s", ride.up);
        let p = app.drive.state.pos;
        moved = moved.max(((p[0] - start[0]).powi(2) + (p[2] - start[2]).powi(2)).sqrt());
        let r = app.npcs.drivers.race.as_ref().unwrap();
        ran |= r.phase == RacePhase::Running || r.runs > 0;
    }
    assert!(ran, "the racers ran");
    assert!(moved < 0.3, "the player's car was moved {moved:.2} m");
}

#[test]
fn a_car_driven_into_a_racer_or_a_truck_is_a_thud_not_a_launch() {
    // A racer standing at the line in the first lane, hit from beside across the lanes,
    // square and at 40 degrees, at full throttle.
    for angle in [0.0f32, 40.0] {
        let mut app = basin("21:00", "off");
        let c = app.npcs.drivers.race.as_ref().unwrap().course.clone();
        let shape = hero_shape(&app);
        let g = &app.npcs.graph;
        let pose = g.pose(c.lanes[0], c.line_s[0] - 0.5);
        let script = Script {
            tag: RACE_TAG,
            stand_seg: c.lanes[0],
            stand_s: c.line_s[0] - 0.5,
            ..Script::NONE
        };
        let lane = c.lanes[0];
        let s = c.line_s[0] - 0.5;
        app.npcs
            .traffic
            .spawn_scripted(&app.npcs.graph, lane, s, 0.0, shape, script)
            .expect("room");
        app.npcs.set_density(Density::Off);
        // From the far lane, aimed across at its middle (square, or from behind at an
        // angle), once settled.
        let far = app.npcs.graph.pose(c.lanes[2], c.line_s[2] - 0.5).pos;
        let middle = [
            pose.pos[0] - 2.0 * pose.dir[0],
            pose.pos[2] - 2.0 * pose.dir[1],
        ];
        let d = [middle[0] - far[0], middle[1] - far[2]];
        let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let across = [d[0] / n, d[1] / n];
        let (sn, cs) = angle.to_radians().sin_cos();
        let aim = [
            across[0] * cs - pose.dir[0] * sn,
            across[1] * cs - pose.dir[1] * sn,
        ];
        let from = [middle[0] - aim[0] * n, middle[1] - aim[1] * n];
        put(&mut app, from, aim);
        run(&mut app, 1.5, false);
        let ride = run(&mut app, 4.0, true);
        assert!(ride.up <= 0.3, "{angle}: launched {:.2} m/s up", ride.up);
        assert!(
            ride.kick < 1.0,
            "{angle}: shoved {:.2} m/s in a frame",
            ride.kick
        );
        assert!(
            ride.near < 5.0 && ride.speed < 2.0,
            "{angle}: it reached the racer and stopped: {:.1} m, {:.1} m/s",
            ride.near,
            ride.speed
        );
    }

    // A convoy truck coming up behind the player's car standing in its lane: it stops
    // short and never touches it.
    let mut app = basin("06:00", "off");
    let v = app.npcs.drivers.convoy.as_ref().expect("a convoy").clone();
    let shape = app
        .npcs
        .shapes
        .iter()
        .position(|&k| crate::vehicles::BUILTIN_SHAPES[k].0 == "anvil_semi.shape.ron")
        .unwrap() as u8;
    let truck = |stand: f32| Script {
        tag: CONVOY_TAG,
        v0_mps: CONVOY_MPS,
        stand_seg: if stand > 0.0 { v.lane } else { NONE },
        stand_s: stand,
        ..Script::NONE
    };
    app.npcs
        .traffic
        .spawn_scripted(&app.npcs.graph, v.lane, 30.0, CONVOY_MPS, shape, truck(0.0))
        .expect("room");
    app.npcs.set_density(Density::Off);
    let ahead = app.npcs.graph.pose(v.lane, 200.0);
    put(&mut app, [ahead.pos[0], ahead.pos[2]], ahead.dir);
    run(&mut app, 1.5, false);
    let start = app.drive.state.pos;
    let ride = run(&mut app, 20.0, false);
    let p = app.drive.state.pos;
    let moved = ((p[0] - start[0]).powi(2) + (p[2] - start[2]).powi(2)).sqrt();
    assert!(moved < 0.3 && ride.up < 0.3, "touched: moved {moved:.2} m");
    assert!(
        ride.near < 15.0,
        "the truck came up behind: {:.1} m",
        ride.near
    );

    // A truck standing in its lane, driven into square from the next lane over.
    let mut app = basin("06:00", "off");
    let g = &app.npcs.graph;
    let l = g.lanes[v.lane as usize];
    let next = (0..g.lanes.len() as u32)
        .find(|&m| {
            let o = &g.lanes[m as usize];
            o.edge == l.edge && o.forward == l.forward && o.slot + 1 == l.slot
        })
        .expect("a lane beside");
    app.npcs
        .traffic
        .spawn_scripted(&app.npcs.graph, v.lane, 150.0, 0.0, shape, truck(150.0))
        .expect("room");
    app.npcs.set_density(Density::Off);
    run(&mut app, 0.5, false);
    let g = &app.npcs.graph;
    let side = g.pose(v.lane, 150.0 - 8.0).pos;
    let from = g.pose(next, g.lanes[next as usize].length_m.min(142.0)).pos;
    let d = [side[0] - from[0], side[2] - from[2]];
    let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let aim = [d[0] / n, d[1] / n];
    put(
        &mut app,
        [from[0] - aim[0] * 6.0, from[2] - aim[1] * 6.0],
        aim,
    );
    run(&mut app, 1.5, false);
    let ride = run(&mut app, 4.0, true);
    assert!(ride.up <= 0.3, "launched {:.2} m/s up", ride.up);
    assert!(ride.kick < 1.0, "shoved {:.2} m/s in a frame", ride.kick);
    assert!(
        ride.near < 4.5 && ride.speed < 2.0,
        "it reached the truck and stopped: {:.1} m, {:.1} m/s",
        ride.near,
        ride.speed
    );
}
