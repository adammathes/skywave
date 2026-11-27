//! The places a scene can be in ([`PlaceKey`]): the hand-laid test town, a procedural place
//! built at load from its seed and its scenario's region template (and authored layout, if
//! it has one), or the landmark gallery. A scenario names its facade grammar
//! ([`grammar_of`]; the test town uses the first scenario's, [`grammar`]); every grammar
//! letters its shop signs from one lexicon and carries the landmark kits
//! ([`builtin::landmarks`]).
//!
//! A release build bakes each world's place for [`builtin::BAKED_SEED`] at build time
//! (`build.rs`), so the default start decodes it instead of generating it; any other seed,
//! and every debug build, generates at load. Generation is deterministic, so the two are
//! the same place. Native builds carry the baked places in the binary; the web build ships
//! them as files beside the page, one per world, and the platform fetches a world's before
//! loading it and hands it over with [`provide_baked`].

use std::sync::{Mutex, OnceLock};

use place::Place;
use skywave_core::Seed;
use world::template::RegionTemplate;
use world::{ChunkPlan, Grammar, Palette};

use crate::builtin::{self, GRAMMARS};
use crate::scenario::Scenario;
use crate::scene::PlaceKey;

/// `(scenario id, encoded Place)` for every world, baked into the binary by `build.rs`
/// (empty in a debug build and on the web).
const BAKED: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/baked.rs"));

/// Baked places the platform fetched, by scenario id ([`provide_baked`]).
static PROVIDED: Mutex<Vec<(String, &'static [u8])>> = Mutex::new(Vec::new());

/// Hands over scenario `id`'s baked place (the file `build.rs` wrote as `<id>.place`), for
/// the next load of that world at [`builtin::BAKED_SEED`]. It is kept for the session.
pub fn provide_baked(id: &str, bytes: Vec<u8>) {
    let mut provided = PROVIDED.lock().unwrap_or_else(|e| e.into_inner());
    if !provided.iter().any(|(p, _)| p == id) {
        provided.push((id.to_owned(), Vec::leak(bytes)));
    }
}

/// Scenario `s`'s place for `seed` as the build baked it, if it is at hand.
fn baked(seed: Seed, s: &Scenario) -> Option<&'static [u8]> {
    if seed != builtin::BAKED_SEED {
        return None;
    }
    let provided = PROVIDED.lock().unwrap_or_else(|e| e.into_inner());
    BAKED
        .iter()
        .copied()
        .chain(provided.iter().map(|(id, b)| (id.as_str(), *b)))
        .find(|(id, _)| *id == s.id)
        .map(|(_, bytes)| bytes)
}

/// The palette named `id`.
pub fn palette(id: &str) -> Option<Palette> {
    Palette::named(id)
}

/// The palette scenario `s` paints its place with.
pub fn palette_of(s: &Scenario) -> Palette {
    palette(&s.palette).unwrap_or_else(Palette::socal_1985)
}

/// The facade grammar the coast's buildings are planned by, parsed once. It is built in
/// and its tests parse it, so a failure here is a build that should not have shipped.
pub fn grammar() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR.get_or_init(|| {
        builtin::load_grammar(builtin::CALIFORNIA_COAST_80S, builtin::LEXICON)
            .expect("the built-in grammar")
    })
}

/// The facade grammar scenario `s` names, parsed once (the coast's, [`grammar`], for a
/// file that is not built in, which [`Scenario::validate`] refuses).
pub fn grammar_of(s: &Scenario) -> &'static Grammar {
    static GRAMMARS_PARSED: OnceLock<Vec<Grammar>> = OnceLock::new();
    let all = GRAMMARS_PARSED.get_or_init(|| {
        GRAMMARS
            .iter()
            .map(|(_, text)| builtin::load_builtin_grammar(text).expect("the built-in grammars"))
            .collect()
    });
    GRAMMARS
        .iter()
        .position(|(g, _)| *g == s.grammar)
        .map_or_else(grammar, |i| &all[i])
}

/// The place `key` names for `seed`, in the first scenario.
#[cfg(test)]
pub fn build(key: PlaceKey, seed: Seed) -> Result<Place, String> {
    build_in(key, seed, crate::scenario::get(0))
}

