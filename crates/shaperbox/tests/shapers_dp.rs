//! Dynamics, the section envelope follower and Pitch against the exact Python models in
//! `.amp/in/sb/data` (`sb_dynamics_exact.py`, `sb_envfollow_exact.py`, `sb_pitch_exact.py`).
//!
//! The reference vectors in `tests/data/*.bin` (zlib) come from a generator that runs those models
//! on the signals below; the committed files keep every 2nd (Dynamics, follower) or 3rd (Pitch)
//! sample of one channel. Set `SHAPERBOX_DP_DATA` to a directory of full vectors (every sample,
//! every channel) to compare all of them. Run with `--nocapture` to see the error per case.

use std::collections::HashMap;

use backspin_shaperbox::ftz::FlushToZero;
use backspin_shaperbox::shapers::{
    Wave,
    dynamics::{DynamicsBand, makeup_gain},
    envfollow::{
        EnvParams, EnvShaper, EnvSources, SectionFollower, SlidingMax, ratio_from_amount,
        threshold_cu,
    },
    pitch::{PitchBand, PitchParams, latency, ratio_from_wave},
    reverb::send_mix,
};

const SR: f32 = 48000.0;

/// One reference case: kept channels, every `stride`-th sample of `n`.
struct Case {
    chans: Vec<Vec<f32>>,
    stride: usize,
    n: usize,
}

fn load(file: &str) -> HashMap<String, Case> {
    let dir = std::env::var("SHAPERBOX_DP_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data").to_string());
    let raw = std::fs::read(format!("{dir}/{file}")).unwrap();
    let data = miniz_oxide::inflate::decompress_to_vec_zlib(&raw).unwrap();
    let mut p = 0;
    let mut take = |k: usize| {
        let s = &data[p..p + k];
        p += k;
        s
    };
    let u32_at = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap()) as usize;
    let count = u32_at(take(4));
    let mut out = HashMap::new();
    for _ in 0..count {
        let len = take(1)[0] as usize;
        let name = String::from_utf8(take(len).to_vec()).unwrap();
        let nch = take(1)[0] as usize;
        let stride = take(1)[0] as usize;
        let n = u32_at(take(4));
        let m = n.div_ceil(stride);
        let chans = (0..nch)
            .map(|_| {
                take(4 * m)
                    .chunks(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                    .collect()
            })
            .collect();
        out.insert(name, Case { chans, stride, n });
    }
    out
}

/// The maximum error of `got` (per channel) against a reference case.
fn max_err(case: &Case, got: &[&[f32]]) -> f32 {
    let mut e = 0.0f32;
    for (c, want) in case.chans.iter().enumerate() {
        for (j, w) in want.iter().enumerate() {
            let g = got[c][j * case.stride];
            e = e.max(if g == *w { 0.0 } else { (g - w).abs() });
        }
    }
    e
}

// ---- signals: f32 basic ops only, mirrored in the generator -------------------------------------

fn noise(n: usize, seed: u64, amp: impl Fn(usize) -> f64) -> Vec<f32> {
    let mut x = seed;
    (0..n)
        .map(|i| {
            x = (x * 1_103_515_245 + 12345) & 0x7fff_ffff;
            let u = (x as f32) * f32::from_bits(0x3000_0000);
            let v = u * 2.0 - 1.0;
            (v as f64 * amp(i)) as f32
        })
        .collect()
}

fn osc(n: usize, c: f32, y1: f32, start: usize, env: Option<(usize, f32)>) -> Vec<f32> {
    let c2 = 2.0 * c;
    let mut out = vec![0.0; n];
    let (mut ya, mut yb, mut e) = (0.0f32, y1, 1.0f32);
    for (i, o) in out.iter_mut().enumerate().skip(start) {
        if let Some((period, _)) = env
            && (i - start).is_multiple_of(period)
        {
            (ya, yb, e) = (0.0, y1, 1.0);
        }
        let y = c2 * yb - ya;
        (ya, yb) = (yb, y);
        *o = match env {
            Some((_, decay)) => {
                e *= decay;
                y * e
            }
            None => y,
        };
    }
    out
}

