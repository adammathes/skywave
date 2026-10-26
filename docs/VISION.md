# Skywave — Vision

## One sentence
Drive a period-correct car through a coherent American place that looks like 2002 and is lit like 2026, with real era-appropriate music on the radio, and nothing to do but enjoy it.

## Why
The best part of the open-world games of the early polygon eras was never the missions. It was cruising a city at dusk with the radio on: the lights, the skyline, the song that happened to come on. Twenty-five years later that experience is still rare, and the originals have aged badly (third person only, dated controls, violence everywhere, no first-person dashboard). Skywave is that feeling, extracted, modernized, and made open. The influences are wide: open-world sandboxes, arcade and simulation driving games, adventure games with a strong sense of place, and the look of early 3D across many styles. The project names none of them, and copies none of them.

## Pillars (in priority order; when two conflict, the higher wins)
1. **The drive is the game.** Everything serves the moment-to-moment feel of driving and looking. Instant input response, no hitches, no loading screens, cars that feel distinct.
2. **A place, not a road.** Worlds are coherent: a downtown, a beach strip, suburbs, a highway out to hills and farms. You can learn your way around, share a seed, and come back. Never an endless procedural ribbon.
3. **2002 geometry, 2026 light.** Low-poly, hard silhouettes, flat and vertex colors, tiny palette textures. But HDR, physically plausible sky, real shadows, haze, bloom on neon, wet reflections, a time of day that moves. A visual feast on a polygon budget.
4. **Real music.** The radio plays actual songs from your own library or real internet radio stations, never generated filler. Stations are era-tagged and user-customizable.
5. **Zero violence.** Nothing can be hurt. The world is populated and alive, and completely safe.
6. **Self-contained and open.** BSD-3-Clause. Runs offline with no servers, no inference, no accounts. Ships nothing copyrighted.
7. **Minimal.** Small codebase, few dependencies, boring technology, deliberate features. If a feature does not serve pillar 1, 2 or 3 it waits.

## The first slice
1980s Southern California beach town. A main drag with neon, a beach strip, residential grid, a highway heading inland through hills to farmland. One sports car and one wagon that feel completely different. Yacht rock on the radio at golden hour. If that slice does not feel wonderful, nothing else matters.

## Then
- More eras (90s, 00s, 10s, 20s) as a layer over the same places: building styles, signage, props, car roster, radio presets.
- More regions: other US templates first, then non-US (Shibuya 2005).
- **Place compiler:** an offline tool that takes a real city and an era and compiles a mini drivable version from open map, building and elevation data. The runtime does not care whether a place was generated or compiled.
- Optional looks (cel shading, dither/CRT), destinations with directions, getting out of the car, mobile.

## Non-goals
- Multiplayer. Missions as the point. Combat, police, damage, wanted levels. Photorealism. A general-purpose engine. Live services. Anything requiring an LLM at runtime.

## Who it is for
People who remember the feeling, and people who never got to have it. Works in a browser tab on an integrated GPU; also a native app later.

## Success looks like
You open a URL, pick the wagon, pick a station, and twenty minutes disappear.
