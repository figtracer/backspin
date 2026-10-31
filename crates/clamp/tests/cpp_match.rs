//! The port against the C++ audio path of clamp-mac (`Clamp/ClampDSP.h`).
//!
//! `matches_cpp` builds `tests/cpp/clamp_ref.cpp` against a clamp-mac checkout (`CLAMP_MAC_DIR`,
//! default `~/dev/clamp-mac`) with the system C++ compiler, runs the full suite through both, and
//! requires bit-identical doubles. It skips when the checkout or the compiler is absent.
//! `CLAMP_BLESS=1` also writes the C++ output of the small suite to `tests/data/reference.bin`.
//!
//! `matches_reference` checks the small suite against that file on every machine: exactly on
//! macOS, elsewhere within 1 f32 ulp or 1e-12 (the C math library can differ in the last bit).

use backspin_clamp::{Clamp, Params};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One step of a scenario.
enum Cmd {
    New { sample_rate: f32, max_block: usize },
    Params(Params),
    Block(Vec<f32>, Vec<f32>),
}

/// xorshift64*, so the suite is the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.below(v.len())]
    }
}

const SAMPLE_RATES: [f32; 3] = [44100.0, 48000.0, 96000.0];
const IN_DB: [f64; 6] = [-24.0, -6.5, 0.0, 6.0, 12.0, 24.0];
const OUT_DB: [f64; 5] = [-24.0, -3.5, 0.0, 6.0, 24.0];
const CEILING_DB: [f64; 5] = [-48.0, -20.5, -6.0, -0.5, 0.0];
const SHAPE: [f64; 7] = [0.0, 0.01, 0.25, 0.37, 0.5, 0.99, 1.0];

/// A block of one of several test signals, as f32 values.
fn signal(rng: &mut Rng, n: usize, phase: &mut f64) -> (Vec<f32>, Vec<f32>) {
    let kind = rng.below(7);
    let amp = rng.pick(&[0.001, 0.1, 0.5, 1.0, 2.0, 4.0]);
    let freq = rng.pick(&[0.0005, 0.01, 0.05, 0.2, 0.45]);
    let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let (a, b) = match kind {
            // Sine, the right channel shifted.
            0 => {
                *phase += freq;
                let t = *phase * std::f64::consts::TAU;
                (amp * t.sin(), amp * (t + 1.0).sin())
            }
            // Noise.
            1 => (
                amp * (rng.unit() * 2.0 - 1.0),
                amp * (rng.unit() * 2.0 - 1.0),
            ),
            // Correlated noise, for the M and S modes.
            2 => {
                let m = amp * (rng.unit() * 2.0 - 1.0);
                (m, m * 0.9 + 0.01 * (rng.unit() - 0.5))
            }
            // Impulses on silence.
            3 => {
                let hit = rng.below(64) == 0;
                (
                    if hit { amp } else { 0.0 },
                    if i % 17 == 0 { -amp } else { 0.0 },
                )
            }
            // Silence: log(0) in TYPE B.
            4 => (0.0, 0.0),
            // DC and a tiny right channel.
            5 => (amp, 1e-30),
            // Sweep.
            _ => {
                *phase += freq * (i as f64 / n as f64);
                let t = *phase * std::f64::consts::TAU;
                (amp * t.sin(), -amp * t.cos())
            }
        };
        l.push(a as f32);
        r.push(b as f32);
    }
    (l, r)
}

fn random_params(rng: &mut Rng, p: &mut Params) {
    p.in_db = rng.pick(&IN_DB);
    p.out_db = rng.pick(&OUT_DB);
    p.ceiling_db = rng.pick(&CEILING_DB);
    p.shape = rng.pick(&SHAPE);
}

