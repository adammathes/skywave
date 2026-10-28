//! The words a shop sign may carry: a closed, reviewed [`Lexicon`]
//! (`assets/grammar/lexicon.ron`) of invented names, generic descriptors, trade words and
//! secondary-line words, combined by rule ([`Form`]) into a [`ShopSign`]. Nothing is made
//! up free-form, so nothing can spell a real brand that the lexicon's reviewers did not
//! let in (`docs/backlog/BUILDINGS.md`, "Signage").
//!
//! Which trade a shop is, which pictogram and lettering it gets and how often it carries a
//! second line are the grammar's ([`crate::grammar::SignRules`]); [`assign`] draws a sign
//! for each shop and keeps a name or a wordmark from repeating within
//! `SignRules::name_radius`.

use alloc::string::String;
use alloc::vec::Vec;

use place::Era;
use skywave_core::{hash64, mix64};

use crate::grammar::{Line, SignRules};
use crate::math::V2;
use crate::signs::letters::LetterStyle;
use crate::signs::pictogram::Pictogram;

/// The reviewed words.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Lexicon {
    /// What a grammar's `SignRules::lexicon` names it by.
    pub id: String,
    /// The phonotactic tables the names were built from: every name is two or three
    /// syllables `onset? nucleus coda?` from these.
    pub syllables: Syllables,
    /// Invented given names.
    pub given: Vec<String>,
    /// Invented surnames.
    pub surnames: Vec<String>,
    /// Generic place and quality words: `SUNSET`, `HARBOR`, `GOLDEN`.
    pub descriptors: Vec<String>,
    /// What a shop sells, as its sign says it: `DONUTS`, `TIRES`, `ICE CREAM`.
    pub trades: Vec<String>,
    /// Words of a secondary line: `OPEN`, `VACANCY`, and `SINCE` for "SINCE 1962".
    pub lines: Vec<String>,
}

/// Name-building tables.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Syllables {
    pub onsets: Vec<String>,
    pub nuclei: Vec<String>,
    pub codas: Vec<String>,
}

/// Longest name, in letters (a blade sign stacks one).
pub const NAME_MAX: usize = 7;
/// Longest trade or descriptor, in characters.
pub const WORD_MAX: usize = 10;

/// Why [`Lexicon::validate`] refused a lexicon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LexiconError {
    EmptyId,
    /// A list is empty.
    Empty(&'static str),
    /// A word has characters other than `A`-`Z` (and single spaces in trades), or is too
    /// long or short.
    BadWord(String),
    /// A name is not built from the syllable tables.
    NotPhonotactic(String),
    /// A word is listed twice.
    Duplicate(String),
    /// `lines` lacks `SINCE`.
    NoSince,
}

impl core::fmt::Display for LexiconError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LexiconError::EmptyId => write!(f, "id is empty"),
            LexiconError::Empty(list) => write!(f, "{list} is empty"),
            LexiconError::BadWord(w) => write!(f, "{w:?} is not a sign word"),
            LexiconError::NotPhonotactic(w) => {
                write!(f, "{w:?} is not built from the syllable tables")
            }
            LexiconError::Duplicate(w) => write!(f, "{w:?} is listed twice"),
            LexiconError::NoSince => write!(f, "lines must list SINCE"),
        }
    }
}

impl Lexicon {
    /// Given names, then surnames.
    pub fn name(&self, i: usize) -> &str {
        let g = self.given.len();
        if i < g {
            &self.given[i]
        } else {
            &self.surnames[i - g]
        }
    }

    pub fn names(&self) -> usize {
        self.given.len() + self.surnames.len()
    }

