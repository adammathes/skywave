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

use place::{BuildingUse, Era, StyleTag, ZoneId};

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
    /// Rules per [`StyleTag`] and zone: [`Grammar::style`] takes the first entry, in
    /// file order, whose style matches and whose `zones` hold the building's zone.
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

/// A set of zones, one bit per [`ZoneId`]. Written in RON as `All` or
/// `Only([Strip, Commercial])`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(from = "ZoneList", into = "ZoneList")
)]
pub struct ZoneMask(pub u16);

impl ZoneMask {
    pub const ALL: ZoneMask = ZoneMask(u16::MAX);

    pub fn has(self, zone: ZoneId) -> bool {
        self.0 & (1 << zone as u16) != 0
    }
}

/// How a [`ZoneMask`] is written.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ZoneList {
    All,
    Only(Vec<ZoneId>),
}

impl From<ZoneList> for ZoneMask {
    fn from(l: ZoneList) -> ZoneMask {
        match l {
            ZoneList::All => ZoneMask::ALL,
            ZoneList::Only(zs) => ZoneMask(zs.iter().fold(0, |m, &z| m | 1 << z as u16)),
        }
    }
}

impl From<ZoneMask> for ZoneList {
    fn from(m: ZoneMask) -> ZoneList {
        if m == ZoneMask::ALL {
            return ZoneList::All;
        }
        ZoneList::Only(ZONES.iter().copied().filter(|&z| m.has(z)).collect())
    }
}

/// Every zone, in `ZoneId` order.
pub const ZONES: [ZoneId; 12] = [
    ZoneId::Downtown,
    ZoneId::Strip,
    ZoneId::Residential,
    ZoneId::Suburb,
    ZoneId::Industrial,
    ZoneId::Commercial,
    ZoneId::Park,
    ZoneId::Beach,
    ZoneId::Farmland,
    ZoneId::Hills,
    ZoneId::Highway,
    ZoneId::Water,
];

/// How one style is planned in some zones.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct StyleRules {
    pub style: StyleTag,
    /// The zones these rules dress.
    pub zones: ZoneMask,
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
    /// What the bays of the street front's ground floor are, by weight. All zero: no base
    /// band, the ground floor is windows and a door like the floors above.
    pub ground: ByUse<GroundBays>,
    /// Base bay width, pier centre to pier centre, drawn per building, m.
    pub front_bay_w: Range,
    /// Pier width between base bays, m.
    pub pier_w: Range,
    pub storefront: Storefront,
}

/// Integer weights of the kinds of base bay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct GroundBays {
    pub storefront: u16,
    /// The office or hotel entrance (at most one per front).
    pub lobby: u16,
    pub garage: u16,
    /// Blank wall.
    pub wall: u16,
    /// The door to the stairs of the flats above (at most one; none on one storey).
    pub door: u16,
}

impl GroundBays {
    pub fn total(&self) -> u32 {
        [
            self.storefront,
            self.lobby,
            self.garage,
            self.wall,
            self.door,
        ]
        .iter()
        .map(|&w| u32::from(w))
        .sum()
    }
}

/// A storefront bay, bottom up: bulkhead, display glass, transom, sign band. Heights are
/// drawn per building.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Storefront {
    /// Bulkhead (the panel under the display glass), m.
    pub bulkhead_h: Range,
    /// Top of the display glass above the floor, m.
    pub glass_top: Range,
    /// Transom over the display glass, m.
    pub transom_h: Range,
    /// Transoms still glazed rather than panelled over.
    pub transom_glass_256: u32,
    /// Entry door `(width, height)`, m.
    pub entry: (f32, f32),
    /// Fronts whose entries are recessed (pre-war); the rest are flush.
    pub recessed_256: u32,
    /// How far a recessed entry sits back, m.
    pub recess: Range,
    /// Entries at one side of the bay rather than centred.
    pub side_entry_256: u32,
}

/// The upper floors.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Body {
    pub window: ByUse<Window>,
    /// The kind of upper window, by weight, drawn per building.
    pub kinds: ByUse<WindowKinds>,
    /// Smallest window height, m.
    pub window_h_min: f32,
    /// Sill above the plinth on the ground floor, above the floor on the others, m.
    pub sill_ground: f32,
    pub sill_upper: f32,
}

