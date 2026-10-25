# Controls and input

Instant, remappable, works with keyboard and gamepad everywhere. Input is sampled once at the start of each frame; the sim sees the freshest state with no queue.

## Actions
| Action | Type | Keyboard default | Gamepad default |
|---|---|---|---|
| Throttle | axis 0..1 | W / Up | Right trigger |
| Brake/Reverse | axis 0..1 | S / Down | Left trigger |
| Steer | axis −1..1 | A/D, Left/Right (150 ms ramp, 300 ms return) | Left stick X (deadzone 0.08, response curve 1.5) |
| Handbrake | button | Space | A / Cross |
| Camera cycle | button | C | Y / Triangle |
| Look (cockpit/chase) | axis pair | Mouse (hold right button) | Right stick |
| Look back | button | Q | Right stick click |
| Radio next / prev | buttons | ] / [ | D-pad right / left |
| Radio off | button | \ | D-pad down |
| Headlights | button | L | D-pad up |
| Horn | button | H | Left stick click |
| Map toggle | button | M | Back / Share |
| Time of day scrub | axis | , / . (hold) | hold View + right stick X |
| Pause/menu | button | Esc | Start / Options |
| Screenshot | button | F12 | — |
| Debug overlay | button | F3 | — |

## Mapping layer (`app::input`)
Platform delivers raw events (key codes, gamepad buttons/axes by standard mapping). `Bindings` (settings JSON) maps raw → action with per-action deadzone/curve/invert; multiple raw inputs can bind one action; a remap UI captures the next input. Keyboard produces smoothed axes via ramps so steering is controllable without a stick. Gamepad: native `gilrs` (Apache/MIT), web Gamepad API (standard mapping) via JS glue; hot-plug supported; rumble optional later.

## Latency rules
Present mode vsync with one frame in flight; no input queue; sim at 120 Hz so a 60 Hz frame contains two steps that both read the frame's input (this is accepted; we do not sub-sample input events in v1). Pointer lock for mouse look on web with explicit click-to-capture.

## Settings UI
A minimal in-game menu (bitmap font, keyboard/gamepad navigable, mouse optional): Drive (assists, units mph/kph, auto/manual), Controls (remap table, deadzones), Video (resolution scale, shadow quality, bloom, style post, fov), Audio (radio volume, engine volume, stations manager), World (seed code, era, region, time speed, traffic/pedestrian density), About (license, credits). All stored in one settings JSON.

## Accessibility
Steering sensitivity and assist strength are settings. Color-blind-safe HUD palette. Camera shake toggle. No flashing effects above 3 Hz.

## Acceptance tests
- Binding round-trip: settings → bindings → settings identical.
- Scripted raw events produce the expected `InputState` including ramps and deadzones (table-driven test).
- Remap capture resolves conflicts (a key bound to two actions prompts; test the resolution function).
- Manual: keyboard and a standard gamepad in Chrome, Firefox, Safari and native Linux/Mac at the M1 gate.
