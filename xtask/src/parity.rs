//! `cargo xtask parity`: renders each case with the Gross Beat harness and with
//! `backspin-render`, with identical flags and input, and compares the outputs sample by sample.
//!
//! The harness writes its generated input with `--dump-input`, and the port reads that file, so
//! both get the same input. Noise inputs are written once per sample rate. The tolerance is zero:
//! a case passes only if every sample is bit-identical.

use backspin_engine::{Engine, fst_state};
use backspin_render::wav;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const HARNESS: &str = ".amp/in/harness/target/release/harness";
const RENDER: &str = "target/release/backspin-render";
const PRESET_DIR: &str = "/Applications/FL Studio 2026.app/Contents/Resources/FL/Data/Patches/Plugin presets/Effects/Gross Beat";
const PRESETS: [&str; 9] = [
    "Default",
    "Flanging",
    "Juggling Science",
    "Momentary",
    "Patterns",
    "Pitch shifter",
    "Repeater",
    "Stutter",
    "Turntablist",
];
const INPUTS: [&str; 4] = ["ramp", "sine:1000", "impulse:4410", "noise"];

/// One render configuration.
#[derive(Clone, Debug)]
struct Case {
    group: &'static str,
    name: String,
    /// Flags passed to both renderers (everything except the input and output).
    flags: Vec<String>,
    /// A generator spec, or "noise".
    input: String,
    sr: u32,
}

/// The result of one case.
#[derive(Clone, Debug)]
struct Outcome {
    case: Case,
    max: f32,
    first: Option<usize>,
    error: Option<String>,
}

impl Outcome {
    fn passed(&self) -> bool {
        self.error.is_none() && self.first.is_none()
    }
}

/// Runs the parity matrix. Flags: `--filter TEXT` (only cases whose name contains TEXT),
/// `--group NAME` (presets, configs, options, sweeps, pos, midi), `--jobs N`, `--keep` (keep the
/// WAV files of passing cases), `--quick` (one input per case).
pub fn run(args: &[String]) -> Result<(), String> {
    let mut filter = None;
    let mut groups: Vec<String> = Vec::new();
    let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut keep = false;
    let mut quick = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--filter" => filter = it.next().cloned(),
            "--group" => groups.push(it.next().cloned().ok_or("--group needs a value")?),
            "--jobs" => {
                jobs = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--jobs needs a number")?
            }
            "--keep" => keep = true,
            "--quick" => quick = true,
            other => return Err(format!("unknown parity flag {other}")),
        }
    }
    if !Path::new(HARNESS).exists() {
        return Err(format!(
            "{HARNESS} is missing; build it with cargo build --release in .amp/in/harness"
        ));
    }
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "backspin-render"])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("building backspin-render failed".into());
    }
    let out_dir = PathBuf::from("target/parity");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let mut cases = matrix(&out_dir)?;
    if !groups.is_empty() {
        cases.retain(|c| groups.iter().any(|g| g == c.group));
    }
    if let Some(f) = &filter {
        cases.retain(|c| c.name.contains(f.as_str()));
    }
    if quick {
        cases.retain(|c| c.input == "ramp" || c.group == "midi");
    }
    for sr in cases
        .iter()
        .filter(|c| c.input == "noise")
        .map(|c| c.sr)
        .collect::<std::collections::BTreeSet<_>>()
    {
        write_noise(&out_dir, sr)?;
    }
    eprintln!("parity: {} cases, {jobs} jobs", cases.len());

    let queue = Arc::new(Mutex::new(
        cases.into_iter().enumerate().collect::<Vec<_>>(),
    ));
    let results = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicUsize::new(0));
    let total = queue.lock().unwrap().len();
    let workers: Vec<_> = (0..jobs)
        .map(|_| {
            let (queue, results, done, out_dir) = (
                queue.clone(),
                results.clone(),
                done.clone(),
                out_dir.clone(),
            );
            std::thread::spawn(move || {
                loop {
                    let Some((i, case)) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let outcome = run_case(&out_dir, i, case, keep);
                    if !outcome.passed() {
                        eprintln!(
                            "FAIL {} max {:.3e} first {:?} {}",
                            outcome.case.name,
                            outcome.max,
                            outcome.first,
                            outcome.error.as_deref().unwrap_or("")
                        );
                    }
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 100 == 0 {
                        eprintln!("parity: {n}/{total}");
                    }
                    results.lock().unwrap().push((i, outcome));
                }
            })
        })
        .collect();
    for w in workers {
        w.join().map_err(|_| "worker panicked")?;
    }
    let mut results = std::mem::take(&mut *results.lock().unwrap());
    results.sort_by_key(|r| r.0);
    report(&out_dir, &results)
}