/// Integer weights of the kinds of window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct WindowKinds {
    /// One window per bay.
    pub punched: u16,
    /// Two narrow windows per bay with a mullion pier between.
    pub paired: u16,
    /// A continuous band of glass across the facade, mullions on the bay lines.
    pub ribbon: u16,
    /// A projecting bay window (upper floors).
    pub bay: u16,
    /// One wide window per bay.
    pub picture: u16,
}

impl WindowKinds {
    pub fn weights(&self) -> [u16; 5] {
        [
            self.punched,
            self.paired,
            self.ribbon,
            self.bay,
            self.picture,
        ]
    }
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
    /// The kind of top on a flat roof, by weight, drawn per building.
    pub kinds: TopKinds,
    /// How far a cornice projects, m.
    pub cornice_d: f32,
}

/// Integer weights of the kinds of flat-roof top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct TopKinds {
    /// A plain parapet with coping.
    pub parapet: u16,
    /// A projecting cornice.
    pub cornice: u16,
    /// A parapet stepped up over the middle of the street front.
    pub stepped: u16,
    /// A curved parapet, shoulders and a crown, over the middle of the street front.
    pub shaped: u16,
    /// A false front: the street wall carried up well above the roof (one storey only;
    /// taller buildings get the plain parapet).
    pub false_front: u16,
    /// A cornice on brackets along the street front.
    pub bracketed: u16,
    /// A shingled mansard all round the roof edge.
    pub mansard: u16,
}

impl TopKinds {
    pub fn weights(&self) -> [u16; 7] {
        [
            self.parapet,
            self.cornice,
            self.stepped,
            self.shaped,
            self.false_front,
            self.bracketed,
            self.mansard,
        ]
    }
}

/// The entry of `weights` that `r` picks (`r` reduced modulo the total; listed order is
/// draw order). `None` when every weight is zero.
pub fn pick(weights: &[u16], r: u32) -> Option<usize> {
    let total: u32 = weights.iter().map(|&w| u32::from(w)).sum();
    if total == 0 {
        return None;
    }
    let mut r = r % total;
    for (i, &w) in weights.iter().enumerate() {
        if r < u32::from(w) {
            return Some(i);
        }
        r -= u32::from(w);
    }
    None
}

/// What clutters a flat roof.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Addenda {
    /// Vent hoods and plumbing vents, a count.
    pub roof_vents: ByUse<CountRange>,
    /// Packaged rooftop units (a cabinet on a curb, a duct elbow into the roof) per
    /// 1000 m² of roof, drawn per building; a roof with a range above zero gets one at
    /// least.
    pub roof_units: ByUse<Range>,
    /// Exhaust fans per 1000 m² of roof.
    pub exhaust_fans: ByUse<Range>,
    /// Skylights per 1000 m² of roof.
    pub skylights: ByUse<Range>,
    /// Buildings of this many floors or more get a stair bulkhead...
    pub bulkhead_floors: u8,
    /// ...and one in this many of those a water tank (on a roof over 9 m each way).
    pub water_tank_1_in: u32,
}

/// The awning over a storefront, or the canopy along a strip mall's front.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Awning {
    /// A striped canvas shed awning with a valance.
    Striped = 0,
    /// The same in one color.
    Solid = 1,
    /// A flat stucco canopy with a trim fascia.
    Canopy = 2,
    None = 3,
    /// A quarter-round vinyl awning.
    Bullnose = 4,
    /// A shallow ribbed aluminum shed with a deep fascia.
    Aluminum = 5,
    /// A shingled mansard canopy (on posts along a strip mall).
    Mansard = 6,
}

/// The signage a terrain place's detailing gives its buildings.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct SignRules {
    /// Awning kinds over a run of storefronts, with integer weights; listed order is draw
    /// order.
    pub awnings: Vec<(Awning, u32)>,
    /// The canopy along a strip mall's front, the same way (`Mansard` or `Canopy`; any
    /// other kind is drawn as `Canopy`).
    pub mall_canopies: Vec<(Awning, u32)>,
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
        draw(&self.awnings, r, Awning::None)
    }

    /// The strip-mall canopy `r` picks.
    pub fn mall_canopy(&self, r: u64) -> Awning {
        draw(&self.mall_canopies, r, Awning::None)
    }
}

