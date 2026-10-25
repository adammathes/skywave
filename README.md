# Skywave

A chill, single-player driving game. Period-correct cars, a coherent seed-generated American place, low-poly geometry lit like it is 2026, and real era-appropriate music on the car radio. Nothing to do but drive and look.

Inspired by the best part of the GTA III-era sandboxes: cruising at dusk with the radio on. First-person dashboard view, modern controls, zero violence, runs in a browser tab.

*Screenshot coming with the first milestone.*

## Status
Pre-alpha. The first slice is a 1980s California beach town with a sports car, a station wagon and yacht rock. See [`docs/MILESTONES.md`](docs/MILESTONES.md).

## Run
Web (primary):
```
rustup target add wasm32-unknown-unknown
tools/web-build.sh            # builds to dist/
python3 -m http.server -d dist 8000   # or any static server; open http://localhost:8000/?seed=1
```
Native (Linux/Mac):
```
cargo run --release -p platform-native
```

## Your music
The radio plays your own files (point it at folders, one per station) or real internet radio stations. No audio ships with the game. See [`docs/AUDIO_RADIO.md`](docs/AUDIO_RADIO.md).

## Design docs
`docs/VISION.md` · `ARCHITECTURE.md` · `PLACES.md` · `WORLD_GENERATION.md` · `VEHICLES.md` · `RENDERING.md` · `AUDIO_RADIO.md` · `NPCS.md` · `CONTROLS_INPUT.md` · `TESTING.md` · `MILESTONES.md` · `AGENT_WORKFLOW.md` · `adr/`

## Tech
Rust. `wgpu` + `winit` + `glam`, and not much else. WebGPU with a WebGL2 fallback in the browser; Metal/Vulkan native. Deterministic worlds from a seed. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`docs/adr/0001-language.md`](docs/adr/0001-language.md).

## License
BSD-3-Clause. See `LICENSE`.