    /// Checks every word; drawing signs assumes a lexicon that passed.
    pub fn validate(&self) -> Result<(), LexiconError> {
        if self.id.trim().is_empty() {
            return Err(LexiconError::EmptyId);
        }
        let lists: [(&'static str, &[String], usize, bool); 5] = [
            ("given", &self.given, NAME_MAX, false),
            ("surnames", &self.surnames, NAME_MAX, false),
            ("descriptors", &self.descriptors, WORD_MAX, false),
            ("trades", &self.trades, WORD_MAX, true),
            ("lines", &self.lines, WORD_MAX, false),
        ];
        let sy = &self.syllables;
        for (what, t) in [
            ("onsets", &sy.onsets),
            ("nuclei", &sy.nuclei),
            ("codas", &sy.codas),
        ] {
            if t.is_empty() {
                return Err(LexiconError::Empty(what));
            }
        }
        let mut seen: Vec<&str> = Vec::new();
        for (what, list, max, spaces) in lists {
            if list.is_empty() {
                return Err(LexiconError::Empty(what));
            }
            for w in list {
                let ok_char = |c: u8| c.is_ascii_uppercase() || (spaces && c == b' ');
                let b = w.as_bytes();
                let shaped = (3..=max).contains(&b.len())
                    && b.iter().all(|&c| ok_char(c))
                    && b[0] != b' '
                    && b[b.len() - 1] != b' '
                    && !w.contains("  ");
                if !shaped {
                    return Err(LexiconError::BadWord(w.clone()));
                }
                if (what == "given" || what == "surnames") && !self.phonotactic(w) {
                    return Err(LexiconError::NotPhonotactic(w.clone()));
                }
                // A trade may also be a secondary line (`COCKTAILS`), nothing else twice.
                let dup =
                    seen.contains(&w.as_str()) && !(what == "lines" && self.trades.contains(w));
                if dup {
                    return Err(LexiconError::Duplicate(w.clone()));
                }
                seen.push(w);
            }
        }
        if !self.lines.iter().any(|l| l == "SINCE") {
            return Err(LexiconError::NoSince);
        }
        Ok(())
    }

    /// Whether `w` splits into two or three syllables `onset? nucleus coda?`.
    pub fn phonotactic(&self, w: &str) -> bool {
        fn syl(t: &Syllables, w: &[u8], left: u32) -> bool {
            if w.is_empty() {
                return left == 0;
            }
            if left == 0 {
                return false;
            }
            let pre = |list: &[String], w: &[u8]| -> Vec<usize> {
                let mut v: Vec<usize> = list
                    .iter()
                    .filter(|p| w.starts_with(p.as_bytes()))
                    .map(|p| p.len())
                    .collect();
                v.push(0);
                v
            };
            for o in pre(&t.onsets, w) {
                let w1 = &w[o..];
                for n in pre(&t.nuclei, w1).into_iter().filter(|&n| n > 0) {
                    let w2 = &w1[n..];
                    for c in pre(&t.codas, w2) {
                        if syl(t, &w2[c..], left - 1) {
                            return true;
                        }
                    }
                }
            }
            false
        }
        let w = w.as_bytes();
        syl(&self.syllables, w, 2) || syl(&self.syllables, w, 3)
    }
}

/// How the words of a wordmark combine.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Form {
    /// `KESSICK'S`
    Possessive = 0,
    /// `KESSICK'S TACOS`
    PossessiveTrade = 1,
    /// `SUNSET TACOS`
    DescriptorTrade = 2,
    /// `TACOS`
    Trade = 3,
}

/// A secondary line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Second {
    /// A word of `Lexicon::lines`.
    Word(u16),
    /// `SINCE` and a year before the era.
    Since(u16),
}

/// One shop's sign: what it says and how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShopSign {
    /// Index into `Lexicon::trades`.
    pub trade: u16,
    pub form: Form,
    /// Index into the names ([`Lexicon::name`]), for the possessive forms.
    pub name: u16,
    /// Index into `Lexicon::descriptors`.
    pub descriptor: u16,
    pub pictogram: Option<Pictogram>,
    pub style: LetterStyle,
    pub second: Option<Second>,
    /// Dark lettering on a lit box rather than neon on a dark one.
    pub lit_box: bool,
}

/// The first year of an era.
pub fn era_start(era: Era) -> u16 {
    1970 + 10 * era as u16
}

fn push_words(out: &mut Vec<u8>, w: &str) {
    if !out.is_empty() {
        out.push(b' ');
    }
    out.extend_from_slice(w.as_bytes());
}

