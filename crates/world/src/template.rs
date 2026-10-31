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
//! - A [`Curve`] is a list of `(x, weight)` points with strictly increasing `x`, linearly
//!   interpolated and held flat beyond the ends; an empty curve contributes 0.
//! - Per-zone data lives in [`Zones`], one named field per land [`ZoneId`] (all of them
//!   except `Water`, which is never painted by rules). Field order matches `ZoneId` order.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use place::{EdgeKind, ZoneId};

/// `(min, max)`, inclusive.
pub type Range = (f32, f32);
/// `(min, max)` count, inclusive.
pub type CountRange = (u8, u8);
/// `(x, weight)` points, `x` strictly increasing; see the module docs.
pub type Curve = Vec<(f32, f32)>;

/// Which side of the road traffic keeps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DrivingSide {
    Right,
    Left,
}

/// The place edge the big water lies along. North is `-Z`, west is `-X`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CoastSide {
    West,
    East,
    North,
    South,
}

/// What the big water is. Both are flat at `water_level_m`; the kind selects shading,
/// waves and the shore props downstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CoastWater {
    Ocean,
    Lake,
}

/// What bounds the place on each side (`docs/WORLD_GENERATION.md`, "Map edges"). The coast
/// side must be water (`Ocean`, `Lake` or `Bay`), every other side land; none is `Open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Edges {
    pub north: EdgeKind,
    pub east: EdgeKind,
    pub south: EdgeKind,
    pub west: EdgeKind,
}

impl Edges {
    /// `[north, east, south, west]`, the order of `place::Terrain::edges`.
    pub fn sides(&self) -> [EdgeKind; 4] {
        [self.north, self.east, self.south, self.west]
    }
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

/// Canal pocket: parallel finger canals joined at both ends (stage 1, "lagoon/canal
/// network"), dug into a flattened patch behind the strip.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Canals {
    /// Probability in `[0, 1]` that a place gets a pocket.
    pub chance: f32,
    /// Number of parallel fingers.
    pub fingers: CountRange,
    /// Distance between finger centerlines.
    pub spacing_m: f32,
    /// Finger length, running inland.
    pub length_m: Range,
    pub width_m: f32,
}

/// Coastline: where the big water is and how the shore bends.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Coast {
    pub side: CoastSide,
    pub water: CoastWater,
    /// Width of open water between the place edge and the shoreline, before bays.
    pub offset_m: Range,
    /// Amplitude of the low-frequency shoreline curve.
    pub wiggle_m: f32,
    /// Number of bays. The first is the main bay the strip centers on.
    pub bays: CountRange,
    /// How far a bay cuts inland.
    pub bay_depth_m: Range,
    /// Bay width along the coast.
    pub bay_width_m: Range,
    pub canals: Option<Canals>,
}

/// Elevation profile, measured inland from the shoreline. Heights are meters above
/// `water_level_m` unless noted.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Elevation {
    /// Absolute height of the water surface.
    pub water_level_m: f32,
    /// Deepest water, below `water_level_m`.
    pub shelf_depth_m: f32,
    /// Sand between the waterline and the first street, varying along the coast.
    pub beach_width_m: Range,
    /// Height of the beach's inland edge.
    pub beach_height_m: f32,
    /// Gently rising coastal plain behind the beach.
    pub plain_width_m: Range,
    /// How much the plain rises across its width.
    pub plain_rise_m: Range,
    /// Amplitude of rolling relief on the plain and hills.
    pub relief_m: f32,
    /// Domain warp amplitude: how far ridges and valleys wander from straight lines.
    pub warp_m: f32,
    /// Ridges roughly parallel to the coast, rising inland. 0 for flat places.
    pub ridge_count: CountRange,
    /// Ridge crest height above the plain's inland edge.
    pub ridge_height_m: Range,
    /// Half-width of a ridge's cross-section.
    pub ridge_width_m: Range,
    /// Flat-floored valleys running inland from the plain into the hills.
    pub valley_count: CountRange,
    pub valley_length_m: Range,
    pub valley_width_m: Range,
    /// Valley floor height above the plain's inland edge (it rises gently along the valley
    /// up to this).
    pub valley_floor_m: Range,
}