/// [`build_in`] and the place's [`ChunkPlan`] for meshes painted with [`palette_of`]`(s)`,
/// made once: a generated procedural place's is the one its detail pass read, a baked
/// one's is made afresh (the same plan: the test `fresh_plans_are_the_planned_ones`).
pub fn load(key: PlaceKey, seed: Seed, s: &Scenario) -> Result<(Place, ChunkPlan), String> {
    if key == PlaceKey::Coast && baked(seed, s).is_none() {
        return generate_planned(seed, s);
    }
    let place = build_in(key, seed, s)?;
    let plan = ChunkPlan::new(&place, &palette_of(s), grammar_of(s));
    Ok((place, plan))
}

/// Scenario `s`'s procedural place for `seed`, generated, with the plan its detail pass
/// read brought up to date for [`palette_of`]`(s)`.
fn generate_planned(seed: Seed, s: &Scenario) -> Result<(Place, ChunkPlan), String> {
    let t = template_of(s)?;
    let pal = palette_of(s);
    Ok(match &s.layout {
        Some(id) => world::procgen::authored::build_place_planned(
            seed,
            &builtin::layout(id)?,
            &t,
            grammar_of(s),
            &pal,
        ),
        None => world::procgen::build_place_planned(seed, &t, grammar_of(s), &pal),
    })
}

/// Scenario `s`'s region template, checked.
fn template_of(s: &Scenario) -> Result<RegionTemplate, String> {
    builtin::template(&s.region)
}