/// Prints one row per case and writes `target/parity/report.md`.
fn report(out_dir: &Path, results: &[(usize, Outcome)]) -> Result<(), String> {
    let mut md = String::from(
        "| group | case | max abs diff | first differing sample | result |\n|---|---|---|---|---|\n",
    );
    let mut by_group: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    for (_, o) in results {
        let first = o.first.map_or("-".to_string(), |f| f.to_string());
        let res = if let Some(e) = &o.error {
            format!("error: {e}")
        } else if o.passed() {
            "pass".into()
        } else {
            "FAIL".into()
        };
        println!(
            "{:<8} {:<80} {:>10.3e} {:>10} {res}",
            o.case.group, o.case.name, o.max, first
        );
        md += &format!(
            "| {} | {} | {:.3e} | {} | {} |\n",
            o.case.group, o.case.name, o.max, first, res
        );
        let g = by_group.entry(o.case.group).or_default();
        g.0 += 1;
        g.1 += o.passed() as usize;
    }
    let mut summary = String::from("| group | cases | passed |\n|---|---|---|\n");
    for (g, (n, p)) in &by_group {
        summary += &format!("| {g} | {n} | {p} |\n");
    }
    std::fs::write(out_dir.join("report.md"), format!("{summary}\n{md}"))
        .map_err(|e| e.to_string())?;
    let failed: usize = by_group.values().map(|(n, p)| n - p).sum();
    println!("\n{summary}");
    if failed > 0 {
        return Err(format!("{failed} cases failed"));
    }
    Ok(())
}

/// Renders one case with both renderers and compares.
fn run_case(out_dir: &Path, i: usize, case: Case, keep: bool) -> Outcome {
    let dir = out_dir.join(format!("case{i:05}"));
    let _ = std::fs::create_dir_all(&dir);
    let (h, p, input) = (
        dir.join("harness.wav"),
        dir.join("port.wav"),
        dir.join("input.wav"),
    );
    let mut outcome = Outcome {
        case: case.clone(),
        max: 0.0,
        first: None,
        error: None,
    };
    let mut hargs = case.flags.clone();
    let mut pargs = case.flags.clone();
    if case.input == "noise" {
        let noise = noise_path(out_dir, case.sr);
        hargs.extend(["--input".into(), noise.display().to_string()]);
        pargs.extend(["--input".into(), noise.display().to_string()]);
    } else {
        hargs.extend([
            "--gen".into(),
            case.input.clone(),
            "--dump-input".into(),
            input.display().to_string(),
        ]);
        pargs.extend(["--input".into(), input.display().to_string()]);
    }
    hargs.extend(["--output".into(), h.display().to_string()]);
    pargs.extend(["--output".into(), p.display().to_string()]);
    let run = |bin: &str, args: &[String]| -> Result<(), String> {
        let mut tries = 0;
        loop {
            if bin == HARNESS {
                launch_gate();
            }
            let out = Command::new(bin)
                .args(args)
                .output()
                .map_err(|e| e.to_string())?;
            if out.status.success() {
                return Ok(());
            }
            tries += 1;
            // The harness crashes when two instances start at the same moment; retry those.
            if bin != HARNESS || out.status.code().is_some() || tries == 5 {
                let err = String::from_utf8_lossy(&out.stderr);
                let last = err
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("");
                return Err(format!("{bin} failed ({}): {last}", out.status));
            }
        }
    };
    let result = run(HARNESS, &hargs)
        .and_then(|_| run(RENDER, &pargs))
        .and_then(|_| {
            let a = wav::read(&h.display().to_string(), case.sr)?;
            let b = wav::read(&p.display().to_string(), case.sr)?;
            if a.len() != b.len() {
                return Err(format!("length {} != {}", a.len(), b.len()));
            }
            for (k, (x, y)) in a.iter().zip(&b).enumerate() {
                for ch in 0..2 {
                    if x[ch].to_bits() != y[ch].to_bits() {
                        let d = (x[ch] - y[ch]).abs();
                        outcome.max = outcome.max.max(if d.is_nan() { f32::INFINITY } else { d });
                        outcome.first.get_or_insert(k);
                    }
                }
            }
            Ok(())
        });
    if let Err(e) = result {
        outcome.error = Some(e);
    }
    if outcome.passed() && !keep {
        let _ = std::fs::remove_dir_all(&dir);
    } else {
        let mut cmd = format!("{HARNESS} {}\n{RENDER} {}\n", quote(&hargs), quote(&pargs));
        cmd += &format!("# {}\n", case.name);
        let _ = std::fs::write(dir.join("commands.sh"), cmd);
    }
    outcome
}