fn bursts(n: usize, seed: u64) -> Vec<f32> {
    noise(n, seed, |i| if (i / 300) % 2 == 0 { 0.8 } else { 0.02 })
}

fn drums(n: usize) -> Vec<f32> {
    let tone = osc(n, 0.9986, 0.05, 50, Some((700, 0.9985)));
    let nz = noise(n, 7, |_| 0.5);
    (0..n)
        .map(|i| {
            if i >= 50 && (i - 50) % 700 < 64 {
                tone[i] + nz[i]
            } else {
                tone[i]
            }
        })
        .collect()
}

fn signal(name: &str, n: usize) -> Vec<[f32; 2]> {
    let (l, r) = match name {
        "bursts" => (bursts(n, 1), bursts(n, 2)),
        "drums" => (drums(n), drums(n)),
        "sine" => {
            let s = osc(n, 0.99144, 0.065, 200, None);
            (s.clone(), s)
        }
        "harmonic" => {
            let (a, b, c) = (
                osc(n, 0.99979, 0.004, 0, None),
                osc(n, 0.99916, 0.006, 0, None),
                osc(n, 0.99811, 0.005, 0, None),
            );
            let h: Vec<f32> = (0..n).map(|i| a[i] + b[i] + c[i]).collect();
            let r = h.iter().map(|v| v * 0.75).collect();
            (h, r)
        }
        "clicks" => {
            let mut x = vec![0.0f32; n];
            for s in (300..n).step_by(1500) {
                for k in 0..3 {
                    if s + k < n {
                        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                        x[s + k] = (0.8 * (1.0 - k as f64 / 3.0) * sign) as f32;
                    }
                }
            }
            (x.clone(), x)
        }
        "noise" => (noise(n, 11, |_| 0.5), noise(n, 12, |_| 0.5)),
        _ => unreachable!("{name}"),
    };
    l.into_iter().zip(r).map(|(a, b)| [a, b]).collect()
}

fn channels(x: &[[f32; 2]]) -> [Vec<f32>; 2] {
    [
        x.iter().map(|f| f[0]).collect(),
        x.iter().map(|f| f[1]).collect(),
    ]
}

// ---- Dynamics ----------------------------------------------------------------------------------

fn dyn_base() -> EnvParams {
    EnvParams {
        active: true,
        threshold: 0.5,
        amount: 0.0,
        attack: 0.0,
        release: 0.0,
        knee: 0.0,
        makeup: 0.5,
        ..EnvParams::default()
    }
}

/// The cases of the generator: name, input, sidechain (`Some("")` for a missing bus), settings.
fn dyn_cases() -> Vec<(&'static str, &'static str, Option<&'static str>, EnvParams)> {
    let b = dyn_base();
    vec![
        ("sine defaults", "sine", None, b),
        (
            "bursts attack .3 release .5",
            "bursts",
            None,
            EnvParams {
                attack: 0.3,
                release: 0.5,
                ..b
            },
        ),
        (
            "drums hold .3 autoRelease",
            "drums",
            None,
            EnvParams {
                attack: 0.05,
                release: 0.3,
                hold: 0.3,
                auto_release: true,
                ..b
            },
        ),
        (
            "sine thr .8 amount .9 makeup .8",
            "sine",
            None,
            EnvParams {
                threshold: 0.8,
                amount: 0.9,
                makeup: 0.8,
                ..b
            },
        ),
        (
            "drums autoMakeup",
            "drums",
            None,
            EnvParams {
                auto_makeup: true,
                amount: 0.3,
                attack: 0.4,
                makeup: 0.6,
                ..b
            },
        ),
        (
            "bursts inert knee offset depth mode",
            "bursts",
            None,
            EnvParams {
                knee: 1.0,
                offset: 0.0,
                depth: 2,
                mode: 1,
                attack: 0.1,
                release: 0.3,
                ..b
            },
        ),
        (
            "drums free hp .5 lp .7",
            "drums",
            None,
            EnvParams {
                attack: 0.1,
                release: 0.3,
                filter_setup_is_free: true,
                hp_cutoff: 0.5,
                lp_cutoff: 0.7,
                ..b
            },
        ),
        (
            "bursts sidechain drums",
            "bursts",
            Some("drums"),
            EnvParams {
                attack: 0.1,
                release: 0.3,
                use_sidechain: true,
                auto_makeup: true,
                ..b
            },
        ),
        (
            "bursts sidechain without bus",
            "bursts",
            Some(""),
            EnvParams {
                attack: 0.1,
                release: 0.3,
                use_sidechain: true,
                ..b
            },
        ),
        (
            "bursts hold .2",
            "bursts",
            None,
            EnvParams {
                attack: 0.1,
                release: 0.3,
                hold: 0.2,
                ..b
            },
        ),
        (
            "bursts free hp .3",
            "bursts",
            None,
            EnvParams {
                attack: 0.2,
                release: 0.1,
                filter_setup_is_free: true,
                hp_cutoff: 0.3,
                ..b
            },
        ),
        (
            "bursts free sidechain drums",
            "bursts",
            Some("drums"),
            EnvParams {
                attack: 0.1,
                release: 0.3,
                use_sidechain: true,
                filter_setup_is_free: true,
                lp_cutoff: 0.6,
                ..b
            },
        ),
    ]
}

