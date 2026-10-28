# Vehicles

Cars are data. A vehicle is a `VehicleSpec` (RON file in `assets/vehicles/`), a `ShapeSpec` for the parametric mesh, and nothing else. The sim, the renderer, the NPC system and the menu all read the same file.

## Rules
- **Inspired by, never copied.** Each car evokes a 1980s archetype (proportions, stance, era details) without reproducing a real model's design. No real brand or model names anywhere, including comments and commit messages. Names are invented, era-flavored (e.g. "Gannet GT", "Homestead Wagon").
- Cars must feel different. Specs encode that difference; the sim must make it legible within 5 seconds of driving.
- Specs are tuned for fun first, plausibility second, simulation fidelity a distant third.

## `VehicleSpec` (RON)
```
VehicleSpec(
  id: "gannet_gt", name: "Gannet GT", era: 1980s, year: 1981,          // the model year its archetype is dated to, 1900–2100
  built: 0,                                                                 // optional: the year an old car was built up with later parts (0: `year`)
  class: Sports,                                                            // Sports|Sedan|Wagon|Coupe|Pickup|Van|Hatch|Convertible|Luxury|Muscle|Utility
  traffic_only: false,                                                      // optional: only NPC traffic drives it (the semi); never on the Vehicle page
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
  shape: "gannet_gt.shape.ron",
)
```

