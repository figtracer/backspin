//! The wave / LFO engine (core.md section 4): points to knots (0x2d1114), the 8192-entry table of
//! the uniform cubic B-spline (0x2d13ec), the Catmull-Rom coefficients with the step fix
//! (0x2fcbb0), the table read (0x2df6b0), LFO timing and the smoothing filter.

/// Table entries per wave period.
pub const TABLE: usize = 8192;

/// A wave point: `x` in table units (8192 = one period), `y` in [-1, 1], `factor` = knot
/// multiplicity (1 soft, 3 a corner through the point).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: f32,
    pub factor: i32,
}

/// The knots of 0x2d1114: each point `factor` times, padded periodically by three knots on each
/// side.
fn knots(points: &[Point]) -> (Vec<i32>, Vec<f32>) {
    let mut k: Vec<(i32, f32)> = Vec::new();
    for p in points {
        for _ in 0..p.factor.max(0) {
            k.push((p.x, p.y));
        }
    }
    if k.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let t = TABLE as i32;
    let all: Vec<(i32, f32)> = if k.len() == 1 {
        let y = k[0].1;
        [(0, y); 3].into_iter().chain([(t, y); 3]).collect()
    } else {
        let n = k.len();
        let before = (0..3usize).map(|j| {
            let i = (n as isize - 3 + j as isize).rem_euclid(n as isize) as usize;
            (k[i].0 - t * (1 + ((2 - j) / n) as i32), k[i].1)
        });
        let after = (0..3).map(|j| (k[j % n].0 + t * (1 + (j / n) as i32), k[j % n].1));
        before.chain(k.iter().copied()).chain(after).collect()
    };
    all.into_iter().unzip()
}

/// The wave value when it is constant (no points, or all y equal), else `None`.
pub fn constant(points: &[Point]) -> Option<f32> {
    match points.first() {
        None => Some(0.0),
        Some(p) => points.iter().all(|q| q.y == p.y).then_some(p.y),
    }
}

/// `!(0 <= t < 1)` (also true for NaN): take the other root.
fn outside(t: f64) -> bool {
    !(0.0..1.0).contains(&t)
}

/// 0x2d13ec: the table of the B-spline through the knots, sampled at integer x.
pub fn render_table(points: &[Point]) -> Vec<f32> {
    let mut table = vec![0.0f32; TABLE];
    if let Some(y) = constant(points) {
        table.fill(y);
        return table;
    }
    let (kx, ky) = knots(points);
    let n = kx.len();
    if n <= 3 {
        return table;
    }
    let last_index = TABLE as i32 - 1;
    let mut s: i32 = 0;
    let (mut x0, mut x1, mut x2) = (kx[0], kx[1], kx[2]);
    for i in 0..n - 3 {
        let x3 = kx[i + 3];
        if x3 != x0 && x3 != 0 {
            let last = ((x1 + x2 * 4 + 5999 + x3) / 6 - 1000).min(last_index);
            if s <= last {
                let (y0, y1, y2, y3) = (
                    ky[i] as f64,
                    ky[i + 1] as f64,
                    ky[i + 2] as f64,
                    ky[i + 3] as f64,
                );
                let a3 = x0 * 3 + x1 * -6 + x2 * 3;
                let u = (x1 * 3 - (x0 + x2 * 3)) + x3;
                let u4 = if (u as u32) < 2 { 1 } else { u };
                let mut c0 = (x0 + s * -6 + x1 * 4 + x2) as f64 / u4 as f64;
                let du = if u != 0 { u as f64 } else { 1.0 };
                let bx = (x2 * 3 + x0 * -3) as f64;
                let step = if u != 0 { -6.0 / u as f64 } else { -6.0 };
                let a = a3 as f64 / du;
                let b = bx / du;
                let q = (-(a * a) + b * 3.0) / 9.0;
                let q3 = q * q * q;
                let sq = if q3 <= 0.0 {
                    (-(q * q * q)).sqrt()
                } else {
                    0.0
                };
                let mut t;
                if u == 0 {
                    if a3 == 0 {
                        t = -c0 / bx;
                    } else {
                        let p = bx / a3 as f64;
                        let d = (p * p * 0.25 - c0 / a3 as f64).sqrt();
                        t = p * -0.5 + d;
                        if outside(t) {
                            t = p * -0.5 - d;
                        }
                    }
                } else {
                    t = 0.0;
                    if c0 != 0.0 {
                        let a_3 = a / 3.0;
                        let r = (a * (-(a * a) + b * 4.5)) / 27.0 + c0 * -0.5;
                        let disc = q3 + r * r;
                        if disc <= 0.0 {
                            if disc >= 0.0 {
                                let c = r.cbrt();
                                t = c * 2.0 - a_3;
                                if outside(t) {
                                    t = -c - a_3;
                                }
                            } else {
                                let m = (-q).sqrt() + (-q).sqrt();
                                let th = (r / sq).acos() / 3.0;
                                t = m * th.cos() - a_3;
                                if outside(t) {
                                    t = -a_3 + (th + 2.094_395_102_393_195_3).cos() * m;
                                    if outside(t) {
                                        t = -a_3 + (th + 4.188_790_204_786_390_5).cos() * m;
                                    }
                                }
                            }
                        } else {
                            let sd = disc.sqrt();
                            t = ((r + sd).cbrt() - a_3) + (r - sd).cbrt();
                        }
                    }
                }
                let d3 = (y1 * 3.0 - y0) + y2 * -3.0 + y3;
                let d2 = y1 * -6.0 + y0 * 3.0 + y2 * 3.0;
                let d1 = y2 * 3.0 + y0 * -3.0;
                let d0 = y0 + y1 * 4.0 + y2;
                let poly = |t: f64| ((d0 + (d1 + (d2 + d3 * t) * t) * t) / 6.0) as f32;
                table[s as usize] = poly(t);
                let k = last - s;
                if k == 0 || last < s {
                    s += 1;
                } else if u == 0 {
                    let mut idx = s as usize;
                    s = last + 1;
                    if a.abs() >= 1e-8 {
                        let bb = b / a;
                        for _ in 0..k {
                            c0 += step;
                            let d = (bb * bb * 0.25 - c0 / a).sqrt();
                            let mut t = bb * -0.5 + d;
                            if outside(t) {
                                t = bb * -0.5 - d;
                            }
                            idx += 1;
                            table[idx] = poly(t);
                        }
                    } else {
                        for _ in 0..k {
                            c0 += step;
                            idx += 1;
                            table[idx] = poly(-c0 / b);
                        }
                    }
                } else {
                    let mut idx = s as usize + 1;
                    s = last + 1;
                    let mut g = 0.01;
                    while idx as i32 != s {
                        c0 += step;
                        let mut tn = 0.0;
                        if c0 != 0.0 {
                            let mut guess = t + g;
                            for _ in 0..32 {
                                let g2 = guess * guess;
                                let der = b + (a + a) * guess + g2 * 3.0;
                                if der.abs() < 1e-5 {
                                    tn = guess + 0.001;
                                } else {
                                    tn = guess - (c0 + a * g2 + g2 * guess + guess * b) / der;
                                    if (tn - guess).abs() < 0.0001 {
                                        break;
                                    }
                                }
                                guess = tn;
                            }
                        }
                        g = tn - t;
                        table[idx] = poly(tn);
                        idx += 1;
                        t = tn;
                    }
                }
            }
        }
        x0 = x1;
        x1 = x2;
        x2 = x3;
    }
    table
}

