//! The ring between a stream thread and the audio callback: interleaved stereo `f32`
//! samples at 48 kHz, one writer at a time, one reader.
//!
//! Positions are sample counts since the ring was made (`u64`, never wrapping in practice);
//! a slot is `position & mask`. Samples are stored as their bits in `AtomicU32`s, so the
//! ring needs no `unsafe`: the writer stores samples, then publishes the new write position
//! with `Release`; the reader loads it with `Acquire` before reading them.
//!
//! Switching stations must silence the old stream at once and keep it from writing again,
//! even if its thread is still inside a read. [`Ring::cut`] does both under the writer
//! lock: it moves the floor (everything before it is skipped by the reader) to the write
//! position and bumps the epoch; a writer whose epoch is no longer current is refused. The
//! audio callback never takes the lock.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

/// The writer's epoch ended: its station was switched away from or stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gone;

pub struct Ring {
    slots: Box<[AtomicU32]>,
    mask: u64,
    write: AtomicU64,
    read: AtomicU64,
    /// The reader skips everything before this position.
    floor: AtomicU64,
    /// The epoch allowed to write; changed only under `writer`.
    epoch: AtomicU32,
    writer: Mutex<()>,
}

impl Ring {
    /// A ring of `capacity` samples, rounded up to a power of two.
    pub fn new(capacity: usize) -> Ring {
        let n = capacity.next_power_of_two();
        Ring {
            slots: (0..n).map(|_| AtomicU32::new(0)).collect(),
            mask: n as u64 - 1,
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            epoch: AtomicU32::new(0),
            writer: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Ends the current writer's epoch and drops what it wrote; returns the new epoch.
    pub fn cut(&self) -> u32 {
        let _w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        self.floor
            .store(self.write.load(Ordering::Acquire), Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    /// The current epoch.
    pub fn epoch(&self) -> u32 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Writes as many of `samples` as fit for the writer of `epoch`; returns how many.
    pub fn push(&self, epoch: u32, samples: &[f32]) -> Result<usize, Gone> {
        let _w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if self.epoch.load(Ordering::Acquire) != epoch {
            return Err(Gone);
        }
        let w = self.write.load(Ordering::Relaxed);
        let used = w - self.start(w);
        let n = samples.len().min(self.slots.len() - used as usize);
        for (i, &s) in samples[..n].iter().enumerate() {
            self.slots[((w + i as u64) & self.mask) as usize].store(s.to_bits(), Ordering::Relaxed);
        }
        self.write.store(w + n as u64, Ordering::Release);
        Ok(n)
    }

    /// Where the readable samples start, given the write position `w`.
    fn start(&self, w: u64) -> u64 {
        let r = self
            .read
            .load(Ordering::Acquire)
            .max(self.floor.load(Ordering::Acquire));
        r.min(w)
    }

    /// Samples ready to read.
    pub fn available(&self) -> usize {
        let floor = self.floor.load(Ordering::Acquire);
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire).max(floor);
        w.saturating_sub(r) as usize
    }

    /// Reads up to `out.len()` samples into `out`; returns how many. Reader side only.
    pub fn pop(&self, out: &mut [f32]) -> usize {
        let floor = self.floor.load(Ordering::Acquire);
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Relaxed).max(floor).min(w);
        let n = out.len().min((w - r) as usize);
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = f32::from_bits(
                self.slots[((r + i as u64) & self.mask) as usize].load(Ordering::Relaxed),
            );
        }
        self.read.store(r + n as u64, Ordering::Release);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_drains_and_wraps() {
        let ring = Ring::new(6);
        assert_eq!(ring.capacity(), 8);
        let e = ring.epoch();
        assert_eq!(ring.push(e, &[1.0, 2.0, 3.0, 4.0, 5.0]), Ok(5));
        assert_eq!(ring.push(e, &[6.0, 7.0, 8.0, 9.0]), Ok(3), "full at 8");
        assert_eq!(ring.push(e, &[9.0]), Ok(0));
        let mut out = [0.0; 6];
        assert_eq!(ring.pop(&mut out), 6);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        // Wraps round the end.
        assert_eq!(ring.push(e, &[9.0, 10.0, 11.0, 12.0]), Ok(4));
        assert_eq!(ring.available(), 6);
        let mut out = [0.0; 10];
        assert_eq!(ring.pop(&mut out), 6);
        assert_eq!(out[..6], [7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
        assert_eq!(ring.pop(&mut out), 0, "empty");
    }

    #[test]
    fn a_cut_drops_the_old_writer_and_its_samples() {
        let ring = Ring::new(8);
        let old = ring.epoch();
        ring.push(old, &[1.0, 2.0, 3.0]).unwrap();
        let new = ring.cut();
        assert_ne!(new, old);
        assert_eq!(ring.available(), 0, "the old samples are skipped");
        assert_eq!(
            ring.push(old, &[9.0]),
            Err(Gone),
            "the old writer is refused"
        );
        assert_eq!(ring.push(new, &[4.0, 5.0]), Ok(2));
        let mut out = [0.0; 4];
        assert_eq!(ring.pop(&mut out), 2);
        assert_eq!(out[..2], [4.0, 5.0]);
        // The whole ring is free again after a cut, even though nothing read the old part.
        ring.push(new, &[0.5; 3]).unwrap();
        ring.cut();
        assert_eq!(ring.push(ring.epoch(), &[0.0; 9]), Ok(8));
    }

    #[test]
    fn a_reader_and_a_writer_on_two_threads_keep_order() {
        let ring = std::sync::Arc::new(Ring::new(256));
        let w = std::sync::Arc::clone(&ring);
        let total = 100_000u32;
        let writer = std::thread::spawn(move || {
            let e = w.epoch();
            let mut next = 0u32;
            while next < total {
                let chunk: Vec<f32> = (next..(next + 64).min(total)).map(|i| i as f32).collect();
                next += w.push(e, &chunk).unwrap() as u32;
            }
        });
        let mut want = 0u32;
        let mut out = [0.0f32; 48];
        while want < total {
            let n = ring.pop(&mut out);
            for &s in &out[..n] {
                assert_eq!(s, want as f32);
                want += 1;
            }
        }
        writer.join().unwrap();
    }
}
