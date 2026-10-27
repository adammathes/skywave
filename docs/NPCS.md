# NPCs: traffic and pedestrians

The world should feel inhabited and completely safe. NPCs exist to make a place feel alive in the corner of your eye, not to be interacted with.

## Zero violence, by construction
- Pedestrians are **non-collidable** with the player's car. When a car approaches within 6 m at speed, they step back onto the sidewalk or hop aside; if contact would still occur the car passes through and the pedestrian plays a "whoa" step. No ragdolls, no sounds of impact, no reaction beyond a startled gesture.
- Traffic cars are soft bodies in the sim: they yield, brake and avoid; if the player hits one, the response is a gentle bumper-car push with no damage state, no honking escalation, no chase. A tap of the horn makes them pull aside.
- No police, no wanted level, no NPC speech beyond ambient murmur.
- Settings: traffic density (off/low/normal/busy), pedestrian density (off/low/normal/busy).

## Traffic (`traffic::cars`)
- Vehicles spawn and despawn on lanes outside the player's view frustum beyond 150 m, keeping the density target per resident chunk (normal: 6 per chunk in Downtown, 2 in Residential, 0.3 in Farmland).
- Movement: lane following along `Lanes` from the Place (centerline splines), intelligent-driver-model car following (desired speed = lane `speed_kph` × 0.9–1.1 per driver, gap by speed), stop lines at junctions with a simple right-of-way/traffic-light timer per junction (lights only at arterial junctions; stop signs elsewhere), turn choice weighted toward continuing straight. Lane changes only to pass stopped cars.
- Player interaction: NPCs treat the player's car as a car for following/yielding; they never block a lane for more than 10 s (teleport-despawn when hidden if deadlocked).
- Rendering: the same parametric vehicle meshes, era roster weighted by class popularity, random palette color, headlights on at dusk.
- Sim cost: kinematic (no rigid body), 2D along lanes, ≤ 0.05 ms per car per step; 64 cars resident on web.

## Pedestrians (`traffic::peds`)
- Spawn on sidewalks near the player (40–120 m), despawn beyond 150 m or out of view; density by zone (Strip/Downtown high, Suburb low, Farmland none), time of day (busiest evening on the Strip, few at 4 am).
- Movement: walk along sidewalk polylines at 1.2–1.6 m/s, cross at crossings when the junction light allows, idle in groups near POIs (pier, diner). Kinematic, 2D.
- Appearance: a 12-part articulated low-poly figure (< 300 triangles), procedural walk cycle, palette by era (80s: pastels, shorts, headbands; 20s: athleisure, phones). Variation by proportions, hair block, hat/bag props. No faces beyond a skin-tone block.
- Behavior near the car: described above. Also: wave at a honk 30% of the time.

## Parked cars
Part of the Place detail (`Props::ParkedCar`), static instances from the era roster; collidable like props (soft push).

## Acceptance tests
- Traffic reaches density targets within 30 s at a fixed camera on seed 1; no car intersects another at spawn.
- Scripted scenario: player at 60 mph into a 4-way junction; traffic yields; no NPC position ever overlaps the player hull after the response step.
- Pedestrian scenario: scripted car toward a crowd; every pedestrian ends ≥ 1.5 m from the car's path or passed through; no NaNs.
- Cost: 64 cars + 256 pedestrians step in ≤ 1 ms on the CI runner (release).
- Density settings respected; "off" spawns nothing.

## As built: the lane graph (`traffic::LaneGraph`)
Built once from a Place's road graph (`LaneGraph::new(&place)`, a pure function of the place); `Place::lanes` is empty from every producer today, so the graph is derived, not read. Plain arrays: `lanes`, one `points`/`arc` pool for every centerline, `turns` grouped by the lane they leave, `stop_lines` (`place::StopLine`), one `Junction` per node.
- **Lanes** run junction to junction. Edges are chained through two-road nodes where the road carries on unchanged (same lanes each way, class, width, speed, one-way; bend under 60°), so chunk-border splits never end a lane; `Lane::edge`/`forward` name the edge a lane ends on, `start_node`/`end_node` its nodes. Centerlines are the chained profile (samples ≥ 1 m apart) offset right of travel, at the lane centers the paint uses: two-way roads split each half of the curb-to-curb width less the 0.45 m gutter; highways 3.6 m lanes out from a 1.2 m half median; one-way ramps across the whole width, slot 0 left. Polylines, mitered at bends.
- **Setbacks**: each road end at a junction is cut back to where its pavement clears every other road's (`(hw_other + hw·|cos|)/max(sin, 0.5)`, at most 20 m) plus 4.4 m (crosswalk and stop bar room; 1 m at a bend). A road shorter than its two setbacks keeps a fifth of its length as lane.
- **Turns**: from every incoming lane to the outgoing lanes of every other road at the node. Through moves (within 35°) keep their slot, clamped; left turns go slot 0 to slot 0; right turns outermost to outermost; no U-turns (over 150°) at three-road or larger nodes; nothing over 60° at a node touching a highway (no crossing the median). A lane no rule serves takes the straightest move, so only lanes into dead ends (`Control::DeadEnd`, degree-1 nodes) have none. The curve is a cubic from the lane's end to the next lane's start, tangent to both: circular-arc handles for the angle turned, or the quadratic through the corner where the two lane lines meet ahead of both, whichever is rounder. `Turn::length_m` and `min_radius_m` (for turn speed) are measured over 16 chords. Coast seed 1: 2.9% of turns are tighter than 3 m, all on squeezed roads or acute corners.
- **Control**, the rule `world::props` places signals and stop signs by: signals where three or more arterial ends meet, or two on a four-way at an even node; stop signs on every other non-arterial approach at three-road or larger nodes; arterials have priority; highway nodes and bends are free. A controlled lane ends at its stop line (`stop_distance(lane, s)`).
- **Signals** (`traffic::signal`): one fixed two-phase cycle in 120 Hz ticks: 25 s green, 3.5 s yellow, 1.5 s all red per phase, 60 s in all. Approaches roughly parallel to the node's first road are phase 0, across it phase 1. Each junction's start offset is `hash64(place seed, node) % cycle`. The light is a pure function of the tick (`LaneGraph::light(lane, tick)`, `SignalClock::step`), so all signal state is one counter.
- **Queries** (no allocation): `pose(lane, s)`, `end_pose(lane)`, `turn_pose(turn, s)` (position, heading in the vehicle sim's convention, ground direction), `next(lane)` (the turns), `stop_distance`, `light`.
- **Tests** (`crates/traffic/tests/coast.rs`): pinned `LaneGraph::hash` for the coast at seed 1 (moves in its own commit, with the reason); every lane but those into dead ends has a turn leaving from its end node; turn curves meet both lanes in position and direction with no jumps; sharp turns stay under 5%; size under twice seed 1's (1149 lanes, 2214 turns, 363 KiB).