/// Every TYPE, MODE and OVERSAMPLING combination with parameter changes between blocks, then
/// scenarios that change OVERSAMPLING, TYPE, MODE and BYPASS between blocks. `blocks` sets the
/// length of each scenario.
fn suite(blocks: usize) -> Vec<Cmd> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut cmds = Vec::new();
    let mut i = 0;
    for kind in 0..2u8 {
        for mode in 0..6u8 {
            for oversampling in 0..5u8 {
                let max_block = rng.pick(&[32, 64, 128, 512]);
                cmds.push(Cmd::New {
                    sample_rate: SAMPLE_RATES[i % 3],
                    max_block,
                });
                i += 1;
                let mut p = Params {
                    oversampling,
                    kind,
                    mode,
                    ..Params::default()
                };
                let mut phase = 0.0;
                for b in 0..blocks {
                    // The small suite changes them at every block.
                    if b % 3 == 0 || blocks < 6 {
                        random_params(&mut rng, &mut p);
                        cmds.push(Cmd::Params(p));
                    }
                    let n = if rng.below(4) == 0 {
                        max_block
                    } else {
                        1 + rng.below(max_block)
                    };
                    let (l, r) = signal(&mut rng, n, &mut phase);
                    cmds.push(Cmd::Block(l, r));
                }
            }
        }
    }

    // Everything changes between blocks.
    for s in 0..6 {
        let max_block = 256;
        cmds.push(Cmd::New {
            sample_rate: SAMPLE_RATES[s % 3],
            max_block,
        });
        let mut p = Params::default();
        let mut phase = 0.0;
        for _ in 0..4 * blocks {
            random_params(&mut rng, &mut p);
            match rng.below(4) {
                0 => p.oversampling = rng.below(5) as u8,
                1 => p.kind = rng.below(2) as u8,
                2 => p.mode = rng.below(6) as u8,
                _ => p.bypass = rng.below(3) == 0,
            }
            cmds.push(Cmd::Params(p));
            let n = 1 + rng.below(max_block);
            let (l, r) = signal(&mut rng, n, &mut phase);
            cmds.push(Cmd::Block(l, r));
        }
    }
    cmds
}

/// One block's output: the doubles of both channels and the meter.
struct Out {
    l: Vec<f64>,
    r: Vec<f64>,
    meter: (f32, f32),
}

fn encode(cmds: &[Cmd]) -> Vec<u8> {
    let mut v = Vec::new();
    for c in cmds {
        match c {
            Cmd::New {
                sample_rate,
                max_block,
            } => {
                v.push(b'n');
                v.extend((*sample_rate as f64).to_le_bytes());
                v.extend((*max_block as u32).to_le_bytes());
            }
            Cmd::Params(p) => {
                v.push(b'p');
                v.extend([p.bypass as u8, p.oversampling, p.kind, p.mode]);
                for x in [p.in_db, p.out_db, p.ceiling_db, p.shape] {
                    v.extend(x.to_le_bytes());
                }
            }
            Cmd::Block(l, r) => {
                v.push(b'b');
                v.extend((l.len() as u32).to_le_bytes());
                for x in l.iter().chain(r) {
                    v.extend((*x as f64).to_le_bytes());
                }
            }
        }
    }
    v
}

fn decode(cmds: &[Cmd], bytes: &[u8]) -> Vec<Out> {
    let mut at = 0;
    let mut take = |n: usize| {
        let s = &bytes[at..at + n];
        at += n;
        s
    };
    let mut outs = Vec::new();
    for c in cmds {
        if let Cmd::Block(l, _) = c {
            let n = l.len();
            let mut f64s = |n: usize| -> Vec<f64> {
                take(8 * n)
                    .chunks(8)
                    .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
                    .collect()
            };
            let (l, r) = (f64s(n), f64s(n));
            let m = take(8);
            let meter = (
                f32::from_le_bytes(m[..4].try_into().unwrap()),
                f32::from_le_bytes(m[4..].try_into().unwrap()),
            );
            outs.push(Out { l, r, meter });
        }
    }
    outs
}