/// The gain reduction against the model, and the output against `x 2^(-GR/6) makeup` with the
/// model's GR.
#[test]
fn dynamics_matches_the_model() {
    let _ftz = FlushToZero::new();
    let refs = load("dynamics.bin");
    let mut worst = 0.0f32;
    for (name, sig, sc, p) in dyn_cases() {
        let case = &refs[name];
        let n = case.n;
        let x = signal(sig, n);
        let sc = sc.and_then(|s| (!s.is_empty()).then(|| signal(s, n)));
        let mut band = DynamicsBand::new(SR, 512, &p);
        let mut out = x.clone();
        let mut gr = Vec::new();
        for a in (0..n).step_by(512) {
            let b = n.min(a + 512);
            let src = EnvSources {
                full: &x[a..b],
                sidechain_band: sc.as_ref().map(|s| &s[a..b]),
                sidechain_full: sc.as_ref().map(|s| &s[a..b]),
            };
            band.process(&mut out[a..b], &p, &src);
            gr.extend_from_slice(&band.gain_reduction()[..b - a]);
        }
        let e_gr = max_err(case, &[&gr]);
        let auto = p.auto_makeup && !p.use_sidechain;
        let (thr, ratio) = (threshold_cu(p.threshold), ratio_from_amount(p.amount));
        let mk = makeup_gain(p.makeup, auto, thr, ratio, p.attack);
        let mut e_out = 0.0f32;
        for (j, g) in case.chans[0].iter().enumerate() {
            let i = j * case.stride;
            let sec = (g * -0.166_666_67).exp2();
            for c in 0..2 {
                e_out = e_out.max((out[i][c] - x[i][c] * sec * mk).abs());
            }
        }
        eprintln!("dynamics {name:40} GR max err {e_gr:.3e}  output max err {e_out:.3e}");
        worst = worst.max(e_gr).max(e_out);
    }
    assert!(worst <= 1e-5, "worst {worst}");
}

#[test]
fn dynamics_inactive_passes_and_chunks() {
    let x = signal("bursts", 1500);
    let p = EnvParams {
        active: false,
        ..dyn_base()
    };
    let mut band = DynamicsBand::new(SR, 256, &p);
    let mut buf = x.clone();
    band.process(&mut buf, &p, &EnvSources::new(&x));
    assert_eq!(buf, x);
    assert_eq!(band.latency(), 0);

    // One long call chunks internally like separate blocks.
    let p = EnvParams {
        attack: 0.2,
        release: 0.4,
        hold: 0.1,
        ..dyn_base()
    };
    let (mut a, mut b) = (x.clone(), x.clone());
    let mut one = DynamicsBand::new(SR, 256, &p);
    one.process(&mut a, &p, &EnvSources::new(&x));
    let mut many = DynamicsBand::new(SR, 256, &p);
    for k in (0..x.len()).step_by(100) {
        let e = x.len().min(k + 100);
        many.process(&mut b[k..e], &p, &EnvSources::new(&x[k..e]));
    }
    assert_eq!(a, b);
}

