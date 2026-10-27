# ADR 0006: native radio streams (rustls, a hand-written HTTP/1.0 GET, nanomp3-core)

Status: accepted, 2026-10-27 (the owner asked for the stations to work in the native build;
`docs/backlog/NOTES.md` row 13)

## Context
The web build plays the radio presets' internet stations through an `<audio>` element. The
native build (Linux, Steam Deck, macOS) had the radio's state machine, keys, dash and dial
sweep, but nothing that played a stream. To match the web it needs to fetch an HTTPS
Icecast/SHOUTcast stream, decode it, play it with the game's sound, read the station's
now-playing title (ICY metadata), survive a dropped connection, and report failures so the
radio's NO SIGNAL path works the same. All of it stays out of the `no_std` crates and off
the game's thread.

The presets' streams (`docs/AUDIO_RADIO.md`): fifteen MP3 stations, and three Radio
Paradise channels (Mellow, Global, Rock) whose 128 kb/s URLs are AAC. Radio Paradise
publishes each of those channels in MP3 at 192 kb/s on the same server
(`.../mellow-192`, `global-192`, `rock-192`, `audio/mpeg`).

## Options measured
Lines are Rust source lines (`src/`, tests excluded) of the crate version that would be
pinned; licenses from each crate's manifest.

| Need | Candidate | Lines | License | Notes |
|---|---|---|---|---|
| TLS | `rustls` 0.23.46 (std, ring, tls12) | 46.5k | Apache-2.0 / ISC / MIT | the only pure-Rust TLS in wide use |
| | + `rustls-webpki` 0.103 | 10.2k | ISC | certificate checks |
| | + `rustls-pki-types` 1.15 | 4.1k | MIT / Apache-2.0 | |
| | + `ring` 0.17.14 | 24.6k Rust, 31k C, ~31k generated asm per target | Apache-2.0 AND ISC | crypto; needs a C compiler (present on every CI runner) |
| | + `untrusted`, `subtle`, `zeroize` | 0.4k, 1.0k, 1.1k | ISC, BSD-3-Clause, Apache-2.0 / MIT | |
| Trust store | `rustls-native-certs` 0.8.5 (+ `openssl-probe` 0.2) | 0.7k (+0.2k) | Apache-2.0 / ISC / MIT | the system's roots; Security.framework on macOS (MIT / Apache-2.0) |
| | `webpki-roots` 1.0 | — | CDLA-Permissive-2.0 | not on our license list; a baked-in root list also goes stale |
| HTTP | `ureq` 3.5 | ~15k + `ureq-proto`, `http`, `httparse`, `percent-encoding`, `base64`, `utf-8` | MIT / Apache-2.0 | rejects SHOUTcast's `ICY 200 OK` status line; brings chunked bodies, pools, cookies we do not need |
| | hand-written GET (`radio/http.rs`) | ~300 with tests | ours | HTTP/1.0 (never chunked), redirects, `ICY` status line, `CONNECT` through `HTTPS_PROXY` |
| MP3 | `nanomp3-core` 0.2.0, no features | 6.3k, no dependencies | MIT / Apache-2.0 | `no_std`, `#![forbid(unsafe_code)]`, allocates nothing; a bit-exact port of minimp3 |
| | `symphonia-bundle-mp3` 0.6 + `symphonia-core` | ~25k | MPL-2.0 | not on our license list |
| | `minimp3` 0.6 | 0.5k + 2k C | MIT + CC0 | C via `cc`; unmaintained bindings |
| | `puremp3` 0.1 | ~3k | MIT / CC0 | last release 2019, slower |
| AAC | `symphonia-codec-aac` 0.6 + core | ~20k | MPL-2.0 | the only pure-Rust AAC decoder |
| | `fdk-aac` | — | FDK license | not open-source compatible |
| | `faad2` | — | GPL-2.0 | excluded |

There is no permissively licensed AAC decoder. Shelling out to a system `curl` would have
replaced TLS with zero dependencies, but makes the radio depend on a program the game does
not ship and cannot check.

## Decision
- **TLS:** `rustls` with the `ring` provider (`std`, `ring`, `tls12` features; TLS 1.2
  because some stream servers still speak only that), roots from the system via
  `rustls-native-certs` (which also honours `SSL_CERT_FILE` and `SSL_CERT_DIR`). About 87k
  lines of Rust and ring's C and assembly, all permissive; `cargo deny` passes unchanged.
  Over the 5k threshold, hence this ADR: TLS is not something to write ourselves.
