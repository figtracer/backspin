//! The wave engine against Volume-shaper captures of the real plugin (`.amp/in/sb/captures/exp`,
//! outside the repository; skipped when absent): table, read, Beat and Hz timing, smoothing.

use backspin_shaperbox::ftz::FlushToZero;
use backspin_shaperbox::wave::{Point, Smoothing, Wave, length_beats, rate_hz, smoothing_coef};
use std::path::PathBuf;

fn read_wav(path: &PathBuf) -> Vec<f32> {
    let d = std::fs::read(path).unwrap();
    let i = d.windows(4).position(|w| w == b"data").unwrap();
    let n = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
    d[i + 8..i + 8 + n]
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .step_by(2)
        .collect()
}

/// Renders the Volume shaper with a wave the way the harness drives the plugin: 1 kHz sine at
/// 0.5, 48 kHz, 120 bpm, blocks of 512, the 16-sample base delay.
fn render(e: &serde_json::Value) -> Vec<f32> {
    let _ftz = FlushToZero::new();
    let (sr, bpm, n) = (48000.0f64, 120.0f64, 48000 * 4);
    let pts: Vec<Point> = e["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| Point {
            x: p[0].as_i64().unwrap() as i32,
            y: p[1].as_f64().unwrap() as f32,
            factor: p[2].as_i64().unwrap() as i32,
        })
        .collect();
    let wave = Wave::new(&pts, true);
    let beats = length_beats(e["length"].as_i64().unwrap_or(15) as i32);
    let hz = e["timingMode"].as_i64().unwrap_or(0) == 1;
    let rate = if hz {
        rate_hz(e["speed"].as_f64().unwrap_or(0.5) as f32)
    } else {
        bpm / (60.0 * beats)
    };
    let inc = rate / sr;
    let g = ((e["gain"].as_f64().unwrap_or(0.5) as f32 * 48.0 - 24.0) / 6.0).exp2();
    let mut sm = Smoothing::new(smoothing_coef(
        e["smoothing"].as_f64().unwrap_or(0.0) as f32,
        sr as f32,
    ));
    let mut out = vec![0.0f32; n];
    let mut phase = 0.0f64;
    for start in (0..n).step_by(512) {
        if !hz {
            let p = (start as f64 / sr * bpm / 60.0) / beats;
            phase = p - p.floor();
        }
        let mut pos = (phase + 32.0) * 8192.0;
        let len = 512.min(n - start);
        let mut w: Vec<f32> = (0..len)
            .map(|_| {
                let y = wave.read(pos);
                pos = inc.mul_add(8192.0, pos);
                y * 0.5 + 0.5
            })
            .collect();
        if sm.coef < 1.0 {
            sm.process(&mut w, true);
        }
        for (k, v) in w.iter().enumerate() {
            let i = start + k;
            let x = (0.5 * (std::f64::consts::TAU * 1000.0 * i as f64 / sr).sin()) as f32;
            out[i] = x * v * g;
        }
        phase += len as f64 * inc;
    }
    let mut delayed = vec![0.0f32; n];
    delayed[16..].copy_from_slice(&out[..n - 16]);
    delayed
}

#[test]
fn waves_match_the_plugin_bit_for_bit() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.amp/in/sb/captures/exp");
    let Ok(j) = std::fs::read_to_string(dir.join("exps.json")) else {
        eprintln!("captures not present; skipped");
        return;
    };
    let exps: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&j).unwrap();
    let mut bad = Vec::new();
    for (name, e) in &exps {
        let plug = read_wav(&dir.join(format!("{name}.wav")));
        let port = render(e);
        let diffs = plug
            .iter()
            .zip(&port)
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        if diffs != 0 {
            bad.push(format!("{name}: {diffs} samples differ"));
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}
