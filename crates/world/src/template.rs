//! Region templates: the data that makes one kind of place unlike another
//! (`docs/WORLD_GENERATION.md`, "Region templates").
//!
//! A [`RegionTemplate`] is parsed from RON (`assets/regions/*.ron`) by `app` and the tools;
//! this crate only defines the types (with serde derives behind the `serde` feature) and
//! [`RegionTemplate::validate`]. RON struct names are the Rust type names.
//!
//! Conventions:
//! - A `(f32, f32)` range is `(min, max)`, inclusive; generation draws uniformly from it.
//! - A `(u8, u8)` count range is `(min, max)`, inclusive.
//! - Per-zone data lives in [`Zones`], one named field per land [`ZoneId`] (all of them
//!   except `Water`, which is never painted by rules). Field order matches `ZoneId` order.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use place::ZoneId;

/// `(min, max)`, inclusive.
pub type Range = (f32, f32);
/// `(min, max)` count, inclusive.
pub type CountRange = (u8, u8);
/// Which side of the road traffic keeps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DrivingSide {
    Right,
    Left,
}

/// Local street layout of a zone (`docs/WORLD_GENERATION.md` stage 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StreetPattern {
    /// Rectangular blocks aligned to the arterials.
    Grid,
    /// Collector loops with cul-de-sacs.
    Curvy,
    /// Contour-following switchbacks, sparse.
    Switchback,
    /// Straight section roads on a mile/half-mile grid.
    Sections,
    /// Very large blocks with truck-width roads.
    Industrial,
    /// The beachfront road, parking lots and a pier road.
    Beachfront,
    /// No local streets: open land the main roads cross (an authored layout's open space).
    Open,
}

/// Street and yard tree species. Shapes come from the shared prop library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Tree {
    /// Tall, skinny fan palm.
    Palm,
    /// Short, fat-trunked palm with a dense crown.
    FanPalm,
    Jacaranda,
    Oak,
    Eucalyptus,
    Sycamore,
    Pine,
    Cypress,
    Elm,
    Maple,
    Ash,
    Linden,
    Honeylocust,
    /// Rows of small fruit or nut trees.
    Orchard,
}

/// Building style families a zone draws from. The era layer maps them onto the Place's
/// `StyleTag`s and facade palettes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BuildingStyle {
    StuccoBox,
    Brick,
    Deco,
    StripMall,
    /// Two- or three-storey stucco apartments over carports.
    Dingbat,
    Craftsman,
    Ranch,
    Tract70s,
    Glass,
    Warehouse,
    Barn,
    Motel,
    /// Narrow one-and-a-half storey brick house with a low hipped roof.
    Bungalow,
    /// Three-storey masonry walk-up, one flat per floor.
    ThreeFlat,
    /// Mid-century office tower with a regular curtain grid.
    Tower,
    /// Raw concrete, deep window reveals.
    Brutalist,
}

/// Language of street signs, shop signs and billboards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SignLanguage {
    /// US English.
    EnUs,
}

/// Street light lamp type, which sets their color at night.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StreetLight {
    Incandescent,
    MercuryVapor,
    Sodium,
    Led,
}

/// Defaults for the era layer when a scenario does not override them.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct EraDefaults {
    pub year: u16,
    pub street_lights: StreetLight,
    /// Name of the palette file the scenario would pick by default.
    pub palette: String,
}

/// The smallest nonzero prop spacing a template may ask for: the detail pass steps along every
/// street side by it.
pub const MIN_PROP_SPACING_M: f32 = 1.0;

/// Street furniture and planting densities. A spacing of 0 means none, otherwise at least
/// [`MIN_PROP_SPACING_M`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Props {
    pub street_light_m: f32,
    pub street_tree_m: f32,
    pub hydrant_m: f32,
    pub power_pole_m: f32,
    pub bench_per_km: f32,
    pub billboard_per_km: f32,
    /// Fraction in `[0, 1]` of buildings with a neon sign.
    pub neon: f32,
    /// Fraction of curb length with parked cars.
    pub parked_cars: Range,
}

