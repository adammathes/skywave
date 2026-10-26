//! Pooled chunk geometry: streamed chunks live in one vertex buffer and one index buffer
//! made at load (`docs/RENDERING.md`, "Mesh and instance budgets"). A chunk takes a slot
//! ([`ChunkStore::alloc_chunk_slot`]), writes its mesh into it with the index runs of its
//! draws ([`ChunkStore::write_chunk`]) and gives it back when it is evicted
//! ([`ChunkStore::free_chunk_slot`]); instances draw a run by its [`MeshId::chunk_part`].
//! Nothing grows: a full pool or an oversized chunk is an error, never a new buffer.
//!
//! The buffers are cut into equal pages (a page holds `page_vertices` vertices and
//! `page_indices` indices), and a written slot holds a run of consecutive pages, as many as
//! its mesh needs (first fit). Chunks differ by three orders of magnitude (a terrain apron
//! chunk is a hundred vertices, a downtown one a hundred thousand), so slots of one fixed
//! size would have to be the largest and the pool many times the resident geometry.
//!
//! [`ChunkPool`] is the bookkeeping alone (slots, pages, part table, counters, the rebased
//! index scratch) with no GPU in it, so streaming can be tested without a device; the
//! [`crate::Renderer`] holds one and copies what it lays out into its buffers.
//!
//! WebGL2 has no base vertex, so indices are stored rebased to the slot's first vertex and
//! a draw is a plain index range of the shared index buffer.

use crate::mesh::{Mesh, Vertex};
use crate::MeshId;

/// Most draws (index runs) one chunk slot holds.
pub const MAX_CHUNK_PARTS: usize = 32;
const PART_BITS: u32 = 5;
const _: () = assert!(1 << PART_BITS == MAX_CHUNK_PARTS);
/// Marks a [`MeshId`] as a chunk part rather than an [`crate::Renderer::add_mesh`] mesh.
const CHUNK_BIT: u32 = 1 << 31;
/// Most vertices one chunk may have (`docs/ARCHITECTURE.md`, "Memory budgets").
pub const MAX_CHUNK_VERTICES: u32 = 256 * 1024;

/// Handle of one slot of the chunk pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotId(pub u32);

impl MeshId {
    /// Draw `part` of the chunk in `slot`.
    pub const fn chunk_part(slot: SlotId, part: usize) -> MeshId {
        MeshId(CHUNK_BIT | slot.0 << PART_BITS | part as u32 & (MAX_CHUNK_PARTS as u32 - 1))
    }

    /// The slot and part this id draws, when it is a chunk part.
    pub const fn as_chunk_part(self) -> Option<(SlotId, usize)> {
        if self.0 & CHUNK_BIT == 0 {
            return None;
        }
        let rest = self.0 & !CHUNK_BIT;
        Some((
            SlotId(rest >> PART_BITS),
            (rest & (MAX_CHUNK_PARTS as u32 - 1)) as usize,
        ))
    }
}

/// Size of the chunk pool, fixed at load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkPoolConfig {
    /// Chunks the pool holds at once.
    pub slots: u32,
    /// Pages in the buffers.
    pub pages: u32,
    pub page_vertices: u32,
    pub page_indices: u32,
}

impl ChunkPoolConfig {
    /// No pool (static meshes only).
    pub const NONE: ChunkPoolConfig = ChunkPoolConfig {
        slots: 0,
        pages: 0,
        page_vertices: 0,
        page_indices: 0,
    };

    /// Vertices per page: a few hundred pages for a town.
    pub const PAGE_VERTICES: u32 = 4096;
    /// Indices per page: the town's chunks have 1.3 to 1.45 indices per vertex.
    pub const PAGE_INDICES: u32 = 6144;

    /// Room for `slots` chunks and `vertices` vertices in all, in pages of the default size.
    pub const fn for_vertices(slots: u32, vertices: u32) -> ChunkPoolConfig {
        ChunkPoolConfig {
            slots,
            pages: vertices.div_ceil(Self::PAGE_VERTICES),
            page_vertices: Self::PAGE_VERTICES,
            page_indices: Self::PAGE_INDICES,
        }
    }

    /// Size of the whole vertex buffer, bytes.
    pub const fn vertex_bytes(&self) -> u64 {
        self.pages as u64 * self.page_vertices as u64 * size_of::<Vertex>() as u64
    }

