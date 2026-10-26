# Vehicles

Cars are data. A vehicle is a `VehicleSpec` (RON file in `assets/vehicles/`), a `ShapeSpec` for the parametric mesh, and nothing else. The sim, the renderer, the NPC system and the menu all read the same file.

## Rules
- **Inspired by, never copied.** Each car evokes a 1980s archetype (proportions, stance, era details) without reproducing a real model's design. No real brand or model names anywhere, including comments and commit messages. Names are invented, era-flavored (e.g. "Corsair GTX", "Palisade Wagon").
- Cars must feel different. Specs encode that difference; the sim must make it legible within 5 seconds of driving.
- Specs are tuned for fun first, plausibility second, simulation fidelity a distant third.

## `VehicleSpec` (RON)
```
VehicleSpec(
  id: "corsair_gtx", name: "Corsair GTX", era: 1980s, class: Sports,       // Sports|Sedan|Wagon|Coupe|Pickup|Van|Hatch|Convertible|Luxury|Muscle
  mass_kg: 1250, dims_m: (4.3, 1.75, 1.15), wheelbase_m: 2.45, track_m: 1.5,
  cg_height_m: 0.45, cg_fore_aft: 0.48,                                     // fraction of wheelbase from front axle
  engine: Engine(idle_rpm: 900, redline_rpm: 7200, torque_nm: [(1000, 180), (3000, 260), (5500, 290), (7000, 230)]),
  gears: [3.1, 2.0, 1.4, 1.05, 0.85], final_drive: 3.7, reverse: 3.0, shift_ms: 250, auto: true,
  drive: Rear,                                                              // Front|Rear|All
  tires: Tires(grip_lat: 1.25, grip_long: 1.15, load_sens: 0.08, slip_peak_deg: 7.0),
  suspension: Susp(spring_n_m: 32000, damper: 3200, travel_m: 0.16, antiroll: 0.3),
  steering: Steer(max_deg: 32, speed_sens: 0.6, return_rate: 8.0),
  brakes: Brakes(max_nm: 3200, bias_front: 0.62, abs: false, handbrake_nm: 1800),
  aero: Aero(cd: 0.34, frontal_m2: 1.9, downforce_coef: 0.02),
  feel: Feel(oversteer_bias: 0.2, body_roll: 0.6, engine_sound: "v8_flatplane", shift_jerk: 0.3),
  interior: Interior(dash: "sport_80s", gauges: [Speedo(0,160,mph), Tach(0,8000)], radio: "digital_green"),
  shape: "corsair_gtx.shape.ron",
)
```

## Physics model (`vehicle::sim`), fixed 120 Hz
One rigid body per car (position, orientation quaternion, linear/angular velocity, inertia tensor from a box of `dims_m` and `mass_kg`). Per step:
1. **Wheels** (4 raycasts from suspension mounts, downward in body space, length = travel + radius): hit → spring + damper force along the ray; store contact normal and speed at contact point.
2. **Tires**: per grounded wheel compute slip angle and slip ratio; longitudinal force from drive torque / brake torque through a simple friction-circle; lateral force = `grip_lat × load × f(slip_angle)` with a peaked curve (linear to `slip_peak_deg`, then falloff to 70%) — a two-parameter curve, not Pacejka. Combined limit via friction circle. Load sensitivity reduces grip per unit load. `oversteer_bias` scales rear lateral grip down slightly for drift-ability.
3. **Drivetrain**: engine RPM from driven wheel speed through gear × final drive (clutch slip only during shifts); torque from curve × throttle; auto gearbox shifts on RPM thresholds with `shift_ms` torque cut; rev limiter. Manual mode later.
4. **Aero** drag and small downforce. Rolling resistance.
5. **Integration**: semi-implicit Euler; sub-steps not needed at 120 Hz.
6. **Collision**: body OBB vs. static world: building footprints extruded (2D polygon vs. OBB on the ground plane with height check), curbs as a height step (wheel ray hits terrain+curb; body may clip over), props as cylinders/boxes with soft push, water as a slow drag region with a reset. Response: impulse with restitution 0.2, friction 0.6, plus a strong damping pulse — bump and stop, never bounce. No damage, no deformation. Flips are possible; an auto-right occurs after 3 s upside down.
7. **Assists** (settings): stability (yaw damping toward intended heading), traction (cap slip ratio), ABS. Default on at "light" for the arcade-sim feel; off gives the "driver" feel.

Steering input is filtered: target angle = `max_deg × (1 − speed_sens × v/60 m/s)` × input; angle moves toward target at a rate so a flick does not snap. Controller sticks map linearly with a small deadzone; keyboard uses a 150 ms ramp.

Outputs for feel: speed, RPM, gear, wheel slip (audio + tire marks later), g-forces (camera), suspension compression (cockpit shake).

