//! The "Remove DC offset" filter (FUN_00072020, dsp.md section 10).

use crate::EPS24;

/// A second-order high-pass near 4 Hz, in f64 per channel with f32 coefficients.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DcFilter {
    k: f32,
    d: f32,
    lp: [f64; 2],
    u: [f64; 2],
}

impl DcFilter {
    /// Sets the coefficients for a sample rate (Dispatcher case 4).
    pub fn set_sample_rate(&mut self, sr: i32) {
        self.k = ((44100.0 / sr as f64) * 2.2834999999999997e-05) as f32;
        self.d = f32::from_bits(0x3f75_c28f);
    }

    /// Clears the state (0x71e70).
    pub fn reset(&mut self) {
        self.lp = [0.0; 2];
        self.u = [0.0; 2];
    }

    /// Filters `buf` in place, then flushes states below `2^-24` to 0.
    pub fn process(&mut self, buf: &mut [[f32; 2]]) {
        let (k, d) = (self.k as f64, self.d as f64);
        for f in buf {
            for ((y, lp), u) in f.iter_mut().zip(&mut self.lp).zip(&mut self.u) {
                let x = *y as f64;
                *u += (x - *lp) * k;
                *lp += *u;
                *u *= d;
                *y = (x - *lp) as f32;
            }
        }
        for v in self.lp.iter_mut().chain(self.u.iter_mut()) {
            if v.abs() < EPS24 {
                *v = 0.0;
            }
        }
    }
}
