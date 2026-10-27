//! Facade grammars: the numbers and weights the building planner reads, one per era and
//! region template (`docs/WORLD_GENERATION.md`, "Facade grammars"; `docs/backlog/BUILDINGS.md`).
//!
//! A [`Grammar`] is parsed from RON (`assets/grammar/<template>_<era>.ron`) by `app` and
//! the tests; this crate only defines the types (serde derives behind the `serde` feature)
//! and [`Grammar::validate`]. The geometry builders stay in [`crate::buildings`]; a grammar
//! says how big, how many and how often, never how a piece is shaped.
//!
//! Conventions (as in [`crate::template`]):
//! - A `(f32, f32)` range is `(min, max)`, inclusive.
//! - A `(u8, u8)` count range is `(min, max)`, inclusive; `(0, 0)` draws nothing.
//! - A share `_256` is out of 256.
//! - A [`ByUse`] value differs by what the building is used for: residential, office, or
//!   anything else.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use place::{BuildingUse, Era, StyleTag};

use crate::signs::letters::LetterStyle;
use crate::signs::lexicon::{Form, Lexicon, LexiconError};
use crate::signs::pictogram::Pictogram;
use crate::template::{CountRange, Range};

/// Every style a grammar must give rules for, in `StyleTag` order.
pub const STYLES: [StyleTag; 12] = [
    StyleTag::StuccoBox,
    StyleTag::Brick,
    StyleTag::Deco,
    StyleTag::StripMall,
    StyleTag::Dingbat,
    StyleTag::Craftsman,
    StyleTag::Ranch,
    StyleTag::Tract70s,
    StyleTag::Glass,
    StyleTag::Warehouse,
    StyleTag::Barn,
    StyleTag::Motel,
];

/// The building rules of one era of one region template.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Grammar {
    pub era: Era,
    /// The region template it dresses (`assets/regions/<template>.ron`).
    pub template: String,
    /// One entry per [`StyleTag`].
    pub styles: Vec<StyleRules>,
    pub signs: SignRules,
    pub lots: LotRules,
    /// The words `signs.lexicon` names (`assets/grammar/lexicon.ron`), parsed beside the
    /// grammar and attached by its loader; not part of the grammar file.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub lexicon: Lexicon,
}

/// A value per kind of use.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ByUse<T> {
    pub residential: T,
    pub office: T,
    /// Commercial, industrial, civic, hotel and parking.
    pub other: T,
}

impl<T: Copy> ByUse<T> {
    pub fn of(&self, usage: BuildingUse) -> T {
        match usage {
            BuildingUse::Residential => self.residential,
            BuildingUse::Office => self.office,
            _ => self.other,
        }
    }
}

/// How one style is planned.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct StyleRules {
    pub style: StyleTag,
    pub massing: Massing,
    pub base: Base,
    pub body: Body,
    pub top: Top,
    pub addenda: Addenda,
}

/// Floor heights.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Massing {
    /// Every floor the same height (flats, motels); otherwise a taller ground floor.
    pub even_floors: ByUse<bool>,
    /// Upper floor height the ground floor is fitted against, m.
    pub floor_h: f32,
    /// Ground floor height, m, before the cap below.
    pub ground_floor_h: Range,
    /// Largest share of the wall height the ground floor may take.
    pub ground_share_max: f32,
}

/// The ground floor.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Base {
    /// Plinth height above the lot, m.
    pub plinth_h: ByUse<f32>,
    /// Window bay width on facades that face a street, m.
    pub bay_w: ByUse<f32>,
    /// Entrance door `(width, height)`, m.
    pub door: ByUse<(f32, f32)>,
}

/// The upper floors.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Body {
    pub window: ByUse<Window>,
    /// Smallest window height, m.
    pub window_h_min: f32,
    /// Sill above the plinth on the ground floor, above the floor on the others, m.
    pub sill_ground: f32,
    pub sill_upper: f32,
}

/// A window's size from the bay and the floor.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Window {
    /// Width: `clamp(bay * w_share, w.0, w.1)`, m.
    pub w_share: f32,
    pub w: Range,
    /// Height: `min(upper floor height - head, h_max)`, m.
    pub head: f32,
    pub h_max: f32,
}