/// The main drag, parallel to the beach and centered on the main bay.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct StripRule {
    /// Length along the coast.
    pub length_m: Range,
    /// Gap between the beach's inland edge and the strip's seaward edge, measured from the
    /// shoreline smoothed along the coast (so the strip sweeps across a bay's apex).
    pub inland_m: Range,
    /// Depth of the strip band (the street and the lots on both sides).
    pub width_m: Range,
}

/// The highway corridor: a bypass from the coast round the town and back
/// (`docs/WORLD_GENERATION.md` stage 3).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct HighwayRule {
    /// Right-of-way width painted as `Highway`.
    pub corridor_m: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ParkRule {
    /// Total parks, including the one on the shore.
    pub count: CountRange,
    pub radius_m: Range,
}

/// Placement of the structural features the zone rules hang off.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Layout {
    pub strip: StripRule,
    /// Distance of the downtown seed point behind the strip's inland edge.
    pub downtown_inland_m: Range,
    pub highway: HighwayRule,
    pub parks: ParkRule,
    /// A pier into the main bay at the strip's center.
    pub pier: bool,
    /// Scenic overlooks on high ground.
    pub overlooks: CountRange,
}

/// How strongly a zone claims a cell. Every scored zone sums its terms per cell and the
/// highest total wins (ties go to the lower `ZoneId`).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Score {
    pub base: f32,
    /// By distance inland from the shoreline, meters.
    pub coast: Curve,
    /// By terrain slope, rise over run.
    pub slope: Curve,
    /// By distance from the downtown seed point, meters. Offsets along the coast count
    /// 0.75×, so downtowns stretch along the shore.
    pub downtown: Curve,
    /// By distance from the highway centerline, meters.
    pub highway: Curve,
    /// Inland basins only (empty elsewhere): by distance out from the foot of the hills,
    /// from the rail line, from the nearest arterial and from the nearest boulevard.
    #[cfg_attr(feature = "serde", serde(default))]
    pub hills: Curve,
    #[cfg_attr(feature = "serde", serde(default))]
    pub rail: Curve,
    #[cfg_attr(feature = "serde", serde(default))]
    pub arterial: Curve,
    #[cfg_attr(feature = "serde", serde(default))]
    pub boulevard: Curve,
    /// Amplitude of the zone's own noise term, in `[-noise, noise]`.
    pub noise: f32,
    /// Feature size of that noise.
    pub noise_scale_m: f32,
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
    /// `None` for zones painted only by structure (`Beach`, `Strip`, `Highway` must be
    /// `None`; `Park` may be either: placed patches are added on top of any scored cells).
    pub score: Option<Score>,
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

/// Zones placed by structure (coast distance, the strip band, the highway corridor) rather
/// than by score.
pub const STRUCTURAL_ZONES: [ZoneId; 3] = [ZoneId::Beach, ZoneId::Strip, ZoneId::Highway];

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

/// The hills an inland basin rises into along one side: the overlook roads climb them.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct BasinHills {
    /// The side they rise toward; its edge must be `Hills`.
    pub side: CoastSide,
    /// From that side in to the foot of the hills.
    pub depth_m: Range,
    /// Height of the hills at the edge band above the floor at their foot.
    pub height_m: Range,
    /// Amplitude of the ridges and gullies on their face.
    pub relief_m: f32,
}

/// The desert fringe along one side of a basin: open dirt and section roads, where the
/// freeway leaves town as the desert highway and turns back along the edge.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct DesertRule {
    /// The side it lies along; its edge must be `Desert`, beside the hills' side.
    pub side: CoastSide,
    /// From the inside of that side's band in to the last grid street.
    pub depth_m: Range,
}

