//! Fixed-capacity containers. Nothing here grows after construction: running out of room
//! returns `None` or `Err`, never panics and never reallocates, so hot paths stay within
//! the memory budgets in `docs/ARCHITECTURE.md`.
//!
//! - [`Pool`]: slots with a free list and generational [`Handle`]s.
//! - [`Arena`]: a bump allocator over one buffer, reset as a whole.
//! - [`FixedVec`]: an inline array with a length.
//!
//! `Pool` and `FixedVec` store their items inline, so a large `N` makes a large value; put
//! big ones in a `Box` or a long-lived owner rather than on the stack.

use alloc::vec::Vec;
use core::cell::Cell;
use core::ptr::NonNull;

/// End-of-list marker for the pool free list.
const NIL: u32 = u32::MAX;

/// Names one item in a [`Pool`]. A handle goes stale when its item is removed: the slot's
/// generation is bumped, so a later item in the same slot gets a different handle.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    pub index: u32,
    pub gen: u32,
}

#[derive(Clone, Debug)]
struct Slot<T> {
    /// Bumped (wrapping) every time the slot is vacated.
    gen: u32,
    /// Next free slot when this one is free, `NIL` otherwise or at the end of the list.
    next_free: u32,
    value: Option<T>,
}

/// Up to `N` items addressed by generational [`Handle`]s. Insert, remove and lookup are O(1);
/// freed slots are reused most-recently-freed first.
///
/// Generations are 32-bit and wrap: a handle kept across 2^32 reuses of one slot would match
/// again. That is far beyond any lifetime in the game.
#[derive(Clone, Debug)]
pub struct Pool<T, const N: usize> {
    slots: [Slot<T>; N],
    free_head: u32,
    len: u32,
}

impl<T, const N: usize> Pool<T, N> {
    /// Indices must fit in a `u32` with `NIL` left over.
    const CAPACITY_FITS_U32: () = assert!(N < NIL as usize, "Pool capacity must be < u32::MAX");

    pub fn new() -> Self {
        let () = Self::CAPACITY_FITS_U32;
        let slots = core::array::from_fn(|i| Slot {
            gen: 0,
            next_free: if i + 1 < N { (i + 1) as u32 } else { NIL },
            value: None,
        });
        Pool {
            slots,
            free_head: if N > 0 { 0 } else { NIL },
            len: 0,
        }
    }

    /// Stores `value` and returns its handle, or `None` (dropping `value`) when full.
    pub fn insert(&mut self, value: T) -> Option<Handle> {
        if self.free_head == NIL {
            return None;
        }
        let index = self.free_head;
        let slot = &mut self.slots[index as usize];
        self.free_head = slot.next_free;
        slot.next_free = NIL;
        slot.value = Some(value);
        self.len += 1;
        Some(Handle {
            index,
            gen: slot.gen,
        })
    }

    /// Takes the item out. `None` if the handle is stale or out of range.
    pub fn remove(&mut self, handle: Handle) -> Option<T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.gen != handle.gen {
            return None;
        }
        let value = slot.value.take()?;
        slot.gen = slot.gen.wrapping_add(1);
        slot.next_free = self.free_head;
        self.free_head = handle.index;
        self.len -= 1;
        Some(value)
    }

    /// The item, or `None` if the handle is stale or out of range.
    pub fn get(&self, handle: Handle) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        if slot.gen != handle.gen {
            return None;
        }
        slot.value.as_ref()
    }

    /// The item, or `None` if the handle is stale or out of range.
    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.gen != handle.gen {
            return None;
        }
        slot.value.as_mut()
    }

    /// Number of live items.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    /// Live items with their handles, in slot order.
    pub fn iter(&self) -> PoolIter<'_, T> {
        PoolIter {
            slots: &self.slots,
            index: 0,
        }
    }

    /// Drops every item and invalidates every outstanding handle.
    pub fn clear(&mut self) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.value.take().is_some() {
                slot.gen = slot.gen.wrapping_add(1);
            }
            slot.next_free = if i + 1 < N { (i + 1) as u32 } else { NIL };
        }
        self.free_head = if N > 0 { 0 } else { NIL };
        self.len = 0;
    }
}