/// The entry of weighted `list` that `r` (any number) picks, `none` when it weighs nothing.
fn draw<T: Copy>(list: &[(T, u32)], r: u64, none: T) -> T {
    let total: u64 = list.iter().map(|&(_, w)| u64::from(w)).sum();
    let mut r = r % total.max(1);
    for &(a, w) in list {
        if r < u64::from(w) {
            return a;
        }
        r -= u64::from(w);
    }
    none
}

/// Parking lots (`world::lots`): how they are laid out, what stands in them and how full
/// they are hour by hour.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LotRules {
    /// Stall width, m (the test town's strip-mall stalls too).
    pub stall_w: f32,
    /// Stall length, m.
    pub stall_d: f32,
    /// Two-way aisle between 90° stalls, m.
    pub aisle_w: f32,
    /// One-way aisle between 60° stalls, m.
    pub aisle_w_60: f32,
    /// Share of lots striped at 60° (one way through: in at one curb cut, out at the other).
    pub angled_256: u32,
    /// The covered walk along a strip mall's shops, m.
    pub walkway: f32,
    /// The planted strip along a lot's street frontage, m.
    pub buffer: f32,
    /// What stands on that strip, weights in listed order.
    pub frontage: Vec<(Frontage, u32)>,
    /// A street frontage at least this long gets a second curb cut, m (an angled lot always
    /// has two).
    pub second_cut_m: f32,
    /// Share of stalls with a concrete wheel stop.
    pub wheel_stop_256: u32,
    /// Light poles along a lot, m apart.
    pub pole_m: Range,
    /// Most stalls in a row between two islands.
    pub run_max: u8,
    /// What an island at a row's end holds, weights in listed order.
    pub islands: Vec<(Planting, u32)>,
    /// Share of lots striped in yellow (the rest white).
    pub yellow_256: u32,
    /// Share of parked cars backed into their stall.
    pub backed_in_256: u32,
    /// Share of the shops, offices and civic buildings backing onto an alley in a zone with
    /// lots that leave a rear lot on it.
    pub rear_lot_256: u32,
    /// Per zone, hour by hour (0: midnight to 1 am), the share of its stalls taken, %. Only
    /// zones listed here get lots.
    pub occupancy: Vec<(ZoneId, [u8; 24])>,
}

/// What lines a lot's street frontage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Frontage {
    Hedge,
    /// A low block wall.
    Wall,
    /// Nothing but the planted strip.
    Open,
}

/// What an island holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Planting {
    Palm,
    Shrub,
    /// Bare ground.
    Bare,
}

impl LotRules {
    /// Hour by hour occupancy of zone `z`'s lots, `None` where it has none.
    pub fn occupancy(&self, z: ZoneId) -> Option<&[u8; 24]> {
        self.occupancy.iter().find(|o| o.0 == z).map(|o| &o.1)
    }

    /// The frontage `r` (any number) picks.
    pub fn frontage(&self, r: u64) -> Frontage {
        draw(&self.frontage, r, Frontage::Open)
    }

    /// The planting `r` picks.
    pub fn island(&self, r: u64) -> Planting {
        draw(&self.islands, r, Planting::Bare)
    }
}