/// The roofline.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Top {
    /// Parapet above a flat roof, m (pitched roofs have none); at most 30% of the height.
    pub parapet_h: ByUse<f32>,
}

/// What clutters a flat roof.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Addenda {
    /// Vent hoods and pipes.
    pub roof_vents: ByUse<CountRange>,
    /// Packaged rooftop units.
    pub roof_units: ByUse<CountRange>,
    /// Buildings of this many floors or more get a stair bulkhead...
    pub bulkhead_floors: u8,
    /// ...and one in this many of those a water tank (on a roof over 9 m each way).
    pub water_tank_1_in: u32,
}

/// The awning over a storefront.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Awning {
    Striped = 0,
    Solid = 1,
    /// A flat canopy.
    Canopy = 2,
    None = 3,
}

/// The signage a terrain place's detailing gives its buildings.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct SignRules {
    /// Awning kinds with integer weights; listed order is draw order.
    pub awnings: Vec<(Awning, u32)>,
    /// Signs that are backlit boxes rather than neon letters.
    pub lit_box_256: u32,
    /// Shops open late.
    pub shop_lit_256: u32,
    /// The id of the lexicon the wordmarks draw their words from.
    pub lexicon: String,
    /// What the shops with signs sell, with integer weights, and how each is pictured.
    pub trades: Vec<TradeRule>,
    /// Wordmark forms with integer weights.
    pub forms: Vec<(Form, u32)>,
    /// Lettering styles with integer weights.
    pub wordmarks: Vec<(LetterStyle, u32)>,
    /// Signs that carry a secondary line (from their trade's `lines`).
    pub line_256: u32,
    /// No two signs closer than this share a wordmark or a name, m.
    pub name_radius: f32,
}

/// One trade a sign may advertise.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct TradeRule {
    /// A word of the lexicon's `trades`.
    pub trade: String,
    pub weight: u32,
    /// Its pictograms with integer weights; empty for wordmark-only signs.
    pub pictograms: Vec<(Pictogram, u32)>,
    /// Its secondary lines with integer weights.
    pub lines: Vec<(Line, u32)>,
    /// Descriptors it never takes: pairs that would name a well-known business.
    #[cfg_attr(feature = "serde", serde(default))]
    pub descriptors_out: Vec<String>,
}

/// A secondary line.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Line {
    /// A word of the lexicon's `lines`.
    Word(String),
    /// `SINCE` and a year before the era.
    Since,
}

impl SignRules {
    /// The awning `r` (any number; reduced modulo the total weight) picks.
    pub fn awning(&self, r: u64) -> Awning {
        let total: u64 = self.awnings.iter().map(|&(_, w)| u64::from(w)).sum();
        let mut r = r % total.max(1);
        for &(a, w) in &self.awnings {
            if r < u64::from(w) {
                return a;
            }
            r -= u64::from(w);
        }
        Awning::None
    }
}

/// Parked cars in front lots and beach aisles.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LotRules {
    /// Strip malls with less open ground in front than this get no front lot, m.
    pub front_min_yard: f32,
    /// Stall width, m.
    pub stall_w: f32,
    /// Share of a front lot's stalls taken, drawn per lot.
    pub front_fill: Range,
    /// Chance a beach-aisle stall is taken.
    pub aisle_fill: f32,
}

/// Why [`Grammar::validate`] rejected a grammar. `field` names are dotted RON paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrammarError {
    EmptyTemplate,
    /// No rules, or more than one entry, for a style.
    Style {
        style: StyleTag,
    },
    /// A number is NaN, infinite, negative, or a range has `min > max`.
    Bad {
        style: Option<StyleTag>,
        field: &'static str,
    },
    /// The attached lexicon is broken.
    Lexicon(LexiconError),
    /// `signs.lexicon` is not the attached lexicon's id, or a sign rule names a word the
    /// lexicon lacks.
    Word {
        field: &'static str,
        word: String,
    },
}

