//! The impulse response source. No Cableguys IR data ships with Backspin: "Standard Hall 1" is
//! read at runtime from a local ShaperBox 3 install (shapers_b.md 5.4), and a procedural hall
//! stands in when no install is found.
//!
//! The plugin image stores the WAV resource XORed with a rotating single bit,
//! `wav[i] = raw[i] ^ (1 << ((i + 3) & 7))`. The loader searches the image for the encoded
//! `RIFF....WAVE` header instead of a fixed offset, so other builds and the Windows binary work
//! as long as they keep the packing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::shapers::tpt_k;

/// The length of the procedural hall at 44.1 kHz, the same as "Standard Hall 1" (3.30 s), so
/// both get the same partition size.
const PROCEDURAL_FRAMES: usize = 145_688;
const PROCEDURAL_SR: f64 = 44100.0;

static HALL: OnceLock<Arc<SourceIr>> = OnceLock::new();

/// A decoded impulse response: its sample rate and one or two channels.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceIr {
    pub sr: f64,
    pub chans: Vec<Vec<f32>>,
}

/// The default ShaperBox 3 binaries to search for "Standard Hall 1" on this OS.
pub fn default_ir_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        out.push(PathBuf::from(
            "/Library/Audio/Plug-Ins/VST3/ShaperBox 3.vst3/Contents/MacOS/ShaperBox",
        ));
    }
    if cfg!(target_os = "windows") {
        out.push(PathBuf::from(
            r"C:\Program Files\Common Files\VST3\ShaperBox 3.vst3\Contents\x86_64-win\ShaperBox 3.vst3",
        ));
    }
    out
}

/// "Standard Hall 1" from the first candidate binary that holds it, else [`procedural_hall`].
/// Reads files; call it off the audio thread.
pub fn load_hall_ir(candidates: &[PathBuf]) -> SourceIr {
    candidates
        .iter()
        .find_map(|p| hall_ir_from_plugin(p))
        .unwrap_or_else(procedural_hall)
}

/// The process-wide hall IR, loaded from `candidates` on the first call. Later calls return the
/// same IR whatever they pass, so the plugin calls this once with its OS paths before it builds
/// a processor.
pub fn init_hall_ir(candidates: &[PathBuf]) -> Arc<SourceIr> {
    HALL.get_or_init(|| Arc::new(load_hall_ir(candidates)))
        .clone()
}

/// The process-wide hall IR, loaded from [`default_ir_paths`] unless [`init_hall_ir`] ran first.
pub fn hall_ir() -> Arc<SourceIr> {
    init_hall_ir(&default_ir_paths())
}

/// Reads the embedded "Standard Hall 1" WAV from a ShaperBox 3 binary without loading it.
pub fn hall_ir_from_plugin(path: &Path) -> Option<SourceIr> {
    let image = std::fs::read(path).ok()?;
    find_embedded_wav(&image)
}

/// The first XOR-packed RIFF WAVE resource in `image` that decodes.
fn find_embedded_wav(image: &[u8]) -> Option<SourceIr> {
    let key = |i: usize| 1u8 << ((i + 3) & 7);
    let riff: [u8; 4] = std::array::from_fn(|i| b"RIFF"[i] ^ key(i));
    let wave: [u8; 4] = std::array::from_fn(|i| b"WAVE"[i] ^ key(i + 8));
    let mut at = 0;
    while let Some(off) = image[at..]
        .windows(12)
        .position(|w| w[..4] == riff && w[8..] == wave)
    {
        let start = at + off;
        at = start + 1;
        let size = u32::from_le_bytes(std::array::from_fn(|i| image[start + 4 + i] ^ key(4 + i)));
        let Some(raw) = image.get(start..start + size as usize + 8) else {
            continue;
        };
        let wav: Vec<u8> = raw.iter().enumerate().map(|(i, b)| b ^ key(i)).collect();
        if let Some(ir) = decode_wav(&wav) {
            return Some(ir);
        }
    }
    None
}

