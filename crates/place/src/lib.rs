//! Plain-data description of a Skywave place.
//!
//! A [`Place`] is what world generation (or the real-data compiler) produces and what the
//! world, traffic and renderer consume. Coordinates are meters in a right-handed, Y-up world:
//! 2D positions are `[x, z]` on the ground plane, X east, Z south; 3D positions are
//! `[x, y, z]`. Names and layers follow `docs/PLACES.md`: the **skeleton** (header, terrain,
//! zones, road graph, parcels, buildings, POIs, spawns) that every producer emits, and the
//! **detail** tables (lanes, sidewalks, facades, props, lights) that `world::detail` adds and
//! that may be empty.
//!
//! The place covers `size_chunks` chunks of `chunk_size_m`; chunk `(0, 0)` is the one with
//! the smallest x and z and starts at [`PlaceHeader::origin_m`]. Every object table is
//! sorted by chunk ([`Place::sort_by_chunk`]); an object belongs to the chunk of its
//! centroid (the mean of its vertices, see [`Place::chunk_of`]). Producers split roads at
//! chunk borders (a node on the border, one edge on each side) so that a chunk's edges stay
//! inside it; this crate does not split, it only sorts and indexes. [`codec`] is the
//! canonical binary encoding with its per-chunk [`ChunkIndex`]; [`validate`] checks the
//! invariants consumers rely on.
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod codec;
mod town;
pub mod validate;

use alloc::vec;
use alloc::vec::Vec;

pub use codec::{decode, decode_chunk, encode, load, read_index, ChunkData};

/// Format version of the [`Place`] layout and of [`codec`].
pub const PLACE_VERSION: u32 = 4;
/// A [`PropKind::ParkedCar`] parked at every hour of the day.
pub const ALL_HOURS: u32 = (1 << 24) - 1;

/// The first four bytes of every encoded place.
pub const PLACE_MAGIC: [u8; 4] = *b"SKYW";

/// Edge length of a streaming chunk in v1 places, meters.
pub const CHUNK_SIZE_M: f32 = 128.0;

/// Most chunks a place may have along either axis.
pub const MAX_SIZE_CHUNKS: u16 = 256;

/// Terrain cells per chunk edge unless a producer asks otherwise: 4 m cells.
pub const DEFAULT_RES_PER_CHUNK: u16 = 32;

/// Width of each sidewalk of an [`Edge`] with `sidewalk: true`, in meters. Sidewalks lie
/// outside `width_m`, one on each side; the outer sidewalk edge is the property line.
pub const SIDEWALK_WIDTH_M: f32 = 2.5;

/// Region template a place was generated from (or that a compiled place borrows its
/// detailing from).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RegionId {
    /// The hand-laid [`Place::test_grid`].
    TestTown = 0,
    CaliforniaCoast = 1,
    GreatLakesCity = 2,
    NorthShoreSuburb = 3,
    SocalBasin = 4,
}

/// The decade a place is dressed for; selects the detailing palette (`docs/PLACES.md`, "Era
/// layer"). The skeleton itself is era-neutral.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Era {
    Seventies = 0,
    Eighties = 1,
    Nineties = 2,
    Aughts = 3,
    Tens = 4,
    Twenties = 5,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PlaceHeader {
    pub version: u32,
    /// Root seed of the place. The detailing pass derives all of its randomness from it.
    pub seed: u64,
    /// Edge length of a streaming chunk, in meters.
    pub chunk_size_m: f32,
    /// Chunks along x and z, each in `1..=MAX_SIZE_CHUNKS`.
    pub size_chunks: [u16; 2],
    /// World chunk coordinates (`floor(x / chunk_size_m)`, `floor(z / chunk_size_m)`) of
    /// the place's chunk `(0, 0)`. `[0, 0]` puts the origin at the place's corner as
    /// `docs/PLACES.md` asks; the test town uses `[-4, -4]` because it is centered on the
    /// world origin.
    pub chunk_origin: [i16; 2],
    pub region: RegionId,
    pub era: Era,
    /// Latitude and longitude of the origin in degrees, for compiled places.
    pub origin_geo: Option<[f64; 2]>,
}

impl PlaceHeader {
    /// World position `[x, z]` of the corner of chunk `(0, 0)`.
    pub fn origin_m(&self) -> [f32; 2] {
        [
            f32::from(self.chunk_origin[0]) * self.chunk_size_m,
            f32::from(self.chunk_origin[1]) * self.chunk_size_m,
        ]
    }

    /// Size of the place along x and z, meters.
    pub fn extent_m(&self) -> [f32; 2] {
        [
            f32::from(self.size_chunks[0]) * self.chunk_size_m,
            f32::from(self.size_chunks[1]) * self.chunk_size_m,
        ]
    }

    /// Number of chunks.
    pub fn chunk_count(&self) -> usize {
        usize::from(self.size_chunks[0]) * usize::from(self.size_chunks[1])
    }

