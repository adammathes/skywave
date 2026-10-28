#!/usr/bin/env node
// Headless captures of the web build: serves dist/, opens it in Chromium on SwiftShader (so
// the WebGL2 path is exercised), waits for the first frame, for streaming to finish
// (`window.skywaveStreamIdle`) and a few more frames, saves a 1280×720
// screenshot and fails if the page errored or the image is blank.
//
//   node tools/screenshot/shoot.mjs --scene strip-golden-chase   # -> out/<name>.png
//   node tools/screenshot/shoot.mjs --all                         # every scene in scenes.json
//   node tools/screenshot/shoot.mjs --all --golden                # -> golden/<name>.png
//   node tools/screenshot/shoot.mjs --selftest                    # wasm vs native hashes
//   node tools/screenshot/shoot.mjs --music                       # the deck plays a library
//   node tools/screenshot/shoot.mjs [--url-query "?seed=1"] [--out path.png]   # ad hoc
//   node tools/screenshot/shoot.mjs ... --size 1280x800          # another viewport (16:10)
//
// `--scene` may repeat and combines with `--selftest`. Scenes are named URL queries in
// `scenes.json` (keys: crates/app/src/scene.rs). `--selftest` opens `?selftest=1`, reads
// the `{"skywave_selftest": {...}}` console line and compares it with `selftest.json`, the
// values native tests pin. `--music` drops a short MP3 (silence with an ID3 tag, generated
// here) on a running game and picks a generated two-album library (Artist/Album/NN
// Title.mp3) through the folder input, then plays it on the deck (play, next track, next
// album, mode, pause) and checks the corner readout's text and the page player's log. Compare captures with goldens using `diff.mjs`.
//
// Uses an already-installed `playwright` package (global/NODE_PATH or tools/screenshot/
// node_modules); it does not download browsers.

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile, mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, extname, join, normalize, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { readPng } from "./png.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..");
const dist = join(root, "dist");

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : fallback;
};
const opts = (name) => args.flatMap((a, i) => (a === name && i + 1 < args.length ? [args[i + 1]] : []));
const flag = (name) => args.includes(name);

const [WIDTH, HEIGHT] = opt("--size", "1280x720").split("x").map(Number);
if (!(WIDTH > 0 && HEIGHT > 0)) {
  console.error("shoot: --size wants WIDTHxHEIGHT, e.g. 1280x800");
  process.exit(2);
}
const READY_TIMEOUT_MS = 60_000;
// SwiftShader renders the town at seconds per frame on a loaded machine.
const CAPTURE_TIMEOUT_MS = 180_000;
// Streaming the ring takes a few hundred frames of SwiftShader.
const STREAM_TIMEOUT_MS = 900_000;

const scenes = JSON.parse(await readFile(join(here, "scenes.json"), "utf8"));
// Written out as is: the game does not percent-decode, and `t=17:40`/`pos=x,z` need none.
const sceneQuery = (name) =>
  "?" + Object.entries(scenes[name].params).map(([k, v]) => `${k}=${v}`).join("&");

const golden = flag("--golden");
let names = flag("--all") ? Object.keys(scenes) : opts("--scene");
for (const n of names) {
  if (!scenes[n]) {
    console.error(`shoot: no scene "${n}" (have: ${Object.keys(scenes).join(", ")})`);
    process.exit(2);
  }
}
const jobs = names.map((name) => ({
  name,
  query: sceneQuery(name),
  out: join(here, golden ? "golden" : "out", `${name}.png`),
}));
const selftest = flag("--selftest");
const music = flag("--music");
if (jobs.length === 0 && !selftest && !music) {
  jobs.push({
    name: "ad hoc",
    query: opt("--url-query", "?seed=1"),
    out: resolve(opt("--out", join(here, "out", "skeleton.png"))),
  });
}

let chromium;
try {
  ({ chromium } = createRequire(import.meta.url)("playwright"));
} catch (e) {
  console.error("shoot: cannot load the `playwright` package (install it, e.g. `npm i playwright`).");
  process.exit(2);
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".css": "text/css; charset=utf-8",
  ".png": "image/png",
};

