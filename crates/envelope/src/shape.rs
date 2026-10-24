//! Segment shapes: the derived coefficients (Prepare 0x23d040) and the evaluator (0x2422d0).

use crate::{EPS24, Envelope, Point};

/// ln(1000001), the curve tension scale (0x2b0fe8).
const LN_1000001: f64 = 13.815511557963774;
/// pi/2 rounded to f32 (0x354600), used by Half sine.
const HALF_PI_F32: f32 = 1.570_796_4;
/// 2pi rounded to f32 (0x40c90fdb), used by Pulse and Wave.
const TWO_PI_F32: f32 = 6.283_185_5;

/// Shape ids (`PointMenu` tags) and their menu captions.
pub struct Shape;

impl Shape {
    pub const SINGLE_CURVE: u8 = 0;
    pub const DOUBLE_CURVE: u8 = 1;
    pub const HOLD: u8 = 2;
    pub const STAIRS: u8 = 3;
    pub const SMOOTH_STAIRS: u8 = 4;
    pub const PULSE: u8 = 5;
    pub const WAVE: u8 = 6;
    pub const SINGLE_CURVE_2: u8 = 7;
    pub const DOUBLE_CURVE_2: u8 = 8;
    pub const HALF_SINE: u8 = 9;
    pub const SMOOTH: u8 = 10;
    pub const SINGLE_CURVE_3: u8 = 11;
    pub const DOUBLE_CURVE_3: u8 = 12;

    /// The menu caption of a shape id, or `None` for ids above 12 (they evaluate as Hold).
    pub fn name(id: u8) -> Option<&'static str> {
        SHAPE_MENU.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
    }
}

/// The shapes in point menu order, with their captions.
pub const SHAPE_MENU: [(u8, &str); 13] = [
    (Shape::HOLD, "Hold"),
    (Shape::SMOOTH, "Smooth"),
    (Shape::SINGLE_CURVE, "Single curve"),
    (Shape::SINGLE_CURVE_2, "Single curve 2"),
    (Shape::SINGLE_CURVE_3, "Single curve 3"),
    (Shape::DOUBLE_CURVE, "Double curve"),
    (Shape::DOUBLE_CURVE_2, "Double curve 2"),
    (Shape::DOUBLE_CURVE_3, "Double curve 3"),
    (Shape::HALF_SINE, "Half sine"),
    (Shape::STAIRS, "Stairs"),
    (Shape::SMOOTH_STAIRS, "Smooth stairs"),
    (Shape::PULSE, "Pulse"),
    (Shape::WAVE, "Wave"),
];

/// The coefficients of the "Single curve" formula for a curvature `k` (0x1e1ce0): `[k, 1/|k|,
/// ln(|k| + 1)]`. The last two are left unchanged when `k` is 0.
pub fn curve_params(k: f32, c: &mut [f32; 3]) {
    c[0] = k;
    if k != 0.0 {
        c[1] = 1.0 / k.abs();
        c[2] = ((k.abs() + 1.0) as f64).ln() as f32;
    }
}

/// Evaluates the "Single curve" formula with raw coefficients (k bits, 1/|k|, ln(|k| + 1)).
pub fn single_curve(c: &[f32; 3], x: f64) -> f64 {
    let k_bits = c[0].to_bits() as i32;
    if k_bits == 0 {
        return x;
    }
    let (r, l) = (c[1] as f64, c[2] as f64);
    if k_bits >= 0 {
        1.0 - (((1.0 - x) * l).exp() - 1.0) * r
    } else {
        ((x * l).exp() - 1.0) * r
    }
}

/// The step or cycle count of shapes 3..=6 (0x23ef30); -1 for other shapes.
pub fn step_count(p: &Point) -> i32 {
    if !(Shape::STAIRS..=Shape::WAVE).contains(&p.shape) {
        return -1;
    }
    let a = (1.0 - p.tension.abs() as f64 * 0.6) as f32;
    let e = ((a as f64 * LN_1000001).exp() - 1.0) as f32;
    let mut n = (e as f64 * 0.001).round_ties_even() as i64 as i32;
    if p.shape == Shape::STAIRS && p.tension >= 0.0 {
        n += 1;
    }
    n
}

impl Envelope {
    /// Recomputes the derived point fields (Prepare 0x23d040): clamps negative `dx` to 0, then
    /// sets the sign byte and the coefficients of points 1..
    pub fn prepare(&mut self) {
        for p in &mut self.points {
            if p.dx < 0.0 {
                p.dx = 0.0;
            }
        }
        for i in 1..self.points.len() {
            self.prepare_point(i);
        }
    }

