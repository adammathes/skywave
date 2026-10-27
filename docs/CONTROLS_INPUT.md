# Controls and input

Instant, remappable, works with keyboard and gamepad everywhere. Input is sampled once at the start of each frame; the sim sees the freshest state with no queue.

## Actions
| Action | Type | Keyboard default | Gamepad default |
|---|---|---|---|
| Throttle | axis 0..1 | W / Up | Right trigger |
| Brake/Reverse | axis 0..1 | S / Down | Left trigger |
| Steer | axis −1..1 | A/D, Left/Right (120 ms ramp, 300 ms return) | Left stick X (deadzone 0.08, response curve 1.5) |
| Handbrake | button | Space | A / Cross |
| Camera cycle | button | C | Y / Triangle |
| Look (cockpit/chase) | axis pair | Mouse (hold right button) | Right stick |
| Look back | button | Q | Right stick click |
| Radio next / prev | buttons | ] / [ | D-pad right / left |
| Radio off | button | \ | D-pad down |
| Headlights | button | L | D-pad up (tap) |
| High beams (`HighBeams`) | button | K | D-pad up held 1 s |
| Horn | button | H | Left stick click |
| Next vehicle (`VehicleNext`) | button | V | — (pause menu › Vehicle) |
| Reset car (`Reset`) | button | R | Back / Share held 1 s (also pause menu › Reset car) |
| Map toggle | button | M | Back / Share |
| Map zoom (full map) | axis −1..1 | − / = (and the mouse wheel) | L1 / R1 |
| Time of day scrub | axis | , / . (hold) | hold View + right stick X |
| Pause/menu | button | Esc | Start / Options |
| Screenshot | button | F12 | — |
| Debug overlay | button | F3 | — |

While the full map is open the sim keeps running, but the car gets no driving input (it coasts): the menu
directions (arrows, d-pad, left stick) pan the map, Map zoom and the wheel zoom it, and Map toggle, Back or
Pause close it (Pause does not open the menu over it). The wheel is read raw as notches per frame
(`InputState::wheel`), one notch a fixed zoom step whatever the frame time.

`VehicleNext` swaps in the next built-in car of the world (wrapping after the last) where the car is: same spot and
heading, at rest unless it was rolling under 5 m/s, odometer from zero, camera and headlights kept; the new
car's name shows on the HUD for 2 s and it becomes the `vehicle` setting, the car the next session starts in
unless the scene names one (`car=`, with `paint=`). It is unbound on the pad by default: the pad path is the
pause menu's Vehicle page (Resume · Reset car · Vehicle › · Settings › · World ›): Car (left/right through
every car the world has, all twenty on the coast, wrapping;
the car being driven marked), Paint (a swatch per palette entry) and Drive this car, with the car turning on a
turntable beside the panel and its stat strip and specs (class, era, peak power and torque, drive and
gearbox, 0–60 mph and top speed, measured with the sim) under the rows. It opens on the car driven, in its
paint; another car starts in its first paint. Select on any row swaps the car shown in, in the paint shown,
the same way, and closes the menu; the car and paint become the `vehicle` and `paint` settings. At a UI scale
over 1.5 there is no room for the turntable and the specs shorten to what fits.

The pause menu's World page shows one card per world (`docs/SCENARIOS.md`) side by side: its still, its
title, the kind of place and year, and how many cars and radio stations it has; the world the game is in
says "here", a world still being built is greyed and says "coming" but can be picked. Left/right (and
up/down) move between the cards, wrapping, and it opens on the current world. Select closes the menu and
reloads the game in that world, settings kept and the radio off; the world becomes the `scenario` setting,
where the next session starts. Back returns to the pause page.