/// Everything a template says about one zone.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ZoneRule {
    /// Block `(length range, depth range)` in meters.
    pub block_m: (Range, Range),
    pub streets: StreetPattern,
    pub alleys: bool,
    pub styles: Vec<BuildingStyle>,
    /// `(species, relative weight)`.
    pub trees: Vec<(Tree, f32)>,
    pub props: Props,
    pub lots: Lots,
    /// How the zone's lots are built on (stage 6); `None` (the default): no buildings.
    #[cfg_attr(feature = "serde", serde(default))]
    pub massing: Option<Massing>,
    /// Steepest grade of the zone's local streets; `None` (the default): the usual 12%. A
    /// city laid straight over its hills climbs steeper.
    #[cfg_attr(feature = "serde", serde(default))]
    pub street_grade: Option<f32>,
}

/// How a zone's lots are built on (`docs/WORLD_GENERATION.md` stage 6): how much of each
/// lot the footprints cover, how tall buildings are and which of the zone's `styles` are
/// likely. Styles with a fixed height of their own (strip malls, motels, bungalows,
/// ranches, dingbats, warehouses, barns) ignore the floor distribution.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Massing {
    /// Target share in `(0, 1]` of a lot's area under footprints. Lots that cannot reach
    /// it build what fits; larger footprints are shrunk to it.
    pub coverage: f32,
    /// Storeys `(min, max)`, at least 1.
    pub floors: CountRange,
    /// Mean storeys of a log-normal draw, clamped to `floors`.
    pub floors_mean: f32,
    /// Log-normal sigma of the storey draw, in `[0, 2]` (0: always the mean, rounded).
    pub floors_spread: f32,
    /// Relative weight of each entry of `styles`, parallel to it; empty: equal weights.
    pub style_weights: Vec<f32>,
}

/// How a zone's blocks are cut into parcels (`docs/WORLD_GENERATION.md` stage 5) and where
/// buildings sit in them (stage 6).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Lots {
    /// Lot width along the frontage. In `Farmland` it is the width of a field.
    pub width_m: Range,
    /// Deepest lot, measured from the frontage. Blocks deeper than about twice this keep
    /// an interior core (`Park` or `LotEmpty`); thinner ones are split back to back.
    pub depth_m: f32,
    /// Building setbacks from the `(front, side, rear)` lot lines.
    pub setback_m: (f32, f32, f32),
}

/// One rule per land zone, in `ZoneId` order.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Zones {
    pub downtown: ZoneRule,
    pub strip: ZoneRule,
    pub residential: ZoneRule,
    pub suburb: ZoneRule,
    pub industrial: ZoneRule,
    pub commercial: ZoneRule,
    pub park: ZoneRule,
    pub beach: ZoneRule,
    pub farmland: ZoneRule,
    pub hills: ZoneRule,
    pub highway: ZoneRule,
}

/// The land zones in `ZoneId` order: every zone except `Water`.
pub const LAND_ZONES: [ZoneId; 11] = [
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
];

impl Zones {
    /// The rule for `zone`; `None` for `Water`.
    pub fn get(&self, zone: ZoneId) -> Option<&ZoneRule> {
        Some(match zone {
            ZoneId::Downtown => &self.downtown,
            ZoneId::Strip => &self.strip,
            ZoneId::Residential => &self.residential,
            ZoneId::Suburb => &self.suburb,
            ZoneId::Industrial => &self.industrial,
            ZoneId::Commercial => &self.commercial,
            ZoneId::Park => &self.park,
            ZoneId::Beach => &self.beach,
            ZoneId::Farmland => &self.farmland,
            ZoneId::Hills => &self.hills,
            ZoneId::Highway => &self.highway,
            ZoneId::Water => return None,
        })
    }
}

/// A landmark kit and the variant (by letter) a place sites by its start.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct RoadsideKit {
    pub kit: String,
    pub variant: char,
}