const server = createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
  const file = normalize(join(dist, path.endsWith("/") ? path + "index.html" : path));
  if (file !== dist && !file.startsWith(dist + sep)) {
    res.writeHead(403).end();
    return;
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, {
      "content-type": MIME[extname(file)] ?? "application/octet-stream",
      "cache-control": "no-store",
    });
    res.end(body);
  } catch {
    res.writeHead(404).end("not found");
  }
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const base = `http://127.0.0.1:${server.address().port}/`;

const browser = await chromium.launch({
  headless: true,
  args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"],
});

/// Opens `query` and waits until the page is ready. Returns the page, the console problems
/// list (filled as they come) and every `console.log` text.
async function open(tag, query) {
  const page = await browser.newPage({ viewport: { width: WIDTH, height: HEIGHT } });
  const problems = [];
  const logs = [];
  page.on("console", (msg) => {
    if (msg.type() === "error" || msg.type() === "warning") {
      problems.push(`console.${msg.type()}: ${msg.text()}`);
    } else if (msg.type() === "log") {
      logs.push(msg.text());
    }
  });
  page.on("pageerror", (err) => problems.push(`pageerror: ${err.message}`));
  console.log(`shoot: ${tag}: opening ${base}${query}`);
  await page.goto(base + query);
  const started = Date.now();
  await page.waitForFunction(() => window.skywaveReady === true || !!window.skywaveError, null, {
    timeout: READY_TIMEOUT_MS,
  });
  const { backend, error } = await page.evaluate(() => ({
    backend: window.skywaveBackend,
    error: window.skywaveError,
  }));
  if (error) throw new Error(`page reported: ${error}`);
  console.log(`shoot: ${tag}: ready after ${Date.now() - started} ms on ${backend}`);
  if (backend !== "selftest") {
    // A streamed scene draws its whole ring once nothing is left to stream; with
    // `stream=0` that is at once.
    await page.waitForFunction(() => window.skywaveStreamIdle === true, null, {
      timeout: STREAM_TIMEOUT_MS,
      polling: 250,
    });
    const stats = await page.evaluate(() => window.skywaveStreamStats);
    console.log(`shoot: ${tag}: streaming idle after ${Date.now() - started} ms${stats ? `: ${stats}` : ""}`);
  }
  return { page, problems, logs };
}

/// Distinct colors in a 32×18 grid of samples.
function distinctColors({ width, height, rgba }) {
  const seen = new Set();
  for (let y = 0; y < 18; y++) {
    for (let x = 0; x < 32; x++) {
      const px = Math.floor(((x + 0.5) * width) / 32);
      const py = Math.floor(((y + 0.5) * height) / 18);
      const i = (py * width + px) * 4;
      seen.add(`${rgba[i]},${rgba[i + 1]},${rgba[i + 2]}`);
    }
  }
  return seen.size;
}

async function shoot(job) {
  const { page, problems } = await open(job.name, job.query);
  try {
    // Let a few more frames run so the capture is a steady-state frame.
    await page.evaluate(
      () => new Promise((ok) => {
        let n = 0;
        const tick = () => (++n >= 10 ? ok() : requestAnimationFrame(tick));
        requestAnimationFrame(tick);
      }),
    );
    const t0 = Date.now();
    const png = await page.screenshot({ type: "png", timeout: CAPTURE_TIMEOUT_MS });
    console.log(`shoot: ${job.name}: capture took ${Date.now() - t0} ms`);
    const img = readPng(png);
    if (img.width !== WIDTH || img.height !== HEIGHT) {
      throw new Error(`capture is ${img.width}x${img.height}, not ${WIDTH}x${HEIGHT}`);
    }
    const distinct = distinctColors(img);
    console.log(`shoot: ${job.name}: ${distinct} distinct colors in a 32x18 sample grid`);
    // A blank capture never becomes a golden; it still lands in out/ to look at.
    const out = distinct < 3 ? join(here, "out", `${job.name}.blank.png`) : job.out;
    await mkdir(dirname(out), { recursive: true });
    await writeFile(out, png);
    console.log(`shoot: ${job.name}: wrote ${relative(process.cwd(), out) || out}`);
    if (distinct < 3) throw new Error("screenshot looks blank");
  } finally {
    for (const p of problems) console.error(`shoot: ${job.name}: ${p}`);
    await page.close();
  }
}