**`year`** (required) dates the car by its archetype; `era` stays the decade. A world shows only the cars of
its era (`app::scenario`, format in `docs/SCENARIOS.md`): a car newer than the scenario's last year
(`years.1`) never appears in it, on the road, parked or on the Vehicle page. The rest are weighted, in
integer thousandths so every platform picks the same: 1 for a car within the scenario's `years`, else
`0.5^((years.0 - year) / 8)` (half every eight years older than the world's first year, so no step at its edge), at least 0.05 for a car the
scenario lists in `heroes` (and never quite 0). The weights are kept per car and per body shape (`Lineup::shapes`, keyed by the spec's `shape`, since the
shape list and the roster are not in the same order). Traffic draws a new car's shape by these weights from its
seeded generator; a parked car multiplies its class list's weights (`propinst::PARKED`) by them. Equal
weights are reduced by their common divisor before a pick, so a world whose cars all fall in its years
picks exactly as before weights existed: the twenty-car 1979–1987 roster in the 1984 coast changes no hash
or golden.

| Car | Year | Car | Year | Car | Year |
|---|---|---|---|---|---|
| Prescott Landau | 1979 | Pipit Hatch | 1981 | Seabreeze Convertible | 1985 |
| Bancroft Sedan | 1980 | Buckboard Pickup | 1982 | Burro 4x4 | 1985 |
| Drayman Van | 1980 | Homestead Wagon | 1983 | Halcyon Turbo | 1986 |
| Gannet GT | 1981 | Meadow Minivan | 1984 | Regent Coupe Cabrio | 1987 |
| Avocet V8 | 1984 | Basilisk V12 | 1985 | Avocet Spider | 1986 |
| Longbow Coupe | 1984 | Appaloosa GT | 1985 | Basilisk Roadster | 1986 |
| Longbow Convertible | 1986 | Appaloosa Convertible | 1987 | | |
| Halberd Coupe | 1997 | Halberd Targa | 1997 | Tanager AWD | 1999 |
| Destrier Coupe | 1969, built 2001 | Siskin Coupe | 2000 | Shrike Coupe | 1995 |
| Courser V8 | 1996 | Falchion Sedan | 1998 | Glaive Turbo | 1999 |
| Sledge Box Truck | 1996 | Plover Sedan | 1997 | Sorrel Utility | 1998 |
| Wren Minivan | 1999 | Percheron Crew Cab | 2000 | Anvil Semi | 2000 |
| Linnet Hatch | 2001 | | | | |

**`built`** (optional) dates an old car carrying later parts: the Destrier Coupe is a 1969 car with a
2001 blower and wheels, so it weighs by its 1969 age in the 2001 world (0.105, above the hero floor) and
never appears in a world before 2001 (the 1984 coast would otherwise show it at 0.42).
`VehicleSpec::first_shown` is the later of the two.

## Physics model (`vehicle::sim`), fixed 120 Hz
One rigid body per car (position, orientation quaternion, linear/angular velocity, inertia tensor from a box of `dims_m` and `mass_kg`). Per step:
1. **Wheels** (4 raycasts from suspension mounts, downward in body space, length = travel + radius): hit → spring + damper force along the ray; store contact normal and speed at contact point.
2. **Tires**: per grounded wheel compute slip angle and slip ratio; longitudinal force from drive torque / brake torque through a simple friction-circle; lateral force = `grip_lat × load × f(slip_angle)` with a peaked curve (linear to `slip_peak_deg`, then falloff to 70%) — a two-parameter curve, not Pacejka. Combined limit via friction circle. Load sensitivity reduces grip per unit load. `oversteer_bias` scales rear lateral grip down slightly for drift-ability.
3. **Drivetrain**: engine RPM from driven wheel speed through gear × final drive (clutch slip only during shifts); torque from curve × throttle; auto gearbox shifts on RPM thresholds with `shift_ms` torque cut; rev limiter. Manual mode later.
4. **Aero** drag and small downforce. Rolling resistance.
5. **Integration**: semi-implicit Euler; sub-steps not needed at 120 Hz.
6. **Collision**: body OBB vs. static world: building footprints extruded (2D polygon vs. OBB on the ground plane with height check), curbs as a height step (wheel ray hits terrain+curb; body may clip over), props as cylinders/boxes with soft push, water as a slow drag region with a reset. Response (`world::collide::resolve`, through `vehicle::sim::knock`): the car is nudged out along the contact normal, then a plain impulse at the contact point stops the approach and leaves a small rebound (10% of the closing speed, at most 0.4 m/s) with light friction (0.3); off center it yaws the car. The car then loses speed across the normal in proportion to the closing speed (40% at 12 m/s or harder), and the side that hit dips (0.03 rad/s per m/s, at most 0.4) for the springs to rock out. The wheels roll on at the new speed, so they do not drive the car back into what it hit. A head-on hit at 10 m/s ends rolling back at 0.4 m/s with the nose dipping 1–2°; a 20° glancing hit at 18 m/s keeps about half its speed and leaves along the wall. Curbs: wheels climb them below 30 mph; faster, the wheel's corner is kicked up. Each step reports an `Impact`: the hardest hit and the fastest curb kick as a `Bump` (closing speed, normal, where on the car in body space) for the sound, and the parked car hit, if any. A parked car stays where it is; `world::collide::Rocking` sways it on its springs (`vehicle::Rock`, 1.4 Hz, settling in about a second) for drawing. No damage, no deformation. Flips are possible; an auto-right occurs after 3 s upside down.
   **NPC cars** (`world::collide::hit_car`, called by the app after `resolve` and before the traffic steps, `docs/NPCS.md`): each NPC car near the player is its shape's body box (nose to tail and its widest point, from the shape the parked-car mesh is built from, centered where that mesh puts it) with its car's mass, its velocity on its path plus any slide, and its yaw rate. The box test is the parked cars'. The player's car is pushed out along the contact normal (2 mm clear), then one impulse exchanges momentum by both masses and inertias with restitution 0.2 and friction 0.3 along the contact, at the player's center of mass's height (on a hill the other car's box stands higher or lower, which would otherwise tip the car). The NPC car's approach is stopped in full; then it leaves at most 4 m/s less the speed it came on at (but at least the 1 m/s rebound), with at most 4 m/s along the contact and 0.6 rad/s of yaw: past that its tires take the rest. So a car standing is shoved and one coming on stops, and the player's car is never left to take the whole exchange. The player's car leaves at the NPC car's new speed less a rebound of at most 1 m/s, so it never drives on into it; then the wall's speed loss and dip, so the body (and every camera on it) jolts as on a wall, and its vertical speed goes in proportion to the ground speed lost (the normal is level, the ground may not be: a climb stopped dead does not carry on upward). Head-on with both cars at 15, 30 or 50 km/h against a car of the player's weight or lighter, both stop and neither is thrown back faster than 0.93 m/s; against a heavier one the player's car takes its share of the momentum and is thrown back (a 1250 kg car by a 1900 kg one at 1.1–1.6 m/s, a 900 kg car up to 4.2 m/s, about 60% of the full exchange); 100 km/h into a standing car is a hard thud (the NPC car shoved at 4 m/s, the player's car stopped against it, its body rising at most 1 cm); 20 m/s uphill into a standing car on 8, 12 and 15% grades rises at most 2.2 cm; a 1.5 m/s side swipe at 10 m/s leaves both near 10 m/s, neither turned more than 0.01 rad. The hit is the step's `Impact::hit` when it is the hardest, so it sounds through the same `Bump` path. No damage, nothing deforms.
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
A `ShapeSpec` (`assets/vehicles/<id>.shape.ron`, Rust type names as RON struct names, `deny_unknown_fields`) is a few curves lofted into a body of a little more than early-2000s-console density (8–14k exterior triangles, 4–8k in the cabin). Shape space: `x` m from the nose growing rearward, `y` m above the ground, widths are half-widths; the loft is symmetric. Not a toy: panel seams, mirrors, rims, lights and trim must read from 30 m.
```
ShapeSpec(
  id: "gannet_gt",                          // matches the VehicleSpec id
  side: [(x, y)...],         // silhouette: top nose→tail (x increasing), then underside back to x = 0; 6–48 points
  plan: [(x, halfwidth)...], // top-down half outline, x from 0 to the length; 2–32 points
  belt: [(x, y)...],         // beltline where the glass starts, held flat beyond its ends
  section: Section(shoulder: 0.93, tumblehome_deg: 24, greenhouse_frac: 0.70,
                   crown: 0.04,   // centerline rise of hood, roof and deck over their edges, m
                   tuck: 0.05),   // rocker tuck under the side, m
  glass: Glass(windshield: (x0, x1), backlight: (x0, x1), side: [(x0, x1)...]),  // gaps between side windows are the B/C pillars
  wheels: Wheels(radius, width, arch_clearance, front_x, rear_x, track),         // rear_x − front_x and track equal the VehicleSpec's
  front: Front(lights: PopUp|Rect|Quad|Round|Hidden, grille: Slot|Egg|Wide|None, bumper: Chrome|Body|Rubber, bumper_y: (lo, hi)),
  rear: Rear(lights: FullWidth|Dual|Triple|Round, spoiler: None|Lip|Wing|TallWing, bumper: Chrome|Body|Rubber, bumper_y: (lo, hi)),
  seams: Seams(hood: bool, trunk: bool, doors: 2|4, hood_width: 0.6, creases: [(x0, y0, x1, y1)...]),  // 6 mm insets; ≤ 2 creases
  parts: Parts(mirrors: Door|Fender, handles: Flush|Pull, wipers: 0–3, exhaust: Single|Dual, antenna: Mast|None,
               rims: Steel|Alloy5|Mesh|Turbine|Dish|Slotted|Twin, hubcaps: bool, whitewalls: bool),  // whitewalls optional (false)
  details: [Louvers, HoodScoop, SideStripe, WoodPanel, WoodApplique, ChromeTrim, VinylRoof, LandauRoof, RallyStripes, MudFlaps, RoofRack,
            BlackTrim, CorneringLamps, PickupBed, SpareTire, TTop, SideStrakes, NacaDucts, EngineLouvers,
            BodyKit, Blower, Targa, Underglow(Blue|Violet|Green), Badge(Diamond|Circle|Bar|Chevron)],
  color_details: [(1, WoodApplique)],  // optional: extra details for one palette entry
  roof: Roof(kind: Hardtop),           // optional (default Hardtop) | Convertible(top: Down|Up)
  axles: [5.30, 6.45],                 // optional: more axles, x m from the nose, fixed wheels (trucks), at most 4
  palette: ["#b9bcc0", "#c4c7cc/#363b43", ...], // era body colors (sRGB), one chosen per instance;
                             // "#upper/#lower" is a two-tone split at the first crease with an accent pinstripe
  accent: "#1c1c1e",         // stripes, vinyl, badge fields
  interior: InteriorShape(dash_height, dash_x, wheel_pos: (x, y), wheel_radius, column_angle_deg, mirror_pos: (x, y),
                          seat: (x, y), driver_offset, rear_seat: bool, dials: 1–3,
                          colors: [dash, seats, carpet, headliner, trim, accent], display: "#3cff7a",
                          // optional: seat insert, piping, dash top ("" = derived), dash band, clutch
                          insert: "#9a2a30", piping: "#b9bbc0", dash_top: "#3c3e44",
                          applique: None|Wood|Brushed, manual: bool),
)
```
**Convertibles** (`roof: Roof(kind: Convertible(top: Down|Up))`): the side silhouette rises over the windshield to the header and drops straight to the deck, `glass.side` is empty and `glass.backlight` is where the top folds (the well behind the rear seat). The loft has nothing above the belt from the header to the boot and no side faces beside the windshield; the windshield gets a swept frame (A-pillars and a header rail, chrome with `ChromeTrim`, else black), the door tops a painted cap with a trim inner face, and the backlight range a deck. `top: Down` puts a padded boot over the well (six segments, rounded ends, pleat seams; `VinylRoof` makes it a conversion shop's taller, squarer boot with a bright foot molding); `top: Up` raises a slightly crowned fabric top from the header to the deck, open over the doors (the door glass is down), solid over the rear quarters, with a plastic rear window and seams. The interior keeps its door cards but no pillars or headliner; LOD1 keeps the boot and the seats (with headrests) as blocks. Louvers, roof racks and vinyl roofs are not drawn on a convertible. `WoodApplique` is a narrow wood-grain band under the belt from the front wheel to the rear wheel; `CorneringLamps` are unlit clear lenses in the front fender sides. Validation (`ShapeError::OpenTop`): no side glass, the boot behind the rear seat and ahead of the deck's end, the header 0.25 m over the belt, the deck at the belt.

**Body-style details** (added with the roster): `PickupBed` opens the top from `glass.backlight.1 + 0.08` (the bulkhead behind the cab's rear window) to 6 cm short of the tail (the tailgate): the loft leaves out everything above the belt ledge there (`Body::bed`, like a convertible's opening) and `bed.rs` lines it with painted rail caps, inner walls, a front bulkhead, the tailgate's inner face, a floor 0.46 m under the rail with raised ribs (LOD0) and box tubs over the rear tires; the trunk seams outline the tailgate on the tail. Validation (`ShapeError::Bed`): a hardtop, a bed of at least a meter, and the side silhouette within 5 cm of the belt along it (the bed rails). `SpareTire` hangs a spare wheel (92% of the road wheels, tread showing) on a bracket off the tail, centered over the rear bumper, in a cover in the accent color with a bright center ring. `TTop` sets two tinted glass panels into the roof behind the windshield header (up to 0.8 m long) either side of a body-colored center bar, in black frames at LOD0; hardtops only. `LandauRoof` puts the vinyl cover (`VinylRoof`'s texture in the accent color) over the rear half of the roof only and wraps it down the blind rear pillars behind the last side window. A van is the ordinary loft with a tall roof, a short nose and side glass only where it has windows; a hatchback is a short deck whose trunk seams become a tailgate outline; pop-up lamps are `front.lights: PopUp`.

**Mid-engined cars** (added with the sports cars) need no new shape: the silhouette puts a short nose, the cabin well forward and a long deck behind it, the deck's `seams.trunk` lid is the engine cover, and a convertible's boot sits between the seats and that deck. Three details dress one: `SideStrakes` runs five body-colored fins (three at LOD2) along each flank from mid-door to a dark intake ahead of the rear wheel, standing out more toward the intake, the intake's top 7 cm under the belt ledge at each station; `NacaDucts` cuts a tapered dark duct into the upper flank ahead of the rear wheel, its top 4 cm under the belt ledge at each station, narrow at the front and opening rearward, with a black lower lip at LOD0; `EngineLouvers` lays nine black slats (five past LOD0) across the deck from 12 cm behind the backlight to 16 cm short of the tail over a dark well that follows the deck's crown. These two details are cut into pieces along and up the flank so they stay on a side that bulges and a belt that climbs (the older side decals, stripes and wood panels, are not). Two rims came with them: `Dish`, a deep polished lip falling to a face sunk inside it with five round holes, and `Slotted`, a nearly flat face with a ring of ten long slots.

**The 2001 hero details** (added with M8 #4). `BodyKit` sweeps an air dam under the nose (from 9 cm
off the ground to the bumper, in the lower paint) with a black splitter lip and two dark intakes, skirts
3 cm proud of the sills between the arches from 3 cm under the sill to 10 cm over it, and a valance under
the tail over a dark diffuser with fins; the dam and valance follow the bumper's path round the corners.
`Spoiler::TallWing` is the wing on uprights 36 cm over the deck (the `Wing` is 17 cm), set further in,
with a deeper blade and bigger end plates. `Rims::Twin` is the aftermarket rim: six pairs of thin spokes
from a small cap to a wide polished lip, the rim at 73% of the tire's radius (a low-profile tire; the
others are 62%). `Blower` stands a supercharger through the hood over the engine: a black plate, a
ribbed polished case, two stacks and a polished butterfly scoop with a dark mouth, its top 37 cm over the
hood. `Targa` (hardtops with side windows; `ShapeError::Targa` otherwise) leaves the roof rails and the
roof out from the windshield header to the end of the first side window, with black seals down the
header's back and the hoop's front and a thin black frame along the top of the door glass; the cabin has
no headliner under it and folds its visors against the header, as a convertible does. `Underglow(color)`
lays four glowing tubes under the car (one under each sill between the arches, one across under each
overhang), lit in their color at intensity 3, into `CarMesh::underglow`, not `lights`: it is its own
lamp group, drawn only while its switch is on, and LOD2 (parked cars, which never light it) has none.

**Fixed axles** (added with the 2001 traffic cars): `axles` lists more axles than the two road axles, each a pair of
wheels at the track drawn into the body (`CarMesh::body`, not `wheels`), as twelve-sided tires with a
flat rim face (eight-sided at LOD2), so a semi's tandems stay within the budget. Validation
(`ShapeError::WheelPlacement`): at most four, inside the body, clear of the road axles and each other
by a tire's diameter, and under a body at least that high off the ground. The road wheels and the
sim's four contacts stay at `front_x` and `rear_x`.

`build` / `build_lod` / `build_with(BuildOptions { color_index, lod, popups_up })` return a `CarMesh` of plain arrays (`MeshData`: positions, normals, uvs in meters for atlas-patterned panels, linear colors, emissive in the renderer's encoding, CCW triangles) in model space (`-z` forward, origin on the ground under the front axle; `body_offset(&VehicleSpec)` moves it to the sim's center-of-mass origin): `body` (shell and every opaque part), `wheels` (hub-local, right-side; left wheels turn 180° about `y`), `glass` (single-sided, facing out), `lights` (every lens with its emissive set as if lit), `interior`, `steering_wheel`, `wheel_centers`, `wheel_radius` and a `Cockpit` (eye, steering pivot and axis, radio display quad, up to 3 dials). Loft: 50–70 stations (30–40 at LOD1), a ring per station from `section`, smooth across rounded panels and hard at creases and seams (12 mm deep, 14 mm wide grooves so they catch light), raised 16 mm feature lines along the hood and trunk lid; arches cut with a lip; lights (headlamp and tail-lamp lenses with reflector facets in chrome surrounds with dividers, side markers, a plate lamp), bumper rub strips (chrome bumpers rubber, rubber bumpers gray), a chrome side molding along the crease with `ChromeTrim`, badges sized to read from the chase camera plus a trunk lock and a blank name plate on the tail, grille, bumpers, rounded mirrors, handles with key locks, wipers with arms, exhaust tips with a muffler, drip rails (`ChromeTrim`/`BlackTrim` hardtops), a fuel door and badges as small meshes; tires with a sidewall bulge and tread grooves, whitewalls when asked for, rims with real spokes, hex lug nuts and a brake drum behind. Two-tone paint is split by the crease's ring slots at LOD0 (its groove becomes the pinstripe) and by height at LOD1 and LOD2 (each ring point's height before the arches lift it, so the lower color runs over the arches); body-colored bumpers and arch lips take the lower color. LOD1 drops seams, small parts and interior detail and turns rims into discs.

The cabin (`interior.rs`): a two-tone dash (lighter padded top, `applique` band across the face with grain lines or a brushed sheen and bright edges, a glovebox), a curved brow over the cluster, dials with chrome bezel rings, a dark inner wall, an odometer window and a strip of warning lamps (two square side gauges beside a single dial), louvered vents with vanes at the dash ends and in the stack, a radio with presets, cassette door and knobs, rotary climate knobs, ashtray and lighter; a console with a bright gate, the shifter's boot and knob (a lit P R N D strip unless `manual`), cup wells and a piped armrest lid; a steering wheel with a padded horn pad, bright spoke inserts, a column shroud with the ignition lock and stalks with knobs; bucket seats with an `insert` panel between rolled bolsters, `piping` along the seams, pleats, head restraints on posts, the recliner knob, rails and belts; a pleated, piped rear bench; door cards with an insert panel, an applique strip, a padded armrest, a pull handle, the release lever, a window crank, a speaker grille, a map pocket and the lock knob; floor mats, sill plates, parcel-shelf speakers, a rounded mirror with its day/night tab, padded visors (a vanity mirror on the passenger's), a dome lamp, a grab handle, and pedals on arms with ribbed pads (a clutch when `manual`). Dial faces are backlit in the `display` color and glow with the instruments at night; the app's needles are white with red tips.

