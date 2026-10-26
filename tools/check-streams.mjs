#!/usr/bin/env node
// Checks every stream URL in the radio presets (assets/radio/*.ron): each must answer a GET
// with HTTP 2xx and an audio content type within the timeout. Reads only the first bytes of
// each stream, then hangs up. Run it where the network reaches the broadcasters (the cloud
// sandbox's proxy blocks them), before flipping a station's `verified` to true:
//
//   node tools/check-streams.mjs                 # every preset
//   node tools/check-streams.mjs 1984_socal      # presets whose file name contains this
//   node tools/check-streams.mjs --timeout 15000 # ms per URL (default 10000)
//
// Behind a proxy, Node 22.21+ honors HTTPS_PROXY with NODE_USE_ENV_PROXY=1.
// Exits 1 if any URL failed. Prints one line per URL: ok/FAIL, station id, HTTP status,
// content type, bytes read, URL.

import { readdir, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const presets = resolve(here, "..", "assets", "radio");

const args = process.argv.slice(2);
const ti = args.indexOf("--timeout");
const TIMEOUT_MS = ti >= 0 ? Number(args[ti + 1]) : 10_000;
const filters = args.filter((a, i) => !a.startsWith("--") && !(ti >= 0 && i === ti + 1));
// Enough to see the stream start; a 128 kbps stream sends this in about a second.
const SAMPLE_BYTES = 16 * 1024;
const AUDIO_TYPES = /^(audio\/|application\/ogg|video\/mp2t)/i;

/// `[{ preset, id, urls }]` from a preset file: each station's id and its `urls: [...]`.
function stations(preset, text) {
  const out = [];
  // Strip `//` comments (the URLs contain `//` only inside strings, after a quote).
  const code = text.replace(/^\s*\/\/.*$/gm, "");
  const re = /Station\(\s*id:\s*"([^"]+)"[\s\S]*?source:\s*(Stream|Files)\(([\s\S]*?)\)\s*,?\s*\)/g;
  for (const m of code.matchAll(re)) {
    if (m[2] !== "Stream") continue;
    const list = /urls:\s*\[([\s\S]*?)\]/.exec(m[3]);
    const urls = list ? [...list[1].matchAll(/"([^"]+)"/g)].map((u) => u[1]) : [];
    out.push({ preset, id: m[1], urls });
  }
  return out;
}

async function check(url) {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), TIMEOUT_MS);
  try {
    const res = await fetch(url, {
      signal: ctl.signal,
      redirect: "follow",
      headers: { "Icy-MetaData": "0", "User-Agent": "skywave-check-streams" },
    });
    const type = res.headers.get("content-type") || "";
    let bytes = 0;
    if (res.body) {
      const reader = res.body.getReader();
      while (bytes < SAMPLE_BYTES) {
        const { done, value } = await reader.read();
        if (done) break;
        bytes += value.byteLength;
      }
      await reader.cancel().catch(() => {});
    }
    const ok = res.ok && AUDIO_TYPES.test(type) && bytes > 0;
    return { ok, status: res.status, type, bytes };
  } catch (e) {
    return { ok: false, status: 0, type: String(e && e.name === "AbortError" ? "timeout" : e.message || e), bytes: 0 };
  } finally {
    clearTimeout(timer);
    ctl.abort();
  }
}

const files = (await readdir(presets)).filter((f) => f.endsWith(".ron")).sort();
const chosen = files.filter((f) => filters.length === 0 || filters.some((p) => f.includes(p)));
if (chosen.length === 0) {
  console.error(`check-streams: no preset matches ${filters.join(", ")}`);
  process.exit(2);
}
let failed = 0;
let total = 0;
for (const f of chosen) {
  const list = stations(f.replace(/\.ron$/, ""), await readFile(join(presets, f), "utf8"));
  console.log(`# ${f}: ${list.length} stream stations`);
  for (const s of list) {
    if (s.urls.length === 0) {
      console.log(`FAIL ${s.id}: no urls`);
      failed++;
      continue;
    }
    for (const url of s.urls) {
      total++;
      const r = await check(url);
      if (!r.ok) failed++;
      console.log(`${r.ok ? "ok  " : "FAIL"} ${s.id} ${r.status} ${r.type || "-"} ${r.bytes}B ${url}`);
    }
  }
}
console.log(`check-streams: ${total - failed}/${total} URLs answered with audio`);
process.exit(failed > 0 ? 1 : 0);
