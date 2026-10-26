//! Runs one shaper kernel over a WAV file with a constant wave, for comparing with the real plugin:
//! `kernel SHAPER in.wav out.wav w [params...]` (block 512, 48 kHz).
//!   volume w gain | width w gain | pan w mode haasTime haasMix | time w delayTimeMax bpm

use backspin_shaperbox::shapers::{Wave, pan::PanBand, time, volume, width};
use backspin_shaperbox::smoother::Smoother;

fn read(path: &str) -> (u32, Vec<[f32; 2]>) {
    let d = std::fs::read(path).unwrap();
    let sr = u32::from_le_bytes(d[24..28].try_into().unwrap());
    let i = d.windows(4).position(|w| w == b"data").unwrap();
    let n = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
    let s: Vec<f32> = d[i + 8..i + 8 + n]
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    (sr, s.chunks(2).map(|c| [c[0], c[1]]).collect())
}

fn write(path: &str, sr: u32, buf: &[[f32; 2]]) {
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

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (shaper, inp, outp) = (a[1].as_str(), &a[2], &a[3]);
    let w: f32 = a[4].parse().unwrap();
    let p = |i: usize| a[5 + i].parse::<f32>().unwrap();
    let (sr, mut buf) = read(inp);
    let wave = Wave::Constant(w);
    let mut pan = PanBand::new(sr as f32);
    let mut tb = time::TimeBand::new(sr as f32);
    let mut gain = Smoother::new(if shaper == "volume" { p(0) } else { 0.5 }, sr as f64, 0.0);
    let mut hm = Smoother::new(
        std::env::var("HM0")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(if shaper == "pan" { p(2) } else { 0.0 }),
        sr as f64,
        0.0,
    );
    for block in buf.chunks_mut(512) {
        let n = block.len();
        match shaper {
            "volume" => {
                gain.process(p(0), n);
                volume::process(block, wave, &gain);
            }
            "width" => width::process(block, wave, p(0)),
            "pan" => {
                hm.process(p(2), n);
                pan.process(block, wave, p(0) as i32, p(1), &hm);
            }
            "time" => tb.process(
                block,
                wave,
                time::range_seconds(p(0) as i32, p(1), 1.0),
                None,
            ),
            _ => panic!("unknown shaper {shaper}"),
        }
    }
    write(outp, sr, &buf);
}