impl fmt::Display for GrammarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrammarError::EmptyTemplate => write!(f, "template is empty"),
            GrammarError::Style { style } => {
                write!(f, "styles must list {style:?} exactly once")
            }
            GrammarError::Bad {
                style: Some(s),
                field,
            } => write!(f, "{s:?}: {field} is invalid"),
            GrammarError::Bad { style: None, field } => write!(f, "{field} is invalid"),
            GrammarError::Lexicon(e) => write!(f, "lexicon: {e}"),
            GrammarError::Word { field, word } => {
                write!(f, "{field}: {word:?} is not in the lexicon")
            }
        }
    }
}

impl Grammar {
    /// The rules for `style`. The grammar must have passed [`Grammar::validate`].
    pub fn style(&self, style: StyleTag) -> &StyleRules {
        self.styles
            .iter()
            .find(|r| r.style == style)
            .expect("a validated grammar has every style")
    }

    /// Checks every field; planning assumes a grammar that passed.
    pub fn validate(&self) -> Result<(), GrammarError> {
        if self.template.trim().is_empty() {
            return Err(GrammarError::EmptyTemplate);
        }
        for style in STYLES {
            if self.styles.iter().filter(|r| r.style == style).count() != 1 {
                return Err(GrammarError::Style { style });
            }
        }
        for r in &self.styles {
            r.validate().map_err(|field| GrammarError::Bad {
                style: Some(r.style),
                field,
            })?;
        }
        let bad = |field| GrammarError::Bad { style: None, field };
        let s = &self.signs;
        if s.awnings.iter().map(|&(_, w)| w).sum::<u32>() == 0 {
            return Err(bad("signs.awnings"));
        }
        if s.lit_box_256 > 256 {
            return Err(bad("signs.lit_box_256"));
        }
        if s.shop_lit_256 > 256 {
            return Err(bad("signs.shop_lit_256"));
        }
        self.lexicon.validate().map_err(GrammarError::Lexicon)?;
        let word = |field, word: &str| GrammarError::Word {
            field,
            word: word.into(),
        };
        if s.lexicon != self.lexicon.id {
            return Err(word("signs.lexicon", &s.lexicon));
        }
        if s.trades.iter().map(|t| t.weight).sum::<u32>() == 0 {
            return Err(bad("signs.trades"));
        }
        for t in &s.trades {
            if !self.lexicon.trades.contains(&t.trade) {
                return Err(word("signs.trades", &t.trade));
            }
            if t.pictograms.iter().any(|p| p.1 == 0) || t.lines.iter().any(|l| l.1 == 0) {
                return Err(bad("signs.trades"));
            }
            if let Some(d) = t
                .descriptors_out
                .iter()
                .find(|d| !self.lexicon.descriptors.contains(d))
            {
                return Err(word("signs.trades.descriptors_out", d));
            }
            if t.descriptors_out.len() >= self.lexicon.descriptors.len() {
                return Err(bad("signs.trades"));
            }
            for (l, _) in &t.lines {
                if let Line::Word(w) = l {
                    if !self.lexicon.lines.contains(w) {
                        return Err(word("signs.trades.lines", w));
                    }
                }
            }
        }
        if s.forms.iter().map(|f| f.1).sum::<u32>() == 0 {
            return Err(bad("signs.forms"));
        }
        if s.wordmarks.iter().map(|w| w.1).sum::<u32>() == 0 {
            return Err(bad("signs.wordmarks"));
        }
        if s.line_256 > 256 {
            return Err(bad("signs.line_256"));
        }
        if !s.name_radius.is_finite() || s.name_radius < 0.0 {
            return Err(bad("signs.name_radius"));
        }
        let l = &self.lots;
        if !positive(l.front_min_yard) {
            return Err(bad("lots.front_min_yard"));
        }
        if !positive(l.stall_w) || l.stall_w < 2.0 {
            return Err(bad("lots.stall_w"));
        }
        if !range(l.front_fill) || l.front_fill.1 > 1.0 {
            return Err(bad("lots.front_fill"));
        }
        if !(0.0..=1.0).contains(&l.aisle_fill) {
            return Err(bad("lots.aisle_fill"));
        }
        Ok(())
    }
}