#[test]
fn sliding_max_is_the_plain_maximum() {
    let mut sm = SlidingMax::new(300);
    let v = noise(5000, 3, |_| 1.0);
    let mut hist = vec![0.0f32; 1000];
    for (i, x) in v.iter().enumerate() {
        let n = [1, 7, 250, 600][(i / 900) % 4];
        hist.push(x.max(0.0));
        let want = hist[hist.len() - n..]
            .iter()
            .fold(f32::MIN, |m, v| m.max(*v));
        assert_eq!(sm.push_max(*x, n), want, "sample {i} window {n}");
    }
}

// ---- section envelope follower -----------------------------------------------------------------

fn ef_cases() -> Vec<(&'static str, &'static str, EnvShaper, f32, EnvParams)> {
    let b = EnvParams {
        active: true,
        ..EnvParams::default()
    };
    use EnvShaper::*;
    vec![
        ("mode 0 defaults", "bursts", Default, 0.5, b),
        (
            "mode 0 amount .8 depth 1 offset .3",
            "bursts",
            Default,
            0.5,
            EnvParams {
                amount: 0.8,
                depth: 1,
                offset: 0.3,
                ..b
            },
        ),
        (
            "mode 1 amount .3 depth 2",
            "drums",
            Default,
            0.25,
            EnvParams {
                mode: 1,
                amount: 0.3,
                depth: 2,
                ..b
            },
        ),
        (
            "mode 0 threshold .3 hold .4",
            "drums",
            Default,
            0.5,
            EnvParams {
                threshold: 0.3,
                hold: 0.4,
                amount: 0.9,
                ..b
            },
        ),
        (
            "mode 0 autoRelease",
            "drums",
            Default,
            0.5,
            EnvParams {
                auto_release: true,
                attack: 0.3,
                release: 0.5,
                amount: 0.9,
                ..b
            },
        ),
        (
            "mode 1 autoRelease hold .3",
            "bursts",
            Default,
            0.65,
            EnvParams {
                mode: 1,
                auto_release: true,
                hold: 0.3,
                release: 0.1,
                amount: 0.7,
                ..b
            },
        ),
        (
            "mode 0 free hp .4 lp .8",
            "drums",
            Default,
            0.5,
            EnvParams {
                filter_setup_is_free: true,
                hp_cutoff: 0.4,
                lp_cutoff: 0.8,
                amount: 0.9,
                ..b
            },
        ),
        (
            "reverb mode 0",
            "bursts",
            Reverb,
            0.8,
            EnvParams {
                amount: 0.3,
                depth: 1,
                ..b
            },
        ),
        (
            "pitch mode 1",
            "drums",
            Pitch,
            0.7,
            EnvParams {
                mode: 1,
                amount: 0.6,
                offset: 0.6,
                ..b
            },
        ),
        (
            "noise depth 2",
            "bursts",
            Noise,
            0.4,
            EnvParams {
                amount: 0.9,
                depth: 2,
                ..b
            },
        ),
    ]
}

#[test]
fn section_follower_matches_the_model() {
    let _ftz = FlushToZero::new();
    let refs = load("envfollow.bin");
    let mut worst = 0.0f32;
    for (name, sig, kind, w, p) in ef_cases() {
        let case = &refs[name];
        let x = signal(sig, case.n);
        let mut sf = SectionFollower::new(SR, 512, kind, &p);
        let mut port = Vec::new();
        for a in (0..case.n).step_by(512) {
            let b = case.n.min(a + 512);
            let src = EnvSources::new(&x[a..b]);
            let out = sf.process(Wave::Constant(w), &x[a..b], &src, &p);
            port.extend((0..b - a).map(|i| out.at(i)));
        }
        let e = max_err(case, &[&port]);
        eprintln!("envfollow {name:40} port max err {e:.3e}");
        worst = worst.max(e);
    }
    assert!(worst <= 1e-6, "worst {worst}");

    // Inactive: the port passes through.
    let p = EnvParams::default();
    let x = signal("bursts", 64);
    let mut sf = SectionFollower::new(SR, 64, EnvShaper::Default, &p);
    let out = sf.process(Wave::Constant(0.3), &x, &EnvSources::new(&x), &p);
    assert!(out.is_constant(0.3));
}

// ---- Pitch -------------------------------------------------------------------------------------

