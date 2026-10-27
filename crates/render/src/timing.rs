//! GPU time per pass from timestamp queries, where the device has
//! [`wgpu::Features::TIMESTAMP_QUERY`] (native Vulkan, Metal, DX12; not WebGL2). Each pass
//! writes a timestamp at its start and end; the frame resolves them into one of a few
//! readback buffers, mapped once the GPU is done with it, so reading never stalls the
//! frame. Results are a frame or two old. Only used for stats: mapping allocates a little
//! per frame inside wgpu, so it is off unless [`crate::Renderer::enable_gpu_timing`] asks.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

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

const QUERIES: u32 = 2 * GPU_PASSES as u32;
const BYTES: u64 = QUERIES as u64 * wgpu::QUERY_SIZE as u64;
const SLOTS: usize = 3;
const FREE: u8 = 0;
const PENDING: u8 = 1;
const READY: u8 = 2;

struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// Which passes wrote their timestamps in the frame this slot holds.
    ran: u8,
}

pub(crate) struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: [Slot; SLOTS],
    /// The slot this frame resolves into, if one was free.
    current: Option<usize>,
    ran: u8,
    /// Nanoseconds per timestamp tick.
    period_ns: f32,
    /// Microseconds per pass of the newest frame read back.
    pub last_us: [u32; GPU_PASSES],
}

impl GpuTimer {
    /// A timer, when the device can write timestamps in render passes.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuTimer> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("pass timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: QUERIES,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp resolve"),
            size: BYTES,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = std::array::from_fn(|_| Slot {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timestamp readback"),
                size: BYTES,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(FREE)),
            ran: 0,
        });
        Some(GpuTimer {
            queries,
            resolve,
            slots,
            current: None,
            ran: 0,
            period_ns: queue.get_timestamp_period(),
            last_us: [0; GPU_PASSES],
        })
    }

    /// Reads back every finished frame and picks a free slot for this one.
    pub fn begin_frame(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        for slot in &mut self.slots {
            if slot.state.load(Ordering::Acquire) != READY {
                continue;
            }
            if let Ok(view) = slot.buffer.get_mapped_range(..) {
                let ticks: &[u64] = bytemuck::cast_slice(&view);
                for (p, us) in self.last_us.iter_mut().enumerate() {
                    *us = if slot.ran & (1 << p) != 0 {
                        let dt = ticks[2 * p + 1].saturating_sub(ticks[2 * p]);
                        (dt as f64 * f64::from(self.period_ns) / 1000.0).round() as u32
                    } else {
                        0
                    };
                }
            }
            slot.buffer.unmap();
            slot.state.store(FREE, Ordering::Release);
        }
        self.current = self
            .slots
            .iter()
            .position(|s| s.state.load(Ordering::Acquire) == FREE);
        self.ran = 0;
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
        let i = 2 * pass as u32;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: begin.then_some(i),
            end_of_pass_write_index: end.then_some(i + 1),
        })
    }

    /// Notes that `pass` wrote both its timestamps this frame.
    pub fn mark(&mut self, pass: usize) {
        self.ran |= 1 << pass;
    }

    /// Copies this frame's timestamps toward its readback slot.
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.current else {
            return;
        };
        encoder.resolve_query_set(&self.queries, 0..QUERIES, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.slots[i].buffer, 0, BYTES);
    }

    /// After the frame's submit: maps its slot once the GPU is done with it.
    pub fn after_submit(&mut self) {
        let Some(i) = self.current.take() else {
            return;
        };
        let slot = &mut self.slots[i];
        slot.ran = self.ran;
        slot.state.store(PENDING, Ordering::Release);
        let state = slot.state.clone();
        slot.buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            state.store(if r.is_ok() { READY } else { FREE }, Ordering::Release);
        });
    }
}
