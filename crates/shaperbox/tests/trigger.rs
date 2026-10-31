//! The LFO trigger modes (core.md 5.5): MIDI retrigger and one-shot timing, the lookahead
//! latencies, the version-56 algorithm remap, and no allocation with events and the audio
//! trigger.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use backspin_shaperbox::graph::{BASE_DELAY, NoteEvent, Processor, Transport};
use backspin_shaperbox::state::{AudioTrigger, Band, Lfo, Section, State};
use backspin_shaperbox::valuetree::Var;
use backspin_shaperbox::wave::Point;

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
}

#[global_allocator]
static A: Counting = Counting;

const SR: f32 = 48000.0;
const T: Transport = Transport {
    ppq: 0.0,
    bpm: 120.0,
    playing: true,
};

/// A linear saw from 1 to -1: the mapped wave is `1 - phase`.
fn saw() -> Vec<Point> {
    vec![
        Point {
            x: 0,
            y: 1.0,
            factor: 3,
        },
        Point {
            x: 8192,
            y: -1.0,
            factor: 3,
        },
    ]
}

/// A Volume shaper alone with `lfo` on every band, unity gain, no LFO smoothing.
fn volume(lfo: Lfo, trigger_smoothing: bool) -> State {
    let mut s = State {
        trigger_smoothing,
        ..State::default()
    };
    let sh = &mut s.shapers[5];
    sh.visible = true;
    sh.on = true;
    sh.bands = std::array::from_fn(|_| Band {
        mix: 1.0,
        sections: vec![Section {
            id: "volume".into(),
            lfo: lfo.clone(),
            env: Default::default(),
        }],
        values: vec![("gain".into(), Var::Double(0.5))],
    });
    s
}

/// Renders a DC input of `n` samples with the notes; returns the output.
fn render(state: State, n: usize, notes: &[NoteEvent]) -> Vec<[f32; 2]> {
    let mut p = Processor::new(state, SR);
    let mut buf = vec![[1.0f32; 2]; n];
    let mut events = Vec::new();
    for (k, chunk) in buf.chunks_mut(512).enumerate() {
        let start = k * 512;
        events.clear();
        events.extend(
            notes
                .iter()
                .filter(|e| (start..start + chunk.len()).contains(&e.offset))
                .map(|e| NoteEvent {
                    offset: e.offset - start,
                    ..*e
                }),
        );
        let t = Transport {
            ppq: start as f64 * T.bpm / 60.0 / SR as f64,
            ..T
        };
        p.process_with_events(chunk, t, None, &events);
    }
    buf
}

fn note(offset: usize, on: bool) -> NoteEvent {
    NoteEvent {
        offset,
        channel: 1,
        note: 60,
        on,
    }
}

/// Retrigger (triggerMode 1, 1 beat): free-running from phase 0, the phase restarts at the note
/// sample; a note-off does not trigger (captures/exp_lfo e1 and e6 of the RE workspace).
#[test]
fn midi_retrigger_restarts_the_phase_at_the_note() {
    let lfo = Lfo {
        points: saw(),
        trigger_mode: 1,
        length: 10,
        smoothing: 0.0,
        ..Lfo::default()
    };
    let out = render(
        volume(lfo, false),
        48000,
        &[note(14400, true), note(20000, false)],
    );
    let w = |k: usize| out[k + BASE_DELAY][0];
    assert_eq!(w(0), 1.0);
    assert!((w(14399) - (1.0 - 14399.0 / 24000.0)).abs() < 1e-6);
    assert_eq!(w(14400), 1.0);
    assert!((w(14400 + 1200) - (1.0 - 1200.0 / 24000.0)).abs() < 1e-6);
    assert!((w(20000) - (1.0 - 5600.0 / 24000.0)).abs() < 1e-6);
}

/// One-shot (triggerMode 2, end phase 0.5): holds the snapped wave value at the end phase before
/// the first note, runs the wave to `end - 1/64`, ramps to the end value with 64 increments per
/// sample and holds (e4).
#[test]
fn one_shot_holds_runs_and_ramps() {
    let lfo = Lfo {
        points: saw(),
        trigger_mode: 2,
        length: 10,
        smoothing: 0.0,
        one_shot_end_phase: 0.5,
        ..Lfo::default()
    };
    let out = render(volume(lfo, false), 48000, &[note(14400, true)]);
    let w = |k: usize| out[k + BASE_DELAY][0];
    assert_eq!(w(0), 0.5);
    assert_eq!(w(14399), 0.5);
    assert_eq!(w(14400), 1.0);
    // The table runs to phase 0.484375 (sample 11625), the ramp ends at phase 0.5 (sample 11999).
    assert!((w(14400 + 11000) - (1.0 - 11000.0 / 24000.0)).abs() < 1e-6);
    let ramp = w(14400 + 11800);
    assert!(ramp > 0.5 && ramp < 1.0 - 11625.0 / 24000.0);
    assert_eq!(w(14400 + 11999), 0.5);
    assert_eq!(w(14400 + 20000), 0.5);
}

