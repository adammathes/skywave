//! A small PNG decoder for the game's own pictures (the World page's stills): 8-bit
//! grayscale, RGB, RGBA and palette images, not interlaced, every row filter, zlib
//! (stored, fixed and dynamic Huffman blocks). Checksums are not verified: the files are
//! built in and their tests decode them.

/// A decoded image: `w × h` RGBA8 texels, row-major from the top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Decodes a PNG file (see the module docs for what it takes).
pub fn decode(data: &[u8]) -> Result<Image, String> {
    const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if data.get(..8) != Some(&SIGNATURE[..]) {
        return Err("not a PNG".into());
    }
    let mut at = 8;
    let (mut w, mut h, mut kind) = (0u32, 0u32, 0u8);
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut idat = Vec::new();
    loop {
        let be32 = |p: usize| -> Result<u32, String> {
            let b = data.get(p..p + 4).ok_or("truncated")?;
            Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        let len = be32(at)? as usize;
        let name = data.get(at + 4..at + 8).ok_or("truncated")?;
        let body = data.get(at + 8..at + 8 + len).ok_or("truncated")?;
        match name {
            b"IHDR" => {
                if len != 13 {
                    return Err("bad IHDR".into());
                }
                (w, h, kind) = (be32(at + 8)?, be32(at + 12)?, body[9]);
                if body[8] != 8 || body[12] != 0 || !matches!(kind, 0 | 2 | 3 | 6) {
                    return Err("only 8-bit, non-interlaced gray, RGB, RGBA or palette".into());
                }
                if w == 0 || h == 0 || w > 4096 || h > 4096 {
                    return Err("bad size".into());
                }
            }
            b"PLTE" => {
                palette = body
                    .chunks_exact(3)
                    .map(|c| [c[0], c[1], c[2], 255])
                    .collect();
            }
            b"tRNS" if kind == 3 => {
                for (p, &a) in palette.iter_mut().zip(body) {
                    p[3] = a;
                }
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    let channels = match kind {
        0 | 3 => 1,
        2 => 3,
        _ => 4,
    };
    let stride = w as usize * channels;
    let raw = inflate(idat.get(2..).ok_or("no image data")?)?;
    if raw.len() < (stride + 1) * h as usize {
        return Err("short image data".into());
    }
    let mut px = vec![0u8; stride * h as usize];
    for y in 0..h as usize {
        let filter = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        let (done, row) = px.split_at_mut(y * stride);
        let row = &mut row[..stride];
        let up = if y > 0 {
            &done[(y - 1) * stride..]
        } else {
            &[][..]
        };
        for i in 0..stride {
            let a = if i >= channels { row[i - channels] } else { 0 };
            let b = up.get(i).copied().unwrap_or(0);
            let c = if i >= channels {
                up.get(i - channels).copied().unwrap_or(0)
            } else {
                0
            };
            let pred = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err("bad row filter".into()),
            };
            row[i] = line[i].wrapping_add(pred);
        }
    }
    let rgba = match kind {
        0 => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        2 => px
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        3 => px
            .iter()
            .map(|&i| {
                palette
                    .get(usize::from(i))
                    .copied()
                    .ok_or("bad palette index")
            })
            .collect::<Result<Vec<[u8; 4]>, _>>()?
            .concat(),
        _ => px,
    };
    Ok(Image { w, h, rgba })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Reads a deflate stream least significant bit first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    buf: u32,
    n: u32,
}

impl Bits<'_> {
    fn get(&mut self, count: u32) -> Result<u32, String> {
        while self.n < count {
            let byte = *self.data.get(self.at).ok_or("truncated deflate stream")?;
            self.buf |= u32::from(byte) << self.n;
            self.at += 1;
            self.n += 8;
        }
        let v = self.buf & ((1u32 << count) - 1);
        self.buf = self.buf.checked_shr(count).unwrap_or(0);
        self.n -= count;
        Ok(v)
    }
}

/// A canonical Huffman code: codes per length and the symbols in code order.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Huffman {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[usize::from(l)] += 1;
        }
        counts[0] = 0;
        let mut offs = [0u16; 16];
        for l in 1..16 {
            offs[l] = offs[l - 1] + counts[l - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l > 0 {
                symbols[usize::from(offs[usize::from(l)])] = s as u16;
                offs[usize::from(l)] += 1;
            }
        }
        Huffman { counts, symbols }
    }

    fn decode(&self, bits: &mut Bits) -> Result<u16, String> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= bits.get(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                return self
                    .symbols
                    .get((index + code - first) as usize)
                    .copied()
                    .ok_or_else(|| "bad code".into());
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err("bad code".into())
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order code length code lengths come in.
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Inflates a raw deflate stream (RFC 1951).
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut bits = Bits {
        data,
        at: 0,
        buf: 0,
        n: 0,
    };
    let mut out = Vec::new();
    loop {
        let last = bits.get(1)? == 1;
        match bits.get(2)? {
            0 => {
                // Stored: from the next byte boundary.
                bits.buf = 0;
                bits.n = 0;
                let at = bits.at;
                let head = data.get(at..at + 4).ok_or("truncated stored block")?;
                let len = usize::from(u16::from_le_bytes([head[0], head[1]]));
                let body = data
                    .get(at + 4..at + 4 + len)
                    .ok_or("truncated stored block")?;
                out.extend_from_slice(body);
                bits.at = at + 4 + len;
            }
            1 => {
                let mut lengths = [0u8; 288];
                for (s, l) in lengths.iter_mut().enumerate() {
                    *l = match s {
                        0..=143 => 8,
                        144..=255 => 9,
                        256..=279 => 7,
                        _ => 8,
                    };
                }
                let lit = Huffman::new(&lengths);
                let dist = Huffman::new(&[5; 30]);
                codes(&mut bits, &lit, &dist, &mut out)?;
            }
            2 => {
                let nlen = bits.get(5)? as usize + 257;
                let ndist = bits.get(5)? as usize + 1;
                let ncode = bits.get(4)? as usize + 4;
                let mut clens = [0u8; 19];
                for &k in &CLEN_ORDER[..ncode] {
                    clens[k] = bits.get(3)? as u8;
                }
                let clen = Huffman::new(&clens);
                let mut lengths = vec![0u8; nlen + ndist];
                let mut i = 0;
                while i < nlen + ndist {
                    let sym = clen.decode(&mut bits)?;
                    let (value, repeat) = match sym {
                        0..=15 => (sym as u8, 1),
                        16 => {
                            let prev = *lengths[..i].last().ok_or("repeat with no length")?;
                            (prev, 3 + bits.get(2)?)
                        }
                        17 => (0, 3 + bits.get(3)?),
                        _ => (0, 11 + bits.get(7)?),
                    };
                    for _ in 0..repeat {
                        *lengths.get_mut(i).ok_or("too many lengths")? = value;
                        i += 1;
                    }
                }
                let lit = Huffman::new(&lengths[..nlen]);
                let dist = Huffman::new(&lengths[nlen..]);
                codes(&mut bits, &lit, &dist, &mut out)?;
            }
            _ => return Err("bad block type".into()),
        }
        if last {
            return Ok(out);
        }
    }
}

/// One Huffman-coded block's literals and back-references, to the end-of-block code.
fn codes(bits: &mut Bits, lit: &Huffman, dist: &Huffman, out: &mut Vec<u8>) -> Result<(), String> {
    loop {
        let sym = usize::from(lit.decode(bits)?);
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            _ => {
                let k = sym - 257;
                let base = *LEN_BASE.get(k).ok_or("bad length code")?;
                let len = usize::from(base) + bits.get(u32::from(LEN_EXTRA[k]))? as usize;
                let d = usize::from(dist.decode(bits)?);
                let base = *DIST_BASE.get(d).ok_or("bad distance code")?;
                let back = usize::from(base) + bits.get(u32::from(DIST_EXTRA[d]))? as usize;
                let start = out.len().checked_sub(back).ok_or("distance too far back")?;
                for j in 0..len {
                    out.push(out[start + j]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG of `rows` (each a filter byte and its bytes) in one stored deflate block.
    fn stored_png(w: u32, h: u32, kind: u8, extra: &[(&[u8; 4], &[u8])], rows: &[u8]) -> Vec<u8> {
        let mut f = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let mut chunk = |name: &[u8; 4], body: &[u8]| {
            f.extend_from_slice(&(body.len() as u32).to_be_bytes());
            f.extend_from_slice(name);
            f.extend_from_slice(body);
            f.extend_from_slice(&[0; 4]);
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, kind, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        for (name, body) in extra {
            chunk(name, body);
        }
        let mut z = vec![0x78, 0x01, 1];
        z.extend_from_slice(&(rows.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(rows.len() as u16)).to_le_bytes());
        z.extend_from_slice(rows);
        z.extend_from_slice(&[0; 4]);
        chunk(b"IDAT", &z);
        chunk(b"IEND", &[]);
        f
    }

    #[test]
    fn decodes_every_filter_and_kind() {
        // 2×3 RGB: rows with filters None, Sub, Up; then Average and Paeth on RGBA.
        let rows = [
            0, 10, 20, 30, 40, 50, 60, //
            1, 1, 2, 3, 4, 4, 4, //
            2, 1, 1, 1, 1, 1, 1,
        ];
        let img = decode(&stored_png(2, 3, 2, &[], &rows)).unwrap();
        assert_eq!((img.w, img.h), (2, 3));
        assert_eq!(
            img.rgba,
            [
                10, 20, 30, 255, 40, 50, 60, 255, //
                1, 2, 3, 255, 5, 6, 7, 255, //
                2, 3, 4, 255, 6, 7, 8, 255
            ]
        );
        let rows = [
            0, 100, 100, 100, 255, //
            3, 50, 50, 50, 0, //
            4, 0, 0, 0, 0,
        ];
        let img = decode(&stored_png(1, 3, 6, &[], &rows)).unwrap();
        assert_eq!(img.rgba[4..8], [100, 100, 100, 127]);
        assert_eq!(img.rgba[8..12], [100, 100, 100, 127], "Paeth picks up");
        // A palette with transparency, and gray.
        let plte: (&[u8; 4], &[u8]) = (b"PLTE", &[1, 2, 3, 4, 5, 6]);
        let trns: (&[u8; 4], &[u8]) = (b"tRNS", &[9]);
        let img = decode(&stored_png(2, 1, 3, &[plte, trns], &[0, 1, 0])).unwrap();
        assert_eq!(img.rgba, [4, 5, 6, 255, 1, 2, 3, 9]);
        let img = decode(&stored_png(1, 1, 0, &[], &[0, 77])).unwrap();
        assert_eq!(img.rgba, [77, 77, 77, 255]);
        // Refusals.
        assert!(decode(b"GIF89a").is_err());
        assert!(decode(&stored_png(2, 1, 3, &[plte], &[0, 7, 0])).is_err());
        assert!(decode(&stored_png(1, 1, 2, &[], &[5, 0, 0, 0])).is_err());
        let full = stored_png(1, 1, 2, &[], &[0, 1, 2, 3]);
        assert!(decode(&full[..full.len() - 20]).is_err());
    }

    #[test]
    fn inflates_fixed_and_dynamic_blocks() {
        // "abcabcabcabc" with a fixed-Huffman back-reference (zlib, level 9, Z_FIXED).
        let fixed = [0x4b, 0x4c, 0x4a, 0x4e, 0x84, 0x21, 0x00];
        assert_eq!(inflate(&fixed).unwrap(), b"abcabcabcabc");
        // A dynamic block (zlib, level 9).
        let dynamic = [
            0x15, 0x88, 0x41, 0x11, 0x00, 0x00, 0x08, 0x83, 0xb2, 0xc2, 0x66, 0xff, 0x0a, 0xea,
            0x07, 0x38, 0x50, 0xd0, 0x88, 0xcc, 0x83, 0xe8, 0xdc, 0xcc, 0x15, 0xaf, 0x14, 0xea,
            0x02,
        ];
        assert_eq!(
            inflate(&dynamic).unwrap(),
            b"abbaabbcbabaebabaacbbebbacaacabacacdaadb".as_slice()
        );
        assert!(inflate(&[0x07]).is_err(), "block type 3");
        assert!(inflate(&fixed[..3]).is_err());
    }
}