As built (LOD0 exterior / interior triangles; LOD1 and LOD2 exterior):

| Car | Exterior | Interior | LOD1 | LOD2 |
|---|---|---|---|---|
| Gannet | 12,034 | 6,448 | 3,342 | 1,966 |
| Halcyon | 11,952 | 6,508 | 3,350 | 2,086 |
| Pipit | 11,154 | 7,116 | 3,162 | 1,910 |
| Seabreeze | 12,014 | 6,886 | 3,266 | 1,974 |
| Regent | 11,898 | 6,854 | 3,300 | 1,960 |
| Bancroft | 12,058 | 7,092 | 3,574 | 2,210 |
| Prescott | 12,690 | 7,272 | 3,654 | 2,302 |
| Homestead | 13,486 | 7,016 | 3,946 | 2,366 |
| Meadow | 11,566 | 7,140 | 3,438 | 2,026 |
| Buckboard | 11,614 | 6,352 | 3,228 | 1,984 |
| Drayman | 12,220 | 6,472 | 3,336 | 1,872 |
| Burro | 11,674 | 7,068 | 3,422 | 2,210 |
| Basilisk V12 | 12,042 | 6,460 | 3,322 | 2,142 |
| Basilisk Roadster | 11,850 | 6,198 | 3,260 | 1,992 |
| Avocet V8 | 12,332 | 6,476 | 3,474 | 2,118 |
| Avocet Spider | 11,872 | 6,186 | 3,296 | 2,020 |
| Longbow Coupe | 12,784 | 6,524 | 3,434 | 2,258 |
| Longbow Convertible | 12,252 | 6,150 | 3,156 | 2,036 |
| Appaloosa GT | 12,072 | 7,144 | 3,220 | 1,880 |
| Appaloosa Convertible | 11,570 | 6,902 | 3,188 | 1,860 |
| Halberd Coupe | 11,416 | 7,124 | 3,198 | 2,034 |
| Halberd Targa | 11,448 | 6,966 | 3,182 | 2,034 |
| Tanager AWD | 11,662 | 7,116 | 3,180 | 2,064 |
| Destrier Coupe | 13,080 | 7,124 | 3,320 | 2,100 |
| Siskin Coupe | 12,364 | 7,180 | 3,550 | 2,442 |
| Plover Sedan | 11,772 | 7,212 | 3,294 | 2,190 |
| Sorrel Utility | 12,062 | 7,120 | 3,466 | 2,234 |
| Wren Minivan | 12,086 | 7,176 | 3,594 | 2,202 |
| Percheron Crew Cab | 12,028 | 7,308 | 3,412 | 2,084 |
| Linnet Hatch | 11,202 | 7,196 | 3,402 | 2,214 |
| Sledge Box Truck | 11,832 | 6,792 | 3,584 | 2,184 |
| Anvil Semi | 12,930 | 6,460 | 4,052 | 2,416 |
| Shrike Coupe | 11,378 | 6,456 | 3,162 | 2,022 |
| Glaive Turbo | 12,270 | 7,128 | 3,268 | 2,272 |
| Falchion Sedan | 13,036 | 7,132 | 3,594 | 2,358 |
| Courser V8 | 11,620 | 7,100 | 3,386 | 2,150 |

