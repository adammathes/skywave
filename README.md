# Skywave

A chill, single-player driving game. Period-correct cars, a coherent seed-generated American place, low-poly geometry lit like it is 2026, and real era-appropriate music on the car radio. Nothing to do but drive and look.

Inspired by the best part of the open-world games of the early polygon eras: cruising at dusk with the radio on. First-person dashboard view, modern controls, zero violence, runs in a browser tab.

*Screenshot coming with the first milestone.*

## Status
Pre-alpha. The first slice is a 1980s California beach town with a sports car, a station wagon and yacht rock. See [`docs/MILESTONES.md`](docs/MILESTONES.md).

## Download
Tagged versions are on the [Releases](https://github.com/adammathes/skywave/releases) page, and every
push to the working branch (and to `main`) refreshes the rolling
[`nightly`](https://github.com/adammathes/skywave/releases/tag/nightly) pre-release with the same files:
- `Skywave-x86_64.AppImage`: Linux and Steam Deck, one file; `chmod +x` it and run it.
- `skywave-linux-x86_64-<version>.tar.gz`: the same Linux build as a folder (see the `README-linux.txt` inside).
- `skywave-macos-arm64-<version>.tar.gz`: an unsigned Apple Silicon app; right-click it and choose Open the first time.
- `skywave-web-<version>.zip`: serve the folder with any static web server.

Nightly names use `nightly` for `<version>`. `SHA256SUMS.txt` lists every file's checksum.
The latest `main` plays in the browser at `https://<owner>.github.io/skywave/` (GitHub Pages; placeholder until Pages is enabled).

## Steam Deck
In Desktop Mode, open Konsole and run:
```
curl -fsSL https://raw.githubusercontent.com/adammathes/skywave/claude/intelligent-hypatia-vnrj1a/tools/install-deck.sh | bash
```
(The URL names the working branch for now; it moves to `main` later.) The script puts the nightly
AppImage in `~/Applications/Skywave/`, adds a menu entry, and prints the steps to add it to Steam
as a non-steam game. Run it again to update; `--tag vX.Y.Z` installs a tagged release instead
(`... | bash -s -- --tag vX.Y.Z`). No sudo, nothing outside your home folder.

To do it by hand, download
[`Skywave-x86_64.AppImage`](https://github.com/adammathes/skywave/releases/download/nightly/Skywave-x86_64.AppImage),
make it executable, then in Steam choose Games → Add a Non-Steam Game to My Library → Browse and
pick it. Back in Gaming Mode it is in your Library.

Skywave is gamepad-first and the Deck's controls work as a standard gamepad, no layout needed.
It renders at the screen's native 1280×800; the pause menu has a Resolution scale setting under
Video (saved now, applied once the renderer supports it). Settings live in
`~/.config/skywave/settings.json`.

## Run
Web (primary):
```
rustup target add wasm32-unknown-unknown
tools/web-build.sh            # builds to dist/ (runs wasm-opt if on PATH: npm install -g binaryen)
python3 -m http.server -d dist 8000   # or any static server; open http://localhost:8000/?seed=1
```
Native (Linux/Mac):
```
cargo run --release -p platform-native     # --version, --seed N, --help
```
Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed and as the nightly on every push to `main` or the working branch (or as a dry run from the Actions tab).

## Your music
The radio plays your own files (point it at folders, one per station) or real internet radio stations. No audio ships with the game. See [`docs/AUDIO_RADIO.md`](docs/AUDIO_RADIO.md).

## Design docs
`docs/VISION.md` · `ARCHITECTURE.md` · `PLACES.md` · `WORLD_GENERATION.md` · `VEHICLES.md` · `RENDERING.md` · `AUDIO_RADIO.md` · `NPCS.md` · `CONTROLS_INPUT.md` · `TESTING.md` · `MILESTONES.md` · `AGENT_WORKFLOW.md` · `adr/`

## Tech
Rust. `wgpu` + `winit` + `glam`, and not much else. WebGPU with a WebGL2 fallback in the browser; Metal/Vulkan native. Deterministic worlds from a seed. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`docs/adr/0001-language.md`](docs/adr/0001-language.md).

## License
BSD-3-Clause. See `LICENSE`.
