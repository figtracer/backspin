//! Smoothing filters.

use crate::Frame;

/// Returns the one-pole coefficient that reaches ~63% of a step in `ms` milliseconds.
pub fn one_pole_coeff(ms: f32, sample_rate: f32) -> f32 {
    if ms <= 0.0 {
        return 1.0;
    }
    1.0 - (-1.0 / (ms * 0.001 * sample_rate)).exp()
}

/// One-pole low-pass smoother.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    /// Current output.
    pub value: f32,
}

impl OnePole {
    /// Moves toward `target` with coefficient `coeff` (0..=1).
    #[inline]
    pub fn next(&mut self, target: f32, coeff: f32) -> f32 {
        self.value += (target - self.value) * coeff;
        self.value
    }
}

/// One-pole smoother with separate rise (attack) and fall (release) coefficients.
#[derive(Clone, Copy, Debug)]
pub struct AsymSmoother {
    /// Current output.
    pub value: f32,
}

impl Default for AsymSmoother {
    fn default() -> Self {
        Self { value: 1.0 }
    }
}

impl AsymSmoother {
    /// Moves toward `target`, with `attack` when rising and `release` when falling.
    #[inline]
    pub fn next(&mut self, target: f32, attack: f32, release: f32) -> f32 {
        let coeff = if target > self.value { attack } else { release };
        self.value += (target - self.value) * coeff;
        self.value
    }
}

/// Three cascaded one-pole filters. Gives a value inertia, like a turntable platter.
#[derive(Clone, Copy, Debug, Default)]
pub struct Inertia {
    stages: [OnePole; 3],
}

impl Inertia {
    /// Moves toward `target` with coefficient `coeff`.
    #[inline]
    pub fn next(&mut self, target: f32, coeff: f32) -> f32 {
        let a = self.stages[0].next(target, coeff);
        let b = self.stages[1].next(a, coeff);
        self.stages[2].next(b, coeff)
    }

    /// Jumps to `value` at once.
    pub fn reset(&mut self, value: f32) {
        for s in &mut self.stages {
            s.value = value;
        }
    }
}

/// One-pole DC-blocking high-pass filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlocker {
    x1: Frame,
    y1: Frame,
}

impl DcBlocker {
    /// Filters one frame. `r` is the pole radius, close to 1.
    #[inline]
    pub fn next(&mut self, x: Frame, r: f32) -> Frame {
        let mut y = [0.0; 2];
        for c in 0..2 {
            y[c] = x[c] - self.x1[c] + r * self.y1[c];
        }
        self.x1 = x;
        self.y1 = y;
        y
    }

    /// Returns the pole radius for a cutoff of `hz`.
    pub fn radius(hz: f32, sample_rate: f32) -> f32 {
        1.0 - 2.0 * std::f32::consts::PI * hz / sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pole_converges() {
        let mut s = OnePole::default();
        let c = one_pole_coeff(1.0, 48000.0);
        for _ in 0..48000 {
            s.next(1.0, c);
        }
        assert!((s.value - 1.0).abs() < 1e-4);
    }

    #[test]
    fn asym_uses_attack_and_release() {
        let mut s = AsymSmoother { value: 0.0 };
        s.next(1.0, 1.0, 0.0);
        assert_eq!(s.value, 1.0);
        s.next(0.0, 1.0, 0.0);
        assert_eq!(s.value, 1.0);
    }

    #[test]
    fn dc_blocker_removes_offset() {
        let mut dc = DcBlocker::default();
        let r = DcBlocker::radius(5.0, 48000.0);
        let mut y = [1.0; 2];
        for _ in 0..48000 {
            y = dc.next([1.0, 1.0], r);
        }
        assert!(y[0].abs() < 1e-3, "{y:?}");
    }
}