impl ShopSign {
    /// Draws the sign of a shop from the hash `h`. `rules` and `lex` must have passed
    /// validation together ([`crate::grammar::Grammar::validate`]).
    pub fn draw(rules: &SignRules, lex: &Lexicon, era: Era, h: u64) -> ShopSign {
        let rule = pick(&rules.trades, |t| t.weight, h).expect("a validated grammar has trades");
        let trade = lex
            .trades
            .iter()
            .position(|t| *t == rule.trade)
            .unwrap_or(0) as u16;
        let mut form = *pick(&rules.forms, |f| f.1, h >> 8).map_or(&Form::Trade, |f| &f.0);
        let name = ((h >> 16) % lex.names().max(1) as u64) as u16;
        let n = lex.descriptors.len().max(1);
        let mut descriptor = ((h >> 28) % n as u64) as usize;
        // Not one the trade leaves out (validation keeps at least one).
        while rule.descriptors_out.contains(&lex.descriptors[descriptor]) {
            descriptor = (descriptor + 1) % n;
        }
        let descriptor = descriptor as u16;
        // "SURF SURFBOARDS" says it twice: the descriptor gives way to a name.
        let d = &lex.descriptors[usize::from(descriptor)];
        if form == Form::DescriptorTrade && lex.trades[usize::from(trade)].starts_with(d.as_str()) {
            form = Form::PossessiveTrade;
        }
        let pictogram = pick(&rule.pictograms, |p| p.1, h >> 36).map(|p| p.0);
        let style = pick(&rules.wordmarks, |w| w.1, h >> 44).map_or(LetterStyle::Block, |w| w.0);
        let h2 = mix64(h);
        let second = if (h2 & 255) < u64::from(rules.line_256) {
            pick(&rule.lines, |l| l.1, h2 >> 8).and_then(|(line, _)| match line {
                Line::Since => {
                    let back = 2 + ((h2 >> 24) % 34) as u16;
                    Some(Second::Since(era_start(era) - back))
                }
                Line::Word(w) => {
                    // Never the trade again ("COCKTAILS / COCKTAILS").
                    let i = lex.lines.iter().position(|l| l == w)?;
                    (*w != lex.trades[usize::from(trade)]).then_some(Second::Word(i as u16))
                }
            })
        } else {
            None
        };
        ShopSign {
            trade,
            form,
            name,
            descriptor,
            pictogram,
            style,
            second,
            lit_box: ((h2 >> 40) & 255) < u64::from(rules.lit_box_256),
        }
    }

    /// Whether the main line carries a name.
    pub fn named(&self) -> bool {
        matches!(self.form, Form::Possessive | Form::PossessiveTrade)
    }

    /// The main line: `KESSICK'S TACOS`.
    pub fn text(&self, lex: &Lexicon) -> Vec<u8> {
        let mut out = Vec::with_capacity(20);
        let trade = &lex.trades[usize::from(self.trade)];
        match self.form {
            Form::Possessive | Form::PossessiveTrade => {
                out.extend_from_slice(lex.name(usize::from(self.name)).as_bytes());
                out.extend_from_slice(b"'S");
                if self.form == Form::PossessiveTrade {
                    push_words(&mut out, trade);
                }
            }
            Form::DescriptorTrade => {
                push_words(&mut out, &lex.descriptors[usize::from(self.descriptor)]);
                push_words(&mut out, trade);
            }
            Form::Trade => push_words(&mut out, trade),
        }
        out
    }

    /// One word for stacked letters: the name, the descriptor or the trade.
    pub fn word(&self, lex: &Lexicon) -> Vec<u8> {
        let w = match self.form {
            Form::Possessive | Form::PossessiveTrade => lex.name(usize::from(self.name)),
            Form::DescriptorTrade => &lex.descriptors[usize::from(self.descriptor)],
            Form::Trade => &lex.trades[usize::from(self.trade)],
        };
        w.as_bytes().to_vec()
    }

    /// The secondary line, if any: `OPEN`, `SINCE 1962`.
    pub fn second_text(&self, lex: &Lexicon) -> Option<Vec<u8>> {
        Some(match self.second? {
            Second::Word(i) => lex.lines[usize::from(i)].as_bytes().to_vec(),
            Second::Since(year) => {
                let mut out = b"SINCE ".to_vec();
                let mut y = year;
                let mut digits = [0u8; 4];
                for d in digits.iter_mut().rev() {
                    *d = b'0' + (y % 10) as u8;
                    y /= 10;
                }
                out.extend_from_slice(&digits);
                out
            }
        })
    }
}

