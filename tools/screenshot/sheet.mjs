// Contact sheets for the landmark gallery (`shoot.mjs --gallery <kit>`): tiles captures
// into one PNG, a column per variant (A, B, C...) and a row per time of day and camera,
// with the kit's name, each variant's letter and name over its column and each row's time
// and camera at its left, lettered in a built-in 5x7 pixel font. No npm packages.
//
//   composeSheet({ title, columns: [{ letter, name }], rows: [{ time, camera }],
//                  tiles: (row, col) => img | null, tileWidth }) -> { width, height, rgba }
//
// `img` is what png.mjs's readPng returns; a missing tile is drawn as a dark cell.

// 5x7 glyphs, a row per number, bit 4 the leftmost pixel.
const FONT = {
  A: [14, 17, 17, 31, 17, 17, 17], B: [30, 17, 17, 30, 17, 17, 30], C: [14, 17, 16, 16, 16, 17, 14],
  D: [30, 17, 17, 17, 17, 17, 30], E: [31, 16, 16, 30, 16, 16, 31], F: [31, 16, 16, 30, 16, 16, 16],
  G: [14, 17, 16, 23, 17, 17, 15], H: [17, 17, 17, 31, 17, 17, 17], I: [14, 4, 4, 4, 4, 4, 14],
  J: [7, 2, 2, 2, 2, 18, 12], K: [17, 18, 20, 24, 20, 18, 17], L: [16, 16, 16, 16, 16, 16, 31],
  M: [17, 27, 21, 21, 17, 17, 17], N: [17, 17, 25, 21, 19, 17, 17], O: [14, 17, 17, 17, 17, 17, 14],
  P: [30, 17, 17, 30, 16, 16, 16], Q: [14, 17, 17, 17, 21, 18, 13], R: [30, 17, 17, 30, 20, 18, 17],
  S: [15, 16, 16, 14, 1, 1, 30], T: [31, 4, 4, 4, 4, 4, 4], U: [17, 17, 17, 17, 17, 17, 14],
  V: [17, 17, 17, 17, 17, 10, 4], W: [17, 17, 17, 21, 21, 21, 10], X: [17, 17, 10, 4, 10, 17, 17],
  Y: [17, 17, 10, 4, 4, 4, 4], Z: [31, 1, 2, 4, 8, 16, 31],
  0: [14, 17, 19, 21, 25, 17, 14], 1: [4, 12, 4, 4, 4, 4, 14], 2: [14, 17, 1, 2, 4, 8, 31],
  3: [31, 2, 4, 2, 1, 17, 14], 4: [2, 6, 10, 18, 31, 2, 2], 5: [31, 16, 30, 1, 1, 17, 14],
  6: [6, 8, 16, 30, 17, 17, 14], 7: [31, 1, 2, 4, 8, 8, 8], 8: [14, 17, 17, 14, 17, 17, 14],
  9: [14, 17, 17, 15, 1, 2, 12],
  " ": [0, 0, 0, 0, 0, 0, 0], ",": [0, 0, 0, 0, 12, 4, 8], ".": [0, 0, 0, 0, 0, 12, 12],
  "-": [0, 0, 0, 31, 0, 0, 0], ":": [0, 12, 12, 0, 12, 12, 0], "'": [4, 4, 8, 0, 0, 0, 0],
  "/": [1, 1, 2, 4, 8, 16, 16], "(": [2, 4, 8, 8, 8, 4, 2], ")": [8, 4, 2, 2, 2, 4, 8],
  "_": [0, 0, 0, 0, 0, 0, 31], "?": [14, 17, 1, 2, 4, 0, 4],
};

const INK = [236, 232, 220];
const DIM = [150, 150, 158];
const BG = [24, 24, 30];
const CELL = [44, 44, 52];
const GAP = 4;

function fill(img, x0, y0, w, h, rgb) {
  for (let y = Math.max(0, y0); y < Math.min(img.height, y0 + h); y++) {
    for (let x = Math.max(0, x0); x < Math.min(img.width, x0 + w); x++) {
      const i = (y * img.width + x) * 4;
      img.rgba[i] = rgb[0];
      img.rgba[i + 1] = rgb[1];
      img.rgba[i + 2] = rgb[2];
      img.rgba[i + 3] = 255;
    }
  }
}

/// Letters `text` (upper-cased; unknown characters as `?`) with its top left at (x, y),
/// each font pixel `s` image pixels; returns the width drawn.
function text(img, x, y, s, rgb, str, maxW = Infinity) {
  let cx = x;
  for (const ch of String(str).toUpperCase()) {
    if (cx + 5 * s - x > maxW) break;
    const g = FONT[ch] ?? FONT["?"];
    for (let r = 0; r < 7; r++) {
      for (let c = 0; c < 5; c++) {
        if (g[r] & (16 >> c)) fill(img, cx + c * s, y + r * s, s, s, rgb);
      }
    }
    cx += 6 * s;
  }
  return cx - x;
}

/// `src` shrunk by the whole factor `f` (each output pixel the mean of an f×f block) and
/// copied with its top left at (x, y).
function blit(img, src, x, y, f) {
  const w = Math.floor(src.width / f);
  const h = Math.floor(src.height / f);
  for (let ty = 0; ty < h; ty++) {
    for (let tx = 0; tx < w; tx++) {
      const sum = [0, 0, 0];
      for (let dy = 0; dy < f; dy++) {
        for (let dx = 0; dx < f; dx++) {
          const i = ((ty * f + dy) * src.width + tx * f + dx) * 4;
          sum[0] += src.rgba[i];
          sum[1] += src.rgba[i + 1];
          sum[2] += src.rgba[i + 2];
        }
      }
      const o = ((y + ty) * img.width + x + tx) * 4;
      img.rgba[o] = Math.round(sum[0] / (f * f));
      img.rgba[o + 1] = Math.round(sum[1] / (f * f));
      img.rgba[o + 2] = Math.round(sum[2] / (f * f));
      img.rgba[o + 3] = 255;
    }
  }
}

export function composeSheet({ title, columns, rows, tiles, captureWidth, captureHeight, tileWidth = 480 }) {
  const f = Math.max(1, Math.round(captureWidth / tileWidth));
  const tw = Math.floor(captureWidth / f);
  const th = Math.floor(captureHeight / f);
  const left = 130;
  const titleH = 44;
  const headH = 64;
  const width = left + columns.length * (tw + GAP) + GAP;
  const height = titleH + headH + rows.length * (th + GAP) + GAP;
  const img = { width, height, rgba: new Uint8Array(width * height * 4) };
  fill(img, 0, 0, width, height, BG);
  text(img, 12, 12, 3, INK, title, width - 24);
  columns.forEach((c, k) => {
    const x = left + GAP + k * (tw + GAP);
    const w = text(img, x, titleH + 6, 5, INK, c.letter);
    text(img, x + w + 12, titleH + 20, 2, DIM, c.name, tw - w - 12);
  });
  rows.forEach((r, j) => {
    const y = titleH + headH + GAP + j * (th + GAP);
    text(img, 12, y + 8, 2, INK, r.time);
    text(img, 12, y + 30, 2, DIM, r.camera);
    columns.forEach((_, k) => {
      const x = left + GAP + k * (tw + GAP);
      const src = tiles(j, k);
      if (src) blit(img, src, x, y, f);
      else {
        fill(img, x, y, tw, th, CELL);
        text(img, x + 12, y + 12, 2, DIM, "missing");
      }
    });
  });
  return img;
}
