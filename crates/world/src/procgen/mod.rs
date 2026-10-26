//! The procedural place producer (`docs/WORLD_GENERATION.md`). So far the macro stages:
//! coast and terrain (stage 1) and the zone map (stage 2), as a [`MacroMap`], and the
//! arterial network with the highway (stage 3), as a `place::RoadGraph` ([`arterials()`]);
//! then the chunk stages: local streets (stage 4: [`streets()`], [`streets_all()`]) and
//! blocks and parcels (stage 5: [`parcels()`], [`ParcelPlan`]).
//!
//! Everything is a pure function of `(seed, template, size)`. Each stage draws from its own
//! `Rng::new(seed, &[stage, ..])` and its own noise keyed by `hash64(seed, [stage, ..])`, so
//! changing one stage's draws never shifts another's.

pub mod arterials;
pub mod macro_map;
pub mod parcels;
pub mod streets;
pub mod terrain;
pub mod zones;

pub use arterials::{arterials, graded_heights, hash_graph};
pub use macro_map::{Frame, MacroMap, MacroPoi, MacroPoiKind, Strip, WaterBody, WaterKind};
pub use parcels::{
    hash_parcels, parcels, parcels_all, Block, Face, FaceKind, FaceParcels, FaceStats, ParcelChunk,
    ParcelPlan,
};
pub use streets::{hash_chunk, streets, streets_all, GridCell, NodeRef, StreetChunk, StreetPlan};

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
