//! The Delay and Limiter band DSP (shapers_c.md) and the pass-through of the Oscilloscope and
//! Shift shapers in the graph. The plugin never runs the Delay and Limiter, so these checks
//! are structural (echo timing, feedback decay, ping-pong, the ceiling and the latency); the
//! Python models in `.amp/in/sb/data/sb_delay_exact.py` and `sb_limiter_exact.py` cross-check
//! the arithmetic against `sbrender`.

use backspin_shaperbox::ftz::FlushToZero;
use backspin_shaperbox::graph::{BASE_DELAY, Processor, Transport};
use backspin_shaperbox::shapers::Wave;
use backspin_shaperbox::shapers::delay::{DelayBand, DelayParams, center_seconds};
use backspin_shaperbox::shapers::limiter::{LimiterBand, LimiterParams, lookahead_samples};
use backspin_shaperbox::smoother::Smoother;
use backspin_shaperbox::state::{Band, Lfo, Section, State};
use backspin_shaperbox::valuetree::Var;
use backspin_shaperbox::wave::Point;

const SR: f32 = 48000.0;

fn transport() -> Transport {
    Transport {
        ppq: 0.0,
        bpm: 120.0,
        playing: true,
    }
}

/// A section whose wave is the constant `y`.
fn section(id: &str, y: f32) -> Section {
    Section {
        id: id.into(),
        lfo: Lfo {
            points: vec![Point { x: 0, y, factor: 3 }],
            ..Lfo::default()
        },
        env: Default::default(),
    }
}

/// A single-shaper state: shaper `k` on with a full-range split, every band `mix` and the
/// same sections and values.
fn single(k: usize, mix: f32, sections: &[(&str, f32)], values: &[(&str, Var)]) -> State {
    let mut s = State::default();
    let sh = &mut s.shapers[k];
    sh.visible = true;
    sh.on = true;
    sh.bands = std::array::from_fn(|_| Band {
        mix,
        sections: sections.iter().map(|(id, y)| section(id, *y)).collect(),
        values: values
            .iter()
            .map(|(n, v)| ((*n).into(), v.clone()))
            .collect(),
    });
    s
}

fn impulse(n: usize, at: usize) -> Vec<[f32; 2]> {
    let mut buf = vec![[0.0f32; 2]; n];
    buf[at] = [1.0, 0.5];
    buf
}

fn run(s: State, buf: &mut [[f32; 2]]) {
    let mut p = Processor::new(s, SR);
    for chunk in buf.chunks_mut(512) {
        p.process(chunk, transport());
    }
}

/// Oscilloscope and Shift on: the output is the input after the base delay, as in the plugin.
#[test]
fn oscilloscope_and_shift_pass_through() {
    for (k, sections) in [
        (12, vec![("oscilloscope", 0.3)]),
        (13, vec![("shift", 0.8), ("feedback", 0.5)]),
    ] {
        let values = [("viewMode", Var::Int(2)), ("numStages", Var::Int(3))];
        let mut buf: Vec<[f32; 2]> = (0..4096)
            .map(|i| {
                let v = ((i * 7919) % 1000) as f32 / 1000.0 - 0.5;
                [v, -v * 0.5]
            })
            .collect();
        let input = buf.clone();
        run(single(k, 1.0, &sections, &values), &mut buf);
        for i in BASE_DELAY..buf.len() {
            assert_eq!(buf[i], input[i - BASE_DELAY], "shaper {k} sample {i}");
        }
    }
}

/// With the default band settings (3/16 bar, feedback 0.5, mix 0.25) an impulse comes back as
/// echoes 0.375 s apart at 120 bpm, each half the previous one, after the dry impulse; the
/// send-style mix keeps the dry signal whole and halves the wet one. Every tap passes the
/// feedback filter (a 21 kHz one-pole low-pass, then a 20.6 Hz high-pass), so an echo is a short
/// smear whose peak is `g = 0.84` of the tap.
#[test]
fn delay_echoes_at_the_centre_time() {
    let period = (SR * center_seconds(7, 120.0)) as usize;
    assert_eq!(period, 18000);
    let n = 4 * period + 1024;
    let mut buf = impulse(n, 100);
    run(
        single(
            6,
            0.25,
            &[("send", 1.0), ("volume", 1.0), ("time", 0.0)],
            &[],
        ),
        &mut buf,
    );
    let at = |i: usize| buf[100 + BASE_DELAY + i];
    assert_eq!(at(0), [1.0, 0.5]);
    let window = |k: usize| {
        (0..64)
            .map(|j| at(k * period + j))
            .fold([0.0f32; 2], |m, f| {
                [m[0].max(f[0].abs()), m[1].max(f[1].abs())]
            })
    };
    let mut prev = window(1);
    assert!(prev[0] > 0.35 && prev[0] < 0.5, "echo 1: {prev:?}");
    assert!((prev[1] / prev[0] - 0.5).abs() < 1e-5, "echo 1: {prev:?}");
    assert_eq!(at(period - 1), [0.0; 2]);
    assert!(
        at(period)[0] > 0.35,
        "the echo starts on the period: {:?}",
        at(period)
    );
    for k in 2..=3 {
        let got = window(k);
        let ratio = got[0] / prev[0];
        assert!(ratio > 0.4 && ratio < 0.55, "echo {k}: {got:?} vs {prev:?}");
        assert!(at(k * period - period / 2)[0].abs() < 5e-3);
        prev = got;
    }
}

