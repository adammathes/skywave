#!/usr/bin/env node
// A burst of consecutive frames from the web build on WebGL2 (SwiftShader), to catch what
// changes from one frame to the next: lights or models that pop or flicker. Opens a scene,
// optionally holds keys (drive), then copies the canvas inside each animation frame, right
// after the game drew it, so the frames are consecutive and need no screenshot round trip.
// Game time runs at 1/60 s per frame (the page's animation-frame clock is replaced).
//
//   node tools/screenshot/burst.mjs --url-query "?place=coast&t=22:30&lights=1&traffic=busy" \
//       --press KeyH --frames 8 --hold KeyW --warm 20 --out out/burst-night
//
// Writes <out>/<NN>.png and prints, per pair of frames, how many pixels changed by more than
// a threshold and the changed area's box: with the camera still (no --hold) only the moving
// traffic should differ; a lamp pool or a building that differs is the flicker.
// `--scene <name>` takes a scene from scenes.json instead of --url-query (`still=1` is
// dropped, so the world runs); `--dist <dir>` serves another build than dist/.

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { readPng, writePng } from "./png.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : fallback;
};
// Another build to serve (an older dist/ copied aside, for a before/after strip).
const dist = resolve(opt("--dist", join(here, "..", "..", "dist")));
const opts = (name) => args.flatMap((a, i) => (a === name && i + 1 < args.length ? [args[i + 1]] : []));

const [WIDTH, HEIGHT] = opt("--size", "1280x720").split("x").map(Number);
const FRAMES = Number(opt("--frames", "8"));
const WARM = Number(opt("--warm", "10"));
const HOLD = opts("--hold");
const PRESS = opts("--press");
const THRESHOLD = Number(opt("--threshold", "24"));
const out = resolve(opt("--out", join(here, "out", "burst")));
let query = opt("--url-query", "?seed=1");
const scene = opt("--scene", null);
if (scene) {
  const scenes = JSON.parse(await readFile(join(here, "scenes.json"), "utf8"));
  if (!scenes[scene]) {
    console.error(`burst: no scene "${scene}"`);
    process.exit(2);
  }
  query = "?" + Object.entries(scenes[scene].params)
    .filter(([k]) => k !== "still")
    .map(([k, v]) => `${k}=${v}`)
    .join("&");
}

const { chromium } = createRequire(import.meta.url)("playwright");
const MIME = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };
const server = createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
  const file = normalize(join(dist, path.endsWith("/") ? path + "index.html" : path));
  if (file !== dist && !file.startsWith(dist + sep)) return res.writeHead(403).end();
  try {
    const body = await readFile(file);
    res.writeHead(200, { "content-type": MIME[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404).end();
  }
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const base = `http://127.0.0.1:${server.address().port}/`;
const browser = await chromium.launch({
  headless: true,
  args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"],
});
let failed = false;
try {
  const page = await browser.newPage({ viewport: { width: WIDTH, height: HEIGHT } });
  page.on("pageerror", (e) => console.error(`burst: pageerror: ${e.message}`));
  page.on("console", (m) => m.type() === "error" && console.error(`burst: console: ${m.text()}`));
  // Every animation frame is 1/60 s of game time however long SwiftShader takes to draw it,
  // so consecutive frames are what a 60 Hz display would show.
  await page.addInitScript(() => {
    const raf = window.requestAnimationFrame.bind(window);
    let real = -1, fake = 0;
    window.requestAnimationFrame = (cb) => raf((t) => {
      if (t !== real) {
        real = t;
        fake += 1000 / 60;
      }
      cb(fake);
    });
  });
  console.log(`burst: opening ${base}${query}`);
  await page.goto(base + query);
  await page.waitForFunction(() => window.skywaveReady === true || !!window.skywaveError, null, { timeout: 60_000 });
  const err = await page.evaluate(() => window.skywaveError);
  if (err) throw new Error(err);
  await page.waitForFunction(() => window.skywaveStreamIdle === true, null, { timeout: 900_000, polling: 250 });
  console.log(`burst: ready on ${await page.evaluate(() => window.skywaveBackend)}`);
  // Keys tapped first (H closes the controls card a fresh profile shows).
  for (const k of PRESS) {
    await page.keyboard.down(k);
    await page.evaluate(() => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok))));
    await page.keyboard.up(k);
  }
  for (const k of HOLD) await page.keyboard.down(k);
  // Copies the canvas in the animation frame after the game's own callback (callbacks run
  // in the order they were registered, and the game registers its next one first).
  const size = await page.evaluate(
    ({ warm, count }) => new Promise((ok) => {
      const gl = document.getElementById("skywave");
      const got = (window.burstFrames = []);
      let n = 0;
      const tick = () => {
        n++;
        if (n > warm) {
          const c = document.createElement("canvas");
          c.width = gl.width;
          c.height = gl.height;
          c.getContext("2d").drawImage(gl, 0, 0);
          got.push(c);
        }
        if (got.length >= count) ok({ w: gl.width, h: gl.height });
        else requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    }),
    { warm: WARM, count: FRAMES },
  );
  for (const k of HOLD) await page.keyboard.up(k);
  await mkdir(out, { recursive: true });
  const { w, h } = size;
  const imgs = [];
  for (let i = 0; i < FRAMES; i++) {
    const url = await page.evaluate((i) => window.burstFrames[i].toDataURL("image/png"), i);
    imgs.push(readPng(Buffer.from(url.slice(url.indexOf(",") + 1), "base64")).rgba);
  }
  for (const [i, rgba] of imgs.entries()) {
    await writeFile(join(out, `${String(i).padStart(2, "0")}.png`), writePng(w, h, rgba));
  }
  for (let i = 1; i < imgs.length; i++) {
    const a = imgs[i - 1], b = imgs[i];
    let changed = 0, x0 = w, y0 = h, x1 = -1, y1 = -1;
    for (let p = 0; p < w * h; p++) {
      const d = Math.max(Math.abs(a[4 * p] - b[4 * p]), Math.abs(a[4 * p + 1] - b[4 * p + 1]), Math.abs(a[4 * p + 2] - b[4 * p + 2]));
      if (d > THRESHOLD) {
        changed++;
        const x = p % w, y = (p / w) | 0;
        x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y);
      }
    }
    // Where it changed: the later frame dimmed, the changed pixels in magenta.
    const d = new Uint8Array(w * h * 4);
    for (let p = 0; p < w * h; p++) {
      const hit = Math.max(Math.abs(a[4 * p] - b[4 * p]), Math.abs(a[4 * p + 1] - b[4 * p + 1]), Math.abs(a[4 * p + 2] - b[4 * p + 2])) > THRESHOLD;
      d[4 * p] = hit ? 255 : b[4 * p] >> 2;
      d[4 * p + 1] = hit ? 0 : b[4 * p + 1] >> 2;
      d[4 * p + 2] = hit ? 255 : b[4 * p + 2] >> 2;
      d[4 * p + 3] = 255;
    }
    await writeFile(join(out, `diff-${String(i).padStart(2, "0")}.png`), writePng(w, h, d));
    console.log(`burst: ${i - 1}->${i}: ${changed} px changed (${(100 * changed / (w * h)).toFixed(2)}%)${changed ? ` in [${x0},${y0}]-[${x1},${y1}]` : ""}`);
  }
  console.log(`burst: wrote ${imgs.length} frames to ${out}`);
} catch (e) {
  console.error(`burst: FAILED: ${e.message}`);
  failed = true;
} finally {
  await browser.close();
  server.close();
}
process.exit(failed ? 1 : 0);
