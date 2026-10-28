//! The canonical binary encoding of a [`Place`] and its per-chunk [`ChunkIndex`].
//!
//! Little-endian throughout, no padding, no alignment. `f32`/`f64` are IEEE-754 bit
//! patterns, so NaN payloads and signed zeros round-trip. `bool` is one byte, 0 or 1; enums
//! are one byte holding their `repr(u8)` discriminant; `Option<T>` is a `bool` byte then
//! `T` (absent: zero bytes of `T`'s size, header only, see below). A **list** is a `u32`
//! count then its elements; `pts2` is a list of `[f32; 2]`, `pts3` a list of `[f32; 3]`.
//! The encoding is canonical: one place has exactly one encoding, decoding rejects anything
//! else, and `encode(decode(b)) == b` for every accepted `b`.
//!
//! ```text
//! Header, 56 bytes at offset 0
//!    0  [u8; 4]  magic "SKYW"
//!    4  u32      version (PLACE_VERSION)
//!    8  u64      seed
//!   16  f32      chunk_size_m
//!   20  u16 x2   size_chunks [x, z]
//!   24  i16 x2   chunk_origin [x, z]
//!   28  u8       region (RegionId)
//!   29  u8       era (Era)
//!   30  u8       origin_geo present (0 | 1)
//!   31  u8       reserved, 0
//!   32  f64 x2   origin_geo [lat, lon]; both +0.0 when absent
//!   48  u32      index_offset: byte offset of the chunk index
//!   52  u32      file_len: total length in bytes
//!
//! Grids, at 56
//!   u16 res_per_chunk, u8 x4 edges [north, east, south, west] (EdgeKind), f32 water_level
//!   list of f32  terrain heights ((sx*res+1) * (sz*res+1), row-major, z major)
//!   list of u8   zone cells (sx*res * sz*res, row-major, z major; ZoneId)
//!   list of water bodies: u8 kind (WaterKind), pts2 polygon
//!
//! Object tables, in `Table` order, each a list of records:
//!   Nodes       f32 x, f32 z, f32 elevation
//!   Edges       u32 a, u32 b, u8 class, u8 lanes_fwd, u8 lanes_back, f32 width_m,
//!               bool sidewalk, u8 speed_kph, bool oneway, pts3 profile
//!   Parcels     pts2 polygon, u8 zone, u32 frontage_edge, u8 lot_kind
//!   Buildings   pts2 footprint, f32 height_m, u8 floors, u8 roof, u8 style,
//!               Option<u16> built_year, u8 usage
//!   Pois        f32 x 2 pos, u8 kind, u64 name_seed
//!   Spawns      f32 x 3 pos, f32 heading, u8 kind
//!   Lanes       u32 edge, bool forward, u8 slot, pts3 centerline
//!   StopLines   u32 edge, u32 node, f32 x 3 a, f32 x 3 b
//!   Turns       u32 node, u32 from, u32 to
//!   Sidewalks   pts3 polyline
//!   Crossings   u32 node, f32 x 3 a, f32 x 3 b
//!   Facades     u32 building, u16 side, u8 cols, u8 rows, u8 ground, bool awning,
//!               u8 sign_slots, list of u8 lit
//!   Props       f32 x 3 pos, f32 yaw, u8 kind tag (PropKind declaration order) then its
//!               payload: Hedge u16 len_dm, Sign u64 text_seed, NeonSign u8 palette, Fence
//!               u16 len_dm, ParkedCar u8 class u32 hours, Signal u8 arm_dm, Pier u8 len_m
//!   Lights      f32 x 3 pos, f32 x 3 color, f32 radius, u8 kind
//!
//! Chunk index, at index_offset, to the end of the file
//!   u32 chunk_count (sx * sz), u32 table_count (TABLE_COUNT)
//!   chunk_count entries of 256 bytes, chunk (cx, cz) at entry cz * sx + cx:
//!      0  f32 x 3  bounds_min
//!     12  f32 x 3  bounds_max
//!     24  u64      hash
//!     32  TABLE_COUNT x { u32 first, u32 count, u32 offset, u32 len }
//! ```
//!
//! A chunk's table range lists the records whose chunk ([`Place::chunk_of`]) is that
//! chunk: since tables are sorted by chunk they are contiguous, `first` is the number of
//! records of earlier chunks and `offset..offset + len` their bytes in the file. The chunk
//! hash is `skywave_core::hash_bytes(0, ..)` over, concatenated: the chunk's
//! `(res + 1)^2` height samples (rows of the heights list, z then x, borders shared with
//! neighbours), its `res^2` zone cells (likewise), then each table's byte range in order.
//! So [`read_index`] and [`decode_chunk`] give one chunk's records without decoding the
//! rest, and [`decode`] checks every range, bound and hash against what it decoded.
//! Records outside the place belong to no chunk (and fail [`Place::validate`]).

use alloc::vec;
use alloc::vec::Vec;
use skywave_core::hash_bytes;

use crate::{
    Building, BuildingUse, ChunkEntry, ChunkIndex, Crossing, Edge, EdgeKind, Era, Facade, Facades,
    GroundFloor, Lane, Lanes, Light, LightKind, Lights, LotKind, Node, Parcel, Place, PlaceError,
    PlaceHeader, Poi, PoiKind, Prop, PropKind, Props, RegionId, RoadClass, RoadGraph, Roof,
    Sidewalk, Sidewalks, Spawn, SpawnKind, StopLine, StyleTag, Table, TableRange, Terrain, Turn,
    WaterBody, WaterKind, ZoneId, Zones, MAX_SIZE_CHUNKS, PLACE_MAGIC, PLACE_VERSION, TABLE_COUNT,
};

/// Length of the fixed header.
pub const HEADER_LEN: usize = 56;
/// Length of one chunk index entry.
pub const ENTRY_LEN: usize = 32 + 16 * TABLE_COUNT;

/// Where the grids and records landed in an encoding.
struct Layout {
    /// Offset of the first height sample.
    heights_at: usize,
    /// Offset of the first zone cell.
    zones_at: usize,
    /// Per table: the offset of every record, then the offset just past the last.
    records: [Vec<u32>; TABLE_COUNT],
}