    /// Place chunk `(cx, cz)` holding the ground point `p`, or `None` outside the place.
    /// A point on a border belongs to the chunk on its +x / +z side.
    pub fn chunk_at(&self, p: [f32; 2]) -> Option<[u16; 2]> {
        let o = self.origin_m();
        let mut c = [0u16; 2];
        for k in 0..2 {
            let t = (p[k] - o[k]) / self.chunk_size_m;
            if !(t >= 0.0 && t < f32::from(self.size_chunks[k])) {
                return None;
            }
            c[k] = t as u16; // t >= 0: truncation is floor.
        }
        Some(c)
    }

    /// Linear index (`cz * size_x + cx`) of chunk `c`.
    pub fn chunk_slot(&self, c: [u16; 2]) -> usize {
        usize::from(c[1]) * usize::from(self.size_chunks[0]) + usize::from(c[0])
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WaterKind {
    Ocean = 0,
    Lake = 1,
    River = 2,
    Canal = 3,
    /// A dry concrete flood channel: no water, its polygon is the concrete between its
    /// lips (and down its ramps) that the ground mesh paints.
    Channel = 4,
}

/// What bounds a place along one side (`docs/WORLD_GENERATION.md`, "Map edges"): what the
/// terrain does in the band inside that side and how the horizon continues past it.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EdgeKind {
    /// Nothing: the ground just ends (the test town's apron).
    Open = 0,
    /// Open sea to the horizon.
    Ocean = 1,
    /// Open fresh water to the horizon.
    Lake = 2,
    /// Water with a far shore of low hills across it.
    Bay = 3,
    /// Cliffs rising into high ridges.
    Mountains = 4,
    /// A steep ridge with rolling hills behind it.
    Hills = 5,
    /// A wooded bluff with forest beyond.
    Forest = 6,
    /// A ridge the highway turns back along.
    HighwayLoop = 7,
    /// A rail embankment with yards and industry beyond.
    RailYards = 8,
    /// A low escarpment of mesas with open desert beyond: the desert highway turns back
    /// along it.
    Desert = 9,
}

impl EdgeKind {
    /// Water to the horizon: the coast side of a place.
    pub fn is_water(self) -> bool {
        matches!(self, EdgeKind::Ocean | EdgeKind::Lake | EdgeKind::Bay)
    }
}

/// Place sides in [`Terrain::edges`] order. North is `-z`, west is `-x`.
pub const NORTH: usize = 0;
pub const EAST: usize = 1;
pub const SOUTH: usize = 2;
pub const WEST: usize = 3;

/// A body of water: a counter-clockwise polygon at the terrain's `water_level`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaterBody {
    pub polygon: Vec<[f32; 2]>,
    pub kind: WaterKind,
}

/// The place-wide heightfield. Samples sit on the corners of a grid of `res_per_chunk`
/// cells per chunk: `(size_x * res + 1) * (size_z * res + 1)` heights, row-major (z major,
/// x minor), sample `(0, 0)` at [`PlaceHeader::origin_m`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Terrain {
    pub res_per_chunk: u16,
    /// Elevations in meters.
    pub heights: Vec<f32>,
    /// Elevation of the water surface, meters.
    pub water_level: f32,
    pub water: Vec<WaterBody>,
    /// What bounds the place on each side, `[north, east, south, west]` ([`NORTH`] ..).
    pub edges: [EdgeKind; 4],
}

impl Terrain {
    /// Flat ground at `elevation` over the whole place described by `header`.
    pub fn flat(header: &PlaceHeader, res_per_chunk: u16, elevation: f32) -> Terrain {
        let [w, h] = Terrain::samples(header, res_per_chunk);
        let mut heights = Vec::new();
        heights.resize(w * h, elevation);
        Terrain {
            res_per_chunk,
            heights,
            water_level: elevation - 2.0,
            water: Vec::new(),
            edges: [EdgeKind::Open; 4],
        }
    }

    /// Samples along x and z for a place of `header`'s size at `res_per_chunk`.
    pub fn samples(header: &PlaceHeader, res_per_chunk: u16) -> [usize; 2] {
        let r = usize::from(res_per_chunk);
        [
            usize::from(header.size_chunks[0]) * r + 1,
            usize::from(header.size_chunks[1]) * r + 1,
        ]
    }

    /// Elevation at sample `(x, z)`, or `None` outside the grid.
    pub fn sample(&self, header: &PlaceHeader, x: usize, z: usize) -> Option<f32> {
        let [w, h] = Terrain::samples(header, self.res_per_chunk);
        if x < w && z < h {
            self.heights.get(z * w + x).copied()
        } else {
            None
        }
    }