/// The flat grid of a basin's arterials (`docs/WORLD_GENERATION.md`, "Inland basins").
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct GridRule {
    /// Distance between arterials, both ways.
    pub spacing_m: f32,
    /// Lanes each way of a boulevard: two one-way carriageways either side of a raised,
    /// planted median, one boulevard each way across the grid.
    pub boulevard_lanes: u8,
    /// Width of a boulevard's median between the carriageways' curbs.
    pub median_m: f32,
}

/// The freeway across a basin: overpasses at every arterial, a few interchanges (one of
/// them with a loop on-ramp), and out past the desert edge it turns back along it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct FreewayRule {
    /// Right-of-way width painted as `Highway`.
    pub corridor_m: f32,
    /// Diamond interchanges, the loop interchange among them.
    pub interchanges: CountRange,
    /// Interchanges whose on-ramp in one quadrant is a loop.
    pub loops: u8,
}

/// The concrete flood channel: a trapezoid cut from the foot of the hills to the far edge,
/// bridged by every road over it, with ramps down its walls.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ChannelRule {
    /// Depth of the bed below the ground (under the 8 m a road profile may stand off the
    /// terrain, `place::validate`, so a bridge over it stays valid).
    pub depth_m: f32,
    /// Width of the flat bed.
    pub bed_m: f32,
    /// Horizontal run of each wall from the lip to the bed.
    pub wall_m: f32,
    /// Right-of-way painted as `Highway`: the channel and its fenced banks.
    pub row_m: f32,
    /// A ramp down into the bed at the first bridge past every this many meters along it.
    pub ramp_every_m: f32,
    /// Grade of a ramp.
    pub ramp_grade: f32,
}

/// The rail line across a basin, at grade with every road it crosses, with spurs off it in
/// the industrial land.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct RailRule {
    /// Right-of-way width painted as `Highway`.
    pub row_m: f32,
    /// Spurs: sidings off the main line, beside it, each ending at a bumper.
    pub spurs: CountRange,
    pub spur_m: Range,
}

/// The airfield on the basin's far edge: a runway (the future drag strip) on open ground.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct AirfieldRule {
    pub length_m: Range,
    /// The open field across the runway.
    pub width_m: f32,
    /// Runway pavement width.
    pub runway_m: f32,
}

/// An inland basin (`socal_basin`): no coast; a flat floor tilting up to hills on one side,
/// a desert on another, laid out as a grid with a freeway, a flood channel, a rail line and
/// an airfield (`docs/WORLD_GENERATION.md`, "Inland basins").
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Basin {
    /// Below all ground: there is no water.
    pub water_level_m: f32,
    /// Absolute height of the floor at the side opposite the hills.
    pub floor_m: Range,
    /// How fast the floor rises toward the hills (rise over run).
    pub tilt: f32,
    /// Amplitude of the floor's rolling relief.
    pub relief_m: f32,
    pub hills: BasinHills,
    pub desert: DesertRule,
    pub grid: GridRule,
    /// The downtown seed: how far out from the foot of the hills, and where along them as
    /// a fraction of the way from the side opposite the desert.
    pub downtown_m: Range,
    pub downtown_along: Range,
    pub freeway: FreewayRule,
    pub channel: ChannelRule,
    pub rail: RailRule,
    pub airfield: AirfieldRule,
    pub parks: ParkRule,
    /// Scenic overlooks on the hills.
    pub overlooks: CountRange,
}

/// A kind of place: everything world generation needs besides the seed and the era. A
/// coastal place has `coast`, `elevation` and `layout`; an inland basin has `basin` and
/// none of them.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct RegionTemplate {
    /// Identifier, e.g. `california_coast`; matches the file name.
    pub name: String,
    pub driving_side: DrivingSide,
    pub sign_language: SignLanguage,
    pub era: EraDefaults,
    #[cfg_attr(feature = "serde", serde(default))]
    pub coast: Option<Coast>,
    pub edges: Edges,
    #[cfg_attr(feature = "serde", serde(default))]
    pub elevation: Option<Elevation>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub layout: Option<Layout>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub basin: Option<Basin>,
    pub zones: Zones,
}