/// The canonical encoding of `place` with its chunk index.
pub fn encode(place: &Place) -> Vec<u8> {
    let mut w = Vec::new();
    let h = &place.header;
    w.extend_from_slice(&PLACE_MAGIC);
    put_u32(&mut w, h.version);
    w.extend_from_slice(&h.seed.to_le_bytes());
    put_f32(&mut w, h.chunk_size_m);
    for v in h.size_chunks {
        w.extend_from_slice(&v.to_le_bytes());
    }
    for v in h.chunk_origin {
        w.extend_from_slice(&v.to_le_bytes());
    }
    w.push(h.region as u8);
    w.push(h.era as u8);
    w.push(u8::from(h.origin_geo.is_some()));
    w.push(0);
    for v in h.origin_geo.unwrap_or([0.0; 2]) {
        w.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    put_u32(&mut w, 0); // index_offset, patched below
    put_u32(&mut w, 0); // file_len, patched below

    let t = &place.terrain;
    w.extend_from_slice(&t.res_per_chunk.to_le_bytes());
    w.extend(t.edges.iter().map(|&e| e as u8));
    put_f32(&mut w, t.water_level);
    put_u32(&mut w, t.heights.len() as u32);
    let heights_at = w.len();
    for &v in &t.heights {
        put_f32(&mut w, v);
    }
    put_u32(&mut w, place.zones.cells.len() as u32);
    let zones_at = w.len();
    w.extend(place.zones.cells.iter().map(|&z| z as u8));
    put_u32(&mut w, t.water.len() as u32);
    for b in &t.water {
        w.push(b.kind as u8);
        put_pts2(&mut w, &b.polygon);
    }

    let mut records: [Vec<u32>; TABLE_COUNT] = Default::default();
    for table in Table::ALL {
        let offsets = &mut records[table as usize];
        put_u32(&mut w, place.table_len(table) as u32);
        for i in 0..place.table_len(table) {
            offsets.push(w.len() as u32);
            put_record(&mut w, place, table, i);
        }
        offsets.push(w.len() as u32);
    }

    let layout = Layout {
        heights_at,
        zones_at,
        records,
    };
    let index = build_index(place, &w, &layout);
    let index_offset = w.len() as u32;
    put_u32(&mut w, index.entries.len() as u32);
    put_u32(&mut w, TABLE_COUNT as u32);
    for e in &index.entries {
        for v in e.bounds_min.iter().chain(&e.bounds_max) {
            put_f32(&mut w, *v);
        }
        w.extend_from_slice(&e.hash.to_le_bytes());
        for r in &e.tables {
            for v in [r.first, r.count, r.offset, r.len] {
                put_u32(&mut w, v);
            }
        }
    }
    let file_len = w.len() as u32;
    w[48..52].copy_from_slice(&index_offset.to_le_bytes());
    w[52..56].copy_from_slice(&file_len.to_le_bytes());
    w
}

/// The index [`encode`] writes for `place`.
pub fn index(place: &Place) -> ChunkIndex {
    let bytes = encode(place);
    read_index(&bytes).expect("encode writes a readable index")
}

fn put_u32(w: &mut Vec<u8>, v: u32) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(w: &mut Vec<u8>, v: f32) {
    w.extend_from_slice(&v.to_bits().to_le_bytes());
}

fn put_f32s(w: &mut Vec<u8>, v: &[f32]) {
    for &x in v {
        put_f32(w, x);
    }
}

fn put_pts2(w: &mut Vec<u8>, pts: &[[f32; 2]]) {
    put_u32(w, pts.len() as u32);
    for p in pts {
        put_f32s(w, p);
    }
}

fn put_pts3(w: &mut Vec<u8>, pts: &[[f32; 3]]) {
    put_u32(w, pts.len() as u32);
    for p in pts {
        put_f32s(w, p);
    }
}

fn put_record(w: &mut Vec<u8>, place: &Place, table: Table, i: usize) {
    match table {
        Table::Nodes => {
            let n = &place.roads.nodes[i];
            put_f32s(w, &n.pos);
            put_f32(w, n.elevation);
        }
        Table::Edges => {
            let e = &place.roads.edges[i];
            put_u32(w, e.a);
            put_u32(w, e.b);
            w.extend_from_slice(&[e.class as u8, e.lanes_fwd, e.lanes_back]);
            put_f32(w, e.width_m);
            w.extend_from_slice(&[u8::from(e.sidewalk), e.speed_kph, u8::from(e.oneway)]);
            put_pts3(w, &e.profile);
        }
        Table::Parcels => {
            let p = &place.parcels[i];
            put_pts2(w, &p.polygon);
            w.push(p.zone as u8);
            put_u32(w, p.frontage_edge);
            w.push(p.lot_kind as u8);
        }
        Table::Buildings => {
            let b = &place.buildings[i];
            put_pts2(w, &b.footprint);
            put_f32(w, b.height_m);
            w.extend_from_slice(&[b.floors, b.roof as u8, b.style as u8]);
            w.push(u8::from(b.built_year.is_some()));
            w.extend_from_slice(&b.built_year.unwrap_or(0).to_le_bytes());
            w.push(b.usage as u8);
        }
        Table::Pois => {
            let p = &place.pois[i];
            put_f32s(w, &p.pos);
            w.push(p.kind as u8);
            w.extend_from_slice(&p.name_seed.to_le_bytes());
        }
        Table::Spawns => {
            let s = &place.spawns[i];
            put_f32s(w, &s.pos);
            put_f32(w, s.heading);
            w.push(s.kind as u8);
        }
        Table::Lanes => {
            let l = &place.lanes.lanes[i];
            put_u32(w, l.edge);
            w.extend_from_slice(&[u8::from(l.forward), l.slot]);
            put_pts3(w, &l.centerline);
        }
        Table::StopLines => {
            let s = &place.lanes.stop_lines[i];
            put_u32(w, s.edge);
            put_u32(w, s.node);
            put_f32s(w, &s.a);
            put_f32s(w, &s.b);
        }
        Table::Turns => {
            let t = &place.lanes.turns[i];
            for v in [t.node, t.from, t.to] {
                put_u32(w, v);
            }
        }
        Table::Sidewalks => put_pts3(w, &place.sidewalks.walks[i].polyline),
        Table::Crossings => {
            let c = &place.sidewalks.crossings[i];
            put_u32(w, c.node);
            put_f32s(w, &c.a);
            put_f32s(w, &c.b);
        }
        Table::Facades => {
            let f = &place.facades.items[i];
            put_u32(w, f.building);
            w.extend_from_slice(&f.side.to_le_bytes());
            w.extend_from_slice(&[f.cols, f.rows, f.ground as u8, u8::from(f.awning)]);
            w.push(f.sign_slots);
            put_u32(w, f.lit.len() as u32);
            w.extend_from_slice(&f.lit);
        }
        Table::Props => {
            let p = &place.props.items[i];
            put_f32s(w, &p.pos);
            put_f32(w, p.yaw);
            w.push(prop_tag(p.kind));
            match p.kind {
                PropKind::Hedge { len_dm } | PropKind::Fence { len_dm } => {
                    w.extend_from_slice(&len_dm.to_le_bytes())
                }
                PropKind::Sign { text_seed } => w.extend_from_slice(&text_seed.to_le_bytes()),
                PropKind::NeonSign { palette } => w.push(palette),
                PropKind::ParkedCar {
                    vehicle_class,
                    hours,
                } => {
                    w.push(vehicle_class);
                    w.extend_from_slice(&hours.to_le_bytes());
                }
                PropKind::Signal { arm_dm } => w.push(arm_dm),
                PropKind::Pier { len_m } => w.push(len_m),
                _ => {}
            }
        }
        Table::Lights => {
            let l = &place.lights.items[i];
            put_f32s(w, &l.pos);
            put_f32s(w, &l.color);
            put_f32(w, l.radius);
            w.push(l.kind as u8);
        }
    }
}

fn prop_tag(k: PropKind) -> u8 {
    match k {
        PropKind::StreetLight => 0,
        PropKind::PalmTree => 1,
        PropKind::Oak => 2,
        PropKind::Hedge { .. } => 3,
        PropKind::Bench => 4,
        PropKind::Hydrant => 5,
        PropKind::Mailbox => 6,
        PropKind::Sign { .. } => 7,
        PropKind::NeonSign { .. } => 8,
        PropKind::Billboard => 9,
        PropKind::PowerPole => 10,
        PropKind::Fence { .. } => 11,
        PropKind::ParkedCar { .. } => 12,
        PropKind::Eucalyptus => 13,
        PropKind::TrashCan => 14,
        PropKind::StopSign => 15,
        PropKind::Signal { .. } => 16,
        PropKind::Pier { .. } => 17,
    }
}

/// Visits every point of record `i` of `table` as `(x, z, y)`; `y` is `None` for ground
/// polygons. Building tops come as `Some(height_m)` with `top: true`.
fn record_points(place: &Place, table: Table, i: usize, f: &mut dyn FnMut(f32, f32, Point)) {
    let flat = |f: &mut dyn FnMut(f32, f32, Point), pts: &[[f32; 2]]| {
        for p in pts {
            f(p[0], p[1], Point::Ground);
        }
    };
    let solid = |f: &mut dyn FnMut(f32, f32, Point), pts: &[[f32; 3]]| {
        for p in pts {
            f(p[0], p[2], Point::At(p[1]));
        }
    };
    match table {
        Table::Nodes => {
            let n = &place.roads.nodes[i];
            f(n.pos[0], n.pos[1], Point::At(n.elevation));
        }
        Table::Edges => solid(f, &place.roads.edges[i].profile),
        Table::Parcels => flat(f, &place.parcels[i].polygon),
        Table::Buildings => {
            let b = &place.buildings[i];
            for p in &b.footprint {
                f(p[0], p[1], Point::Top(b.height_m));
            }
        }
        Table::Pois => flat(f, &[place.pois[i].pos]),
        Table::Spawns => solid(f, &[place.spawns[i].pos]),
        Table::Lanes => solid(f, &place.lanes.lanes[i].centerline),
        Table::StopLines => {
            let s = &place.lanes.stop_lines[i];
            solid(f, &[s.a, s.b]);
        }
        Table::Turns => {}
        Table::Sidewalks => solid(f, &place.sidewalks.walks[i].polyline),
        Table::Crossings => {
            let c = &place.sidewalks.crossings[i];
            solid(f, &[c.a, c.b]);
        }
        Table::Facades => {}
        Table::Props => solid(f, &[place.props.items[i].pos]),
        Table::Lights => solid(f, &[place.lights.items[i].pos]),
    }
}

#[derive(Clone, Copy)]
enum Point {
    /// On the ground: extends x and z only.
    Ground,
    /// At this height.
    At(f32),
    /// This far above the chunk's highest terrain sample.
    Top(f32),
}

/// The chunk index of an encoding: `bytes` holds the grids and records at `layout`.
fn build_index(place: &Place, bytes: &[u8], layout: &Layout) -> ChunkIndex {
    let h = &place.header;
    let chunks = h.chunk_count();
    let res = usize::from(place.terrain.res_per_chunk);
    let [sw, _] = Terrain::samples(h, place.terrain.res_per_chunk);
    let [zw, _] = Zones::cells_per_side(h, place.terrain.res_per_chunk);
    let o = h.origin_m();

    let mut entries: Vec<ChunkEntry> = Vec::with_capacity(chunks);
    for _ in 0..chunks {
        entries.push(ChunkEntry {
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            hash: 0,
            tables: [TableRange::default(); TABLE_COUNT],
        });
    }
    // Table ranges: per slot, the records before it and its own.
    for table in Table::ALL {
        let n = place.table_len(table);
        let mut count = vec![0u32; chunks];
        for i in 0..n {
            let k = place.chunk_key(table, i);
            if let Some(c) = count.get_mut(k as usize) {
                *c += 1;
            }
        }
        let offsets = &layout.records[table as usize];
        let mut first = 0u32;
        for (slot, e) in entries.iter_mut().enumerate() {
            let c = count[slot];
            let (a, b) = (offsets[first as usize], offsets[(first + c) as usize]);
            e.tables[table as usize] = TableRange {
                first,
                count: c,
                offset: a,
                len: b - a,
            };
            first += c;
        }
    }
    let mut scratch = Vec::new();
    for cz in 0..usize::from(h.size_chunks[1]) {
        for cx in 0..usize::from(h.size_chunks[0]) {
            let slot = cz * usize::from(h.size_chunks[0]) + cx;
            let e = &mut entries[slot];
            // Terrain samples and zone cells, then the records.
            scratch.clear();
            let (mut tlo, mut thi) = (f32::MAX, f32::MIN);
            for z in cz * res..=(cz + 1) * res {
                let at = layout.heights_at + 4 * (z * sw + cx * res);
                let row = &bytes[at..at + 4 * (res + 1)];
                scratch.extend_from_slice(row);
                for x in 0..=res {
                    let v = place.terrain.heights[z * sw + cx * res + x];
                    tlo = tlo.min(v);
                    thi = thi.max(v);
                }
            }
            for z in cz * res..(cz + 1) * res {
                let at = layout.zones_at + z * zw + cx * res;
                scratch.extend_from_slice(&bytes[at..at + res]);
            }
            let cs = h.chunk_size_m;
            let x0 = o[0] + cx as f32 * cs;
            let z0 = o[1] + cz as f32 * cs;
            let mut lo = [x0, tlo, z0];
            let mut hi = [x0 + cs, thi, z0 + cs];
            for table in Table::ALL {
                let r = e.tables[table as usize];
                scratch.extend_from_slice(&bytes[r.offset as usize..(r.offset + r.len) as usize]);
                for i in r.first..r.first + r.count {
                    record_points(place, table, i as usize, &mut |x, z, y| {
                        lo[0] = lo[0].min(x);
                        hi[0] = hi[0].max(x);
                        lo[2] = lo[2].min(z);
                        hi[2] = hi[2].max(z);
                        let y = match y {
                            Point::Ground => return,
                            Point::At(y) => y,
                            Point::Top(dy) => thi + dy,
                        };
                        lo[1] = lo[1].min(y);
                        hi[1] = hi[1].max(y);
                    });
                }
            }
            e.bounds_min = lo;
            e.bounds_max = hi;
            e.hash = hash_bytes(0, &scratch);
        }
    }
    ChunkIndex {
        size_chunks: h.size_chunks,
        entries,
    }
}

/// A cursor over encoded bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], PlaceError> {
        let end = self.pos.checked_add(n).ok_or(PlaceError::Truncated)?;
        let s = self.bytes.get(self.pos..end).ok_or(PlaceError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N], PlaceError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u8(&mut self) -> Result<u8, PlaceError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, PlaceError> {
        Ok(u16::from_le_bytes(self.arr()?))
    }

    fn u32(&mut self) -> Result<u32, PlaceError> {
        Ok(u32::from_le_bytes(self.arr()?))
    }

    fn u64(&mut self) -> Result<u64, PlaceError> {
        Ok(u64::from_le_bytes(self.arr()?))
    }

    fn f32(&mut self) -> Result<f32, PlaceError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn f32s<const N: usize>(&mut self) -> Result<[f32; N], PlaceError> {
        let mut a = [0.0; N];
        for v in &mut a {
            *v = self.f32()?;
        }
        Ok(a)
    }

    fn bad(&self) -> PlaceError {
        PlaceError::BadValue {
            offset: (self.pos - 1) as u32,
        }
    }

    /// A byte mapped through `f`, which returns `None` for values out of range.
    fn tag<T>(&mut self, f: fn(u8) -> Option<T>) -> Result<T, PlaceError> {
        let v = self.u8()?;
        f(v).ok_or_else(|| self.bad())
    }

    fn bool(&mut self) -> Result<bool, PlaceError> {
        self.tag(|v| match v {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        })
    }

    /// A list count, checked against the bytes left for records of at least `min` bytes.
    fn count(&mut self, min: usize) -> Result<usize, PlaceError> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min) > self.bytes.len() - self.pos {
            return Err(PlaceError::Truncated);
        }
        Ok(n)
    }

    fn pts2(&mut self) -> Result<Vec<[f32; 2]>, PlaceError> {
        let n = self.count(8)?;
        (0..n).map(|_| self.f32s()).collect()
    }

    fn pts3(&mut self) -> Result<Vec<[f32; 3]>, PlaceError> {
        let n = self.count(12)?;
        (0..n).map(|_| self.f32s()).collect()
    }
}