    /// Size of the whole index buffer, bytes.
    pub const fn index_bytes(&self) -> u64 {
        self.pages as u64 * self.page_indices as u64 * 4
    }
}

/// Why the pool refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolError {
    /// Every slot is taken.
    Full,
    /// No run of free pages is long enough for the mesh now (evictions may free one).
    OutOfSpace { pages: u32 },
    /// The slot is not allocated (never was, or was freed).
    BadSlot(SlotId),
    /// The mesh is over [`MAX_CHUNK_VERTICES`] or larger than the whole pool.
    TooBig { vertices: usize, indices: usize },
    /// More parts than [`MAX_CHUNK_PARTS`].
    TooManyParts(usize),
    /// A part's index run is not inside the mesh's indices, or an index is past its
    /// vertices.
    BadRange,
}

impl core::fmt::Display for PoolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PoolError::Full => write!(f, "every chunk slot is taken"),
            PoolError::OutOfSpace { pages } => {
                write!(f, "no run of {pages} free pages in the chunk pool")
            }
            PoolError::BadSlot(s) => write!(f, "chunk slot {} is not allocated", s.0),
            PoolError::TooBig { vertices, indices } => write!(
                f,
                "chunk of {vertices} vertices, {indices} indices is over the chunk budget"
            ),
            PoolError::TooManyParts(n) => {
                write!(f, "chunk of {n} parts (at most {MAX_CHUNK_PARTS})")
            }
            PoolError::BadRange => write!(f, "chunk part range outside its mesh"),
        }
    }
}

/// Chunk pool counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub slots: u32,
    /// Slots allocated now.
    pub used: u32,
    /// Most slots ever allocated at once.
    pub peak: u32,
    pub pages: u32,
    /// Pages written slots hold now.
    pub pages_used: u32,
    /// Most pages ever held at once.
    pub pages_peak: u32,
    /// Chunks written so far.
    pub writes: u64,
    /// Size of the pool's two buffers, bytes.
    pub bytes: u64,
    /// Bytes the live slots' meshes take.
    pub bytes_used: u64,
}

/// Where a checked write goes and the indices to copy (rebased to the slot).
#[derive(Debug)]
pub struct ChunkWrite<'a> {
    /// Byte offset of the slot's first vertex in the vertex buffer.
    pub vertex_offset: u64,
    /// Byte offset of the slot's first index in the index buffer.
    pub index_offset: u64,
    pub indices: &'a [u32],
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    live: bool,
    first_page: u32,
    /// 0 until written.
    page_count: u32,
    part_count: u8,
    bytes: u64,
}

/// The chunk pool's bookkeeping (see the module docs). Allocates only in [`ChunkPool::new`].
#[derive(Debug)]
pub struct ChunkPool {
    config: ChunkPoolConfig,
    slots: Vec<Slot>,
    /// Free slots, a stack: the most recently freed is reused first.
    free: Vec<u32>,
    page_used: Vec<bool>,
    /// Absolute `[first index, count]` of every slot's parts.
    parts: Vec<[u32; 2]>,
    stats: PoolStats,
    rebased: Vec<u32>,
}

impl ChunkPool {
    pub fn new(config: ChunkPoolConfig) -> ChunkPool {
        let n = config.slots as usize;
        let largest = MAX_CHUNK_VERTICES.min(config.pages.saturating_mul(config.page_vertices));
        let most_indices = (largest.div_ceil(config.page_vertices.max(1)) as usize)
            .saturating_mul(config.page_indices as usize);
        ChunkPool {
            config,
            slots: vec![Slot::default(); n],
            free: (0..config.slots).rev().collect(),
            page_used: vec![false; config.pages as usize],
            parts: vec![[0; 2]; n * MAX_CHUNK_PARTS],
            stats: PoolStats {
                slots: config.slots,
                pages: config.pages,
                bytes: config.vertex_bytes() + config.index_bytes(),
                ..PoolStats::default()
            },
            rebased: Vec::with_capacity(most_indices),
        }
    }

    pub fn config(&self) -> ChunkPoolConfig {
        self.config
    }

    pub fn stats(&self) -> PoolStats {
        self.stats
    }