    /// Bilinearly interpolated elevation at the ground point `p`, or `None` outside the
    /// place (or when the grid is malformed).
    pub fn height_at(&self, header: &PlaceHeader, p: [f32; 2]) -> Option<f32> {
        let [w, h] = Terrain::samples(header, self.res_per_chunk);
        if self.res_per_chunk == 0 || self.heights.len() != w * h {
            return None;
        }
        let o = header.origin_m();
        let cell = header.chunk_size_m / f32::from(self.res_per_chunk);
        let u = (p[0] - o[0]) / cell;
        let v = (p[1] - o[1]) / cell;
        let (wm, hm) = ((w - 1) as f32, (h - 1) as f32);
        if !(u >= 0.0 && u <= wm && v >= 0.0 && v <= hm) {
            return None;
        }
        let (x0, z0) = ((u as usize).min(w - 2), (v as usize).min(h - 2));
        let (fx, fz) = (u - x0 as f32, v - z0 as f32);
        let at = |x: usize, z: usize| self.heights[z * w + x];
        let a = at(x0, z0) + (at(x0 + 1, z0) - at(x0, z0)) * fx;
        let b = at(x0, z0 + 1) + (at(x0 + 1, z0 + 1) - at(x0, z0 + 1)) * fx;
        Some(a + (b - a) * fz)
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ZoneId {
    Downtown = 0,
    Strip = 1,
    Residential = 2,
    Suburb = 3,
    Industrial = 4,
    Commercial = 5,
    Park = 6,
    Beach = 7,
    Farmland = 8,
    Hills = 9,
    Highway = 10,
    Water = 11,
}

/// The zone map: one [`ZoneId`] per terrain cell, `(size_x * res) * (size_z * res)` cells,
/// row-major (z major), cell `(0, 0)` at [`PlaceHeader::origin_m`].
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Zones {
    pub cells: Vec<ZoneId>,
}

impl Zones {
    /// Cells along x and z for a place of `header`'s size at `res_per_chunk`.
    pub fn cells_per_side(header: &PlaceHeader, res_per_chunk: u16) -> [usize; 2] {
        let r = usize::from(res_per_chunk);
        [
            usize::from(header.size_chunks[0]) * r,
            usize::from(header.size_chunks[1]) * r,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Node {
    pub pos: [f32; 2],
    /// Road surface height at the node, meters.
    pub elevation: f32,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RoadClass {
    Local = 0,
    Collector = 1,
    Arterial = 2,
    Highway = 3,
    Alley = 4,
    Beachfront = 5,
    Rural = 6,
}

/// A road segment from node `a` to node `b` (indices into `RoadGraph::nodes`). "Forward"
/// is the direction from `a` to `b`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub class: RoadClass,
    /// Lanes from `a` to `b`.
    pub lanes_fwd: u8,
    /// Lanes from `b` to `a`; 0 when `oneway`.
    pub lanes_back: u8,
    /// Curb-to-curb width (asphalt including gutters), meters.
    pub width_m: f32,
    /// Whether the road has a [`SIDEWALK_WIDTH_M`] sidewalk on both sides.
    pub sidewalk: bool,
    /// Posted speed, km/h.
    pub speed_kph: u8,
    pub oneway: bool,
    /// Centerline samples `[x, y, z]` from node `a` to node `b`, both included; `y` is the
    /// road surface height.
    pub profile: Vec<[f32; 3]>,
}

impl Edge {
    /// Total lanes, both directions.
    pub fn lanes(&self) -> u8 {
        self.lanes_fwd.saturating_add(self.lanes_back)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RoadGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// `Lot | Park | Lot_Empty` in `docs/PLACES.md`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LotKind {
    Lot = 0,
    Park = 1,
    LotEmpty = 2,
}

/// A piece of land fronting a road edge. Polygons are counter-clockwise seen from above.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Parcel {
    pub polygon: Vec<[f32; 2]>,
    pub zone: ZoneId,
    /// Index into `RoadGraph::edges` of the road the parcel faces.
    pub frontage_edge: u32,
    pub lot_kind: LotKind,
}

/// Architectural style of a building; the detailing pass keys facades and palettes on it.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StyleTag {
    /// Smooth stucco walls, simple parapet.
    StuccoBox = 0,
    /// Brick courses, stone trim, drainpipes.
    Brick = 1,
    /// Streamline/Art Deco: stepped parapet, speed-line bands.
    Deco = 2,
    /// Long, low row of storefronts under one deep canopy.
    StripMall = 3,
    /// Two-storey apartment box over open carports.
    Dingbat = 4,
    /// Low-pitched bungalow with a deep porch.
    Craftsman = 5,
    /// Long, low single-storey house.
    Ranch = 6,
    /// Repeated tract house of the 1970s.
    Tract70s = 7,
    /// Curtain-wall glass box.
    Glass = 8,
    Warehouse = 9,
    Barn = 10,
    /// Two-storey motel around a parking court.
    Motel = 11,
}

/// Roof shape. Pitched roofs rise above `Building::height_m` (the wall top).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Roof {
    /// Flat roof behind a parapet.
    Flat = 0,
    /// Two slopes meeting at a ridge along the long axis; gable walls at the ends.
    Gable = 1,
    /// Four slopes: a ridge along the long axis, hips at the ends.
    Hip = 2,
    /// Steep lower slopes on all four sides under a flat or shallow top.
    Mansard = 3,
}

/// What a building is used for (`use` in `docs/PLACES.md`; `use` is a Rust keyword).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BuildingUse {
    Residential = 0,
    Commercial = 1,
    Office = 2,
    Industrial = 3,
    Civic = 4,
    Hotel = 5,
    Parking = 6,
}

/// An extruded building: a closed, counter-clockwise (seen from above) footprint polygon.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Building {
    pub footprint: Vec<[f32; 2]>,
    /// Height of the top of the walls (parapet included) above the ground, meters.
    pub height_m: f32,
    /// Number of storeys, at least 1.
    pub floors: u8,
    pub roof: Roof,
    pub style: StyleTag,
    /// Year of construction when known (compiled places); lets the era layer drop
    /// buildings newer than the era.
    pub built_year: Option<u16>,
    pub usage: BuildingUse,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PoiKind {
    Pier = 0,
    Lighthouse = 1,
    Overlook = 2,
    Diner = 3,
    Gas = 4,
    Marina = 5,
    Stadium = 6,
}

/// A point of interest for the map and directions. Its name is invented from `name_seed`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Poi {
    pub pos: [f32; 2],
    pub kind: PoiKind,
    pub name_seed: u64,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SpawnKind {
    /// Where the car starts.
    PlayerStart = 0,
    /// Where a car is swapped or parked.
    Garage = 1,
}

/// A place a car can be put down.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Spawn {
    /// Ground point `[x, y, z]` under the car.
    pub pos: [f32; 3],
    /// Heading in the vehicle sim's convention: radians, 0 faces -z (north), positive
    /// turns left (counter-clockwise seen from above), so east is `-PI / 2`.
    pub heading: f32,
    pub kind: SpawnKind,
}

/// One lane of an edge: its centerline in driving direction.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Lane {
    pub edge: u32,
    /// Driving from the edge's `a` to `b`.
    pub forward: bool,
    /// 0 is the lane next to the centerline (or the left edge of a one-way road).
    pub slot: u8,
    pub centerline: Vec<[f32; 3]>,
}

