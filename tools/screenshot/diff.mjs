#!/usr/bin/env node
// Compares captures in tools/screenshot/out/ with the committed goldens in
// tools/screenshot/golden/: per-pixel color difference ΔE (CIE76 on sRGB → CIELAB, D65),
// and a scene fails when more than 2% of its pixels differ by ΔE > 6. A failing scene also
// gets out/<name>.diff.png: the golden dimmed to grey with differing pixels in red→yellow.
//
//   node tools/screenshot/diff.mjs --scene strip-golden-chase   # may repeat
//   node tools/screenshot/diff.mjs --all                         # every scene in scenes.json
//
// Exit code 1 when any scene fails or a capture or golden is missing. No npm packages.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { readPng, writePng } from "./png.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const scenes = JSON.parse(readFileSync(join(here, "scenes.json"), "utf8"));
const names = args.includes("--all")
  ? Object.keys(scenes)
  : args.flatMap((a, i) => (a === "--scene" && i + 1 < args.length ? [args[i + 1]] : []));
if (names.length === 0) {
  console.error("usage: node tools/screenshot/diff.mjs --scene <name> [--scene <name>...] | --all");
  process.exit(2);
}

const DELTA_E = 6;
const MAX_SHARE = 0.02;

// sRGB byte → linear.
const LINEAR = Float64Array.from({ length: 256 }, (_, i) => {
  const c = i / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
});
const f = (t) => (t > 216 / 24389 ? Math.cbrt(t) : (24389 / 27 * t + 16) / 116);

/// CIELAB of every pixel, as [L, a, b] triples.
function lab({ rgba }) {
  const n = rgba.length / 4;
  const out = new Float32Array(n * 3);
  for (let i = 0; i < n; i++) {
    const r = LINEAR[rgba[i * 4]];
    const g = LINEAR[rgba[i * 4 + 1]];
    const b = LINEAR[rgba[i * 4 + 2]];
    const x = f((0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047);
    const y = f(0.2126 * r + 0.7152 * g + 0.0722 * b);
    const z = f((0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883);
    out[i * 3] = 116 * y - 16;
    out[i * 3 + 1] = 500 * (x - y);
    out[i * 3 + 2] = 200 * (y - z);
  }
  return out;
}

function compare(name) {
  const outPath = join(here, "out", `${name}.png`);
  const goldenPath = join(here, "golden", `${name}.png`);
  for (const p of [outPath, goldenPath]) {
    if (!existsSync(p)) {
      console.error(`diff: ${name}: missing ${p}`);
      return false;
    }
  }
  const got = readPng(readFileSync(outPath));
  const want = readPng(readFileSync(goldenPath));
  if (got.width !== want.width || got.height !== want.height) {
    console.error(
      `diff: ${name}: FAIL: capture is ${got.width}x${got.height}, golden ${want.width}x${want.height}`,
    );
    return false;
  }
  const a = lab(got);
  const b = lab(want);
  const n = got.width * got.height;
  const de = new Float32Array(n);
  let over = 0;
  let max = 0;
  for (let i = 0; i < n; i++) {
    const dl = a[i * 3] - b[i * 3];
    const da = a[i * 3 + 1] - b[i * 3 + 1];
    const db = a[i * 3 + 2] - b[i * 3 + 2];
    const d = Math.sqrt(dl * dl + da * da + db * db);
    de[i] = d;
    if (d > DELTA_E) over++;
    if (d > max) max = d;
  }
  const share = over / n;
  const pass = share <= MAX_SHARE;
  const pct = (v) => `${(v * 100).toFixed(3)}%`;
  console.log(
    `diff: ${name}: ${pct(share)} of pixels beyond ΔE ${DELTA_E} (limit ${pct(MAX_SHARE)}), ` +
      `max ΔE ${max.toFixed(1)}: ${pass ? "PASS" : "FAIL"}`,
  );
  if (!pass) {
    // Heat image: the golden as dim grey, differing pixels red (ΔE 6) to yellow (ΔE 30+).
    const heat = new Uint8Array(n * 4);
    for (let i = 0; i < n; i++) {
      const p = i * 4;
      if (de[i] > DELTA_E) {
        const t = Math.min(1, (de[i] - DELTA_E) / 24);
        heat[p] = 255;
        heat[p + 1] = Math.round(40 + 215 * t);
        heat[p + 2] = 0;
      } else {
        const grey = Math.round(
          0.25 * (0.299 * want.rgba[p] + 0.587 * want.rgba[p + 1] + 0.114 * want.rgba[p + 2]),
        );
        heat[p] = heat[p + 1] = heat[p + 2] = grey;
      }
      heat[p + 3] = 255;
    }
    const diffPath = join(here, "out", `${name}.diff.png`);
    writeFileSync(diffPath, writePng(got.width, got.height, heat));
    console.log(`diff: ${name}: wrote ${diffPath}`);
  }
  return pass;
}

let failed = 0;
for (const name of names) {
  if (!scenes[name]) {
    console.error(`diff: no scene "${name}" (have: ${Object.keys(scenes).join(", ")})`);
    failed++;
  } else if (!compare(name)) {
    failed++;
  }
}
console.log(`diff: ${names.length - failed} of ${names.length} scenes pass`);
process.exit(failed ? 1 : 0);
