# Audio and radio

The radio is half the experience. It plays **real music the user owns or real stations that broadcast**, organized into era-tagged stations you flip through from the dashboard. The repository contains no audio and no lyrics; presets are URLs and metadata only.

## Stations
```
Station(
  id: "181fm_yachtrock", name: "181.FM Yacht Rock", era: "1970s",
  format: "yacht rock and smooth soft rock",
  source: Stream(urls: ["https://listen.181fm.com/181-yachtrock_128k.mp3", ...],
                 homepage: "https://www.181.fm/links", codec: Mp3, bitrate_kbps: 128,
                 terms_checked: "2026-10-27", verified: false)
        | Files(folder_hint: "yacht-rock"),
)
// later: Youtube(playlist: "...") (web only), shuffle_seed for files ("already playing")
```
A station's name is the real service's own channel name (a fact); the dash shows it as it is. There is no DJ or ident audio: station idents are visual only. A `RadioPreset` file (`assets/radio/<era>_<scenario>.ron`) lists the stations of one era and scenario (`crates/audio/src/station.rs` has the format and its validation). The user's settings switch stations off, add streams by URL and pick a preset per scenario. The dash shows the station name and the toast its format; the current track title (ICY metadata from streams, tags from files) comes later.

### Sources
1. **Files** (native + web). Native: user picks folders; `symphonia` (MPL-2.0 — allowed, note in deny.toml) decodes MP3/OGG/FLAC/AAC/WAV; tags via `symphonia` metadata. Web: file picker or drag-and-drop → `File` handles decoded by Web Audio (`decodeAudioData`) in JS glue; the WASM side only sees track lists and playback commands. Folder structure maps to stations (one folder per station) or a single library with station filters by tag/year. Playback order is a seeded shuffle so returning to a station resumes mid-"broadcast" (position = (wall clock of the place − station offset) mod playlist length): stations feel like they were already playing while you were on another one.
2. **Streams** (native + web). Internet radio over HTTPS (Icecast/SHOUTcast MP3/AAC). Native: HTTP client (`ureq`, MIT/Apache) feeding `symphonia`; ICY metadata parsed for titles. Web: an `<audio>` element driven from JS glue (streams work without CORS for playback; titles need CORS-enabled stations, otherwise none). Presets ship with a handful of well-known public stations per era/format whose terms allow listening (e.g. listener-supported stations with public stream URLs); each preset records the homepage and the date checked. Nothing is rehosted.
3. **YouTube** (later, web only). Official IFrame Player API with user-supplied playlist IDs; the player element must remain visible (ToS), so it is rendered as the "radio display" in a corner of the page. No audio-only tricks.

### Switching
Next/previous station, radio off, volume. Switching plays a 0.5 s dial sweep (static, a whistle, a squelch click) then the new station. Station list and current track on the dash display and as a small HUD toast. Stations persist across sessions; the last station is restored.

