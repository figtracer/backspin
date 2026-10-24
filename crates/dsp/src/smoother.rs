//! The one-pole smoothers (FUN_001dac40 setup, FUN_001dae40 process; dsp.md 6.3).

use crate::{EPS24, LN_001};

/// The coefficient that decays to 1 % in `samples` samples: `exp(ln 0.01 / max(samples, 1))`,
/// or 0 if `samples <= 0`.
pub fn coef(samples: f64) -> f64 {
    if 0.0 < samples {
        let x = if samples <= 1.0 { 1.0 } else { samples };
        (LN_001 / x).exp()
    } else {
        0.0
    }
}

/// A one-pole smoother with separate rise and fall coefficients and an f64 state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OnePole {
    /// Coefficient used when the input is above the state.
    pub rise: f64,
    /// Coefficient used when the input is at or below the state.
    pub fall: f64,
    /// The state.
    pub state: f64,
}

impl OnePole {
    /// Sets both coefficients from times in samples (0x1dac40).
    pub fn set_times(&mut self, rise: f64, fall: f64) {
        self.rise = coef(rise);
        self.fall = coef(fall);
    }

    /// Smooths `buf` in place, then flushes a state below `2^-24` to 0 (0x1dae40).
    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf {
            let x = *v as f64;
            self.state = if x <= self.state {
                x + (self.state - x) * self.fall
            } else {
                x + (self.state - x) * self.rise
            };
            *v = self.state as f32;
        }
        if self.state.abs() < EPS24 {
            self.state = 0.0;
        }
    }
}