/// name, mode, input, pitch w (`None`: the ramp), formant w, linkFormant, fillGaps,
/// grainSizeCyclic, whether the case runs the pitch detector on voiced material.
type PitchCase = (
    &'static str,
    i32,
    &'static str,
    Option<f32>,
    f32,
    bool,
    f32,
    f32,
    bool,
);

const PITCH_CASES: [PitchCase; 14] = [
    (
        "cyclic noise ratio 2 gc .5",
        3,
        "noise",
        Some(0.75),
        0.5,
        true,
        1.0,
        0.5,
        false,
    ),
    (
        "cyclic noise ratio .5 gc .25",
        3,
        "noise",
        Some(0.25),
        0.5,
        true,
        1.0,
        0.25,
        false,
    ),
    (
        "cyclic sine ratio ramp",
        3,
        "sine",
        None,
        0.5,
        true,
        1.0,
        0.1,
        false,
    ),
    (
        "psola vocals noise ratio 1",
        1,
        "noise",
        Some(0.5),
        0.5,
        true,
        1.0,
        0.5,
        false,
    ),
    (
        "psola vocals noise ratio 2",
        1,
        "noise",
        Some(0.75),
        0.5,
        true,
        1.0,
        0.5,
        false,
    ),
    (
        "psola vocals noise ratio .5 fill 0",
        1,
        "noise",
        Some(0.25),
        0.5,
        true,
        0.0,
        0.5,
        false,
    ),
    (
        "psola vocals noise ratio 2 formant unlinked",
        1,
        "noise",
        Some(0.75),
        0.5,
        false,
        1.0,
        0.5,
        false,
    ),
    (
        "psola vocals noise ratio .5 formant 2 unlinked",
        1,
        "noise",
        Some(0.25),
        0.75,
        false,
        0.5,
        0.5,
        false,
    ),
    (
        "psola vocals sine ratio 2 (detector)",
        1,
        "sine",
        Some(0.75),
        0.5,
        true,
        1.0,
        0.5,
        true,
    ),
    (
        "psola vocals harmonic ratio .5 formant unlinked (detector)",
        1,
        "harmonic",
        Some(0.25),
        0.625,
        false,
        1.0,
        0.5,
        true,
    ),
    (
        "psola drums noise ratio 1",
        2,
        "noise",
        Some(0.5),
        0.5,
        true,
        1.0,
        0.5,
        false,
    ),
    (
        "psola drums clicks ratio 2",
        2,
        "clicks",
        Some(0.75),
        0.5,
        true,
        1.0,
        0.5,
        false,
    ),
    (
        "psola drums clicks ratio .5 unlinked",
        2,
        "clicks",
        Some(0.25),
        0.5,
        false,
        1.0,
        0.5,
        false,
    ),
    (
        "psola drums drums ratio 2 (detector)",
        2,
        "drums",
        Some(0.75),
        0.5,
        true,
        1.0,
        0.5,
        true,
    ),
];

/// WSOLA (mode 0) cases: name, input, pitch wave (`None`: the ramp), grainSizeWsola. The
/// generator renders twice the length of the other Pitch cases.
const WSOLA_CASES: [(&str, &str, Option<f32>, i32); 7] = [
    ("wsola noise ratio 2 grain 0", "noise", Some(0.75), 0),
    ("wsola noise ratio .5 grain 2", "noise", Some(0.25), 2),
    ("wsola sine ratio sqrt2 grain 1", "sine", Some(0.625), 1),
    ("wsola drums ratio 2 (transients)", "drums", Some(0.75), 0),
    ("wsola clicks ratio 1", "clicks", Some(0.5), 0),
    ("wsola clicks ratio .5 grain 1", "clicks", Some(0.25), 1),
    ("wsola sine ratio ramp", "sine", None, 0),
];

fn ramp(n: usize) -> Vec<f32> {
    let k = (1.0f64 / 9600.0) as f32;
    (0..n).map(|i| 0.25 + i as f32 * k).collect()
}