`Reset` puts the car at rest, upright, on the centerline of the nearest road lane (`world::lanes`: the
place's lane table, or each road's lanes laid out from its cross section, driving on the right), heading
the way that lane drives, on the deck it is on; with no lane within 200 m it goes back to the place's start.
The odometer, headlights and camera are kept, and "Car reset" shows for 2 s. On the pad it is a long press
(`Binding::long`): letting go of Back after 1 s or more held fires it, so a tap still opens the map, and a
hold that scrubbed time (the right stick moved, however late) fires neither. A remap of Reset to another pad
button is a long press too. Every other action on a long-press button acts on release rather than press (the
Bindings page says so under its rows); that is what keeps a tap and a hold apart.
`Headlights` toggles the low beams (pop-up lamps rise and fall with it). `HighBeams` with the headlights
on toggles the high beams (a taller, brighter, longer spot, 40 kcd per lamp to 140 m, against the low beam's
14 kcd to 60 m); with them off it switches them on on high beams. The high beams stay selected while the
headlights are switched off and on again with `Headlights`.
On the pad both share D-pad up: a tap is the headlights, a hold of 1 s or more (`Binding::long`, as for
`Reset`) the high beams, so the headlights act on release there. The cockpit dash shows a green tell-tale
on the first dial while the headlights are on and a blue one beside it with the high beams. Settings ›
Drive › Headlights is Auto (the default) or Manual: Auto switches them on at dusk and off at dawn (when the
NPC drivers' first lamps switch: 25 min after sunset, 25 min before sunrise) and the switch overrides them in
between until the next dusk or dawn; Manual leaves them to the switch. Brake lamps light while the service
brakes are asked for anything: the brake pedal, or the throttle while reversing (reversing on the brake
pedal and coasting leave them dark), and whenever the car stands with the brake held, whatever gear
brake-to-reverse has picked.
The automatic respawn (a car that falls through the world, or whose state goes non-finite, starts again)
is separate and stays.

## Per-axis options (mandatory, every analog action)
`invert` (Y invert on look is the canonical case and must work on gamepad and mouse), `sensitivity` (0.25–4×), `deadzone` (0–0.3), `curve` (1.0 linear to 2.5), `swap` presets (triggers↔stick for throttle/brake, left↔right sticks, southpaw). Stored per device kind (keyboard/mouse vs gamepad). A reset-to-default per action and per device.

## Mapping layer (`app::input`)
Platform delivers raw events (key codes, gamepad buttons/axes by standard mapping). `Bindings` (settings JSON) maps raw → action with per-action deadzone/curve/invert; multiple raw inputs can bind one action; a remap UI captures the next input. Keyboard produces smoothed axes via ramps so steering is controllable without a stick. Gamepad: native `gilrs` (Apache/MIT), web Gamepad API (standard mapping) via JS glue; hot-plug supported; rumble optional later.

## Latency rules
Present mode vsync with one frame in flight; no input queue; sim at 120 Hz so a 60 Hz frame contains two steps that both read the frame's input (this is accepted; we do not sub-sample input events in v1). Pointer lock for mouse look on web while the right mouse button is held (the press is the explicit capture gesture); released on button up.

## Gamepad-first UI (Steam Deck, TV)
Every menu, dialog and the remap flow is fully navigable with a stick/d-pad and two buttons (confirm/back), with on-screen button glyphs, focus highlight, and no hover-only states. Mouse and keyboard are additions, not requirements. UI scale setting for TV distance. Steam Deck's controller appears as a standard gamepad; nothing Steam-specific is required.

Remap flow (`app::ui`): selecting a bindings row waits up to 8 s for the next fresh press (key, mouse button,
pad button, or a stick/trigger pushed past halfway) on that row's device; the device's Back input cancels.
Two-way actions have a row per direction; a key binds one direction, a stick axis binds whole. If another
action in the same context uses the input, a dialog offers "Keep on <that action>" or "Move to <this one>"
(`Bindings::resolve_conflict`). The new input replaces that row's previous inputs on the device. The menu
tree is documented in `crates/app/src/ui/pages.rs`; the sim is paused while a menu is open.

## Settings UI (bindings UI ships in M1, the rest grows per milestone)
A minimal in-game menu (bitmap font, keyboard/gamepad navigable, mouse optional): Drive (handling: Arcade / Light / Off assists; headlights: Auto / Manual; later units mph/kph, auto/manual), Camera (camera, cockpit FOV, seat height and fore-aft, dashboard or hood only, camera shake), Controls (remap table, deadzones), Video (resolution scale, shadow quality, bloom, style post), Audio (radio volume, engine volume, stations manager), World (seed code, era, region, time speed, traffic/pedestrian density), About (license, credits). All stored in one settings JSON.

## Accessibility
Steering sensitivity and assist strength are settings. Color-blind-safe HUD palette. Camera shake toggle. No flashing effects above 3 Hz.

## Acceptance tests
- Binding round-trip: settings → bindings → settings identical.
- Invert Y: with the option on, a positive raw look-Y produces a negative `LookY` axis on both gamepad and mouse paths (table test); invert X likewise; sensitivity and curve applied in the documented order (deadzone → curve → sensitivity → invert).
- Menu navigation: scripted gamepad input reaches every settings item and changes a value with no keyboard/mouse events (headless UI model test).
- Scripted raw events produce the expected `InputState` including ramps and deadzones (table-driven test).
- Remap capture resolves conflicts (a key bound to two actions prompts; test the resolution function).
- Manual: keyboard and a standard gamepad in Chrome, Firefox, Safari and native Linux/Mac at the M1 gate.