async function runSelftest() {
  const want = JSON.parse(await readFile(join(here, "selftest.json"), "utf8"));
  const { page, problems, logs } = await open("selftest", "?selftest=1");
  try {
    const prefix = '{"skywave_selftest"';
    let line;
    for (let waited = 0; !(line = logs.find((l) => l.startsWith(prefix))) && waited < 5000; waited += 100) {
      await new Promise((ok) => setTimeout(ok, 100));
    }
    if (!line) throw new Error("the page printed no skywave_selftest line");
    const got = JSON.parse(line).skywave_selftest;
    console.log(`shoot: selftest: wasm ${JSON.stringify(got)}`);
    const keys = [...new Set([...Object.keys(want), ...Object.keys(got)])].sort();
    const bad = keys.filter((k) => want[k] !== got[k]);
    for (const k of bad) {
      console.error(`shoot: selftest: ${k}: wasm ${got[k] ?? "(missing)"}, native ${want[k] ?? "(missing)"}`);
    }
    if (problems.length) throw new Error("the self-test page logged errors or warnings");
    if (bad.length) throw new Error(`${bad.length} of ${keys.length} hashes differ from selftest.json`);
    console.log(`shoot: selftest: all ${keys.length} hashes match selftest.json`);
  } finally {
    for (const p of problems) console.error(`shoot: selftest: ${p}`);
    await page.close();
  }
}

/// A `secs`-second MP3: an ID3v2.3 tag (TIT2, TPE1, and TALB and TRCK when given), then
/// silent MPEG-1 Layer III frames (128 kb/s, 44.1 kHz, mono; all-zero side information
/// decodes to silence).
function silentMp3(secs, title, artist, album, track) {
  const frame = (id, text) => {
    const body = Buffer.concat([Buffer.from([0]), Buffer.from(text, "latin1")]);
    const head = Buffer.alloc(10);
    head.write(id, 0, "latin1");
    head.writeUInt32BE(body.length, 4);
    return Buffer.concat([head, body]);
  };
  const frames = Buffer.concat([
    frame("TIT2", title),
    frame("TPE1", artist),
    ...(album ? [frame("TALB", album), frame("TRCK", String(track))] : []),
  ]);
  const n = frames.length;
  const tag = Buffer.concat([
    Buffer.from([0x49, 0x44, 0x33, 3, 0, 0, (n >> 21) & 0x7f, (n >> 14) & 0x7f, (n >> 7) & 0x7f, n & 0x7f]),
    frames,
  ]);
  const count = Math.ceil((secs * 44100) / 1152);
  const audio = Buffer.alloc(count * 417);
  for (let i = 0; i < count; i++) audio.set([0xff, 0xfb, 0x90, 0xc0], i * 417);
  return Buffer.concat([tag, audio]);
}

/// Writes a two-album library as a music app exports it (`Artist/Album/NN Title.mp3`),
/// silent MP3s with tags, into a fresh temporary folder; returns its path.
async function writeLibrary() {
  const dir = await mkdtemp(join(tmpdir(), "skywave-library-"));
  const albums = [
    ["Night Shift", "Coastline", ["Sunset Drive", "Palm Shadows"]],
    ["Tidewater", "Harbor Lights", ["Pier"]],
  ];
  for (const [artist, album, titles] of albums) {
    const at = join(dir, "Music", artist, album);
    await mkdir(at, { recursive: true });
    for (const [i, title] of titles.entries()) {
      const name = `${String(i + 1).padStart(2, "0")} ${title}.mp3`;
      await writeFile(join(at, name), silentMp3(30, title, artist, album, i + 1));
    }
  }
  return join(dir, "Music");
}