/// Where traffic on `edge` stops before `node`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StopLine {
    pub edge: u32,
    pub node: u32,
    pub a: [f32; 3],
    pub b: [f32; 3],
}

/// An allowed movement through junction `node` from lane `from` to lane `to` (indices into
/// `Lanes::lanes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Turn {
    pub node: u32,
    pub from: u32,
    pub to: u32,
}

/// Lane centerlines, stop lines and junction turn tables (traffic and AI).
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Lanes {
    pub lanes: Vec<Lane>,
    pub stop_lines: Vec<StopLine>,
    pub turns: Vec<Turn>,
}

/// A walkable polyline on a sidewalk.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sidewalk {
    pub polyline: Vec<[f32; 3]>,
}

/// A pedestrian crossing at junction `node`, curb `a` to curb `b`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Crossing {
    pub node: u32,
    pub a: [f32; 3],
    pub b: [f32; 3],
}

/// Pedestrian paths.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sidewalks {
    pub walks: Vec<Sidewalk>,
    pub crossings: Vec<Crossing>,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum GroundFloor {
    Wall = 0,
    Door = 1,
    Storefront = 2,
    Lobby = 3,
    Garage = 4,
}

/// The detailing of one wall (`side`: footprint edge from vertex `side` to `side + 1`) of
/// `building`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Facade {
    pub building: u32,
    pub side: u16,
    /// Window grid: columns along the wall, rows (floors) up from the ground.
    pub cols: u8,
    pub rows: u8,
    pub ground: GroundFloor,
    pub awning: bool,
    pub sign_slots: u8,
    /// Emissive windows: bit `row * cols + col` (LSB first), `ceil(cols * rows / 8)` bytes.
    pub lit: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Facades {
    pub items: Vec<Facade>,
}

/// What a [`Prop`] is. Its facing is the unit ground direction `(-sin yaw, -cos yaw)` in
/// `(x, z)` (a [`Spawn`]'s heading convention); what that means depends on the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PropKind {
    /// Facing: along the arm, over the road.
    StreetLight,
    PalmTree,
    Oak,
    /// A run of hedge `len_dm` decimeters long centered on `pos`, along the facing.
    Hedge {
        len_dm: u16,
    },
    /// Facing: the way a sitter looks.
    Bench,
    Hydrant,
    /// A curbside mailbox on a post, its door toward the facing.
    Mailbox,
    /// A street-name post (two crossed blades) at a junction; `text_seed` names the
    /// streets.
    Sign {
        text_seed: u64,
    },
    /// A neon blade sign projecting from a wall along the facing; `palette` indexes the
    /// era's neon colors.
    NeonSign {
        palette: u8,
    },
    /// Facing: the way the panel faces.
    Billboard,
    /// Facing: along the line.
    PowerPole,
    /// A run of fence `len_dm` decimeters long centered on `pos`, along the facing.
    Fence {
        len_dm: u16,
    },
    /// Facing: the nose. `vehicle_class`: the vehicle crate's `VehicleClass` in the low 4
    /// bits, the paint (an index into the shape's palette, wrapping) in the high 4; the app
    /// maps the class onto a car shape. `hours`: bit `h` (0..24) set when the car is parked
    /// there through hour `h` of the day ([`ALL_HOURS`] for always), so a lot fills and
    /// empties with the clock; a parking lot's cars also set bit 24.
    ParkedCar {
        vehicle_class: u8,
        hours: u32,
    },
    /// A tall gum tree: a long bare trunk and a sparse, lobed crown.
    Eucalyptus,
    TrashCan,
    /// Facing: toward the drivers it stops.
    StopSign,
    /// A traffic signal pole with a mast arm of `arm_dm` decimeters along the facing.
    Signal {
        arm_dm: u8,
    },
    /// A pier `len_m` meters long from its land end at `pos` (on its deck) out along the
    /// facing.
    Pier {
        len_m: u8,
    },
    /// A length of railway track `len_dm` decimeters long centered on `pos`, along the
    /// facing, on its `bed` ([`TRACK_BALLAST`] and the others).
    Track {
        len_dm: u16,
        bed: u8,
    },
    /// A railroad crossing sign (the crossed blades and the twin lamps) on a post; facing:
    /// toward the drivers it warns.
    Crossbuck,
}