fn region(v: u8) -> Option<RegionId> {
    Some(match v {
        0 => RegionId::TestTown,
        1 => RegionId::CaliforniaCoast,
        2 => RegionId::GreatLakesCity,
        3 => RegionId::NorthShoreSuburb,
        _ => return None,
    })
}

fn era(v: u8) -> Option<Era> {
    Some(match v {
        0 => Era::Seventies,
        1 => Era::Eighties,
        2 => Era::Nineties,
        3 => Era::Aughts,
        4 => Era::Tens,
        5 => Era::Twenties,
        _ => return None,
    })
}

fn edge_kind(v: u8) -> Option<EdgeKind> {
    Some(match v {
        0 => EdgeKind::Open,
        1 => EdgeKind::Ocean,
        2 => EdgeKind::Lake,
        3 => EdgeKind::Bay,
        4 => EdgeKind::Mountains,
        5 => EdgeKind::Hills,
        6 => EdgeKind::Forest,
        7 => EdgeKind::HighwayLoop,
        8 => EdgeKind::RailYards,
        _ => return None,
    })
}

fn water_kind(v: u8) -> Option<WaterKind> {
    Some(match v {
        0 => WaterKind::Ocean,
        1 => WaterKind::Lake,
        2 => WaterKind::River,
        3 => WaterKind::Canal,
        _ => return None,
    })
}

