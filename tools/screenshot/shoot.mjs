#!/usr/bin/env node
// Headless smoke test for the web build: serves dist/, opens it in Chromium on SwiftShader
// (so the WebGL2 path is exercised), waits for the first frame, saves a screenshot and
// fails if the page errored or the image is blank.
//
//   node tools/screenshot/shoot.mjs [--url-query "?seed=1"] [--out path.png]
//
// Uses an already-installed `playwright` package (global/NODE_PATH or tools/screenshot/
// node_modules); it does not download browsers.

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..");
const dist = join(root, "dist");

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : fallback;
};
const query = opt("--url-query", "?seed=1");
const out = resolve(opt("--out", join(here, "out", "skeleton.png")));
const WIDTH = 1280;
const HEIGHT = 720;
const READY_TIMEOUT_MS = 60_000;

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
const url = `http://127.0.0.1:${server.address().port}/${query}`;

const browser = await chromium.launch({
  headless: true,
  args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"],
});
const problems = [];
let exitCode = 0;
try {
  const page = await browser.newPage({ viewport: { width: WIDTH, height: HEIGHT } });
  page.on("console", (msg) => {
    if (msg.type() === "error" || msg.type() === "warning") {
      problems.push(`console.${msg.type()}: ${msg.text()}`);
    }
  });
  page.on("pageerror", (err) => problems.push(`pageerror: ${err.message}`));

  console.log(`shoot: opening ${url}`);
  await page.goto(url);
  const started = Date.now();
  await page.waitForFunction(() => window.skywaveReady === true || !!window.skywaveError, null, {
    timeout: READY_TIMEOUT_MS,
  });
  const { backend, error } = await page.evaluate(() => ({
    backend: window.skywaveBackend,
    error: window.skywaveError,
  }));
  if (error) throw new Error(`page reported: ${error}`);
  console.log(`shoot: first frame after ${Date.now() - started} ms on ${backend}`);
  // Let a few more frames run so the capture is a steady-state frame.
  await page.evaluate(
    () => new Promise((ok) => {
      let n = 0;
      const tick = () => (++n >= 10 ? ok() : requestAnimationFrame(tick));
      requestAnimationFrame(tick);
    }),
  );

  // SwiftShader renders this scene at seconds per frame on a loaded machine; give the
  // compositor time to produce a frame.
  const t0 = Date.now();
  const png = await page.screenshot({ type: "png", timeout: 180_000 });
  console.log(`shoot: capture took ${Date.now() - t0} ms`);
  await mkdir(dirname(out), { recursive: true });
  await writeFile(out, png);
  console.log(`shoot: wrote ${out}`);

  // Decode the PNG in a blank page and sample a grid of pixels.
  const probe = await browser.newPage();
  const distinct = await probe.evaluate(async (b64) => {
    const img = new Image();
    img.src = `data:image/png;base64,${b64}`;
    await img.decode();
    const c = document.createElement("canvas");
    c.width = img.width;
    c.height = img.height;
    const ctx = c.getContext("2d");
    ctx.drawImage(img, 0, 0);
    const seen = new Set();
    for (let y = 0; y < 18; y++) {
      for (let x = 0; x < 32; x++) {
        const px = ctx.getImageData(
          Math.floor(((x + 0.5) * img.width) / 32),
          Math.floor(((y + 0.5) * img.height) / 18),
          1,
          1,
        ).data;
        seen.add(`${px[0]},${px[1]},${px[2]}`);
      }
    }
    return seen.size;
  }, png.toString("base64"));
  console.log(`shoot: ${distinct} distinct colors in a 32x18 sample grid`);
  if (distinct < 3) throw new Error("screenshot looks blank");
} catch (e) {
  console.error(`shoot: FAILED: ${e.message}`);
  exitCode = 1;
} finally {
  for (const p of problems) console.error(`shoot: ${p}`);
  await browser.close();
  server.close();
}
process.exit(exitCode);