### As built (M3, web first)
- **`crates/audio`** (std, platform-neutral). `Radio` is the switching state machine: `Off`, `Tuning { from, to, t }` (`TUNE_SECONDS`, 0.5 s, of the dial sweep), `Playing { station }`, `Error { station, reason }`. Next/previous from off resume the last station; `\` is the power button. It emits `RadioCommand`s (`Play { url, token }`, `Stop`, `Volume(f32)`), counts its tunes (`Radio::tunes`, which the app copies into `AudioParams::tunes` so the mixer plays the sweep; the automatic move after "no signal" is a tune too) and takes `PlayerEvent`s (`Started`, `Failed`, `Blocked`, each with the token of its `Play`; stale ones are ignored). A failed URL moves to the station's next URL; when none is left the station is in `Error` ("NO SIGNAL" on the dash and in the toast) for `NO_SIGNAL_SECONDS` (2 s), then the radio tunes the next station up the dial by itself. A station that failed is remembered for the session (by id) and skipped by next, previous, power-on and the automatic move, until it plays again; when every station has failed, "NO SIGNAL" stays and the dial turns plainly, so the user can retry. `RadioSettings` (volume, power, last station, disabled stations, added streams, preset per scenario) is the `radio` section of the settings JSON. Native will consume the same commands.
- **`app::radio`**. The built-in presets, the keys (`]` `[` `\`, d-pad right/left/down), the toast ("1/7  181.FM Yacht Rock" over its format, 3 s; "Press any key for radio" while the browser holds playback), and the dash: in the cockpit view the radio display quad's corners are projected to the screen and the station name is drawn there as HUD text in the display's glow color, scrolling a character at a time when it does not fit. The radio starts with the first frame of a running game, never in a still scene (screenshot goldens stay silent). Settings → Audio (one page with the sound effects' volumes): master, engine, world and radio volume, mute, preset (per scenario), Stations (on/off per station and added stream), "Add stream URL" (web: a text prompt; `https://` only), "Remove added streams". The master volume scales the radio too and mute silences it; while a station plays, its volume setting is `AudioParams::radio`, which ducks the engine and world buses.
- **Web** (`platform-web` + the radio block of `web/index.html`). One hidden `<audio preload="none">` element: `Play` sets its `src` and calls `play()`; `error` events, rejected plays and a stream that neither plays nor errors within 10 s come back as `failed`, a `NotAllowedError` (no user gesture yet) as `blocked`, retried on the next key, click or touch (the key that dismisses the help card counts). The dial sweep is not played here: it comes from the mixer through the sfx worklet, like the engine. Media Session metadata carries the station name. `window.skywaveRadio` is the debug handle (`.log`, `.state`, `.audio`). Nothing is fetched until the user tunes a station.

## Presets
Real stations whose broadcasters publish their stream URLs for listening in other players; nothing is rehosted. `verified` stays `false` until the owner has listened from the game.

**The referer rule.** The game is a web page (`https://adammathes.github.io/skywave/`), so every stream request carries that page as its `Referer` and `Origin`, and must be HTTPS (an `http://` stream is blocked as mixed content). A station whose servers refuse requests from other sites, or whose terms do not allow its streams inside third-party players or sites, does not go in a preset, and we never work around a refusal (no `referrerpolicy`, no proxy). SomaFM is out for this reason: its servers answer 403 to a request with a foreign `Referer`, and it does not allow its streams inside third-party sites.

**The checker.** `node tools/check-streams.mjs` (Node 22; behind a proxy add `NODE_USE_ENV_PROXY=1`) asks for every preset URL the way a browser on the page does: HTTPS only, a browser User-Agent, the page's `Referer` and `Origin`, redirects followed. A URL passes when it answers 200 with an `audio/*` content type and sends at least 5 seconds of audio (`bitrate_kbps` × 5 s of bytes) within 15 s. It prints one line per URL (status, content type, bytes, time to the first byte) and exits 1 if any URL fails. All 22 URLs passed on 2026-10-27.

**Adding a station.**
1. Find the station's own listen or stream-links page (or its `.pls`/`.m3u` playlist) and read its terms: the stream URL must be published for listening, and nothing may forbid other players. That page goes in `homepage`, today's date in `terms_checked`.
2. Take the `https://` URL (mirrors in order in `urls`), and its real codec and bitrate (the `icy-br` header, or measured).
3. The id is lowercase letters, digits and `_`; the name is the station's own name (at most 48 printable ASCII characters).
4. Run the checker; every URL must pass. Keep a preset at 5 to 7 stations (a test checks).

| Preset | Station | Format | Stream | Published at |
|---|---|---|---|---|
| `1984_socal` | 181.FM Yacht Rock | yacht rock and smooth soft rock | `https://listen.181fm.com/181-yachtrock_128k.mp3` (MP3 128) | 181.FM stream links |
| | 181.FM Mellow Gold | 70s and 80s soft rock | `.../181-mellow_128k.mp3` | |
| | 181.FM Lite 80's | 80s soft hits | `.../181-lite80s_128k.mp3` | |
| | 181.FM Awesome 80's | 80s pop and rock hits | `.../181-awesome80s_128k.mp3` | |
| | Radio Paradise Mellow Mix | mellow rock and soft pop | `https://stream.radioparadise.com/mellow-128` (AAC 128) | Radio Paradise stream links |
| | Radio Paradise Main Mix | eclectic album rock | `.../mp3-128` (MP3 128) | |
| | Radio Paradise Global Mix | world music and eclectic grooves | `.../global-128` (AAC 128) | |
| `1977_chicago` | 181.FM Soul | classic soul and R&B | `.../181-soul_128k.mp3` | 181.FM stream links |
| | 181.FM Super 70s | 70s pop and rock hits | `.../181-70s_128k.mp3` | |
| | WDCB 90.9 FM | Chicago jazz and blues | `https://wdcb-ice.streamguys1.com/wdcb128` (MP3 128) | WDCB streaming support page |
| | 181.FM True Blues | electric and classic blues | `.../181-blues_128k.mp3` | |
| | WHPK 88.5 FM | Hyde Park college radio | `https://whpk-stream.uchicago.edu/stream` (MP3 256) | whpk.org listen link |
| | WFMU Rock'n'Soul Radio | soul, R&B and rock 'n' roll 45s | `https://stream{2,0}.wfmu.org/rocknsoul` (MP3 128) | WFMU's `wfmu_rock.pls` |
| | Radio Paradise Rock Mix | classic and album rock | `.../rock-128` (AAC 128) | |
| `1986_north_shore` | 181.FM Awesome 80's | 80s pop and rock hits | as above | |
| | 181.FM Classic Buzz | classic alternative and new wave | `.../181-classicbuzz_128k.mp3` | |
| | WNUR 89.3 FM | Evanston student freeform, jazz and rock | `https://stream.rcs.revma.com/w4pmmfkdx4zuv` (MP3 ~192) | wnur.org's own player |
| | 181.FM Lite 80's | 80s soft hits | as above | |
| | 181.FM The Eagle | classic rock | `.../181-eagle_128k.mp3` | |
| | Radio Paradise Rock Mix | classic and album rock | as above | |
| | Radio Paradise Main Mix | eclectic album rock | as above | |

Left out: SomaFM (the referer rule); KCRW Eclectic24, WLUW and Vocalo (no stream URL published on their own pages that we could find; their players only); KEXP, WWOZ, WXPN and The Current answer the checker but fit neither era nor place. Gaps: no openly streamed disco station with a stable published URL; users add their own by URL (and, next, from their own files).

## Engine and world sound
No recorded audio in v1; everything is synthesized at 48 kHz by the `audio` crate (std; the synth and mixer use no dependencies, only the radio's presets need `ron` and `serde`). The app fills one plain `audio::synth::AudioParams` per frame (`app::sound`, read with `App::audio_params`): engine (profile, rpm, idle and redline, throttle pedal, gear, shift time), forward speed, per wheel (combined slip relative to the tire's peak, load relative to rest, tire speed, grounded, surface under the contact from the collider), curb-kick and wall-hit counters with the last kick's and hit's speed and side of the car (from `world::Bump`) (counted per sim step from `world::Impact`, rising edges only, so no event is lost or doubled between frames), the horn, ocean and bird amounts, the nearest passing NPC cars (`TrafficVoice`: level, pan, heard frequency), the volumes, mute and paused. All synth math is `+ - * /`, `floor` and `sqrt` with a polynomial sine and `exp2` (`audio::synth::dsp`), so a scripted drive renders bit-identically everywhere; its WAV is pinned by hash in `mixer::tests`.

### Engine (`audio::synth::engine`)
One phase at the engine-cycle rate (`rpm / 120`); every partial is a whole multiple of it, so pitch changes never click. Per sample: harmonics 1..N of the firing frequency `rpm × cylinders / 120` with profile amplitudes, a sub-oscillator at half of it, each firing scaled by its cylinder's gain in a per-profile pattern (unevenness gives a cross-plane V8 its burble) times a random variation (largest at idle), exhaust and intake noise (band-passed white noise; exhaust rides the firing pulse, intake grows with throttle squared), all through a low-pass that opens with rpm and throttle. rpm slews with a 20 ms time constant, throttle 30 ms, so 60 Hz parameter steps never zipper. Events: lifting off above half the rev range queues 2–5 exhaust pops (band-passed noise cracks and a 70 Hz kick, 40–160 ms apart) and a closed throttle up high crackles now and then; at 98.5% of the redline under throttle the limiter cuts the sound to 25% at the profile's rate; a change between forward gears dips the level to 50% and closes the filter for the spec's `shift_ms` (the gearbox's torque cut). Idle hunts ±1.2% at 0.8 Hz.

