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
| Radio next / prev; deck next / previous track (`RadioNext`, `RadioPrev`) | buttons | ] / [ | D-pad right / left |
| Radio off; deck play/pause (`RadioOff`) | button | \ | D-pad down (tap) |
| Radio / deck (`DeckSource`) | button | ' | Back / Share held + D-pad down (also Audio › Source) |
| Deck next / previous album (`AlbumNext`, `AlbumPrev`) | buttons | P / O | Back / Share held + D-pad right / left |
| Deck mode (`DeckMode`) | button | ; | Back / Share held + D-pad up (also Audio › Deck mode) |
| Headlights | button | L | D-pad up (tap) |
| High beams (`HighBeams`) | button | K | D-pad up held 1 s |
| Underglow (`Underglow`) | button | U | — |
| Horn | button | B (H until the controls card took it) | Left stick click |
| Next vehicle (`VehicleNext`) | button | V | — (pause menu › Vehicle) |
| Reset car (`Reset`) | button | R | Back / Share held 1 s (also pause menu › Reset car) |
| Map toggle | button | M | Back / Share |
| Map zoom (full map) | axis −1..1 | − / = (and the mouse wheel) | L1 / R1 |
| Time of day scrub | axis | , / . (hold) | hold View + right stick X |
| Pause/menu | button | Esc | Start / Options (tap) |
| Screenshot | button | F12 | — |
| Performance overlay (`DebugOverlay`) | button | F3 | — |
| Controls card (`Controls`) | button | H, F1 | Start / Options held 1 s (also pause menu › Show controls) |
| Fullscreen (`Fullscreen`) | button | F | — (pause menu › Fullscreen) |

F3 cycles the performance overlay Off → FPS → Full → Off. FPS is the frame rate alone (one decimal, the
same average the full overlay shows) in the top left corner, which the driving HUD leaves empty; Full is
every counter and the frame graph. The mode is saved (`hud.overlay`: `"off"`, `"fps"` or `"full"`; default
off) and is also on Settings › HUD › Performance overlay. Any menu (pause, settings, vehicle select,
worlds), the full map and the controls card hide the overlay while open and bring it back when they close,
without changing the mode. F3 still cycles the saved mode under a menu, so it can be set from there; it
shows once the menu closes. The window in the background hides it too. While a station toast is up the FPS
number sits below its two lines (a long station name reaches the corner at large UI scales). Holding F3
and pressing C (Camera cycle) toggles the free-fly camera; the F3 press that starts the chord also moves
the overlay one mode.

`Fullscreen` toggles fullscreen, under menus, the map and the controls card too (not while a remap is
waiting for its input). On the web it puts the canvas in fullscreen (`requestFullscreen`, Safari's
`webkitRequestFullscreen` where that is all there is) and takes it out on the next press. Browsers allow
that only from a user gesture, so the page toggles it in the key's own keydown handler, not on the next
frame. The browser's own Esc also leaves fullscreen; that press does not open the pause menu (it is
dropped while the page is fullscreen and for 0.5 s after it leaves). The canvas follows its new size on
the next frame either way, and right-button pointer lock for mouse look works in fullscreen too. The
pause menu's Fullscreen item does the same and leaves the menu open; on the web a browser may refuse it
when no key or click came just before (a pad's Select is not a gesture to browsers), and a pad binding
of `Fullscreen` likewise. On native it switches the window to borderless fullscreen on its monitor and
back.

While the full map is open the sim keeps running, but the car gets no driving input (it coasts): the menu
directions (arrows, d-pad, left stick) pan the map, Map zoom and the wheel zoom it, and Map toggle, Back or
Pause close it (Pause does not open the menu over it). The wheel is read raw as notches per frame
(`InputState::wheel`), one notch a fixed zoom step whatever the frame time.

`VehicleNext` swaps in the next built-in car of the world (wrapping after the last) where the car is: same spot and
heading, at rest unless it was rolling under 5 m/s, odometer from zero, camera and headlights kept; the new
car's name shows on the HUD for 2 s and it becomes the `vehicle` setting, the car the next session starts in
unless the scene names one (`car=`, with `paint=`). It is unbound on the pad by default: the pad path is the
pause menu's Vehicle page (Resume · Reset car · Vehicle › · Settings › · Worlds ›): Car (left/right through
every car the world has, all twenty on the coast, wrapping;
the car being driven marked), Paint (a swatch per palette entry) and Drive this car, with the car turning on a
turntable beside the panel and its stat strip and specs (class, era, peak power and torque, drive and
gearbox, 0–60 mph and top speed, measured with the sim) under the rows. It opens on the car driven, in its
paint; another car starts in its first paint. Select on any row swaps the car shown in, in the paint shown,
the same way, and closes the menu; the car and paint become the `vehicle` and `paint` settings. At a UI scale
over 1.5 there is no room for the turntable and the specs shorten to what fits.