/// Ping-pong feeds the mono sum into the right line and crosses the feedback: the first echo is
/// right only, the second left only. `stereoOffset` below 0.5 lengthens the left delay.
#[test]
fn delay_ping_pong_and_stereo_offset() {
    let mut band = DelayBand::new(SR, 512);
    let p = DelayParams {
        time_center: 2,
        modulation: 0.0,
        feedback: 0.5,
        ping_pong: true,
        stereo_offset: 0.5,
    };
    let period = (SR * center_seconds(2, 120.0)) as usize;
    assert_eq!(period, 3000);
    let lp = Smoother::new(1.0, SR as f64, 0.0);
    let hp = Smoother::new(0.0, SR as f64, 0.0);
    let mut buf = impulse(3 * period + 512, 7);
    for chunk in buf.chunks_mut(512) {
        band.process(
            chunk,
            Wave::Constant(1.0),
            Wave::Constant(1.0),
            Wave::Constant(0.5),
            &p,
            &lp,
            &hp,
            120.0,
        );
    }
    let first = buf[7 + period];
    let second = buf[7 + 2 * period];
    assert_eq!(first[0], 0.0);
    assert!(first[1] > 0.5 && first[1] < 0.75, "{first:?}");
    assert!(second[0] > 0.25 && second[0] < 0.375, "{second:?}");
    // Only the high-pass tail of the first echo remains on the right.
    assert!(second[1].abs() < 1e-3, "{second:?}");

    let mut band = DelayBand::new(SR, 512);
    let p = DelayParams {
        ping_pong: false,
        stereo_offset: 0.25,
        feedback: 0.0,
        ..p
    };
    let mut buf = impulse(2 * period + 512, 7);
    for chunk in buf.chunks_mut(512) {
        band.process(
            chunk,
            Wave::Constant(1.0),
            Wave::Constant(1.0),
            Wave::Constant(0.5),
            &p,
            &lp,
            &hp,
            120.0,
        );
    }
    // A = 0.5: the left tap sits at 1.5 periods, the right at one.
    assert_eq!(buf[7 + period][0], 0.0);
    assert!(buf[7 + period][1] > 0.35, "{:?}", buf[7 + period]);
    assert!(
        buf[7 + period * 3 / 2][0] > 0.7,
        "{:?}",
        buf[7 + period * 3 / 2]
    );
    assert!(buf[7 + period * 3 / 2][1].abs() < 1e-3);
}

/// A moving time wave reads the ring through the sinc head: the echo of an impulse spreads over
/// the 63 taps around the modulated delay instead of landing on one sample.
#[test]
fn delay_modulated_time_interpolates() {
    let mut band = DelayBand::new(SR, 512);
    let p = DelayParams {
        time_center: 2,
        modulation: 0.5,
        feedback: 0.0,
        ..DelayParams::default()
    };
    let lp = Smoother::new(1.0, SR as f64, 0.0);
    let hp = Smoother::new(0.0, SR as f64, 0.0);
    let n = 6000;
    let mut buf = impulse(n, 7);
    // The wave rises slowly: w = 0.5 + 2e-6 i over the render, a delay of 3000 (1 + 2e-6 i)
    // samples, so the echo lands between samples (at about 3025).
    let w: Vec<f32> = (0..n).map(|i| 0.5 + 2e-6 * i as f32).collect();
    for (k, chunk) in buf.chunks_mut(512).enumerate() {
        band.process(
            chunk,
            Wave::Constant(1.0),
            Wave::Constant(1.0),
            Wave::PerSample(&w[k * 512..k * 512 + chunk.len()]),
            &p,
            &lp,
            &hp,
            120.0,
        );
    }
    let peak = buf.iter().map(|f| f[0].abs()).fold(0.0f32, f32::max);
    let pos = buf.iter().position(|f| f[0].abs() == peak).unwrap();
    assert!(peak > 0.3 && peak < 1.0, "{peak}");
    assert!((3000..3060).contains(&pos), "{pos}");
    let spread = buf[pos - 32..pos + 32]
        .iter()
        .filter(|f| f[0].abs() > 0.05)
        .count();
    assert!(spread >= 3, "{spread}");
}

