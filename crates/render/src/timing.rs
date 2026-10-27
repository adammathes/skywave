//! GPU time per pass from timestamp queries, and fragments shaded from occlusion queries,
//! where the device has [`wgpu::Features::TIMESTAMP_QUERY`] (native Vulkan, Metal, DX12;
//! not WebGL2). Each pass writes a timestamp at its start and end; with
//! [`wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES`] the parts of a pass (each shadow
//! cascade; the forward pass's depth pre-pass, meshes, late meshes and sky) write theirs
//! from inside it.
//! Occlusion queries count the samples that pass the depth test: in the forward pass one
//! per draw (the fragments it shaded, overdraw included), the sky, and in the shadow pass
//! one per cascade (depth texels written). The frame resolves them into one of a few
//! readback buffers, mapped once the GPU is done with it, so reading never stalls the
//! frame. Results are a frame or two old. Only used for stats: mapping allocates a little
//! per frame inside wgpu, so it is off unless [`crate::Renderer::enable_gpu_timing`] asks.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use crate::CASCADES;

/// The passes timed, in [`crate::RenderStats::gpu_us`] order.
pub const GPU_PASSES: usize = 6;
/// Names of the timed passes.
pub const GPU_PASS_NAMES: [&str; GPU_PASSES] =
    ["shadow", "forward", "corona", "bloom", "tonemap", "hud"];
pub(crate) const PASS_SHADOW: usize = 0;
pub(crate) const PASS_FORWARD: usize = 1;
pub(crate) const PASS_CORONA: usize = 2;
pub(crate) const PASS_BLOOM: usize = 3;
pub(crate) const PASS_TONEMAP: usize = 4;
pub(crate) const PASS_HUD: usize = 5;

/// Parts of passes timed from inside them, in [`crate::RenderStats::gpu_part_us`] order:
/// each shadow cascade, then the forward pass's depth pre-pass, meshes, `late` meshes and
/// sky.
pub const GPU_PARTS: usize = CASCADES + 4;
/// Names of the timed parts.
pub const GPU_PART_NAMES: [&str; GPU_PARTS] = [
    "cascade0", "cascade1", "cascade2", "depth", "meshes", "late", "sky",
];
pub(crate) const PART_DEPTH: usize = CASCADES;
pub(crate) const PART_MESHES: usize = CASCADES + 1;
pub(crate) const PART_LATE: usize = CASCADES + 2;
pub(crate) const PART_SKY: usize = CASCADES + 3;

/// Forward draws counted one by one; later draws in a frame go uncounted (flagged in
/// [`GpuCounts::complete`]).
pub(crate) const MAX_COUNTED_DRAWS: usize = 2048;
/// Readings kept for the medians.
const HISTORY: usize = 64;

const SPANS: usize = GPU_PASSES + GPU_PARTS;
const TIMESTAMPS: u32 = 2 * SPANS as u32;
/// Occlusion queries: the forward draws, the sky, each cascade.
const OCCLUSIONS: u32 = (MAX_COUNTED_DRAWS + 1 + CASCADES) as u32;
const OCC_SKY: u32 = MAX_COUNTED_DRAWS as u32;
const OCC_CASCADE: u32 = OCC_SKY + 1;
const TS_BYTES: u64 = TIMESTAMPS as u64 * wgpu::QUERY_SIZE as u64;
const OCC_BYTES: u64 = OCCLUSIONS as u64 * wgpu::QUERY_SIZE as u64;
const SLOTS: usize = 3;
const FREE: u8 = 0;
const PENDING: u8 = 1;
const READY: u8 = 2;

/// What the forward and shadow passes wrote, from occlusion queries: samples that passed
/// the depth test, so overdraw counts every time a pixel is shaded. Exact on most drivers
/// (Vulkan without precise occlusion may round). All 0 unless GPU timing is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuCounts {
    /// Fragments the meshes shaded (overdraw included), of which the `late` ones.
    pub meshes: u64,
    pub late: u64,
    /// Sky fragments (with the sky drawn last, the pixels nothing else covers).
    pub sky: u64,
    /// Point-light evaluations: each draw's fragments × the mean size of its instances'
    /// light sets (the shader's loop runs that many times per fragment).
    pub light_evals: u64,
    /// Mesh fragments of instances that receive the sun's shadow: each lit one runs the PCF
    /// kernel ([`crate::RenderStats::pcf_taps`] comparisons, twice in a blend band).
    pub shadowed: u64,
    /// Depth texels written into each cascade (0 for one kept from an earlier frame).
    pub depth_texels: [u64; CASCADES],
    /// Every forward draw was counted (false past [`MAX_COUNTED_DRAWS`]).
    pub complete: bool,
}

/// One forward draw's weights for [`GpuCounts`]: its instances' mean light-set size and
/// whether they receive the sun's shadow.
#[derive(Clone, Copy, Default)]
pub(crate) struct DrawWeight {
    pub lights: f32,
    pub shadowed: bool,
    pub late: bool,
}