impl<T, const N: usize> Default for Pool<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterator returned by [`Pool::iter`].
pub struct PoolIter<'a, T> {
    slots: &'a [Slot<T>],
    index: usize,
}

impl<'a, T> Iterator for PoolIter<'a, T> {
    type Item = (Handle, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.slots.len() {
            let i = self.index;
            self.index += 1;
            let slot = &self.slots[i];
            if let Some(value) = &slot.value {
                let handle = Handle {
                    index: i as u32,
                    gen: slot.gen,
                };
                return Some((handle, value));
            }
        }
        None
    }
}

/// Largest alignment [`Arena`] serves. The buffer starts on this boundary, so every offset
/// (and therefore when the arena runs out) is the same on every platform and allocator.
pub const ARENA_MAX_ALIGN: usize = 16;

/// The arena's unit of storage; only here to give the buffer its alignment.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Block {
    _bytes: [u8; ARENA_MAX_ALIGN],
}

/// A bump allocator over one buffer allocated at construction. Allocations are handed out
/// from `&self` so several can be alive at once; [`Arena::reset`] takes `&mut self`, which
/// the borrow checker only allows once every allocation is gone.
///
/// Not `Send` or `Sync`; the sim is single-threaded in v1.
pub struct Arena {
    /// Owns the memory. Never touched after construction except by `Drop`, so `base` stays
    /// valid (`Vec::as_mut_ptr` promises not to create a reference to the elements).
    _buf: Vec<Block>,
    base: NonNull<u8>,
    capacity: usize,
    used: Cell<usize>,
}

impl Arena {
    /// An arena of `capacity` bytes. This is its only heap allocation.
    pub fn new(capacity: usize) -> Arena {
        let blocks = capacity.div_ceil(ARENA_MAX_ALIGN);
        let mut buf = alloc::vec![
            Block {
                _bytes: [0; ARENA_MAX_ALIGN]
            };
            blocks
        ];
        // A `Vec`'s pointer is never null and is aligned for `Block`, even when empty.
        let base = NonNull::new(buf.as_mut_ptr())
            .unwrap_or(NonNull::dangling())
            .cast::<u8>();
        Arena {
            _buf: buf,
            base,
            capacity,
            used: Cell::new(0),
        }
    }

    /// `n` values of `T`, each set to `T::default()`, aligned for `T`. `None` when the arena
    /// does not have room (including alignment padding) or `T` needs more than
    /// [`ARENA_MAX_ALIGN`]; the arena is then unchanged. Zero-sized requests take no space
    /// and always succeed.
    // Handing out `&mut` from `&self` is the point of a bump allocator: each call returns a
    // disjoint byte range, so the slices never alias. See the SAFETY comment below.
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_slice<T: Copy + Default>(&self, n: usize) -> Option<&mut [T]> {
        let size = core::mem::size_of::<T>();
        let align = core::mem::align_of::<T>();
        let bytes = size.checked_mul(n)?;
        if bytes == 0 {
            // SAFETY: a dangling, aligned, non-null pointer is valid for a slice that
            // occupies zero bytes (either `n == 0` or `T` is zero-sized).
            return Some(unsafe {
                core::slice::from_raw_parts_mut(NonNull::dangling().as_ptr(), n)
            });
        }
        if align > ARENA_MAX_ALIGN {
            return None;
        }
        let start = self.used.get().checked_next_multiple_of(align)?;
        let end = start.checked_add(bytes)?;
        if end > self.capacity {
            return None;
        }
        self.used.set(end);
        // SAFETY:
        // - `base` points to the start of `_buf`, which holds at least `capacity` bytes, lives
        //   as long as `self` and is only accessed through `base`.
        // - `start..end` lies inside the buffer (`end <= capacity`), and `base + start` is
        //   aligned for `T`: `base` is `ARENA_MAX_ALIGN`-aligned, `align` divides it, and
        //   `start` is a multiple of `align`.
        // - The range is disjoint from every earlier allocation since `used` only grows,
        //   and `used` returns to 0 only in `reset(&mut self)`, when no slice can be alive.
        // - Every element is written with `T::default()` before the slice is formed, so the
        //   slice is fully initialised whatever bit patterns `T` forbids. `T: Copy` means
        //   nothing needs dropping when the arena is reset or dropped.
        unsafe {
            let ptr = self.base.as_ptr().add(start).cast::<T>();
            for i in 0..n {
                ptr.add(i).write(T::default());
            }
            Some(core::slice::from_raw_parts_mut(ptr, n))
        }
    }

