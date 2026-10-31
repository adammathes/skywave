//! [`Pool`]: a fixed-capacity container of slots with a free list and generational
//! [`Handle`]s. It never grows after construction: running out of room returns `None`, never
//! panics and never reallocates, so hot paths stay within the memory budgets in
//! `docs/ARCHITECTURE.md`.
//!
//! `Pool` stores its items inline, so a large `N` makes a large value; put big ones in a
//! `Box` or a long-lived owner rather than on the stack.

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
}