fn zone(v: u8) -> Option<ZoneId> {
    Some(match v {
        0 => ZoneId::Downtown,
        1 => ZoneId::Strip,
        2 => ZoneId::Residential,
        3 => ZoneId::Suburb,
        4 => ZoneId::Industrial,
        5 => ZoneId::Commercial,
        6 => ZoneId::Park,
        7 => ZoneId::Beach,
        8 => ZoneId::Farmland,
        9 => ZoneId::Hills,
        10 => ZoneId::Highway,
        11 => ZoneId::Water,
        _ => return None,
    })
}

fn road_class(v: u8) -> Option<RoadClass> {
    Some(match v {
        0 => RoadClass::Local,
        1 => RoadClass::Collector,
        2 => RoadClass::Arterial,
        3 => RoadClass::Highway,
        4 => RoadClass::Alley,
        5 => RoadClass::Beachfront,
        6 => RoadClass::Rural,
        _ => return None,
    })
}

fn lot_kind(v: u8) -> Option<LotKind> {
    Some(match v {
        0 => LotKind::Lot,
        1 => LotKind::Park,
        2 => LotKind::LotEmpty,
        _ => return None,
    })
}

fn roof(v: u8) -> Option<Roof> {
    Some(match v {
        0 => Roof::Flat,
        1 => Roof::Gable,
        2 => Roof::Hip,
        3 => Roof::Mansard,
        _ => return None,
    })
}

fn style(v: u8) -> Option<StyleTag> {
    Some(match v {
        0 => StyleTag::StuccoBox,
        1 => StyleTag::Brick,
        2 => StyleTag::Deco,
        3 => StyleTag::StripMall,
        4 => StyleTag::Dingbat,
        5 => StyleTag::Craftsman,
        6 => StyleTag::Ranch,
        7 => StyleTag::Tract70s,
        8 => StyleTag::Glass,
        9 => StyleTag::Warehouse,
        10 => StyleTag::Barn,
        11 => StyleTag::Motel,
        _ => return None,
    })
}

fn usage(v: u8) -> Option<BuildingUse> {
    Some(match v {
        0 => BuildingUse::Residential,
        1 => BuildingUse::Commercial,
        2 => BuildingUse::Office,
        3 => BuildingUse::Industrial,
        4 => BuildingUse::Civic,
        5 => BuildingUse::Hotel,
        6 => BuildingUse::Parking,
        _ => return None,
    })
}

fn poi_kind(v: u8) -> Option<PoiKind> {
    Some(match v {
        0 => PoiKind::Pier,
        1 => PoiKind::Lighthouse,
        2 => PoiKind::Overlook,
        3 => PoiKind::Diner,
        4 => PoiKind::Gas,
        5 => PoiKind::Marina,
        6 => PoiKind::Stadium,
        _ => return None,
    })
}

fn spawn_kind(v: u8) -> Option<SpawnKind> {
    Some(match v {
        0 => SpawnKind::PlayerStart,
        1 => SpawnKind::Garage,
        _ => return None,
    })
}

fn ground_floor(v: u8) -> Option<GroundFloor> {
    Some(match v {
        0 => GroundFloor::Wall,
        1 => GroundFloor::Door,
        2 => GroundFloor::Storefront,
        3 => GroundFloor::Lobby,
        4 => GroundFloor::Garage,
        _ => return None,
    })
}

fn light_kind(v: u8) -> Option<LightKind> {
    Some(match v {
        0 => LightKind::Street,
        1 => LightKind::Neon,
        2 => LightKind::Window,
        3 => LightKind::Headlight,
        _ => return None,
    })
}

/// The fixed header and the two offsets it carries.
struct Head {
    header: PlaceHeader,
    index_offset: usize,
}

fn read_head(bytes: &[u8]) -> Result<Head, PlaceError> {
    let mut r = Reader { bytes, pos: 0 };
    if bytes.len() < 4 || r.arr::<4>()? != PLACE_MAGIC {
        return Err(PlaceError::Magic);
    }
    let version = r.u32()?;
    if version != PLACE_VERSION {
        return Err(PlaceError::Version);
    }
    let seed = r.u64()?;
    let chunk_size_m = r.f32()?;
    let size_chunks = [r.u16()?, r.u16()?];
    let chunk_origin = [r.u16()? as i16, r.u16()? as i16];
    let region = r.tag(region)?;
    let era = r.tag(era)?;
    let geo = r.bool()?;
    if r.u8()? != 0 {
        return Err(r.bad());
    }
    let ll = [f64::from_bits(r.u64()?), f64::from_bits(r.u64()?)];
    if !geo && ll.iter().any(|v| v.to_bits() != 0) {
        return Err(PlaceError::BadValue { offset: 32 });
    }
    let index_offset = r.u32()? as usize;
    let file_len = r.u32()? as usize;
    if bytes.len() < file_len {
        return Err(PlaceError::Truncated);
    }
    if bytes.len() > file_len {
        return Err(PlaceError::TrailingBytes);
    }
    if !(chunk_size_m.is_finite() && chunk_size_m > 0.0) {
        return Err(PlaceError::ChunkSize);
    }
    if !size_chunks
        .iter()
        .all(|s| (1..=MAX_SIZE_CHUNKS).contains(s))
    {
        return Err(PlaceError::PlaceSize);
    }
    let header = PlaceHeader {
        version,
        seed,
        chunk_size_m,
        size_chunks,
        chunk_origin,
        region,
        era,
        origin_geo: geo.then_some(ll),
    };
    let entries = header.chunk_count();
    if index_offset < HEADER_LEN || index_offset + 8 + entries * ENTRY_LEN != file_len {
        return Err(PlaceError::Layout);
    }
    Ok(Head {
        header,
        index_offset,
    })
}

fn read_entry(r: &mut Reader) -> Result<ChunkEntry, PlaceError> {
    let bounds_min = r.f32s()?;
    let bounds_max = r.f32s()?;
    let hash = r.u64()?;
    let mut tables = [TableRange::default(); TABLE_COUNT];
    for t in &mut tables {
        *t = TableRange {
            first: r.u32()?,
            count: r.u32()?,
            offset: r.u32()?,
            len: r.u32()?,
        };
    }
    Ok(ChunkEntry {
        bounds_min,
        bounds_max,
        hash,
        tables,
    })
}

/// The chunk index of an encoded place, read without decoding the tables. Ranges, bounds
/// and hashes are as stored; [`decode`] and [`decode_chunk`] check them.
pub fn read_index(bytes: &[u8]) -> Result<ChunkIndex, PlaceError> {
    let head = read_head(bytes)?;
    let mut r = Reader {
        bytes,
        pos: head.index_offset,
    };
    let n = head.header.chunk_count();
    if r.u32()? as usize != n || r.u32()? as usize != TABLE_COUNT {
        return Err(PlaceError::Layout);
    }
    let entries = (0..n)
        .map(|_| read_entry(&mut r))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChunkIndex {
        size_chunks: head.header.size_chunks,
        entries,
    })
}

/// The records of one table, decoded into the place under construction.
fn read_table(r: &mut Reader, p: &mut Place, table: Table, n: usize) -> Result<(), PlaceError> {
    for _ in 0..n {
        read_record(r, p, table)?;
    }
    Ok(())
}