## Parametric mesh (`vehicle::shape`)
A `ShapeSpec` is a few curves lofted into an early-2000s-console-density body (target **3,000–6,000 triangles** for the body and wheels, plus **1,500–3,000** for the interior). Not a toy: panel seams, mirrors, rims, lights and trim must read from 30 m.
```
ShapeSpec(
  side: [(x, y)...],         // side silhouette polyline (front→back along the top, then underside), 14–24 points
  plan: [(x, halfwidth)...], // top-down half outline
  section: Section(shoulder: 0.9, tumblehome_deg: 8, greenhouse_frac: 0.55),  // body cross-section shape
  wheels: Wheels(radius: 0.32, width: 0.22, arch_clearance: 0.05, front_x, rear_x),
  front: Face(lights: PopUp|Rect|Quad|Round|Hidden, grille: Slot|Egg|Wide|None, bumper: Chrome|Body|Rubber),
  rear: Face(lights: FullWidth|Dual|Triple|Round, spoiler: None|Lip|Wing),
  seams: Seams(hood: true, trunk: true, doors: 2|4, creases: [(x0,y0,x1,y1)...]),   // inset lines cut into the loft (0.6 cm deep)
  parts: Parts(mirrors: Door|Fender, handles: Flush|Pull, wipers: 2, exhaust: Single|Dual, antenna: Mast|None, rims: Steel|Alloy5|Mesh|Turbine, hubcaps: bool),
  details: [Louvers, HoodScoop, SideStripe, WoodPanel, ChromeTrim, VinylRoof, RallyStripes, MudFlaps, RoofRack, Badge(shape) ...],
  palette: ["#c8102e", "#f2f0e6", ...],   // era-appropriate body colors, one chosen per instance
  interior: InteriorShape(dash_height, wheel_pos, wheel_radius, mirror_pos, column_angle),
)
```
Loft: sample `side` and `plan` at 24–40 x stations, build a 12–16 point ring per station from `section` (smooth-shaded across rounded panels, hard at creases and seams), connect rings; cut wheel arches with a lip; inset seams and creases; add lights, grille slats, bumpers, mirrors, handles, wipers, exhaust and badges as small meshes with their own colors and emissive (headlights, tail lights, dash glow); rims are real spoke geometry (not a texture) with a tire sidewall ring. Tiny pattern textures from the shared atlas for grille mesh and glass. Interior: dash with a hooded cluster and a center stack, instrument faces drawn procedurally at runtime with live needles, steering wheel torus with spokes and a hub rotating with input, column stalks, A/B pillars and headliner edge, door cards with armrests, rear-view and side mirror housings, seats with bolsters, a radio head unit with a display quad showing station text, era-specific details (80s: square vents, digital green/orange displays, velour tones). Flat-colored with a 4–6 color palette per car plus the atlas.

Export path: `tools/shape-preview` renders a spec to PNG from fixed angles so agents can look at it. glTF import (`gltf` crate, MIT) is the escape hatch for a hand-made model; the same `VehicleSpec` applies.

## Launch roster (1980s), archetypes not models
| id | Archetype evoked | Class | Feel target |
|---|---|---|---|
| `corsair_gtx` | gull-wing stainless wedge | Sports | quick but underpowered, light rear, pop-up lights |
| `vendetta_v8` | Italian flat V8 wedge | Sports | fastest, twitchy, loud |
| `kaiser_500` | German executive sedan | Sedan | planted, heavy, confident |
| `monarch_brougham` | big American land yacht | Luxury | floaty, slow steering, soft |
| `palisade_wagon` | wood-panel family wagon | Wagon | slow, understeer, lots of body roll |
| `ridgeback_pickup` | square-body pickup | Pickup | high, rear-light, bouncy unloaded |
| `voyager_van` | panel/conversion van | Van | tall, tippy, slow |
| `sprint_gti` | hot hatch | Hatch | nimble, front-drive lift-off tuck |
| `tsubame_zx` | Japanese sports coupe | Coupe | balanced, turbo surge |
| `sunliner_cabrio` | small convertible | Convertible | light, slow, open-air camera |

Later eras add rosters the same way (90s: bubble sedans, SUVs; 00s: crossovers, tuners; 10s: hybrid wedges; 20s: EVs with instant torque).

## Acceptance tests (`crates/vehicle/tests`)
- Each spec loads and validates (positive mass, monotone gears, torque curve sane).
- Straight-line: 0–60 mph times within a band per class (Sports 5–7 s, Wagon 11–15 s, etc.); top speed ordering Sports > Coupe > Sedan > Wagon > Van.
- Skidpad: steady-state lateral g per class ordered Sports > Hatch > Sedan > Wagon > Van.
- Stability: full-lock at 30 mph does not flip any car; wagon flips at a lower lateral g than the sports car when struck by a scripted impulse.
- Determinism: scripted 10 s input replay produces identical state hashes.
- Mesh: every shape builds, is manifold enough (no NaN normals), body 3k–6k triangles, interior 1.5k–3k, wheels fit arches, LOD1 ≤ 40% of LOD0.
- Visual: `tools/shape-preview` PNGs attached to the PR introducing or changing a car.
