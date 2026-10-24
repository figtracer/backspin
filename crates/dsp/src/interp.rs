//! Fractional-position readers.

use crate::{Frame, Ring};
use std::f64::consts::PI;

/// Frames on each side of the read position that the sinc reader uses.
pub const SINC_HALF_TAPS: usize = 16;

const SINC_RES: usize = 512;
const WINDOW_RES: usize = 1024;

/// Reads with 4-point cubic Hermite interpolation.
#[inline]
pub fn cubic(ring: &Ring, pos: f64) -> Frame {
    let i = pos.floor();
    let t = (pos - i) as f32;
    let i = i as i64;
    let xm1 = ring.frame(i - 1);
    let x0 = ring.frame(i);
    let x1 = ring.frame(i + 1);
    let x2 = ring.frame(i + 2);
    let mut out = [0.0; 2];
    for c in 0..2 {
        let c0 = x0[c];
        let c1 = 0.5 * (x1[c] - xm1[c]);
        let c2 = xm1[c] - 2.5 * x0[c] + 2.0 * x1[c] - 0.5 * x2[c];
        let c3 = 0.5 * (x2[c] - xm1[c]) + 1.5 * (x0[c] - x1[c]);
        out[c] = ((c3 * t + c2) * t + c1) * t + c0;
    }
    out
}

/// Windowed-sinc reader that lowers its cutoff when reading faster than real time.
#[derive(Clone, Debug)]
pub struct Sinc {
    sinc: Vec<f32>,
    window: Vec<f32>,
}

impl Default for Sinc {
    fn default() -> Self {
        Self::new()
    }
}

impl Sinc {
    /// Builds the lookup tables.
    pub fn new() -> Self {
        let sinc = (0..=SINC_HALF_TAPS * SINC_RES + 1)
            .map(|i| {
                let x = i as f64 / SINC_RES as f64;
                if x == 0.0 {
                    1.0
                } else {
                    ((PI * x).sin() / (PI * x)) as f32
                }
            })
            .collect();
        // Blackman-Harris window over |d| / half in 0..=1.
        let window = (0..=WINDOW_RES + 1)
            .map(|i| {
                let u = (i as f64 / WINDOW_RES as f64).min(1.0);
                let n = 0.5 + 0.5 * u;
                let w = 0.35875 - 0.48829 * (2.0 * PI * n).cos() + 0.14128 * (4.0 * PI * n).cos()
                    - 0.01168 * (6.0 * PI * n).cos();
                w as f32
            })
            .collect();
        Self { sinc, window }
    }

    #[inline]
    fn lookup(table: &[f32], x: f32, res: f32) -> f32 {
        let f = x * res;
        let i = f as usize;
        if i + 1 >= table.len() {
            return 0.0;
        }
        let t = f - i as f32;
        table[i] + (table[i + 1] - table[i]) * t
    }

    /// Reads at `pos`. `speed` is the current read speed; above 1 the cutoff drops to `1/speed`
    /// (down to 1/8) to limit aliasing.
    pub fn read(&self, ring: &Ring, pos: f64, speed: f64) -> Frame {
        let c = (1.0 / speed.abs().max(1.0)).max(0.125) as f32;
        let base = pos.floor();
        let frac = (pos - base) as f32;
        let base = base as i64;
        let half = SINC_HALF_TAPS as i64;
        let mut acc = [0.0f32; 2];
        let mut wsum = 0.0f32;
        for k in (1 - half)..=half {
            let d = (k as f32 - frac).abs();
            let w = Self::lookup(&self.window, d / SINC_HALF_TAPS as f32, WINDOW_RES as f32);
            let s = Self::lookup(&self.sinc, c * d, SINC_RES as f32);
            let weight = w * s;
            let f = ring.frame(base + k);
            acc[0] += f[0] * weight;
            acc[1] += f[1] * weight;
            wsum += weight;
        }
        if wsum.abs() > 1e-6 {
            acc[0] /= wsum;
            acc[1] /= wsum;
        }
        acc
    }
}

/// Selects the reader.
#[derive(Clone, Debug, Default)]
pub struct Interpolator {
    sinc: Sinc,
}

impl Interpolator {
    /// Reads at `pos` with sinc when `hq` is set, cubic Hermite otherwise.
    #[inline]
    pub fn read(&self, ring: &Ring, pos: f64, speed: f64, hq: bool) -> Frame {
        if hq {
            self.sinc.read(ring, pos, speed)
        } else {
            cubic(ring, pos)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_ring(freq: f64, n: usize) -> Ring {
        let mut ring = Ring::new(n);
        for i in 0..n {
            let v = (2.0 * PI * freq * i as f64).sin() as f32;
            ring.push([v, v]);
        }
        ring
    }

    #[test]
    fn integer_positions_are_exact() {
        let ring = sine_ring(0.01, 256);
        let sinc = Sinc::new();
        for i in 20..200 {
            let want = ring.frame(i)[0];
            assert!((cubic(&ring, i as f64)[0] - want).abs() < 1e-6);
            assert!((sinc.read(&ring, i as f64, 1.0)[0] - want).abs() < 1e-3);
        }
    }

    #[test]
    fn fractional_positions_track_sine() {
        let freq = 0.01;
        let ring = sine_ring(freq, 512);
        let sinc = Sinc::new();
        for i in 0..100 {
            let pos = 100.0 + i as f64 * 2.37;
            let want = (2.0 * PI * freq * pos).sin() as f32;
            assert!((cubic(&ring, pos)[0] - want).abs() < 1e-3, "cubic at {pos}");
            assert!((sinc.read(&ring, pos, 1.0)[0] - want).abs() < 2e-3, "sinc at {pos}");
        }
    }

    #[test]
    fn fast_reads_attenuate_high_frequencies() {
        // Near Nyquist, reading at 4x speed should be strongly filtered.
        let ring = sine_ring(0.45, 512);
        let sinc = Sinc::new();
        let peak = (0..200)
            .map(|i| sinc.read(&ring, 100.0 + i as f64 * 1.3, 4.0)[0].abs())
            .fold(0.0, f32::max);
        assert!(peak < 0.05, "{peak}");
    }
}
