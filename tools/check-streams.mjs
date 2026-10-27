#!/usr/bin/env node
// Checks every stream URL in the radio presets (assets/radio/*.ron) the way a browser on the
// published page asks for it: HTTPS only (the page is https, so an http stream is blocked as
// mixed content), a browser User-Agent, the page's Referer and Origin, redirects followed. A
// URL passes when it answers 200 with an `audio/*` content type and sends at least 5 seconds
// of audio (the station's `bitrate_kbps` × 5 s of bytes) within the timeout; then it hangs up.
// Some broadcasters refuse requests from other sites (a 403 with a Referer): those fail here
// exactly as they fail in the game.
//
//   node tools/check-streams.mjs                 # every preset
//   node tools/check-streams.mjs 1984_socal      # presets whose file name contains this
//   node tools/check-streams.mjs --timeout 20000 # ms per URL (default 15000)
//
// Behind a proxy, Node 22.21+ honors HTTPS_PROXY with NODE_USE_ENV_PROXY=1.
// Prints one line per URL: ok/FAIL, station id, HTTP status, content type, bytes read, time
// to the first byte, URL. Exits 1 if any URL failed.

import { readdir, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const presets = resolve(here, "..", "assets", "radio");

const PAGE = "https://adammathes.github.io/skywave/";
const HEADERS = {
  "User-Agent":
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
  Accept: "*/*",
  Referer: PAGE,
  Origin: new URL(PAGE).origin,
};
const SECONDS = 5;

const args = process.argv.slice(2);
const ti = args.indexOf("--timeout");
const TIMEOUT_MS = ti >= 0 ? Number(args[ti + 1]) : 15_000;
const filters = args.filter((a, i) => !a.startsWith("--") && !(ti >= 0 && i === ti + 1));

/// `[{ id, urls, kbps }]` from a preset file: each stream station's id, `urls: [...]` and
/// `bitrate_kbps`.
function stations(text) {
  const out = [];
  // Strip `//` comments (the URLs contain `//` only inside strings, after a quote).
  const code = text.replace(/^\s*\/\/.*$/gm, "");
  const re = /Station\(\s*id:\s*"([^"]+)"[\s\S]*?source:\s*(Stream|Files)\(([\s\S]*?)\)\s*,?\s*\)/g;
  for (const m of code.matchAll(re)) {
    if (m[2] !== "Stream") continue;
    const list = /urls:\s*\[([\s\S]*?)\]/.exec(m[3]);
    const urls = list ? [...list[1].matchAll(/"([^"]+)"/g)].map((u) => u[1]) : [];
    const kbps = Number((/bitrate_kbps:\s*(\d+)/.exec(m[3]) || [])[1] || 128);
    out.push({ id: m[1], urls, kbps });
  }
  return out;
}

async function check(url, kbps) {
  const want = Math.ceil((kbps * 1000 / 8) * SECONDS);
  if (!url.startsWith("https://")) return { ok: false, status: 0, type: "not https", bytes: 0, ms: 0 };
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), TIMEOUT_MS);
  const t0 = performance.now();
  let ms = 0;
  let status = 0;
  let type = "";
  let bytes = 0;
  try {
    const res = await fetch(url, { signal: ctl.signal, redirect: "follow", headers: HEADERS });
    ms = Math.round(performance.now() - t0);
    status = res.status;
    type = res.headers.get("content-type") || "";
    if (res.body && status === 200 && /^audio\//i.test(type)) {
      const reader = res.body.getReader();
      while (bytes < want) {
        const { done, value } = await reader.read();
        if (done) break;
        bytes += value.byteLength;
      }
      await reader.cancel().catch(() => {});
    }
    const ok = status === 200 && /^audio\//i.test(type) && bytes >= want;
    return { ok, status, type, bytes, ms };
  } catch (e) {
    const why = e && e.name === "AbortError" ? "timeout" : String((e && (e.cause?.code || e.message)) || e);
    return { ok: false, status, type: type || why, bytes, ms: ms || Math.round(performance.now() - t0) };
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
  const list = stations(await readFile(join(presets, f), "utf8"));
  console.log(`# ${f}: ${list.length} stream stations`);
  for (const s of list) {
    if (s.urls.length === 0) {
      console.log(`FAIL ${s.id}: no urls`);
      failed++;
      continue;
    }
    for (const url of s.urls) {
      total++;
      const r = await check(url, s.kbps);
      if (!r.ok) failed++;
      console.log(`${r.ok ? "ok  " : "FAIL"} ${s.id} ${r.status} ${r.type || "-"} ${r.bytes}B ${r.ms}ms ${url}`);
    }
  }
}
console.log(`check-streams: ${total - failed}/${total} URLs played ${SECONDS} s of audio as from ${PAGE}`);
process.exit(failed > 0 ? 1 : 0);
