#!/usr/bin/env node
// Checks every stream URL in the radio presets (assets/radio/*.ron) the way a browser on the
// published page asks for it: HTTPS only (the page is https, so an http stream is blocked as
// mixed content), a browser User-Agent, the page's Referer and Origin, redirects followed. A
// URL passes when it answers 200 with an `audio/*` content type and sends at least 5 seconds
// of audio (the station's `bitrate_kbps` × 5 s of bytes) within the timeout; then it hangs up.
// Some broadcasters refuse requests from other sites (a 403 with a Referer): those fail here
// exactly as they fail in the game.
//
// Presets are ad-free (docs/AUDIO_RADIO.md, "the ad-free rule"): every station carries a
// `// ad-free: ...` comment line just above it saying when and how that was checked; the
// checker warns about any station without one. It also warns about signs of ad insertion:
// response headers, redirect hosts and ICY titles of the ad-stitching servers that
// commercial aggregators use for pre-rolls (see AD_MARKS).
//
//   node tools/check-streams.mjs                 # every preset
//   node tools/check-streams.mjs 1984_socal      # presets whose file name contains this
//   node tools/check-streams.mjs --timeout 20000 # ms per URL (default 15000)
//   node tools/check-streams.mjs --listen 60     # read 60 s per URL (of audio and of wall
//                                                # time) instead of 5, printing the ICY
//                                                # titles as they change
//   node tools/check-streams.mjs --listen 60 --save out/  # and keep the audio (ICY blocks
//                                                # removed) as out/<id>-<n>.mp3|aac to listen
//   node tools/check-streams.mjs --url https://x/stream --kbps 128  # a candidate URL
//
// Listening sends `Icy-MetaData: 1` (to read the titles), which the page's player does not.
// Behind a proxy, Node 22.21+ honors HTTPS_PROXY with NODE_USE_ENV_PROXY=1.
// Prints one line per URL: ok/FAIL, station id, HTTP status, content type, bytes read, time
// to the first byte, URL; then a `warn` line for each ad sign. Exits 1 if any URL failed.

import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
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

const args = process.argv.slice(2);
const VALUED = ["--timeout", "--listen", "--save", "--url", "--kbps"];
const opt = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const LISTEN = opt("--listen") !== undefined ? Number(opt("--listen")) : 0;
const SECONDS = LISTEN > 0 ? LISTEN : 5;
const TIMEOUT_MS = Number(opt("--timeout") ?? (LISTEN > 0 ? (LISTEN + 20) * 1000 : 15_000));
const SAVE = opt("--save");
const filters = args.filter((a, i) => !a.startsWith("--") && !VALUED.includes(args[i - 1]));

/// Signs of server-side ad insertion: the stitching servers commercial aggregators put in
/// front of their streams, and the markers they leave in ICY titles. A match is a warning to
/// listen for, not proof.
const AD_MARKS = [
  [/^server$/i, /^DAS$/, "dynamic ad-stitching server"],
  [/^x-das-/i, /./, "ad-stitching session header"],
  [/^x-ad/i, /./, "ad header"],
  [/^location$/i, /adswizz|tritondigital|streamtheworld|spacial|aw_0_|cdnstream/i, "ad-insertion CDN"],
  [/^url$/, /adswizz|tritondigital|streamtheworld|aw_0_|cdnstream/i, "ad-insertion CDN"],
  [/^title$/, /adw_ad|insertionType|preroll|advert|commercial break|after this break/i, "ad marker in the ICY title"],
];

function adSigns(pairs) {
  const out = [];
  for (const [k, v] of pairs) {
    for (const [key, val, why] of AD_MARKS) {
      if (key.test(k) && val.test(v)) out.push(`${why} (${k}: ${v.slice(0, 80)})`);
    }
  }
  return out;
}

/// `[{ id, urls, kbps, adFree }]` from a preset file: each stream station's id, `urls: [...]`,
/// `bitrate_kbps`, and whether the comment block just above it has an `ad-free:` line.
function stations(text) {
  const out = [];
  // Blank out `//` comment lines (the URLs contain `//` only inside strings, after a quote),
  // keeping offsets, and remember where each `ad-free:` line ends.
  const notes = [...text.matchAll(/^\s*\/\/\s*ad-free:.*$/gm)].map((m) => m.index + m[0].length);
  const code = text.replace(/^\s*\/\/.*$/gm, (c) => " ".repeat(c.length));
  const re = /Station\(\s*id:\s*"([^"]+)"[\s\S]*?source:\s*(Stream|Files)\(([\s\S]*?)\)\s*,?\s*\)/g;
  let prev = 0;
  for (const m of code.matchAll(re)) {
    const adFree = notes.some((n) => n > prev && n < m.index);
    prev = m.index + m[0].length;
    if (m[2] !== "Stream") continue;
    const list = /urls:\s*\[([\s\S]*?)\]/.exec(m[3]);
    const urls = list ? [...list[1].matchAll(/"([^"]+)"/g)].map((u) => u[1]) : [];
    const kbps = Number((/bitrate_kbps:\s*(\d+)/.exec(m[3]) || [])[1] || 128);
    out.push({ id: m[1], urls, kbps, adFree });
  }
  return out;
}

