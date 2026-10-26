# ADR 0003: gilrs for native gamepads

Status: accepted, 2026-10-26

## Context
Native builds (desktop Linux, Steam Deck, Mac) need gamepads with a standard mapping and hot-plug. Writing evdev/IOKit/XInput readers ourselves is a few thousand lines and a controller database we would have to maintain.

## Decision
`gilrs` 0.11 (Apache-2.0/MIT, ~5k lines + ~6k in gilrs-core, bundles the SDL controller database under zlib). It reads `SDL_GAMECONTROLLERCONFIG`, which Steam sets, and knows the Steam Deck. On Linux it needs `libudev` at build and run time (`libudev-dev` in CI; present on SteamOS and desktops). Web uses the browser Gamepad API directly with the same standard-mapping table (`app::input::pad_table`).

## Consequences
One shared index table drives both platforms. A future console port replaces the platform layer only.
