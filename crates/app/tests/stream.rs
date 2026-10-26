//! Acceptance tests for `app::stream`: a camera driven across the test town at 30 m/s for a
//! minute, every step timed by a fake clock that charges a fixed cost per piece, the chunks
//! written into a GPU-less `render::ChunkPool`. Run with `--nocapture` to read the numbers:
//! `cargo test -p app --test stream -- --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use app::stream::{
    ChunkBuild, ChunkPart, ChunkSource, ChunkStep, PartRole, PlaceSource, StaticLight,
    StreamBudget, Streamer, STAGES, STAGE_TILES,
};
use place::Place;
use render::{ChunkPool, Mesh, Vertex};
use world::Palette;

/// Counts heap allocations made on the current thread.
struct CountingAlloc;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn allocations() -> usize {
    ALLOCS.with(|n| n.get())
}

const STEP_HZ: f64 = 120.0;
const DRIVE_S: f64 = 60.0;
const SPEED: f32 = 30.0;

/// A source whose pieces of stages each cost `cost` seconds of the fake clock `now`.
struct Timed<'a, S> {
    inner: &'a S,
    now: &'a Cell<f64>,
    cost: f64,
}

impl<S: ChunkSource> ChunkSource for Timed<'_, S> {
    fn chunk_size(&self) -> f32 {
        self.inner.chunk_size()
    }
    fn chunk_range(&self) -> [[i32; 2]; 2] {
        self.inner.chunk_range()
    }
    fn static_lights(&self) -> &[StaticLight] {
        self.inner.static_lights()
    }
    fn chunk_mesh(&self, chunk: [i32; 2], stage: u8, out: &mut ChunkBuild) -> ChunkStep {
        self.now.set(self.now.get() + self.cost);
        self.inner.chunk_mesh(chunk, stage, out)
    }
}

/// A source that allocates nothing: every chunk is one triangle.
struct Tiny;

impl ChunkSource for Tiny {
    fn chunk_size(&self) -> f32 {
        128.0
    }
    fn chunk_range(&self) -> [[i32; 2]; 2] {
        [[-20, -20], [20, 20]]
    }
    fn static_lights(&self) -> &[StaticLight] {
        &[]
    }
    fn chunk_mesh(&self, chunk: [i32; 2], stage: u8, out: &mut ChunkBuild) -> ChunkStep {
        if stage < STAGE_TILES {
            return ChunkStep::More;
        }
        let x = chunk[0] as f32 * 128.0;
        for p in [[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 0.0, 1.0]] {
            out.mesh
                .vertices
                .push(Vertex::new(p, [0.0, 1.0, 0.0], [255; 4]));
        }
        out.mesh.indices.extend_from_slice(&[0, 1, 2]);
        out.parts.push(ChunkPart {
            range: [0, 3],
            role: PartRole::Tile,
            bounds: None,
        });
        out.ranges.push([0, 3]);
        ChunkStep::Done
    }
}

/// What a drive saw.
#[derive(Debug, Default)]
struct Drive {
    steps: usize,
    /// Steps that ran stages, and the most stages in one.
    busy_steps: usize,
    most_stages: u32,
    /// Most a step went past its budget, seconds of the fake clock.
    max_overrun: f64,
    /// Steps at which the streamer was idle, each checked for full coverage.
    idle_checks: usize,
    /// Allocations in steps (and their syncs) after warmup: all of them, and in steps that
    /// ran no stage.
    allocs_after_warmup: usize,
    allocs_in_quiet_steps: usize,
}

/// Drives the camera from `from` along +x at [`SPEED`] for [`DRIVE_S`], one streaming step
/// per sim step and a sync every second step (60 Hz frames). Checks every step.
fn drive<S: ChunkSource>(
    source: &S,
    budget: StreamBudget,
    cost: f64,
    from: [f32; 2],
    warmup_s: f64,
) -> (Drive, Streamer, ChunkPool) {
    let now = Cell::new(0.0);
    let timed = Timed {
        inner: source,
        now: &now,
        cost,
    };
    let mut streamer = Streamer::new(&timed, budget);
    let mut pool = ChunkPool::new(budget.pool_config());
    let clock = || now.get();
    let mut d = Drive::default();
    let r = budget.radius;
    let [lo, hi] = source.chunk_range();
    let n = (DRIVE_S * STEP_HZ) as usize;
    for i in 0..n {
        let t = i as f64 / STEP_HZ;
        let eye = [from[0] + SPEED * t as f32, from[1]];
        let before = allocations();
        let start = now.get();
        let stages = streamer.step(&timed, eye, [1.0, 0.0], &clock);
        let took = now.get() - start;
        if i % 2 == 1 {
            streamer.sync(&mut pool);
        }
        let allocs = allocations() - before;
        d.steps += 1;
        if stages > 0 {
            d.busy_steps += 1;
            d.most_stages = d.most_stages.max(stages);
        }
        d.max_overrun = d.max_overrun.max(took - budget.step_budget_s);
        if t >= warmup_s {
            d.allocs_after_warmup += allocs;
            if stages == 0 {
                d.allocs_in_quiet_steps += allocs;
            }
        }
        assert!(streamer.stats().resident <= streamer.capacity(), "step {i}");
        assert!(pool.stats().used <= pool.stats().slots);
        if streamer.is_idle() {
            d.idle_checks += 1;
            let c = streamer.chunk_of(eye);
            for dz in -r..=r {
                for dx in -r..=r {
                    let k = [c[0] + dx, c[1] + dz];
                    let inside = (lo[0]..=hi[0]).contains(&k[0]) && (lo[1]..=hi[1]).contains(&k[1]);
                    if inside {
                        assert!(streamer.is_live(k), "step {i}: {k:?} not drawn while idle");
                    }
                }
            }
        }
    }
    (d, streamer, pool)
}