/// Spaces harness launches at least 60 ms apart: instances that start together crash.
fn launch_gate() {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if let Some(t) = *last {
        let gap = std::time::Duration::from_millis(60);
        let since = t.elapsed();
        if since < gap {
            std::thread::sleep(gap - since);
        }
    }
    *last = Some(std::time::Instant::now());
}

fn quote(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') {
                format!("'{a}'")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn noise_path(out_dir: &Path, sr: u32) -> PathBuf {
    out_dir.join(format!("noise_{sr}.wav"))
}

/// Writes 4 s of white noise from a fixed-seed LCG, different on L and R.
fn write_noise(out_dir: &Path, sr: u32) -> Result<(), String> {
    let mut rng = Rng(0x5eed_1234);
    let frames = sr as usize * 4;
    let buf: Vec<[f32; 2]> = (0..frames)
        .map(|_| [rng.unit() * 0.5, rng.unit() * 0.5])
        .collect();
    wav::write(&noise_path(out_dir, sr).display().to_string(), &buf, sr)
}

/// A small deterministic generator for noise and random slot pairs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    /// Uniform in [-1, 1).
    fn unit(&mut self) -> f32 {
        (self.next() as f64 / (1u64 << 31) as f64 * 2.0 - 1.0) as f32
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Named slots of a preset: (time indexes, volume indexes).
fn named_slots(path: &Path) -> Result<(Vec<usize>, Vec<usize>), String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut e = Engine::new();
    e.load_state(fst_state(&data).map_err(|e| e.to_string())?);
    let named = |b: usize| {
        (0..36)
            .filter(|&i| !e.banks[b].slots[i].name.is_empty())
            .collect()
    };
    Ok((named(0), named(1)))
}

/// Writes a v9 state blob of a preset with some option bytes changed (offsets 44..48 of a
/// version 0x90003 state: Remove DC, attack compensation, HQ, scratch clock view, click).
fn options_state(out_dir: &Path, preset: &str, opts: [Option<u8>; 5]) -> Result<PathBuf, String> {
    let data = std::fs::read(Path::new(PRESET_DIR).join(format!("{preset}.fst")))
        .map_err(|e| e.to_string())?;
    let mut e = Engine::new();
    e.load_state(fst_state(&data).map_err(|e| e.to_string())?);
    let mut blob = e.save_state();
    let mut tag = String::new();
    for (k, o) in opts.iter().enumerate() {
        if let Some(v) = o {
            blob[44 + k] = *v;
            tag += &format!("_{k}{v}");
        }
    }
    let path = out_dir.join(format!("state_{}{tag}.bin", preset.replace(' ', "_")));
    std::fs::write(&path, blob).map_err(|e| e.to_string())?;
    Ok(path)
}

fn s(x: impl ToString) -> String {
    x.to_string()
}

/// Common flags of a render.
fn base(sr: u32, tempo: f64, block: usize, seconds: f64, state: &Path) -> Vec<String> {
    vec![
        s("--sr"),
        s(sr),
        s("--tempo"),
        s(tempo),
        s("--block"),
        s(block),
        s("--seconds"),
        s(seconds),
        s("--state"),
        state.display().to_string(),
    ]
}

fn param(flags: &mut Vec<String>, i: usize, v: i64) {
    flags.extend([s("--param"), format!("{i}={v}")]);
}

/// Builds the full matrix of `PORT_PROMPT.md`.
fn matrix(out_dir: &Path) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    let mut rng = Rng(20261024);
    let preset_path = |p: &str| Path::new(PRESET_DIR).join(format!("{p}.fst"));
    let push = |cases: &mut Vec<Case>,
                group,
                name: String,
                flags: Vec<String>,
                sr: u32,
                inputs: &[&str]| {
        for input in inputs {
            cases.push(Case {
                group,
                name: format!("{name} in={input}"),
                flags: flags.clone(),
                input: s(input),
                sr,
            });
        }
    };

    // Every named slot of every preset, and 10 random (time, volume) pairs per preset.
    let mut pairs: Vec<(&str, i64, i64)> = Vec::new();
    for preset in PRESETS {
        let path = preset_path(preset);
        let (ts, vs) = named_slots(&path)?;
        for t in ts {
            let mut f = base(48000, 120.0, 512, 4.0, &path);
            param(&mut f, 0, t as i64);
            param(&mut f, 1, 0);
            push(
                &mut cases,
                "presets",
                format!("{preset} t={t} v=0"),
                f,
                48000,
                &INPUTS,
            );
        }
        for v in vs {
            let mut f = base(48000, 120.0, 512, 4.0, &path);
            param(&mut f, 0, 0);
            param(&mut f, 1, v as i64);
            push(
                &mut cases,
                "presets",
                format!("{preset} t=0 v={v}"),
                f,
                48000,
                &INPUTS,
            );
        }
        for _ in 0..10 {
            let (t, v) = (rng.below(36) as i64, rng.below(36) as i64);
            pairs.push((preset, t, v));
            let mut f = base(48000, 120.0, 512, 4.0, &path);
            param(&mut f, 0, t);
            param(&mut f, 1, v);
            push(
                &mut cases,
                "presets",
                format!("{preset} t={t} v={v} pair"),
                f,
                48000,
                &INPUTS,
            );
        }
    }

    // Sample rates, tempos and block sizes: two random pairs per preset.
    for sr in [44100, 48000, 96000] {
        for tempo in [90.0, 120.0, 174.0] {
            for block in [512, 64, 333] {
                for k in 0..PRESETS.len() {
                    for j in 0..2 {
                        let (preset, t, v) = pairs[k * 10 + j + (sr as usize + block) % 7];
                        let mut f = base(sr, tempo, block, 3.0, &preset_path(preset));
                        param(&mut f, 0, t);
                        param(&mut f, 1, v);
                        push(
                            &mut cases,
                            "configs",
                            format!("{preset} t={t} v={v} sr={sr} bpm={tempo} blk={block}"),
                            f,
                            sr,
                            &["ramp", "noise"],
                        );
                    }
                }
            }
        }
    }

    // Options on and off on top of a few presets.
    let opt_presets = [
        "Repeater",
        "Turntablist",
        "Stutter",
        "Pitch shifter",
        "Juggling Science",
    ];
    let variants: [(&str, [Option<u8>; 5]); 9] = [
        ("hq0", [None, None, Some(0), None, None]),
        ("hq1", [None, None, Some(1), None, None]),
        ("click0", [None, None, None, None, Some(0)]),
        ("click1", [None, None, None, None, Some(1)]),
        ("click2", [None, None, None, None, Some(2)]),
        ("hq0_click2", [None, None, Some(0), None, Some(2)]),
        ("attcomp0", [None, Some(0), None, None, None]),
        ("attcomp1", [None, Some(1), None, None, None]),
        ("dc_flip", [Some(2), None, None, None, None]),
    ];
    for preset in opt_presets {
        let k = PRESETS.iter().position(|p| *p == preset).unwrap();
        for (tag, opts) in variants {
            let mut opts = opts;
            let remove_dc = matches!(preset, "Pitch shifter" | "Juggling Science");
            if tag == "dc_flip" {
                opts[0] = Some(!remove_dc as u8);
            }
            let state = options_state(out_dir, preset, opts)?;
            for j in 0..3 {
                let (_, t, v) = pairs[k * 10 + j];
                for pm in [None, Some(2)] {
                    let mut f = base(48000, 120.0, 512, 4.0, &state);
                    param(&mut f, 0, t);
                    param(&mut f, 1, v);
                    if let Some(pm) = pm {
                        f.extend([s("--process-mode"), s(pm)]);
                    }
                    let pm_tag = pm.map_or(String::new(), |p| format!(" pm={p}"));
                    push(
                        &mut cases,
                        "options",
                        format!("{preset} {tag} t={t} v={v}{pm_tag}"),
                        f,
                        48000,
                        &["ramp", "noise", "sine:1000"],
                    );
                }
            }
        }
    }

    // Parameter sweeps: min, default, max and two values in between.
    let sweeps: [(usize, [i64; 5]); 7] = [
        (2, [0, 256, 256, 64, 200]),
        (3, [0, 256, 256, 77, 190]),
        (4, [0, 1765, 10000, 600, 5000]),
        (5, [0, 5000, 10000, 1500, 8000]),
        (6, [-256, 0, 256, -100, 150]),
        (7, [0, 0, 1, 0, 1]),
        (8, [0, 65536, 65536, 20000, 50000]),
    ];
    for preset in ["Repeater", "Turntablist", "Stutter", "Patterns"] {
        let k = PRESETS.iter().position(|p| *p == preset).unwrap();
        for (p, values) in sweeps {
            let mut seen = Vec::new();
            for v in values {
                if seen.contains(&v) {
                    continue;
                }
                seen.push(v);
                for j in 0..2 {
                    let (_, t, vs) = pairs[k * 10 + j];
                    let mut f = base(48000, 120.0, 512, 5.0, &preset_path(preset));
                    param(&mut f, 0, t);
                    param(&mut f, 1, vs);
                    param(&mut f, p, v);
                    push(
                        &mut cases,
                        "sweeps",
                        format!("{preset} t={t} v={vs} p{p}={v}"),
                        f,
                        48000,
                        &["ramp", "noise"],
                    );
                }
            }
        }
    }

    // Song position.
    for preset in ["Repeater", "Momentary", "Turntablist", "Pitch shifter"] {
        let k = PRESETS.iter().position(|p| *p == preset).unwrap();
        for pos in [0.0, 1.0, 2.5] {
            for j in 0..3 {
                let (_, t, v) = pairs[k * 10 + j];
                let mut f = base(48000, 120.0, 512, 4.0, &preset_path(preset));
                f.extend([s("--pos"), s(pos)]);
                param(&mut f, 0, t);
                param(&mut f, 1, v);
                push(
                    &mut cases,
                    "pos",
                    format!("{preset} t={t} v={v} pos={pos}"),
                    f,
                    48000,
                    &["ramp", "noise"],
                );
            }
        }
    }

    midi_cases(&mut cases, &preset_path);
    Ok(cases)
}