    /// Frees every allocation at once.
    pub fn reset(&mut self) {
        self.used.set(0);
    }

    /// Bytes handed out so far, including alignment padding.
    pub fn used(&self) -> usize {
        self.used.get()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl core::fmt::Debug for Arena {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Arena")
            .field("used", &self.used.get())
            .field("capacity", &self.capacity)
            .finish()
    }
}

/// Up to `N` plain-old-data values in an inline array. Slots past `len` hold `T::default()`
/// or stale values and are never exposed.
#[derive(Clone, Copy, Debug)]
pub struct FixedVec<T: Copy + Default, const N: usize> {
    items: [T; N],
    len: usize,
}

impl<T: Copy + Default, const N: usize> FixedVec<T, N> {
    pub fn new() -> Self {
        FixedVec {
            items: [T::default(); N],
            len: 0,
        }
    }

    /// Appends `value`, or gives it back when full.
    pub fn push(&mut self, value: T) -> Result<(), T> {
        if self.len == N {
            return Err(value);
        }
        self.items[self.len] = value;
        self.len += 1;
        Ok(())
    }

    /// Removes and returns the last value.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.items[self.len])
    }

    pub fn as_slice(&self) -> &[T] {
        &self.items[..self.len]
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items[..self.len]
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }
}

impl<T: Copy + Default, const N: usize> Default for FixedVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_insert_get_remove() {
        let mut pool: Pool<u32, 4> = Pool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 4);
        let a = pool.insert(10).unwrap();
        let b = pool.insert(20).unwrap();
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.get(a), Some(&10));
        *pool.get_mut(b).unwrap() += 1;
        assert_eq!(pool.get(b), Some(&21));
        assert_eq!(pool.remove(a), Some(10));
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.get(b), Some(&21));
    }

    #[test]
    fn pool_exhaustion_returns_none() {
        let mut pool: Pool<u8, 3> = Pool::new();
        for i in 0..3 {
            assert!(pool.insert(i).is_some());
        }
        assert_eq!(pool.insert(99), None);
        assert_eq!(pool.insert(99), None);
        assert_eq!(pool.len(), 3);

        let mut empty: Pool<u8, 0> = Pool::new();
        assert_eq!(empty.insert(1), None);
        assert_eq!(empty.iter().count(), 0);
        empty.clear();
    }

    #[test]
    fn pool_stale_handle_returns_none() {
        let mut pool: Pool<u32, 2> = Pool::new();
        let h = pool.insert(7).unwrap();
        assert_eq!(pool.remove(h), Some(7));
        assert_eq!(pool.get(h), None);
        assert_eq!(pool.get_mut(h), None);
        assert_eq!(pool.remove(h), None);
        assert_eq!(pool.len(), 0);
        // Out of range and forged handles are rejected too.
        let bogus = Handle { index: 5, gen: 0 };
        assert_eq!(pool.get(bogus), None);
        assert_eq!(pool.remove(bogus), None);
        assert_eq!(pool.get(Handle { index: 0, gen: 9 }), None);
    }

    #[test]
    fn pool_reuse_bumps_generation() {
        let mut pool: Pool<u32, 1> = Pool::new();
        let first = pool.insert(1).unwrap();
        pool.remove(first).unwrap();
        let second = pool.insert(2).unwrap();
        assert_eq!(second.index, first.index);
        assert_eq!(second.gen, first.gen.wrapping_add(1));
        assert_eq!(pool.get(first), None);
        assert_eq!(pool.get(second), Some(&2));
    }

    #[test]
    fn pool_free_list_reuses_every_slot() {
        let mut pool: Pool<usize, 8> = Pool::new();
        let mut handles = [Handle { index: 0, gen: 0 }; 8];
        for round in 0..5 {
            for (i, h) in handles.iter_mut().enumerate() {
                *h = pool.insert(round * 8 + i).unwrap();
            }
            assert_eq!(pool.insert(0), None);
            // Remove in a scattered order to exercise the free list.
            for &i in &[3, 0, 7, 5, 1, 6, 2, 4] {
                assert_eq!(pool.remove(handles[i]), Some(round * 8 + i));
            }
            assert!(pool.is_empty());
        }
        for h in &handles {
            assert_eq!(h.gen, 4);
        }
    }

    #[test]
    fn pool_iter_and_clear() {
        let mut pool: Pool<u32, 4> = Pool::new();
        let a = pool.insert(1).unwrap();
        let b = pool.insert(2).unwrap();
        let c = pool.insert(3).unwrap();
        pool.remove(b);
        let mut seen = [(Handle { index: 0, gen: 0 }, 0u32); 4];
        let mut n = 0;
        for (h, &v) in pool.iter() {
            seen[n] = (h, v);
            n += 1;
        }
        assert_eq!(&seen[..n], &[(a, 1), (c, 3)]);

        pool.clear();
        assert!(pool.is_empty());
        assert_eq!(pool.iter().count(), 0);
        assert_eq!(pool.get(a), None);
        assert_eq!(pool.get(c), None);
        // Full capacity is available again and old handles stay stale.
        for i in 0..4 {
            let h = pool.insert(i).unwrap();
            assert_ne!(h, a);
            assert_ne!(h, c);
        }
        assert_eq!(pool.insert(9), None);
    }

    #[test]
    fn pool_drops_values() {
        use alloc::rc::Rc;
        let tracker = Rc::new(());
        let mut pool: Pool<Rc<()>, 4> = Pool::new();
        let h = pool.insert(tracker.clone()).unwrap();
        pool.insert(tracker.clone()).unwrap();
        // A rejected insert drops its value.
        let mut full: Pool<Rc<()>, 0> = Pool::new();
        assert_eq!(full.insert(tracker.clone()), None);
        assert_eq!(Rc::strong_count(&tracker), 3);
        drop(pool.remove(h));
        assert_eq!(Rc::strong_count(&tracker), 2);
        pool.clear();
        assert_eq!(Rc::strong_count(&tracker), 1);
    }

    fn is_aligned<T>(slice: &[T]) -> bool {
        slice.as_ptr().is_aligned()
    }

    #[test]
    fn arena_alignment() {
        let arena = Arena::new(256);
        // An odd-sized byte allocation first forces padding before each wider type.
        let bytes = arena.alloc_slice::<u8>(3).unwrap();
        let words = arena.alloc_slice::<u32>(5).unwrap();
        let doubles = arena.alloc_slice::<f64>(2).unwrap();
        let more_bytes = arena.alloc_slice::<u8>(1).unwrap();
        let vecs = arena.alloc_slice::<[f32; 4]>(3).unwrap();
        assert!(is_aligned(bytes) && bytes.len() == 3);
        assert!(is_aligned(words) && words.len() == 5);
        assert!(is_aligned(doubles) && doubles.len() == 2);
        assert!(is_aligned(more_bytes) && more_bytes.len() == 1);
        assert!(is_aligned(vecs) && vecs.len() == 3);
        // Values start at default and the slices are independent.
        assert!(words.iter().all(|&w| w == 0));
        assert!(vecs.iter().all(|v| *v == [0.0; 4]));
        bytes.fill(0xAB);
        words.fill(u32::MAX);
        doubles.fill(1.5);
        more_bytes[0] = 7;
        vecs[2] = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(bytes, &[0xAB; 3]);
        assert_eq!(words, &[u32::MAX; 5]);
        assert_eq!(doubles, &[1.5, 1.5]);
        assert_eq!(more_bytes, &[7]);
        assert_eq!(vecs[2], [1.0, 2.0, 3.0, 4.0]);
        // Offsets are relative to an aligned base, so padding is exact everywhere:
        // u8 0..3, u32 4..24, f64 24..40, u8 40..41, [f32; 4] 44..92.
        assert_eq!(arena.used(), 92);
    }

    #[test]
    fn arena_rejects_over_aligned_types() {
        #[repr(align(32))]
        #[derive(Clone, Copy, Default)]
        struct Wide {
            _pad: u8,
        }
        let arena = Arena::new(256);
        assert!(arena.alloc_slice::<Wide>(1).is_none());
        assert_eq!(arena.used(), 0);
        #[repr(align(16))]
        #[derive(Clone, Copy, Default)]
        struct Max {
            _pad: u8,
        }
        arena.alloc_slice::<u8>(1).unwrap();
        let s = arena.alloc_slice::<Max>(2).unwrap();
        assert!(is_aligned(s));
        assert_eq!(arena.used(), 48);
    }

    #[test]
    fn arena_capacity_is_exact_in_bytes() {
        // Storage rounds up to whole blocks internally, but only `capacity` bytes are usable.
        let arena = Arena::new(5);
        assert_eq!(arena.capacity(), 5);
        assert!(arena.alloc_slice::<u8>(6).is_none());
        assert!(arena.alloc_slice::<u8>(5).is_some());
        assert!(arena.alloc_slice::<u8>(1).is_none());
    }

    #[test]
    fn arena_exhaustion_returns_none() {
        let arena = Arena::new(16);
        assert_eq!(arena.capacity(), 16);
        assert!(arena.alloc_slice::<u32>(4).is_some());
        assert_eq!(arena.used(), 16);
        assert!(arena.alloc_slice::<u8>(1).is_none());
        assert!(arena.alloc_slice::<f64>(1).is_none());
        assert_eq!(arena.used(), 16);

        let arena = Arena::new(16);
        assert!(arena.alloc_slice::<u8>(17).is_none());
        assert!(arena.alloc_slice::<u64>(usize::MAX).is_none());
        assert!(arena.alloc_slice::<u64>(usize::MAX / 4).is_none());
        assert_eq!(arena.used(), 0);

        let empty = Arena::new(0);
        assert!(empty.alloc_slice::<u8>(1).is_none());
        assert_eq!(empty.alloc_slice::<u32>(0).map(|s| s.len()), Some(0));
    }

    #[test]
    fn arena_zero_sized_requests_take_no_space() {
        let arena = Arena::new(8);
        assert_eq!(arena.alloc_slice::<f64>(0).map(|s| s.len()), Some(0));
        assert_eq!(arena.alloc_slice::<()>(1000).map(|s| s.len()), Some(1000));
        assert_eq!(arena.used(), 0);
    }

    #[test]
    fn arena_reset_reuses_space() {
        let mut arena = Arena::new(64);
        let first = arena.alloc_slice::<u32>(16).unwrap();
        first.fill(0xDEAD_BEEF);
        let first_ptr = first.as_ptr();
        assert!(arena.alloc_slice::<u32>(1).is_none());
        arena.reset();
        assert_eq!(arena.used(), 0);
        let second = arena.alloc_slice::<u32>(16).unwrap();
        assert_eq!(second.as_ptr(), first_ptr);
        // Reused memory is re-initialised, not left with the old contents.
        assert!(second.iter().all(|&w| w == 0));
        assert_eq!(arena.capacity(), 64);
    }

    #[test]
    fn arena_survives_move() {
        let arena = Arena::new(32);
        let moved = arena;
        let s = moved.alloc_slice::<u16>(4).unwrap();
        s.copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(s, &[1, 2, 3, 4]);
    }

    #[test]
    fn fixed_vec_push_until_full() {
        let mut v: FixedVec<u32, 3> = FixedVec::new();
        assert!(v.is_empty());
        assert_eq!(v.capacity(), 3);
        assert_eq!(v.push(1), Ok(()));
        assert_eq!(v.push(2), Ok(()));
        assert_eq!(v.push(3), Ok(()));
        assert_eq!(v.push(4), Err(4));
        assert_eq!(v.len(), 3);
        assert_eq!(v.as_slice(), &[1, 2, 3]);
        v.as_mut_slice()[0] = 9;
        assert_eq!(v.pop(), Some(3));
        assert_eq!(v.as_slice(), &[9, 2]);
        v.clear();
        assert!(v.is_empty());
        assert_eq!(v.pop(), None);
        assert_eq!(v.as_slice(), &[] as &[u32]);

        let mut zero: FixedVec<u8, 0> = FixedVec::new();
        assert_eq!(zero.push(1), Err(1));
    }
}