/// A 2.0 sine against a -6 dB threshold: after the lookahead the output never exceeds the
/// ceiling, and the gain reduction tracks the level; the dry path is delayed by the latency.
#[test]
fn limiter_holds_the_ceiling() {
    let _ftz = FlushToZero::new();
    let p = LimiterParams::default();
    let mut band = LimiterBand::new(SR, p);
    assert_eq!(band.latency(), lookahead_samples(0.5, SR) + 12);
    assert_eq!(band.latency(), 156);
    let n = 48000;
    let mut buf: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let v = 2.0 * (std::f32::consts::TAU * 1000.0 * i as f32 / SR).sin();
            [v, v * 0.5]
        })
        .collect();
    // Wave 0.25: the threshold is 2^(-45/6) = -45 units... use y = 0.8 -> w = 0.9 -> -6 "dB".
    for chunk in buf.chunks_mut(512) {
        band.process(chunk, Wave::Constant(0.9));
    }
    let tail = &buf[4800..];
    let peak = tail.iter().map(|f| f[0].abs()).fold(0.0f32, f32::max);
    assert!(peak <= 1.0 && peak > 0.9, "{peak}");
    // Below the threshold (after the 1 / threshold scaling: 0.1 * 2 = 0.2 peak) nothing changes
    // but the scaling and the delay.
    let mut band = LimiterBand::new(SR, p);
    let mut quiet: Vec<[f32; 2]> = buf.iter().map(|f| [f[0] * 0.05, f[1] * 0.05]).collect();
    let input = quiet.clone();
    for chunk in quiet.chunks_mut(512) {
        band.process(chunk, Wave::Constant(0.9));
    }
    let lat = band.latency();
    let scale = 1.0 / 2f32.powf((0.9f32 * 60.0 - 60.0) / 6.0);
    for i in 2000..3000 {
        let want = input[i - lat][0] * scale;
        assert!(
            (quiet[i][0] - want).abs() < 1e-5,
            "{i}: {} vs {want}",
            quiet[i][0]
        );
    }
    // In the graph the dry path is delayed by the same latency, so mix 0.5 adds them aligned.
    let mut g: Vec<[f32; 2]> = input.clone();
    run(single(14, 0.5, &[("limiter", 0.8)], &[]), &mut g);
    for i in 2000..3000 {
        let want = input[i - lat - BASE_DELAY][0] * (0.5 + 0.5 * scale);
        assert!((g[i][0] - want).abs() < 1e-5, "{i}: {} vs {want}", g[i][0]);
    }
}

/// Without lookahead the gain applies to the current sample and `transientBoost` 0 attacks
/// instantly; `autoRelease` runs its own release and stays within the ceiling too.
#[test]
fn limiter_variants_stay_finite_and_limited() {
    let _ftz = FlushToZero::new();
    for (lookahead, tb, auto) in [
        (0.0, 1, false),
        (0.5, 0, false),
        (0.5, 2, true),
        (1.0, 1, true),
    ] {
        let p = LimiterParams {
            lookahead,
            transient_boost: tb,
            auto_release: auto,
            ..LimiterParams::default()
        };
        let mut band = LimiterBand::new(SR, p);
        if lookahead == 0.0 {
            assert_eq!(band.latency(), 0);
        }
        let mut buf: Vec<[f32; 2]> = (0..24000)
            .map(|i| {
                let burst = if (i / 4000) % 2 == 0 { 3.0 } else { 0.1 };
                let v = burst * (std::f32::consts::TAU * 440.0 * i as f32 / SR).sin();
                [v, v]
            })
            .collect();
        for chunk in buf.chunks_mut(512) {
            band.process(chunk, Wave::Constant(1.0));
        }
        let peak = buf.iter().map(|f| f[0].abs()).fold(0.0f32, f32::max);
        assert!(
            peak <= 1.0 && buf.iter().all(|f| f[0].is_finite()),
            "{lookahead} {tb} {auto}: {peak}"
        );
        // The loud bursts are limited; the quiet parts pass at most unchanged and at least at
        // the gain the release has reached.
        assert!(buf[2000][0].abs() < 1.0);
        let quiet = buf[6000..7800]
            .iter()
            .map(|f| f[0].abs())
            .fold(0.0f32, f32::max);
        assert!(
            quiet > 0.05 && quiet <= 0.1 + 1e-4,
            "{lookahead} {tb} {auto}: {quiet}"
        );
    }
}