/// The place `key` names for `seed`: the test town, scenario `s`'s procedural place, or the
/// landmark gallery dressed as `s`'s world.
pub fn build_in(key: PlaceKey, seed: Seed, s: &Scenario) -> Result<Place, String> {
    match key {
        PlaceKey::TestTown => {
            // The test town's layout is fixed; the session seed drives its detailing.
            let mut place = Place::test_grid();
            place.header.seed = seed.0;
            Ok(place)
        }
        PlaceKey::Coast => match baked(seed, s) {
            Some(bytes) => place::decode(bytes).map_err(|e| format!("baked place: {e:?}")),
            None => builtin::generate(seed, &s.region, s.layout.as_deref(), grammar_of(s)),
        },
        PlaceKey::Gallery => {
            let t = template_of(s)?;
            let g = grammar_of(s);
            let region = world::procgen::build::region_of(&t);
            let pal = palette_of(s);
            Ok(world::landmarks::gallery(
                seed.0,
                &g.landmarks,
                region,
                g.era,
                &pal,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{
        grammar_text, landmarks, load_builtin_grammar, load_grammar, parse_kit, region_text,
        CALIFORNIA_COAST_80S, LANDMARKS, LEXICON, SOCAL_BASIN_2001,
    };

    /// A plan made afresh from the finished place (a baked load) builds as the plan its
    /// detail pass read (a generated load), in every world, and the place survives its
    /// encoding.
    #[test]
    fn fresh_plans_are_the_planned_ones() {
        for s in crate::scenario::all() {
            let (p, plan) = generate_planned(Seed(1), s).unwrap();
            let (pal, g) = (palette_of(s), grammar_of(s));
            let fresh = ChunkPlan::new(&p, &pal, g);
            assert!(
                plan.far_terrain(&p, &pal) == fresh.far_terrain(&p, &pal),
                "{}",
                s.id
            );
            assert!(
                world::WorldCollider::with_plan(&p, g, &plan)
                    == world::WorldCollider::with_plan(&p, g, &fresh),
                "{}",
                s.id
            );
            assert_eq!(place::decode(&place::encode(&p)).unwrap(), p, "{}", s.id);
        }
    }

    #[test]
    fn the_built_in_grammar_loads_and_round_trips() {
        let g = load_grammar(CALIFORNIA_COAST_80S, LEXICON).unwrap();
        assert_eq!(g.template, "california_coast");
        assert_eq!(g.era, place::Era::Eighties);
        assert_eq!(&g, grammar());
        let text = ron::ser::to_string(&g).unwrap();
        assert_eq!(load_grammar(&text, LEXICON).unwrap(), g);
    }

    #[test]
    fn the_basin_grammar_loads_with_its_own_lexicon() {
        let g = load_builtin_grammar(SOCAL_BASIN_2001).unwrap();
        assert_eq!(g.template, "socal_basin");
        assert_eq!(g.era, place::Era::Aughts);
        assert_eq!(g.lexicon.id, "us_basin");
        assert!(g.street.bus_stop_m > 0.0 && !g.signs.fascias.is_empty());
        // The coast's lexicon is not the basin's: it lacks its trades.
        assert!(load_grammar(SOCAL_BASIN_2001, LEXICON).is_err());
        let text = ron::ser::to_string(&g).unwrap();
        assert_eq!(load_builtin_grammar(&text).unwrap(), g);
        assert_eq!(
            load_builtin_grammar(CALIFORNIA_COAST_80S).unwrap(),
            *grammar()
        );
    }

    #[test]
    fn scenarios_find_their_region_grammar_and_palette() {
        for s in crate::scenario::all() {
            assert!(region_text(&s.region).is_some());
            assert!(core::ptr::eq(grammar_of(s), grammar_of(s)), "parsed once");
            // A world still being built, or one from an authored layout that has no grammar of
            // its own yet, may dress in another's grammar.
            if !s.stub && s.layout.is_none() {
                assert_eq!(grammar_of(s).template, s.region);
            }
            assert!(palette(&s.palette).is_some());
        }
        // The first scenario's grammar is the coast's.
        assert_eq!(grammar_of(crate::scenario::get(0)), grammar());
        assert!(region_text("atlantis").is_none() && grammar_text("atlantis.ron").is_none());
    }

    #[test]
    fn the_landmark_kits_load_and_round_trip() {
        let kits = landmarks();
        assert_eq!(kits.len(), LANDMARKS.len());
        for ((id, text), k) in LANDMARKS.iter().zip(kits) {
            assert_eq!(&k.id, id, "a kit's id is its file stem");
            let back = ron::ser::to_string(k).unwrap();
            assert_eq!(&parse_kit(&back).unwrap(), k);
            assert_eq!(&parse_kit(text).unwrap(), k);
        }
        // File-name order, so a kit's index stays put as kits are added.
        assert!(LANDMARKS.windows(2).all(|w| w[0].0 < w[1].0));
        for s in crate::scenario::all() {
            assert_eq!(grammar_of(s).landmarks, kits);
        }
        assert!(parse_kit("Kit(").is_err());
        let mut k = kits[0].clone();
        k.lot = [1.0, 1.0];
        let err = parse_kit(&ron::ser::to_string(&k).unwrap()).unwrap_err();
        assert!(err.contains(kits[0].id.as_str()), "{err}");
    }

    #[test]
    fn the_gallery_builds_in_every_world() {
        for s in crate::scenario::all() {
            let p = build_in(PlaceKey::Gallery, Seed(1), s).unwrap();
            assert_eq!(p.validate(), Ok(()));
            let n: usize = landmarks().iter().map(|k| k.variants.len()).sum();
            let marks = p
                .props
                .items
                .iter()
                .filter(|q| matches!(q.kind, place::PropKind::Landmark { .. }))
                .count();
            assert_eq!(marks, n);
            assert_eq!(p.header.era, grammar_of(s).era);
        }
    }

    #[test]
    fn broken_grammars_are_refused() {
        assert!(load_grammar("Grammar(", LEXICON).is_err());
        assert!(load_grammar(CALIFORNIA_COAST_80S, "Lexicon(").is_err());
        let mut g = grammar().clone();
        g.styles.clear();
        let text = ron::ser::to_string(&g).unwrap();
        let err = load_grammar(&text, LEXICON).unwrap_err();
        assert!(err.contains("StuccoBox"), "{err}");
        let mut lex = grammar().lexicon.clone();
        lex.id = "elsewhere".into();
        let lex = ron::ser::to_string(&lex).unwrap();
        let err = load_grammar(CALIFORNIA_COAST_80S, &lex).unwrap_err();
        assert!(err.contains("us_roadside"), "{err}");
    }

    /// Faults the map audit (`world::audit`) still finds in each world, seed 1. A world's
    /// count may only go down: fix the map, then lower its number here. New worlds start at 0.
    const MAP_FAULTS: [(&str, usize); 4] = [
        ("socal_beach_1984", 41),
        ("bay_loop_2005", 0),
        ("mirage_bowl_2001", 0),
        ("lake_city_1986", 0),
    ];

    /// The map audit over every built-in world: no road you cannot reach, no loop back to
    /// its own junction, no stub, and a dead end only where its zone's streets have them
    /// (cul-de-sacs, switchbacks, the pier road) or the layout says `dead_end`. Prints
    /// each fault and where it is (`cargo test -p app map_audit -- --nocapture`).
    #[test]
    fn every_world_passes_the_map_audit() {
        use world::template::StreetPattern::{Beachfront, Curvy, Switchback};
        let mut wrong = Vec::new();
        for s in crate::scenario::all() {
            let place = build_in(PlaceKey::Coast, Seed(1), s).expect("the world builds");
            let t = crate::builtin::template(&s.region).unwrap();
            let layout = s
                .layout
                .as_deref()
                .map(|id| crate::builtin::layout(id).unwrap());
            // Where the layout says a road ends, and the far end of a runway.
            let runway = layout
                .iter()
                .filter_map(|l| l.airfield.as_ref())
                .flat_map(|a| [a.runway.0, a.runway.1].map(|p| world::math::V2::new(p.0, p.1)));
            let ends: Vec<world::math::V2> = layout
                .iter()
                .flat_map(|l| {
                    l.roads
                        .iter()
                        .filter(|r| r.dead_end)
                        .map(|r| l.road_line(r))
                })
                .filter_map(|line| line?.last().copied())
                .chain(runway)
                .collect();
            let ok = |p: [f32; 2]| {
                let p = world::math::V2::new(p[0], p[1]);
                let rule = t.zones.get(world::zone_at(&place, p));
                rule.is_some_and(|r| matches!(r.streets, Curvy | Switchback | Beachfront))
                    || ends.iter().any(|e| (*e - p).length() < 60.0)
            };
            let found = world::audit::roads(&place, ok);
            for f in &found {
                eprintln!(
                    "{}: {:?} at ({:.0}, {:.0}): {:?} road, {:.0} m (?scenario={}&pos={:.0},{:.0})",
                    s.id, f.fault, f.at[0], f.at[1], f.class, f.len_m, s.id, f.at[0], f.at[1]
                );
            }
            let known = MAP_FAULTS.iter().find(|k| k.0 == s.id).map_or(0, |k| k.1);
            eprintln!("{}: {} map faults (pinned {known})", s.id, found.len());
            if found.len() != known {
                wrong.push((s.id.clone(), found.len(), known));
            }
        }
        assert!(
            wrong.is_empty(),
            "map faults (world, found, pinned in MAP_FAULTS): {wrong:?}; more: fix the map, \
             fewer: lower the pin"
        );
    }
    /// The longest dead-straight stretch of a highway or arterial in `place`: its ends, in
    /// the order its edge runs.
    fn longest_straight(place: &Place) -> ([f32; 3], [f32; 3]) {
        use place::RoadClass::{Arterial, Highway};
        let flat = |p: [f32; 3]| world::math::V2::new(p[0], p[2]);
        let mut best = ([0.0; 3], [0.0; 3], 0.0);
        let main = |e: &&place::Edge| matches!(e.class, Highway | Arterial) && !e.oneway;
        for e in place.roads.edges.iter().filter(main) {
            let pr = &e.profile;
            let mut i = 0;
            while i + 1 < pr.len() {
                let dir = (flat(pr[i + 1]) - flat(pr[i])).normalize();
                let mut j = i + 1;
                while j + 1 < pr.len() {
                    let off = flat(pr[j + 1]) - flat(pr[i]);
                    if (off - dir * off.dot(dir)).length() > 0.25 {
                        break;
                    }
                    j += 1;
                }
                let len = (flat(pr[j]) - flat(pr[i])).length();
                if len > best.2 {
                    best = (pr[i], pr[j], len);
                }
                i = j;
            }
        }
        (best.0, best.1)
    }

    /// The ride at speed (`docs/WORLD_GENERATION.md`, "Vertical curves"): down each world's
    /// longest straight at 40 m/s no wheel leaves the road and the car's vertical
    /// acceleration stays calm, and no main road's grade changes faster than its bound.
    /// A road profile with a kink at every sample fails all three.
    #[test]
    fn every_world_rides_smooth_at_speed() {
        use vehicle::{Assists, VehicleInput, VehicleState};
        let spec = crate::vehicles::builtin_spec("gannet_gt").unwrap();
        let mut rough = Vec::new();
        for s in crate::scenario::all() {
            let place = build_in(PlaceKey::Coast, Seed(1), s).expect("the world builds");
            let ground = world::WorldCollider::build(&place, grammar_of(s));
            let (a, b) = longest_straight(&place);
            let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
            let len = (dx * dx + dz * dz).sqrt();
            assert!(len > 100.0, "{}: a straight of {len} m", s.id);
            const SPEED: f32 = 40.0;
            let input = VehicleInput {
                throttle: 0.5,
                brake: 0.0,
                steer: 0.0,
                handbrake: false,
            };
            let mut car = VehicleState::spawn(&spec, [a[0], a[1] + 0.3, a[2]], (-dx).atan2(-dz));
            car.vel = [
                SPEED * dx / len,
                SPEED * (b[1] - a[1]).signum() * 0.0,
                SPEED * dz / len,
            ];
            for w in car.wheels.iter_mut() {
                w.spin = SPEED / 0.32;
            }
            // A second to settle on the springs, then to within 20 m of the far end.
            let steps = ((len - 20.0) / SPEED * 120.0) as usize;
            let (mut lifts, mut sum, mut n) = (0, 0.0f32, 0);
            let mut vy = 0.0;
            for k in 0..steps {
                vehicle::step(&spec, &mut car, &input, &Assists::ARCADE, &ground);
                if k >= 120 {
                    lifts += car.wheels.iter().filter(|w| !w.grounded).count();
                    let acc = (car.vel[1] - vy) * 120.0;
                    sum += acc * acc;
                    n += 1;
                }
                vy = car.vel[1];
            }
            let rms = (sum / n.max(1) as f32).sqrt();
            // The sharpest change of grade on a main road, per meter.
            let mut bend = 0.0f32;
            for e in &place.roads.edges {
                if !matches!(
                    e.class,
                    place::RoadClass::Highway | place::RoadClass::Arterial
                ) {
                    continue;
                }
                for w in e.profile.windows(3) {
                    let run = |p: [f32; 3], q: [f32; 3]| {
                        ((q[0] - p[0]).powi(2) + (q[2] - p[2]).powi(2))
                            .sqrt()
                            .max(0.01)
                    };
                    let (d0, d1) = (run(w[0], w[1]), run(w[1], w[2]));
                    let turn = (w[2][1] - w[1][1]) / d1 - (w[1][1] - w[0][1]) / d0;
                    // Two samples a step apart; a sliver at a junction says nothing.
                    if d0 < 4.0 || d1 < 4.0 {
                        continue;
                    }
                    let x = turn.abs() / (0.5 * (d0 + d1));
                    bend = bend.max(x);
                }
            }
            eprintln!(
                "{}: {len:.0} m straight from ({:.0}, {:.0}): {lifts} wheel lifts, vertical \
                 acceleration {rms:.2} m/s2 rms; sharpest main-road bend {:.2}% per meter",
                s.id,
                a[0],
                a[2],
                bend * 100.0
            );
            // Over a crest at the curve bound the car feels 40² × 0.001 = 1.6 m/s²: a hill
            // road may ride that all the way down (the Bay Loop's longest straight does); a
            // kinked profile rides ten times it.
            if lifts > 0 || rms > 2.0 || bend > 0.008 {
                rough.push((s.id.clone(), lifts, rms, bend));
            }
        }
        assert!(
            rough.is_empty(),
            "rough rides (world, wheel lifts, rms, bend): {rough:?}"
        );
    }
}

#[cfg(test)]
mod bay_hills {
    use super::*;
    use skywave_core::Seed;
    use vehicle::{Assists, VehicleInput, VehicleState};

    /// The `n` steepest `len`-m stretches of the Bay Loop's roads (one per edge), as
    /// (start, end, grade), start downhill.
    fn crests(p: &Place, len: f32, n: usize) -> Vec<([f32; 3], [f32; 3], f32)> {
        let mut out = Vec::new();
        for e in &p.roads.edges {
            let pr = &e.profile;
            let mut d = vec![0.0f32];
            for w in pr.windows(2) {
                let (dx, dz) = (w[1][0] - w[0][0], w[1][2] - w[0][2]);
                d.push(d.last().unwrap() + (dx * dx + dz * dz).sqrt());
            }
            let mut best: Option<([f32; 3], [f32; 3], f32)> = None;
            let mut j = 0;
            for i in 0..pr.len() {
                while j < pr.len() && d[j] - d[i] < len {
                    j += 1;
                }
                if j == pr.len() {
                    break;
                }
                let g = (pr[j][1] - pr[i][1]) / (d[j] - d[i]);
                let (a, b) = if g > 0.0 {
                    (pr[i], pr[j])
                } else {
                    (pr[j], pr[i])
                };
                if best.is_none_or(|x| g.abs() > x.2) {
                    best = Some((a, b, g.abs()));
                }
            }
            out.extend(best);
        }
        out.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        out.truncate(n);
        out
    }

    fn input(throttle: f32, brake: f32) -> VehicleInput {
        VehicleInput {
            throttle,
            brake,
            steer: 0.0,
            handbrake: false,
        }
    }

    /// On `ground`, from `a` facing `b`: rolling uphill at 5 m/s, brake to a stop for 3 s,
    /// then (after `gap` s off both pedals) full throttle for 6 s. Returns (rolled back m,
    /// uphill speed after 2 s, after 6 s, m/s).
    fn restart(
        spec: &vehicle::VehicleSpec,
        ground: &world::WorldCollider,
        a: [f32; 3],
        b: [f32; 3],
        gap: f32,
    ) -> (f32, f32, f32) {
        let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
        let h = (-dx).atan2(-dz);
        let mid = [
            a[0] + 0.2 * dx,
            a[1] + 0.2 * (b[1] - a[1]) + 0.3,
            a[2] + 0.2 * dz,
        ];
        let mut s = VehicleState::spawn(spec, mid, h);
        let dir = {
            let dy = b[1] - a[1];
            let l = (dx * dx + dy * dy + dz * dz).sqrt();
            [dx / l, dy / l, dz / l]
        };
        s.vel = [5.0 * dir[0], 5.0 * dir[1], 5.0 * dir[2]];
        let ar = Assists::ARCADE;
        for _ in 0..360 {
            vehicle::step(spec, &mut s, &input(0.0, 1.0), &ar, ground);
        }
        assert!(
            s.gear > 0 && s.speed_mps.abs() < 0.2,
            "{} held: gear {} {}",
            spec.id,
            s.gear,
            s.speed_mps
        );
        let p0 = s.pos;
        let along = |p: [f32; 3]| {
            (p[0] - p0[0]) * dir[0] + (p[1] - p0[1]) * dir[1] + (p[2] - p0[2]) * dir[2]
        };
        let mut back = 0.0f32;
        let mut v2 = 0.0;
        let n_gap = (gap * 120.0) as usize;
        for k in 0..720 {
            let i = if k < n_gap {
                input(0.0, 0.0)
            } else {
                input(1.0, 0.0)
            };
            vehicle::step(spec, &mut s, &i, &ar, ground);
            back = back.max(-along(s.pos));
            if k == 240 {
                v2 = s.speed_mps;
            }
        }
        (back, v2, s.speed_mps)
    }

    /// Every car the player can pick (the traffic-only semi aside), braked to a stop on
    /// each of the Bay Loop's three steepest 30 m stretches, on the real collider: it stays
    /// in drive, pulls away at once and climbs; after half a second off both pedals it
    /// rolls back well under a car length. Prints the table.
    #[test]
    fn every_car_restarts_on_the_bay_loops_steepest_crests() {
        let sc = crate::scenario::get(crate::scenario::index_of("bay_loop_2005").unwrap());
        let p = build_in(PlaceKey::Coast, Seed(1), sc).unwrap();
        let ground = world::WorldCollider::build(&p, grammar_of(sc));
        let cs = crests(&p, 30.0, 3);
        // Steep still: the vertical curves round a hill's ends, which takes a point off the
        // grade of its steepest 30 m.
        assert!(cs.len() == 3 && cs[2].2 > 0.18, "{cs:?}");
        for (a, _, g) in &cs {
            println!(
                "crest {:.1}% over 30 m at ({:.0}, {:.0})",
                g * 100.0,
                a[0],
                a[2]
            );
        }
        for (id, text) in crate::vehicles::BUILTIN
            .iter()
            .filter(|c| c.0 != "anvil_semi")
        {
            let (id, text) = (*id, *text);
            let spec = crate::vehicles::load_spec(text).unwrap();
            let mut line = format!("{id:22}");
            for (a, b, _) in &cs {
                for gap in [0.0, 0.5] {
                    let (back, v2, v6) = restart(&spec, &ground, *a, *b, gap);
                    line += &format!(" | back {back:4.2} v2s {v2:4.1} v6s {v6:4.1}");
                    let limit = if gap > 0.0 { 1.2 } else { 0.2 };
                    assert!(
                        back < limit && v6 > 4.0,
                        "{id} gap {gap}: back {back} m, {v6} m/s"
                    );
                }
            }
            println!("{line}");
        }
    }
}