- **HTTP:** a hand-written `GET` in `platform-native/src/radio/http.rs`: HTTP/1.0 so the
  body is never chunked, `Icy-MetaData: 1`, `HTTP/1.x` and `ICY` status lines, up to five
  redirects (never from https down to http), a 5 s timeout on the DNS lookup and on each
  connect, a TLS handshake that may take until the stream's 10 s deadline, a 500 ms read
  timeout so a thread notices it is no longer wanted, and the proxy in `HTTPS_PROXY` (`http://host:port`, by
  `CONNECT`) unless `NO_PROXY` names the host. `http://` works too (tests use a server on
  the loopback); the radio only ever hands it `https://` URLs.
- **MP3:** `nanomp3-core` with default features off (no `wide` SIMD, no Layer I/II):
  6.3k lines, no dependencies. Its manifest declares Rust 1.89 (what its `wide` SIMD
  feature needs; its README gives 1.80 without it), so `platform-native` builds on 1.89 or
  newer (CI and releases build on stable) or with `--ignore-rust-version`. The workspace's
  own `rust-version` stays 1.87: raising it turns on newer clippy lints across crates this
  change does not otherwise touch.
- **AAC:** not decoded natively. A stream whose content type is not MP3 fails at once, so
  the radio tries the station's next URL, as it does for a dead mirror. The three AAC
  stations list their channel's MP3 URL second; the web keeps playing the AAC first, the
  native build plays the MP3. A user-added AAC-only stream shows NO SIGNAL natively.
- **Threading:** one stream thread per `Play`. It connects, splits the ICY metadata from the
  audio, decodes, resamples linearly to 48 kHz stereo (as the web worklet does) and writes
  into one single-producer ring (4 s, atomics only, no `unsafe`). The `cpal` callback reads
  the ring through a `Tap` and adds the radio to the mixer's output at the radio volume,
  with a soft knee above 0.9 so music over the engine never clips hard. A switch "cuts" the
  ring: it drops the unread audio and bumps an epoch under the writer's lock, so the old
  thread can never write again; it ends at its next check (at most a read timeout later)
  without reporting. The game's thread only sends commands and drains a channel; it never
  waits on the network or the audio thread.
- **Events and NO SIGNAL:** `Started` once 0.25 s of audio is in the ring (the callback also
  waits for that much before it plays, at the start and after running dry). A stream that
  never started fails at once (refused, HTTP error, not MP3, no audio within 10 s, the
  web's limit): `Failed`, and `audio::Radio` moves to the next URL, then NO SIGNAL and the
  next station, exactly as on the web.
- **Reconnect:** a stream that played and then dropped reconnects after 0.5, 1, 2, 4 and
  8 s; five seconds of steady play reset the count; when the tries run out it reports
  `Failed`. A connection that stays open but sends no audio that decodes for 10 s counts
  as a drop too, so the radio never shows a station as playing in silence. (The web
  `<audio>` element simply errors; native is gentler with a flaky network.)
- **ICY titles:** `StreamTitle` from each metadata block, kept per `Play` token. The window
  title shows "Skywave · station · title". Showing it on the dash needs an `App` method in
  `crates/app/src/ui`, a separate task; `radio::Player::title` is ready for it.
- **CI and benchmarks:** `--radio-off` drops the radio's commands, so nothing is fetched.
  Without an audio device the radio is dropped the same way.

## Consequences
Natively a preset station plays only if one of its URLs is MP3 on a server that speaks a
modern TLS suite: on 2026-10-27 that left three of the sixteen distinct preset stations on NO SIGNAL
(Intergalactic FM's two channels, AAC only; WMXM, whose server offers only CBC suites, which
rustls leaves out on purpose). The live test lists them and checks they still fail.
Native release builds grow by the TLS stack (ring compiles C; `cc` is on every runner and
the Steam Deck build container). `cargo deny` reports two more duplicate versions
(`core-foundation`, `getrandom`) as warnings. If a permissive AAC decoder appears, it slots
into `radio/decode.rs` behind the same content-type switch and the MP3 fallbacks can stay.
Files playback (M3) can reuse the decoder and the ring.