fn read_record(r: &mut Reader, p: &mut Place, table: Table) -> Result<(), PlaceError> {
    match table {
        Table::Nodes => p.roads.nodes.push(Node {
            pos: r.f32s()?,
            elevation: r.f32()?,
        }),
        Table::Edges => p.roads.edges.push(Edge {
            a: r.u32()?,
            b: r.u32()?,
            class: r.tag(road_class)?,
            lanes_fwd: r.u8()?,
            lanes_back: r.u8()?,
            width_m: r.f32()?,
            sidewalk: r.bool()?,
            speed_kph: r.u8()?,
            oneway: r.bool()?,
            profile: r.pts3()?,
        }),
        Table::Parcels => p.parcels.push(Parcel {
            polygon: r.pts2()?,
            zone: r.tag(zone)?,
            frontage_edge: r.u32()?,
            lot_kind: r.tag(lot_kind)?,
        }),
        Table::Buildings => {
            let footprint = r.pts2()?;
            let height_m = r.f32()?;
            let floors = r.u8()?;
            let roof = r.tag(roof)?;
            let style = r.tag(style)?;
            let has_year = r.bool()?;
            let year = r.u16()?;
            if !has_year && year != 0 {
                return Err(PlaceError::BadValue {
                    offset: (r.pos - 2) as u32,
                });
            }
            p.buildings.push(Building {
                footprint,
                height_m,
                floors,
                roof,
                style,
                built_year: has_year.then_some(year),
                usage: r.tag(usage)?,
            });
        }
        Table::Pois => p.pois.push(Poi {
            pos: r.f32s()?,
            kind: r.tag(poi_kind)?,
            name_seed: r.u64()?,
        }),
        Table::Spawns => p.spawns.push(Spawn {
            pos: r.f32s()?,
            heading: r.f32()?,
            kind: r.tag(spawn_kind)?,
        }),
        Table::Lanes => p.lanes.lanes.push(Lane {
            edge: r.u32()?,
            forward: r.bool()?,
            slot: r.u8()?,
            centerline: r.pts3()?,
        }),
        Table::StopLines => p.lanes.stop_lines.push(StopLine {
            edge: r.u32()?,
            node: r.u32()?,
            a: r.f32s()?,
            b: r.f32s()?,
        }),
        Table::Turns => p.lanes.turns.push(Turn {
            node: r.u32()?,
            from: r.u32()?,
            to: r.u32()?,
        }),
        Table::Sidewalks => p.sidewalks.walks.push(Sidewalk {
            polyline: r.pts3()?,
        }),
        Table::Crossings => p.sidewalks.crossings.push(Crossing {
            node: r.u32()?,
            a: r.f32s()?,
            b: r.f32s()?,
        }),
        Table::Facades => {
            let building = r.u32()?;
            let side = r.u16()?;
            let (cols, rows) = (r.u8()?, r.u8()?);
            let ground = r.tag(ground_floor)?;
            let awning = r.bool()?;
            let sign_slots = r.u8()?;
            let n = r.count(1)?;
            let lit = r.take(n)?.to_vec();
            p.facades.items.push(Facade {
                building,
                side,
                cols,
                rows,
                ground,
                awning,
                sign_slots,
                lit,
            });
        }
        Table::Props => {
            let pos = r.f32s()?;
            let yaw = r.f32()?;
            let kind = match r.u8()? {
                0 => PropKind::StreetLight,
                1 => PropKind::PalmTree,
                2 => PropKind::Oak,
                3 => PropKind::Hedge { len_dm: r.u16()? },
                4 => PropKind::Bench,
                5 => PropKind::Hydrant,
                6 => PropKind::Mailbox,
                7 => PropKind::Sign {
                    text_seed: r.u64()?,
                },
                8 => PropKind::NeonSign { palette: r.u8()? },
                9 => PropKind::Billboard,
                10 => PropKind::PowerPole,
                11 => PropKind::Fence { len_dm: r.u16()? },
                12 => PropKind::ParkedCar {
                    vehicle_class: r.u8()?,
                    hours: r.u32()?,
                },
                13 => PropKind::Eucalyptus,
                14 => PropKind::TrashCan,
                15 => PropKind::StopSign,
                16 => PropKind::Signal { arm_dm: r.u8()? },
                17 => PropKind::Pier { len_m: r.u8()? },
                _ => return Err(r.bad()),
            };
            p.props.items.push(Prop { pos, yaw, kind });
        }
        Table::Lights => p.lights.items.push(Light {
            pos: r.f32s()?,
            color: r.f32s()?,
            radius: r.f32()?,
            kind: r.tag(light_kind)?,
        }),
    }
    Ok(())
}

/// Decodes a whole place and checks its chunk index: every range, bound and hash must
/// match what was decoded. Structural invariants are [`Place::validate`]'s job.
pub fn decode(bytes: &[u8]) -> Result<Place, PlaceError> {
    let head = read_head(bytes)?;
    let h = head.header;
    let mut r = Reader {
        bytes,
        pos: HEADER_LEN,
    };
    let res_per_chunk = r.u16()?;
    let mut edges = [EdgeKind::Open; 4];
    for e in &mut edges {
        *e = r.tag(edge_kind)?;
    }
    let water_level = r.f32()?;
    let [sw, sd] = Terrain::samples(&h, res_per_chunk);
    if res_per_chunk == 0 || r.u32()? as usize != sw * sd {
        return Err(PlaceError::TerrainSize);
    }
    let heights_at = r.pos;
    let heights = (0..sw * sd)
        .map(|_| r.f32())
        .collect::<Result<Vec<_>, _>>()?;
    let [zw, zd] = Zones::cells_per_side(&h, res_per_chunk);
    if r.u32()? as usize != zw * zd {
        return Err(PlaceError::ZoneSize);
    }
    let zones_at = r.pos;
    let cells = (0..zw * zd)
        .map(|_| r.tag(zone))
        .collect::<Result<Vec<_>, _>>()?;
    let n = r.count(5)?;
    let mut water = Vec::new();
    for _ in 0..n {
        water.push(WaterBody {
            kind: r.tag(water_kind)?,
            polygon: r.pts2()?,
        });
    }
    let mut place = Place {
        header: h,
        terrain: Terrain {
            res_per_chunk,
            heights,
            water_level,
            water,
            edges,
        },
        zones: Zones { cells },
        roads: RoadGraph::default(),
        parcels: Vec::new(),
        buildings: Vec::new(),
        pois: Vec::new(),
        spawns: Vec::new(),
        lanes: Lanes::default(),
        sidewalks: Sidewalks::default(),
        facades: Facades::default(),
        props: Props::default(),
        lights: Lights::default(),
    };
    let mut records: [Vec<u32>; TABLE_COUNT] = Default::default();
    for table in Table::ALL {
        let n = r.count(1)?;
        for _ in 0..n {
            records[table as usize].push(r.pos as u32);
            read_record(&mut r, &mut place, table)?;
        }
        records[table as usize].push(r.pos as u32);
    }
    if r.pos != head.index_offset {
        return Err(PlaceError::Layout);
    }
    let stored = read_index(bytes)?;
    let layout = Layout {
        heights_at,
        zones_at,
        records,
    };
    let expected = build_index(&place, bytes, &layout);
    for (i, (s, e)) in stored.entries.iter().zip(&expected.entries).enumerate() {
        let chunk = i as u32;
        if s.tables != e.tables {
            return Err(PlaceError::IndexRange { chunk });
        }
        if s.hash != e.hash {
            return Err(PlaceError::ChunkHash { chunk });
        }
        let bits = |v: &[f32; 3]| v.map(f32::to_bits);
        if bits(&s.bounds_min) != bits(&e.bounds_min) || bits(&s.bounds_max) != bits(&e.bounds_max)
        {
            return Err(PlaceError::IndexBounds { chunk });
        }
    }
    Ok(place)
}