impl RegionTemplate {
    /// The coast of a coastal template (one that passed [`RegionTemplate::validate`]
    /// without a `basin`).
    pub fn coast(&self) -> &Coast {
        self.coast.as_ref().expect("a coastal template")
    }

    /// The elevation profile of a coastal template.
    pub fn elevation(&self) -> &Elevation {
        self.elevation.as_ref().expect("a coastal template")
    }

    /// The layout of a coastal template.
    pub fn layout(&self) -> &Layout {
        self.layout.as_ref().expect("a coastal template")
    }
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
    /// A curve point is NaN/infinite, or its `x` does not strictly increase at `index`.
    BadCurve {
        zone: ZoneId,
        curve: &'static str,
        index: usize,
    },
    /// `Beach`, `Strip` and `Highway` are placed by structure and must not be scored.
    StructuralZoneScored {
        zone: ZoneId,
    },
    /// No zone has a score, so nothing could fill the land.
    NoScoredZones,
    BadScore {
        zone: ZoneId,
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
    /// The coast side's edge is not water, another side's is, or one is `Open`.
    BadEdge {
        field: &'static str,
    },
    /// Neither a coast (`coast`, `elevation` and `layout`) nor a `basin`, or parts of both.
    BadShape,
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
            TemplateError::BadCurve { zone, curve, index } => write!(
                f,
                "zones.{zone:?}.score.{curve}[{index}]: points must be finite with x strictly increasing"
            ),
            TemplateError::StructuralZoneScored { zone } => {
                write!(f, "zones.{zone:?}.score must be None (placed by structure)")
            }
            TemplateError::NoScoredZones => write!(f, "no zone has a score"),
            TemplateError::BadScore { zone, field } => {
                write!(f, "zones.{zone:?}.score.{field} is invalid")
            }
            TemplateError::BadBlockSize { zone } => {
                write!(f, "zones.{zone:?}.block_m: ranges must be positive, min <= max")
            }
            TemplateError::BadTreeWeight { zone, index } => {
                write!(f, "zones.{zone:?}.trees[{index}]: weight must be finite and > 0")
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
            TemplateError::BadEdge { field } => write!(
                f,
                "{field}: the coast side must be water like coast.water, the others land"
            ),
            TemplateError::BadShape => write!(
                f,
                "a template has coast, elevation and layout, or a basin and none of them"
            ),
        }
    }
}

/// Linear interpolation of `curve` at `x`, held flat beyond the ends; 0 when empty.
pub fn curve_at(curve: &[(f32, f32)], x: f32) -> f32 {
    let (first, last) = match (curve.first(), curve.last()) {
        (Some(f), Some(l)) => (*f, *l),
        _ => return 0.0,
    };
    if x <= first.0 {
        return first.1;
    }
    if x >= last.0 {
        return last.1;
    }
    for w in curve.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return y0 + (y1 - y0) * ((x - x0) / (x1 - x0));
        }
    }
    last.1
}

fn finite(field: &'static str, x: f32) -> Result<(), TemplateError> {
    if x.is_finite() {
        Ok(())
    } else {
        Err(TemplateError::NonFinite { field })
    }
}

/// `lo <= x <= hi`.
fn within(field: &'static str, x: f32, lo: f32, hi: f32) -> Result<(), TemplateError> {
    finite(field, x)?;
    if x < lo || x > hi {
        return Err(TemplateError::OutOfRange { field });
    }
    Ok(())
}

/// A `(min, max)` range with `floor <= min <= max`.
fn range(field: &'static str, r: Range, floor: f32) -> Result<(), TemplateError> {
    finite(field, r.0)?;
    finite(field, r.1)?;
    if r.0 < floor || r.0 > r.1 {
        return Err(TemplateError::BadRange { field });
    }
    Ok(())
}

/// A positive `(min, max)` range.
fn positive(field: &'static str, r: Range) -> Result<(), TemplateError> {
    range(field, r, 0.0)?;
    if r.0 <= 0.0 {
        return Err(TemplateError::BadRange { field });
    }
    Ok(())
}