    fn check(&self, slot: SlotId) -> Result<usize, PoolError> {
        let i = slot.0 as usize;
        match self.slots.get(i) {
            Some(s) if s.live => Ok(i),
            _ => Err(PoolError::BadSlot(slot)),
        }
    }

    /// Takes a free slot (empty: it draws nothing until written).
    pub fn alloc(&mut self) -> Result<SlotId, PoolError> {
        let i = self.free.pop().ok_or(PoolError::Full)?;
        self.slots[i as usize] = Slot {
            live: true,
            ..Slot::default()
        };
        self.stats.used += 1;
        self.stats.peak = self.stats.peak.max(self.stats.used);
        Ok(SlotId(i))
    }

    /// Gives `slot` and its pages back; its parts stop drawing.
    pub fn free(&mut self, slot: SlotId) -> Result<(), PoolError> {
        let i = self.check(slot)?;
        self.release_pages(i);
        self.slots[i] = Slot::default();
        self.free.push(slot.0);
        self.stats.used -= 1;
        Ok(())
    }

    fn release_pages(&mut self, i: usize) {
        let s = &mut self.slots[i];
        let first = s.first_page as usize;
        self.page_used[first..first + s.page_count as usize].fill(false);
        self.stats.pages_used -= s.page_count;
        self.stats.bytes_used -= s.bytes;
        s.page_count = 0;
        s.part_count = 0;
        s.bytes = 0;
    }

    /// First run of `n` free pages, not counting `own` (the pages the slot being
    /// rewritten holds now, which it gives up only once the new run is found).
    fn find_run(&self, n: u32, own: core::ops::Range<usize>) -> Option<u32> {
        let mut run = 0;
        for (p, &used) in self.page_used.iter().enumerate() {
            if used && !own.contains(&p) {
                run = 0;
                continue;
            }
            run += 1;
            if run == n {
                return Some((p + 1) as u32 - n);
            }
        }
        None
    }

    /// Checks that `mesh` with draws `parts` (`[first index, count]` into its indices)
    /// fits, gives `slot` the pages for it (in place of any it had) and lays the write out.
    /// A refused write leaves the slot as it was.
    pub fn stage_write(
        &mut self,
        slot: SlotId,
        mesh: &Mesh,
        parts: &[[u32; 2]],
    ) -> Result<ChunkWrite<'_>, PoolError> {
        let i = self.check(slot)?;
        let (nv, ni) = (mesh.vertices.len(), mesh.indices.len());
        let c = self.config;
        let pages = (nv as u64)
            .div_ceil(u64::from(c.page_vertices.max(1)))
            .max((ni as u64).div_ceil(u64::from(c.page_indices.max(1))))
            .max(1);
        if nv > MAX_CHUNK_VERTICES as usize || pages > u64::from(c.pages) {
            return Err(PoolError::TooBig {
                vertices: nv,
                indices: ni,
            });
        }
        if parts.len() > MAX_CHUNK_PARTS {
            return Err(PoolError::TooManyParts(parts.len()));
        }
        let in_mesh = |&[first, count]: &[u32; 2]| {
            u64::from(first) + u64::from(count) <= ni as u64 && count % 3 == 0
        };
        if !parts.iter().all(in_mesh) || mesh.indices.iter().any(|&k| k as usize >= nv) {
            return Err(PoolError::BadRange);
        }
        let pages = pages as u32;
        let s = self.slots[i];
        let own = s.first_page as usize..(s.first_page + s.page_count) as usize;
        let first_page = self
            .find_run(pages, own)
            .ok_or(PoolError::OutOfSpace { pages })?;
        self.release_pages(i);
        self.page_used[first_page as usize..(first_page + pages) as usize].fill(true);
        let first_vertex = first_page * c.page_vertices;
        let first_index = first_page * c.page_indices;
        self.rebased.clear();
        self.rebased
            .extend(mesh.indices.iter().map(|&k| k + first_vertex));
        let table = &mut self.parts[i * MAX_CHUNK_PARTS..][..parts.len()];
        for (t, &[first, count]) in table.iter_mut().zip(parts) {
            *t = [first_index + first, count];
        }
        let bytes = (nv * size_of::<Vertex>() + ni * 4) as u64;
        self.slots[i] = Slot {
            live: true,
            first_page,
            page_count: pages,
            part_count: parts.len() as u8,
            bytes,
        };
        self.stats.pages_used += pages;
        self.stats.pages_peak = self.stats.pages_peak.max(self.stats.pages_used);
        self.stats.bytes_used += bytes;
        self.stats.writes += 1;
        Ok(ChunkWrite {
            vertex_offset: u64::from(first_vertex) * size_of::<Vertex>() as u64,
            index_offset: u64::from(first_index) * 4,
            indices: &self.rebased,
        })
    }

    /// The absolute `[first index, count]` that `id` draws, or `None` when it is not a
    /// written part of a live slot.
    pub fn draw_range(&self, id: MeshId) -> Option<[u32; 2]> {
        let (slot, part) = id.as_chunk_part()?;
        let i = self.check(slot).ok()?;
        (part < usize::from(self.slots[i].part_count))
            .then(|| self.parts[i * MAX_CHUNK_PARTS + part])
    }
}