/// What the runtime does with a place file: [`decode`] (which checks the chunk index and
/// hashes), then [`Place::validate`].
pub fn load(bytes: &[u8]) -> Result<Place, PlaceError> {
    let place = decode(bytes)?;
    place.validate()?;
    Ok(place)
}

/// One chunk's share of a place, loaded through the index: its height samples and zone
/// cells, and its records of every table (record `k` of a table is the full place's record
/// `entry.tables[table].first + k`; references inside records stay place-wide indices).
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkData {
    pub header: PlaceHeader,
    pub chunk: [u16; 2],
    pub entry: ChunkEntry,
    /// `(res + 1)^2` samples, row-major (z major), sample `(0, 0)` at the chunk's corner.
    pub heights: Vec<f32>,
    /// `res^2` cells, row-major (z major).
    pub zones: Vec<ZoneId>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub parcels: Vec<Parcel>,
    pub buildings: Vec<Building>,
    pub pois: Vec<Poi>,
    pub spawns: Vec<Spawn>,
    pub lanes: Vec<Lane>,
    pub stop_lines: Vec<StopLine>,
    pub turns: Vec<Turn>,
    pub sidewalks: Vec<Sidewalk>,
    pub crossings: Vec<Crossing>,
    pub facades: Vec<Facade>,
    pub props: Vec<Prop>,
    pub lights: Vec<Light>,
}

