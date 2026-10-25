//! Renders the ShaperBox section with the flags of the offline host for the real plugin
//! (`.amp/in/sb/harness`), so the two can be compared sample by sample:
//! `sbrender --state preset.dat [--gen noise|sine:HZ|ramp|impulse:P --seconds S | --input in.wav]
//!  --output out.wav [--tempo 120] [--sr 48000] [--block 512] [--pos BEATS]`

use backspin_shaperbox::graph::{Processor, Transport};
use backspin_shaperbox::read_preset;
use backspin_shaperbox::state::State;

fn read_wav(path: &str) -> Vec<[f32; 2]> {
    let d = std::fs::read(path).unwrap();
    let i = d.windows(4).position(|w| w == b"data").unwrap();
    let n = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
    let s: Vec<f32> = d[i + 8..i + 8 + n]
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    s.chunks(2).map(|c| [c[0], c[1]]).collect()
}

fn write_wav(path: &str, sr: u32, buf: &[[f32; 2]]) {
    let data: Vec<u8> = buf
        .iter()
        .flat_map(|f| f.iter().flat_map(|v| v.to_le_bytes()))
        .collect();
    let mut out = b"RIFF".to_vec();
    out.extend((36 + data.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(3u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(sr.to_le_bytes());
    out.extend((sr * 8).to_le_bytes());
    out.extend(8u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    std::fs::write(path, out).unwrap();
}

/// The host's test signals (noise: fixed-seed LCG, different on L and R).
fn generate(spec: &str, frames: usize, sr: f64) -> Vec<[f32; 2]> {
    let (kind, arg) = spec.split_once(':').unwrap_or((spec, ""));
    if kind == "noise" {
        let mut s = 0x5eed_1234u64;
        let mut next = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f64 / (1u64 << 31) as f64 * 2.0 - 1.0) as f32 * 0.5
        };
        return (0..frames).map(|_| [next(), next()]).collect();
    }
    let mono: Vec<f32> = match kind {
        "impulse" => {
            let p: usize = arg.parse().unwrap();
            (0..frames)
                .map(|i| if i % p.max(1) == 0 { 1.0 } else { 0.0 })
                .collect()
        }
        "sine" => {
            let hz: f64 = arg.parse().unwrap();
            (0..frames)
                .map(|i| (0.5 * (std::f64::consts::TAU * hz * i as f64 / sr).sin()) as f32)
                .collect()
        }
        _ => {
            let p: usize = if arg.is_empty() {
                frames
            } else {
                arg.parse().unwrap()
            };
            (0..frames)
                .map(|i| (i % p.max(1)) as f32 / p.max(1) as f32)
                .collect()
        }
    };
    mono.iter().map(|&v| [v, v]).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let get = |k: &str| a.iter().position(|x| x == k).map(|i| a[i + 1].clone());
    let sr: f64 = get("--sr").map_or(48000.0, |v| v.parse().unwrap());
    let tempo: f64 = get("--tempo").map_or(120.0, |v| v.parse().unwrap());
    let block: usize = get("--block").map_or(512, |v| v.parse().unwrap());
    let pos: f64 = get("--pos").map_or(0.0, |v| v.parse().unwrap());
    let state = match get("--state") {
        Some(p) => State::from_tree(&read_preset(&std::fs::read(p).unwrap()).unwrap()),
        None => State::default(),
    };
    let mut buf = match (get("--input"), get("--gen")) {
        (Some(p), _) => read_wav(&p),
        (_, Some(g)) => {
            let secs: f64 = get("--seconds").map_or(4.0, |v| v.parse().unwrap());
            generate(&g, (secs * sr) as usize, sr)
        }
        _ => panic!("--input or --gen"),
    };
    let mut p = Processor::new(state, sr as f32);
    let mut start = 0;
    while start < buf.len() {
        let n = block.min(buf.len() - start);
        let beats = pos + start as f64 / sr * tempo / 60.0;
        p.process(
            &mut buf[start..start + n],
            Transport {
                ppq: beats,
                bpm: tempo,
                playing: true,
            },
        );
        start += n;
    }
    write_wav(&get("--output").unwrap(), sr as u32, &buf);
}
