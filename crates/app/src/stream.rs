//! Chunk streaming: the town is meshed a chunk at a time, in a ring round the camera, as
//! the sim steps, instead of all at load (`docs/ARCHITECTURE.md`, "Streaming").
//!
//! - A [`ChunkSource`] builds one chunk in [`STAGES`] steps (ground, roads, buildings,
//!   props, then its light tiles) into a reused [`ChunkBuild`]; [`PlaceSource`] does it for
//!   any [`Place`] held in memory (the test town, or a generated one).
//! - The [`Streamer`] keeps the chunks within [`StreamBudget::radius`] of the camera's
//!   chunk resident, nearest first and those ahead before those behind; each sim step it
//!   runs stages until [`StreamBudget::step_budget_s`] is spent (measured with the clock
//!   the platform passes in, checked before each stage, so a step overruns by at most one
//!   stage), and evicts chunks more than one chunk past the radius. Resident chunks live in
//!   a fixed [`Pool`]; finished builds wait in a few staging buffers for
//!   [`Streamer::sync`], which writes them into the renderer's chunk slots
//!   ([`render::ChunkStore`]) and frees the evicted ones'. After warmup nothing here
//!   allocates per step; the stages themselves (the world builders) allocate while they
//!   run.

use place::Place;
use render::{ChunkPoolConfig, ChunkStore, Mesh, PointLight, PoolError, SlotId, MAX_CHUNK_PARTS};
use skywave_core::{Handle, Pool};
use world::{ChunkPlan, MeshKind, Palette, Stage, WorldMesh};

use crate::lighting::{self, Bounds};
use crate::lighttiles;
use crate::worldmesh::atlas_mesh;
use crate::{convert, lamp_heads, Part};

/// Stages of one chunk: the four world stages ([`Stage`]), then the light tiles.
pub const STAGES: u8 = 5;
/// The last stage: cut the chunk's geometry into light tiles and lay out its parts.
pub const STAGE_TILES: u8 = 4;

/// Parts of a chunk that are light tiles: the rest of [`MAX_CHUNK_PARTS`] is its glow and
/// its street lamp lenses.
pub const MAX_CHUNK_TILES: usize = MAX_CHUNK_PARTS - 2;

/// What a stage left to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkStep {
    /// More stages follow.
    More,
    /// The chunk is built: [`ChunkBuild::mesh`] and [`ChunkBuild::parts`] are final.
    Done,
}

/// What a drawn part of a chunk is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PartRole {
    /// Lit geometry, shaded by the lights that reach its bounds.
    #[default]
    Tile,
    /// Neon and signals: glows by itself, drawn with a boosted instance color.
    Glow,
    /// Street lamp lenses, switched by the photocell.
    Lamps,
}

/// One draw of a chunk: a run of its mesh's indices and what it covers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChunkPart {
    /// First index and index count in [`ChunkBuild::mesh`].
    pub range: [u32; 2],
    pub role: PartRole,
    /// Bounds for light culling; `None` for parts that light themselves.
    pub bounds: Option<Bounds>,
}

/// A chunk being built: what the stages have made so far, then its finished mesh. Reused
/// from chunk to chunk, so its buffers keep their capacity.
#[derive(Debug, Default)]
pub struct ChunkBuild {
    pub chunk: [i32; 2],
    /// The lit geometry of every kind so far, in render vertices.
    pub base: Mesh,
    /// Neon and signal triangles so far.
    pub glow: Mesh,
    /// Street lamp lenses so far.
    pub lamps: Mesh,
    /// The finished chunk: its tiles, then its glow and its lamps, one after the other.
    pub mesh: Mesh,
    pub parts: Vec<ChunkPart>,
    /// `parts`' index ranges, as [`ChunkStore::write_chunk`] takes them.
    pub ranges: Vec<[u32; 2]>,
}