/// With `triggerSmoothing` the Volume runs 289 samples late and ramps into the reset over 6 ms
/// (e2): the reset lands 305 samples after the note in the output.
#[test]
fn trigger_smoothing_adds_the_volume_lookahead() {
    let lfo = Lfo {
        points: saw(),
        trigger_mode: 1,
        length: 10,
        smoothing: 0.0,
        ..Lfo::default()
    };
    let st = volume(lfo, true);
    assert_eq!(Processor::new(st.clone(), SR).latency(), BASE_DELAY + 289);
    let out = render(st, 48000, &[note(14400, true)]);
    assert_eq!(out[304][0], 0.0);
    assert!((out[305][0] - (1.0 - 289.0 / 24000.0)).abs() < 1e-6);
    assert_eq!(out[14400 + 305][0], 1.0);
    // The ramp from 0.6 toward 1 runs ahead of the reset.
    let mid = out[14400 + 305 - 100][0];
    assert!(mid > 0.5 && mid < 1.0);
}

/// A section that listens to the audio trigger adds its lookahead to the latency and the delay,
/// whether the detector is active or not.
#[test]
fn audio_trigger_lookahead() {
    let lfo = Lfo {
        points: saw(),
        trigger_mode: 1,
        trigger_source: 1,
        length: 10,
        smoothing: 0.0,
        ..Lfo::default()
    };
    let mut st = volume(lfo, false);
    st.audio_trigger = AudioTrigger {
        active: false,
        ..AudioTrigger::default()
    };
    assert_eq!(Processor::new(st.clone(), SR).latency(), BASE_DELAY + 672);
    let out = render(st, 4096, &[]);
    assert_eq!(out[687][0], 0.0);
    assert!((out[688][0] - (1.0 - 672.0 / 24000.0)).abs() < 1e-6);
}

/// Version 56 presets store the detector algorithm with an offset of one.
#[test]
fn version_56_algorithm_remap() {
    use backspin_shaperbox::valuetree::Tree;
    let leaf = |kind: &str, id: &str, v: Var| Tree {
        kind: kind.into(),
        props: vec![("id".into(), Var::String(id.into())), ("value".into(), v)],
        children: Vec::new(),
    };
    let tree = |version: &str, alg: i32| Tree {
        kind: "PluginState".into(),
        props: vec![("version".into(), Var::String(version.into()))],
        children: vec![Tree {
            kind: "AudioTriggerState".into(),
            props: vec![("id".into(), Var::String("audioTriggering".into()))],
            children: vec![
                leaf("bool", "active", Var::Bool(true)),
                leaf("int", "algorithm", Var::Int(alg)),
            ],
        }],
    };
    for (v, alg, want) in [
        ("56", 3, 2),
        ("56", 1, 0),
        ("56", 0, 0),
        ("74", 2, 2),
        ("75", 1, 1),
    ] {
        let s = State::from_tree(&tree(v, alg));
        assert_eq!(
            s.audio_trigger.algorithm, want,
            "version {v} algorithm {alg}"
        );
        assert!(s.audio_trigger.active);
    }
}

#[test]
fn process_with_events_does_not_allocate() {
    let lfo = Lfo {
        points: saw(),
        trigger_mode: 2,
        trigger_source: 1,
        length: 10,
        ..Lfo::default()
    };
    let mut st = volume(lfo, true);
    st.audio_trigger = AudioTrigger {
        active: true,
        algorithm: 1,
        ..AudioTrigger::default()
    };
    st.shapers[5].bands[1].sections[0].lfo.trigger_source = 0;
    let mut p = Processor::new(st, SR);
    let mut buf: Vec<[f32; 2]> = (0..48000)
        .map(|i| {
            let v = if i % 12000 < 2000 { 0.8 } else { 0.0 } * ((i * 7919) % 1000) as f32 / 1000.0;
            [v, -v]
        })
        .collect();
    let sc = buf.clone();
    let notes = [note(10, true), note(600, true), note(900, false)];
    ALLOCS.store(0, Ordering::Relaxed);
    COUNTING.set(true);
    for (k, chunk) in buf.chunks_mut(1000).enumerate() {
        let sidechain = (k % 2 == 0).then_some(&sc[..chunk.len()]);
        p.process_with_events(chunk, T, sidechain, &notes);
    }
    COUNTING.set(false);
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0);
    assert!(buf.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
}