fn count(field: &'static str, r: CountRange) -> Result<(), TemplateError> {
    if r.0 > r.1 {
        return Err(TemplateError::BadRange { field });
    }
    Ok(())
}

fn curve(zone: ZoneId, name: &'static str, c: &[(f32, f32)]) -> Result<(), TemplateError> {
    for (index, &(x, y)) in c.iter().enumerate() {
        let increasing = index == 0 || x > c[index - 1].0;
        if !x.is_finite() || !y.is_finite() || !increasing {
            return Err(TemplateError::BadCurve {
                zone,
                curve: name,
                index,
            });
        }
    }
    Ok(())
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

        match (&self.coast, &self.elevation, &self.layout, &self.basin) {
            (Some(c), Some(e), Some(l), None) => self.validate_coast(c, e, l)?,
            (None, None, None, Some(b)) => self.validate_basin(b)?,
            _ => return Err(TemplateError::BadShape),
        }

        let mut scored = 0;
        for zone in LAND_ZONES {
            let Some(rule) = self.zones.get(zone) else {
                continue;
            };
            validate_zone(zone, rule)?;
            if rule.score.is_some() {
                scored += 1;
            }
        }
        if scored == 0 {
            return Err(TemplateError::NoScoredZones);
        }
        Ok(())
    }

    fn validate_coast(
        &self,
        coast: &Coast,
        elevation: &Elevation,
        layout: &Layout,
    ) -> Result<(), TemplateError> {
        let c = coast;
        positive("coast.offset_m", c.offset_m)?;
        within("coast.wiggle_m", c.wiggle_m, 0.0, 500.0)?;
        count("coast.bays", c.bays)?;
        range("coast.bay_depth_m", c.bay_depth_m, 0.0)?;
        positive("coast.bay_width_m", c.bay_width_m)?;
        if let Some(k) = &c.canals {
            within("coast.canals.chance", k.chance, 0.0, 1.0)?;
            count("coast.canals.fingers", k.fingers)?;
            if k.fingers.0 < 1 {
                return Err(TemplateError::BadRange {
                    field: "coast.canals.fingers",
                });
            }
            within("coast.canals.width_m", k.width_m, 4.0, 60.0)?;
            within(
                "coast.canals.spacing_m",
                k.spacing_m,
                2.0 * k.width_m,
                400.0,
            )?;
            positive("coast.canals.length_m", k.length_m)?;
        }

        let coast = match c.side {
            CoastSide::North => 0,
            CoastSide::East => 1,
            CoastSide::South => 2,
            CoastSide::West => 3,
        };
        let names = ["edges.north", "edges.east", "edges.south", "edges.west"];
        for (i, (kind, field)) in self.edges.sides().into_iter().zip(names).enumerate() {
            let wrong_water = matches!(
                (kind, c.water),
                (EdgeKind::Ocean, CoastWater::Lake) | (EdgeKind::Lake, CoastWater::Ocean)
            );
            if kind == EdgeKind::Open || kind.is_water() != (i == coast) || wrong_water {
                return Err(TemplateError::BadEdge { field });
            }
        }

        let e = elevation;
        within("elevation.water_level_m", e.water_level_m, -100.0, 1000.0)?;
        within("elevation.shelf_depth_m", e.shelf_depth_m, 0.5, 200.0)?;
        positive("elevation.beach_width_m", e.beach_width_m)?;
        within("elevation.beach_height_m", e.beach_height_m, 0.2, 20.0)?;
        positive("elevation.plain_width_m", e.plain_width_m)?;
        range("elevation.plain_rise_m", e.plain_rise_m, 0.0)?;
        within("elevation.relief_m", e.relief_m, 0.0, 50.0)?;
        within("elevation.warp_m", e.warp_m, 0.0, 1000.0)?;
        count("elevation.ridge_count", e.ridge_count)?;
        if e.ridge_count.1 > 6 {
            return Err(TemplateError::BadRange {
                field: "elevation.ridge_count",
            });
        }
        range("elevation.ridge_height_m", e.ridge_height_m, 0.0)?;
        positive("elevation.ridge_width_m", e.ridge_width_m)?;
        count("elevation.valley_count", e.valley_count)?;
        if e.valley_count.1 > 4 {
            return Err(TemplateError::BadRange {
                field: "elevation.valley_count",
            });
        }
        positive("elevation.valley_length_m", e.valley_length_m)?;
        positive("elevation.valley_width_m", e.valley_width_m)?;
        range("elevation.valley_floor_m", e.valley_floor_m, 0.0)?;

        let l = layout;
        positive("layout.strip.length_m", l.strip.length_m)?;
        range("layout.strip.inland_m", l.strip.inland_m, 0.0)?;
        positive("layout.strip.width_m", l.strip.width_m)?;
        range("layout.downtown_inland_m", l.downtown_inland_m, 0.0)?;
        within(
            "layout.highway.corridor_m",
            l.highway.corridor_m,
            8.0,
            200.0,
        )?;
        count("layout.parks.count", l.parks.count)?;
        positive("layout.parks.radius_m", l.parks.radius_m)?;
        count("layout.overlooks", l.overlooks)?;

        Ok(())
    }

    fn validate_basin(&self, b: &Basin) -> Result<(), TemplateError> {
        within("basin.water_level_m", b.water_level_m, -1000.0, 1000.0)?;
        range("basin.floor_m", b.floor_m, b.water_level_m + 2.0)?;
        within("basin.tilt", b.tilt, 0.0, 0.03)?;
        within("basin.relief_m", b.relief_m, 0.0, 10.0)?;
        positive("basin.hills.depth_m", b.hills.depth_m)?;
        range("basin.hills.height_m", b.hills.height_m, 0.0)?;
        within("basin.hills.relief_m", b.hills.relief_m, 0.0, 60.0)?;
        positive("basin.desert.depth_m", b.desert.depth_m)?;
        let g = &b.grid;
        within("basin.grid.spacing_m", g.spacing_m, 200.0, 800.0)?;
        if !(1..=4).contains(&g.boulevard_lanes) {
            return Err(TemplateError::OutOfRange {
                field: "basin.grid.boulevard_lanes",
            });
        }
        within("basin.grid.median_m", g.median_m, 2.0, 20.0)?;
        positive("basin.downtown_m", b.downtown_m)?;
        range("basin.downtown_along", b.downtown_along, 0.0)?;
        within("basin.downtown_along", b.downtown_along.1, 0.0, 1.0)?;
        let f = &b.freeway;
        within("basin.freeway.corridor_m", f.corridor_m, 24.0, 200.0)?;
        count("basin.freeway.interchanges", f.interchanges)?;
        if f.loops > f.interchanges.0 {
            return Err(TemplateError::OutOfRange {
                field: "basin.freeway.loops",
            });
        }
        let c = &b.channel;
        // A bridge's profile may stand at most 8 m over the terrain.
        within("basin.channel.depth_m", c.depth_m, 1.0, 7.8)?;
        within("basin.channel.bed_m", c.bed_m, 4.0, 60.0)?;
        within(
            "basin.channel.wall_m",
            c.wall_m,
            0.5 * c.depth_m,
            4.0 * c.depth_m,
        )?;
        within(
            "basin.channel.row_m",
            c.row_m,
            c.bed_m + 2.0 * c.wall_m + 4.0,
            200.0,
        )?;
        within("basin.channel.ramp_every_m", c.ramp_every_m, 200.0, 5000.0)?;
        within("basin.channel.ramp_grade", c.ramp_grade, 0.04, 0.15)?;
        let r = &b.rail;
        within("basin.rail.row_m", r.row_m, 12.0, 100.0)?;
        count("basin.rail.spurs", r.spurs)?;
        positive("basin.rail.spur_m", r.spur_m)?;
        let a = &b.airfield;
        positive("basin.airfield.length_m", a.length_m)?;
        within("basin.airfield.runway_m", a.runway_m, 10.0, 60.0)?;
        within(
            "basin.airfield.width_m",
            a.width_m,
            a.runway_m + 20.0,
            400.0,
        )?;
        count("basin.parks.count", b.parks.count)?;
        positive("basin.parks.radius_m", b.parks.radius_m)?;
        count("basin.overlooks", b.overlooks)?;
        // Every side land; the hills' side `Hills` and the desert's `Desert`, beside it.
        let index = |s: CoastSide| match s {
            CoastSide::North => 0,
            CoastSide::East => 1,
            CoastSide::South => 2,
            CoastSide::West => 3,
        };
        let names = ["edges.north", "edges.east", "edges.south", "edges.west"];
        let sides = self.edges.sides();
        for (kind, field) in sides.into_iter().zip(names) {
            if kind == EdgeKind::Open || kind.is_water() {
                return Err(TemplateError::BadEdge { field });
            }
        }
        let (h, d) = (index(b.hills.side), index(b.desert.side));
        if sides[h] != EdgeKind::Hills {
            return Err(TemplateError::BadEdge { field: names[h] });
        }
        if sides[d] != EdgeKind::Desert || (h + d) % 2 == 0 {
            return Err(TemplateError::BadEdge { field: names[d] });
        }
        Ok(())
    }

    /// The lowest and highest terrain heights this template can produce, the edge walls
    /// on top of everything else; generation clamps to them.
    pub fn height_bounds(&self) -> (f32, f32) {
        let wall = self
            .edges
            .sides()
            .iter()
            .filter_map(|&k| crate::edges::wall(k))
            .fold(0.0f32, |a, w| a.max(w.height_m.1));
        let Some(e) = &self.elevation else {
            let b = self.basin.as_ref().expect("a coast or a basin");
            // The floor tilts up across the whole place toward the hills: 4096 m covers places
            // up to 32 chunks a side (v1 places are 16).
            let hi = b.floor_m.1
                + b.tilt * 4096.0
                + 2.0 * b.relief_m
                + b.hills.height_m.1
                + 2.0 * b.hills.relief_m
                + wall
                + 4.0;
            return (b.water_level_m, hi);
        };
        let lo = e.water_level_m - e.shelf_depth_m;
        let hi = e.water_level_m
            + e.beach_height_m
            + e.plain_rise_m.1
            + e.ridge_height_m.1.max(e.valley_floor_m.1)
            + 2.0 * e.relief_m
            + wall
            + 4.0;
        (lo, hi)
    }
}