impl ChunkBuild {
    fn reset(&mut self, chunk: [i32; 2]) {
        self.chunk = chunk;
        for m in [
            &mut self.base,
            &mut self.glow,
            &mut self.lamps,
            &mut self.mesh,
        ] {
            m.vertices.clear();
            m.indices.clear();
        }
        self.parts.clear();
        self.ranges.clear();
    }

    /// Appends world meshes' render parts (patterned by [`atlas_mesh`]).
    pub fn add_world(&mut self, mesh: &WorldMesh, kind: MeshKind) {
        let m = atlas_mesh(mesh, kind);
        lighttiles::append(&mut self.base, &convert(&m, Part::Base));
        lighttiles::append(&mut self.glow, &convert(&m, Part::Glow));
        lighttiles::append(&mut self.lamps, &convert(&m, Part::Lamps));
    }

    /// The last stage: cuts `base` into light tiles for `lights` (the static lights that
    /// may reach the chunk) on the `chunk_size` grid and lays out [`ChunkBuild::mesh`] and
    /// its parts.
    pub fn finish(&mut self, chunk_size: f32, lights: &[PointLight]) {
        let base = std::mem::take(&mut self.base);
        let tiles = lighttiles::chunk_tiles(base, self.chunk, chunk_size, lights, MAX_CHUNK_TILES);
        for t in &tiles {
            self.push_part(&t.mesh, PartRole::Tile, Some(t.bounds));
        }
        let (glow, lamps) = (
            std::mem::take(&mut self.glow),
            std::mem::take(&mut self.lamps),
        );
        self.push_part(&glow, PartRole::Glow, None);
        self.push_part(&lamps, PartRole::Lamps, None);
        self.glow = glow;
        self.lamps = lamps;
    }

    fn push_part(&mut self, part: &Mesh, role: PartRole, bounds: Option<Bounds>) {
        if part.indices.is_empty() {
            return;
        }
        let range = [self.mesh.indices.len() as u32, part.indices.len() as u32];
        lighttiles::append(&mut self.mesh, part);
        self.parts.push(ChunkPart {
            range,
            role,
            bounds,
        });
        self.ranges.push(range);
    }
}

/// A static light and the chunk whose geometry it belongs to (it is on while that chunk is
/// resident).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StaticLight {
    pub light: PointLight,
    pub chunk: [i32; 2],
}

/// Something that builds chunks: the in-memory test town today, a generated place next.
pub trait ChunkSource {
    /// Chunk edge, meters.
    fn chunk_size(&self) -> f32;
    /// Chunks that may hold anything, `[min, max]` inclusive; the rest are empty.
    fn chunk_range(&self) -> [[i32; 2]; 2];
    /// Every static light, known before any chunk is built, sorted by chunk.
    fn static_lights(&self) -> &[StaticLight];
    /// Runs stage `stage` (`0..STAGES`) of `chunk` into `out`, which holds what the
    /// earlier stages of the same chunk made (and was reset before stage 0).
    fn chunk_mesh(&self, chunk: [i32; 2], stage: u8, out: &mut ChunkBuild) -> ChunkStep;
}

/// A [`Place`] held in memory, built chunk by chunk from one [`ChunkPlan`]. Any place
/// works: the test town, or one a generator made.
pub struct PlaceSource {
    place: Place,
    palette: Palette,
    plan: ChunkPlan,
    statics: Vec<StaticLight>,
}

