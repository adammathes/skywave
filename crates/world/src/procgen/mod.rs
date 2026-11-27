//! The place producer (`docs/WORLD_GENERATION.md`). An authored layout ([`authored`],
//! `docs/LAYOUTS.md`) draws the macro stages: terrain and zones as a [`MacroMap`] and the
//! main roads as a `place::RoadGraph`. The seed fills in the chunk stages: local streets
//! (stage 4: [`streets()`], [`streets_all()`]), blocks and parcels (stage 5: [`parcels()`],
//! [`ParcelPlan`]), buildings (stage 6: [`buildings()`], [`buildings_all()`]) and detailing
//! (stage 7: [`detail()`]: props and lights).
//!
//! Everything is a pure function of `(seed, template, layout)`. Each stage draws from its own
//! `Rng::new(seed, &[stage, ..])` and its own noise keyed by `hash64(seed, [stage, ..])`, so
//! changing one stage's draws never shifts another's.

pub mod authored;
pub mod build;
pub mod buildings;
pub mod detail;
pub mod macro_map;
pub mod parcels;
mod profile;
pub mod setpieces;
pub mod streets;
pub mod terrain;
pub mod zones;

pub use build::{place_from, PLACE_CHUNKS};
pub use buildings::{buildings, buildings_all, hash_buildings, lot_buildings};
pub use detail::{detail, hash_detail};
pub use macro_map::{BasinPlan, GridLine, MacroMap, MacroPoi, MacroPoiKind, WaterBody, WaterKind};
pub use parcels::{
    hash_parcels, parcels, parcels_all, Block, Face, FaceKind, FaceParcels, FaceStats, ParcelChunk,
    ParcelPlan,
};
pub use setpieces::SetPieces;
pub use streets::{
    hash_chunk, hash_graph, streets, streets_all, GridCell, NodeRef, StreetChunk, StreetPlan,
};

/// Stage ids, the first word of every stage's hash key.
pub mod stage {
    /// Shoreline, bays, beach widths.
    pub const COAST: u64 = 1;
    /// Plain, ridges, terrain noise.
    pub const TERRAIN: u64 = 2;
    /// Strip, downtown, highway, pass, valleys, canal pocket.
    pub const LAYOUT: u64 = 3;
    /// Zone noise and park placement.
    pub const ZONES: u64 = 4;
    /// Overlooks and other points of interest.
    pub const POIS: u64 = 5;
    /// Arterial network and highway interchanges.
    pub const ARTERIALS: u64 = 6;
    /// Local streets (chunk stage).
    pub const STREETS: u64 = 7;
    /// Blocks and parcels (chunk stage).
    pub const PARCELS: u64 = 8;
    /// Buildings on parcels (chunk stage).
    pub const BUILDINGS: u64 = 9;
    /// Detailing: props and lights (chunk stage).
    pub const DETAIL: u64 = 10;
    /// Parking lots (`crate::lots`).
    pub const LOTS: u64 = 11;
}

/// Edge length of a streaming chunk.
pub const CHUNK_M: f32 = 128.0;
/// Height samples per chunk edge.
pub const RES_PER_CHUNK: u32 = 32;
/// Distance between height samples.
pub const CELL_M: f32 = CHUNK_M / RES_PER_CHUNK as f32;
/// Height samples per zone cell edge: a zone cell is 4×4 samples, 16 m.
pub const ZONE_SAMPLES: u32 = 4;
/// Edge length of a zone cell.
pub const ZONE_CELL_M: f32 = CELL_M * ZONE_SAMPLES as f32;