/// A kind of place: the zone rules that dress an authored layout (`crate::layout`,
/// `docs/LAYOUTS.md`), which draws the terrain, the zones and the main roads itself.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct RegionTemplate {
    /// Identifier, e.g. `pacific_shore`; matches the file name.
    pub name: String,
    pub driving_side: DrivingSide,
    pub sign_language: SignLanguage,
    pub era: EraDefaults,
    /// Landmark kits a place sites by its start, in order: each on the commercial or strip
    /// lot on an arterial nearest the start that fits it (`docs/WORLD_GENERATION.md`,
    /// "Hero landmarks").
    #[cfg_attr(feature = "serde", serde(default))]
    pub roadside: Vec<RoadsideKit>,
    /// Side streets never dead-end except in the zones whose pattern has them (`Curvy`,
    /// `Switchback`, `Beachfront`): one that stops short (at a river, a right of way, a
    /// park) is cut back to its last crossing. Off by default.
    #[cfg_attr(feature = "serde", serde(default))]
    pub through_streets: bool,
    pub zones: Zones,
}

/// Why [`RegionTemplate::validate`] rejected a template. Checks run in field order and the
/// first failure is reported. `field` names are dotted RON paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateError {
    EmptyName,
    EmptyPalette,
    /// `era.year` outside 1900..=2100.
    EraYear,
    /// A number is NaN or infinite.
    NonFinite {
        field: &'static str,
    },
    /// A range has `min > max` or `min` below the field's floor (0 for lengths).
    BadRange {
        field: &'static str,
    },
    /// A scalar is outside its allowed interval.
    OutOfRange {
        field: &'static str,
    },
    BadBlockSize {
        zone: ZoneId,
    },
    BadTreeWeight {
        zone: ZoneId,
        index: usize,
    },
    BadProps {
        zone: ZoneId,
        field: &'static str,
    },
    BadLots {
        zone: ZoneId,
        field: &'static str,
    },
    BadMassing {
        zone: ZoneId,
        field: &'static str,
    },
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            TemplateError::EmptyName => write!(f, "name is empty"),
            TemplateError::EmptyPalette => write!(f, "era.palette is empty"),
            TemplateError::EraYear => write!(f, "era.year must be within 1900..=2100"),
            TemplateError::NonFinite { field } => write!(f, "{field} is not finite"),
            TemplateError::BadRange { field } => {
                write!(f, "{field}: range must be (min, max) with 0 <= min <= max")
            }
            TemplateError::OutOfRange { field } => write!(f, "{field} is out of range"),
            TemplateError::BadBlockSize { zone } => {
                write!(
                    f,
                    "zones.{zone:?}.block_m: ranges must be positive, min <= max"
                )
            }
            TemplateError::BadTreeWeight { zone, index } => {
                write!(
                    f,
                    "zones.{zone:?}.trees[{index}]: weight must be finite and > 0"
                )
            }
            TemplateError::BadProps { zone, field } => {
                write!(f, "zones.{zone:?}.props.{field} is invalid")
            }
            TemplateError::BadLots { zone, field } => {
                write!(f, "zones.{zone:?}.lots.{field} is invalid")
            }
            TemplateError::BadMassing { zone, field } => {
                write!(f, "zones.{zone:?}.massing.{field} is invalid")
            }
        }
    }
}

impl RegionTemplate {
    /// Checks every field; generation assumes a template that passed.
    pub fn validate(&self) -> Result<(), TemplateError> {
        if self.name.trim().is_empty() {
            return Err(TemplateError::EmptyName);
        }
        if self.era.palette.trim().is_empty() {
            return Err(TemplateError::EmptyPalette);
        }
        if !(1900..=2100).contains(&self.era.year) {
            return Err(TemplateError::EraYear);
        }

        let letter = |r: &RoadsideKit| r.variant.is_ascii_uppercase() && !r.kit.is_empty();
        if !self.roadside.iter().all(letter) {
            return Err(TemplateError::OutOfRange { field: "roadside" });
        }
        for zone in LAND_ZONES {
            if let Some(rule) = self.zones.get(zone) {
                validate_zone(zone, rule)?;
            }
        }
        Ok(())
    }
}

fn finite(field: &'static str, x: f32) -> Result<(), TemplateError> {
    if x.is_finite() {
        Ok(())
    } else {
        Err(TemplateError::NonFinite { field })
    }
}