/// The item `r` (any number; reduced modulo the total weight) picks from weighted `items`;
/// `None` when there are none.
fn pick<T>(items: &[T], weight: impl Fn(&T) -> u32, r: u64) -> Option<&T> {
    let total: u64 = items.iter().map(|t| u64::from(weight(t))).sum();
    let mut r = r % total.max(1);
    for t in items {
        let w = u64::from(weight(t));
        if r < w {
            return Some(t);
        }
        r -= w;
    }
    items.last()
}

/// Rng key for shop signs.
const KEY_SHOP_SIGN: u64 = 0x5348_5047_4E53;
/// Draws before a sign keeps a repeat.
const TRIES: u64 = 24;

/// The signs of shops at `at` (in order) for a place with detail seed `seed`: no two
/// within `rules.name_radius` share a main line, or a name. None when no lexicon is
/// attached (a grammar parsed without its loader). Deterministic.
pub fn assign(rules: &SignRules, lex: &Lexicon, era: Era, seed: u64, at: &[V2]) -> Vec<ShopSign> {
    if lex.trades.is_empty()
        || lex.names() == 0
        || lex.descriptors.is_empty()
        || rules.trades.is_empty()
    {
        return Vec::new();
    }
    let r2 = rules.name_radius * rules.name_radius;
    let mut out: Vec<ShopSign> = Vec::with_capacity(at.len());
    let mut texts: Vec<Vec<u8>> = Vec::with_capacity(at.len());
    for (i, &p) in at.iter().enumerate() {
        let mut best = None;
        for k in 0..TRIES {
            let s = ShopSign::draw(rules, lex, era, hash64(seed, &[KEY_SHOP_SIGN, i as u64, k]));
            let t = s.text(lex);
            let clash = out.iter().zip(&texts).zip(at).any(|((o, ot), &q)| {
                let d = q - p;
                d.dot(d) < r2 && (*ot == t || (o.named() && s.named() && o.name == s.name))
            });
            best = Some((s, t));
            if !clash {
                break;
            }
        }
        let (s, t) = best.expect("at least one try");
        out.push(s);
        texts.push(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::{basin_01, coast_80s};
    use crate::signs::testing::{blocklist, key};

    #[test]
    fn shipped_lexicon_validates() {
        shipped_lexicon_validates_in(coast_80s());
        shipped_lexicon_validates_in(basin_01());
    }

    #[test]
    fn no_word_is_a_known_brand() {
        no_word_is_a_known_brand_in(coast_80s());
        no_word_is_a_known_brand_in(basin_01());
    }

    #[test]
    fn no_descriptor_names_a_chain_with_its_trade() {
        // The coast leaves out three listed pairs; the basin leaves out ten
        // (their names are keys on the blocklist, never spelled here).
        assert_eq!(chains_left_out_in(coast_80s()), 3);
        assert_eq!(chains_left_out_in(basin_01()), 10);
    }

    #[test]
    fn no_sign_says_placeholder_words_or_repeats_itself() {
        no_sign_says_placeholder_words_or_repeats_itself_in(coast_80s());
        no_sign_says_placeholder_words_or_repeats_itself_in(basin_01());
    }

    #[test]
    fn every_weighted_choice_is_reached() {
        every_weighted_choice_is_reached_in(coast_80s());
        every_weighted_choice_is_reached_in(basin_01());
    }

    /// Every sign the coast grammar can draw, over enough hashes to meet every word.
    fn signs(g: crate::grammar::Grammar) -> (crate::grammar::Grammar, Vec<ShopSign>) {
        let v = (0..6000u64)
            .map(|h| ShopSign::draw(&g.signs, &g.lexicon, g.era, mix64(h)))
            .collect();
        (g, v)
    }

    fn shipped_lexicon_validates_in(g0: crate::grammar::Grammar) {
        let g = g0.clone();
        g.lexicon.validate().expect("lexicon");
        assert!(g.lexicon.phonotactic("HOLDREK"));
        assert!(!g.lexicon.phonotactic("XQZZT"));
        let mut bad = g.lexicon.clone();
        bad.surnames.push("KES7".into());
        assert!(bad.validate().is_err());
        let mut bad = g.lexicon.clone();
        bad.given.push(bad.surnames[0].clone());
        assert!(matches!(bad.validate(), Err(LexiconError::Duplicate(_))));
    }

    fn no_word_is_a_known_brand_in(g0: crate::grammar::Grammar) {
        let block = blocklist();
        assert!(block.len() >= 150, "{} entries", block.len());
        // The key folds case, spaces and punctuation.
        assert_eq!(key(b"Ab'c d"), key(b"ABCD"));
        let lex = g0.lexicon.clone();
        let all = lex
            .given
            .iter()
            .chain(&lex.surnames)
            .chain(&lex.descriptors)
            .chain(&lex.trades)
            .chain(&lex.lines);
        for w in all {
            assert!(
                !block.contains(&key(w.as_bytes())),
                "{w} is on the blocklist"
            );
            let mut s = w.as_bytes().to_vec();
            s.push(b'S');
            assert!(!block.contains(&key(&s)), "{w}S is on the blocklist");
        }
        let (g, signs) = signs(g0.clone());
        for s in &signs {
            let t = s.text(&g.lexicon);
            assert!(!block.contains(&key(&t)), "{:?}", core::str::from_utf8(&t));
            // And any two neighbouring words of it.
            let words: Vec<&[u8]> = t.split(|&c| c == b' ').collect();
            for pair in words.windows(2) {
                let mut j = pair[0].to_vec();
                j.extend_from_slice(pair[1]);
                assert!(!block.contains(&key(&j)), "{:?}", core::str::from_utf8(&t));
            }
            if let Some(l) = s.second_text(&g.lexicon) {
                let mut both = t.clone();
                both.extend_from_slice(&l);
                assert!(
                    !block.contains(&key(&both)),
                    "{:?}",
                    core::str::from_utf8(&both)
                );
            }
        }
    }

    /// Whether `t`, or `t` with the `S` of a plural trade dropped, is on the blocklist.
    fn listed(block: &[u64], t: &[u8]) -> bool {
        block.contains(&key(t))
            || t.strip_suffix(b"S")
                .is_some_and(|t| block.contains(&key(t)))
    }

    /// Every descriptor-and-trade pair of `g0` whose words, singular or plural, name a
    /// listed chain is left out of the trade's descriptors, and no sign drawn spells one;
    /// returns how many pairs that leaves out.
    fn chains_left_out_in(g0: crate::grammar::Grammar) -> usize {
        let block = blocklist();
        let mut out = 0;
        for t in &g0.signs.trades {
            for d in &g0.lexicon.descriptors {
                let pair = [d.as_bytes(), t.trade.as_bytes()].concat();
                if listed(&block, &pair) {
                    assert!(
                        t.descriptors_out.contains(d),
                        "{d} {} names a chain",
                        t.trade
                    );
                    out += 1;
                }
            }
        }
        let (g, signs) = signs(g0);
        for s in &signs {
            let t = s.text(&g.lexicon);
            assert!(!listed(&block, &t), "{:?}", core::str::from_utf8(&t));
        }
        out
    }

    fn no_sign_says_placeholder_words_or_repeats_itself_in(g0: crate::grammar::Grammar) {
        const PLACEHOLDERS: [&str; 10] = [
            "SHOP",
            "STORE",
            "NAME",
            "LOREM",
            "IPSUM",
            "TEXT",
            "SIGN",
            "BUSINESS",
            "TODO",
            "PLACEHOLDER",
        ];
        let (g, signs) = signs(g0.clone());
        let mut seen = alloc::collections::BTreeSet::new();
        for s in &signs {
            let t = s.text(&g.lexicon);
            let mut words: Vec<Vec<u8>> = t.split(|&c| c == b' ').map(|w| w.to_vec()).collect();
            if let Some(l) = s.second_text(&g.lexicon) {
                words.extend(l.split(|&c| c == b' ').map(|w| w.to_vec()));
            }
            for w in &words {
                let bare: Vec<u8> = w
                    .iter()
                    .copied()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect();
                for p in PLACEHOLDERS {
                    assert_ne!(bare, p.as_bytes(), "{:?}", core::str::from_utf8(&t));
                }
            }
            for (i, a) in words.iter().enumerate() {
                for b in &words[i + 1..] {
                    assert!(a != b, "{:?} repeats", core::str::from_utf8(&t));
                }
            }
            // Every word on the sign is from the lexicon (a possessive's name, a year).
            for w in &words {
                let base = w.strip_suffix(b"'S").unwrap_or(w);
                let lex = &g.lexicon;
                let known = (0..lex.names()).any(|i| lex.name(i).as_bytes() == base)
                    || lex
                        .descriptors
                        .iter()
                        .chain(&lex.lines)
                        .any(|d| d.as_bytes() == base)
                    || lex
                        .trades
                        .iter()
                        .any(|d| d.as_bytes().split(|&c| c == b' ').any(|p| p == base))
                    || (base.len() == 4 && base.iter().all(u8::is_ascii_digit));
                assert!(known, "{:?}", core::str::from_utf8(w));
            }
            if let Some(Second::Since(y)) = s.second {
                assert!((1930..era_start(g.era)).contains(&y), "{y}");
            }
            seen.insert(t);
        }
        assert!(seen.len() > 1500, "{} distinct wordmarks", seen.len());
    }

    fn every_weighted_choice_is_reached_in(g0: crate::grammar::Grammar) {
        let (g, signs) = signs(g0.clone());
        for t in &g.signs.trades {
            let i = g.lexicon.trades.iter().position(|w| *w == t.trade).unwrap();
            assert!(
                signs.iter().any(|s| usize::from(s.trade) == i),
                "{}",
                t.trade
            );
            for &(p, _) in &t.pictograms {
                assert!(
                    signs
                        .iter()
                        .any(|s| usize::from(s.trade) == i && s.pictogram == Some(p)),
                    "{} {p:?}",
                    t.trade
                );
            }
        }
        for style in LetterStyle::ALL {
            assert!(signs.iter().any(|s| s.style == style), "{style:?}");
        }
        assert!(signs
            .iter()
            .any(|s| matches!(s.second, Some(Second::Since(_)))));
        assert!(signs.iter().any(|s| s.lit_box) && signs.iter().any(|s| !s.lit_box));
    }

    #[test]
    fn names_do_not_repeat_within_the_radius() {
        let g = coast_80s();
        // Shops every 12 m along four parallel streets 60 m apart.
        let at: Vec<V2> = (0..400)
            .map(|i| V2::new((i % 100) as f32 * 12.0, (i / 100) as f32 * 60.0))
            .collect();
        let signs = assign(&g.signs, &g.lexicon, g.era, 7, &at);
        let r = g.signs.name_radius;
        let mut close = 0;
        for i in 0..at.len() {
            for j in 0..i {
                let d = at[i] - at[j];
                if d.dot(d) >= r * r {
                    continue;
                }
                close += 1;
                let (a, b) = (&signs[i], &signs[j]);
                assert_ne!(a.text(&g.lexicon), b.text(&g.lexicon), "{i} {j}");
                assert!(!(a.named() && b.named() && a.name == b.name), "{i} {j}");
            }
        }
        assert!(close > 1000);
        assert_eq!(signs, assign(&g.signs, &g.lexicon, g.era, 7, &at));
        assert_ne!(signs, assign(&g.signs, &g.lexicon, g.era, 8, &at));
        assert!(assign(&g.signs, &Lexicon::default(), g.era, 7, &at).is_empty());
    }

    #[test]
    fn secondary_lines_read() {
        let g = coast_80s();
        let mut s = ShopSign::draw(&g.signs, &g.lexicon, g.era, 1);
        s.second = Some(Second::Since(1962));
        assert_eq!(s.second_text(&g.lexicon).unwrap(), b"SINCE 1962");
        s.form = Form::PossessiveTrade;
        let t = s.text(&g.lexicon);
        assert!(
            t.windows(3).any(|w| w == b"'S "),
            "{:?}",
            core::str::from_utf8(&t)
        );
    }
}