struct Slot {
    timestamps: wgpu::Buffer,
    occlusion: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// Which spans wrote their timestamps in the frame this slot holds.
    ran: u32,
    /// Which cascades were counted.
    cascades: u8,
    /// The frame's forward draws, counted, and whether its sky was.
    weights: Vec<DrawWeight>,
    sky: bool,
    complete: bool,
    frame: u64,
}

pub(crate) struct GpuTimer {
    timestamps: wgpu::QuerySet,
    occlusion: wgpu::QuerySet,
    ts_resolve: wgpu::Buffer,
    occ_resolve: wgpu::Buffer,
    slots: [Slot; SLOTS],
    /// The slot this frame resolves into, if one was free.
    current: Option<usize>,
    ran: u32,
    cascades: u8,
    weights: Vec<DrawWeight>,
    sky: bool,
    complete: bool,
    /// Timestamps can be written inside passes.
    pub inside: bool,
    /// Nanoseconds per timestamp tick.
    period_ns: f32,
    /// Microseconds per pass and per part of the newest frame read back.
    pub last_us: [u32; GPU_PASSES],
    pub last_part_us: [u32; GPU_PARTS],
    /// Medians over the last [`HISTORY`] readings.
    pub median_us: [u32; GPU_PASSES],
    pub median_part_us: [u32; GPU_PARTS],
    history: [[u32; SPANS]; HISTORY],
    readings: usize,
    pub counts: GpuCounts,
    /// The frame the newest reading is from (0: none yet).
    pub frame: u64,
}