impl PlaceSource {
    pub fn new(place: Place, palette: Palette) -> PlaceSource {
        let plan = ChunkPlan::new(&place, &palette);
        let mut statics: Vec<StaticLight> = place
            .lights
            .items
            .iter()
            .filter_map(|l| {
                lighting::place_light(l).map(|light| StaticLight {
                    light,
                    chunk: plan.chunk_of([l.pos[0], l.pos[2]]),
                })
            })
            .collect();
        // Until the place lists its street lamps, they hang where the lamp heads are: each
        // street light prop meshed on its own gives the same lenses (bit for bit) as in its
        // chunk's mesh.
        let has_lamps = place
            .lights
            .items
            .iter()
            .any(|l| l.kind == place::LightKind::Street);
        if !has_lamps {
            let kit = world::props::PropKit::new();
            let mut m = WorldMesh::default();
            for p in plan.props() {
                if p.kind != world::props::PropKind::StreetLight {
                    continue;
                }
                m.vertices.clear();
                m.indices.clear();
                world::props::build(&mut m, p, &palette, &kit);
                let lenses = convert(&atlas_mesh(&m, MeshKind::Prop), Part::Lamps);
                let chunk = plan.chunk_of([p.pos.x, p.pos.y]);
                for h in lamp_heads(&lenses) {
                    statics.push(StaticLight {
                        light: lighting::streetlamp(h),
                        chunk,
                    });
                }
            }
        }
        statics.sort_by_key(|s| s.chunk);
        PlaceSource {
            place,
            palette,
            plan,
            statics,
        }
    }

    pub fn place(&self) -> &Place {
        &self.place
    }
}

impl ChunkSource for PlaceSource {
    fn chunk_size(&self) -> f32 {
        self.plan.chunk_size()
    }

    fn chunk_range(&self) -> [[i32; 2]; 2] {
        self.plan.chunk_range()
    }

    fn static_lights(&self) -> &[StaticLight] {
        &self.statics
    }

    fn chunk_mesh(&self, chunk: [i32; 2], stage: u8, out: &mut ChunkBuild) -> ChunkStep {
        if let Some(&s) = Stage::ALL.get(usize::from(stage)) {
            for c in self.plan.build_stage(&self.place, &self.palette, chunk, s) {
                out.add_world(&c.mesh, c.kind);
            }
            return ChunkStep::More;
        }
        let Some(bounds) = lighttiles::mesh_bounds(&out.base) else {
            out.finish(self.chunk_size(), &[]);
            return ChunkStep::Done;
        };
        let near: Vec<PointLight> = self
            .statics
            .iter()
            .map(|s| s.light)
            .filter(|l| bounds.reached_by(l))
            .collect();
        out.finish(self.chunk_size(), &near);
        ChunkStep::Done
    }
}

/// How much streaming a platform affords (`docs/ARCHITECTURE.md`, "Memory budgets").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamBudget {
    /// Chunks kept resident each way from the camera's chunk: `(2r + 1)²` in all.
    pub radius: i32,
    /// Time each sim step may spend building chunks, seconds.
    pub step_budget_s: f64,
    /// Vertices the renderer's chunk pool holds, all slots together.
    pub pool_vertices: u32,
}

impl StreamBudget {
    /// 9×9 chunks, 2 ms a step; a pool for 3M vertices (the whole test town is 2.1M), 126
    /// MB of buffers.
    pub const WEB: StreamBudget = StreamBudget {
        radius: 4,
        step_budget_s: 0.002,
        pool_vertices: 3_000_000,
    };
    /// 13×13 chunks, 4 ms a step; 4M vertices, 168 MB.
    pub const NATIVE: StreamBudget = StreamBudget {
        radius: 6,
        step_budget_s: 0.004,
        pool_vertices: 4_000_000,
    };

    /// Chunks resident at most: the ring, and the ring one chunk out where chunks wait to
    /// be evicted, `(2r + 3)²`.
    pub const fn capacity(&self) -> usize {
        let side = 2 * self.radius as usize + 3;
        side * side
    }

    /// The renderer's chunk pool for this budget: a slot per resident chunk.
    pub const fn pool_config(&self) -> ChunkPoolConfig {
        ChunkPoolConfig::for_vertices(self.capacity() as u32, self.pool_vertices)
    }
}

/// Most resident chunks of any budget: [`StreamBudget::NATIVE`]'s.
pub const MAX_RESIDENT: usize = StreamBudget::NATIVE.capacity();
/// Finished chunks that may wait for [`Streamer::sync`] at once.
pub const STAGING: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Stages running (the current work).
    Building,
    /// Built, waiting in a staging buffer to be written to the GPU.
    Waiting,
    /// Written (or empty): drawn.
    Live,
}