/// Runs the scenario through the port, with the f64 or the f32 interface.
fn run_rust(cmds: &[Cmd], f64_io: bool) -> Vec<Out> {
    let mut clamp = None;
    let mut p = Params::default();
    let mut outs = Vec::new();
    for c in cmds {
        match c {
            Cmd::New {
                sample_rate,
                max_block,
            } => {
                clamp = Some(Clamp::new(*sample_rate, *max_block));
                p = Params::default();
            }
            Cmd::Params(q) => p = *q,
            Cmd::Block(l, r) => {
                let clamp = clamp.as_mut().unwrap();
                let (l, r) = if f64_io {
                    let mut l: Vec<f64> = l.iter().map(|&x| x as f64).collect();
                    let mut r: Vec<f64> = r.iter().map(|&x| x as f64).collect();
                    clamp.process_f64(&mut l, &mut r, &p);
                    (l, r)
                } else {
                    let (mut l, mut r) = (l.clone(), r.clone());
                    clamp.process(&mut l, &mut r, &p);
                    let wide = |v: Vec<f32>| v.into_iter().map(|x| x as f64).collect();
                    (wide(l), wide(r))
                };
                outs.push(Out {
                    l,
                    r,
                    meter: clamp.meter(),
                });
            }
        }
    }
    outs
}

/// Builds the C++ harness, or `None` when clamp-mac or the compiler is absent.
fn harness() -> Option<PathBuf> {
    let root = std::env::var_os("CLAMP_MAC_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("dev/clamp-mac")))?;
    if !root.join("Clamp/ClampDSP.h").exists() {
        eprintln!("clamp-mac not found at {}; skipped", root.display());
        return None;
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cpp/clamp_ref.cpp");
    let exe = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clamp_ref");
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
    let iplug = root.join("iPlug2");
    // No FMA contraction: the original (MSVC x64) and Rust do not fuse.
    let status = Command::new(&cxx)
        .args(["-std=c++17", "-O2", "-ffp-contract=off"])
        .arg(format!("-I{}", root.join("Clamp").display()))
        .arg(format!("-I{}", iplug.join("IPlug").display()))
        .arg(format!("-I{}", iplug.join("IPlug/Extras").display()))
        .arg(format!("-I{}", iplug.join("WDL").display()))
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .status();
    match status {
        Ok(s) if s.success() => Some(exe),
        _ => {
            eprintln!("could not build the C++ harness with {cxx}; skipped");
            None
        }
    }
}

fn run_cpp(exe: &Path, cmds: &[Cmd], name: &str) -> Vec<Out> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let (script, output) = (
        dir.join(format!("{name}.in")),
        dir.join(format!("{name}.out")),
    );
    std::fs::write(&script, encode(cmds)).unwrap();
    let status = Command::new(exe)
        .arg(&script)
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success(), "harness failed: {status}");
    decode(cmds, &std::fs::read(&output).unwrap())
}

/// Distance in f32 ulps (same-sign ordering; NaN equals NaN).
fn ulps(a: f32, b: f32) -> u32 {
    if a.is_nan() && b.is_nan() {
        return 0;
    }
    let key = |x: f32| {
        let i = x.to_bits() as i32;
        if i < 0 { i32::MIN - i } else { i }
    };
    key(a).abs_diff(key(b))
}

/// How port output compares with reference output.
#[derive(Debug)]
struct Stats {
    samples: usize,
    /// Samples with bit-identical f64 values (for an f64 reference).
    identical: usize,
    /// The largest f32 ulp distance of the audio and of the meter.
    worst: u32,
    worst_meter: u32,
    /// Samples more than 1 f32 ulp and more than 1e-12 apart.
    far: usize,
}

fn compare(port: &[Out], reference: &[Out]) -> Stats {
    assert_eq!(port.len(), reference.len());
    let mut st = Stats {
        samples: 0,
        identical: 0,
        worst: 0,
        worst_meter: 0,
        far: 0,
    };
    for (a, b) in port.iter().zip(reference) {
        for (x, y) in a.l.iter().chain(&a.r).zip(b.l.iter().chain(&b.r)) {
            st.samples += 1;
            st.identical += (x.to_bits() == y.to_bits()) as usize;
            let u = ulps(*x as f32, *y as f32);
            st.worst = st.worst.max(u);
            st.far += (u > 1 && (x - y).abs() > 1e-12) as usize;
        }
        st.worst_meter = st
            .worst_meter
            .max(ulps(a.meter.0, b.meter.0))
            .max(ulps(a.meter.1, b.meter.1));
    }
    st
}