/// Somewhere streamed chunks are written: the [`crate::Renderer`], or a bare
/// [`ChunkPool`] in tests and headless runs.
pub trait ChunkStore {
    fn alloc_chunk_slot(&mut self) -> Result<SlotId, PoolError>;
    /// Writes `mesh` into `slot` with its draws `parts` (`[first index, count]`).
    fn write_chunk(
        &mut self,
        slot: SlotId,
        mesh: &Mesh,
        parts: &[[u32; 2]],
    ) -> Result<(), PoolError>;
    fn free_chunk_slot(&mut self, slot: SlotId) -> Result<(), PoolError>;
}

impl ChunkStore for ChunkPool {
    fn alloc_chunk_slot(&mut self) -> Result<SlotId, PoolError> {
        self.alloc()
    }

    fn write_chunk(
        &mut self,
        slot: SlotId,
        mesh: &Mesh,
        parts: &[[u32; 2]],
    ) -> Result<(), PoolError> {
        self.stage_write(slot, mesh, parts).map(|_| ())
    }

    fn free_chunk_slot(&mut self, slot: SlotId) -> Result<(), PoolError> {
        self.free(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three slots over four pages of 6 vertices and 6 indices.
    const CONFIG: ChunkPoolConfig = ChunkPoolConfig {
        slots: 3,
        pages: 4,
        page_vertices: 6,
        page_indices: 6,
    };

    /// `n` separate triangles.
    fn tris(n: u32) -> Mesh {
        let mut m = Mesh::default();
        for _ in 0..n {
            let b = m.vertices.len() as u32;
            for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
                m.vertices.push(Vertex::new(p, [0.0, 1.0, 0.0], [255; 4]));
            }
            m.indices.extend_from_slice(&[b, b + 1, b + 2]);
        }
        m
    }

    #[test]
    fn part_ids_round_trip_and_never_collide_with_meshes() {
        for (slot, part) in [(0, 0), (1, 31), (4000, 7)] {
            let id = MeshId::chunk_part(SlotId(slot), part);
            assert_eq!(id.as_chunk_part(), Some((SlotId(slot), part)));
        }
        assert_eq!(MeshId(5).as_chunk_part(), None);
        assert_ne!(MeshId::chunk_part(SlotId(0), 0), MeshId(0));
    }

    #[test]
    fn slots_and_pages_cycle_through_alloc_write_and_free() {
        let mut pool = ChunkPool::new(CONFIG);
        let caps = (
            pool.free.capacity(),
            pool.rebased.capacity(),
            pool.parts.capacity(),
        );
        for round in 0..50 {
            let held: Vec<SlotId> = (0..3).map(|_| pool.alloc().unwrap()).collect();
            assert_eq!(pool.alloc(), Err(PoolError::Full), "round {round}");
            // One page, two pages, then one more: the fourth page is left.
            pool.write_chunk(held[0], &tris(2), &[[0, 6]]).unwrap();
            pool.write_chunk(held[1], &tris(4), &[[0, 12]]).unwrap();
            pool.write_chunk(held[2], &tris(2), &[[0, 6]]).unwrap();
            assert_eq!(pool.stats().pages_used, 4);
            // Rewriting a slot gives its old pages up for the new run.
            pool.write_chunk(held[0], &tris(1), &[[0, 3]]).unwrap();
            assert_eq!(pool.stats().pages_used, 4);
            assert_eq!(
                pool.write_chunk(held[0], &tris(4), &[]),
                Err(PoolError::OutOfSpace { pages: 2 })
            );
            assert_eq!(
                pool.draw_range(MeshId::chunk_part(held[0], 0)),
                Some([0, 3])
            );
            for &s in held.iter().rev() {
                pool.free(s).unwrap();
            }
            assert_eq!(pool.free(held[0]), Err(PoolError::BadSlot(held[0])));
            assert_eq!((pool.stats().used, pool.stats().pages_used), (0, 0));
        }
        let stats = pool.stats();
        assert_eq!((stats.peak, stats.pages_peak, stats.writes), (3, 4, 200));
        assert_eq!(stats.bytes, 4 * 6 * 36 + 4 * 6 * 4);
        assert_eq!(
            (
                pool.free.capacity(),
                pool.rebased.capacity(),
                pool.parts.capacity()
            ),
            caps
        );
    }

    #[test]
    fn writes_are_bounded_and_rebased_into_their_pages() {
        let mut pool = ChunkPool::new(CONFIG);
        let a = pool.alloc().unwrap();
        let b = pool.alloc().unwrap();
        pool.write_chunk(a, &tris(1), &[[0, 3]]).unwrap();
        // Four triangles take two pages, after a's one: draws are the first triangle,
        // then the other three.
        let m = tris(4);
        let w = pool.stage_write(b, &m, &[[0, 3], [3, 9]]).unwrap();
        assert_eq!(w.vertex_offset, 6 * size_of::<Vertex>() as u64);
        assert_eq!(w.index_offset, 6 * 4);
        assert_eq!(w.indices[..3], [6, 7, 8]);
        assert_eq!(pool.draw_range(MeshId::chunk_part(b, 0)), Some([6, 3]));
        assert_eq!(pool.draw_range(MeshId::chunk_part(b, 1)), Some([9, 9]));
        assert_eq!(pool.draw_range(MeshId::chunk_part(b, 2)), None);
        assert_eq!(pool.stats().bytes_used, (15 * 36 + 15 * 4) as u64);
        // Larger than the whole pool, too many parts, runs outside the mesh, an index past
        // the vertices, a slot not handed out: refused, and the slot keeps what it had.
        assert_eq!(
            pool.stage_write(b, &tris(9), &[]).unwrap_err(),
            PoolError::TooBig {
                vertices: 27,
                indices: 27
            }
        );
        let many = [[0, 3]; MAX_CHUNK_PARTS + 1];
        assert_eq!(
            pool.stage_write(b, &m, &many).unwrap_err(),
            PoolError::TooManyParts(MAX_CHUNK_PARTS + 1)
        );
        for bad in [[9, 6], [0, 13], [1, 2]] {
            assert_eq!(
                pool.stage_write(b, &m, &[bad]).unwrap_err(),
                PoolError::BadRange
            );
        }
        let mut dangling = tris(1);
        dangling.indices[2] = 3;
        assert_eq!(
            pool.stage_write(b, &dangling, &[[0, 3]]).unwrap_err(),
            PoolError::BadRange
        );
        assert!(matches!(
            pool.stage_write(SlotId(2), &m, &[]),
            Err(PoolError::BadSlot(_))
        ));
        assert_eq!(pool.draw_range(MeshId::chunk_part(b, 1)), Some([9, 9]));
        // Freed, its parts stop drawing; the next chunk in the slot starts empty.
        pool.free(b).unwrap();
        assert_eq!(pool.draw_range(MeshId::chunk_part(b, 0)), None);
        assert_eq!(pool.stats().bytes_used, (3 * 36 + 3 * 4) as u64);
        assert_eq!(pool.alloc().unwrap(), b);
        assert_eq!(pool.draw_range(MeshId::chunk_part(b, 0)), None);
        // Over the per-chunk vertex budget, however large the pool.
        let mut big = ChunkPool::new(ChunkPoolConfig::for_vertices(1, 2 * MAX_CHUNK_VERTICES));
        let s = big.alloc().unwrap();
        let huge = Mesh {
            vertices: vec![
                Vertex::new([0.0; 3], [0.0; 3], [0; 4]);
                MAX_CHUNK_VERTICES as usize + 1
            ],
            indices: Vec::new(),
        };
        assert!(matches!(
            big.stage_write(s, &huge, &[]),
            Err(PoolError::TooBig { .. })
        ));
    }
}