/// [`PropKind::Track`] beds: on a ballast embankment over the ground...
pub const TRACK_BALLAST: u8 = 0;
/// ...set flush in a road it crosses at grade (`pos` on the road's surface)...
pub const TRACK_CROSSING: u8 = 1;
/// ...on a steel deck bridging the ground under it (`pos` at the deck's top)...
pub const TRACK_BRIDGE: u8 = 2;
/// ...or on ballast, ending at a bumper at its far end along the facing (a spur's end).
pub const TRACK_END: u8 = 3;

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Prop {
    /// Base point: where it stands, `[x, y, z]`.
    pub pos: [f32; 3],
    /// Rotation about +Y, radians (see [`PropKind`] for the facing it gives).
    pub yaw: f32,
    pub kind: PropKind,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Props {
    pub items: Vec<Prop>,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LightKind {
    Street = 0,
    Neon = 1,
    Window = 2,
    Headlight = 3,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Light {
    pub pos: [f32; 3],
    /// Linear RGB.
    pub color: [f32; 3],
    /// Falloff radius, meters.
    pub radius: f32,
    pub kind: LightKind,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Lights {
    pub items: Vec<Light>,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Place {
    pub header: PlaceHeader,
    pub terrain: Terrain,
    pub zones: Zones,
    pub roads: RoadGraph,
    pub parcels: Vec<Parcel>,
    pub buildings: Vec<Building>,
    pub pois: Vec<Poi>,
    pub spawns: Vec<Spawn>,
    pub lanes: Lanes,
    pub sidewalks: Sidewalks,
    pub facades: Facades,
    pub props: Props,
    pub lights: Lights,
}

/// The object tables, in encoding and index order.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Nodes = 0,
    Edges = 1,
    Parcels = 2,
    Buildings = 3,
    Pois = 4,
    Spawns = 5,
    Lanes = 6,
    StopLines = 7,
    Turns = 8,
    Sidewalks = 9,
    Crossings = 10,
    Facades = 11,
    Props = 12,
    Lights = 13,
}

/// Number of [`Table`]s.
pub const TABLE_COUNT: usize = 14;

impl Table {
    pub const ALL: [Table; TABLE_COUNT] = [
        Table::Nodes,
        Table::Edges,
        Table::Parcels,
        Table::Buildings,
        Table::Pois,
        Table::Spawns,
        Table::Lanes,
        Table::StopLines,
        Table::Turns,
        Table::Sidewalks,
        Table::Crossings,
        Table::Facades,
        Table::Props,
        Table::Lights,
    ];
}

/// The slice of one table that belongs to one chunk: elements `first..first + count`,
/// encoded at bytes `offset..offset + len` of the encoded place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableRange {
    pub first: u32,
    pub count: u32,
    pub offset: u32,
    pub len: u32,
}

/// What the index knows about one chunk.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkEntry {
    /// Axis-aligned bounds `[x, y, z]` of the chunk square, its terrain and its objects'
    /// vertices; building tops are counted at the chunk's highest terrain sample plus
    /// `height_m` (pitched roofs rise a little above).
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// [`codec`]'s hash of the chunk's terrain samples, zone cells and table bytes.
    pub hash: u64,
    /// Indexed by `Table as usize`.
    pub tables: [TableRange; TABLE_COUNT],
}

/// Per-chunk byte ranges into every object table, bounds and content hashes, row-major (z
/// major) by chunk: entry `cz * size_x + cx`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkIndex {
    pub size_chunks: [u16; 2],
    pub entries: Vec<ChunkEntry>,
}

impl ChunkIndex {
    pub fn entry(&self, c: [u16; 2]) -> Option<&ChunkEntry> {
        if c[0] < self.size_chunks[0] && c[1] < self.size_chunks[1] {
            self.entries
                .get(usize::from(c[1]) * usize::from(self.size_chunks[0]) + usize::from(c[0]))
        } else {
            None
        }
    }
}

/// Why decoding or [`Place::validate`] rejected a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceError {
    // Decoding.
    /// The bytes do not start with [`PLACE_MAGIC`].
    Magic,
    /// The data ends inside a field.
    Truncated,
    /// Bytes left over after the index.
    TrailingBytes,
    /// An enum tag or bool byte outside its range, at this byte offset.
    BadValue {
        offset: u32,
    },
    /// The header's index offset or file length disagrees with the data.
    Layout,
    /// A chunk's table ranges disagree with the tables.
    IndexRange {
        chunk: u32,
    },
    /// A chunk's bounds disagree with its contents.
    IndexBounds {
        chunk: u32,
    },
    /// A chunk's contents do not match its hash.
    ChunkHash {
        chunk: u32,
    },
    /// Chunk coordinates outside the place.
    ChunkCoord,

    // Header and grids.
    Version,
    ChunkSize,
    PlaceSize,
    OriginGeo,
    NonFinite,
    TerrainSize,
    ZoneSize,
    Water {
        body: u32,
    },

    // Roads.
    EdgeIndex {
        edge: u32,
    },
    DegenerateEdge {
        edge: u32,
    },
    NoLanes {
        edge: u32,
    },
    /// A one-way edge with lanes back.
    OneWay {
        edge: u32,
    },
    RoadWidth {
        edge: u32,
    },
    Speed {
        edge: u32,
    },
    /// Fewer than two samples, non-finite, or not starting at `a` and ending at `b`.
    Profile {
        edge: u32,
    },
    /// A profile sample more than [`validate::PROFILE_TERRAIN_TOLERANCE_M`] off the terrain.
    ProfileOffTerrain {
        edge: u32,
    },
    /// No connected component holds 95% of the edges.
    Disconnected,

    // Parcels and buildings.
    /// Not a simple counter-clockwise polygon.
    Parcel {
        parcel: u32,
    },
    FrontageEdge {
        parcel: u32,
    },
    /// The parcel does not reach its frontage edge's property line.
    FrontageGap {
        parcel: u32,
    },
    /// Not a simple counter-clockwise polygon.
    Footprint {
        building: u32,
    },
    BuildingHeight {
        building: u32,
    },
    Floors {
        building: u32,
    },
    /// Two buildings overlap by more than 5% of the smaller footprint.
    BuildingOverlap {
        a: u32,
        b: u32,
    },

    // Spawns, chunking, detail.
    NoPlayerStart,
    /// An object whose centroid lies outside the place.
    OutOfBounds {
        table: Table,
        item: u32,
    },
    /// A table not sorted by chunk.
    Unsorted {
        table: Table,
    },
    /// A detail record referring to a missing node, edge, lane or building (or wall).
    DetailIndex {
        table: Table,
        item: u32,
    },
    /// A detail record with too few points, a bad mask or a bad radius.
    DetailGeometry {
        table: Table,
        item: u32,
    },
}