/// The test town streamed at the web budget across its whole width (and out past its
/// apron): a step overruns by at most one piece, the ring is complete whenever the
/// streamer says it is idle, chunks behind are evicted, the pools never overflow, and
/// steps that build nothing allocate nothing.
#[test]
fn the_test_town_streams_across_at_thirty_meters_a_second() {
    let mut place = Place::test_grid();
    place.header.seed = 1;
    let source = PlaceSource::new(place, Palette::socal_1985());
    let budget = StreamBudget::WEB;
    // Stages at 0.7 ms each: a 2 ms step fits two and starts a third.
    let cost = 0.0007;
    let started = std::time::Instant::now();
    let (d, streamer, pool) = drive(&source, budget, cost, [-900.0, 20.0], 1.0);
    let s = streamer.stats();
    let p = pool.stats();
    eprintln!(
        "{:.1} s real: {} steps, {} busy (most {} stages), max overrun {:.2} ms (stage {:.2} ms); \
         {} built, {} evicted, peak {} resident of {}; {} idle checks; pool peak {} slots of {}, \
         {} pages of {} ({:.1} of {:.1} MB); allocations after warmup {} ({} in quiet steps)",
        started.elapsed().as_secs_f64(),
        d.steps,
        d.busy_steps,
        d.most_stages,
        d.max_overrun * 1e3,
        cost * 1e3,
        s.built,
        s.evicted,
        s.peak_resident,
        streamer.capacity(),
        d.idle_checks,
        p.peak,
        p.slots,
        p.pages_peak,
        p.pages,
        p.bytes_used as f64 / 1e6,
        p.bytes as f64 / 1e6,
        d.allocs_after_warmup,
        d.allocs_in_quiet_steps,
    );
    assert!(d.max_overrun <= cost + 1e-12, "{}", d.max_overrun);
    assert_eq!(d.most_stages, 3);
    assert!(d.idle_checks > 1000, "{}", d.idle_checks);
    assert!(s.evicted > 20, "{}", s.evicted);
    assert!(s.peak_resident <= streamer.capacity());
    assert!(p.peak <= p.slots && p.pages_peak <= p.pages);
    assert_eq!(s.upload_errors, 0, "{:?}", s.last_error);
    assert_eq!(d.allocs_in_quiet_steps, 0);
    // It ends idle, past the town: only the apron's chunks are resident.
    assert!(streamer.is_idle());
    // The stages are all that allocates: the world builders' scratch.
    assert!(d.busy_steps > 0 && s.stages >= s.built * u64::from(STAGES));
}

/// With a source that allocates nothing, no step and no sync allocates after warmup,
/// however much they build and evict.
#[test]
fn streaming_allocates_nothing_per_step_after_warmup() {
    let budget = StreamBudget::NATIVE;
    let (d, streamer, pool) = drive(&Tiny, budget, 0.0011, [-1500.0, 0.0], 2.0);
    let s = streamer.stats();
    eprintln!("{d:?}; {s:?}; {:?}", pool.stats());
    assert!(s.built > 200 && s.evicted > 100, "{s:?}");
    assert_eq!(d.allocs_after_warmup, 0);
    assert!(d.max_overrun <= 0.0011 + 1e-12);
    assert_eq!(pool.stats().used as usize, s.resident);
}

/// Every chunk the streamer draws covers its ring: the drawn parts are the built chunks'.
#[test]
fn drawn_parts_are_the_built_chunks() {
    let budget = StreamBudget {
        radius: 2,
        ..StreamBudget::WEB
    };
    let now = Cell::new(0.0);
    let mut streamer = Streamer::new(&Tiny, budget);
    let mut pool = ChunkPool::new(budget.pool_config());
    let clock = || now.get();
    // The clock never moves: one step builds the whole ring, as far as staging allows.
    let mut steps = 0;
    while !streamer.is_idle() {
        streamer.step(&Tiny, [10.0, 10.0], [0.0, -1.0], &clock);
        streamer.sync(&mut pool);
        steps += 1;
        assert!(steps < 100);
    }
    assert_eq!(streamer.drawn().count(), 25);
    for (slot, parts) in streamer.drawn() {
        assert_eq!(parts.len(), 1);
        let id = render::MeshId::chunk_part(slot, 0);
        assert!(pool.draw_range(id).is_some());
    }
    let _ = Mesh::default();
}
