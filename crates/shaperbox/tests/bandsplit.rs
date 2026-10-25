//! The band split against impulse captures of the real plugin (`.amp/in/sb/captures/exp_bs`, kept
//! outside the repository; skipped when absent).

use backspin_shaperbox::bandsplit::{Bands, Bandsplit, MID, multiband_sum};
use std::path::PathBuf;

fn read_wav(path: &PathBuf) -> (u32, Vec<f32>) {
    let d = std::fs::read(path).unwrap();
    let sr = u32::from_le_bytes(d[24..28].try_into().unwrap());
    let ch = u16::from_le_bytes(d[22..24].try_into().unwrap()) as usize;
    let i = d.windows(4).position(|w| w == b"data").unwrap();
    let n = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
    let v: Vec<f32> = d[i + 8..i + 8 + n]
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    (sr, v.iter().step_by(ch).copied().collect())
}

#[test]
fn matches_the_plugin_bit_for_bit() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.amp/in/sb/captures/exp_bs");
    let Ok(j) = std::fs::read_to_string(dir.join("captures.json")) else {
        eprintln!("captures not present; skipped");
        return;
    };
    let cases: Vec<serde_json::Value> = serde_json::from_str(&j).unwrap();
    let n = 8192;
    let mut bad = Vec::new();
    for c in &cases {
        let (low, high) = (
            c["low"].as_f64().unwrap() as f32,
            c["high"].as_f64().unwrap() as f32,
        );
        let (ls, hs) = (
            c["ls"].as_i64().unwrap() as i32,
            c["hs"].as_i64().unwrap() as i32,
        );
        let band = c["band"].as_i64();
        let srv = c["sr"].as_i64().unwrap();
        let name = format!(
            "bs_{:.3}_{:.3}_s{ls}{hs}_{}{}.wav",
            c["low"].as_f64().unwrap(),
            c["high"].as_f64().unwrap(),
            band.map_or("sum".to_string(), |b| format!("b{b}")),
            if srv == 48000 {
                String::new()
            } else {
                format!("_sr{srv}")
            }
        );
        let (sr, l) = read_wav(&dir.join(&name));
        let delay = l.iter().position(|&v| v != 0.0).unwrap_or(16);
        let plug = &l[delay..delay + n];
        let mut x = vec![[0.0f32; 2]; n];
        x[0] = [1.0, 1.0];
        let _ftz = backspin_shaperbox::ftz::FlushToZero::new();
        let mut bs = Bandsplit::new();
        let mut bands = Bands::default();
        let mut out = vec![[0.0f32; 2]; n];
        // The plugin processes in blocks of at most 512 samples.
        for (k, chunk) in x.chunks(512).enumerate() {
            bs.process(chunk, low, high, ls, hs, sr as f32, &mut bands);
            let o = &mut out[k * 512..k * 512 + chunk.len()];
            multiband_sum(
                &bands,
                low,
                high,
                band.is_some(),
                band.map_or(MID, |b| b as usize),
                o,
            );
        }
        let diffs = plug
            .iter()
            .zip(&out)
            .filter(|(a, b)| a.to_bits() != b[0].to_bits())
            .count();
        let max = plug
            .iter()
            .zip(&out)
            .map(|(a, b)| (a - b[0]).abs())
            .fold(0.0f32, f32::max);
        let first = plug
            .iter()
            .zip(&out)
            .position(|(a, b)| a.to_bits() != b[0].to_bits());
        if diffs != 0 {
            bad.push(format!(
                "{name}: {diffs} differ, max {max:e}, first {first:?} plug {:e} port {:e}",
                first.map_or(0.0, |i| plug[i]),
                first.map_or(0.0, |i| out[i][0])
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} captures differ: {:?}",
        bad.len(),
        cases.len(),
        &bad[..bad.len().min(8)]
    );
}