| `feel.engine_sound` | Cylinders | Harmonics | Character |
|---|---|---|---|
| `v8_flatplane` | 8 | 10, slow roll-off | even firing, bright rasp, low-pass 0.7 → 6.7 kHz, crackles; limiter 16 Hz |
| `v8_lazy` | 8 | 7, fast roll-off | uneven firing pattern, strong sub, dark (low-pass 0.4 → 3.4 kHz), fewer pops; limiter 12 Hz |
| `i4` | 4 | 10, flat | buzzy, thin bottom, most intake roar; limiter 18 Hz |
| `v6`, anything else (`default`) | 6 | 8 | smooth, mid-weighted; limiter 15 Hz |

Measured with `tools/audio-preview` (release): idle at 900 rpm puts the strongest component at the firing frequency (60 Hz for the V8s, 30 Hz i4, 45 Hz six) 30–57 dB over its surroundings, with harmonics 2–4 present; a full-throttle sweep keeps the peak within the 20 ms slew of `rpm × 8 / 120` from 110 to 430 Hz; no DC; full-mix peaks stay under -1.9 dBFS. The engine costs ~0.04 ms per 256-frame block and the whole mixer ~0.03–0.05 ms (budget 0.3 ms).

### World (`audio::synth::world`)
- Tire squeal: white noise through two resonant band-passes (~780–1100 Hz wandering, and 2.15× that), level `smoothstep(0.9, 1.7, slip)` × tire speed (full at 4 m/s, so a burnout squeals) × load; asphalt full, concrete 85%, grass none. Left wheels lean left, right wheels right.
- Rolling: leaky-integrated (brown) noise low-passed at `90 + 10 × speed` Hz, by the share of grounded wheels on each surface: concrete brighter with a 95 Hz knock every 4.6 m slab joint; grass adds a band-passed swish.
- Wind: band-passed noise per ear (decorrelated), level ∝ speed² up to 45 m/s, with two slow gust LFOs.
- Curb kick: a 50–90 Hz falling thump and a short click, level from the wheel's speed (full at 24 m/s). Wall hit: a 60–90 Hz thud and a soft 45 ms crunch, level from the approach speed. Each leans toward the side of the car it was on (±35% per ear); nothing harsh.
- Horn: two tones a minor third apart (410 and 500 Hz, five harmonics each) brightened by a 2 kHz band-pass, 6 ms attack, 40 ms release; on the ui bus.
- Ambience: an ocean wash (low-passed noise per ear swelling on 12 s and 20 s cycles) by the share of beach and water zones round the car (the car's point twice, eight points 60 m out); bird calls (2–5 swept chirps at 2.6–4.2 kHz, randomly panned, every few seconds) by the share of residential, suburb and park zones, faded in with the sun between -2° and 8°. Both stay well under the road and engine.
- Traffic: the four nearest moving NPC cars within 120 m (`app::npc`), each a pulse at its firing frequency (30 Hz + 4.5 Hz per m/s, times the Doppler factor of the closing speed with the player's car) low-passed at four times that with a little rumble, at its level (6 m over the distance, louder with speed) and panned by its bearing from the camera; levels ease over 80 ms so a voice handed to another car does not click.

- Radio dial (`audio::synth::tune`), on each new `AudioParams::tunes` (a press restarts it): band-passed static whose center sweeps 500 Hz → 4 kHz over the burst with a 7 Hz jitter, sparse crackle pops, all low-passed at 4.5 kHz (the receiver's bandwidth); a faint carrier whistle gliding 2.4 kHz → 260 Hz; at `TUNE_SECONDS - 80 ms` the squelch shuts with a 1.4 kHz click, and the last 60 ms are silent until the station starts. Scaled by the radio volume setting (`AudioParams::tune_volume`), on the ui bus. At the default mix (everything at 0.8) its RMS is ~1.6× (+4 dB) the engine idle's (`mixer::tests`), and its WAV is pinned by hash.

No turn-signal click yet: there is no turn-signal action.

## Mixer
`audio::mixer::Mixer`: 48 kHz interleaved stereo `f32`, fixed 256-frame blocks. The synths fill three buses: engine (mono, centered), world, ui (the horn, the radio's dial sweep; menu sounds later). Bus gains: engine and world volume (squared, so the slider feels even) × the radio duck × 0 while paused; ui fixed. Ducking: `duck_gain(radio) = 1 - 0.45 × r × (2 - r)` for the radio level `r` in `[0, 1]` (to 55%, -5 dB, under a full-volume radio; ease-out, so half the radio does three quarters of the ducking); the radio integration sets `AudioParams::radio`. Then master volume (squared, 0 when muted), a 10 Hz DC blocker and a soft limiter: a peak follower with instant attack and 150 ms release whose output level bends from unity at 0.6 toward a -0.5 dBFS ceiling (`KNEE + room × (1 - e^(-over/room))`), so nothing clips. Gains move 16% of the way to their target per block and ramp linearly across it. `Mixer::render(&mut [f32])` takes any buffer length and allocates nothing (tested with a counting allocator). Tests: a 60 s render with everything at full tilt has no NaN and peaks under the ceiling; the ducking curve, volume, mute and pause are checked numerically.

### Sinks
- Native (`platform-native/src/sfx.rs`): a `cpal` output stream on the default device at 48 kHz (f32 preferred, any channel count; 256-frame buffers when the device allows, ≈5 ms). The callback owns the mixer; the main thread copies each frame's `AudioParams` into a mutex the callback only `try_lock`s, so the audio thread never waits. No device: silent, with a log line. ADR 0004.
- Web (`platform-web/src/sfx.rs` and the `sfx` script in `web/index.html`): the mixer runs on the main thread after `App::frame`. The page's AudioWorklet (registered from a Blob URL) plays a 16k-frame ring fed by `postMessage`; each frame the game asks `skywaveSfx.wanted(dt)` how much to render (two blocks plus one animation frame ahead of what the context has played, measured with `AudioContext.currentTime`) and pushes a view of wasm memory that the glue copies. The context is created at 48 kHz (the worklet resamples linearly if the browser picks another rate) on the first key, pointer or touch gesture, and resumed by later ones. `window.skywaveAudio` exposes the state, the worklet, and pushed/consumed/underrun counters. No SharedArrayBuffer, so no COOP/COEP headers are needed.
- Radio streams and files play outside this mixer (an `<audio>` element on the web); they only set the duck level.

## Licensing
- Users' files are theirs. Streams are the broadcasters'. We ship no audio. Presets are facts (URLs, names) and include a `terms_checked` date.
- Decoders: `symphonia` (MPL-2.0, file-level copyleft, acceptable as an unmodified dependency; ADR if we ever patch it). `cpal` (Apache-2.0).
- The YouTube integration follows the IFrame API terms (visible player, no downloading).

## Acceptance tests
- Station model: seeded shuffle reproduces; resume position math correct across station switches.
- Mixer: ducking curve, volume, no clipping (soft limiter) under synthetic input in a 60 s offline render; zero allocation while rendering. (Underruns are a sink matter: the web worklet counts them in `window.skywaveAudio`.)
- Synth: engine pitch tracks RPM; renders deterministically to a WAV compared by hash.
- Native: decode a generated test tone file through the full path (a WAV generated in the test, not a shipped asset).
- Manual gate: yacht rock at golden hour, switch stations, close and reopen — same station resumes.