fn run_pitch(
    x: &[[f32; 2]],
    p: &PitchParams,
    pitch: Option<&[f32]>,
    wp: f32,
    wf: f32,
) -> Vec<[f32; 2]> {
    let mut band = PitchBand::new(SR, 512, p);
    let mut out = x.to_vec();
    for a in (0..x.len()).step_by(512) {
        let b = x.len().min(a + 512);
        let pw = pitch.map_or(Wave::Constant(wp), |r| Wave::PerSample(&r[a..b]));
        band.process(&mut out[a..b], pw, Wave::Constant(wf), p, None);
    }
    out
}

#[test]
fn pitch_matches_the_model() {
    let _ftz = FlushToZero::new();
    let refs = load("pitch.bin");
    let (mut worst, mut worst_detector) = (0.0f32, 0.0f32);
    for (name, mode, sig, wp, wf, link, fill, gc, detector) in PITCH_CASES {
        let case = &refs[name];
        let x = signal(sig, case.n);
        let p = PitchParams {
            mode,
            grain_size_cyclic: gc,
            link_formant: link,
            fill_gaps: fill,
            ..PitchParams::default()
        };
        let r = ramp(case.n);
        let out = run_pitch(
            &x,
            &p,
            wp.is_none().then_some(&r[..]),
            wp.unwrap_or(0.0),
            wf,
        );
        let [l, rr] = channels(&out);
        let e = max_err(case, &[&l, &rr]);
        eprintln!("pitch {name:58} max err {e:.3e}");
        if detector {
            worst_detector = worst_detector.max(e);
        } else {
            worst = worst.max(e);
        }
    }
    assert!(worst <= 1e-6, "worst {worst}");
    assert!(
        worst_detector.is_finite() && worst_detector <= 1.0,
        "detector {worst_detector}"
    );
}

#[test]
fn wsola_matches_the_model() {
    let _ftz = FlushToZero::new();
    let refs = load("pitch.bin");
    let mut worst = 0.0f32;
    for (name, sig, wp, gw) in WSOLA_CASES {
        let case = &refs[name];
        let x = signal(sig, case.n);
        let p = PitchParams {
            mode: 0,
            grain_size_wsola: gw,
            ..PitchParams::default()
        };
        let r = ramp(case.n);
        let out = run_pitch(
            &x,
            &p,
            wp.is_none().then_some(&r[..]),
            wp.unwrap_or(0.0),
            0.5,
        );
        let [l, rr] = channels(&out);
        let e = max_err(case, &[&l, &rr]);
        eprintln!("pitch {name:58} max err {e:.3e}");
        worst = worst.max(e);
    }
    assert!(worst <= 1e-6, "worst {worst}");
}

#[test]
fn pitch_latency_and_mix() {
    assert_eq!(latency(48000.0, 1), 2381);
    assert_eq!(latency(48000.0, 2), 2861);
    assert_eq!(latency(44100.0, 1), 2187);
    assert_eq!(latency(96000.0, 1), 4754);
    assert_eq!(latency(48000.0, 0), 1687);
    // The model formula; shapers_b.md 2.6 quotes 1635 (code-derived, inconsistent with it).
    assert_eq!(latency(44100.0, 0), 1551);
    assert_eq!(latency(48000.0, 3), 0);
    assert_eq!(ratio_from_wave(0.5), 1.0);
    assert_eq!(ratio_from_wave(1.0), 4.0);
    let p = |mode| PitchParams {
        mode,
        ..PitchParams::default()
    };

    // At ratio 1 the wet path is the input delayed by the latency, so mix 0.5 with the delayed
    // dry gives 2 x[n - lat].
    let n = 6000;
    let x = signal("noise", n);
    let mut band = PitchBand::new(SR, 512, &p(1));
    let lat = band.latency();
    let mut out = x.clone();
    for a in (0..n).step_by(512) {
        let b = n.min(a + 512);
        let mut dry = x[a..b].to_vec();
        band.process(
            &mut out[a..b],
            Wave::Constant(0.5),
            Wave::Constant(0.5),
            &p(1),
            None,
        );
        band.delay_dry(&mut dry);
        send_mix(&mut out[a..b], &dry, 0.5);
    }
    for i in lat + 600..n {
        for c in 0..2 {
            assert_eq!(out[i][c], 2.0 * x[i - lat][c], "sample {i}");
        }
    }
}
