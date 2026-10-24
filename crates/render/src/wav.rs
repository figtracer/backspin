//! 32-bit float stereo WAV files, read and written like the harness does.

/// Reads a WAV file as stereo f32 frames (mono is duplicated).
pub fn read(path: &str, expect_sr: u32) -> Result<Vec<[f32; 2]>, String> {
    let d = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return Err(format!("{path}: not a RIFF/WAVE file"));
    }
    let (mut fmt, mut chans, mut bits, mut sr, mut samples) = (0u16, 0u16, 0u16, 0u32, None);
    let mut p = 12;
    while p + 8 <= d.len() {
        let id = &d[p..p + 4];
        let len = u32::from_le_bytes(d[p + 4..p + 8].try_into().unwrap()) as usize;
        let body = &d[p + 8..(p + 8 + len).min(d.len())];
        match id {
            b"fmt " => {
                fmt = u16::from_le_bytes([body[0], body[1]]);
                chans = u16::from_le_bytes([body[2], body[3]]);
                sr = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes([body[14], body[15]]);
                if fmt == 0xfffe && body.len() >= 26 {
                    fmt = u16::from_le_bytes([body[24], body[25]]);
                }
            }
            b"data" => samples = Some(body),
            _ => {}
        }
        p += 8 + len + (len & 1);
    }
    let body = samples.ok_or(format!("{path}: no data chunk"))?;
    if sr != expect_sr {
        eprintln!(
            "[render] warning: {path} is {sr} Hz, rendering at {expect_sr} Hz without resampling"
        );
    }
    let vals: Vec<f32> = match (fmt, bits) {
        (1, 16) => body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (1, 24) => body
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (1, 32) => body
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c) as f32 / 2_147_483_648.0)
            .collect(),
        (3, 32) => body
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
        _ => return Err(format!("{path}: unsupported format {fmt} / {bits} bit")),
    };
    match chans {
        1 => Ok(vals.iter().map(|&v| [v, v]).collect()),
        2 => Ok(vals.as_chunks::<2>().0.to_vec()),
        n => Err(format!("{path}: {n} channels not supported")),
    }
}

/// Writes stereo f32 frames as a 32-bit float WAV file.
pub fn write(path: &str, buf: &[[f32; 2]], sr: u32) -> Result<(), String> {
    let data_len = (buf.len() * 8) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&sr.to_le_bytes());
    out.extend_from_slice(&(sr * 8).to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for f in buf {
        out.extend_from_slice(&f[0].to_le_bytes());
        out.extend_from_slice(&f[1].to_le_bytes());
    }
    std::fs::write(path, out).map_err(|e| format!("{path}: {e}"))
}
