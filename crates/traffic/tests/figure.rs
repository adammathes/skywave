//! Posing a pedestrian figure allocates nothing: the gait, the step aside, the hop, blending
//! and turning joints into part transforms all run per figure per frame.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use traffic::figure::{figure, look_yaw, Era, Joints, Xform, PARTS};

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

#[test]
fn posing_allocates_nothing() {
    let figures = [figure(1, Era::Seventies), figure(2, Era::Eighties)];
    let mut out = [Xform::IDENTITY; PARTS];
    let mut checksum = 0.0f32;
    let before = allocations();
    for f in &figures {
        let sk = &f.skeleton;
        for i in 0..240 {
            let t = i as f32 / 240.0;
            let speed = 1.6 * t;
            let walk = sk.walk(t * 3.0, speed);
            let aside = sk.step_aside(t, 1.0, look_yaw(2.0, -1.0));
            let hop = sk.hop(t, -1.0);
            let j = Joints::blend(&Joints::blend(&walk, &aside, t), &hop, 0.5);
            sk.pose(&j, &mut out);
            checksum += out[PARTS - 1].pos[1] + sk.phase_rate(speed);
        }
    }
    assert_eq!(allocations(), before, "posing allocated");
    assert!(checksum.is_finite());
}