    fn prepare_point(&mut self, i: usize) {
        let n = step_count(&self.points[i]);
        let p = &mut self.points[i];
        let t = p.tension;
        p.sign = if t.to_bits() & 0x7fff_ffff == 0 {
            0
        } else if t.to_bits() >> 31 != 0 {
            -1
        } else {
            1
        };
        match p.shape {
            Shape::SINGLE_CURVE | Shape::DOUBLE_CURVE => {
                let e = (t.abs() as f64 * LN_1000001).exp();
                let s = if t.abs() == 0.0 {
                    0.0
                } else if t.to_bits() >> 31 != 0 {
                    -1.0
                } else {
                    1.0
                };
                curve_params((e - 1.0) as f32 * s, &mut p.c);
            }
            Shape::STAIRS => {
                let s: f32 = if t < 0.0 { -1.0 } else { 1.0 };
                p.c[2] = s * 0.5;
                p.c[0] = (n as i64 + 1) as f32;
                p.c[1] = 1.0 / (p.c[0] - s);
            }
            Shape::SMOOTH_STAIRS => {
                p.c[0] = (n as i64 + 1) as f32;
                p.c[1] = 1.0 / p.c[0];
            }
            Shape::PULSE => {
                let s: f32 = if t < 0.0 { -1.0 } else { 1.0 };
                p.c[2] = s * 0.5;
                p.c[0] = ((n as i64 + 1) * 2) as f32;
                p.c[1] = TWO_PI_F32;
            }
            Shape::WAVE => {
                p.c[0] = n as f32 + 0.5;
                p.c[1] = TWO_PI_F32;
                if n == 0 && p.sign > 0 {
                    p.sign = 2;
                }
            }
            Shape::SINGLE_CURVE_2 | Shape::DOUBLE_CURVE_2 => p.c[0] = t,
            Shape::HALF_SINE => p.c[0] = t.abs(),
            Shape::SMOOTH => self.prepare_smooth(i),
            Shape::SINGLE_CURVE_3 | Shape::DOUBLE_CURVE_3 => {
                let mut d = t.abs() as f64 * 0.9;
                if d <= 1.52587890625e-05 {
                    d = 1.52587890625e-05;
                }
                let d = 1.0 - d;
                p.c[0] = (1.0 - d * d * d) as f32;
                p.c[1] = (1.0 - p.c[0]) / p.c[0];
                p.c[2] = p.c[1];
                if p.sign > 0 {
                    p.c[1] = -p.c[1];
                    p.c[2] = p.c[1] - 1.0;
                }
                if p.shape == Shape::DOUBLE_CURVE_3 {
                    p.c[1] *= 0.5;
                    p.c[2] *= 0.5;
                }
            }
            _ => {}
        }
        let p = &mut self.points[i];
        if matches!(p.shape, 0 | 1 | 7 | 8 | 9 | 11 | 12) && p.tension == 0.0 {
            p.sign = 2;
        }
    }

    /// The Smooth tangents (Prepare 0x23d040 case 10). Neighbours only count if they are Smooth;
    /// there is no wrap across the envelope end.
    fn prepare_smooth(&mut self, i: usize) {
        let pts = &self.points;
        let p = &pts[i];
        let len = if p.dx <= EPS24 { EPS24 } else { p.dx };
        let y1 = p.value as f64;
        let ip = i - 1;
        let y0 = pts[ip].value as f64;
        let mut ym = y0;
        let r0 = if pts[ip].shape != Shape::SMOOTH {
            1.0
        } else {
            if ip >= 1 {
                ym = pts[ip - 1].value as f64;
            }
            pts[ip].dx / len
        };
        let (r1, yn) = match pts.get(i + 1) {
            Some(q) if q.shape == Shape::SMOOTH => (q.dx / len, q.value as f64),
            _ => (1.0, y1),
        };
        let dy = y1 - y0;
        let tension = p.tension;
        let p = &mut self.points[i];
        // A NaN step counts as flat, as in the original.
        if dy.abs().partial_cmp(&EPS24).is_none_or(|o| o.is_lt()) {
            p.sign = 2;
            return;
        }
        let q0 = if EPS24 < r0 { r0 } else { EPS24 };
        let q1 = if EPS24 < r1 { r1 } else { EPS24 };
        let mut a = (((yn - y0) / dy - 1.0) / q1) as f32;
        a = if 0.0 < a * 1.0 {
            let pp = q1 as f32 * 2.0 + 1.0;
            let qq = q1 as f32 + 2.0;
            (a * 1.0 * (pp + qq)) / (pp * a + qq * 1.0)
        } else {
            0.0
        };
        p.c[0] = a;
        let mut b = (-((ym - y0) / dy) / q0) as f32;
        b = if 0.0 < b * 1.0 {
            let qq = q0 as f32 + 2.0;
            let pp = q0 as f32 * 2.0 + 1.0;
            (b * 1.0 * (qq + pp)) / (qq * 1.0 + pp * b)
        } else {
            0.0
        };
        p.c[1] = b;
        p.c[2] = 1.0 - tension.abs();
    }
}

