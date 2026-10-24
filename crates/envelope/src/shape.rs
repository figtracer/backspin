//! Segment shapes.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// The curve of the segment that ends at a point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Shape {
    /// Keeps the start value, then jumps to the end value at the end point.
    Hold,
    /// Smoothstep S-curve.
    Smooth,
    /// Exponential curve. Tension bends it.
    #[default]
    SingleCurve,
    /// Power curve. Tension sets the exponent.
    SingleCurve2,
    /// Rational curve. Tension bends it.
    SingleCurve3,
    /// Mirrored exponential S-curve.
    DoubleCurve,
    /// Mirrored power S-curve.
    DoubleCurve2,
    /// Mirrored rational S-curve.
    DoubleCurve3,
    /// Quarter sine. Tension sign picks ease-out or ease-in.
    HalfSine,
    /// Steps. Tension sets the step count.
    Stairs,
    /// Steps with smoothstep edges. Tension sets the step count.
    SmoothStairs,
    /// Square wave between the start and end values. Tension sets the cycle count.
    Pulse,
    /// Raised cosine wave between the start and end values. Tension sets the cycle count.
    Wave,
}

impl Shape {
    /// All shapes, in menu order.
    pub const ALL: [Self; 13] = [
        Self::Hold,
        Self::Smooth,
        Self::SingleCurve,
        Self::SingleCurve2,
        Self::SingleCurve3,
        Self::DoubleCurve,
        Self::DoubleCurve2,
        Self::DoubleCurve3,
        Self::HalfSine,
        Self::Stairs,
        Self::SmoothStairs,
        Self::Pulse,
        Self::Wave,
    ];

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Hold => "Hold",
            Self::Smooth => "Smooth",
            Self::SingleCurve => "Single curve",
            Self::SingleCurve2 => "Single curve 2",
            Self::SingleCurve3 => "Single curve 3",
            Self::DoubleCurve => "Double curve",
            Self::DoubleCurve2 => "Double curve 2",
            Self::DoubleCurve3 => "Double curve 3",
            Self::HalfSine => "Half sine",
            Self::Stairs => "Stairs",
            Self::SmoothStairs => "Smooth stairs",
            Self::Pulse => "Pulse",
            Self::Wave => "Wave",
        }
    }

    /// Returns true if the tension sets a step or cycle count.
    pub fn is_periodic(self) -> bool {
        matches!(self, Self::Stairs | Self::SmoothStairs | Self::Pulse | Self::Wave)
    }

    /// Returns the progress from the start value to the end value at `t` (0..=1).
    ///
    /// `tension` is -1..=1. Most shapes return 0 at `t = 0` and 1 at `t = 1`. Pulse and Wave
    /// oscillate and end at their start value.
    pub fn eval(self, t: f64, tension: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        let k = tension.clamp(-1.0, 1.0);
        match self {
            Self::Hold => {
                if t < 1.0 {
                    0.0
                } else {
                    1.0
                }
            }
            Self::Smooth => smoothstep(t),
            Self::SingleCurve => exp_curve(t, k),
            Self::SingleCurve2 => pow_curve(t, k),
            Self::SingleCurve3 => rational_curve(t, k),
            Self::DoubleCurve => mirrored(t, k, exp_curve),
            Self::DoubleCurve2 => mirrored(t, k, pow_curve),
            Self::DoubleCurve3 => mirrored(t, k, rational_curve),
            Self::HalfSine => {
                let ease_out = (t * PI / 2.0).sin();
                let ease_in = 1.0 - (t * PI / 2.0).cos();
                let w = (k + 1.0) / 2.0;
                ease_out * (1.0 - w) + ease_in * w
            }
            Self::Stairs => {
                let n = count(k);
                if t >= 1.0 {
                    1.0
                } else {
                    (t * n).floor() / n
                }
            }
            Self::SmoothStairs => {
                let n = count(k);
                if t >= 1.0 {
                    1.0
                } else {
                    let s = t * n;
                    (s.floor() + smoothstep(s.fract())) / n
                }
            }
            Self::Pulse => {
                let n = count(k);
                if (t * n).fract() < 0.5 {
                    0.0
                } else {
                    1.0
                }
            }
            Self::Wave => 0.5 - 0.5 * (2.0 * PI * count(k) * t).cos(),
        }
    }
}

/// Maps tension -1..=1 to a step or cycle count of 1..=16.
pub fn count(tension: f64) -> f64 {
    (1.0 + ((tension.clamp(-1.0, 1.0) + 1.0) / 2.0 * 15.0).round()).max(1.0)
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn exp_curve(t: f64, k: f64) -> f64 {
    if k.abs() < 1e-6 {
        return t;
    }
    let a = k * 8.0;
    (a * t).exp_m1() / a.exp_m1()
}

fn pow_curve(t: f64, k: f64) -> f64 {
    t.powf(8f64.powf(k))
}

fn rational_curve(t: f64, k: f64) -> f64 {
    let c = k * 0.98;
    let r = (1.0 + c) / (1.0 - c);
    let d = t + (1.0 - t) * r;
    if d <= 0.0 {
        t
    } else {
        t / d
    }
}

fn mirrored(t: f64, k: f64, f: fn(f64, f64) -> f64) -> f64 {
    if t < 0.5 {
        0.5 * f(2.0 * t, k)
    } else {
        1.0 - 0.5 * f(2.0 - 2.0 * t, k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_shapes_hit_endpoints() {
        for shape in Shape::ALL {
            if matches!(shape, Shape::Pulse | Shape::Wave) {
                continue;
            }
            for k in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                assert!(shape.eval(0.0, k).abs() < 1e-9, "{shape:?} k={k} at 0");
                assert!((shape.eval(1.0, k) - 1.0).abs() < 1e-9, "{shape:?} k={k} at 1");
                let mut prev = 0.0;
                for i in 0..=100 {
                    let v = shape.eval(i as f64 / 100.0, k);
                    assert!(v + 1e-9 >= prev, "{shape:?} k={k} not monotonic");
                    prev = v;
                }
            }
        }
    }

    #[test]
    fn zero_tension_curves_are_linear() {
        for shape in [
            Shape::SingleCurve,
            Shape::SingleCurve2,
            Shape::SingleCurve3,
            Shape::DoubleCurve,
            Shape::DoubleCurve2,
            Shape::DoubleCurve3,
        ] {
            for i in 0..=10 {
                let t = i as f64 / 10.0;
                assert!((shape.eval(t, 0.0) - t).abs() < 1e-9, "{shape:?}");
            }
        }
    }

    #[test]
    fn stairs_count() {
        assert_eq!(count(-1.0), 1.0);
        assert_eq!(count(1.0), 16.0);
        assert_eq!(Shape::Stairs.eval(0.3, 1.0), (0.3f64 * 16.0).floor() / 16.0);
    }
}