The pause menu's World page (Worlds ›) shows a card per world (`docs/SCENARIOS.md`), three a page side by
side (one a page, its text beside the still, past a UI scale of 1.5; "< 1 / 2 >" under them when they page): its still, its
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
headlights are switched off and on again with `Headlights`. `Underglow` switches a car's underglow on and
off (off at the start; it stays switched across a change of car and lights nothing on a car without one):
its neon tubes glow and two short-reaching lights pool its color on the road round the car. It is a
keyboard switch only by default; the pad can bind it on the Bindings page.
On the pad both share D-pad up: a tap is the headlights, a hold of 1 s or more (`Binding::long`, as for
`Reset`) the high beams, so the headlights act on release there. The cockpit dash shows a green tell-tale
on the first dial while the headlights are on and a blue one beside it with the high beams. Settings ›
Drive › Headlights is Auto (the default) or Manual: Auto switches them on at dusk and off at dawn (when the
NPC drivers' first lamps switch: 25 min after sunset, 25 min before sunrise) and the switch overrides them in
between until the next dusk or dawn; Manual leaves them to the switch. Brake lamps light while the service
brakes are asked for anything: the brake pedal, or the throttle while reversing (reversing on the brake
pedal and coasting leave them dark), and whenever the car stands with the brake held, whatever gear
brake-to-reverse has picked.
The deck (the player's own music, `docs/AUDIO_RADIO.md`) shares the radio's keys: while it is the source,
next/previous are tracks (previous within 3 s of a track's start goes back a track, later it restarts it)
and the power key plays and pauses. The source key (`'`, a free key beside the radio's `[` `]` `\`)
switches between them: the one playing stops and the other starts. On the pad D-pad down stays the power
(play/pause) and right/left the tracks; with Back / Share held (already the map's and the time scrub's
modifier, so it opens no map) the D-pad is the deck's: down the source, right and left the albums, up the
mode. The d-pad itself stays a plain button, so the menus and the controls card read it on press as
before. The pause menu's Audio page has Source, Deck mode and Library (every album as "Artist - Album",
select to play it) too. O and P on the radio put the tape in: the deck becomes the source on that album.
The automatic respawn (a car that falls through the world, or whose state goes non-finite, starts again)
is separate and stays.

## The controls card
`Controls` shows a card of every action (but the free-fly camera's) with its keyboard and its
controller inputs side by side, read from the live bindings, so a remap shows at once. Rows are grouped
Drive (steer, throttle, brake, handbrake, horn, reset car, next car), Camera (cycle, look around, look
back), Lights (headlights, high beams), Radio (on/off, next, previous), Menus (pause, map, map zoom, time
of day, move, select, back) and Help (the card itself, the performance overlay, and one row for screenshot and fullscreen,
"F12 / F", so the card stays one Deck screen). Each input is
named once, " / " between them, "-" for none; a row whose inputs are directions names them by family
(`Arrows`, `D-pad`, `LS`, `RS`, `Mouse`), so Steer reads "A / D / Arrows" and "LS". Pad inputs use the
names the Bindings page uses (A/B/X/Y, LB/RB, LT/RT, LS/RS, D-pad up/down/left/right, Start, Select);
the HUD font is ASCII, so there are no arrow glyphs.

- **At the start:** when a running game starts (never in a still scene, so screenshot goldens and the
  self-test are untouched) the card shows for 6 s with the footer "H or F1: controls" (the pad's input when
  the pad was used last), then goes. Any input dismisses it sooner and still does its job (a trigger
  drives, Esc opens the pause menu); the card's own key only dismisses it. After the player has dismissed
  it in 3 separate sessions it stops showing at the start (`hud.controls_dismissed` in the settings
  counts them). While the start card has more lines than fit (UI scale over 1), up and down scroll it
  and its 6 s start again instead of dismissing it. Settings › HUD › Show controls at start (default on) turns the start card off; switching it
  back on gives it its 3 sessions again. After the start, only the key or the menu item shows it.
- **On the key:** H or F1 on the keyboard; on the pad, Start held 1 s (`Binding::long`), so a tap of Start
  is still the pause menu (which now opens on release) and play stays gamepad-only. Like every long
  press, the card comes up when Start is let go after the second, not while it is still held. The pause
  menu's Show controls item shows the same card over the menu, which stays paused under it.
- **Closing:** its key again, Back (B / Esc) or Pause; up and down scroll it while it has more lines than
  fit, and nothing else closes it (steering, A, Space and the other menu directions do not). The key that
  closes it does nothing else (Esc does not also open the pause menu); M closes it and opens the map. Over
  the pause menu it closes back to the menu.
- It pauses nothing, but like the full map it takes the keys and sticks while it is up: the car coasts,
  and the radio, light, reset and car keys wait until it is gone. While it is up it takes the driving
  HUD's place and the performance overlay (F3) hides.
- **Layout:** two columns, Keyboard and Controller, beside the row names while the safe area is wide
  enough (UI scale 1 and 1.25). Only at scale 1 is it one screen (at any resolution: the HUD is a 16:9
  box scaled to fit); from 1.25 on it scrolls. From 1.5 on (a TV at 2) each row's inputs go on a line
  under its name, keyboard left and controller right.

A bindings file written before `Controls` existed loads with the horn moved to B if it was still on its
old default H alone and no other driving action reads B; otherwise the card keeps only its default inputs no other action has taken (F1).

## Per-axis options (mandatory, every analog action)
`invert` (Y invert on look is the canonical case and must work on gamepad and mouse), `sensitivity` (0.25–4×), `deadzone` (0–0.3), `curve` (1.0 linear to 2.5), `swap` presets (triggers↔stick for throttle/brake, left↔right sticks, southpaw). Stored per device kind (keyboard/mouse vs gamepad). A reset-to-default per action and per device.

## Mapping layer (`app::input`)
Platform delivers raw events (key codes, gamepad buttons/axes by standard mapping). `Bindings` (settings JSON) maps raw → action with per-action deadzone/curve/invert; multiple raw inputs can bind one action; a remap UI captures the next input. Keyboard produces smoothed axes via ramps so steering is controllable without a stick. Gamepad: native `gilrs` (Apache/MIT); on macOS Apple's Game Controller framework first, because the system switches the pads it knows (Xbox, DualShock, DualSense, Switch Pro, MFi) to report modes IOKit HID cannot read, and gilrs only for the rest (`platform-native/src/pad_mac.rs`); web Gamepad API (standard mapping) via JS glue; hot-plug supported; rumble optional later. The native startup line (`--stats`, or `RUST_LOG=info`) names the backend, and each pad is logged as it connects.

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