impl StyleRules {
    fn validate(&self) -> Result<(), &'static str> {
        let m = &self.massing;
        check(positive(m.floor_h), "massing.floor_h")?;
        check(range(m.ground_floor_h), "massing.ground_floor_h")?;
        check(
            m.ground_share_max > 0.0 && m.ground_share_max <= 1.0,
            "massing.ground_share_max",
        )?;
        let b = &self.base;
        check(all(&b.plinth_h, |&h| h >= 0.0), "base.plinth_h")?;
        check(all(&b.bay_w, |&w| w >= 1.0), "base.bay_w")?;
        check(
            all(&b.door, |&(w, h)| positive(w) && positive(h)),
            "base.door",
        )?;
        let y = &self.body;
        check(
            all(&y.window, |w| w.w_share >= 0.0 && range(w.w) && w.w.0 > 0.0),
            "body.window",
        )?;
        check(
            all(&y.window, |w| w.head >= 0.0 && positive(w.h_max)),
            "body.window",
        )?;
        check(positive(y.window_h_min), "body.window_h_min")?;
        check(y.sill_ground >= 0.0, "body.sill_ground")?;
        check(y.sill_upper >= 0.0, "body.sill_upper")?;
        check(all(&self.top.parapet_h, |&h| h >= 0.0), "top.parapet_h")?;
        let a = &self.addenda;
        check(all(&a.roof_vents, |c| c.0 <= c.1), "addenda.roof_vents")?;
        check(all(&a.roof_units, |c| c.0 <= c.1), "addenda.roof_units")?;
        check(a.water_tank_1_in >= 1, "addenda.water_tank_1_in")?;
        Ok(())
    }
}

