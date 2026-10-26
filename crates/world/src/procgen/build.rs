//! The procedural pipeline end to end: a skeleton [`Place`] from a seed and a template.
//!
//! [`build_place`] runs the macro map (stages 1–2), the arterial network (stage 3) and the
//! local streets of every chunk (stage 4, merged with the arterials), then fills the Place
//! skeleton: terrain from the macro heights (which already carry the highway corridor) at
//! [`DEFAULT_RES_PER_CHUNK`], zones, water bodies, the road graph and one `PlayerStart` in
//! the outer lane of the strip's spine. Parcels and buildings come later.

use alloc::vec::Vec;

use place::{
    Era, Facades, Lanes, Lights, Place, PlaceHeader, Props, RegionId, RoadClass, RoadGraph,
    Sidewalks, Spawn, SpawnKind, Terrain, WaterBody, WaterKind as PlaceWaterKind, Zones,
    CHUNK_SIZE_M, DEFAULT_RES_PER_CHUNK, PLACE_VERSION,
};
use skywave_core::Seed;

use super::arterials::arterials;
use super::macro_map::{MacroMap, WaterKind};
use super::streets::streets_all;
use super::{RES_PER_CHUNK, ZONE_SAMPLES};
use crate::math::{atan2, segment_distance, V2};
use crate::template::RegionTemplate;

/// Chunks along each side of a v1 place (2 km square).
pub const PLACE_CHUNKS: [u16; 2] = [16, 16];

/// The region a template name stands for.
pub fn region_of(template: &RegionTemplate) -> RegionId {
    match template.name.as_str() {
        "california_coast" => RegionId::CaliforniaCoast,
        "great_lakes_city" => RegionId::GreatLakesCity,
        "north_shore_suburb" => RegionId::NorthShoreSuburb,
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

/// A skeleton Place for `(seed, template)`: see the module docs. `template` must have
/// passed [`RegionTemplate::validate`]. Deterministic.
pub fn build_place(seed: Seed, template: &RegionTemplate) -> Place {
    let m = MacroMap::generate(seed, template, PLACE_CHUNKS);
    let arterials = arterials(seed, template, &m);
    // The start is found on the arterials as they are, before the streets split them.
    let start = player_start(&m, &arterials);
    let roads = streets_all(seed, template, &m, &arterials);
    assemble(&m, template, roads, start)
}

/// The Place skeleton for a macro map and its road graph, the start on its arterials.
pub fn place_from(m: &MacroMap, template: &RegionTemplate, roads: RoadGraph) -> Place {
    let start = player_start(m, &roads);
    assemble(m, template, roads, start)
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

/// The car's start: on the arterial edge nearest the middle of the strip, in the outer
/// lane of the side traffic drives on (right), facing along the edge from `a` to `b`.
fn player_start(m: &MacroMap, roads: &RoadGraph) -> Option<Spawn> {
    let line = &m.strip.centerline;
    let target = V2::from_array(*line.get(line.len() / 2)?);
    let mut best: Option<(f32, usize, usize, f32)> = None;
    for (i, e) in roads.edges.iter().enumerate() {
        if e.class != RoadClass::Arterial || e.oneway {
            continue;
        }
        for k in 0..e.profile.len().saturating_sub(1) {
            let (a, b) = (e.profile[k], e.profile[k + 1]);
            let (d, t) = segment_distance(target, V2::new(a[0], a[2]), V2::new(b[0], b[2]));
            if best.is_none_or(|q| d < q.0) {
                best = Some((d, i, k, t));
            }
        }
    }
    let (_, i, k, t) = best?;
    let e = &roads.edges[i];
    let (a, b) = (e.profile[k], e.profile[k + 1]);
    let (pa, pb) = (V2::new(a[0], a[2]), V2::new(b[0], b[2]));
    let dir = (pb - pa).normalize();
    // The outer lane's center: lanes split the curb-to-curb half width less the gutter.
    let inner = e.width_m * 0.5 - 0.45;
    let lanes = f32::from(e.lanes_fwd.max(1));
    let offset = inner - 0.5 * inner / lanes;
    let p = pa.lerp(pb, t) + dir.right() * offset;
    let y = a[1] + (b[1] - a[1]) * t;
    Some(Spawn {
        pos: [p.x, y, p.y],
        heading: atan2(-dir.x, -dir.y),
        kind: SpawnKind::PlayerStart,
    })
}
