// A tiny PNG reader and writer on Node's zlib, for the screenshot tools (no npm packages).
//
// readPng(buffer) -> { width, height, rgba }: 8-bit grey, grey+alpha, RGB, RGBA or
// palette images, not interlaced; `rgba` is a Uint8Array of width*height*4 bytes.
// writePng(width, height, rgba) -> Buffer: an 8-bit RGBA PNG.

import { deflateSync, inflateSync } from "node:zlib";

const SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(bytes) {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/// Samples per pixel for each PNG color type.
const CHANNELS = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };

export function readPng(buf) {
  if (buf.length < 8 || !buf.subarray(0, 8).equals(SIGNATURE)) throw new Error("not a PNG");
  let pos = 8;
  let header = null;
  let palette = null;
  let transparency = null;
  const idat = [];
  while (pos + 8 <= buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("latin1", pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    pos += 12 + len;
    if (type === "IHDR") {
      header = {
        width: data.readUInt32BE(0),
        height: data.readUInt32BE(4),
        depth: data[8],
        colorType: data[9],
        interlace: data[12],
      };
    } else if (type === "PLTE") {
      palette = data;
    } else if (type === "tRNS") {
      transparency = data;
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
  }
  if (!header) throw new Error("PNG has no IHDR");
  const { width, height, depth, colorType, interlace } = header;
  const channels = CHANNELS[colorType];
  if (depth !== 8 || !channels || interlace !== 0) {
    throw new Error(`unsupported PNG: depth ${depth}, color type ${colorType}, interlace ${interlace}`);
  }
  if (colorType === 3 && !palette) throw new Error("palette PNG without PLTE");
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  if (raw.length < height * (stride + 1)) throw new Error("PNG data is short");

  // Undo the per-row filters in place into `px`.
  const px = new Uint8Array(height * stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const src = y * (stride + 1) + 1;
    const row = y * stride;
    const up = row - stride;
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? px[row + i - channels] : 0;
      const b = y > 0 ? px[up + i] : 0;
      const c = y > 0 && i >= channels ? px[up + i - channels] : 0;
      let v = raw[src + i];
      switch (filter) {
        case 0:
          break;
        case 1:
          v += a;
          break;
        case 2:
          v += b;
          break;
        case 3:
          v += (a + b) >> 1;
          break;
        case 4: {
          const p = a + b - c;
          const pa = Math.abs(p - a);
          const pb = Math.abs(p - b);
          const pc = Math.abs(p - c);
          v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
          break;
        }
        default:
          throw new Error(`bad PNG row filter ${filter}`);
      }
      px[row + i] = v & 0xff;
    }
  }

  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0, n = width * height; i < n; i++) {
    const s = i * channels;
    const d = i * 4;
    switch (colorType) {
      case 0:
        rgba[d] = rgba[d + 1] = rgba[d + 2] = px[s];
        rgba[d + 3] = 255;
        break;
      case 4:
        rgba[d] = rgba[d + 1] = rgba[d + 2] = px[s];
        rgba[d + 3] = px[s + 1];
        break;
      case 2:
        rgba[d] = px[s];
        rgba[d + 1] = px[s + 1];
        rgba[d + 2] = px[s + 2];
        rgba[d + 3] = 255;
        break;
      case 6:
        rgba.set(px.subarray(s, s + 4), d);
        break;
      case 3: {
        const k = px[s];
        rgba[d] = palette[k * 3];
        rgba[d + 1] = palette[k * 3 + 1];
        rgba[d + 2] = palette[k * 3 + 2];
        rgba[d + 3] = transparency && k < transparency.length ? transparency[k] : 255;
        break;
      }
    }
  }
  return { width, height, rgba };
}

function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "latin1");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}

export function writePng(width, height, rgba) {
  if (rgba.length !== width * height * 4) throw new Error("writePng: wrong pixel count");
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const stride = width * 4;
  const raw = Buffer.alloc(height * (stride + 1));
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0; // no filter
    raw.set(rgba.subarray(y * stride, (y + 1) * stride), y * (stride + 1) + 1);
  }
  return Buffer.concat([
    SIGNATURE,
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