/// Why [`Grammar::validate`] rejected a grammar. `field` names are dotted RON paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrammarError {
    EmptyTemplate,
    /// No rules for a style in a zone.
    Style {
        style: StyleTag,
        zone: ZoneId,
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
            GrammarError::Style { style, zone } => {
                write!(f, "styles has no rules for {style:?} in {zone:?}")
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
    /// The rules for `style` in `zone`: the first entry, in file order, of that style
    /// whose zones hold `zone`. The grammar must have passed [`Grammar::validate`].
    pub fn style(&self, style: StyleTag, zone: ZoneId) -> &StyleRules {
        self.styles
            .iter()
            .find(|r| r.style == style && r.zones.has(zone))
            .expect("a validated grammar has every style in every zone")
    }

    /// Checks every field; planning assumes a grammar that passed.
    pub fn validate(&self) -> Result<(), GrammarError> {
        if self.template.trim().is_empty() {
            return Err(GrammarError::EmptyTemplate);
        }
        for style in STYLES {
            for zone in ZONES {
                if !self
                    .styles
                    .iter()
                    .any(|r| r.style == style && r.zones.has(zone))
                {
                    return Err(GrammarError::Style { style, zone });
                }
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
        if s.awnings.iter().map(|&(_, w)| u64::from(w)).sum::<u64>() == 0 {
            return Err(bad("signs.awnings"));
        }
        if s.mall_canopies
            .iter()
            .map(|&(_, w)| u64::from(w))
            .sum::<u64>()
            == 0
        {
            return Err(bad("signs.mall_canopies"));
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
        for (ok, field) in [
            (positive(l.stall_w) && l.stall_w >= 2.0, "lots.stall_w"),
            (positive(l.stall_d) && l.stall_d >= 4.0, "lots.stall_d"),
            (positive(l.aisle_w) && l.aisle_w >= 3.0, "lots.aisle_w"),
            (
                positive(l.aisle_w_60) && l.aisle_w_60 >= 3.0,
                "lots.aisle_w_60",
            ),
            (l.angled_256 <= 256, "lots.angled_256"),
            (positive(l.walkway), "lots.walkway"),
            (positive(l.buffer), "lots.buffer"),
            (l.frontage.iter().any(|f| f.1 > 0), "lots.frontage"),
            (positive(l.second_cut_m), "lots.second_cut_m"),
            (l.wheel_stop_256 <= 256, "lots.wheel_stop_256"),
            (range(l.pole_m) && l.pole_m.0 >= 10.0, "lots.pole_m"),
            (l.run_max >= 2, "lots.run_max"),
            (l.islands.iter().any(|f| f.1 > 0), "lots.islands"),
            (l.yellow_256 <= 256, "lots.yellow_256"),
            (l.backed_in_256 <= 256, "lots.backed_in_256"),
            (l.rear_lot_256 <= 256, "lots.rear_lot_256"),
            (
                l.occupancy.iter().all(|o| o.1.iter().all(|&p| p <= 100)),
                "lots.occupancy",
            ),
        ] {
            if !ok {
                return Err(bad(field));
            }
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
        check(
            range(b.front_bay_w) && b.front_bay_w.0 >= 2.0,
            "base.front_bay_w",
        )?;
        check(
            range(b.pier_w) && b.pier_w.1 * 2.0 < b.front_bay_w.0,
            "base.pier_w",
        )?;
        let s = &b.storefront;
        check(range(s.bulkhead_h), "base.storefront.bulkhead_h")?;
        check(
            range(s.glass_top) && s.glass_top.0 > s.bulkhead_h.1 + 1.0,
            "base.storefront.glass_top",
        )?;
        check(range(s.transom_h), "base.storefront.transom_h")?;
        check(
            positive(s.entry.0) && positive(s.entry.1) && s.entry.1 <= s.glass_top.0,
            "base.storefront.entry",
        )?;
        check(range(s.recess), "base.storefront.recess")?;
        check(
            s.transom_glass_256 <= 256 && s.recessed_256 <= 256 && s.side_entry_256 <= 256,
            "base.storefront",
        )?;
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
        check(self.top.kinds.weights().iter().any(|&w| w > 0), "top.kinds")?;
        check(
            self.top.cornice_d.is_finite() && (0.0..=1.0).contains(&self.top.cornice_d),
            "top.cornice_d",
        )?;
        check(
            all(&y.kinds, |k| k.weights().iter().any(|&w| w > 0)),
            "body.kinds",
        )?;
        let a = &self.addenda;
        check(all(&a.roof_vents, |c| c.0 <= c.1), "addenda.roof_vents")?;
        check(all(&a.roof_units, |&r| range(r)), "addenda.roof_units")?;
        check(all(&a.exhaust_fans, |&r| range(r)), "addenda.exhaust_fans")?;
        check(all(&a.skylights, |&r| range(r)), "addenda.skylights")?;
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
    use alloc::vec;

    /// The rules the planner had in code before they moved into the grammar, written the
    /// way the code wrote them, plus the three-band facade's starting values (B4, from
    /// `docs/backlog/BUILDINGS.md`). `industrial` is the entry for industrial zones.
    fn former(style: StyleTag, industrial: bool) -> StyleRules {
        use StyleTag as S;
        let bays = |storefront, lobby, garage, wall, door| GroundBays {
            storefront,
            lobby,
            garage,
            wall,
            door,
        };
        let none = GroundBays::default();
        let (ground_office, ground_other) = match (style, industrial) {
            (S::StuccoBox | S::Brick, true) => {
                let o = if style == S::Brick {
                    bays(2, 2, 0, 2, 0)
                } else {
                    bays(2, 2, 0, 1, 0)
                };
                (o, bays(0, 0, 3, 2, 1))
            }
            (S::StuccoBox, _) => (bays(2, 2, 0, 1, 0), bays(6, 0, 0, 1, 1)),
            (S::Brick, _) => (bays(2, 2, 0, 2, 0), bays(5, 0, 0, 1, 2)),
            (S::Deco, _) => (bays(2, 3, 0, 1, 0), bays(6, 0, 0, 1, 1)),
            _ => (none, none),
        };
        let (front_bay_w, pier_w, transom_glass_256, recessed_256, side_entry_256) = match style {
            S::StuccoBox => ((5.5, 8.0), (0.35, 0.5), 64, 64, 128),
            S::Brick => ((5.0, 7.6), (0.5, 0.7), 200, 200, 96),
            S::Deco => ((5.5, 8.0), (0.4, 0.55), 160, 160, 64),
            _ => ((5.5, 8.0), (0.35, 0.5), 128, 0, 128),
        };
        let kinds = |punched, paired, ribbon, bay, picture| WindowKinds {
            punched,
            paired,
            ribbon,
            bay,
            picture,
        };
        let one = kinds(1, 0, 0, 0, 0);
        let window_kinds = match style {
            S::StuccoBox => ByUse {
                residential: kinds(3, 1, 0, 0, 1),
                office: kinds(2, 1, 2, 0, 0),
                other: kinds(3, 2, 0, 0, 1),
            },
            S::Brick => ByUse {
                residential: kinds(3, 1, 0, 1, 0),
                office: kinds(3, 1, 0, 0, 0),
                other: kinds(3, 2, 0, 0, 0),
            },
            S::Deco => ByUse {
                residential: kinds(2, 1, 0, 0, 1),
                office: kinds(2, 0, 1, 0, 0),
                other: kinds(2, 0, 1, 0, 0),
            },
            S::Craftsman => ByUse {
                residential: kinds(2, 1, 0, 0, 0),
                office: one,
                other: one,
            },
            S::Tract70s => ByUse {
                residential: kinds(2, 0, 0, 0, 1),
                office: one,
                other: one,
            },
            S::Dingbat => ByUse {
                residential: kinds(1, 0, 0, 0, 1),
                office: one,
                other: one,
            },
            _ => ByUse {
                residential: one,
                office: one,
                other: one,
            },
        };
        // B9: parapet, cornice, stepped, shaped, false front, bracketed, mansard.
        let (top_kinds, cornice_d) = match (style, industrial) {
            (S::StuccoBox, true) => ([3, 1, 1, 0, 1, 0, 0], 0.3),
            (S::StuccoBox, _) => ([3, 1, 1, 1, 2, 0, 1], 0.3),
            (S::Brick, true) => ([1, 1, 0, 0, 0, 1, 0], 0.24),
            (S::Brick, _) => ([1, 2, 0, 0, 1, 2, 0], 0.24),
            (S::Deco, _) => ([1, 0, 3, 1, 0, 0, 0], 0.2),
            (S::Motel, _) => ([2, 0, 0, 0, 0, 0, 0], 0.2),
            _ => ([1, 0, 0, 0, 0, 0, 0], 0.2),
        };
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
        let banded = matches!(style, S::StuccoBox | S::Brick | S::Deco);
        StyleRules {
            style,
            zones: if industrial {
                ZoneMask::from(ZoneList::Only(vec![ZoneId::Industrial]))
            } else {
                ZoneMask::ALL
            },
            massing: Massing {
                even_floors: ByUse {
                    residential: true,
                    office: motel,
                    other: motel,
                },
                floor_h: 3.3,
                ground_floor_h: if banded { (4.0, 5.0) } else { (3.8, 4.8) },
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
                ground: ByUse {
                    residential: none,
                    office: ground_office,
                    other: ground_other,
                },
                front_bay_w,
                pier_w,
                storefront: Storefront {
                    bulkhead_h: (0.4, 0.6),
                    glass_top: (2.6, 3.0),
                    transom_h: (0.5, 0.9),
                    transom_glass_256,
                    entry: (1.1, 2.25),
                    recessed_256,
                    recess: (1.0, 1.8),
                    side_entry_256,
                },
            },
            body: Body {
                window: ByUse {
                    residential: window(true),
                    office: window(false),
                    other: window(false),
                },
                kinds: window_kinds,
                window_h_min: 0.6,
                sill_ground: 0.65,
                sill_upper: 0.9,
            },
            top: Top {
                parapet_h: parapet,
                kinds: TopKinds {
                    parapet: top_kinds[0],
                    cornice: top_kinds[1],
                    stepped: top_kinds[2],
                    shaped: top_kinds[3],
                    false_front: top_kinds[4],
                    bracketed: top_kinds[5],
                    mansard: top_kinds[6],
                },
                cornice_d,
            },
            addenda: Addenda {
                // `rng.range(1, 3)` and `rng.range(2, 5)` are half-open.
                roof_vents: ByUse {
                    residential: (1, 2),
                    office: (2, 4),
                    other: (2, 4),
                },
                // B9: per 1000 m² of roof.
                roof_units: ByUse {
                    residential: (0.0, 0.0),
                    office: (2.0, 4.0),
                    other: if motel { (1.0, 2.0) } else { (2.0, 4.0) },
                },
                exhaust_fans: ByUse {
                    residential: (0.0, 0.0),
                    office: if motel { (0.0, 0.0) } else { (0.5, 1.0) },
                    other: if motel { (0.0, 0.0) } else { (1.0, 2.0) },
                },
                skylights: ByUse {
                    residential: (0.0, 0.0),
                    office: if style == S::Warehouse {
                        (2.0, 4.0)
                    } else {
                        (0.0, 0.0)
                    },
                    other: match style {
                        S::StripMall => (3.0, 6.0),
                        S::Warehouse => (2.0, 4.0),
                        _ => (0.0, 0.0),
                    },
                },
                bulkhead_floors: 4,
                water_tank_1_in: 2,
            },
        }
    }

    #[test]
    fn shipped_grammar_keeps_the_former_constants() {
        let g = coast_80s();
        g.validate().expect("validate");
        assert_eq!(g.era, Era::Eighties);
        assert_eq!(g.template, "california_coast");
        // Every style once for all zones, stucco and brick again for industrial zones.
        assert_eq!(g.styles.len(), STYLES.len() + 2);
        for r in &g.styles {
            let industrial = r.zones != ZoneMask::ALL;
            assert_eq!(*r, former(r.style, industrial), "{:?}", r.style);
        }
        assert_eq!(g.signs.lit_box_256, 70);
        assert_eq!(g.signs.shop_lit_256, 115);
        // Weights in listed order (B8).
        let awnings: Vec<Awning> = (0..15u64).map(|r| g.signs.awning(r)).collect();
        use Awning as A;
        let want = [
            [A::Striped; 3].as_slice(),
            &[A::Solid; 3],
            &[A::Bullnose; 2],
            &[A::Aluminum; 2],
            &[A::Canopy; 2],
            &[A::Mansard],
            &[A::None; 2],
        ]
        .concat();
        assert_eq!(awnings, want);
        assert_eq!(g.signs.mall_canopy(0), A::Mansard);
        assert_eq!(g.signs.mall_canopy(4), A::Canopy);
        // The test town's strip-mall stalls (B10's lots keep the width).
        assert_eq!(g.lots.stall_w, 2.8);
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
    fn rules_are_chosen_by_zone_in_file_order() {
        let g = coast_80s();
        let industrial = g.style(StyleTag::Brick, ZoneId::Industrial);
        let strip = g.style(StyleTag::Brick, ZoneId::Strip);
        assert_eq!(
            industrial.zones,
            ZoneMask::from(ZoneList::Only(vec![ZoneId::Industrial]))
        );
        assert_eq!(strip.zones, ZoneMask::ALL);
        // Industrial brick: loading doors, no shops; the same elsewhere.
        assert_eq!(industrial.base.ground.other.storefront, 0);
        assert!(strip.base.ground.other.storefront > 0);
        assert_eq!(industrial.massing, strip.massing);
        // Moving the `All` entry first hides the industrial one.
        let mut g2 = g.clone();
        let i = g2.styles.iter().position(|r| *r == *strip).unwrap();
        let all = g2.styles.remove(i);
        g2.styles.insert(0, all);
        assert_eq!(g2.style(StyleTag::Brick, ZoneId::Industrial), strip);
    }

    #[test]
    fn zone_lists_round_trip() {
        for l in [
            ZoneList::All,
            ZoneList::Only(vec![]),
            ZoneList::Only(vec![ZoneId::Strip, ZoneId::Beach]),
        ] {
            let m = ZoneMask::from(l.clone());
            assert_eq!(ZoneList::from(m), l);
        }
        assert!(ZoneMask::ALL.has(ZoneId::Water));
        assert!(!ZoneMask::from(ZoneList::Only(vec![ZoneId::Strip])).has(ZoneId::Beach));
    }

    #[test]
    fn pick_follows_the_weights_in_order() {
        assert_eq!(pick(&[0, 0], 5), None);
        let counts = (0..60u32).fold([0; 3], |mut c, r| {
            c[pick(&[1, 0, 2], r).unwrap()] += 1;
            c
        });
        assert_eq!(counts, [20, 0, 40]);
        assert_eq!(pick(&[1, 0, 2], 0), Some(0));
        assert_eq!(pick(&[1, 0, 2], 1), Some(2));
    }

    #[test]
    fn validate_rejects_broken_grammars() {
        let mut g = coast_80s();
        g.styles.pop();
        assert_eq!(
            g.validate(),
            Err(GrammarError::Style {
                style: StyleTag::Motel,
                zone: ZoneId::Downtown,
            })
        );
        // A zone left uncovered.
        let mut g = coast_80s();
        for r in &mut g.styles {
            if r.style == StyleTag::Barn {
                r.zones = ZoneMask::from(ZoneList::Only(vec![ZoneId::Farmland]));
            }
        }
        assert_eq!(
            g.validate(),
            Err(GrammarError::Style {
                style: StyleTag::Barn,
                zone: ZoneId::Downtown,
            })
        );
        let mut g = coast_80s();
        g.styles[0].base.storefront.glass_top = (0.5, 0.6);
        assert!(matches!(
            g.validate(),
            Err(GrammarError::Bad {
                field: "base.storefront.glass_top",
                ..
            })
        ));
        let mut g = coast_80s();
        g.styles[0].top.kinds = TopKinds::default();
        assert!(g.validate().is_err());
        let mut g = coast_80s();
        g.styles[0].body.kinds.office = WindowKinds::default();
        assert!(g.validate().is_err());
        let mut g = coast_80s();
        g.signs.awnings = vec![(Awning::Striped, u32::MAX), (Awning::None, 1)];
        assert!(g.validate().is_ok());
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
        g.signs.mall_canopies.clear();
        assert!(g.validate().is_err());
        let mut g = coast_80s();
        g.styles[0].addenda.skylights.other = (2.0, 1.0);
        assert!(g.validate().is_err());
        let mut g = coast_80s();
        g.lots.pole_m = (40.0, 30.0);
        assert!(g.validate().is_err());
        let mut g = coast_80s();
        g.lots.occupancy[0].1[12] = 101;
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
