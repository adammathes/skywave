# Audio and radio

The radio is half the experience. It plays **real music the user owns or real stations that broadcast**, organized into era-tagged stations you flip through from the dashboard. The repository contains no audio and no lyrics; presets are URLs and metadata only.

## Stations
```
Station(
  id: "somafm_seventies", name: "SomaFM Left Coast 70s", era: "1970s",
  format: "mellow 70s album rock",
  source: Stream(urls: ["https://ice2.somafm.com/seventies-128-mp3", ...],
                 homepage: "https://somafm.com/seventies/", codec: Mp3, bitrate_kbps: 128,
                 terms_checked: "2026-10-26", verified: false)
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
Next/previous station, radio off, volume. Switching plays a 150 ms tune noise burst then the new station. Station list and current track on the dash display and as a small HUD toast. Stations persist across sessions; the last station is restored.

### As built (M3, web first)
- **`crates/audio`** (std, platform-neutral). `Radio` is the switching state machine: `Off`, `Tuning { from, to, t }` (150 ms of tune noise), `Playing { station }`, `Error { station, reason }`. Next/previous from off resume the last station; `\` is the power button. It emits `RadioCommand`s (`Play { url, token }`, `Stop`, `Volume(f32)`, `TuneNoise { seconds }`) and takes `PlayerEvent`s (`Started`, `Failed`, `Blocked`, each with the token of its `Play`; stale ones are ignored). A failed URL moves to the station's next URL; when none is left the station is in `Error` ("NO SIGNAL" on the dash, the dial still turns). `RadioSettings` (volume, power, last station, disabled stations, added streams, preset per scenario) is the `radio` section of the settings JSON. Native will consume the same commands.
- **`app::radio`**. The built-in presets, the keys (`]` `[` `\`, d-pad right/left/down), the toast ("3/7  SomaFM Left Coast 70s" over its format, 3 s; "Press any key for radio" while the browser holds playback), and the dash: in the cockpit view the radio display quad's corners are projected to the screen and the station name is drawn there as HUD text in the display's glow color, scrolling a character at a time when it does not fit. The radio starts with the first frame of a running game, never in a still scene (screenshot goldens stay silent). Settings → Audio: radio volume, preset (per scenario), Stations (on/off per station and added stream), "Add stream URL" (web: a text prompt; `https://` only), "Remove added streams".
- **Web** (`platform-web` + the radio block of `web/index.html`). One hidden `<audio preload="none">` element: `Play` sets its `src` and calls `play()`; `error` events and rejected plays come back as `failed`, a `NotAllowedError` (no user gesture yet) as `blocked`, retried on the next key, click or touch (the key that dismisses the help card counts). Tune noise is a short filtered noise buffer made with Web Audio. Media Session metadata carries the station name. `window.skywaveRadio` is the debug handle (`.log`, `.state`, `.audio`). Nothing is fetched until the user tunes a station.

## Presets
Real, listener-supported internet stations with published stream URLs; nothing is rehosted. `tools/check-streams.mjs` (Node) fetches the first bytes of every URL and checks for an audio content type; run it before changing a preset (all 41 URLs answered `audio/mpeg` or `audio/aac` on 2026-10-26). `verified` stays `false` until the owner has listened from the game. SomaFM's linking page asks that direct stream links use its published (permanent) URLs, which are the ones in its `.pls` playlists, listed here in the order ice2, ice6, ice5. Radio Paradise's stream-links page needs a browser; its terms are for the owner to re-read before release.

| Preset | Station | Format | Stream (128 kbps) |
|---|---|---|---|
| `1984_socal` | SomaFM Left Coast 70s | mellow 70s album rock | `https://ice{2,6,5}.somafm.com/seventies-128-mp3` (MP3) |
| | Radio Paradise Mellow Mix | mellow rock and soft pop | `https://stream.radioparadise.com/mellow-128` (AAC) |
| | SomaFM Underground 80s | early 80s synthpop and new wave | `https://ice{2,6,5}.somafm.com/u80s-128-mp3` |
| | Radio Paradise Main Mix | eclectic album rock | `https://stream.radioparadise.com/mp3-128` (MP3) |
| | SomaFM Secret Agent | spy-film lounge and downtempo | `https://ice{2,6,5}.somafm.com/secretagent-128-mp3` |
| | SomaFM Illinois Street Lounge | bachelor pad lounge and exotica | `https://ice{2,6,5}.somafm.com/illstreet-128-mp3` |
| | SomaFM Bossa Beyond | bossa nova and samba | `https://ice{2,6,5}.somafm.com/bossa-128-mp3` |
| `1977_chicago` | SomaFM Seven Inch Soul | vintage soul from 45s | `https://ice{2,6,5}.somafm.com/7soul-128-mp3` |
| | SomaFM Left Coast 70s | mellow 70s album rock | as above |
| | Radio Paradise Rock Mix | classic and album rock | `https://stream.radioparadise.com/rock-128` (AAC) |
| | SomaFM Illinois Street Lounge | bachelor pad lounge and exotica | as above |
| | SomaFM Sonic Universe | jazz, classic to adventurous | `https://ice{2,6,5}.somafm.com/sonicuniverse-128-mp3` |
| `1986_north_shore` | SomaFM Underground 80s | early 80s synthpop and new wave | as above |
| | Radio Paradise Rock Mix | classic and album rock | as above |
| | Radio Paradise Main Mix | eclectic album rock | as above |
| | SomaFM Left Coast 70s | mellow 70s album rock | as above |
| | SomaFM Seven Inch Soul | vintage soul from 45s | as above |

Gaps: no long-running, openly streamed disco, Chicago blues or classic-hits station with a stable published URL made the Chicago and North Shore lists; users add their own by URL (and, next, from their own files).

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