impl Point {
    /// The interpolation weight at phase `x` in `[0, 1)` (the shape switch of 0x2422d0).
    pub fn eval(&self, x: f64) -> f64 {
        if self.sign == 2 {
            return x;
        }
        let c = &self.c;
        let s = self.sign;
        let below_half = ((x.to_bits() >> 32) as u32) < 0x3fe0_0000;
        match self.shape {
            Shape::SINGLE_CURVE => single_curve(c, x),
            Shape::DOUBLE_CURVE => {
                if below_half {
                    single_curve(c, x * 2.0) * 0.5
                } else {
                    1.0 - single_curve(c, (1.0 - x) * 2.0) * 0.5
                }
            }
            Shape::STAIRS => {
                (((x * c[0] as f64 - c[2] as f64).round_ties_even() as i64 as i32) as f32 * c[1])
                    as f64
            }
            Shape::SMOOTH_STAIRS => {
                let u = x * c[0] as f64 - 0.5;
                let fl = (u - 0.5).round_ties_even() as i64 as i32 as f64;
                let v = (u - fl) * 2.0 - 1.0;
                let w = if s < 0 {
                    let q = 1.0 - v.abs();
                    (1.0 - q * q * q) * if v.is_sign_negative() { -1.0 } else { 1.0 }
                } else {
                    v * v * v
                };
                (fl + w * 0.5 + 1.0) * c[1] as f64
            }
            Shape::PULSE => {
                (((x * c[0] as f64 - c[2] as f64).round_ties_even() as i64 as u32) & 1) as f64
            }
            Shape::WAVE => {
                if s < 0 {
                    (((x * c[0] as f64 + 0.75) * c[1] as f64).sin() + 1.0) * 0.5
                } else {
                    let u = x * c[0] as f64 - 0.5;
                    ((u - (u - 0.5).round_ties_even() as i64 as i32 as f64) * 2.0 - 1.0).abs()
                }
            }
            Shape::SINGLE_CURVE_2 => {
                let d = if 0 < s { 1.0 - x } else { x };
                let d3 = d * d * d;
                x - (d3 * d3 - d) * c[0] as f64
            }
            Shape::DOUBLE_CURVE_2 => {
                if below_half {
                    let y = x * 2.0;
                    let d = if 0 < s { 1.0 - y } else { y };
                    let d3 = d * d * d;
                    (y - (d3 * d3 - d) * c[0] as f64) * 0.5
                } else {
                    let y = (1.0 - x) * 2.0;
                    let d = if 0 < s { 1.0 - y } else { y };
                    let d3 = d * d * d;
                    1.0 - (y - (d3 * d3 - d) * c[0] as f64) * 0.5
                }
            }
            Shape::HALF_SINE => {
                let h = if s < 1 {
                    1.0 - (x * HALF_PI_F32 as f64).cos()
                } else {
                    (x * HALF_PI_F32 as f64).sin()
                };
                x - (x - h) * c[0] as f64
            }
            Shape::SMOOTH => {
                let x2 = x * x;
                let xm = x - 1.0;
                x + (((x2 * 3.0 - x2 * 2.0 * x)
                    + x2 * xm * c[0] as f64
                    + x * xm * xm * c[1] as f64)
                    - x)
                    * c[2] as f64
            }
            Shape::SINGLE_CURVE_3 => {
                let (c0, c1, c2) = (c[0] as f64, c[1] as f64, c[2] as f64);
                if s < 1 {
                    c1 / (1.0 - c0 * x) - c2
                } else {
                    c1 / (1.0 - c0 * (1.0 - x)) - c2
                }
            }
            Shape::DOUBLE_CURVE_3 => {
                let (c0, c1, c2) = (c[0] as f64, c[1] as f64, c[2] as f64);
                if below_half {
                    if s < 1 {
                        c1 / (1.0 - c0 * x * 2.0) - c2
                    } else {
                        c1 / (1.0 - c0 * (1.0 - x * 2.0)) - c2
                    }
                } else {
                    let u = (1.0 - x) * 2.0;
                    if s < 1 {
                        (1.0 - c1 / (1.0 - c0 * u)) + c2
                    } else {
                        (1.0 - c1 / (1.0 - c0 * (1.0 - u))) + c2
                    }
                }
            }
            _ => 0.0,
        }
    }

    /// The derived coefficients (for tests and the editor).
    pub fn coefficients(&self) -> [f32; 3] {
        self.c
    }
}
