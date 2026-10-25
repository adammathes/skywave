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
