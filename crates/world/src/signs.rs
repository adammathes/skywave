//! Lettering for signs: invented business and street names from a syllable generator
//! (never a real brand, street or station; see `CLAUDE.md`) and a 5-pixel-tall block font
//! that turns a name into a few rectangles per letter.
//!
//! Everything is a pure function of its seed.

use alloc::vec::Vec;

use skywave_core::mix64;

/// First syllables of invented names.
const HEADS: [&str; 25] = [
    "BAR", "BEL", "CAR", "DAL", "DOM", "FEN", "GAR", "HAL", "JOR", "KIP", "LAN", "LOR", "MEL",
    "ZAN", "NOR", "PAR", "QUIN", "RAL", "REN", "SOM", "TAM", "TIV", "VAR", "VEL", "WEN",
];
/// Second syllables.
const TAILS: [&str; 12] = [
    "BA", "DEN", "LA", "LEY", "LO", "NA", "NEY", "RA", "RO", "SA", "VA", "WIN",
];
/// What a shop sells, in its sign's words.
const TRADES: [&str; 32] = [
    "DINER",
    "CAFE",
    "TACOS",
    "DONUTS",
    "PIZZA",
    "LIQUOR",
    "RECORDS",
    "SURF SHOP",
    "SURF",
    "VIDEO",
    "ARCADE",
    "SHOES",
    "DRUGS",
    "CLEANERS",
    "MARKET",
    "BAR",
    "LOUNGE",
    "TV",
    "RADIO",
    "BIKES",
    "BOOKS",
    "DELI",
    "BAKERY",
    "HAIR",
    "TOYS",
    "GIFTS",
    "CAMERA",
    "AUTO",
    "TIRES",
    "GRILL",
    "BURGERS",
    "COCKTAILS",
];
/// Street name endings.
const STREET_TYPES: [&str; 7] = ["ST", "AVE", "DR", "WAY", "LN", "PL", "BLVD"];

fn pick<'a>(items: &[&'a str], h: u64) -> &'a str {
    items[(h % items.len() as u64) as usize]
}

/// An invented given-name-like word: two syllables.
fn word(seed: u64) -> Vec<u8> {
    let h = mix64(seed ^ 0x4E41_4D45);
    let mut w = Vec::with_capacity(8);
    w.extend_from_slice(pick(&HEADS, h).as_bytes());
    w.extend_from_slice(pick(&TAILS, h >> 20).as_bytes());
    w
}

/// An invented shop name of at most `max_len` characters (uppercase ASCII, spaces and
/// apostrophes): "NAME'S TRADE", "NAME TRADE", "NAME'S" or the trade alone, the longest
/// form that fits. Empty when not even the shortest fits.
pub fn business_name(seed: u64, max_len: usize) -> Vec<u8> {
    let h = mix64(seed ^ 0x5348_4F50);
    let name = word(seed);
    let trade = pick(&TRADES, h >> 8).as_bytes();
    let mut forms: [Vec<u8>; 4] = Default::default();
    // Possessive with the trade, the plain pairing, the possessive alone, the trade alone.
    forms[0].extend_from_slice(&name);
    forms[0].extend_from_slice(b"'S ");
    forms[0].extend_from_slice(trade);
    forms[1].extend_from_slice(&name);
    forms[1].push(b' ');
    forms[1].extend_from_slice(trade);
    forms[2].extend_from_slice(&name);
    forms[2].extend_from_slice(b"'S");
    forms[3].extend_from_slice(trade);
    // Half the shops prefer the plain pairing over the possessive.
    let order: [usize; 4] = if h & 1 == 0 {
        [0, 1, 2, 3]
    } else {
        [1, 0, 3, 2]
    };
    for k in order {
        if forms[k].len() <= max_len {
            return core::mem::take(&mut forms[k]);
        }
    }
    Vec::new()
}

/// One word of at most `max_len` letters for a vertical sign: the shop's trade when it is
/// one short word, else an invented name, else the shortest trades.
pub fn blade_word(seed: u64, max_len: usize) -> Vec<u8> {
    let h = mix64(seed ^ 0x424C_4144);
    for k in 0..TRADES.len() as u64 {
        let t = pick(&TRADES, h.wrapping_add(k));
        if t.len() <= max_len && !t.contains(' ') {
            if k > 0 && h & 2 == 0 {
                let w = word(seed);
                if w.len() <= max_len {
                    return w;
                }
            }
            return t.as_bytes().to_vec();
        }
    }
    b"BAR".to_vec()
}

/// An invented street name: "NAME TYPE".
pub fn street_name(seed: u64) -> Vec<u8> {
    let mut n = word(seed ^ 0x5354_5254);
    n.push(b' ');
    n.extend_from_slice(pick(&STREET_TYPES, mix64(seed) >> 12).as_bytes());
    n
}