#[derive(Clone, Copy, Debug)]
struct Resident {
    chunk: [i32; 2],
    phase: Phase,
    gpu: Option<SlotId>,
    /// Its parts are `Streamer::parts[pool index][..part_count]`.
    part_count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Work {
    handle: Handle,
    stage: u8,
    staging: usize,
}

/// Streaming counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamStats {
    pub steps: u64,
    pub stages: u64,
    /// Chunks built, evicted.
    pub built: u64,
    pub evicted: u64,
    /// Resident chunks now and at most.
    pub resident: usize,
    pub peak_resident: usize,
    /// Longest step, and most a step went past its budget, seconds.
    pub max_step_s: f64,
    pub max_overrun_s: f64,
    /// Chunks the GPU pool could not take (left undrawn), and the last reason.
    pub upload_errors: u64,
    pub last_error: Option<PoolError>,
}

/// Keeps the chunks round the camera resident (see the module docs).
pub struct Streamer {
    budget: StreamBudget,
    chunk_size: f32,
    range: [[i32; 2]; 2],
    capacity: usize,
    center: Option<[i32; 2]>,
    residents: Box<Pool<Resident, MAX_RESIDENT>>,
    /// The parts of the resident in each pool slot, [`MAX_CHUNK_PARTS`] apiece.
    parts: Vec<ChunkPart>,
    /// Edge of `grid`: the window of radius + 1 round `center`.
    window: i32,
    /// The resident in each chunk of the window, row by row.
    grid: Vec<Option<Handle>>,
    staging: Vec<ChunkBuild>,
    staging_busy: [bool; STAGING],
    current: Option<Work>,
    /// Built chunks for `sync` to write: resident and staging buffer.
    ready: Vec<(Handle, usize)>,
    /// GPU slots of evicted chunks, for `sync` to free.
    to_free: Vec<SlotId>,
    /// Scratch for eviction.
    evicting: Vec<Handle>,
    /// Nothing in the ring is missing (since the camera last changed chunk).
    complete: bool,
    /// The drawn set changed since the last `sync`.
    changed: bool,
    stats: StreamStats,
}

impl Streamer {
    /// A streamer for `source`'s chunks within `budget`; nothing resident yet. All its
    /// memory is allocated here.
    pub fn new(source: &impl ChunkSource, budget: StreamBudget) -> Streamer {
        let budget = StreamBudget {
            radius: budget.radius.clamp(0, 6),
            ..budget
        };
        let window = 2 * (budget.radius + 1) + 1;
        Streamer {
            budget,
            chunk_size: source.chunk_size(),
            range: source.chunk_range(),
            capacity: budget.capacity().min(MAX_RESIDENT),
            center: None,
            residents: Box::new(Pool::new()),
            parts: vec![ChunkPart::default(); MAX_RESIDENT * MAX_CHUNK_PARTS],
            window,
            grid: vec![None; (window * window) as usize],
            staging: (0..STAGING).map(|_| ChunkBuild::default()).collect(),
            staging_busy: [false; STAGING],
            current: None,
            ready: Vec::with_capacity(STAGING),
            to_free: Vec::with_capacity(MAX_RESIDENT),
            evicting: Vec::with_capacity(MAX_RESIDENT),
            complete: false,
            changed: false,
            stats: StreamStats::default(),
        }
    }

    pub fn budget(&self) -> StreamBudget {
        self.budget
    }

