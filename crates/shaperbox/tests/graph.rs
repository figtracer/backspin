//! The processing graph with Dynamics, Pitch and section envelope followers: latency, and no
//! allocation in `process` (the audio thread).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use backspin_shaperbox::graph::{BASE_DELAY, Processor, Transport};
use backspin_shaperbox::shapers::envfollow::EnvParams;
use backspin_shaperbox::state::{Band, Lfo, Section, State};
use backspin_shaperbox::valuetree::Var;
use backspin_shaperbox::wave::Point;

/// Counts the allocations a thread makes while its `COUNTING` is set.
struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

fn counting() -> bool {
    COUNTING.try_with(Cell::get).unwrap_or(false)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if counting() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if counting() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

/// A section with a moving wave and, when `env` is set, an active follower.
fn section(id: &str, env: Option<EnvParams>) -> Section {
    let points = vec![
        Point {
            x: 0,
            y: 0.5,
            factor: 3,
        },
        Point {
            x: 4096,
            y: -0.5,
            factor: 3,
        },
        Point {
            x: 8192,
            y: 0.5,
            factor: 3,
        },
    ];
    Section {
        id: id.into(),
        lfo: Lfo {
            points,
            ..Lfo::default()
        },
        env: env.unwrap_or_default(),
    }
}

fn band(sections: Vec<Section>, values: Vec<(&str, Var)>) -> Band {
    Band {
        mix: 0.7,
        sections,
        values: values.into_iter().map(|(k, v)| (k.into(), v)).collect(),
    }
}

/// Pitch (WSOLA), Dynamics and Width with followers, every one split into three bands.
fn state() -> State {
    let mut s = State::default();
    let env = EnvParams {
        active: true,
        amount: 0.8,
        filter_setup_is_free: true,
        hp_cutoff: 0.3,
        ..EnvParams::default()
    };
    for k in [7, 10, 15] {
        let sh = &mut s.shapers[k];
        sh.visible = true;
        sh.on = true;
        sh.split.low = 0.3;
        sh.split.high = 0.7;
        sh.split.low_slope = 2;
        sh.split.high_slope = 1;
    }
    s.shapers[7].bands = std::array::from_fn(|_| band(vec![section("width", Some(env))], vec![]));
    s.shapers[10].bands =
        std::array::from_fn(|_| band(vec![section("dynamics", Some(env))], vec![]));
    s.shapers[15].bands = std::array::from_fn(|b| {
        band(
            vec![section("pitch", Some(env)), section("formant", None)],
            vec![("grainSizeWsola", Var::Int(b as i32 % 3))],
        )
    });
    s
}

#[test]
fn process_does_not_allocate() {
    for mode in 0..4 {
        let mut st = state();
        st.shapers[15].mode = mode;
        let mut p = Processor::new(st, 48000.0);
        let mut buf: Vec<[f32; 2]> = (0..48000)
            .map(|i| {
                let v = ((i * 7919) % 1000) as f32 / 1000.0 - 0.5;
                [v, -v]
            })
            .collect();
        let sc = buf.clone();
        let t = Transport {
            ppq: 0.0,
            bpm: 120.0,
            playing: true,
        };
        ALLOCS.store(0, Ordering::Relaxed);
        COUNTING.set(true);
        for (k, chunk) in buf.chunks_mut(1000).enumerate() {
            if k % 2 == 0 {
                p.process(chunk, t);
            } else {
                p.process_with_sidechain(chunk, t, Some(&sc[..chunk.len()]));
            }
        }
        COUNTING.set(false);
        assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "mode {mode}");
        assert!(buf.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
    }
}

#[test]
fn latency_follows_use_constant_base_latency() {
    let mut st = state();
    assert_eq!(Processor::new(st.clone(), 48000.0).latency(), BASE_DELAY);
    st.use_constant_base_latency = false;
    let mut p = Processor::new(st, 48000.0);
    assert_eq!(p.latency(), 0);
    // Without the base delay and with no Drive the dry path is not delayed.
    p.set_state(State {
        use_constant_base_latency: false,
        ..State::default()
    });
    let mut buf = vec![[0.25f32, -0.5]; 64];
    p.process(
        &mut buf,
        Transport {
            ppq: 0.0,
            bpm: 120.0,
            playing: true,
        },
    );
    assert_eq!(buf[0], [0.25, -0.5]);
}