fn range(field: &'static str, r: Range, floor: f32) -> Result<(), TemplateError> {
    finite(field, r.0)?;
    finite(field, r.1)?;
    if r.0 < floor || r.0 > r.1 {
        return Err(TemplateError::BadRange { field });
    }
    Ok(())
}

fn positive(field: &'static str, r: Range) -> Result<(), TemplateError> {
    range(field, r, 0.0)?;
    if r.0 <= 0.0 {
        return Err(TemplateError::BadRange { field });
    }
    Ok(())
}

fn validate_zone(zone: ZoneId, rule: &ZoneRule) -> Result<(), TemplateError> {
    let (len, depth) = rule.block_m;
    if positive("block_m", len).is_err() || positive("block_m", depth).is_err() {
        return Err(TemplateError::BadBlockSize { zone });
    }
    for (index, &(_, w)) in rule.trees.iter().enumerate() {
        if !(w.is_finite() && w > 0.0) {
            return Err(TemplateError::BadTreeWeight { zone, index });
        }
    }
    let p = &rule.props;
    let spacings = [
        ("street_light_m", p.street_light_m),
        ("street_tree_m", p.street_tree_m),
        ("hydrant_m", p.hydrant_m),
        ("power_pole_m", p.power_pole_m),
    ];
    for (field, x) in spacings {
        if !(x.is_finite() && (x == 0.0 || x >= MIN_PROP_SPACING_M)) {
            return Err(TemplateError::BadProps { zone, field });
        }
    }
    let densities = [
        ("bench_per_km", p.bench_per_km),
        ("billboard_per_km", p.billboard_per_km),
    ];
    for (field, x) in densities {
        if !(x.is_finite() && x >= 0.0) {
            return Err(TemplateError::BadProps { zone, field });
        }
    }
    if !(p.neon.is_finite() && (0.0..=1.0).contains(&p.neon)) {
        return Err(TemplateError::BadProps {
            zone,
            field: "neon",
        });
    }
    let (a, b) = p.parked_cars;
    if !(a.is_finite() && b.is_finite() && 0.0 <= a && a <= b && b <= 1.0) {
        return Err(TemplateError::BadProps {
            zone,
            field: "parked_cars",
        });
    }
    let l = &rule.lots;
    let (w0, w1) = l.width_m;
    // Lots narrower than 4 m could not hold a building; widths below 8 m are slivers.
    if !(w0.is_finite() && w1.is_finite() && 4.0 <= w0 && w0 <= w1 && w1 <= 1000.0) {
        return Err(TemplateError::BadLots {
            zone,
            field: "width_m",
        });
    }
    if !(l.depth_m.is_finite() && (8.0..=1000.0).contains(&l.depth_m)) {
        return Err(TemplateError::BadLots {
            zone,
            field: "depth_m",
        });
    }
    let (a, b, c) = l.setback_m;
    if ![a, b, c]
        .iter()
        .all(|x| x.is_finite() && (0.0..=100.0).contains(x))
    {
        return Err(TemplateError::BadLots {
            zone,
            field: "setback_m",
        });
    }
    if let Some(m) = &rule.massing {
        validate_massing(zone, m, rule.styles.len())?;
    }
    Ok(())
}

fn validate_massing(zone: ZoneId, m: &Massing, styles: usize) -> Result<(), TemplateError> {
    let bad = |field| Err(TemplateError::BadMassing { zone, field });
    if !(m.coverage.is_finite() && m.coverage > 0.0 && m.coverage <= 1.0) {
        return bad("coverage");
    }
    let (lo, hi) = m.floors;
    if lo < 1 || lo > hi || hi > 60 {
        return bad("floors");
    }
    let mean = m.floors_mean;
    if !(mean.is_finite() && f32::from(lo) <= mean && mean <= f32::from(hi)) {
        return bad("floors_mean");
    }
    if !(m.floors_spread.is_finite() && (0.0..=2.0).contains(&m.floors_spread)) {
        return bad("floors_spread");
    }
    let w = &m.style_weights;
    let finite = w.iter().all(|x| x.is_finite() && *x >= 0.0);
    if !w.is_empty() && (w.len() != styles || !finite || !w.iter().any(|&x| x > 0.0)) {
        return bad("style_weights");
    }
    Ok(())
}