const SMALL_BLOCKS: usize = 2;
const FULL_BLOCKS: usize = 24;

fn reference_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/reference.bin")
}

#[test]
fn matches_cpp() {
    let Some(exe) = harness() else { return };
    let cmds = suite(FULL_BLOCKS);
    let cpp = run_cpp(&exe, &cmds, "full");

    let st = compare(&run_rust(&cmds, true), &cpp);
    eprintln!("f64 interface: {st:?}");
    assert_eq!(st.identical, st.samples);
    assert_eq!(st.worst_meter, 0);

    // The f32 interface rounds the same doubles.
    let st = compare(&run_rust(&cmds, false), &cpp);
    eprintln!("f32 interface: {st:?}");
    assert_eq!((st.worst, st.worst_meter), (0, 0));

    if std::env::var_os("CLAMP_BLESS").is_some() {
        let small = suite(SMALL_BLOCKS);
        let mut v = Vec::new();
        for o in run_cpp(&exe, &small, "small") {
            for x in o.l.iter().chain(&o.r) {
                v.extend((*x as f32).to_le_bytes());
            }
            v.extend(o.meter.0.to_le_bytes());
            v.extend(o.meter.1.to_le_bytes());
        }
        std::fs::write(reference_path(), v).unwrap();
    }
}

/// Infinities and NaN, which the shapers pass through `std::clamp`, `std::min` and `std::fmax`.
#[test]
fn matches_cpp_non_finite() {
    let Some(exe) = harness() else { return };
    let mut cmds = Vec::new();
    let values = [
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        1e30,
        -1e-40,
        0.0,
        -0.0,
    ];
    for kind in 0..2u8 {
        for mode in 0..6u8 {
            for (i, &v) in values.iter().enumerate() {
                cmds.push(Cmd::New {
                    sample_rate: 48000.0,
                    max_block: 16,
                });
                let p = Params {
                    oversampling: (i % 5) as u8,
                    kind,
                    mode,
                    shape: [0.0, 0.5, 1.0][i % 3],
                    ceiling_db: -12.0,
                    ..Params::default()
                };
                cmds.push(Cmd::Params(p));
                let l: Vec<f32> = (0..16).map(|j| if j == 3 { v } else { 0.25 }).collect();
                let r: Vec<f32> = (0..16).map(|j| if j == 5 { v } else { -0.5 }).collect();
                cmds.push(Cmd::Block(l, r));
            }
        }
    }
    let cpp = run_cpp(&exe, &cmds, "non_finite");
    let st = compare(&run_rust(&cmds, true), &cpp);
    eprintln!("{st:?}");
    assert_eq!((st.worst, st.worst_meter), (0, 0));
}

#[test]
fn matches_reference() {
    let cmds = suite(SMALL_BLOCKS);
    let bytes = std::fs::read(reference_path()).unwrap();
    let mut f32s = bytes
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()));
    let mut reference = Vec::new();
    for c in &cmds {
        if let Cmd::Block(l, _) = c {
            let n = l.len();
            let l = (&mut f32s).take(n).map(|x| x as f64).collect();
            let r = (&mut f32s).take(n).map(|x| x as f64).collect();
            let meter = (f32s.next().unwrap(), f32s.next().unwrap());
            reference.push(Out { l, r, meter });
        }
    }
    assert!(
        f32s.next().is_none(),
        "reference.bin does not match the suite"
    );
    let st = compare(&run_rust(&cmds, false), &reference);
    eprintln!("{st:?}");
    if cfg!(target_os = "macos") {
        // Same libm as the C++ build that wrote the file.
        assert_eq!((st.worst, st.worst_meter), (0, 0));
    } else {
        assert_eq!(st.far, 0);
        assert!(st.worst_meter <= 4);
    }
}
