//! Behaviour that the C++ comparison does not cover: bypass, blocks larger than `max_block`, and
//! no allocation on the audio path.

use backspin_clamp::{Clamp, Params};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Counts the allocations of each thread, to check that `process` makes none. Other tests run on
/// other threads at the same time.
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|a| a.set(a.get() + 1));
        // SAFETY: the caller's contract is passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract is passed on.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / (1 << 23) as f32 - 1.0
        })
        .collect()
}

fn hot() -> Params {
    Params {
        in_db: 18.0,
        ceiling_db: -6.0,
        shape: 0.3,
        ..Params::default()
    }
}

#[test]
fn bypass_passes_input_through() {
    let mut clamp = Clamp::new(48000.0, 64);
    let (mut l, mut r) = (noise(64, 1), noise(64, 2));
    let (l0, r0) = (l.clone(), r.clone());
    let p = Params {
        bypass: true,
        ..hot()
    };
    clamp.process(&mut l, &mut r, &p);
    assert_eq!((l, r), (l0, r0));
    assert_eq!(clamp.meter(), (0.0, 0.0));
}

#[test]
fn large_blocks_match_one_block() {
    for oversampling in 0..5 {
        let p = Params {
            oversampling,
            kind: oversampling % 2,
            mode: 3,
            ..hot()
        };
        let (l0, r0) = (noise(1000, 3), noise(1000, 4));
        let mut big = Clamp::new(44100.0, 1000);
        let (mut bl, mut br) = (l0.clone(), r0.clone());
        big.process(&mut bl, &mut br, &p);
        let mut small = Clamp::new(44100.0, 37);
        let (mut sl, mut sr) = (l0, r0);
        small.process(&mut sl, &mut sr, &p);
        assert_eq!((bl, br), (sl, sr));
        assert_eq!(big.meter(), small.meter());
    }
}

#[test]
fn process_does_not_allocate() {
    let mut clamp = Clamp::new(48000.0, 256);
    let (mut l, mut r) = (noise(256, 5), noise(256, 6));
    let before = ALLOCATIONS.with(Cell::get);
    for oversampling in [0, 4, 1, 3, 2] {
        for kind in 0..2 {
            for mode in 0..6 {
                let p = Params {
                    oversampling,
                    kind,
                    mode,
                    ..hot()
                };
                clamp.process(&mut l, &mut r, &p);
                clamp.process(&mut l[..17], &mut r[..17], &p);
            }
        }
    }
    assert_eq!(ALLOCATIONS.with(Cell::get), before);
}