impl Place {
    /// A hand-laid flat town of 8 x 8 blocks on a 100 m grid (~0.8 km across) with an
    /// east-west and a north-south arterial (14 m) crossing at the center and 10 m local
    /// streets elsewhere. Around the crossing: a two-block commercial strip on the
    /// east-west arterial, a small downtown on the north-south one, a strip-mall superblock,
    /// a park, shops along the arterials and stucco houses everywhere else. The car starts
    /// in the eastbound outer lane at the west end of the strip.
    pub fn test_grid() -> Place {
        town::test_grid()
    }

    /// Checks the invariants every consumer relies on (`docs/PLACES.md`, "Validation").
    /// Chunk hashes are checked by [`codec::decode`].
    pub fn validate(&self) -> Result<(), PlaceError> {
        validate::validate(self)
    }

    /// The first [`SpawnKind::PlayerStart`].
    pub fn player_start(&self) -> Option<&Spawn> {
        self.spawns
            .iter()
            .find(|s| s.kind == SpawnKind::PlayerStart)
    }

    /// Canonical serialization ([`codec::encode`]), appended to `out`.
    pub fn write_bytes(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&codec::encode(self));
    }

    /// Hash of [`codec::encode`]: the determinism fingerprint of a place.
    pub fn content_hash(&self) -> u64 {
        skywave_core::hash_bytes(0, &codec::encode(self))
    }

    /// The chunk element `item` of `table` belongs to: the chunk of its centroid (the mean
    /// of its vertices; facades follow their building, turns their node). `None` when the
    /// centroid lies outside the place or a reference is dangling.
    pub fn chunk_of(&self, table: Table, item: usize) -> Option<[u16; 2]> {
        let h = &self.header;
        let mean2 = |pts: &[[f32; 2]]| centroid(pts.iter().map(|p| [p[0], p[1]]));
        let mean3 = |pts: &[[f32; 3]]| centroid(pts.iter().map(|p| [p[0], p[2]]));
        let node = |n: u32| self.roads.nodes.get(n as usize).map(|n| n.pos);
        let p = match table {
            Table::Nodes => Some(self.roads.nodes.get(item)?.pos),
            Table::Edges => {
                let e = self.roads.edges.get(item)?;
                let (a, b) = (node(e.a)?, node(e.b)?);
                Some([(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5])
            }
            Table::Parcels => mean2(&self.parcels.get(item)?.polygon),
            Table::Buildings => mean2(&self.buildings.get(item)?.footprint),
            Table::Pois => Some(self.pois.get(item)?.pos),
            Table::Spawns => {
                let s = self.spawns.get(item)?;
                Some([s.pos[0], s.pos[2]])
            }
            Table::Lanes => mean3(&self.lanes.lanes.get(item)?.centerline),
            Table::StopLines => {
                let s = self.lanes.stop_lines.get(item)?;
                mean3(&[s.a, s.b])
            }
            Table::Turns => node(self.lanes.turns.get(item)?.node),
            Table::Sidewalks => mean3(&self.sidewalks.walks.get(item)?.polyline),
            Table::Crossings => {
                let c = self.sidewalks.crossings.get(item)?;
                mean3(&[c.a, c.b])
            }
            Table::Facades => {
                let f = self.facades.items.get(item)?;
                return self.chunk_of(Table::Buildings, f.building as usize);
            }
            Table::Props => {
                let p = self.props.items.get(item)?;
                Some([p.pos[0], p.pos[2]])
            }
            Table::Lights => {
                let l = self.lights.items.get(item)?;
                Some([l.pos[0], l.pos[2]])
            }
        };
        h.chunk_at(p?)
    }

    /// Number of elements in `table`.
    pub fn table_len(&self, table: Table) -> usize {
        match table {
            Table::Nodes => self.roads.nodes.len(),
            Table::Edges => self.roads.edges.len(),
            Table::Parcels => self.parcels.len(),
            Table::Buildings => self.buildings.len(),
            Table::Pois => self.pois.len(),
            Table::Spawns => self.spawns.len(),
            Table::Lanes => self.lanes.lanes.len(),
            Table::StopLines => self.lanes.stop_lines.len(),
            Table::Turns => self.lanes.turns.len(),
            Table::Sidewalks => self.sidewalks.walks.len(),
            Table::Crossings => self.sidewalks.crossings.len(),
            Table::Facades => self.facades.items.len(),
            Table::Props => self.props.items.len(),
            Table::Lights => self.lights.items.len(),
        }
    }

    /// Sort key of element `item` of `table`: its chunk's linear index, or `u32::MAX` when
    /// it has none (those sort last and fail validation).
    pub fn chunk_key(&self, table: Table, item: usize) -> u32 {
        match self.chunk_of(table, item) {
            Some(c) => self.header.chunk_slot(c) as u32,
            None => u32::MAX,
        }
    }

    /// Stably sorts every table by chunk and rewrites the indices that refer into the
    /// reordered tables (edge nodes, frontage edges, detail references). Producers call
    /// this last.
    pub fn sort_by_chunk(&mut self) {
        let order = |p: &Place, t: Table| -> Vec<u32> {
            let keys: Vec<u32> = (0..p.table_len(t)).map(|i| p.chunk_key(t, i)).collect();
            let mut idx: Vec<u32> = (0..keys.len() as u32).collect();
            idx.sort_by_key(|&i| keys[i as usize]);
            idx
        };
        // Every key is computed from the tables as they are, before any is moved.
        let nodes = order(self, Table::Nodes);
        let edges = order(self, Table::Edges);
        let buildings = order(self, Table::Buildings);
        let lanes = order(self, Table::Lanes);
        let rest: Vec<(Table, Vec<u32>)> = [
            Table::Parcels,
            Table::Pois,
            Table::Spawns,
            Table::StopLines,
            Table::Turns,
            Table::Sidewalks,
            Table::Crossings,
            Table::Facades,
            Table::Props,
            Table::Lights,
        ]
        .into_iter()
        .map(|t| (t, order(self, t)))
        .collect();

        let (node_to, edge_to) = (inverse(&nodes), inverse(&edges));
        let (building_to, lane_to) = (inverse(&buildings), inverse(&lanes));
        let remap = |to: &[u32], i: &mut u32| {
            if let Some(&n) = to.get(*i as usize) {
                *i = n;
            }
        };
        permute(&mut self.roads.nodes, &nodes);
        permute(&mut self.roads.edges, &edges);
        permute(&mut self.buildings, &buildings);
        permute(&mut self.lanes.lanes, &lanes);
        for (t, o) in &rest {
            match t {
                Table::Parcels => permute(&mut self.parcels, o),
                Table::Pois => permute(&mut self.pois, o),
                Table::Spawns => permute(&mut self.spawns, o),
                Table::StopLines => permute(&mut self.lanes.stop_lines, o),
                Table::Turns => permute(&mut self.lanes.turns, o),
                Table::Sidewalks => permute(&mut self.sidewalks.walks, o),
                Table::Crossings => permute(&mut self.sidewalks.crossings, o),
                Table::Facades => permute(&mut self.facades.items, o),
                Table::Props => permute(&mut self.props.items, o),
                Table::Lights => permute(&mut self.lights.items, o),
                _ => {}
            }
        }
        for e in &mut self.roads.edges {
            remap(&node_to, &mut e.a);
            remap(&node_to, &mut e.b);
        }
        for p in &mut self.parcels {
            remap(&edge_to, &mut p.frontage_edge);
        }
        for l in &mut self.lanes.lanes {
            remap(&edge_to, &mut l.edge);
        }
        for s in &mut self.lanes.stop_lines {
            remap(&edge_to, &mut s.edge);
            remap(&node_to, &mut s.node);
        }
        for t in &mut self.lanes.turns {
            remap(&node_to, &mut t.node);
            remap(&lane_to, &mut t.from);
            remap(&lane_to, &mut t.to);
        }
        for c in &mut self.sidewalks.crossings {
            remap(&node_to, &mut c.node);
        }
        for f in &mut self.facades.items {
            remap(&building_to, &mut f.building);
        }
    }
}