Budgets (`docs/backlog/ART.md`): exterior 8,000–14,000, interior 4,000–8,000, LOD1 exterior 3,000–5,000 and ≤ 40% of LOD0 (27–30%), LOD2 1,500–2,500.

Parked cars (`app::propinst`) are LOD1 of the built-in shapes within 80 m and LOD2 beyond, drawn out to 280 m. The Place gives each a `VehicleClass`; `propinst::PARKED` turns it into a weighted pick among shapes within half a meter of that class's collision box (`world::props::parked_car_size`): sporty cars the Gannet, Halcyon, Pipit, Longbow Coupe or Appaloosa GT; sedans the Bancroft, Prescott, Homestead or Buckboard; wagons the Homestead, Meadow, Drayman or Buckboard; coupes the Seabreeze, Halcyon or Burro; luxury cars the Regent or Bancroft (and a list for each other class but `Utility`, which parks as the wagon). The exotics (Basilisk, Avocet), the sports cars' open versions and the 2001 hero cars never park: they are the player's. In 2001 the lists also hold, at their ends (so the 1984 coast, which has none of them, picks as before), the 2001 traffic cars that fit a stall: the Linnet with the sporty cars, the Plover, Sorrel, Wren and Percheron with the sedans and wagons, the Plover with the coupes and luxury cars; the box truck and the semi are too long for a stall and never park. Each entry's weight is times the world's era weight of the shape and times its weight in the zone the car stands in (`vehicles::ZONE_WEIGHTS`, below). NPC traffic draws from `vehicles::TRAFFIC_SHAPES`: the fourteen everyday 1980s shapes (the first fourteen of `BUILTIN_SHAPES`) and the seven 2001 traffic cars; a world drives the ones its years allow, so the coast drives its fourteen in their old order. Each place parks a shape in at most six paints, two from its palette and four era colors drawn by the era's weights from the place seed, which keeps the parked-car meshes at 24.8 MB on the coast (`docs/PERFORMANCE.md`). Their paint is the car's own palette for a third of them and otherwise the era's street colors, `shape::ERA_1980S_PAINT` (weighted: tan, white, silver and burgundy most, then browns, blues, black, red, the two-tones silver over blue and champagne over brown, greens, creams, gold), passed as `BuildOptions::paint`; each instance gets a slight dust and wear tint.

