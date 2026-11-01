//! Stress tests of the audio chain, as the plugin runs it: the engine with its command queues,
//! the shape section with its processor mailbox, Clamp and the output guard. They mimic a user
//! who scrolls through the shape and Gross Beat presets while the host plays, with host
//! parameter changes, MIDI, transport jumps, Clamp at its extremes and hostile input. Every
//! block must come out finite and bounded without the guard tripping; a trip names the stage,
//! the presets and the settings that produced it.
//!
//! `stress_quick` runs a small budget on every test run. `stress_scroll` (`cargo test --release
//! -p backspin stress_scroll -- --ignored --nocapture`) walks the whole shape library at several
//! sample rates and block sizes; `BACKSPIN_STRESS_SEED` picks another random sequence.

use crate::params::{BackspinParams, ClampParams};
use crate::shared::Command;
use crate::{Backspin, Clock, GUARD_LIMIT, Midi};
use backspin_engine::{Engine, PARAMS, fst_state};
use nih_plug::prelude::*;
use std::sync::Arc;

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// In `0..n`.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// In `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// In `[-1, 1)`.
    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

/// What the host feeds the plugin.
#[derive(Clone, Copy, Debug)]
enum Input {
    Silence,
    /// White noise at a peak level (1 is full scale, 4 is +12 dB).
    Noise(f32),
    Dc(f32),
    /// One full-scale frame per block.
    Impulse,
    /// Values around 1e-39, below the normal range.
    Denormal,
    Square(f32),
    /// Noise with NaN and infinities sprinkled in: the sanitizer's job.
    Nasty,
    /// Noise with values of 1e30 sprinkled in. They are finite, so they go through the chain
    /// and the guard is expected to trip, at once or when the engine replays them from its
    /// ring; the trip flushes the chain, and from then on the output must be clean again.
    Huge,
}

const INPUTS: [Input; 11] = [
    Input::Silence,
    Input::Noise(1.0),
    Input::Noise(4.0),
    Input::Noise(0.01),
    Input::Dc(1.0),
    Input::Dc(-0.5),
    Input::Impulse,
    Input::Denormal,
    Input::Square(4.0),
    Input::Nasty,
    Input::Huge,
];

fn fill(rng: &mut Rng, kind: Input, l: &mut [f32], r: &mut [f32]) {
    for i in 0..l.len() {
        let (a, b) = match kind {
            Input::Silence => (0.0, 0.0),
            Input::Noise(g) => (rng.signed() * g, rng.signed() * g),
            Input::Dc(v) => (v, v),
            Input::Impulse => {
                let v = if i == 0 { 1.0 } else { 0.0 };
                (v, -v)
            }
            Input::Denormal => (rng.signed() * 1e-39, rng.signed() * 1e-39),
            Input::Square(g) => {
                let v = if i % 64 < 32 { g } else { -g };
                (v, v)
            }
            Input::Nasty => {
                let bad = |rng: &mut Rng, x: f32| match rng.below(64) {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    2 => f32::NEG_INFINITY,
                    _ => x,
                };
                let (a, b) = (rng.signed(), rng.signed());
                (bad(rng, a), bad(rng, b))
            }
            Input::Huge => {
                let big = |rng: &mut Rng, x: f32| if rng.chance(64) { 1e30 } else { x };
                let (a, b) = (rng.signed(), rng.signed());
                (big(rng, a), big(rng, b))
            }
        };
        l[i] = a;
        r[i] = b;
    }
}

/// The Gross Beat presets: FL's factory folder when installed, else the embedded pack.
fn gross_beat_presets() -> Vec<(String, Vec<u8>)> {
    backspin_presets::gross_beat_folders(crate::sb::embedded())
        .iter()
        .flat_map(|f| f.presets.iter())
        .filter_map(|p| Some((p.name.clone(), p.bytes()?.into_owned())))
        .collect()
}

