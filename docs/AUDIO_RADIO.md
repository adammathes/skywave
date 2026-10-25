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
No recorded audio in v1; everything synthesized at 48 kHz in `audio::synth`: engine as a per-vehicle oscillator bank (fundamental = RPM × cylinders/120, harmonics per `engine_sound` profile, low-pass by throttle, exhaust pop on lift), tire slip noise (filtered noise by slip), wind by speed, surface rumble, turn signal click, horn (two-tone), ocean and bird ambience by zone and time. Loudness mixed under the radio with a ducking curve so music stays dominant. CC0 samples may replace synthesis later if clearly better.

## Mixer
Single mixer at 48 kHz stereo f32, 256-sample buffers on native (`cpal`), 128–256 on web (AudioWorklet in JS glue pulling from a ring buffer filled by WASM). Radio audio is mixed in by the platform (streams/files are decoded outside WASM on web) with a shared volume/ducking control. Latency target for engine sound < 20 ms.

## Licensing
- Users' files are theirs. Streams are the broadcasters'. We ship no audio. Presets are facts (URLs, names) and include a `terms_checked` date.
- Decoders: `symphonia` (MPL-2.0, file-level copyleft, acceptable as an unmodified dependency; ADR if we ever patch it). `cpal` (Apache-2.0).
- The YouTube integration follows the IFrame API terms (visible player, no downloading).

## Acceptance tests
- Station model: seeded shuffle reproduces; resume position math correct across station switches.
- Mixer: ducking curve, volume, no clipping (soft limiter) under synthetic input; buffer underrun counter is zero in a 60 s offline render.
- Synth: engine pitch tracks RPM; renders deterministically to a WAV compared by hash.
- Native: decode a generated test tone file through the full path (a WAV generated in the test, not a shipped asset).
- Manual gate: yacht rock at golden hour, switch stations, close and reopen — same station resumes.