In the game (`app::car`): LOD0 and LOD1 are built when a car is first driven (at startup, or on the first switch to it from the pause menu's Vehicle page or `VehicleNext`, see `docs/CONTROLS_INPUT.md`), with pop-ups down and up, and kept for the session: switching back reuses its mesh ids, so a session registers at most one set per built-in car. Another paint
(the Vehicle page's paint chooser; the `paint` setting and scene key) registers only what the paint changes,
each level's bodies and inside-out shell (`CarModel::register_paint`), once per car and paint driven or shown
on the page's turntable, which draws from the same ids. Meshes registered after load wait in `App::take_new_meshes`, which the platforms drain into the renderer each frame before drawing. Lamp states are four baked copies of `lights` (headlights off/on × brake off/on), lenses grouped by emissive hue (tail lamps at scale 1 with the headlights on, brake lamps at 3.5); the headlight switch also swaps the pop-up body. The lamps' instance glow (`lighting::car_glow`) dims a scale-1 white lens to 1.5 after exposure at night and raises it to 0.7 by day, so at noon a brake lamp reads at ~2.5, red and clearly brighter than a tail lamp (~0.7, under the bloom threshold); as built, a lens's glow was a tenth of its own sunlit color at noon and brake lamps did not show in the sun. NPC cars, drawn from one parked-car mesh lit by one glow, get their brake lamps from an overlay (`car::brake_overlay_mesh`: the tail lenses alone at 3.5, lifted 6 mm at level 1 and 6 cm at level 2 to win the depth test), one small mesh per shape and level drawn over each braking car. A car with `Underglow` registers its tubes as one more mesh per level, drawn with the lamp glow while `Drive::underglow` is on (the `Underglow` action, U, off at the start), and adds two omni lights of the tubes' color (0.8 kcd, 4.5 m) on the centerline a quarter of the tubes' run in from each end, 35 cm over the tubes inside the body (from the tubes' own height the road beside the car met them edge-on and stayed dark), after the head and tail lamps in `Drive::lights` (`CAR_LIGHTS` is 6). The cockpit shows a green low-beam and a blue high-beam tell-tale on the first dial's face below the hub. Glass maps onto a pane of the window atlas tile and the `Wide`/`Egg` grille onto the egg-crate tile; with no blended pass yet, glass is opaque from outside and culled from inside. The cockpit cameras add the interior, the steering wheel (450° at full lock), the body turned inside out as the cabin walls, and procedural glowing needles and scale ticks on the dials. A convertible's or a targa's interior and steering wheel are drawn from every camera (its cabin is open); its inside-out shell has no roof either, so the cockpit sees the sky.

Export path: `tools/shape-preview` renders a spec to PNG from fixed angles so agents can look at it. glTF import (`gltf` crate, MIT) is the escape hatch for a hand-made model; the same `VehicleSpec` applies.

## Launch roster (1980s), archetypes not models
| id | Archetype evoked | Class | Feel target, as tuned (0–60 mph, top speed, skidpad) |
|---|---|---|---|
| `basilisk_v12` | very low, very wide mid-engined V12 wedge flagship: flank ducts, flared rear haunches, slatted rear window, wide tail-lamp band, tall wing, deep-dish rims | Sports | 1,490 kg, ~333 kW V12, 5-speed, RWD; heavy, serious, planted by its wing; the quickest and fastest car in the game; 4.5 s, 79 m/s, 1.19 g |
| `basilisk_roadster` | the Basilisk with its roof cut away | Convertible | 1,540 kg, same V12, more drag; 4.7 s, 77 m/s, 1.18 g |
| `avocet_v8` | mid-engined V8 two-seater: pop-ups, side strakes into rear intakes, louvred engine lid, four round tail lamps, five-spoke alloys | Sports | 1,300 kg, ~210 kW flat-plane V8, 5-speed, RWD; light, rev-hungry, sharp turn-in, tail-happy on a lift; 5.1 s, 75 m/s, 1.15 g |
| `avocet_spider` | the Avocet with its roof off, top under a boot | Convertible | 1,340 kg, same V8, more drag; 5.3 s, 73 m/s, 1.14 g |
| `gannet_gt` | gull-wing stainless wedge | Sports | 1,250 kg, ~173 kW, 5-speed, RWD; quick, light rear, pop-up lights; 5.6 s, 70 m/s, 1.08 g |
| `longbow_coupe` | front-engined American V8 sports car: long nose, pop-ups, clamshell hood, big glass hatch, slotted rims | Sports | 1,450 kg, ~185 kW V8, 4-speed, RWD; fat torque, huge grip, stiff, mild understeer; 5.9 s, 68 m/s, 1.10 g |
| `longbow_convertible` | the Longbow as a convertible with a short trunk lid | Convertible | 1,510 kg, same V8, softer, more drag; 6.1 s, 67 m/s, 1.09 g |
| `appaloosa_gt` | mid-80s pony-car notchback with a small V8: quad lamps, hood scoop, turbine-look alloys | Coupe (on purpose: as a Sports car its 63 m/s would break the Sports > Coupe top-speed ordering against the Halcyon) | 1,400 kg, ~157 kW V8, 5-speed, RWD; the softest sports car: launches hard, leans, lets the tail out; 6.3 s, 63 m/s, 0.95 g |
| `appaloosa_convertible` | the Appaloosa with a black canvas top, raised | Convertible | 1,480 kg, same V8, floppier, more drag; 6.6 s, 60 m/s, 0.94 g |
| `halcyon_turbo` | front-engined wedge sports coupe, pop-ups, T-top | Coupe | 1,300 kg, ~143 kW turbo four, 5-speed, RWD; balanced, turbo surge past 3,000 rpm; 6.8 s, 65 m/s, 1.01 g |
| `pipit_hatch` | three-door compact hatchback | Hatch | 900 kg, ~85 kW four, 5-speed, FWD; nimble, lift-off tuck; 8.8 s, 49 m/s, 0.93 g |
| `buckboard_pickup` | square-body full-size pickup, long bed | Pickup | 1,700 kg, ~139 kW V8, 3-speed, RWD; high, rear-light, bouncy unloaded; 9.1 s, 47 m/s, 0.67 g |
| `regent_coupe_cabrio` | big luxury coupe cut open by a conversion shop, padded boot | Convertible | 1,650 kg, ~146 kW V8, 4-speed, FWD; floaty, slow steering, lots of roll; 9.9 s, 55 m/s, 0.80 g |
| `bancroft_sedan` | full-size formal four-door | Sedan | 1,750 kg, ~128 kW V8, 4-speed, RWD; quiet, planted, gentle understeer; 10.5 s, 53 m/s, 0.84 g |
| `prescott_landau` | mid-size sedan with a landau vinyl roof and opera windows | Sedan | 1,500 kg, ~110 kW V6, 3-speed, RWD; soft, light steering, nimbler than the Bancroft; 11.4 s, 51 m/s, 0.86 g |
| `burro_4x4` | small two-door four-wheel-drive utility, spare on the tailgate | Utility | 1,350 kg, ~79 kW four, 5-speed, AWD; short, tall, tippy, scrabbles off the line; 12.1 s, 42 m/s, 0.77 g |
| `seabreeze_convertible` | boxy mid-size convertible on a front-drive compact platform, white over tan | Convertible | 1,250 kg, ~97 kW four, 3-speed, FWD; soft, bobbing, comfortable understeer; 12.5 s, 48 m/s, 0.83 g |
| `homestead_wagon` | wood-panel family wagon | Wagon | 1,750 kg, ~110 kW V8, 3-speed, RWD; slow, understeer, lots of body roll; 12.6 s, 49 m/s, 0.76 g |
| `drayman_van` | full-size panel van | Van | 1,900 kg, ~104 kW six, 3-speed, RWD; tall, tippy, the lowest top speed; 13.5 s, 37 m/s, 0.70 g |
| `meadow_minivan` | front-drive minivan, wood-grain edition | Van | 1,450 kg, ~86 kW four, 3-speed, FWD; tall, roomy, wallowy; 15.0 s, 44 m/s, 0.73 g |

The 2001 hero cars (`docs/backlog/MIRAGE2001.md` item 4), player cars of the Mirage Valley and never of an
earlier world; the era's band is 0–60 mph in 4.5–6.5 s, and the 80s exotics still out-sprint them all:

| id | Archetype evoked | Class | Feel target, as tuned (0–60 mph, top speed, skidpad) |
|---|---|---|---|
| `tanager_awd` | late-90s all-wheel-drive twin-turbo V6 coupe: short hood with a scoop, fastback hatch, low wing, four round tail lamps | Sports | 1,700 kg, ~205 kW, 5-speed, AWD; heavy, unflustered, leaves the line without a chirp; 5.5 s, 66 m/s, 1.17 g |
| `halberd_coupe` | mid-90s inline-six twin-turbo grand tourer, 2+2: very long hood, set-back cabin, long fastback glass, small wing | Sports | 1,520 kg, ~180 kW, 6-speed, RWD; one long turbo surge, planted; 5.8 s, 72 m/s, 1.13 g |
| `halberd_targa` | the Halberd with its roof panel lifted out | Convertible | 1,580 kg, same six, softer, more drag; 6.0 s, 71 m/s, 1.11 g |
| `destrier_coupe` | late-60s big-block muscle notchback with a blower through the hood, chrome bumpers, deep-dish rims, rally stripes on some | Muscle | 1,660 kg, ~315 kW blown V8, 4-speed, RWD; lights up its rear tires off the line under any assists, leans, wags its tail; 6.2 s, 59 m/s, 0.89 g |
| `siskin_coupe` | 2000 front-drive compact coupe built up as a tuner: body kit, tall wing, twin-spoke rims, blue underglow | Coupe | 1,180 kg, ~140 kW turbo four, 5-speed, FWD; sharp, high-revving, tucks its nose on a lift; 6.5 s, 62 m/s, 1.06 g |
| `glaive_turbo` | late-90s all-wheel-drive turbo four rally sedan: plain upright four-door, hood scoop, tall wing, mud flaps, mesh rims | Sports | 1,400 kg, ~200 kW turbo four, 5-speed, AWD; short gears that run out at 151 mph, leaves the line clean; 5.4 s, 67.6 m/s, 1.00 g |
| `shrike_coupe` | mid-90s twin-rotor sequential-turbo coupe: low rounded nose with pop-ups, bubble cabin, swollen hips, full-width tail strip, fixed wing | Coupe | 1,290 kg, ~165 kW rotary to 8,500 rpm (the roster's highest), 5-speed, RWD; light, balanced, quick steering; 5.7 s, 64 m/s, 1.04 g |
| `falchion_sedan` | late-90s rear-drive compact sports sedan with an aftermarket body kit, twin round lamps a side, lip on the deck | Sedan | 1,420 kg, ~170 kW inline six, 5-speed, RWD; composed, a little tail on the gas; 6.2 s, 58 m/s, 0.81 g |
| `courser_v8` | mid-90s American front-engined V8 sports car: very long hood, slim lamp strip, steep windshield, T-top, long fastback glass | Muscle | 1,560 kg, ~225 kW pushrod V8, 5-speed, RWD; lights its rear tires under every assist (less than the Destrier), leans, pushes; 6.3 s, 62 m/s, 0.85 g |

The 2001 traffic cars (`docs/backlog/MIRAGE2001.md` item 6), the basin's everyday cars: they drive and park in
Mirage Valley and never in an earlier world. All but the semi are player-drivable and list at the end of the
Vehicle page in 2001; the semi is `traffic_only`.

| id | Archetype evoked | Class | Feel target, as tuned (0–60 mph, top speed, skidpad) |
|---|---|---|---|
| `plover_sedan` | late-90s rounded front-drive mid-size family sedan: low oval-lamped nose, arched roof, short tall deck | Sedan | 1,340 kg, ~110 kW four, 4-speed, FWD; quiet, soft, safe understeer; 9.8 s, 56 m/s, 0.88 g |
| `sorrel_utility` | late-90s mid-size four-door SUV: tall rounded box, roof rails, gray cladding | Utility | 1,820 kg, ~130 kW V6, 4-speed, RWD; tall, leaning, slow steering; 10.4 s, 43 m/s, 0.72 g |
| `wren_minivan` | late-90s one-box front-drive minivan with a sliding side door | Van | 1,760 kg, ~127 kW V6, 4-speed, FWD; roomy, soft, kind understeer; 11.1 s, 46 m/s, 0.75 g |
| `percheron_pickup` | 2000 full-size pickup, four-door crew cab, long bed, chrome grille and bumpers | Pickup | 2,280 kg, ~200 kW V8, 4-speed, RWD; strong, heavy, light rear; 8.4 s, 51 m/s, 0.69 g |
| `linnet_hatch` | 2001 five-door compact hatchback, short and tall-glassed | Hatch | 1,060 kg, ~85 kW four, 5-speed, FWD; light, eager, quick steering; 9.6 s, 54 m/s, 0.91 g |
| `sledge_box_truck` | late-90s medium-duty box truck: short-hood cab and a tall, wider cargo box | Van | 5,600 kg, ~230 kW V8, 4-speed, RWD; slow, tall, patient; 17.2 s, 30 m/s, 0.61 g |
| `anvil_semi` | late-90s conventional tractor with a sleeper and a dry-van trailer, as one body on tandems (`axles`) | Van | 24 t; traffic only, on the freeway; 71 mph flat out, never driven |

Traffic (`traffic::ShapeRule`, from `app::npc::shape_rule`) weighs each shape by the zone of the lane a new car
starts on (freeway lanes count as `Highway`), times its era weight; parked cars use the same table. The 1980s
cars weigh 2 everywhere (by their age alone); the 2001 cars (`vehicles::ZONE_WEIGHTS`, zones downtown, strip,
residential, suburb, industrial, commercial, park, beach, farmland, hills, highway, water):

| Car | Weights | Car | Weights |
|---|---|---|---|
| Plover | 3 3 3 3 2 3 2 2 2 2 3 2 | Percheron | 1 1 2 2 5 1 1 1 4 3 2 1 |
| Sorrel | 2 4 4 4 1 2 2 2 2 3 2 2 | Linnet | 3 3 2 2 1 3 2 2 1 1 2 2 |
| Wren | 1 4 4 4 1 2 2 2 1 2 2 1 | Sledge | 2 1 1 1 5 2 1 1 1 1 1 1 |
| Anvil (semi) | highway 4, else 0 | | |

A shape weighed only on freeway lanes keeps to them at every turn that allows it. Bodies longer than 6 m (the
box truck, the semi) are followed and spaced at their own length and width; every other car keeps the old 4.6 m
(`traffic::cars::CAR_LEN_M`), so the coast's traffic hash does not move. In 2001 the 80s cars still drive at their
age weight, about two cars in five. The basin's parked- and traffic-car meshes are 36.3 MB on the coast's
place (the coast's own 24.8 MB).

Every automatic for now (manual mode comes later). The four sports cars (the Basilisk, the Avocet, the Longbow and the Appaloosa) each have an open version on the same chassis and engine, a little heavier and draggier, so a little slower everywhere. Each car has its own cabin palette (`interior.colors`, insert, piping, applique and dial glow) and its own body palette, which a third of its parked instances wear.

Later eras add rosters the same way (90s: bubble sedans, SUVs; 00s: crossovers, tuners; 10s: hybrid wedges; 20s: EVs with instant torque). The 2001 heroes are the first of them.

## Acceptance tests (`crates/vehicle/tests`)
- Each spec loads and validates (positive mass, monotone gears, torque curve sane).
- Straight-line: 0–60 mph times within a band per car (`tests/roster.rs`: Sports 4–8 s, Wagon 10–16 s, Convertible: Seabreeze 11–14 s, Regent 9–12 s, and a band for each roster car); top speed ordering Sports > Coupe > Sedan > Wagon > Van (every car of a class beats every car of the next).
- Skidpad: steady-state lateral g per class ordered Sports > Hatch > Sedan > Wagon > Van.
- Exotics and pony cars: the Basilisk and the Avocet (and their open versions) beat every other car in 0–60 and top speed; every other sports car (Gannet, Longbow) beats both Appaloosas.
- 2001 traffic cars: each in its band and its class's orderings like the rest (the semi, traffic only, is not driven).
- 2001 heroes: each in its band within 4.5–6.5 s; the Destrier spins its driven tires more than any other car off the line under every assist preset (peak slip ratio over 0.5 in the first 1.5 s) and the Courser over 0.5 too, the Tanager and the Glaive under 0.1 with every assist off; the Shrike has the highest redline. The part 2 heroes fill the gaps the roster left: the Glaive is the only Sports car that fits between the Longbow Convertible and the Longbow in top speed (0.7 m/s either side, so its gearing is set to the hundredth), the Shrike sits under every Sports car as a Coupe, the Falchion under every Coupe and Hatch as a Sedan, the Courser (Muscle, outside the orderings) in the gaps left.
- Distinct: all of them differ pairwise by at least 0.05 s in 0–60 (the Homestead and the Seabreeze, tuned first, are 0.09 s apart; every other pair at least 0.15 s), 0.7 m/s in top speed and 0.01 g on the skidpad; every car stops from 60 mph without assists within 60 m.
- Stability: full-lock at 30 mph does not flip any car; wagon flips at a lower lateral g than the sports car when struck by a scripted impulse.
- Determinism: scripted 10 s input replay produces identical state hashes.
- Mesh: every shape builds, is manifold enough (no NaN normals), exterior 8k–14k triangles, interior 4k–8k, LOD1 exterior 3k–5k, wheels fit arches, LOD1 ≤ 40% of LOD0; two-tone paint splits low/high with a pinstripe; whitewalls only on the outer sidewall; era paints parse, are weighted as listed and change only the paint; a pickup bed is open over its whole length at every LOD with its floor well under the rail; a spare tire hangs behind the tail; a T-top puts glass in the roof. Convertibles: no body or glass triangle over the cabin between the header and the boot, above the belt (top down), a framed windshield and a boot, a raised top that covers the cabin, and much more sky from the seat than a hardtop.
- Visual: `tools/shape-preview` PNGs attached to the PR introducing or changing a car.