/// The shape library's presets, read once.
fn shape_presets() -> Vec<(String, Vec<u8>)> {
    let lib = crate::sb::library();
    lib.first()
        .map(|all| {
            all.presets
                .iter()
                .filter_map(|p| Some((p.name.clone(), p.bytes()?.into_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// Loads a Gross Beat preset as the editor does (`menus::load_preset_bytes`): the model takes
/// the state, the slots and options go to the engine through the command queue, and the
/// parameters go through the host. Returns the host parameter values.
fn load_gross_beat(plugin: &Backspin, data: &[u8]) -> [i32; 9] {
    let blob = fst_state(data).unwrap_or(data);
    let mut new = Engine::new();
    new.load_state(blob);
    let params = new.params;
    let options = Command::Options {
        remove_dc: new.remove_dc,
        attack_comp: new.attack_comp,
        hq: new.hq,
        click: new.click,
    };
    let midi = Command::MidiTrigger(new.midi_trigger);
    {
        let mut m = plugin.shared.model.lock();
        *m = new;
        for bank in 0..2 {
            for i in 0..36 {
                plugin.shared.send_slot(&m, bank, i, false);
            }
        }
    }
    plugin.shared.send(options);
    plugin.shared.send(midi);
    params
}

/// Clamp's host settings.
#[derive(Clone, Copy, Debug)]
struct ClampSettings {
    on: bool,
    oversampling: i32,
    input: f32,
    output: f32,
    ceiling: f32,
    shape: f32,
    kind: i32,
    mode: i32,
}

impl ClampSettings {
    fn random(rng: &mut Rng) -> Self {
        // Extremes often: the corners of every range.
        let corner = |rng: &mut Rng, lo: f32, hi: f32| match rng.below(4) {
            0 => lo,
            1 => hi,
            _ => lo + (hi - lo) * rng.unit(),
        };
        Self {
            on: !rng.chance(4),
            oversampling: rng.below(5) as i32,
            input: (corner(rng, -24.0, 24.0) * 2.0).round() / 2.0,
            output: (corner(rng, -24.0, 24.0) * 2.0).round() / 2.0,
            ceiling: (corner(rng, -48.0, 0.0) * 2.0).round() / 2.0,
            shape: (corner(rng, 0.0, 1.0) * 100.0).round() / 100.0,
            kind: rng.below(2) as i32,
            mode: rng.below(6) as i32,
        }
    }
}

/// Gives the plugin host parameters with these values. nih-plug sets parameter values only from
/// its wrappers, so the test builds parameters whose defaults are the values it wants; the
/// plugin reads them as it reads the host's.
fn set_host_params(plugin: &mut Backspin, values: [i32; 9], clamp: ClampSettings) {
    let mut p = BackspinParams::new(plugin.shared.clone());
    let ints = [
        &mut p.time_slot,
        &mut p.volume_slot,
        &mut p.time_mix,
        &mut p.volume_mix,
        &mut p.attack,
        &mut p.release,
        &mut p.tension,
        &mut p.hold,
        &mut p.scratch,
    ];
    for ((param, info), v) in ints.into_iter().zip(PARAMS).zip(values) {
        *param = IntParam::new(
            info.name,
            v.clamp(info.min, info.max),
            IntRange::Linear {
                min: info.min,
                max: info.max,
            },
        );
    }
    let int =
        |name: &str, v: i32, max: i32| IntParam::new(name, v, IntRange::Linear { min: 0, max });
    let float = |name: &str, v: f32, min: f32, max: f32| {
        FloatParam::new(name, v.clamp(min, max), FloatRange::Linear { min, max })
    };
    p.clamp = ClampParams {
        on: BoolParam::new("Clamp", clamp.on),
        oversampling: int("Clamp oversampling", clamp.oversampling, 4),
        input: float("Clamp in", clamp.input, -24.0, 24.0),
        output: float("Clamp out", clamp.output, -24.0, 24.0),
        ceiling: float("Clamp ceiling", clamp.ceiling, -48.0, 0.0),
        shape: float("Clamp shape", clamp.shape, 0.0, 1.0),
        kind: int("Clamp type", clamp.kind, 1),
        mode: int("Clamp mode", clamp.mode, 5),
    };
    plugin.params = Arc::new(p);
}

/// Random host parameter values in the parameters' ranges.
fn random_params(rng: &mut Rng) -> [i32; 9] {
    let mut v = [0; 9];
    for (x, p) in v.iter_mut().zip(PARAMS) {
        *x = match rng.below(3) {
            0 => p.min,
            1 => p.max,
            _ => p.min + rng.below((p.max - p.min) as usize + 1) as i32,
        };
    }
    v
}

/// The settings of one run: a sample rate, the host's largest block, and how many blocks.
#[derive(Clone, Copy, Debug)]
struct Run {
    sr: f32,
    max_block: usize,
    blocks: usize,
    /// Switch shape presets in order through the library (as the preset arrows do) instead of
    /// at random.
    scroll: bool,
}

/// Everything that was in effect when a block ran, for the failure report (read through `Debug`).
#[derive(Debug)]
#[allow(dead_code)]
struct Context {
    block: usize,
    n: usize,
    input: Input,
    sidechain: Option<Input>,
    shape: String,
    gross_beat: String,
    params: [i32; 9],
    clamp: ClampSettings,
    clock: Clock,
    midi: usize,
}

/// The counts of one run.
#[derive(Debug, Default)]
struct Report {
    blocks: usize,
    frames: usize,
    shape_switches: usize,
    gross_beat_switches: usize,
    trips: usize,
    /// The trips with [`Input::Huge`] values in the chain's memory, which are expected.
    huge_trips: usize,
    /// The largest finite output magnitude.
    peak: f32,
}

#[allow(clippy::too_many_lines)]
fn run(run: Run, seed: u64, shapes: &[(String, Vec<u8>)], gbs: &[(String, Vec<u8>)]) -> Report {
    let mut rng = Rng(seed);
    let mut plugin = Backspin::default();
    plugin.setup(run.sr, run.max_block, false);
    let mut report = Report::default();
    let (mut l, mut r) = (vec![0.0; run.max_block], vec![0.0; run.max_block]);
    let (mut scl, mut scr) = (vec![0.0; run.max_block], vec![0.0; run.max_block]);
    let mut input = Input::Noise(1.0);
    let mut sidechain = Some(Input::Noise(1.0));
    let mut shape = String::from("(none)");
    let mut gross_beat = String::from("(default)");
    let mut params = Engine::new().params;
    let mut clamp = ClampSettings {
        on: false,
        oversampling: 0,
        input: 0.0,
        output: 0.0,
        ceiling: 0.0,
        shape: 1.0,
        kind: 0,
        mode: 0,
    };
    set_host_params(&mut plugin, params, clamp);
    let mut clock = Clock {
        pos: Some(0.0),
        tempo: 120.0,
        playing: true,
    };
    let mut shape_in = 1;
    let mut scroll_index = 0;
    // The last events, for the failure report.
    let mut log: Vec<String> = Vec::new();
    // Huge values went in since the last trip flushed the chain.
    let mut huge_pending = false;
    for block in 0..run.blocks {
        if log.len() > 24 {
            log.drain(..log.len() - 24);
        }
        // The user scrolls the shape presets every one to three blocks.
        shape_in -= 1;
        if shape_in == 0 && !shapes.is_empty() {
            shape_in = 1 + rng.below(3);
            let i = if run.scroll {
                scroll_index = (scroll_index + 1) % shapes.len();
                scroll_index
            } else {
                rng.below(shapes.len())
            };
            let (name, bytes) = &shapes[i];
            if plugin.shared.sb.load(bytes).is_ok() {
                shape = name.clone();
                report.shape_switches += 1;
            }
            // Now and then the section goes off, or the audio thread misses a few blocks of
            // pushes (the mailbox coalesces them).
            if rng.chance(40) {
                plugin.shared.sb.set_enabled(false);
                shape = format!("{shape} (off)");
            }
            if rng.chance(20) {
                for _ in 0..rng.below(8) {
                    let (name, bytes) = &shapes[rng.below(shapes.len())];
                    if plugin.shared.sb.load(bytes).is_ok() {
                        shape = name.clone();
                        report.shape_switches += 1;
                    }
                }
            }
        }
        // Gross Beat presets less often, with the parameters the host gets from the editor.
        if rng.chance(12) && !gbs.is_empty() {
            let (name, bytes) = &gbs[rng.below(gbs.len())];
            params = load_gross_beat(&plugin, bytes);
            log.push(format!("{block}: gb {name} params {params:?}"));
            gross_beat = name.clone();
            report.gross_beat_switches += 1;
            set_host_params(&mut plugin, params, clamp);
        }
        if rng.chance(6) {
            params = random_params(&mut rng);
            log.push(format!("{block}: params {params:?}"));
            set_host_params(&mut plugin, params, clamp);
        }
        if rng.chance(8) {
            clamp = ClampSettings::random(&mut rng);
            set_host_params(&mut plugin, params, clamp);
        }
        if rng.chance(10) {
            input = INPUTS[rng.below(INPUTS.len())];
        }
        if rng.chance(10) {
            sidechain = (!rng.chance(3)).then(|| INPUTS[rng.below(INPUTS.len())]);
        }
        // The transport: play and stop, tempo jumps, seeks and hosts without a position.
        if rng.chance(15) {
            clock.playing = !clock.playing;
            log.push(format!("{block}: playing {}", clock.playing));
        }
        if rng.chance(20) {
            clock.tempo = [20.0, 60.0, 128.0, 174.0, 300.0, 999.0][rng.below(6)];
            log.push(format!("{block}: tempo {}", clock.tempo));
        }
        if rng.chance(25) {
            clock.pos = (!rng.chance(4)).then(|| rng.unit() as f64 * 1000.0);
            log.push(format!("{block}: pos {:?}", clock.pos));
        }

        let n = match rng.below(4) {
            0 => 1 + rng.below(run.max_block),
            1 => run.max_block,
            _ => (run.max_block / 2).max(1),
        };
        fill(&mut rng, input, &mut l[..n], &mut r[..n]);
        if let Some(sc) = sidechain {
            fill(&mut rng, sc, &mut scl[..n], &mut scr[..n]);
        }
        let has_sc = plugin.take_input(&l[..n], &r[..n], sidechain.map(|_| (&scl[..n], &scr[..n])));
        plugin.midi.clear();
        let notes = if rng.chance(3) { rng.below(6) } else { 0 };
        for _ in 0..notes {
            let note = [24 + rng.below(72), rng.below(128)][rng.below(2)] as u32;
            let channel = rng.below(16) as u32;
            let msg = if rng.chance(3) {
                0x80 | channel | note << 8
            } else {
                0x90 | channel | note << 8 | (1 + rng.below(127) as u32) << 16
            };
            plugin.midi.push(Midi {
                at: rng.below(n + 1),
                msg,
            });
        }
        log.push(format!("{block}: n={n} input {input:?} midi {notes}"));
        let tripped = plugin.render_block(n, clock, has_sc);
        huge_pending |= matches!(input, Input::Huge);
        report.blocks += 1;
        report.frames += n;
        for f in &plugin.buf[..n] {
            for &x in f {
                assert!(
                    x.is_finite() && x.abs() <= GUARD_LIMIT,
                    "the guard let {x} through at block {block}"
                );
                report.peak = report.peak.max(x.abs());
            }
        }
        if tripped && huge_pending {
            report.huge_trips += 1;
            huge_pending = false;
            // The rebuild the background thread would do.
            plugin.shared.sb.push();
        } else if tripped {
            report.trips += 1;
            let context = Context {
                block,
                n,
                input,
                sidechain,
                shape: shape.clone(),
                gross_beat: gross_beat.clone(),
                params,
                clamp,
                clock,
                midi: notes,
            };
            eprintln!(
                "guard tripped after the {} stage at {} Hz: {context:#?}\n{}",
                plugin.trip_stage.unwrap_or("?"),
                run.sr,
                log.join("\n")
            );
            if report.trips >= 3 {
                return report;
            }
            // The rebuild the background thread would do.
            plugin.shared.sb.push();
        }
        if let Some(p) = clock.pos.as_mut()
            && clock.playing
        {
            *p += n as f64 * clock.tempo / 60.0 / run.sr as f64;
        }
    }
    report
}

fn seed() -> u64 {
    std::env::var("BACKSPIN_STRESS_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5eed)
}

/// A small budget on every test run: random presets at 48 kHz.
#[test]
fn stress_quick() {
    let shapes = shape_presets();
    let gbs = gross_beat_presets();
    let shapes = if shapes.len() > 120 {
        let mut rng = Rng(seed());
        (0..120)
            .map(|_| shapes[rng.below(shapes.len())].clone())
            .collect()
    } else {
        shapes
    };
    let report = run(
        Run {
            sr: 48000.0,
            max_block: 256,
            blocks: 300,
            scroll: false,
        },
        seed(),
        &shapes,
        &gbs,
    );
    eprintln!("{report:?}");
    assert_eq!(report.trips, 0, "{report:?}");
}

/// The whole library, scrolled in order and at random, at several sample rates and block sizes.
#[test]
#[ignore = "takes minutes; run on demand"]
fn stress_scroll() {
    let shapes = shape_presets();
    let gbs = gross_beat_presets();
    assert!(!shapes.is_empty(), "no shape library");
    let mut total = Report::default();
    for (i, (sr, max_block, scroll)) in [
        (44100.0, 512, true),
        (48000.0, 64, true),
        (96000.0, 2048, true),
        (44100.0, 1024, false),
        (48000.0, 256, false),
        (22050.0, 128, false),
    ]
    .into_iter()
    .enumerate()
    {
        let blocks = if scroll { shapes.len() * 2 } else { 4000 };
        let run = Run {
            sr,
            max_block,
            blocks,
            scroll,
        };
        let t0 = std::time::Instant::now();
        let report = self::run(run, seed() + i as u64, &shapes, &gbs);
        eprintln!("{run:?}: {report:?} in {:?}", t0.elapsed());
        total.blocks += report.blocks;
        total.frames += report.frames;
        total.shape_switches += report.shape_switches;
        total.gross_beat_switches += report.gross_beat_switches;
        total.trips += report.trips;
        total.huge_trips += report.huge_trips;
        total.peak = total.peak.max(report.peak);
    }
    eprintln!("total: {total:?}");
    assert_eq!(total.trips, 0, "{total:?}");
}
