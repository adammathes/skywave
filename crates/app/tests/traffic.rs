//! The traffic's per-frame paths allocate nothing: busy traffic in the test town at night
//! (low beams, glowing lamps, voices), the sim stepped and the scene drawn every frame.
//! `cargo test -p app --test traffic`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use app::{App, InputState, SceneSetup};

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
fn stepping_and_drawing_traffic_allocates_nothing() {
    let query = "place=town&t=22:30&lights=1&traffic=busy&stream=0";
    let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
    let mut input = InputState::default();
    input.set(app::Action::Throttle, 0.4);
    // The first frame settles anything lazy; from then on nothing allocates.
    app.update(&input, 1.0 / 60.0);
    app.scene_view(0.5, 16.0 / 9.0);
    assert!(app.traffic_count() > 50, "{}", app.traffic_count());
    let before = allocations();
    let mut drawn = 0;
    for frame in 0..600 {
        let steps = app.update(&input, 1.0 / 60.0);
        let view = app.scene_view(steps.alpha, 16.0 / 9.0);
        drawn += view.instances.len();
        assert!(view.lights.len() <= render::MAX_LIGHTS, "frame {frame}");
    }
    assert_eq!(
        allocations() - before,
        0,
        "10 s of traffic frames allocated"
    );
    assert!(drawn > 0);
}