/// Decodes a RIFF WAV: PCM 16 or 24 bit (`int / 2^(bits-1)`, exact as JUCE does it) or float 32.
pub fn decode_wav(data: &[u8]) -> Option<SourceIr> {
    if data.get(..4)? != b"RIFF" || data.get(8..12)? != b"WAVE" {
        return None;
    }
    let u16_at = |p: usize| Some(u16::from_le_bytes(data.get(p..p + 2)?.try_into().ok()?));
    let u32_at = |p: usize| Some(u32::from_le_bytes(data.get(p..p + 4)?.try_into().ok()?));
    let mut p = 12;
    let mut fmt = None;
    while p + 8 <= data.len() {
        let id = &data[p..p + 4];
        let size = u32_at(p + 4)? as usize;
        let body = data.get(p + 8..(p + 8 + size).min(data.len()))?;
        if id == b"fmt " {
            fmt = Some((
                u16_at(p + 8)?,
                u16_at(p + 10)? as usize,
                u32_at(p + 12)? as f64,
                u16_at(p + 20)? as usize,
                u16_at(p + 22)?,
            ));
        } else if id == b"data" {
            let (tag, ch, sr, align, bits) = fmt?;
            if ch == 0 || align == 0 {
                return None;
            }
            let frames = body.len() / align;
            let mut chans = vec![Vec::with_capacity(frames); ch];
            for f in body.chunks_exact(align) {
                for (c, out) in chans.iter_mut().enumerate() {
                    let s = &f[c * (bits as usize / 8)..];
                    out.push(match (tag, bits) {
                        (1 | 0xfffe, 16) => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                        (1 | 0xfffe, 24) => {
                            (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0
                        }
                        (3 | 0xfffe, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                        _ => return None,
                    });
                }
            }
            return Some(SourceIr { sr, chans });
        }
        p += 8 + size + (size & 1);
    }
    None
}

/// A stand-in hall when no ShaperBox install is found: 3.3 s of stereo noise at 44.1 kHz with
/// a 12 ms gap, a few early reflections, a 2.4 s RT60 and a low-pass that darkens the tail,
/// scaled to the level of "Standard Hall 1" (peak about 0.07).
pub fn procedural_hall() -> SourceIr {
    let sr = PROCEDURAL_SR;
    let rt60 = 2.4;
    let gap = (0.012 * sr) as usize;
    let early: [(f64, f32, f32); 6] = [
        (0.013, 0.9, 0.6),
        (0.019, 0.5, 0.8),
        (0.027, 0.6, 0.4),
        (0.034, 0.3, 0.55),
        (0.043, 0.45, 0.3),
        (0.056, 0.25, 0.35),
    ];
    let mut chans = Vec::with_capacity(2);
    for (c, seed) in [0x1234_5678u32, 0x9abc_def1].into_iter().enumerate() {
        let mut s = seed;
        let mut lp = 0.0f32;
        let mut hp = 0.0f32;
        let mut out = vec![0.0f32; PROCEDURAL_FRAMES];
        for (i, v) in out.iter_mut().enumerate().skip(gap) {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let noise = (s as i32) as f32 / 2_147_483_648.0;
            let t = (i - gap) as f64 / sr;
            let onset = (t / 0.025).min(1.0) as f32;
            let env = 10f64.powf(-3.0 * t / rt60) as f32 * onset;
            // The cutoff falls from 9 kHz to 2 kHz over the tail (air absorption).
            let fc = 2000.0 + 7000.0 * (-t / 0.8).exp();
            let k = tpt_k(fc as f32, sr as f32);
            let d = k * (noise * env - lp);
            let y = lp + d;
            lp = y + d;
            // A one-pole 60 Hz high-pass keeps the tail free of DC.
            hp += 0.0085 * (y - hp);
            *v = y - hp;
        }
        for &(at, gl, gr) in &early {
            let i = (at * sr) as usize;
            out[i] += if c == 0 { gl } else { gr } * 0.6;
        }
        chans.push(out);
    }
    let peak = chans.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 0.0 {
        let g = 0.072 / peak;
        for v in chans.iter_mut().flatten() {
            *v *= g;
        }
    }
    SourceIr { sr, chans }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(wav: &[u8]) -> Vec<u8> {
        wav.iter()
            .enumerate()
            .map(|(i, b)| b ^ (1 << ((i + 3) & 7)))
            .collect()
    }

    fn wav24(frames: &[[i32; 2]]) -> Vec<u8> {
        let mut data = Vec::<u8>::new();
        for f in frames {
            for v in f {
                data.extend(&v.to_le_bytes()[..3]);
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend((36 + data.len() as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(44100u32.to_le_bytes());
        out.extend((44100u32 * 6).to_le_bytes());
        out.extend(6u16.to_le_bytes());
        out.extend(24u16.to_le_bytes());
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    /// The packed resource is found at any offset (the XOR key restarts at the resource).
    #[test]
    fn finds_packed_wav_anywhere() {
        let wav = wav24(&[[8_388_607, -8_388_608], [1, -1], [4_194_304, 0]]);
        for lead in [0usize, 3, 17] {
            let mut image = vec![0x5au8; lead];
            image.extend(pack(&wav));
            image.extend([0u8; 9]);
            let ir = find_embedded_wav(&image).unwrap();
            assert_eq!(ir.sr, 44100.0);
            assert_eq!(
                ir.chans[0],
                [8_388_607.0 / 8_388_608.0, 1.0 / 8_388_608.0, 0.5]
            );
            assert_eq!(ir.chans[1], [-1.0, -1.0 / 8_388_608.0, 0.0]);
        }
        assert!(find_embedded_wav(&wav).is_none());
    }

    /// The hall from the local ShaperBox 3 install (shapers_b.md 5.4), when there is one.
    #[test]
    fn hall_loads_from_the_installed_plugin() {
        let Some(path) = default_ir_paths().into_iter().find(|p| p.exists()) else {
            eprintln!("ShaperBox 3 not installed; skipped");
            return;
        };
        let ir = hall_ir_from_plugin(&path).unwrap();
        assert_eq!(ir.sr, 44100.0);
        assert_eq!(ir.chans.len(), 2);
        assert_eq!(ir.chans[0].len(), 145_688);
        let peak = |c: &[f32]| c.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak(&ir.chans[0]) - 0.0754).abs() < 1e-4);
        assert!((peak(&ir.chans[1]) - 0.0698).abs() < 1e-4);
    }

    #[test]
    fn missing_install_falls_back_to_the_procedural_hall() {
        let ir = load_hall_ir(&[PathBuf::from("/nonexistent/ShaperBox")]);
        assert_eq!(ir, procedural_hall());
    }

    #[test]
    fn procedural_hall_is_a_decaying_stereo_tail() {
        let ir = procedural_hall();
        assert_eq!(ir.chans.len(), 2);
        assert_eq!(ir.chans[0].len(), PROCEDURAL_FRAMES);
        let rms = |c: &[f32], a: usize, b: usize| {
            (c[a..b].iter().map(|v| v * v).sum::<f32>() / (b - a) as f32).sqrt()
        };
        let (l, r) = (&ir.chans[0], &ir.chans[1]);
        assert!(rms(l, 4410, 8820) > 10.0 * rms(l, 132_300, 145_000));
        assert_ne!(l[20_000], r[20_000]);
        let peak = ir
            .chans
            .iter()
            .flatten()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.072).abs() < 1e-6);
    }
}
