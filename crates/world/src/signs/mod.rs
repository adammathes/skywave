//! Signs: pictograms ([`pictogram`]), stroke letterforms ([`letters`]) and the closed
//! lexicon shop signs draw their words from ([`lexicon`]); and the invented street names
//! and the syllable names the sign bands and billboards still letter (never a real brand,
//! street or station; see `CLAUDE.md`).
//!
//! Everything is a pure function of its seed.

pub mod letters;
pub mod lexicon;
pub mod pictogram;

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
const TRADES: [&str; 31] = [
    "DINER",
    "CAFE",
    "TACOS",
    "DONUTS",
    "PIZZA",
    "LIQUOR",
    "RECORDS",
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

/// An invented street name: "NAME TYPE".
pub fn street_name(seed: u64) -> Vec<u8> {
    let mut n = word(seed ^ 0x5354_5254);
    n.push(b' ');
    n.extend_from_slice(pick(&STREET_TYPES, mix64(seed) >> 12).as_bytes());
    n
}

/// Width of `text` in block capitals five units tall, rounded up (a unit is a fifth of the
/// cap height; the width a sign makes room for).
pub fn text_width(text: &[u8]) -> u32 {
    let w = letters::layout(text, letters::LetterStyle::Block).ink_w() * 5.0;
    let r = w as u32;
    if (r as f32) < w {
        r + 1
    } else {
        r
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
    fn text_width_covers_the_ink() {
        for text in [&b"AB"[..], b"KESSICK'S", b"M W"] {
            let ink = letters::layout(text, letters::LetterStyle::Block).ink_w() * 5.0;
            let w = text_width(text) as f32;
            assert!(w >= ink && w < ink + 1.0, "{w} {ink}");
        }
        // At most four units a letter, as the sign bands make room for.
        let w = text_width(b"ABCDEFGHKMNOPRSTUVWXYZ");
        assert!((50..=88).contains(&w), "{w}");
    }
}
