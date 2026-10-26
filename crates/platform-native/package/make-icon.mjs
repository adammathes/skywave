// Renders the Skywave icon (the sun over the horizon from crates/platform-web/web/index.html)
// to a 256x256 RGBA PNG. Plain Node, no dependencies; the output is committed as skywave.png
// and only needs regenerating when the drawing changes:
//
//   node crates/platform-native/package/make-icon.mjs crates/platform-native/package/skywave.png
import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

const SIZE = 256;
const SUB = 8; // SUB x SUB samples per pixel
const K = SIZE / 32; // the drawing is on a 32 x 32 grid

const rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
const NIGHT = rgb("#141a2c");
const SUN = rgb("#ffb23e");
const LINE = rgb("#eeeade");

// Colour of the drawing at (x, y) in grid units, or null outside it. Shapes paint in order.
function shade(x, y) {
  // Rounded square, rx 6.
  const cx = Math.min(Math.max(x, 6), 26);
  const cy = Math.min(Math.max(y, 6), 26);
  if (x < 0 || y < 0 || x > 32 || y > 32 || (x - cx) ** 2 + (y - cy) ** 2 > 36) return null;
  let c = NIGHT;
  // Half sun: radius 9 around (16, 20), above y = 20.
  if (y <= 20 && (x - 16) ** 2 + (y - 20) ** 2 <= 81) c = SUN;
  // Two bands cut through the sun, 1.3 wide.
  if (Math.abs(y - 16.6) <= 0.65 && x >= 8.6 && x <= 23.4) c = NIGHT;
  if (Math.abs(y - 13.4) <= 0.65 && x >= 10.4 && x <= 21.6) c = NIGHT;
  // Horizon: 1.8 wide, round caps, from x 4.5 to 27.5 at y 23.
  const hx = Math.min(Math.max(x, 4.5), 27.5);
  if ((x - hx) ** 2 + (y - 23) ** 2 <= 0.81) c = LINE;
  return c;
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