/// Rows of glyph `c`, top first, bit `width - 1 - x` set where pixel `x` is lit, and its
/// width in pixels. Unknown characters are blank.
fn glyph(c: u8) -> ([u8; 5], u8) {
    let g = |rows: [&str; 5]| {
        let w = rows[0].len() as u8;
        let mut bits = [0u8; 5];
        for (b, r) in bits.iter_mut().zip(rows) {
            for (x, ch) in r.bytes().enumerate() {
                if ch == b'#' {
                    *b |= 1 << (w as usize - 1 - x);
                }
            }
        }
        (bits, w)
    };
    match c {
        b'A' => g([".#.", "#.#", "###", "#.#", "#.#"]),
        b'B' => g(["##.", "#.#", "##.", "#.#", "##."]),
        b'C' => g([".##", "#..", "#..", "#..", ".##"]),
        b'D' => g(["##.", "#.#", "#.#", "#.#", "##."]),
        b'E' => g(["###", "#..", "##.", "#..", "###"]),
        b'F' => g(["###", "#..", "##.", "#..", "#.."]),
        b'G' => g([".##", "#..", "#.#", "#.#", ".##"]),
        b'H' => g(["#.#", "#.#", "###", "#.#", "#.#"]),
        b'I' => g(["###", ".#.", ".#.", ".#.", "###"]),
        b'J' => g(["..#", "..#", "..#", "#.#", ".#."]),
        b'K' => g(["#.#", "#.#", "##.", "#.#", "#.#"]),
        b'L' => g(["#..", "#..", "#..", "#..", "###"]),
        b'M' => g(["#...#", "##.##", "#.#.#", "#...#", "#...#"]),
        b'N' => g(["#..#", "##.#", "#.##", "#..#", "#..#"]),
        b'O' => g([".#.", "#.#", "#.#", "#.#", ".#."]),
        b'P' => g(["##.", "#.#", "##.", "#..", "#.."]),
        b'Q' => g([".#.", "#.#", "#.#", "##.", ".##"]),
        b'R' => g(["##.", "#.#", "##.", "#.#", "#.#"]),
        b'S' => g([".##", "#..", ".#.", "..#", "##."]),
        b'T' => g(["###", ".#.", ".#.", ".#.", ".#."]),
        b'U' => g(["#.#", "#.#", "#.#", "#.#", "###"]),
        b'V' => g(["#.#", "#.#", "#.#", "#.#", ".#."]),
        b'W' => g(["#...#", "#...#", "#.#.#", "##.##", "#...#"]),
        b'X' => g(["#.#", "#.#", ".#.", "#.#", "#.#"]),
        b'Y' => g(["#.#", "#.#", ".#.", ".#.", ".#."]),
        b'Z' => g(["###", "..#", ".#.", "#..", "###"]),
        b'\'' => g(["#", "#", ".", ".", "."]),
        _ => ([0; 5], 2),
    }
}

/// Width of `text` in font pixels, one pixel between letters.
pub fn text_width(text: &[u8]) -> u32 {
    let w: u32 = text.iter().map(|&c| u32::from(glyph(c).1) + 1).sum();
    w.saturating_sub(1)
}

/// Calls `f(x0, y0, x1, y1)` for the lit rectangles of `text` in font pixels (`x` from the
/// left edge, `y` up from the baseline, 5 pixels tall): each glyph's row runs, merged down
/// the rows where a run repeats.
pub fn text_rects(text: &[u8], mut f: impl FnMut(f32, f32, f32, f32)) {
    let mut x = 0u32;
    for &c in text {
        let (rows, w) = glyph(c);
        // Runs per row: (start, end) exclusive, at most 3 a row.
        let mut runs: [[(u8, u8); 3]; 5] = [[(0, 0); 3]; 5];
        for (r, bits) in rows.iter().enumerate() {
            let mut k = 0;
            let mut px = 0u8;
            while px < w {
                if bits & (1 << (w - 1 - px)) != 0 {
                    let start = px;
                    while px < w && bits & (1 << (w - 1 - px)) != 0 {
                        px += 1;
                    }
                    if k < 3 {
                        runs[r][k] = (start, px);
                        k += 1;
                    }
                } else {
                    px += 1;
                }
            }
        }
        for r in 0..5 {
            for k in 0..3 {
                let run = runs[r][k];
                if run.1 == 0 {
                    continue;
                }
                // Extend down while the next rows repeat the run; clear them.
                let mut r1 = r + 1;
                while r1 < 5 {
                    match runs[r1].iter().position(|&q| q == run) {
                        Some(j) => {
                            runs[r1][j] = (0, 0);
                            r1 += 1;
                        }
                        None => break,
                    }
                }
                let x0 = (x + u32::from(run.0)) as f32;
                let x1 = (x + u32::from(run.1)) as f32;
                // Row `r` is `5 - r` pixels from the top's baseline.
                f(x0, (5 - r1) as f32, x1, (5 - r) as f32);
            }
        }
        x += u32::from(w) + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fit_and_vary() {
        let mut seen = alloc::collections::BTreeSet::new();
        for s in 0..200u64 {
            let n = business_name(s, 14);
            assert!(
                !n.is_empty() && n.len() <= 14,
                "{:?}",
                core::str::from_utf8(&n)
            );
            assert!(n
                .iter()
                .all(|c| c.is_ascii_uppercase() || *c == b' ' || *c == b'\''));
            seen.insert(n);
            let st = street_name(s);
            assert!(st.len() <= 12, "{:?}", core::str::from_utf8(&st));
        }
        assert!(seen.len() > 150, "{} distinct", seen.len());
        assert!(business_name(1, 2).is_empty());
        assert_eq!(business_name(7, 40), business_name(7, 40));
    }

    #[test]
    fn letters_cover_their_pixels_once() {
        for c in b'A'..=b'Z' {
            let (rows, w) = glyph(c);
            let lit: u32 = rows.iter().map(|r| r.count_ones()).sum();
            let mut area = 0.0;
            let mut n = 0;
            text_rects(&[c], |x0, y0, x1, y1| {
                assert!(x0 >= 0.0 && x1 <= f32::from(w) && y0 >= 0.0 && y1 <= 5.0);
                area += (x1 - x0) * (y1 - y0);
                n += 1;
            });
            assert_eq!(area as u32, lit, "{}", c as char);
            assert!(n <= 7, "{} has {n} rects", c as char);
        }
        assert_eq!(text_width(b"AB"), 7);
    }
}