/// Mean of the points, or `None` for none.
fn centroid(pts: impl Iterator<Item = [f32; 2]>) -> Option<[f32; 2]> {
    let (mut sx, mut sz, mut n) = (0.0f32, 0.0f32, 0u32);
    for p in pts {
        sx += p[0];
        sz += p[1];
        n += 1;
    }
    (n > 0).then(|| [sx / n as f32, sz / n as f32])
}

/// `to[old] = new` for a permutation listing old indices in their new order.
fn inverse(order: &[u32]) -> Vec<u32> {
    let mut to = vec![0; order.len()];
    for (new, &old) in order.iter().enumerate() {
        to[old as usize] = new as u32;
    }
    to
}

/// Reorders `v` so that element `k` is the old element `order[k]`.
fn permute<T>(v: &mut Vec<T>, order: &[u32]) {
    let mut slots: Vec<Option<T>> = core::mem::take(v).into_iter().map(Some).collect();
    for &old in order {
        if let Some(x) = slots[old as usize].take() {
            v.push(x);
        }
    }
}

/// Shoelace area, positive for counter-clockwise polygons viewed from +Y.
pub fn signed_area(poly: &[[f32; 2]]) -> f32 {
    let mut acc = 0.0;
    for (i, p) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()];
        acc += p[1] * q[0] - p[0] * q[1];
    }
    acc * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn terrain_sampling() {
        let mut p = Place::test_grid();
        let h = p.header.clone();
        let t = Terrain::flat(&h, 2, 1.5);
        assert_eq!(t.heights.len(), 17 * 17);
        assert_eq!(t.sample(&h, 16, 16), Some(1.5));
        assert_eq!(t.sample(&h, 17, 0), None);
        p.terrain = t;
        // A ramp along x: height = sample column.
        let [w, _] = Terrain::samples(&h, 2);
        for (i, v) in p.terrain.heights.iter_mut().enumerate() {
            *v = (i % w) as f32;
        }
        let o = h.origin_m();
        assert_eq!(
            p.terrain.height_at(&h, [o[0] + 96.0, o[1] + 10.0]),
            Some(1.5)
        );
        assert_eq!(p.terrain.height_at(&h, [o[0] - 1.0, o[1]]), None);
        let far = [o[0] + h.extent_m()[0], o[1] + h.extent_m()[1]];
        assert_eq!(p.terrain.height_at(&h, far), Some(16.0));
    }

    #[test]
    fn chunks_of_points() {
        let h = Place::test_grid().header;
        assert_eq!(h.origin_m(), [-512.0, -512.0]);
        assert_eq!(h.chunk_at([-512.0, -512.0]), Some([0, 0]));
        assert_eq!(h.chunk_at([0.0, -0.01]), Some([4, 3]));
        assert_eq!(h.chunk_at([511.9, 511.9]), Some([7, 7]));
        assert_eq!(h.chunk_at([512.0, 0.0]), None);
        assert_eq!(h.chunk_at([f32::NAN, 0.0]), None);
        assert_eq!(h.chunk_slot([3, 2]), 19);
    }

    #[test]
    fn sort_by_chunk_keeps_references() {
        let mut p = Place::test_grid();
        // Reverse the edges and buildings, add detail that points at them, and sort back.
        p.roads.edges.reverse();
        let n = p.roads.edges.len() as u32;
        for q in &mut p.parcels {
            q.frontage_edge = n - 1 - q.frontage_edge;
        }
        p.buildings.reverse();
        let nb = p.buildings.len() as u32;
        p.facades.items.push(Facade {
            building: nb - 1,
            side: 0,
            cols: 2,
            rows: 1,
            ground: GroundFloor::Door,
            awning: false,
            sign_slots: 0,
            lit: vec![0b10],
        });
        let fronts: Vec<Vec<[f32; 3]>> = p
            .parcels
            .iter()
            .map(|q| p.roads.edges[q.frontage_edge as usize].profile.clone())
            .collect();
        let ends: Vec<[[f32; 2]; 2]> = p
            .roads
            .edges
            .iter()
            .map(|e| {
                [
                    p.roads.nodes[e.a as usize].pos,
                    p.roads.nodes[e.b as usize].pos,
                ]
            })
            .collect();
        let faced = p.buildings[nb as usize - 1].clone();
        p.roads.nodes.reverse();
        let nn = p.roads.nodes.len() as u32;
        for e in &mut p.roads.edges {
            e.a = nn - 1 - e.a;
            e.b = nn - 1 - e.b;
        }
        p.sort_by_chunk();
        assert_eq!(p.validate(), Ok(()));
        // Parcels were already sorted and keep their order.
        for (q, front) in p.parcels.iter().zip(&fronts) {
            assert_eq!(&p.roads.edges[q.frontage_edge as usize].profile, front);
        }
        for e in &p.roads.edges {
            let got = [
                p.roads.nodes[e.a as usize].pos,
                p.roads.nodes[e.b as usize].pos,
            ];
            assert!(ends.contains(&got));
        }
        assert_eq!(p.buildings[p.facades.items[0].building as usize], faced);
        let mut again = p.clone();
        again.sort_by_chunk();
        assert_eq!(again, p);
    }
}