async function runMusic() {
  const { page, problems } = await open("music", "?stream=0");
  const frames = (n) =>
    page.evaluate(
      (n) => new Promise((ok) => {
        let k = 0;
        const tick = () => (++k >= n ? ok() : requestAnimationFrame(tick));
        requestAnimationFrame(tick);
      }),
      n,
    );
  const press = async (key) => {
    await page.keyboard.down(key);
    await frames(2);
    await page.keyboard.up(key);
    await frames(2);
  };
  const readout = () => page.evaluate(() => window.skywaveRadio.readout);
  const waitReadout = (re, what) =>
    page.waitForFunction((src) => new RegExp(src).test(window.skywaveRadio.readout), re.source, {
      timeout: 120_000,
      polling: 250,
    }).catch(async () => {
      throw new Error(`${what}: the readout says ${JSON.stringify(await readout())}`);
    });
  try {
    // A two-album library through the folder picker's input (a synthetic drop cannot carry
    // folders), then a loose dropped file: a mixtape beside it.
    const library = await writeLibrary();
    await page.setInputFiles("input[type=file]", library);
    await page.waitForFunction(() => window.skywaveMusic.tracks === 3, null, { timeout: 30_000 });
    await frames(3);
    const mp3 = silentMp3(30, "Test Tone", "Skywave");
    await page.evaluate((bytes) => {
      const file = new File([new Uint8Array(bytes)], "01 test tone.mp3", { type: "audio/mpeg" });
      const data = new DataTransfer();
      data.items.add(file);
      window.dispatchEvent(new DragEvent("drop", { dataTransfer: data, bubbles: true, cancelable: true }));
    }, [...mp3]);
    await page.waitForFunction(() => window.skywaveMusic.tracks === 4, null, { timeout: 30_000 });
    // The same folder picked again replaces itself.
    await page.setInputFiles("input[type=file]", library);
    await frames(10);
    if ((await page.evaluate(() => window.skywaveMusic.tracks)) !== 4) throw new Error("a second pick doubled the library");
    await frames(3);
    // ' makes the deck (a tape deck in 1984) the source and plays the library's first
    // album: Night Shift's, by artist.
    await press("Quote");
    await waitReadout(/^TAPE >\nNight Shift\nCoastline\nSunset Drive$/, "play");
    await page.waitForFunction(() => /^off/.test(window.skywaveRadio.state) && !window.skywaveRadio.audio.paused, null, {
      timeout: 120_000,
      polling: 250,
    });
    await press("BracketRight");
    await waitReadout(/^TAPE >\nNight Shift\nCoastline\nPalm Shadows$/, "next");
    await press("KeyP");
    await waitReadout(/^TAPE >\nTidewater\nHarbor Lights\nPier$/, "next album");
    await press("Semicolon");
    await waitReadout(/^TAPE SHUFFLE >\n/, "mode");
    await press("Backslash");
    await waitReadout(/^TAPE SHUFFLE \|\|\n/, "pause");
    const { log, text } = await page.evaluate(() => ({
      log: window.skywaveRadio.log.join(" | "),
      text: window.skywaveRadio.readout,
    }));
    console.log(`shoot: music: readout ${JSON.stringify(text)}; player log: ${log}`);
    for (const want of [
      /file [a-z][0-9a-f]+\/Night Shift\/Coastline\/01 Sunset Drive\.mp3 \d+/,
      /file [a-z][0-9a-f]+\/Night Shift\/Coastline\/02 Palm Shadows\.mp3 \d+/,
      /file [a-z][0-9a-f]+\/Tidewater\/Harbor Lights\/01 Pier\.mp3 \d+/,
      /event started \d+/,
      /\| stop( \||$)/,
    ]) {
      if (!want.test(log)) throw new Error(`the player log lacks ${want}`);
    }
    // Warnings are expected (no WebGPU adapter on SwiftShader); errors are not.
    if (problems.some((p) => !p.startsWith("console.warning"))) throw new Error("the page logged errors");
    console.log("shoot: music: the library plays on the deck: play, next, album, mode, pause");
  } finally {
    for (const p of problems) console.error(`shoot: music: ${p}`);
    await page.close();
  }
}

let failed = 0;
try {
  if (music) {
    try {
      await runMusic();
    } catch (e) {
      console.error(`shoot: music: FAILED: ${e.message}`);
      failed++;
    }
  }
  if (selftest) {
    try {
      await runSelftest();
    } catch (e) {
      console.error(`shoot: selftest: FAILED: ${e.message}`);
      failed++;
    }
  }
  // One at a time: SwiftShader already uses every core.
  for (const job of jobs) {
    try {
      await shoot(job);
    } catch (e) {
      console.error(`shoot: ${job.name}: FAILED: ${e.message}`);
      failed++;
    }
  }
} finally {
  await browser.close();
  server.close();
}
process.exit(failed ? 1 : 0);