/// MIDI slot switching (gate 6): note-driven switches, key-held return, nested key-held notes
/// and a repeated note on the current slot.
fn midi_cases(cases: &mut Vec<Case>, preset_path: &dyn Fn(&str) -> PathBuf) {
    let note = |f: &mut Vec<String>, n: &str| f.extend([s("--note"), s(n)]);
    let sets: [(&str, &[&str]); 8] = [
        ("Momentary", &["2b:61:on", "6b:61:off"]),
        ("Momentary", &["2b:61:on", "4b:61:on", "6b:61:off"]),
        (
            "Momentary",
            &["1b:62:on", "2.5b:65:on", "4b:65:off", "5.5b:62:off"],
        ),
        (
            "Momentary",
            &["1.1b:30:on", "3b:30:off", "1.7b:70:on", "5b:70:off"],
        ),
        (
            "Turntablist",
            &[
                "1b:63:on",
                "3.3b:63:off",
                "4b:70:on",
                "4.2b:28:on",
                "7b:70:off",
            ],
        ),
        (
            "Repeater",
            &[
                "1.1b:61:on",
                "2b:61:off",
                "2.5b:80:on",
                "3b:26:on",
                "5b:80:off",
            ],
        ),
        (
            "Stutter",
            &[
                "0.5b:62:on",
                "1.6b:62:off",
                "2b:65:on",
                "3b:66:on",
                "6b:66:off",
            ],
        ),
        (
            "Juggling Science",
            &[
                "1b:61:on",
                "2b:62:on",
                "3b:61:off",
                "4b:62:off",
                "4.5b:25:on",
            ],
        ),
    ];
    for (k, (preset, notes)) in sets.iter().enumerate() {
        for (sr, block) in [(48000, 512), (44100, 333), (48000, 64)] {
            let mut f = base(sr, 120.0, block, 4.5, &preset_path(preset));
            for n in *notes {
                note(&mut f, n);
            }
            for input in ["ramp", "noise"] {
                cases.push(Case {
                    group: "midi",
                    name: format!("{preset} set{k} sr={sr} blk={block} in={input}"),
                    flags: f.clone(),
                    input: s(input),
                    sr,
                });
            }
        }
    }
}
