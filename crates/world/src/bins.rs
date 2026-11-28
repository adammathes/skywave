//! A uniform grid of item lists, for "what is near this point" over a whole place without
//! scanning everything: roads, parcels and footprints by their bounds.

use alloc::vec::Vec;

use crate::math::{floor, V2};

/// Items binned by their bounds on a grid of square cells.
pub(crate) struct Bins {
    origin: V2,
    cell: f32,
    w: usize,
    h: usize,
    cells: Vec<Vec<u32>>,
}

impl Bins {
    /// An empty grid of `cell`-meter cells over `extent` from `origin`.
    pub(crate) fn new(origin: V2, extent: V2, cell: f32) -> Bins {
        let w = (extent.x / cell) as usize + 1;
        let h = (extent.y / cell) as usize + 1;
        Bins {
            origin,
            cell,
            w,
            h,
            cells: (0..w * h).map(|_| Vec::new()).collect(),
        }
    }

    pub(crate) fn range(&self, lo: V2, hi: V2) -> Option<[usize; 4]> {
        let f = |v: f32, o: f32| floor((v - o) / self.cell);
        let (x0, x1) = (f(lo.x, self.origin.x), f(hi.x, self.origin.x));
        let (z0, z1) = (f(lo.y, self.origin.y), f(hi.y, self.origin.y));
        if x1 < 0.0 || z1 < 0.0 || x0 >= self.w as f32 || z0 >= self.h as f32 {
            return None;
        }
        let c = |v: f32, n: usize| (v.max(0.0) as usize).min(n - 1);
        Some([c(x0, self.w), c(x1, self.w), c(z0, self.h), c(z1, self.h)])
    }

    /// Lists `item` in every cell the box `lo..hi` reaches (the part of it on the grid).
    pub(crate) fn insert(&mut self, lo: V2, hi: V2, item: u32) {
        if let Some([x0, x1, z0, z1]) = self.range(lo, hi) {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    self.cells[z * self.w + x].push(item);
                }
            }
        }
    }

    /// The items whose boxes may hold `p`: those of its cell, in the order they were
    /// inserted. Every item whose box holds `p` is among them.
    pub(crate) fn at(&self, p: V2) -> &[u32] {
        match self.range(p, p) {
            Some([x, _, z, _]) => &self.cells[z * self.w + x],
            None => &[],
        }
    }
}

/// The same grid for lookups that must miss nothing wherever the point is: boxes and
/// points off the grid count as in its nearest cells.
impl Bins {
    /// The grid over `place` with `cell`-meter cells.
    pub(crate) fn over(place: &place::Place, cell: f32) -> Bins {
        let ext = place.header.extent_m();
        let origin = V2::from_array(place.header.origin_m());
        Bins::new(origin, V2::new(ext[0], ext[1]), cell)
    }

    fn cell_of(&self, p: V2) -> [usize; 2] {
        let c =
            |v: f32, o: f32, n: usize| (floor((v - o) / self.cell).max(0.0) as usize).min(n - 1);
        [c(p.x, self.origin.x, self.w), c(p.y, self.origin.y, self.h)]
    }

    /// Lists `item` in every cell the box `lo..hi` reaches.
    pub(crate) fn insert_box(&mut self, lo: V2, hi: V2, item: u32) {
        let (a, b) = (self.cell_of(lo), self.cell_of(hi));
        for z in a[1]..=b[1] {
            for x in a[0]..=b[0] {
                self.cells[z * self.w + x].push(item);
            }
        }
    }

    /// [`Bins::insert_box`] of the segment `a b` grown by `r` all round.
    pub(crate) fn insert_seg(&mut self, a: V2, b: V2, r: f32, item: u32) {
        let lo = V2::new(a.x.min(b.x) - r, a.y.min(b.y) - r);
        let hi = V2::new(a.x.max(b.x) + r, a.y.max(b.y) + r);
        self.insert_box(lo, hi, item);
    }

    /// Every item whose inserted box holds `p` (and others of its cell), in the order they
    /// were inserted.
    pub(crate) fn holding(&self, p: V2) -> &[u32] {
        let [x, z] = self.cell_of(p);
        &self.cells[z * self.w + x]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wherever a point is, on the grid or off it, every box that holds it is listed for
    /// it, in the order the boxes went in.
    #[test]
    fn holding_misses_no_box() {
        let mut rng = skywave_core::Rng::new(skywave_core::Seed(7), &[]);
        let mut at = |lo: f32, hi: f32| lo + (hi - lo) * rng.next_f32();
        let mut bins = Bins::new(V2::new(-40.0, 10.0), V2::new(300.0, 200.0), 32.0);
        let boxes: Vec<(V2, V2)> = (0..200)
            .map(|_| {
                let lo = V2::new(at(-150.0, 350.0), at(-100.0, 300.0));
                (lo, lo + V2::new(at(0.0, 90.0), at(0.0, 90.0)))
            })
            .collect();
        for (j, &(lo, hi)) in boxes.iter().enumerate() {
            bins.insert_box(lo, hi, j as u32);
        }
        for _ in 0..2000 {
            let p = V2::new(at(-200.0, 400.0), at(-150.0, 350.0));
            let listed = bins.holding(p);
            assert!(listed.windows(2).all(|w| w[0] < w[1]));
            for (j, &(lo, hi)) in boxes.iter().enumerate() {
                let holds = p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y;
                assert!(!holds || listed.contains(&(j as u32)), "{p:?} in box {j}");
            }
        }
    }
}
