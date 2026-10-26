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
7. **Assists** (settings → Drive → Handling): stability (yaw damping toward the intended heading, plus damping of a sideways slide past the tires' peak slip angle; it lets go while the handbrake is held), traction (cap slip ratio), ABS, grip-limited steering (the road-wheel target follows the front axle's travel plus the stick's share of the peak slip angle above 8–11 m/s; it also pivots the car on the handbrake, helping yaw toward `steer × 1.6 rad/s`), and two multipliers, `grip_boost` (tire grip, lateral and longitudinal) and `brake_boost` (service-brake torque). Presets:

| Preset | stability | traction | ABS | steering | grip_boost | brake_boost |
|---|---|---|---|---|---|---|
| `ARCADE` (default) | 0.8 | 0.8 | on | 1 | 1.2 | 1.4 |
| `LIGHT` | 0.3 | 0.5 | on | 1 | 1 | 1 |
| `OFF` ("driver") | 0 | 0 | spec | 0 | 1 | 1 |

Engine braking is a drag at the driven wheels through whatever gear is in, 0.13 g of the car's weight at idle rpm to 0.22 g at the redline: with aero and rolling resistance every car coasts down at ~0.15 g in gear.

Steering input is filtered: target angle = `max_deg × (1 − speed_sens × v/60 m/s)` × input (or the grip-limited target above); the road-wheel angle chases it at `return_rate` per second so a flick does not snap. Controller sticks map linearly with a small deadzone; keyboard uses a 120 ms ramp out and 300 ms back.

Outputs for feel: speed, RPM, gear, wheel slip (audio + tire marks later), g-forces (camera), suspension compression (cockpit shake).

## Parametric mesh (`vehicle::shape`)
A `ShapeSpec` (`assets/vehicles/<id>.shape.ron`, Rust type names as RON struct names, `deny_unknown_fields`) is a few curves lofted into an early-2000s-console-density body. Shape space: `x` m from the nose growing rearward, `y` m above the ground, widths are half-widths; the loft is symmetric. Not a toy: panel seams, mirrors, rims, lights and trim must read from 30 m.
```
ShapeSpec(
  id: "corsair_gtx",                          // matches the VehicleSpec id
  side: [(x, y)...],         // silhouette: top nose→tail (x increasing), then underside back to x = 0; 6–48 points
  plan: [(x, halfwidth)...], // top-down half outline, x from 0 to the length; 2–32 points
  belt: [(x, y)...],         // beltline where the glass starts, held flat beyond its ends
  section: Section(shoulder: 0.93, tumblehome_deg: 24, greenhouse_frac: 0.70,
                   crown: 0.04,   // centerline rise of hood, roof and deck over their edges, m
                   tuck: 0.05),   // rocker tuck under the side, m
  glass: Glass(windshield: (x0, x1), backlight: (x0, x1), side: [(x0, x1)...]),  // gaps between side windows are the B/C pillars
  wheels: Wheels(radius, width, arch_clearance, front_x, rear_x, track),         // rear_x − front_x and track equal the VehicleSpec's
  front: Front(lights: PopUp|Rect|Quad|Round|Hidden, grille: Slot|Egg|Wide|None, bumper: Chrome|Body|Rubber, bumper_y: (lo, hi)),
  rear: Rear(lights: FullWidth|Dual|Triple|Round, spoiler: None|Lip|Wing, bumper: Chrome|Body|Rubber, bumper_y: (lo, hi)),
  seams: Seams(hood: bool, trunk: bool, doors: 2|4, hood_width: 0.6, creases: [(x0, y0, x1, y1)...]),  // 6 mm insets; ≤ 2 creases
  parts: Parts(mirrors: Door|Fender, handles: Flush|Pull, wipers: 0–3, exhaust: Single|Dual, antenna: Mast|None,
               rims: Steel|Alloy5|Mesh|Turbine, hubcaps: bool),
  details: [Louvers, HoodScoop, SideStripe, WoodPanel, ChromeTrim, VinylRoof, RallyStripes, MudFlaps, RoofRack, BlackTrim, Badge(Diamond|Circle|Bar|Chevron)],
  palette: ["#b9bcc0", ...], // era body colors (sRGB), one chosen per instance
  accent: "#1c1c1e",         // stripes, vinyl, badge fields
  interior: InteriorShape(dash_height, dash_x, wheel_pos: (x, y), wheel_radius, column_angle_deg, mirror_pos: (x, y),
                          seat: (x, y), driver_offset, rear_seat: bool, dials: 1–3,
                          colors: [dash, seats, carpet, headliner, trim, accent], display: "#3cff7a"),
)
```
`build` / `build_lod` / `build_with(BuildOptions { color_index, lod, popups_up })` return a `CarMesh` of plain arrays (`MeshData`: positions, normals, uvs in meters for atlas-patterned panels, linear colors, emissive in the renderer's encoding, CCW triangles) in model space (`-z` forward, origin on the ground under the front axle; `body_offset(&VehicleSpec)` moves it to the sim's center-of-mass origin): `body` (shell and every opaque part), `wheels` (hub-local, right-side; left wheels turn 180° about `y`), `glass` (single-sided, facing out), `lights` (every lens with its emissive set as if lit), `interior`, `steering_wheel`, `wheel_centers`, `wheel_radius` and a `Cockpit` (eye, steering pivot and axis, radio display quad, up to 3 dials). Loft: 24–40 stations, a ring per station from `section`, smooth across rounded panels and hard at creases and seams; arches cut with a lip; lights, grille, bumpers, mirrors, handles, wipers, exhaust and badges as small meshes; rims with real spokes. LOD1 drops seams, small parts and interior detail and turns rims into discs.

As built (LOD0): Corsair 5,402 exterior triangles (body+parts 3,466, glass 356, lights 84, wheels 4 × 374) and 1,554 interior (cabin 1,194, steering wheel 360); Palisade 5,856 (4,264, 184, 144, 4 × 316) and 1,726 (1,378, 348). Budgets: exterior 3,000–6,000, interior 1,500–3,000, LOD1 ≤ 40% (≈ 35% and 37% exterior).

In the game (`app::car`): LOD0 and LOD1 are built once at startup, with pop-ups down and up. Lamp states are four baked copies of `lights` (headlights off/on × brake off/on), lenses grouped by emissive hue; the headlight switch also swaps the pop-up body. Glass maps onto a pane of the window atlas tile and the `Wide`/`Egg` grille onto the egg-crate tile; with no blended pass yet, glass is opaque from outside and culled from inside. The cockpit cameras add the interior, the steering wheel (450° at full lock), the body turned inside out as the cabin walls, and procedural glowing needles and scale ticks on the dials.

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
