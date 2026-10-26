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

## Per-axis options (mandatory, every analog action)
`invert` (Y invert on look is the canonical case and must work on gamepad and mouse), `sensitivity` (0.25–4×), `deadzone` (0–0.3), `curve` (1.0 linear to 2.5), `swap` presets (triggers↔stick for throttle/brake, left↔right sticks, southpaw). Stored per device kind (keyboard/mouse vs gamepad). A reset-to-default per action and per device.

## Mapping layer (`app::input`)
Platform delivers raw events (key codes, gamepad buttons/axes by standard mapping). `Bindings` (settings JSON) maps raw → action with per-action deadzone/curve/invert; multiple raw inputs can bind one action; a remap UI captures the next input. Keyboard produces smoothed axes via ramps so steering is controllable without a stick. Gamepad: native `gilrs` (Apache/MIT), web Gamepad API (standard mapping) via JS glue; hot-plug supported; rumble optional later.

## Latency rules
Present mode vsync with one frame in flight; no input queue; sim at 120 Hz so a 60 Hz frame contains two steps that both read the frame's input (this is accepted; we do not sub-sample input events in v1). Pointer lock for mouse look on web while the right mouse button is held (the press is the explicit capture gesture); released on button up.

## Gamepad-first UI (Steam Deck, TV)
Every menu, dialog and the remap flow is fully navigable with a stick/d-pad and two buttons (confirm/back), with on-screen button glyphs, focus highlight, and no hover-only states. Mouse and keyboard are additions, not requirements. UI scale setting for TV distance. Steam Deck's controller appears as a standard gamepad; nothing Steam-specific is required.

## Settings UI (bindings UI ships in M1, the rest grows per milestone)
A minimal in-game menu (bitmap font, keyboard/gamepad navigable, mouse optional): Drive (assists, units mph/kph, auto/manual), Controls (remap table, deadzones), Video (resolution scale, shadow quality, bloom, style post, fov), Audio (radio volume, engine volume, stations manager), World (seed code, era, region, time speed, traffic/pedestrian density), About (license, credits). All stored in one settings JSON.

## Accessibility
Steering sensitivity and assist strength are settings. Color-blind-safe HUD palette. Camera shake toggle. No flashing effects above 3 Hz.

## Acceptance tests
- Binding round-trip: settings → bindings → settings identical.
- Invert Y: with the option on, a positive raw look-Y produces a negative `LookY` axis on both gamepad and mouse paths (table test); invert X likewise; sensitivity and curve applied in the documented order (deadzone → curve → sensitivity → invert).
- Menu navigation: scripted gamepad input reaches every settings item and changes a value with no keyboard/mouse events (headless UI model test).
- Scripted raw events produce the expected `InputState` including ramps and deadzones (table-driven test).
- Remap capture resolves conflicts (a key bound to two actions prompts; test the resolution function).
- Manual: keyboard and a standard gamepad in Chrome, Firefox, Safari and native Linux/Mac at the M1 gate.
