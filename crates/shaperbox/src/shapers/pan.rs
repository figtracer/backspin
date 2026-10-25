//! Pan and Haas (PanShaperModulation::process 0x549ad4, HaasEffect::process 0x2d31fc in mode 0,
//! HaasDelay 0x2e8e5c; shapers_a.md section 4).

use super::Wave;
use crate::smoother::Smoother;

const SQRT2: f32 = 1.414_213_5;
/// `2 * sqrt2 - 4`.
const C: f32 = -1.343_145_7;
/// `DAT_01307190 = 2^0.75`.
const MODE1_GAIN: f32 = 1.681_792_9;
const HAAS_SIZE: usize = 1 << 12;

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// Mode 0 polynomials before the sqrt2 normalisation: `(pL, pR)`.
fn poly_pair(w: f32) -> (f32, f32) {
    let u = clamp01(1.0 - w);
    let a = u - 1.0;
    let fa = 1.0 - a * a * a.abs();
    let p_r = (a * a - 1.0 + fa).mul_add(C, fa); // fmadd at 0x549d50
    let b = (1.0 - u) - 1.0; // 0x549fd0: computed as (1 - u) - 1, not -u
    let fb = 1.0 - b * b * b.abs();
    let p_l = (b * b - 1.0 + fb).mul_add(C, fb); // fmadd at 0x549d74
    (p_l, p_r)
}

/// Pans one frame by `mode` (0 constant power, 1 -4.5 dB, 2 linear balance, 3 attenuate only,
/// 4 crossfeed).
fn pan(f: [f32; 2], w: f32, mode: i32) -> [f32; 2] {
    let [l, r] = f;
    match mode {
        0 => {
            let (pl, pr) = poly_pair(w);
            [l * (pl * SQRT2), r * (pr * SQRT2)]
        }
        1 => {
            let (pl, pr) = poly_pair(w);
            [
                l * (pl.powf(1.5) * MODE1_GAIN),
                r * (pr.powf(1.5) * MODE1_GAIN),
            ]
        }
        2 => {
            let g = (2.0 * (1.0 - w)).clamp(0.0, 2.0);
            [l * (2.0 - g), r * g]
        }
        3 => {
            let v = (2.0 * (1.0 - w) - 1.0).clamp(-1.0, 1.0);
            if v >= 0.0 {
                [l * (1.0 - v), r]
            } else {
                [l, r * (1.0 + v)]
            }
        }
        _ => {
            let v = (2.0 * w - 1.0).clamp(-1.0, 1.0);
            if v >= 0.0 {
                [l + r * v, r * (1.0 - v)]
            } else {
                [l * (1.0 + v), r - l * v]
            }
        }
    }
}

/// The Haas delay curve (0x2d3618): 0 -> 0 ms, 0.5 -> 5 ms, 1 -> 40 ms, in seconds.
fn haas_curve(x: f32) -> f32 {
    ((2.0 * x * (1.0 - 2.0 * x)).max(0.0) + 40.0 * x * x * x) / 1000.0
}

/// One Pan band.
#[derive(Clone, Debug)]
pub struct PanBand {
    sr: f32,
    ring: [Vec<f32>; 2],
    pos: usize,
    tmp: Vec<[f32; 2]>,
    dl: Vec<f32>,
    dr: Vec<f32>,
    /// The delays of the previous block; a block takes the whole-sample path only when its delay
    /// is constant and equal to the previous one (none before the first block).
    prev: [Option<f32>; 2],
}

impl PanBand {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            ring: [vec![0.0; HAAS_SIZE], vec![0.0; HAAS_SIZE]],
            pos: 0,
            tmp: Vec::new(),
            dl: Vec::new(),
            dr: Vec::new(),
            prev: [None; 2],
        }
    }

    /// `pan_mode` and `haas_time` are state values; `haas_mix` is a smoothed port.
    pub fn process(
        &mut self,
        buf: &mut [[f32; 2]],
        w: Wave,
        pan_mode: i32,
        haas_time: f32,
        haas_mix: &Smoother,
    ) {
        let n = buf.len();
        // Pan a copy and blend it back: `a = min(1, 2 - 2 haasMix)`.
        for (i, f) in buf.iter_mut().enumerate() {
            let p = pan(*f, w.at(i), pan_mode);
            let a = (2.0 - 2.0 * haas_mix.at(i)).min(1.0);
            f[0] = a.mul_add(p[0] - f[0], f[0]);
            f[1] = a.mul_add(p[1] - f[1], f[1]);
        }
        // Haas: per-sample delay times from the wave, then a linear-interpolated ring per channel
        // (whole samples when the delay is constant for the block).
        self.dl.clear();
        self.dr.clear();
        for i in 0..n {
            let t = (0.5 - w.at(i)) * haas_time + 0.5;
            self.dl.push(haas_curve(2.0 * (t - 0.5).max(0.0)));
            self.dr.push(haas_curve(2.0 * (0.5 - t).max(0.0)));
        }
        let const_l = self.dl.iter().all(|&d| d == self.dl[0]) && self.prev[0] == Some(self.dl[0]);
        let const_r = self.dr.iter().all(|&d| d == self.dr[0]) && self.prev[1] == Some(self.dr[0]);
        self.prev = [self.dl.last().copied(), self.dr.last().copied()];
        self.tmp.clear();
        self.tmp.extend_from_slice(buf);
        // HaasDelay (0x2e8e5c): the block goes into the ring first; reads are relative to the
        // write position after the block.
        for f in &self.tmp {
            self.ring[0][self.pos] = f[0];
            self.ring[1][self.pos] = f[1];
            self.pos = (self.pos + 1) % HAAS_SIZE;
        }
        let wpos = self.pos as i64;
        let mask = HAAS_SIZE as i64 - 1;
        let mut wet_buf = [Vec::with_capacity(n), Vec::with_capacity(n)];
        for (ch, (ds, konst)) in [(&self.dl, const_l), (&self.dr, const_r)]
            .into_iter()
            .enumerate()
        {
            let ring = &self.ring[ch];
            if konst {
                // Whole samples: `int(d * sr)` behind the block start, no interpolation.
                let k = (ds[0].clamp(0.0, 0.04) * self.sr) as i64;
                let start = wpos - n as i64 - k;
                for i in 0..n as i64 {
                    wet_buf[ch].push(ring[((start + i) & mask) as usize]);
                }
            } else {
                for (i, &d) in ds.iter().enumerate() {
                    let d = if d >= 0.0 {
                        if d <= 0.04 { d } else { 0.04 }
                    } else {
                        0.0
                    };
                    let f = (d * self.sr + n as f32) - i as f32;
                    let i0 = f as i32;
                    let i1 = (i0 + 1).min(HAAS_SIZE as i32 - 1);
                    let y1 = ring[((wpos - i1 as i64) & mask) as usize];
                    let y0 = ring[((wpos - i0 as i64) & mask) as usize];
                    wet_buf[ch].push(y1 + (((i0 as f32) - f) + 1.0) * (y0 - y1));
                }
            }
        }
        for i in 0..n {
            let wet = [wet_buf[0][i], wet_buf[1][i]];
            let hm = haas_mix.at(i);
            let m = (1.0 - (1.0 - 2.0 * hm).powi(3)).min(1.0);
            // 0x2d38c8: out = fma(dry, 1 - m, m * wet).
            let k = (-1.0f32).mul_add(m, 1.0);
            for c in 0..2 {
                buf[i][c] = self.tmp[i][c].mul_add(k, m * wet[c]);
            }
        }
    }
}
