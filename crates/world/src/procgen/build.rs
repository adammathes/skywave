//! The chunk stages end to end: [`fill`] takes a macro map and its main roads (from an
//! authored layout, [`super::authored`]) and builds the [`Place`]: terrain at
//! [`DEFAULT_RES_PER_CHUNK`], zones, water bodies, the road graph, then local streets
//! (stage 4), parcels (stage 5), buildings (stage 6) and the detailing (stage 7).

use alloc::vec::Vec;

use place::{
    Era, Facades, Lanes, Lights, Place, PlaceHeader, Props, RegionId, RoadGraph, Sidewalks, Spawn,
    Terrain, WaterBody, WaterKind as PlaceWaterKind, Zones, CHUNK_SIZE_M, DEFAULT_RES_PER_CHUNK,
    PLACE_VERSION,
};
use skywave_core::Seed;

use super::buildings::buildings_all;
use super::detail::detail;
use super::macro_map::{MacroMap, WaterKind};
use super::parcels::parcels_all;
use super::streets::streets_all;
use super::{RES_PER_CHUNK, ZONE_SAMPLES};
use crate::grammar::Grammar;
use crate::template::RegionTemplate;
use crate::{ChunkPlan, Palette};

/// Chunks along each side of a v1 place (2 km square).
pub const PLACE_CHUNKS: [u16; 2] = [16, 16];

/// The region a template name stands for.
pub fn region_of(template: &RegionTemplate) -> RegionId {
    match template.name.as_str() {
        "california_coast" => RegionId::CaliforniaCoast,
        "great_lakes_city" => RegionId::GreatLakesCity,
        "north_shore_suburb" => RegionId::NorthShoreSuburb,
        "socal_basin" => RegionId::SocalBasin,
        "bay_area" => RegionId::BayArea,
        _ => RegionId::CaliforniaCoast,
    }
}

/// The decade of `year`, clamped to the eras there are.
pub fn era_of(year: u16) -> Era {
    match year {
        0..=1979 => Era::Seventies,
        1980..=1989 => Era::Eighties,
        1990..=1999 => Era::Nineties,
        2000..=2009 => Era::Aughts,
        2010..=2019 => Era::Tens,
        _ => Era::Twenties,
    }
}

/// Stages 4–7 over a macro map and its arterials, however they were made (procedurally
/// or from an authored layout, `super::authored`): local streets merged into the graph,
/// parcels and buildings on it, the set pieces, then the detailing. Returns the Place and
/// the plan its detail pass read.
pub(super) fn fill(
    seed: Seed,
    template: &RegionTemplate,
    grammar: &Grammar,
    m: &MacroMap,
    arterials: &RoadGraph,
    start: Option<Spawn>,
) -> (Place, ChunkPlan) {
    // Stages 4-6: local streets merged into the graph, then parcels and buildings on it
    // (against the graph as the Place stores it, sorted by chunk).
    let roads = streets_all(seed, template, m, arterials);
    let mut place = assemble(m, template, roads, start);
    place.parcels = parcels_all(seed, template, m, &place.roads);
    place.buildings = buildings_all(
        seed,
        template,
        &grammar.lots,
        m,
        &place.parcels,
        &place.roads,
    );
    // The set pieces take their lots and add their landmarks.
    let sites = super::setpieces::site(m, &place, &grammar.landmarks, &template.roadside);
    super::setpieces::clear(&mut place, &sites);
    place.pois.extend(sites.pois(seed));
    // Stage 7 reads the buildings where the meshes will find them.
    place.sort_by_chunk();
    // The template's own palette where it names a built-in one (the coast's names none).
    let pal = Palette::named(&template.era.palette).unwrap_or_else(Palette::socal_1985);
    let plan = ChunkPlan::new(&place, &pal, grammar);
    detail(seed, template, grammar, m, &sites, &plan, &mut place);
    // Only the props and lights move: every table the plan read is in chunk order already.
    place.sort_by_chunk();
    (place, plan)
}

/// The Place skeleton for a macro map and its road graph, the start on its arterials.
pub fn place_from(m: &MacroMap, template: &RegionTemplate, roads: RoadGraph) -> Place {
    assemble(m, template, roads, None)
}

fn assemble(
    m: &MacroMap,
    template: &RegionTemplate,
    roads: RoadGraph,
    start: Option<Spawn>,
) -> Place {
    let header = PlaceHeader {
        version: PLACE_VERSION,
        seed: m.seed.0,
        chunk_size_m: CHUNK_SIZE_M,
        size_chunks: m.size_chunks,
        chunk_origin: [0, 0],
        region: region_of(template),
        era: era_of(template.era.year),
        origin_geo: None,
    };
    let res = DEFAULT_RES_PER_CHUNK;
    debug_assert_eq!(u32::from(res), RES_PER_CHUNK);
    // The macro grid has one sample fewer per side than the Place's corner grid: the last
    // row and column repeat it.
    let [tw, th] = Terrain::samples(&header, res);
    let (w, h) = (m.width as usize, m.height as usize);
    let mut heights = Vec::with_capacity(tw * th);
    for z in 0..th {
        for x in 0..tw {
            heights.push(m.heights[z.min(h - 1) * w + x.min(w - 1)]);
        }
    }
    let [zw, zh] = Zones::cells_per_side(&header, res);
    let (mzw, mzh) = (m.zone_width as usize, m.zone_height as usize);
    let n = ZONE_SAMPLES as usize;
    let mut cells = Vec::with_capacity(zw * zh);
    for z in 0..zh {
        for x in 0..zw {
            cells.push(m.zones[(z / n).min(mzh - 1) * mzw + (x / n).min(mzw - 1)]);
        }
    }
    let water = m
        .water
        .iter()
        .map(|b| WaterBody {
            polygon: b.polygon.clone(),
            kind: match b.kind {
                WaterKind::Ocean => PlaceWaterKind::Ocean,
                WaterKind::Lake => PlaceWaterKind::Lake,
                WaterKind::Canal => PlaceWaterKind::Canal,
                WaterKind::Channel => PlaceWaterKind::Channel,
            },
        })
        .collect();
    let spawns = start.into_iter().collect();
    let mut place = Place {
        header,
        terrain: Terrain {
            res_per_chunk: res,
            heights,
            water_level: m.water_level,
            water,
            edges: m.edges,
        },
        zones: Zones { cells },
        roads,
        parcels: Vec::new(),
        buildings: Vec::new(),
        pois: Vec::new(),
        spawns,
        lanes: Lanes::default(),
        sidewalks: Sidewalks::default(),
        facades: Facades::default(),
        props: Props::default(),
        lights: Lights::default(),
    };
    place.sort_by_chunk();
    place
}