fn check(ok: bool, field: &'static str) -> Result<(), &'static str> {
    if ok {
        Ok(())
    } else {
        Err(field)
    }
}

fn all<T>(v: &ByUse<T>, ok: impl Fn(&T) -> bool) -> bool {
    ok(&v.residential) && ok(&v.office) && ok(&v.other)
}

/// Finite and above zero (NaN fails).
fn positive(x: f32) -> bool {
    x.is_finite() && x > 0.0
}

/// A finite, non-negative `(min, max)` with `min <= max`.
fn range(r: Range) -> bool {
    r.0.is_finite() && r.1.is_finite() && r.0 >= 0.0 && r.0 <= r.1
}

/// The shipped coast grammar with its lexicon, for this crate's tests.
#[cfg(test)]
pub(crate) fn coast_80s() -> Grammar {
    let text = include_str!("../../../assets/grammar/california_coast_80s.ron");
    let mut g: Grammar = ron::from_str(text).expect("parse");
    let lexicon = include_str!("../../../assets/grammar/lexicon.ron");
    g.lexicon = ron::from_str(lexicon).expect("parse lexicon");
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rules the planner had in code before they moved into the grammar, written the
    /// way the code wrote them.
    fn former(style: StyleTag) -> StyleRules {
        use StyleTag as S;
        let by = |f: &dyn Fn(BuildingUse) -> f32| ByUse {
            residential: f(BuildingUse::Residential),
            office: f(BuildingUse::Office),
            other: f(BuildingUse::Commercial),
        };
        let parapet = by(&|u| match (style, u) {
            (S::StuccoBox, BuildingUse::Residential) => 0.5,
            (S::StuccoBox, _) => 0.8,
            (S::StripMall, _) => 1.8,
            _ => 1.0,
        });
        let plinth = by(&|u| match (u == BuildingUse::Residential, style) {
            (true, _) => 0.12,
            (_, S::StripMall) => 0.25,
            _ => 0.45,
        });
        let bay = by(&|u| match (u, style) {
            (_, S::Motel) => 3.8,
            (_, S::Glass) => 4.5,
            (_, S::Warehouse) => 6.0,
            (BuildingUse::Residential, _) => 3.6,
            (_, S::Brick) => 2.7,
            (BuildingUse::Office, _) => 2.8,
            _ => 3.0,
        });
        let fixed = |w: f32, head: f32, h_max: f32| Window {
            w_share: 0.0,
            w: (w, w),
            head,
            h_max,
        };
        let window = |residential: bool| match (residential, style) {
            (true, S::Ranch) => fixed(1.9, 1.35, 1.2),
            (true, _) => fixed(1.3, 1.35, 1.3),
            (false, S::Brick) => fixed(1.1, 1.3, 1.8),
            (false, S::Deco) => Window {
                w_share: 0.62,
                w: (0.5, 10.0),
                head: 1.45,
                h_max: 1.5,
            },
            _ => Window {
                w_share: 0.5,
                w: (0.9, 1.6),
                head: 1.3,
                h_max: 1.7,
            },
        };
        let motel = style == S::Motel;
        StyleRules {
            style,
            massing: Massing {
                even_floors: ByUse {
                    residential: true,
                    office: motel,
                    other: motel,
                },
                floor_h: 3.3,
                ground_floor_h: (3.8, 4.8),
                ground_share_max: 0.6,
            },
            base: Base {
                plinth_h: plinth,
                bay_w: bay,
                door: ByUse {
                    residential: (1.0, 2.2),
                    office: (2.0, 2.6),
                    other: (2.0, 2.6),
                },
            },
            body: Body {
                window: ByUse {
                    residential: window(true),
                    office: window(false),
                    other: window(false),
                },
                window_h_min: 0.6,
                sill_ground: 0.65,
                sill_upper: 0.9,
            },
            top: Top { parapet_h: parapet },
            addenda: Addenda {
                // `rng.range(1, 3)` and `rng.range(2, 5)` are half-open.
                roof_vents: ByUse {
                    residential: (1, 2),
                    office: (2, 4),
                    other: (2, 4),
                },
                roof_units: ByUse {
                    residential: (0, 0),
                    office: (1, 2),
                    other: (1, 2),
                },
                bulkhead_floors: 4,
                water_tank_1_in: 2,
            },
        }
    }

    #[test]
    fn shipped_grammar_is_the_former_constants() {
        let g = coast_80s();
        g.validate().expect("validate");
        assert_eq!(g.era, Era::Eighties);
        assert_eq!(g.template, "california_coast");
        for style in STYLES {
            assert_eq!(*g.style(style), former(style), "{style:?}");
        }
        assert_eq!(g.signs.lit_box_256, 70);
        assert_eq!(g.signs.shop_lit_256, 115);
        // The former draw: `(h >> 8) % 4` in Striped, Solid, Canopy, None order.
        for r in 0..64u64 {
            assert_eq!(g.signs.awning(r) as u64, r % 4);
        }
        assert_eq!(
            g.lots,
            LotRules {
                front_min_yard: 9.5,
                stall_w: 2.8,
                front_fill: (0.25, 0.75),
                aisle_fill: 0.4,
            }
        );
    }

    #[test]
    fn grammar_round_trips_through_ron() {
        let g = coast_80s();
        let text =
            ron::ser::to_string_pretty(&g, ron::ser::PrettyConfig::default()).expect("serialize");
        let mut back: Grammar = ron::from_str(&text).expect("parse back");
        // The lexicon is a file of its own.
        assert_eq!(back.lexicon, Lexicon::default());
        back.lexicon = g.lexicon.clone();
        assert_eq!(back, g);
        let lex = ron::ser::to_string_pretty(&g.lexicon, Default::default()).expect("serialize");
        assert_eq!(
            ron::from_str::<Lexicon>(&lex).expect("parse back"),
            g.lexicon
        );
    }

    #[test]
    fn validate_rejects_broken_grammars() {
        let mut g = coast_80s();
        g.styles.pop();
        assert_eq!(
            g.validate(),
            Err(GrammarError::Style {
                style: StyleTag::Motel
            })
        );
        let mut g = coast_80s();
        g.styles[0].base.bay_w.office = f32::NAN;
        assert!(matches!(
            g.validate(),
            Err(GrammarError::Bad {
                field: "base.bay_w",
                ..
            })
        ));
        let mut g = coast_80s();
        g.signs.awnings.clear();
        assert!(g.validate().is_err());
        let mut g = coast_80s();
        g.lots.front_fill = (0.8, 0.2);
        assert!(g.validate().is_err());
        let mut g = coast_80s();
        g.signs.trades[0].trade = "WIDGETS".into();
        assert!(matches!(g.validate(), Err(GrammarError::Word { .. })));
        let mut g = coast_80s();
        g.signs.lexicon = "elsewhere".into();
        assert!(matches!(g.validate(), Err(GrammarError::Word { .. })));
        let mut g = coast_80s();
        g.lexicon.trades.clear();
        assert!(matches!(g.validate(), Err(GrammarError::Lexicon(_))));
    }
}