/// Splits an ICY stream (`metaint` audio bytes, then a length byte × 16 of metadata) into
/// audio and titles; `st` carries the position across chunks.
function deIcy(chunk, st, audio, titles, t) {
  let i = 0;
  while (i < chunk.length) {
    if (st.left > 0) {
      const n = Math.min(st.left, chunk.length - i);
      audio.push(chunk.subarray(i, i + n));
      st.left -= n;
      i += n;
    } else if (st.meta === null) {
      st.meta = chunk[i++] * 16;
      st.buf = [];
      if (st.meta === 0) {
        st.meta = null;
        st.left = st.metaint;
      }
    } else {
      const n = Math.min(st.meta - st.buf.length, chunk.length - i);
      for (let k = 0; k < n; k++) st.buf.push(chunk[i + k]);
      i += n;
      if (st.buf.length === st.meta) {
        const text = Buffer.from(st.buf).toString("latin1").replace(/\0+$/, "");
        const title = /StreamTitle='(.*?)';/.exec(text)?.[1] ?? text;
        if (title !== st.title) titles.push([t(), title]);
        st.title = title;
        st.meta = null;
        st.left = st.metaint;
      }
    }
  }
}

async function check(url, kbps) {
  const want = Math.ceil((kbps * 1000 / 8) * SECONDS);
  const signs = [];
  const titles = [];
  const audio = [];
  let firstSecond = 0;
  if (!url.startsWith("https://")) return { ok: false, status: 0, type: "not https", bytes: 0, ms: 0, signs, titles, audio };
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), TIMEOUT_MS);
  const t0 = performance.now();
  let ms = 0;
  let status = 0;
  let type = "";
  let bytes = 0;
  try {
    const headers = LISTEN > 0 ? { ...HEADERS, "Icy-MetaData": "1" } : HEADERS;
    const res = await fetch(url, { signal: ctl.signal, redirect: "follow", headers });
    ms = Math.round(performance.now() - t0);
    status = res.status;
    type = res.headers.get("content-type") || "";
    signs.push(...adSigns([...res.headers.entries(), ["url", res.url]]));
    const metaint = Number(res.headers.get("icy-metaint") || 0);
    const st = { metaint, left: metaint, meta: null, buf: [], title: null };
    const since = () => Math.round((performance.now() - t0) / 100) / 10;
    if (res.body && status === 200 && /^audio\//i.test(type)) {
      const reader = res.body.getReader();
      // Listening reads for LISTEN s of wall time too: a stream that bursts its buffer at
      // the start still plays a minute in real time, the way a listener hears it.
      const enough = () => bytes >= want && (LISTEN === 0 || performance.now() - t0 >= LISTEN * 1000);
      while (!enough()) {
        const { done, value } = await reader.read();
        if (done) break;
        bytes += value.byteLength;
        if (performance.now() - t0 < ms + 1000) firstSecond = bytes;
        if (metaint > 0) deIcy(value, st, audio, titles, since);
        else audio.push(value);
      }
      await reader.cancel().catch(() => {});
    }
    signs.push(...adSigns(titles.map(([, title]) => ["title", title])));
    const ok = status === 200 && /^audio\//i.test(type) && bytes >= want;
    const burst = Math.round(firstSecond / (kbps * 1000 / 8));
    return { ok, status, type, bytes, ms, signs, titles, audio, burst };
  } catch (e) {
    const why = e && e.name === "AbortError" ? "timeout" : String((e && (e.cause?.code || e.message)) || e);
    return { ok: false, status, type: type || why, bytes, ms: ms || Math.round(performance.now() - t0), signs, titles, audio };
  } finally {
    clearTimeout(timer);
    ctl.abort();
  }
}

const groups = [];
if (opt("--url")) {
  const kbps = Number(opt("--kbps") || 128);
  groups.push(["--url", [{ id: "candidate", urls: [opt("--url")], kbps, adFree: true }]]);
} else {
  const files = (await readdir(presets)).filter((f) => f.endsWith(".ron")).sort();
  const chosen = files.filter((f) => filters.length === 0 || filters.some((p) => f.includes(p)));
  if (chosen.length === 0) {
    console.error(`check-streams: no preset matches ${filters.join(", ")}`);
    process.exit(2);
  }
  for (const f of chosen) groups.push([f, stations(await readFile(join(presets, f), "utf8"))]);
}
if (SAVE) await mkdir(SAVE, { recursive: true });
let failed = 0;
let total = 0;
let warned = 0;
for (const [f, list] of groups) {
  console.log(`# ${f}: ${list.length} stream stations`);
  for (const s of list) {
    if (!s.adFree) {
      warned++;
      console.log(`warn ${s.id}: no "// ad-free:" note above the station (docs/AUDIO_RADIO.md)`);
    }
    if (s.urls.length === 0) {
      console.log(`FAIL ${s.id}: no urls`);
      failed++;
      continue;
    }
    for (const [n, url] of s.urls.entries()) {
      total++;
      const r = await check(url, s.kbps);
      if (!r.ok) failed++;
      const burst = LISTEN > 0 && r.burst !== undefined ? ` burst ${r.burst}s` : "";
      console.log(`${r.ok ? "ok  " : "FAIL"} ${s.id} ${r.status} ${r.type || "-"} ${r.bytes}B ${r.ms}ms${burst} ${url}`);
      if (LISTEN > 0) for (const [t, title] of r.titles) console.log(`     ${t}s ${JSON.stringify(title)}`);
      for (const why of new Set(r.signs)) {
        warned++;
        console.log(`warn ${s.id}: ${why}`);
      }
      if (SAVE && r.audio.length > 0) {
        const ext = /aac/i.test(r.type) ? "aac" : "mp3";
        await writeFile(join(SAVE, `${s.id}-${n}.${ext}`), Buffer.concat(r.audio));
      }
    }
  }
}
console.log(
  `check-streams: ${total - failed}/${total} URLs played ${SECONDS} s of audio as from ${PAGE}; ${warned} warnings`,
);
process.exit(failed > 0 ? 1 : 0);