fn validate_zone(zone: ZoneId, rule: &ZoneRule) -> Result<(), TemplateError> {
    if let Some(s) = &rule.score {
        if STRUCTURAL_ZONES.contains(&zone) {
            return Err(TemplateError::StructuralZoneScored { zone });
        }
        if !s.base.is_finite() {
            return Err(TemplateError::BadScore {
                zone,
                field: "base",
            });
        }
        curve(zone, "coast", &s.coast)?;
        curve(zone, "slope", &s.slope)?;
        curve(zone, "downtown", &s.downtown)?;
        curve(zone, "highway", &s.highway)?;
        curve(zone, "hills", &s.hills)?;
        curve(zone, "rail", &s.rail)?;
        curve(zone, "arterial", &s.arterial)?;
        curve(zone, "boulevard", &s.boulevard)?;
        if !(s.noise.is_finite() && s.noise >= 0.0) {
            return Err(TemplateError::BadScore {
                zone,
                field: "noise",
            });
        }
        if !(s.noise_scale_m.is_finite() && s.noise_scale_m >= 16.0) {
            return Err(TemplateError::BadScore {
                zone,
                field: "noise_scale_m",
            });
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn curves_interpolate_and_hold() {
        let c = vec![(0.0, 1.0), (10.0, 3.0), (20.0, -1.0)];
        assert_eq!(curve_at(&c, -5.0), 1.0);
        assert_eq!(curve_at(&c, 5.0), 2.0);
        assert_eq!(curve_at(&c, 15.0), 1.0);
        assert_eq!(curve_at(&c, 99.0), -1.0);
        assert_eq!(curve_at(&[], 3.0), 0.0);
    }
}
