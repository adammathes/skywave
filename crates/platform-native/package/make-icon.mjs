// The Skywave icon: a low-poly car in silhouette on a coast road at sunset. One list of
// flat-shaded polygons on a 32 x 32 grid draws both the 256x256 RGBA PNG (committed as
// skywave.png, used by the AppImage and the Deck install) and the SVG favicon that
// crates/platform-web/web/index.html inlines as a data URL (the loading screen's logo).
// Plain Node, no dependencies; rerun both when the drawing changes:
//
//   node crates/platform-native/package/make-icon.mjs crates/platform-native/package/skywave.png
//   node crates/platform-native/package/make-icon.mjs --svg   # paste into index.html's <link rel=icon>
import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

const SIZE = 256;
const SUB = 8; // SUB x SUB samples per pixel
const K = SIZE / 32;

// Regular n-gon (low-poly circle), flat side down.
const ngon = (cx, cy, r, n) =>
  Array.from({ length: n }, (_, k) => {
    const a = Math.PI / 2 + Math.PI / n + (2 * Math.PI * k) / n;
    return [+(cx + r * Math.cos(a)).toFixed(2), +(cy + r * Math.sin(a)).toFixed(2)];
  });

// Painted in order; later shapes cover earlier ones. Everything is clipped to the rounded square.
const SHAPES = [
  // Sky in three faceted bands, dusk blue down to orange at the horizon.
  ["#1b2140", [[0, 0], [32, 0], [32, 9], [0, 12]]],
  ["#4a2f5c", [[0, 12], [32, 9], [32, 15], [0, 16]]],
  ["#c0504a", [[0, 16], [32, 15], [32, 19], [0, 19]]],
  ["#f08a4b", [[0, 19], [32, 19], [32, 21], [0, 21]]],
  // Low-poly sun sitting on the horizon (the ground below covers its lower half).
  ["#ffb23e", ngon(16, 21, 8, 10)],
  // Sea / far ground, then the road.
  ["#2a2440", [[0, 21], [32, 21], [32, 32], [0, 32]]],
  ["#3a3550", [[0, 27.2], [32, 27.2], [32, 32], [0, 32]]],
  ["#eeeade", [[2, 29.4], [7, 29.4], [7, 30.2], [2, 30.2]]],
  ["#eeeade", [[13.5, 29.4], [18.5, 29.4], [18.5, 30.2], [13.5, 30.2]]],
  ["#eeeade", [[25, 29.4], [30, 29.4], [30, 30.2], [25, 30.2]]],
  // The car, side on: a long, low coupe with a wedge nose.
  ["#10131f", [[3.5, 26], [4, 23.2], [9.5, 22.2], [12.8, 19.2], [20.2, 19.2], [23.6, 22], [28.4, 23], [28.8, 26]]],
  // Glass, lit by the sunset through it.
  ["#f08a4b", [[13.4, 20], [16.3, 20], [16.3, 22], [11.2, 22]]],
  ["#f08a4b", [[17.1, 20], [19.8, 20], [21.9, 22], [17.1, 22]]],
  // Wheels.
  ["#05070d", ngon(9, 26.2, 2.3, 8)],
  ["#05070d", ngon(23.2, 26.2, 2.3, 8)],
  ["#6a6a78", ngon(9, 26.2, 0.9, 6)],
  ["#6a6a78", ngon(23.2, 26.2, 0.9, 6)],
];

const rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
const FILLS = SHAPES.map(([c]) => rgb(c));

function inside(poly, x, y) {
  let c = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i];
    const [xj, yj] = poly[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) c = !c;
  }
  return c;
}

// Colour of the drawing at (x, y) in grid units, or null outside the rounded square (rx 6).
function shade(x, y) {
  const cx = Math.min(Math.max(x, 6), 26);
  const cy = Math.min(Math.max(y, 6), 26);
  if (x < 0 || y < 0 || x > 32 || y > 32 || (x - cx) ** 2 + (y - cy) ** 2 > 36) return null;
  let c = null;
  for (let s = 0; s < SHAPES.length; s++) if (inside(SHAPES[s][1], x, y)) c = FILLS[s];
  return c;
}

if (process.argv[2] === "--svg") {
  const body = SHAPES.map(([c, p]) => `<path d='M${p.map((q) => q.join(" ")).join("L")}z' fill='${c}'/>`).join("");
  const svg =
    `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><clipPath id='r'><rect width='32' height='32' rx='6'/></clipPath>` +
    `<g clip-path='url(#r)'>${body}</g></svg>`;
  console.log("data:image/svg+xml," + svg.replace(/</g, "%3C").replace(/>/g, "%3E").replace(/#/g, "%23"));
  process.exit(0);
}

const px = Buffer.alloc(SIZE * SIZE * 4);
for (let j = 0; j < SIZE; j++) {
  for (let i = 0; i < SIZE; i++) {
    let r = 0;
    let g = 0;
    let b = 0;
    let n = 0;
    for (let sj = 0; sj < SUB; sj++) {
      for (let si = 0; si < SUB; si++) {
        const c = shade((i + (si + 0.5) / SUB) / K, (j + (sj + 0.5) / SUB) / K);
        if (c) {
          r += c[0];
          g += c[1];
          b += c[2];
          n++;
        }
      }
    }
    const o = (j * SIZE + i) * 4;
    if (n) {
      px[o] = Math.round(r / n);
      px[o + 1] = Math.round(g / n);
      px[o + 2] = Math.round(b / n);
      px[o + 3] = Math.round((255 * n) / (SUB * SUB));
    }
  }
}

// PNG: every row uses the "up" filter, which suits the flat shapes.
const stride = SIZE * 4;
const raw = Buffer.alloc(SIZE * (stride + 1));
for (let j = 0; j < SIZE; j++) {
  raw[j * (stride + 1)] = 2;
  for (let k = 0; k < stride; k++) {
    const up = j ? px[(j - 1) * stride + k] : 0;
    raw[j * (stride + 1) + 1 + k] = (px[j * stride + k] - up) & 255;
  }
}
const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const byte of buf) c = crcTable[(c ^ byte) & 255] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, "latin1");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, crc]);
};
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
const out = process.argv[2];
if (!out) {
  console.error("usage: node make-icon.mjs <out.png>");
  process.exit(2);
}
writeFileSync(out, png);
console.log(`make-icon: ${out} (${png.length} bytes)`);
