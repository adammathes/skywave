//! Invented names for points of interest: a stem of two or three made-up syllables from
//! the Poi's `name_seed`, then a word for its kind ("Torvel Pier"). Pure and deterministic;
//! nothing here is, or is looked up from, a real name.

use core::fmt::{self, Write};

use place::PoiKind;
use skywave_core::{Rng, Seed};

/// Rng key for POI names.
const KEY_POI_NAME: u64 = 0x504F_495F_4E41_4D45;

const ONSETS: [&str; 18] = [
    "b", "c", "d", "f", "g", "h", "l", "m", "n", "p", "r", "s", "t", "v", "w", "br", "cl", "tr",
];
const VOWELS: [&str; 10] = ["a", "e", "i", "o", "u", "a", "e", "o", "ai", "ea"];
/// Closing a syllable inside the stem, and at its end.
const MID_CODAS: [&str; 6] = ["", "", "", "n", "r", "l"];
const END_CODAS: [&str; 9] = ["", "", "n", "r", "l", "s", "nd", "th", "m"];

/// The word after the stem for each kind.
pub fn kind_word(kind: PoiKind) -> &'static str {
    match kind {
        PoiKind::Pier => "Pier",
        PoiKind::Lighthouse => "Light",
        PoiKind::Overlook => "Point",
        PoiKind::Diner => "Diner",
        PoiKind::Gas => "Gas",
        PoiKind::Marina => "Marina",
        PoiKind::Stadium => "Field",
        PoiKind::DragStrip => "Strip",
        PoiKind::ChannelRamp => "Ramp",
        PoiKind::Tunnel => "Tunnel",
        PoiKind::CrewGarage => "Garage",
        PoiKind::TacoStand => "Tacos",
        PoiKind::GradeCrossing => "Crossing",
        PoiKind::HillsideLetters => "Hill",
        PoiKind::WaterTower => "Tower",
        PoiKind::DriveIn => "Drive-In",
    }
}

/// Writes the stem for `seed`: two syllables, sometimes three, capitalized.
pub fn write_stem(out: &mut impl Write, seed: u64) -> fmt::Result {
    let mut rng = Rng::new(Seed(seed), &[KEY_POI_NAME]);
    let syllables = if rng.range(0, 6) == 0 { 3 } else { 2 };
    let mut first = true;
    for s in 0..syllables {
        let pick =
            |rng: &mut Rng, list: &[&'static str]| list[rng.range(0, list.len() as u32) as usize];
        let onset = pick(&mut rng, &ONSETS);
        // Two-letter vowels only in the first syllable, so they never stack up.
        let vowels = if s == 0 { &VOWELS[..] } else { &VOWELS[..8] };
        let vowel = pick(&mut rng, vowels);
        let coda = if s + 1 == syllables {
            pick(&mut rng, &END_CODAS)
        } else {
            pick(&mut rng, &MID_CODAS)
        };
        for part in [onset, vowel, coda] {
            for c in part.chars() {
                if first {
                    out.write_char(c.to_ascii_uppercase())?;
                    first = false;
                } else {
                    out.write_char(c)?;
                }
            }
        }
    }
    Ok(())
}

/// Writes the full name, "Stem Kind".
pub fn write_name(out: &mut impl Write, kind: PoiKind, seed: u64) -> fmt::Result {
    write_stem(out, seed)?;
    write!(out, " {}", kind_word(kind))
}

/// The full name as a new string (load time).
pub fn poi_name(kind: PoiKind, seed: u64) -> String {
    let mut s = String::new();
    // Writing to a String cannot fail.
    let _ = write_name(&mut s, kind, seed);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_deterministic_short_and_varied() {
        assert_eq!(poi_name(PoiKind::Diner, 7), poi_name(PoiKind::Diner, 7));
        let mut stems = std::collections::BTreeSet::new();
        for seed in 0..200u64 {
            let name = poi_name(PoiKind::Pier, seed);
            let stem = name.strip_suffix(" Pier").expect("kind word last");
            assert!((3..=15).contains(&stem.len()), "{name}");
            assert!(stem.chars().next().unwrap().is_ascii_uppercase(), "{name}");
            assert!(
                stem.chars().skip(1).all(|c| c.is_ascii_lowercase()),
                "{name}"
            );
            stems.insert(stem.to_string());
        }
        assert!(stems.len() > 180, "only {} distinct stems", stems.len());
        // Pinned so a change to the generator is deliberate.
        assert_eq!(poi_name(PoiKind::Gas, 1), "Gailmin Gas");
        assert!(poi_name(PoiKind::Stadium, 3).ends_with(" Field"));
    }
}