/// The step positions of 0x2fcbb0: consecutive factor-3 points with dx < 2, plus 0 when the first
/// factor-3 point is at 0 and the last at >= 8191.
fn steps(points: &[Point]) -> Vec<usize> {
    let p3: Vec<&Point> = points.iter().filter(|p| p.factor == 3).take(500).collect();
    let mut out = Vec::new();
    if p3.len() > 1 {
        if p3[0].x == 0 && p3[p3.len() - 1].x > 0x1ffe {
            out.push(0);
        }
        for w in p3.windows(2) {
            if w[1].x - w[0].x < 2 {
                out.push(w[1].x.rem_euclid(TABLE as i32) as usize);
            }
        }
    }
    out
}

/// One Catmull-Rom entry `[a, b, c, d]` from four table values, in double with the compiled FMA
/// order, stored as float32.
fn coef(ym: f32, y0: f32, y1: f32, y2: f32) -> [f32; 4] {
    let (ym, y0, y1, y2) = (ym as f64, y0 as f64, y1 as f64, y2 as f64);
    let a = (y0 - y1).mul_add(1.5, (y2 - ym) * 0.5);
    let b = y2.mul_add(-0.5, y0.mul_add(-2.5, y1.mul_add(2.0, ym)));
    let c = (y1 - ym) * 0.5;
    [a as f32, b as f32, c as f32, y0 as f32]
}

/// 0x2fcbb0: the coefficients of every table index, with the step fix for the shapers that use it.
pub fn coefficients(table: &[f32], points: &[Point], step_fix: bool) -> Vec<[f32; 4]> {
    let n = TABLE;
    let y = |i: isize| table[i.rem_euclid(n as isize) as usize];
    let mut co: Vec<[f32; 4]> = (0..n as isize)
        .map(|i| coef(y(i - 1), y(i), y(i + 1), y(i + 2)))
        .collect();
    if step_fix {
        for x in steps(points) {
            let x = x as isize;
            let (m3, m2, m1, p0, p1, p2) = (y(x - 3), y(x - 2), y(x - 1), y(x), y(x + 1), y(x + 2));
            co[(x - 2).rem_euclid(n as isize) as usize] = coef(m3, m2, m1, m1);
            co[(x - 1).rem_euclid(n as isize) as usize] = coef(m2, m1, m1, m1);
            co[x as usize % n] = coef(p0, p0, p1, p2);
        }
    }
    co
}

