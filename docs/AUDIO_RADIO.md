# Audio and radio

The radio is half the experience. It plays **real music the user owns or real stations that broadcast**, organized into era-tagged stations you flip through from the dashboard. The repository contains no audio and no lyrics; presets are URLs and metadata only.

## Stations
```
Station(
  id: "kyct", name: "KYCT 101.3 — The Coast", era: 1980s, format: "yacht rock",
  source: Files(folder_hint: "yacht-rock") | Stream(urls: ["https://..."], homepage: "https://...") | Youtube(playlist: "...")  // youtube later, web only
  dj: None,                       // v1 has no DJ/ident audio; station idents are visual only
  shuffle_seed: 0,                // 0 = derive from place seed so a seed reproduces the "broadcast"
)
```
A `RadioPreset` file (`assets/radio/<era>.ron`) lists stations for an era. The user's own stations live in settings and override/extend presets. The dash shows station name, format and, when available, the current track title (ICY metadata from streams, tags from files).

### Sources
1. **Files** (native + web). Native: user picks folders; `symphonia` (MPL-2.0 — allowed, note in deny.toml) decodes MP3/OGG/FLAC/AAC/WAV; tags via `symphonia` metadata. Web: file picker or drag-and-drop → `File` handles decoded by Web Audio (`decodeAudioData`) in JS glue; the WASM side only sees track lists and playback commands. Folder structure maps to stations (one folder per station) or a single library with station filters by tag/year. Playback order is a seeded shuffle so returning to a station resumes mid-"broadcast" (position = (wall clock of the place − station offset) mod playlist length): stations feel like they were already playing while you were on another one.
2. **Streams** (native + web). Internet radio over HTTPS (Icecast/SHOUTcast MP3/AAC). Native: HTTP client (`ureq`, MIT/Apache) feeding `symphonia`; ICY metadata parsed for titles. Web: an `<audio>` element driven from JS glue (streams work without CORS for playback; titles need CORS-enabled stations, otherwise none). Presets ship with a handful of well-known public stations per era/format whose terms allow listening (e.g. listener-supported stations with public stream URLs); each preset records the homepage and the date checked. Nothing is rehosted.
3. **YouTube** (later, web only). Official IFrame Player API with user-supplied playlist IDs; the player element must remain visible (ToS), so it is rendered as the "radio display" in a corner of the page. No audio-only tricks.

### Switching
Next/previous station, radio off, volume. Switching plays a 150 ms tune noise burst then the new station. Station list and current track on the dash display and as a small HUD toast. Stations persist across sessions; the last station is restored.

## Engine and world sound
No recorded audio in v1; everything is synthesized at 48 kHz by the `audio` crate (std, no dependencies). The app fills one plain `audio::synth::AudioParams` per frame (`app::sound`, read with `App::audio_params`): engine (profile, rpm, idle and redline, throttle pedal, gear, shift time), forward speed, per wheel (combined slip relative to the tire's peak, load relative to rest, tire speed, grounded, surface under the contact from the collider), curb-kick and wall-hit counters with the last hit's speed (counted per sim step from `world::Impact`, rising edges only, so no event is lost or doubled between frames), the horn, ocean and bird amounts, the volumes, mute and paused. All synth math is `+ - * /`, `floor` and `sqrt` with a polynomial sine and `exp2` (`audio::synth::dsp`), so a scripted drive renders bit-identically everywhere; its WAV is pinned by hash in `mixer::tests`.

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
- Curb kick: a 50–90 Hz falling thump and a short click. Wall hit: a 60–90 Hz thud and a soft 45 ms crunch, level from the approach speed; nothing harsh.
- Horn: two tones a minor third apart (410 and 500 Hz, five harmonics each) brightened by a 2 kHz band-pass, 6 ms attack, 40 ms release; on the ui bus.
- Ambience: an ocean wash (low-passed noise per ear swelling on 12 s and 20 s cycles) by the share of beach and water zones round the car (the car's point twice, eight points 60 m out); bird calls (2–5 swept chirps at 2.6–4.2 kHz, randomly panned, every few seconds) by the share of residential, suburb and park zones, faded in with the sun between -2° and 8°. Both stay well under the road and engine.

No turn-signal click yet: there is no turn-signal action.

## Mixer
`audio::mixer::Mixer`: 48 kHz interleaved stereo `f32`, fixed 256-frame blocks. The synths fill three buses: engine (mono, centered), world, ui (the horn; menu sounds later). Bus gains: engine and world volume (squared, so the slider feels even) × the radio duck × 0 while paused; ui fixed. Ducking: `duck_gain(radio) = 1 - 0.45 × r × (2 - r)` for the radio level `r` in `[0, 1]` (to 55%, -5 dB, under a full-volume radio; ease-out, so half the radio does three quarters of the ducking); the radio integration sets `AudioParams::radio`. Then master volume (squared, 0 when muted), a 10 Hz DC blocker and a soft limiter: a peak follower with instant attack and 150 ms release whose output level bends from unity at 0.6 toward a -0.5 dBFS ceiling (`KNEE + room × (1 - e^(-over/room))`), so nothing clips. Gains move 16% of the way to their target per block and ramp linearly across it. `Mixer::render(&mut [f32])` takes any buffer length and allocates nothing (tested with a counting allocator). Tests: a 60 s render with everything at full tilt has no NaN and peaks under the ceiling; the ducking curve, volume, mute and pause are checked numerically.

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