    /// Resident chunks at most.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn stats(&self) -> StreamStats {
        self.stats
    }

    /// The chunk holding ground point `[x, z]`.
    pub fn chunk_of(&self, p: [f32; 2]) -> [i32; 2] {
        [
            (p[0] / self.chunk_size).floor() as i32,
            (p[1] / self.chunk_size).floor() as i32,
        ]
    }

    /// Nothing to do: every chunk of the ring is drawn (or empty) and the GPU is up to
    /// date.
    pub fn is_idle(&self) -> bool {
        self.center.is_some()
            && self.complete
            && self.current.is_none()
            && self.ready.is_empty()
            && self.to_free.is_empty()
    }

    fn cell(&self, c: [i32; 2]) -> Option<usize> {
        let o = self.center?;
        let r = self.budget.radius + 1;
        let (dx, dz) = (c[0] - o[0], c[1] - o[1]);
        (dx.abs() <= r && dz.abs() <= r).then(|| ((dz + r) * self.window + dx + r) as usize)
    }

    /// Whether `chunk` is resident and drawn (an empty chunk counts).
    pub fn is_live(&self, chunk: [i32; 2]) -> bool {
        self.cell(chunk)
            .and_then(|i| self.grid[i])
            .and_then(|h| self.residents.get(h))
            .is_some_and(|r| r.phase == Phase::Live)
    }

    /// Resident chunks (any phase), in pool order.
    pub fn resident_chunks(&self) -> impl Iterator<Item = [i32; 2]> + '_ {
        self.residents.iter().map(|(_, r)| r.chunk)
    }

    /// What to draw: every live chunk's GPU slot and parts, in pool order.
    pub fn drawn(&self) -> impl Iterator<Item = (SlotId, &[ChunkPart])> + '_ {
        self.residents
            .iter()
            .filter_map(|(h, r)| match (r.phase, r.gpu) {
                (Phase::Live, Some(slot)) => {
                    let all = &self.parts[h.index as usize * MAX_CHUNK_PARTS..];
                    Some((slot, &all[..usize::from(r.part_count)]))
                }
                _ => None,
            })
    }

    fn in_range(&self, c: [i32; 2]) -> bool {
        let [lo, hi] = self.range;
        (lo[0]..=hi[0]).contains(&c[0]) && (lo[1]..=hi[1]).contains(&c[1])
    }

    fn release_staging(&mut self, s: usize) {
        self.staging_busy[s] = false;
    }

    /// Moves the ring to `center`: evicts the chunks more than radius + 1 away.
    fn recenter(&mut self, center: [i32; 2]) {
        let keep = self.budget.radius + 1;
        self.evicting.clear();
        for (h, r) in self.residents.iter() {
            if (r.chunk[0] - center[0]).abs() > keep || (r.chunk[1] - center[1]).abs() > keep {
                self.evicting.push(h);
            }
        }
        for k in 0..self.evicting.len() {
            let h = self.evicting[k];
            let Some(r) = self.residents.remove(h) else {
                continue;
            };
            self.stats.evicted += 1;
            if let Some(slot) = r.gpu {
                self.to_free.push(slot);
            }
            if r.phase == Phase::Live {
                self.changed = true;
            }
            if let Some(i) = self.ready.iter().position(|&(rh, _)| rh == h) {
                let (_, s) = self.ready.remove(i);
                self.release_staging(s);
            }
            if let Some(w) = self.current.filter(|w| w.handle == h) {
                self.release_staging(w.staging);
                self.current = None;
            }
        }
        self.center = Some(center);
        self.grid.fill(None);
        for (h, r) in self.residents.iter() {
            let (dx, dz) = (r.chunk[0] - center[0], r.chunk[1] - center[1]);
            self.grid[((dz + keep) * self.window + dx + keep) as usize] = Some(h);
        }
        self.complete = false;
    }

    /// The nearest chunk of the ring that is not resident, those ahead along `heading`
    /// (`[x, z]`) before those behind at the same distance.
    fn next_missing(&self, heading: [f32; 2]) -> Option<[i32; 2]> {
        let o = self.center?;
        let r = self.budget.radius;
        let mut best: Option<(i32, f32, [i32; 2])> = None;
        for dz in -r..=r {
            for dx in -r..=r {
                let c = [o[0] + dx, o[1] + dz];
                if !self.in_range(c) || self.cell(c).and_then(|i| self.grid[i]).is_some() {
                    continue;
                }
                let d2 = dx * dx + dz * dz;
                let ahead = dx as f32 * heading[0] + dz as f32 * heading[1];
                if best.is_none_or(|(bd, ba, _)| d2 < bd || (d2 == bd && ahead > ba)) {
                    best = Some((d2, ahead, c));
                }
            }
        }
        best.map(|(_, _, c)| c)
    }

    /// One sim step: follows the camera at ground point `eye` (`[x, z]`) looking along
    /// `heading`, then builds stages of the nearest missing chunks until the step's
    /// budget is spent, as `now` (seconds, any origin) measures it. Returns the stages
    /// run.
    pub fn step(
        &mut self,
        source: &impl ChunkSource,
        eye: [f32; 2],
        heading: [f32; 2],
        now: &dyn Fn() -> f64,
    ) -> u32 {
        let start = now();
        self.stats.steps += 1;
        let center = self.chunk_of(eye);
        if self.center != Some(center) {
            self.recenter(center);
        }
        let mut stages = 0;
        while now() - start < self.budget.step_budget_s {
            let work = match self.current {
                Some(w) => w,
                None => {
                    if self.complete || self.ready.len() >= STAGING {
                        break;
                    }
                    let Some(c) = self.next_missing(heading) else {
                        self.complete = true;
                        break;
                    };
                    let Some(s) = self.staging_busy.iter().position(|b| !b) else {
                        break;
                    };
                    if self.residents.len() >= self.capacity {
                        break;
                    }
                    let resident = Resident {
                        chunk: c,
                        phase: Phase::Building,
                        gpu: None,
                        part_count: 0,
                    };
                    let Some(handle) = self.residents.insert(resident) else {
                        break;
                    };
                    if let Some(i) = self.cell(c) {
                        self.grid[i] = Some(handle);
                    }
                    self.staging_busy[s] = true;
                    self.staging[s].reset(c);
                    Work {
                        handle,
                        stage: 0,
                        staging: s,
                    }
                }
            };
            let chunk = self.staging[work.staging].chunk;
            let step = source.chunk_mesh(chunk, work.stage, &mut self.staging[work.staging]);
            stages += 1;
            self.stats.stages += 1;
            if step == ChunkStep::Done || work.stage + 1 >= STAGES {
                if let Some(r) = self.residents.get_mut(work.handle) {
                    r.phase = Phase::Waiting;
                }
                self.ready.push((work.handle, work.staging));
                self.current = None;
                self.stats.built += 1;
            } else {
                self.current = Some(Work {
                    stage: work.stage + 1,
                    ..work
                });
            }
        }
        let took = now() - start;
        self.stats.max_step_s = self.stats.max_step_s.max(took);
        self.stats.max_overrun_s = self
            .stats
            .max_overrun_s
            .max(took - self.budget.step_budget_s);
        self.stats.resident = self.residents.len();
        self.stats.peak_resident = self.stats.peak_resident.max(self.stats.resident);
        stages
    }

    /// Frees the evicted chunks' GPU slots and writes the built ones into `gpu`. Returns
    /// whether the drawn set changed. A chunk the pool has no room for yet waits for the
    /// next sync; one it cannot ever take is left undrawn and counted in
    /// [`StreamStats::upload_errors`].
    pub fn sync(&mut self, gpu: &mut impl ChunkStore) -> bool {
        for k in 0..self.to_free.len() {
            if let Err(e) = gpu.free_chunk_slot(self.to_free[k]) {
                self.stats.upload_errors += 1;
                self.stats.last_error = Some(e);
            }
        }
        self.to_free.clear();
        let mut k = 0;
        while k < self.ready.len() {
            let (h, s) = self.ready[k];
            let build = &self.staging[s];
            let Some(r) = self.residents.get_mut(h) else {
                self.ready.remove(k);
                self.staging_busy[s] = false;
                continue;
            };
            if build.parts.is_empty() {
                r.phase = Phase::Live;
                self.ready.remove(k);
                self.staging_busy[s] = false;
                continue;
            }
            let slot = match r.gpu {
                Some(slot) => slot,
                None => match gpu.alloc_chunk_slot() {
                    Ok(slot) => slot,
                    Err(e) => {
                        self.stats.last_error = Some(e);
                        break;
                    }
                },
            };
            r.gpu = Some(slot);
            match gpu.write_chunk(slot, &build.mesh, &build.ranges) {
                Ok(()) => {
                    let n = build.parts.len().min(MAX_CHUNK_PARTS);
                    self.parts[h.index as usize * MAX_CHUNK_PARTS..][..n]
                        .copy_from_slice(&build.parts[..n]);
                    r.part_count = n as u8;
                    r.phase = Phase::Live;
                    self.changed = true;
                }
                Err(e @ PoolError::OutOfSpace { .. }) => {
                    // Evictions will free pages: try again next time.
                    self.stats.last_error = Some(e);
                    k += 1;
                    continue;
                }
                Err(e) => {
                    self.stats.upload_errors += 1;
                    self.stats.last_error = Some(e);
                    let _ = gpu.free_chunk_slot(slot);
                    r.gpu = None;
                    r.phase = Phase::Live;
                }
            }
            self.ready.remove(k);
            self.staging_busy[s] = false;
        }
        std::mem::take(&mut self.changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render::MAX_CHUNK_VERTICES;

    /// Every chunk of the test town fits a pool slot, its parts index its own mesh, and
    /// the stages cost what they cost (printed: debug build).
    #[test]
    fn every_test_town_chunk_fits_the_pool() {
        let mut place = Place::test_grid();
        place.header.seed = 1;
        let t = std::time::Instant::now();
        let src = PlaceSource::new(place, Palette::socal_1985());
        let made = t.elapsed();
        let [lo, hi] = src.chunk_range();
        let mut b = ChunkBuild::default();
        let (mut stage_ms, mut worst) = ([0.0f64; STAGES as usize], [0.0f64; STAGES as usize]);
        let (mut vertices, mut most, mut parts, mut most_parts) = (0, 0, 0, 0);
        let mut roles = [0; 3];
        for z in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                b.reset([x, z]);
                for s in 0..STAGES {
                    let t = std::time::Instant::now();
                    let step = src.chunk_mesh([x, z], s, &mut b);
                    assert_eq!(step == ChunkStep::Done, s == STAGE_TILES);
                    let ms = t.elapsed().as_secs_f64() * 1e3;
                    stage_ms[s as usize] += ms;
                    worst[s as usize] = worst[s as usize].max(ms);
                }
                let m = &b.mesh;
                assert!(m.vertices.len() <= MAX_CHUNK_VERTICES as usize);
                assert!(b.parts.len() <= MAX_CHUNK_PARTS);
                assert_eq!(b.parts.len(), b.ranges.len());
                // The parts tile the mesh's indices in order.
                let mut next = 0;
                for (p, r) in b.parts.iter().zip(&b.ranges) {
                    assert_eq!((p.range, p.range[0]), (*r, next));
                    next += p.range[1];
                    roles[p.role as usize] += 1;
                    assert_eq!(p.bounds.is_some(), p.role == PartRole::Tile);
                }
                assert_eq!(next as usize, m.indices.len());
                assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
                vertices += m.vertices.len();
                most = most.max(m.vertices.len());
                parts += b.parts.len();
                most_parts = most_parts.max(b.parts.len());
            }
        }
        eprintln!(
            "source {made:?}, {} statics; {vertices} vertices (most {most} in a chunk), \
             {parts} parts (most {most_parts}; tiles/glow/lamps {roles:?}); \
             stage ms total {stage_ms:.1?}, worst {worst:.2?}",
            src.static_lights().len()
        );
        assert!(src.static_lights().len() >= 300);
        let pool = StreamBudget::WEB.pool_config();
        assert!(u64::from(pool.pages) * u64::from(pool.page_vertices) > vertices as u64);
    }
}