/// A compiled wave.
#[derive(Clone, Debug)]
pub struct Wave {
    pub constant: Option<f32>,
    pub coefs: Vec<[f32; 4]>,
}

impl Wave {
    pub fn new(points: &[Point], step_fix: bool) -> Self {
        let constant = constant(points);
        let coefs = if constant.is_some() {
            Vec::new()
        } else {
            coefficients(&render_table(points), points, step_fix)
        };
        Self { constant, coefs }
    }

    /// The Catmull-Rom read at table position `pos` (0x2df6b0): `f = f32(pos - int(pos))`,
    /// `fma(fma(fma(a, f, b), f, c), f, d)`.
    pub fn read(&self, pos: f64) -> f32 {
        if let Some(c) = self.constant {
            return c;
        }
        let i = pos as i32;
        let f = (pos - i as f64) as f32;
        let [a, b, c, d] = self.coefs[(i & 0x1fff) as usize];
        a.mul_add(f, b).mul_add(f, c).mul_add(f, d)
    }
}

/// `Defs::LfoLength` (0x2ddd48) as (numerator, denominator) of a whole note.
pub const LENGTHS: [(u32, u32); 23] = [
    (1, 128),
    (1, 64),
    (1, 32),
    (1, 24),
    (1, 16),
    (3, 32),
    (1, 12),
    (1, 8),
    (3, 16),
    (1, 6),
    (1, 4),
    (3, 8),
    (1, 3),
    (1, 2),
    (3, 4),
    (1, 1),
    (3, 2),
    (2, 1),
    (3, 1),
    (4, 1),
    (8, 1),
    (16, 1),
    (32, 1),
];

/// The period of a length in quarter-note beats (0x2dda84).
pub fn length_beats(length: i32) -> f64 {
    let (num, den) = LENGTHS[length.clamp(0, 22) as usize];
    (num * 4) as f64 / den as f64
}

/// Hz mode rate (0x2de3e4): `0.02 * 2^(18 speed)` in float32.
pub fn rate_hz(speed: f32) -> f64 {
    ((((speed as f64 * 18.0) as f32 as f64).exp2() as f32) * 0.02) as f64
}

/// LfoSmoothingFilter coefficient (0x2d243c): `x = f32(K * exp(9.0109 * (1 - s))) / sr` (with
/// `1 - s` in float32), then `x * fma(fma(x, 15.959062, -11.969296), x, 5.9948826)` clamped to
/// [0, 1].
pub fn smoothing_coef(smoothing: f32, sr: f32) -> f32 {
    const K: f64 = 20.601_722_307_054_37 * 0.125;
    let e = (9.010_913_347_279_288 * (1.0f32 - smoothing) as f64).exp();
    let x = (K * e) as f32 / sr;
    let x = x * x.mul_add(15.959_062, -11.969_296).mul_add(x, 5.994_882_6);
    if x <= 1.0 {
        if x >= 0.0 { x } else { 0.0 }
    } else {
        1.0
    }
}

/// LfoSmoothingFilter, inlined in ShaperProcessor::processBlock (0x462b40): two one-poles that
/// each average the current and the previous input,
/// `y1' = fma(c, (x + x_prev) * 0.5 - y1, y1)`, `y2' = fma(c, (y1 + y1') * 0.5 - y2, y2)`,
/// output `y2'`. The state starts at the first input; a constant input within 1e-4 of the output
/// passes unchanged.
#[derive(Clone, Debug)]
pub struct Smoothing {
    pub coef: f32,
    y1: f32,
    y2: f32,
    x_prev: f32,
    first: bool,
}

impl Smoothing {
    pub fn new(coef: f32) -> Self {
        Self {
            coef,
            y1: 0.0,
            y2: 0.0,
            x_prev: 0.0,
            first: true,
        }
    }

    /// Smooths one block in place. `ramping` is false when the block holds one constant value.
    /// Returns whether the output is a per-sample buffer: the filter skips a constant input
    /// within 1e-4 of its output (the output state starts at 0, so a constant non-zero wave is
    /// filtered, and flagged as a buffer, in its first block).
    pub fn process(&mut self, buf: &mut [f32], ramping: bool) -> bool {
        let Some(&x0) = buf.first() else {
            return ramping;
        };
        if !ramping && (self.y2 - x0).abs() < 0.0001 {
            return false;
        }
        if self.first {
            self.first = false;
            self.y1 = x0;
            self.y2 = x0;
            self.x_prev = x0;
        }
        let c = self.coef;
        for v in buf.iter_mut() {
            let x = *v;
            let t = (x + self.x_prev).mul_add(0.5, -self.y1);
            let y1 = c.mul_add(t, self.y1);
            let u = (self.y1 + y1).mul_add(0.5, -self.y2);
            let y2 = c.mul_add(u, self.y2);
            self.y1 = y1;
            self.y2 = y2;
            self.x_prev = x;
            *v = y2;
        }
        true
    }
}
