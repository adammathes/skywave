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

/// First syllables of invented names (pruned of those that make real names; the tests
/// check every pairing against the brand blocklist).
const HEADS: [&str; 20] = [
    "BAR", "DAL", "DOM", "FEN", "HAL", "JOR", "KIP", "LAN", "MEL", "ZAN", "NOR", "PAR", "QUIN",
    "RAL", "REN", "SOM", "TIV", "VAR", "VEL", "WEN",
];
/// Second syllables.
const TAILS: [&str; 9] = ["DEN", "LA", "LO", "NA", "RA", "RO", "SA", "VA", "WIN"];
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

/// The test-only brand blocklist (`crates/world/testdata/brand_blocklist.txt`).
#[cfg(test)]
pub(crate) mod testing {
    use alloc::vec::Vec;

    /// FNV-1a over the letters and digits of `s`, uppercased: how the blocklist stores the
    /// names it keeps out.
    pub fn key(s: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &c in s {
            let c = c.to_ascii_uppercase();
            if c.is_ascii_alphanumeric() {
                h = (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        h
    }

    pub fn blocklist() -> Vec<u64> {
        include_str!("../../testdata/brand_blocklist.txt")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| u64::from_str_radix(l.trim_start_matches("0x"), 16).expect("hex"))
            .collect()
    }

    /// Prints whether each comma-separated entry of `BLOCKLIST_CHECK` is listed, and its
    /// key, so a reviewer can audit a word or a pair without reading the list:
    /// `BLOCKLIST_CHECK="Golden Books" cargo test -p world --lib -- --ignored
    /// blocklist_check --nocapture`.
    #[test]
    #[ignore]
    fn blocklist_check() {
        let list = blocklist();
        let words = std::env::var("BLOCKLIST_CHECK").unwrap_or_default();
        for w in words.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            let k = key(w.as_bytes());
            let on = if list.contains(&k) {
                "LISTED"
            } else {
                "not listed"
            };
            std::println!("{w}: {on} ({k:#018x})");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{blocklist, key};
    use super::*;

    /// The sign bands' and billboards' syllable names, alone, possessive and with every
    /// trade, and the street names: none spells a listed brand.
    #[test]
    fn syllable_names_are_not_brands() {
        let block = blocklist();
        for h in HEADS {
            for t in TAILS {
                let name = [h, t].concat();
                let mut forms = alloc::vec![name.clone(), [name.as_str(), "S"].concat()];
                for trade in TRADES {
                    forms.push([name.as_str(), "S ", trade].concat());
                    forms.push([name.as_str(), " ", trade].concat());
                }
                for st in STREET_TYPES {
                    forms.push([name.as_str(), " ", st].concat());
                }
                for f in forms {
                    assert!(!block.contains(&key(f.as_bytes())), "{f}");
                }
            }
        }
        for trade in TRADES {
            assert!(!block.contains(&key(trade.as_bytes())), "{trade}");
        }
    }

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