impl GpuTimer {
    /// A timer, when the device can write timestamps in render passes.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuTimer> {
        let features = device.features();
        if !features.contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let set = |label, ty, count| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some(label),
                ty,
                count,
            })
        };
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let resolve = wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC;
        let readback = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;
        let slots = std::array::from_fn(|_| Slot {
            timestamps: buffer("timestamp readback", TS_BYTES, readback),
            occlusion: buffer("occlusion readback", OCC_BYTES, readback),
            state: Arc::new(AtomicU8::new(FREE)),
            ran: 0,
            cascades: 0,
            weights: Vec::with_capacity(MAX_COUNTED_DRAWS),
            sky: false,
            complete: false,
            frame: 0,
        });
        Some(GpuTimer {
            timestamps: set("pass timestamps", wgpu::QueryType::Timestamp, TIMESTAMPS),
            occlusion: set("fragment counts", wgpu::QueryType::Occlusion, OCCLUSIONS),
            ts_resolve: buffer("timestamp resolve", TS_BYTES, resolve),
            occ_resolve: buffer("occlusion resolve", OCC_BYTES, resolve),
            slots,
            current: None,
            ran: 0,
            cascades: 0,
            weights: Vec::with_capacity(MAX_COUNTED_DRAWS),
            sky: false,
            complete: true,
            inside: features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES),
            period_ns: queue.get_timestamp_period(),
            last_us: [0; GPU_PASSES],
            last_part_us: [0; GPU_PARTS],
            median_us: [0; GPU_PASSES],
            median_part_us: [0; GPU_PARTS],
            history: [[0; SPANS]; HISTORY],
            readings: 0,
            counts: GpuCounts::default(),
            frame: 0,
        })
    }

    /// Reads back every finished frame and picks a free slot for this one.
    pub fn begin_frame(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        for i in 0..SLOTS {
            if self.slots[i].state.load(Ordering::Acquire) != READY {
                continue;
            }
            self.read_slot(i);
            let slot = &self.slots[i];
            slot.timestamps.unmap();
            slot.occlusion.unmap();
            slot.state.store(FREE, Ordering::Release);
        }
        self.current = self
            .slots
            .iter()
            .position(|s| s.state.load(Ordering::Acquire) == FREE);
        self.ran = 0;
        self.cascades = 0;
        self.weights.clear();
        self.sky = false;
        self.complete = true;
    }

    fn read_slot(&mut self, i: usize) {
        let slot = &self.slots[i];
        // Slots can finish out of order; keep the newest frame.
        if slot.frame < self.frame {
            return;
        }
        let (Ok(ts), Ok(occ)) = (
            slot.timestamps.get_mapped_range(..),
            slot.occlusion.get_mapped_range(..),
        ) else {
            return;
        };
        let ticks: &[u64] = bytemuck::cast_slice(&ts);
        let samples: &[u64] = bytemuck::cast_slice(&occ);
        let mut spans = [0u32; SPANS];
        for (s, us) in spans.iter_mut().enumerate() {
            if slot.ran & (1 << s) != 0 {
                let dt = ticks[2 * s + 1].saturating_sub(ticks[2 * s]);
                *us = (dt as f64 * f64::from(self.period_ns) / 1000.0).round() as u32;
            }
        }
        self.last_us.copy_from_slice(&spans[..GPU_PASSES]);
        self.last_part_us.copy_from_slice(&spans[GPU_PASSES..]);
        let mut c = GpuCounts {
            complete: slot.complete,
            ..GpuCounts::default()
        };
        for (w, &n) in slot.weights.iter().zip(samples) {
            c.meshes += n;
            if w.late {
                c.late += n;
            }
            c.light_evals += (n as f64 * f64::from(w.lights)).round() as u64;
            if w.shadowed {
                c.shadowed += n;
            }
        }
        if slot.sky {
            c.sky = samples[OCC_SKY as usize];
        }
        for k in 0..CASCADES {
            if slot.cascades & (1 << k) != 0 {
                c.depth_texels[k] = samples[OCC_CASCADE as usize + k];
            }
        }
        self.counts = c;
        self.frame = slot.frame;
        self.history[self.readings % HISTORY] = spans;
        self.readings += 1;
        let n = self.readings.min(HISTORY);
        for s in 0..SPANS {
            let mut v = [0u32; HISTORY];
            for (d, h) in v.iter_mut().zip(&self.history[..n]) {
                *d = h[s];
            }
            v[..n].sort_unstable();
            let m = v[n / 2];
            if s < GPU_PASSES {
                self.median_us[s] = m;
            } else {
                self.median_part_us[s - GPU_PASSES] = m;
            }
        }
    }

    /// Timestamp writes for `pass`: its start (`begin`) and end (`end`), so a pass made of
    /// several render passes (bloom) times from the first's start to the last's end.
    pub fn writes(
        &self,
        pass: usize,
        begin: bool,
        end: bool,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.current?;
        if !begin && !end {
            return None;
        }
        let i = 2 * pass as u32;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.timestamps,
            beginning_of_pass_write_index: begin.then_some(i),
            end_of_pass_write_index: end.then_some(i + 1),
        })
    }

    /// The occlusion query set for a pass that counts fragments.
    pub fn occlusion_set(&self) -> Option<&wgpu::QuerySet> {
        self.current.map(|_| &self.occlusion)
    }

    /// Writes the start (`begin`) or end of `part` from inside a pass, where the device
    /// can; a part whose end is written counts as timed.
    pub fn part(&mut self, pass: &mut wgpu::RenderPass<'_>, part: usize, begin: bool) {
        if self.current.is_none() || !self.inside {
            return;
        }
        let i = 2 * (GPU_PASSES + part) as u32 + u32::from(!begin);
        pass.write_timestamp(&self.timestamps, i);
        if !begin {
            self.ran |= 1 << (GPU_PASSES + part);
        }
    }

    /// Begins counting a forward draw's fragments; false (count nothing) when no slot is
    /// free or the frame has counted [`MAX_COUNTED_DRAWS`] already.
    pub fn begin_draw(&mut self, pass: &mut wgpu::RenderPass<'_>, weight: DrawWeight) -> bool {
        if self.current.is_none() {
            return false;
        }
        if self.weights.len() >= MAX_COUNTED_DRAWS {
            self.complete = false;
            return false;
        }
        pass.begin_occlusion_query(self.weights.len() as u32);
        self.weights.push(weight);
        true
    }

    /// Begins counting the sky's fragments (`None`) or cascade `k`'s depth texels.
    pub fn begin_count(&mut self, pass: &mut wgpu::RenderPass<'_>, cascade: Option<usize>) {
        if self.current.is_none() {
            return;
        }
        match cascade {
            None => {
                pass.begin_occlusion_query(OCC_SKY);
                self.sky = true;
            }
            Some(k) => {
                pass.begin_occlusion_query(OCC_CASCADE + k as u32);
                self.cascades |= 1 << k;
            }
        }
    }

    /// Notes that `pass` wrote both its timestamps this frame.
    pub fn mark(&mut self, pass: usize) {
        self.ran |= 1 << pass;
    }

    /// Copies this frame's queries toward its readback slot.
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.current else {
            return;
        };
        encoder.resolve_query_set(&self.timestamps, 0..TIMESTAMPS, &self.ts_resolve, 0);
        encoder.resolve_query_set(&self.occlusion, 0..OCCLUSIONS, &self.occ_resolve, 0);
        let slot = &self.slots[i];
        encoder.copy_buffer_to_buffer(&self.ts_resolve, 0, &slot.timestamps, 0, TS_BYTES);
        encoder.copy_buffer_to_buffer(&self.occ_resolve, 0, &slot.occlusion, 0, OCC_BYTES);
    }

    /// After frame `frame`'s submit: maps its slot once the GPU is done with it.
    pub fn after_submit(&mut self, frame: u64) {
        let Some(i) = self.current.take() else {
            return;
        };
        let slot = &mut self.slots[i];
        slot.ran = self.ran;
        slot.cascades = self.cascades;
        slot.weights.clear();
        slot.weights.extend_from_slice(&self.weights);
        slot.sky = self.sky;
        slot.complete = self.complete;
        slot.frame = frame;
        slot.state.store(PENDING, Ordering::Release);
        // Both buffers are mapped; the slot is ready once both callbacks have run (a failed
        // map leaves its buffer unreadable, and that reading is skipped).
        let pending = Arc::new(AtomicU8::new(2));
        for buffer in [&slot.timestamps, &slot.occlusion] {
            let state = slot.state.clone();
            let pending = pending.clone();
            buffer.map_async(wgpu::MapMode::Read, .., move |_| {
                if pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                    state.store(READY, Ordering::Release);
                }
            });
        }
    }
}