/// Decodes chunk `chunk` alone: the header, its index entry, its grid rows and its table
/// ranges. Checks that every range holds exactly its records and that the chunk hash
/// matches.
pub fn decode_chunk(bytes: &[u8], chunk: [u16; 2]) -> Result<ChunkData, PlaceError> {
    let head = read_head(bytes)?;
    let h = head.header;
    if chunk[0] >= h.size_chunks[0] || chunk[1] >= h.size_chunks[1] {
        return Err(PlaceError::ChunkCoord);
    }
    let slot = h.chunk_slot(chunk);
    let mut r = Reader {
        bytes,
        pos: head.index_offset + 8 + slot * ENTRY_LEN,
    };
    let entry = read_entry(&mut r)?;

    let mut g = Reader {
        bytes,
        pos: HEADER_LEN,
    };
    let res_per_chunk = g.u16()?;
    let res = usize::from(res_per_chunk);
    let [sw, sd] = Terrain::samples(&h, res_per_chunk);
    let [zw, zd] = Zones::cells_per_side(&h, res_per_chunk);
    // res_per_chunk, the edges, water_level, the height count.
    let heights_at = HEADER_LEN + 14;
    let zones_at = heights_at + 4 * sw * sd + 4;
    if res == 0 || zones_at + zw * zd > bytes.len() {
        return Err(PlaceError::TerrainSize);
    }
    let (cx, cz) = (usize::from(chunk[0]), usize::from(chunk[1]));
    let mut hashed = Vec::new();
    let mut heights = Vec::with_capacity((res + 1) * (res + 1));
    for z in cz * res..=(cz + 1) * res {
        g.pos = heights_at + 4 * (z * sw + cx * res);
        let row = g.take(4 * (res + 1))?;
        hashed.extend_from_slice(row);
        heights.extend(
            row.chunks_exact(4)
                .map(|b| f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
        );
    }
    let mut zones = Vec::with_capacity(res * res);
    for z in cz * res..(cz + 1) * res {
        g.pos = zones_at + z * zw + cx * res;
        let row = g.take(res)?;
        hashed.extend_from_slice(row);
        for (k, &v) in row.iter().enumerate() {
            let offset = (g.pos - res + k) as u32;
            zones.push(zone(v).ok_or(PlaceError::BadValue { offset })?);
        }
    }

    // Decode the ranges into an otherwise empty place, then move the tables out.
    let mut p = Place {
        header: h.clone(),
        terrain: Terrain {
            res_per_chunk,
            heights: Vec::new(),
            water_level: 0.0,
            water: Vec::new(),
            edges: [EdgeKind::Open; 4],
        },
        zones: Zones::default(),
        roads: RoadGraph::default(),
        parcels: Vec::new(),
        buildings: Vec::new(),
        pois: Vec::new(),
        spawns: Vec::new(),
        lanes: Lanes::default(),
        sidewalks: Sidewalks::default(),
        facades: Facades::default(),
        props: Props::default(),
        lights: Lights::default(),
    };
    let chunk_id = slot as u32;
    for table in Table::ALL {
        let t = entry.tables[table as usize];
        let (a, b) = (t.offset as usize, t.offset as usize + t.len as usize);
        let range = bytes
            .get(a..b)
            .filter(|_| a >= zones_at && b <= head.index_offset)
            .ok_or(PlaceError::IndexRange { chunk: chunk_id })?;
        hashed.extend_from_slice(range);
        let mut tr = Reader { bytes, pos: a };
        let ok = read_table(&mut tr, &mut p, table, t.count as usize).is_ok() && tr.pos == b;
        if !ok {
            return Err(PlaceError::IndexRange { chunk: chunk_id });
        }
    }
    if hash_bytes(0, &hashed) != entry.hash {
        return Err(PlaceError::ChunkHash { chunk: chunk_id });
    }
    Ok(ChunkData {
        header: h,
        chunk,
        entry,
        heights,
        zones,
        nodes: p.roads.nodes,
        edges: p.roads.edges,
        parcels: p.parcels,
        buildings: p.buildings,
        pois: p.pois,
        spawns: p.spawns,
        lanes: p.lanes.lanes,
        stop_lines: p.lanes.stop_lines,
        turns: p.lanes.turns,
        sidewalks: p.sidewalks.walks,
        crossings: p.sidewalks.crossings,
        facades: p.facades.items,
        props: p.props.items,
        lights: p.lights.items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use skywave_core::{Rng, Seed};

    fn wide(rng: &mut Rng) -> u64 {
        (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32())
    }

    fn rect(x0: f32, z0: f32, w: f32, d: f32) -> Vec<[f32; 2]> {
        vec![[x0, z0], [x0, z0 + d], [x0 + w, z0 + d], [x0 + w, z0]]
    }

    /// A small hilly place with every table filled: 3 x 3 chunks at 16 m terrain cells, a
    /// 3 x 3 road grid following the ground, a parcel and a building on each east-west
    /// edge, and detail of every kind.
    fn synthetic(seed: u64) -> Place {
        let mut rng = Rng::new(Seed(seed), &[0x5359_4E54]);
        let header = PlaceHeader {
            version: PLACE_VERSION,
            seed,
            chunk_size_m: 128.0,
            size_chunks: [3, 3],
            chunk_origin: [0, 0],
            region: RegionId::CaliforniaCoast,
            era: Era::Nineties,
            origin_geo: Some([33.75 + seed as f64 * 0.01, -118.25]),
        };
        let mut terrain = Terrain::flat(&header, 8, 0.0);
        for v in &mut terrain.heights {
            *v = rng.range_f32(0.0, 3.0);
        }
        terrain.water_level = 0.5;
        terrain.water.push(WaterBody {
            polygon: rect(330.0, 200.0, 40.0, 40.0),
            kind: WaterKind::Lake,
        });
        let [zw, zd] = Zones::cells_per_side(&header, 8);
        let cells = (0..zw * zd)
            .map(|i| zone((i % 12) as u8).unwrap())
            .collect();
        let ground = |p: [f32; 2]| terrain.height_at(&header, p).unwrap();
        let mut roads = RoadGraph::default();
        let line = [40.0, 150.0, 260.0];
        for z in line {
            for x in line {
                roads.nodes.push(Node {
                    pos: [x, z],
                    elevation: ground([x, z]),
                });
            }
        }
        let mut edge = |a: u32, b: u32, roads: &mut RoadGraph| {
            let (pa, pb) = (roads.nodes[a as usize].pos, roads.nodes[b as usize].pos);
            let profile = (0..=4)
                .map(|k| {
                    let t = k as f32 / 4.0;
                    let p = [pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t];
                    [p[0], ground(p), p[1]]
                })
                .collect();
            roads.edges.push(Edge {
                a,
                b,
                class: road_class(rng.range(0, 7) as u8).unwrap(),
                lanes_fwd: 1,
                lanes_back: 1,
                width_m: 10.0,
                sidewalk: true,
                speed_kph: 40,
                oneway: false,
                profile,
            });
        };
        for z in 0..3 {
            for x in 0..3 {
                let i = z * 3 + x;
                if x < 2 {
                    edge(i, i + 1, &mut roads);
                }
                if z < 2 {
                    edge(i, i + 3, &mut roads);
                }
            }
        }
        let mut parcels = Vec::new();
        let mut buildings = Vec::new();
        let mut facades = Vec::new();
        for (i, e) in roads.edges.iter().enumerate() {
            let (pa, pb) = (roads.nodes[e.a as usize].pos, roads.nodes[e.b as usize].pos);
            if pa[1] != pb[1] {
                continue;
            }
            // South of the road: property line at half width plus sidewalk.
            let z0 = pa[1] + 7.5;
            parcels.push(Parcel {
                polygon: rect(pa[0] + 10.0, z0, 90.0, 30.0),
                zone: ZoneId::Commercial,
                frontage_edge: i as u32,
                lot_kind: LotKind::Lot,
            });
            facades.push(Facade {
                building: buildings.len() as u32,
                side: 1,
                cols: 5,
                rows: 2,
                ground: GroundFloor::Storefront,
                awning: true,
                sign_slots: 2,
                lit: vec![rng.range(0, 256) as u8, 0b11],
            });
            buildings.push(Building {
                footprint: rect(pa[0] + 15.0, z0 + 2.0, rng.range_f32(30.0, 60.0), 20.0),
                height_m: rng.range_f32(4.0, 20.0),
                floors: 2,
                roof: roof(rng.range(0, 4) as u8).unwrap(),
                style: style(rng.range(0, 12) as u8).unwrap(),
                built_year: (i % 2 == 0).then_some(1950 + i as u16),
                usage: usage(rng.range(0, 7) as u8).unwrap(),
            });
        }
        let at = |rng: &mut Rng| {
            let p = [rng.range_f32(1.0, 383.0), rng.range_f32(1.0, 383.0)];
            [p[0], ground(p), p[1]]
        };
        let pois = (0..7)
            .map(|k| {
                let p = at(&mut rng);
                Poi {
                    pos: [p[0], p[2]],
                    kind: poi_kind(k).unwrap(),
                    name_seed: wide(&mut rng),
                }
            })
            .collect();
        let spawns = vec![
            Spawn {
                pos: [45.0, ground([45.0, 42.0]), 42.0],
                heading: -core::f32::consts::FRAC_PI_2,
                kind: SpawnKind::Garage,
            },
            Spawn {
                pos: [200.0, ground([200.0, 152.0]), 152.0],
                heading: 0.0,
                kind: SpawnKind::PlayerStart,
            },
        ];
        let mut lanes = Lanes::default();
        let mut sidewalks = Sidewalks::default();
        for (i, e) in roads.edges.iter().enumerate() {
            for (forward, dz) in [(true, 2.5), (false, -2.5)] {
                let mut c: Vec<[f32; 3]> =
                    e.profile.iter().map(|p| [p[0], p[1], p[2] + dz]).collect();
                if !forward {
                    c.reverse();
                }
                lanes.lanes.push(Lane {
                    edge: i as u32,
                    forward,
                    slot: 0,
                    centerline: c,
                });
            }
            let last = *e.profile.last().unwrap();
            lanes.stop_lines.push(StopLine {
                edge: i as u32,
                node: e.b,
                a: [last[0] - 3.0, last[1], last[2]],
                b: [last[0] - 3.0, last[1], last[2] + 5.0],
            });
            sidewalks.walks.push(Sidewalk {
                polyline: e
                    .profile
                    .iter()
                    .map(|p| [p[0], p[1], p[2] - 6.25])
                    .collect(),
            });
        }
        lanes.turns.push(Turn {
            node: 4,
            from: 0,
            to: 3,
        });
        lanes.turns.push(Turn {
            node: 0,
            from: 1,
            to: 2,
        });
        let c = roads.nodes[4].pos;
        sidewalks.crossings.push(Crossing {
            node: 4,
            a: [c[0] - 6.0, 1.0, c[1] - 7.0],
            b: [c[0] + 6.0, 1.0, c[1] - 7.0],
        });
        let kinds = [
            PropKind::StreetLight,
            PropKind::PalmTree,
            PropKind::Oak,
            PropKind::Hedge { len_dm: 240 },
            PropKind::Bench,
            PropKind::Hydrant,
            PropKind::Mailbox,
            PropKind::Sign {
                text_seed: wide(&mut rng),
            },
            PropKind::NeonSign { palette: 3 },
            PropKind::Billboard,
            PropKind::PowerPole,
            PropKind::Fence { len_dm: 612 },
            PropKind::ParkedCar {
                vehicle_class: 0x32,
                hours: 0x00F0_0F0F,
            },
            PropKind::Eucalyptus,
            PropKind::TrashCan,
            PropKind::StopSign,
            PropKind::Signal { arm_dm: 61 },
            PropKind::Pier { len_m: 180 },
        ];
        let props = kinds
            .iter()
            .map(|&kind| Prop {
                pos: at(&mut rng),
                yaw: rng.range_f32(-3.0, 3.0),
                kind,
            })
            .collect();
        let lights = (0..4)
            .map(|k| Light {
                pos: at(&mut rng),
                color: [1.0, 0.8, 0.5],
                radius: 12.0,
                kind: light_kind(k).unwrap(),
            })
            .collect();
        let mut p = Place {
            header,
            terrain,
            zones: Zones { cells },
            roads,
            parcels,
            buildings,
            pois,
            spawns,
            lanes,
            sidewalks,
            facades: Facades { items: facades },
            props: Props { items: props },
            lights: Lights { items: lights },
        };
        p.sort_by_chunk();
        p
    }

    fn places() -> Vec<Place> {
        let mut v: Vec<Place> = (1..=8)
            .map(|seed| {
                let mut p = Place::test_grid();
                p.header.seed = seed;
                p
            })
            .collect();
        v.extend((1..=4).map(synthetic));
        v
    }

    #[test]
    fn synthetic_places_are_valid() {
        for seed in 1..=4 {
            let p = synthetic(seed);
            assert_eq!(p.validate(), Ok(()), "seed {seed}");
            for t in Table::ALL {
                assert!(p.table_len(t) > 0, "{t:?} is empty");
            }
        }
    }

    #[test]
    fn round_trip_is_identical() {
        for p in places() {
            let bytes = encode(&p);
            assert_eq!(&bytes[..4], b"SKYW");
            let back = load(&bytes).unwrap();
            assert_eq!(back, p);
            assert_eq!(encode(&back), bytes);
            assert_eq!(back.content_hash(), p.content_hash());
        }
    }

    #[test]
    fn header_layout_is_as_documented() {
        let p = synthetic(1);
        let b = encode(&p);
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        assert_eq!(u32_at(4), PLACE_VERSION);
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), 1);
        assert_eq!(f32::from_bits(u32_at(16)), 128.0);
        assert_eq!(&b[20..24], &[3, 0, 3, 0]);
        assert_eq!(&b[24..32], &[0, 0, 0, 0, 1, 2, 1, 0]);
        let lat = f64::from_bits(u64::from_le_bytes(b[32..40].try_into().unwrap()));
        assert_eq!(lat, 33.76);
        let index_offset = u32_at(48) as usize;
        assert_eq!(u32_at(52) as usize, b.len());
        assert_eq!(b.len(), index_offset + 8 + 9 * ENTRY_LEN);
        assert_eq!(ENTRY_LEN, 256);
        assert_eq!(u32_at(index_offset), 9);
        assert_eq!(u32_at(index_offset + 4), TABLE_COUNT as u32);
        assert_eq!(u16::from_le_bytes([b[56], b[57]]), 8);
    }

    #[test]
    fn single_chunk_loads_reproduce_the_full_tables() {
        for p in [Place::test_grid(), synthetic(2)] {
            let bytes = encode(&p);
            let index = read_index(&bytes).unwrap();
            assert_eq!(index, super::index(&p));
            let mut seen = [0usize; TABLE_COUNT];
            let h = &p.header;
            for cz in 0..h.size_chunks[1] {
                for cx in 0..h.size_chunks[0] {
                    let c = decode_chunk(&bytes, [cx, cz]).unwrap();
                    assert_eq!(&c.entry, index.entry([cx, cz]).unwrap());
                    let r = |t: Table| {
                        let t = c.entry.tables[t as usize];
                        t.first as usize..(t.first + t.count) as usize
                    };
                    assert_eq!(c.nodes, p.roads.nodes[r(Table::Nodes)]);
                    assert_eq!(c.edges, p.roads.edges[r(Table::Edges)]);
                    assert_eq!(c.parcels, p.parcels[r(Table::Parcels)]);
                    assert_eq!(c.buildings, p.buildings[r(Table::Buildings)]);
                    assert_eq!(c.pois, p.pois[r(Table::Pois)]);
                    assert_eq!(c.spawns, p.spawns[r(Table::Spawns)]);
                    assert_eq!(c.lanes, p.lanes.lanes[r(Table::Lanes)]);
                    assert_eq!(c.stop_lines, p.lanes.stop_lines[r(Table::StopLines)]);
                    assert_eq!(c.turns, p.lanes.turns[r(Table::Turns)]);
                    assert_eq!(c.sidewalks, p.sidewalks.walks[r(Table::Sidewalks)]);
                    assert_eq!(c.crossings, p.sidewalks.crossings[r(Table::Crossings)]);
                    assert_eq!(c.facades, p.facades.items[r(Table::Facades)]);
                    assert_eq!(c.props, p.props.items[r(Table::Props)]);
                    assert_eq!(c.lights, p.lights.items[r(Table::Lights)]);
                    for t in Table::ALL {
                        for i in r(t) {
                            assert_eq!(p.chunk_of(t, i), Some([cx, cz]), "{t:?} {i}");
                        }
                        seen[t as usize] += r(t).len();
                    }
                    // The chunk's grids are the full grids' rows.
                    let res = usize::from(p.terrain.res_per_chunk);
                    let [sw, _] = Terrain::samples(h, p.terrain.res_per_chunk);
                    let (x0, z0) = (usize::from(cx) * res, usize::from(cz) * res);
                    assert_eq!(
                        c.heights[res + 2],
                        p.terrain.heights[(z0 + 1) * sw + x0 + 1]
                    );
                    let [zw, _] = Zones::cells_per_side(h, p.terrain.res_per_chunk);
                    assert_eq!(c.zones[res * 2 + 3], p.zones.cells[(z0 + 2) * zw + x0 + 3]);
                    // Bounds hold the chunk square and every vertex.
                    let cs = h.chunk_size_m;
                    let o = h.origin_m();
                    assert!(c.entry.bounds_min[0] <= o[0] + f32::from(cx) * cs);
                    for b in &c.buildings {
                        for v in &b.footprint {
                            assert!(v[0] >= c.entry.bounds_min[0] && v[0] <= c.entry.bounds_max[0]);
                            assert!(v[1] >= c.entry.bounds_min[2] && v[1] <= c.entry.bounds_max[2]);
                        }
                        assert!(c.entry.bounds_max[1] >= b.height_m);
                    }
                }
            }
            for t in Table::ALL {
                assert_eq!(seen[t as usize], p.table_len(t), "{t:?}");
            }
            assert_eq!(
                decode_chunk(&bytes, [h.size_chunks[0], 0]),
                Err(PlaceError::ChunkCoord)
            );
        }
    }

    /// Each class of damage to an encoding is caught with its own error.
    #[test]
    fn decode_rejects_corruption() {
        let p = synthetic(3);
        let good = encode(&p);
        let index_offset = u32::from_le_bytes(good[48..52].try_into().unwrap()) as usize;
        let damaged = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = good.clone();
            f(&mut b);
            decode(&b)
        };
        use PlaceError as E;
        assert_eq!(damaged(&|b| b[0] = b'X'), Err(E::Magic));
        assert_eq!(damaged(&|b| b[4] = 9), Err(E::Version));
        assert_eq!(damaged(&|b| b.truncate(30)), Err(E::Truncated));
        assert_eq!(
            damaged(&|b| {
                b.pop();
            }),
            Err(E::Truncated)
        );
        assert_eq!(damaged(&|b| b.push(0)), Err(E::TrailingBytes));
        assert_eq!(damaged(&|b| b[28] = 200), Err(E::BadValue { offset: 28 }));
        assert_eq!(damaged(&|b| b[30] = 2), Err(E::BadValue { offset: 30 }));
        assert_eq!(damaged(&|b| b[20] = 0), Err(E::PlaceSize));
        assert_eq!(damaged(&|b| b[48] ^= 4), Err(E::Layout));
        // A zone cell out of range.
        let res = 8usize;
        let zones_at = HEADER_LEN + 14 + 4 * (3 * res + 1) * (3 * res + 1) + 4;
        assert_eq!(
            damaged(&|b| b[zones_at + 5] = 77),
            Err(E::BadValue {
                offset: zones_at as u32 + 5
            })
        );
        // A building's height: its chunk's hash no longer matches.
        let index = read_index(&good).unwrap();
        let (slot, e) = index
            .entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.tables[Table::Buildings as usize].count > 0)
            .unwrap();
        let b0 = e.tables[Table::Buildings as usize].offset as usize;
        let height_at = b0 + 4 + 8 * p.buildings[e.tables[3].first as usize].footprint.len();
        assert_eq!(
            damaged(&|b| b[height_at] ^= 1),
            Err(E::ChunkHash { chunk: slot as u32 })
        );
        // The stored index itself.
        let entry_at = index_offset + 8 + slot * ENTRY_LEN;
        assert_eq!(
            damaged(&|b| b[entry_at + 24] ^= 1),
            Err(E::ChunkHash { chunk: slot as u32 })
        );
        assert_eq!(
            damaged(&|b| b[entry_at + 3] ^= 0x40),
            Err(E::IndexBounds { chunk: slot as u32 })
        );
        assert_eq!(
            damaged(&|b| b[entry_at + 32 + 16 * 3 + 4] ^= 1),
            Err(E::IndexRange { chunk: slot as u32 })
        );
        // The single-chunk path checks the same things.
        let mut b = good.clone();
        b[height_at] ^= 1;
        let c = h_chunk(&p, slot);
        assert_eq!(
            decode_chunk(&b, c),
            Err(E::ChunkHash { chunk: slot as u32 })
        );
        let mut b = good.clone();
        b[entry_at + 32 + 16 * 3 + 4] ^= 1;
        assert_eq!(
            decode_chunk(&b, c),
            Err(E::IndexRange { chunk: slot as u32 })
        );
    }

    fn h_chunk(p: &Place, slot: usize) -> [u16; 2] {
        let sx = usize::from(p.header.size_chunks[0]);
        [(slot % sx) as u16, (slot / sx) as u16]
    }

    #[cfg(feature = "serde")]
    #[test]
    fn json_round_trip() {
        for p in [Place::test_grid(), synthetic(4)] {
            let json = serde_json::to_string(&p).unwrap();
            let back: Place = serde_json::from_str(&json).unwrap();
            assert_eq!(back, p);
            assert_eq!(encode(&back), encode(&p));
        }
    }
}
