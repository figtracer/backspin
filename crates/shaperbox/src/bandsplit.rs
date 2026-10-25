//! The per-shaper three-band split (core.md section 3): Bandsplit mode 0 (process 0x2f77b8),
//! CrossoverFilter (0x2f9294) with TPT one-poles (0x2f987c, 0x2f9a74) and RBJ biquads (0x2f9c7c,
//! 0x2fa3a0), and Multiband::process (0x2f716c). Float32 as the plugin computes.

/// `DAT_01328cc0`: 20.60172 Hz (MIDI note 16).
pub const F0: f64 = 20.601_722_307_054_37;

/// Defs::Band.
pub const LOW: usize = 0;
pub const MID: usize = 1;
pub const HIGH: usize = 2;

/// `BandSplitPorts`: the crossover values the DSP sees, `low' = min(low, 0.95)` and
/// `high' = max(high, low' + 0.05)`.
pub fn clamp_params(low: f32, high: f32) -> (f32, f32) {
    let low = low.min(0.95);
    (low, high.max(low + 0.05))
}

/// Which crossovers run: `low' > 0`, `high' < 1`.
pub fn active(low: f32, high: f32) -> (bool, bool) {
    let (l, h) = clamp_params(low, high);
    (l > 0.0, h < 1.0)
}

#[derive(Clone, Copy, PartialEq)]
enum Tpt {
    Lp,
    Hp,
    /// `2 lp - x`.
    Ap,
    /// `x + lp * -2`.
    Apn,
}

/// `G = g / (1 + g)`, `g = tan(pi * min(f / sr, 0.4999))`, `f = f32(F0) * exp2f(10 p)`.
fn tpt_coeff(p: f32, sr: f32) -> f32 {
    let e = ((p as f64 * 10.0) as f32 as f64).exp2() as f32;
    let fc = (F0 as f32 * e) / sr;
    let w = if fc >= 0.0 {
        fc.min(0.4999) * std::f32::consts::PI
    } else {
        0.0
    };
    let g = ((w as f64).tan() as f32).max(0.0);
    g / (g + 1.0)
}

fn tpt_tick(s: &mut f32, x: f32, g: f32, mode: Tpt) -> f32 {
    let v = (x - *s) * g;
    let lp = *s + v;
    *s = v + lp;
    match mode {
        Tpt::Lp => lp,
        Tpt::Hp => x - lp,
        Tpt::Ap => lp * 2.0 - x,
        Tpt::Apn => x + lp * -2.0,
    }
}

/// RBJ Butterworth coefficients `(b0, b1, a1, a2)` (`b2 == b0`), computed in double.
fn rbj_coeff(p: f32, sr: f32, hp: bool) -> [f32; 4] {
    let sr = sr as f64;
    let f = (F0 * (p as f64 * 10.0).exp2()).min(sr / 2.02);
    let w0 = f * std::f64::consts::TAU / sr;
    let (s, c) = (w0.sin(), w0.cos());
    let alpha = s / std::f64::consts::SQRT_2;
    let a0 = alpha + 1.0;
    let (b0, b1) = if hp {
        ((1.0 + c) * 0.5 / a0, -(1.0 + c) / a0)
    } else {
        ((1.0 - c) * 0.5 / a0, (1.0 - c) / a0)
    };
    [
        b0 as f32,
        b1 as f32,
        (c * -2.0 / a0) as f32,
        ((1.0 - alpha) / a0) as f32,
    ]
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// Direct form I with the compiled operation order (fused after the first product).
    fn tick(&mut self, x: f32, k: [f32; 4]) -> f32 {
        let [b0, b1, a1, a2] = k;
        let acc = x.mul_add(b0, b1 * self.x1);
        let acc = self.x2.mul_add(b0, acc);
        let y = (-self.y1).mul_add(a1, acc);
        let y = (-self.y2).mul_add(a2, y);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// One crossover leg, stereo (independent states, same coefficients).
#[derive(Clone, Default)]
struct Crossover {
    hp: bool,
    tpt: [[f32; 3]; 2],
    rbj: [[Biquad; 2]; 2],
    last_slope: Option<i32>,
}

impl Crossover {
    fn new(hp: bool) -> Self {
        Self {
            hp,
            ..Default::default()
        }
    }

    fn reset(&mut self) {
        self.tpt = [[0.0; 3]; 2];
        self.rbj = [[Biquad::default(); 2]; 2];
    }

    fn process(&mut self, buf: &mut [[f32; 2]], p: f32, slope: i32, sr: f32) {
        if self.last_slope != Some(slope) {
            self.reset();
            self.last_slope = Some(slope);
        }
        let t = if self.hp { Tpt::Hp } else { Tpt::Lp };
        match slope {
            0 => {
                let g = tpt_coeff(p, sr);
                for f in buf.iter_mut() {
                    for c in 0..2 {
                        f[c] = tpt_tick(&mut self.tpt[c][0], f[c], g, t);
                    }
                }
            }
            1 => {
                let g = tpt_coeff(p, sr);
                let ap = if self.hp { Tpt::Apn } else { Tpt::Ap };
                for (stage, mode) in [(0, t), (1, t), (2, ap)] {
                    for f in buf.iter_mut() {
                        for c in 0..2 {
                            f[c] = tpt_tick(&mut self.tpt[c][stage], f[c], g, mode);
                        }
                    }
                }
            }
            _ => {
                let k = rbj_coeff(p, sr, self.hp);
                for stage in 0..2 {
                    for f in buf.iter_mut() {
                        for c in 0..2 {
                            f[c] = self.rbj[c][stage].tick(f[c], k);
                        }
                    }
                }
            }
        }
    }
}

/// The three bands of one block; a band the split does not produce is not used.
#[derive(Clone, Default)]
pub struct Bands {
    pub b: [Vec<[f32; 2]>; 3],
    pub produced: [bool; 3],
}

/// Bandsplit mode 0: seven crossovers F0 LP(lo), F1 LP(hi), F2 HP(lo), F3 HP(hi), F4 LP(hi),
/// F5 HP(hi), F6 LP(hi).
#[derive(Clone)]
pub struct Bandsplit {
    f: [Crossover; 7],
    used: [bool; 7],
}

impl Default for Bandsplit {
    fn default() -> Self {
        Self::new()
    }
}

impl Bandsplit {
    pub fn new() -> Self {
        Self {
            f: [false, false, true, true, false, true, false].map(Crossover::new),
            used: [false; 7],
        }
    }

    /// Runs filter `i` in place; a filter idle in the previous block starts from zero.
    fn run(
        &mut self,
        i: usize,
        buf: &mut [[f32; 2]],
        p: f32,
        slope: i32,
        sr: f32,
        used: &mut [bool; 7],
    ) {
        if !self.used[i] {
            self.f[i].reset();
        }
        used[i] = true;
        self.f[i].process(buf, p, slope, sr);
    }

    /// Splits `x` into `out` with the band split state values.
    pub fn process(
        &mut self,
        x: &[[f32; 2]],
        low: f32,
        high: f32,
        ls: i32,
        hs: i32,
        sr: f32,
        out: &mut Bands,
    ) {
        let n = x.len();
        let (low_on, high_on) = active(low, high);
        let (low, high) = clamp_params(low, high);
        let mut used = [false; 7];
        for b in &mut out.b {
            b.clear();
        }
        out.produced = [false; 3];
        match (low_on, high_on) {
            (false, false) => {
                out.b[MID].extend_from_slice(x);
                out.produced[MID] = true;
            }
            (true, true) if ls == 0 && hs == 0 => {
                let mut lo = x.to_vec();
                self.run(0, &mut lo, low, ls, sr, &mut used);
                self.run(1, &mut lo, high, hs, sr, &mut used);
                let mut hi = x.to_vec();
                self.run(3, &mut hi, high, hs, sr, &mut used);
                self.run(2, &mut hi, low, ls, sr, &mut used);
                let mid = (0..n)
                    .map(|i| [0, 1].map(|c| (x[i][c] - lo[i][c]) - hi[i][c]))
                    .collect();
                out.b = [lo, mid, hi];
                out.produced = [true; 3];
            }
            (true, true) => {
                let mut a = x.to_vec();
                self.run(0, &mut a, low, ls, sr, &mut used);
                let mut lp = a.clone();
                self.run(4, &mut lp, high, hs, sr, &mut used);
                self.run(5, &mut a, high, hs, sr, &mut used);
                let lo = lp
                    .iter()
                    .zip(&a)
                    .map(|(u, v)| [u[0] + v[0], u[1] + v[1]])
                    .collect();
                let mut c = x.to_vec();
                self.run(2, &mut c, low, ls, sr, &mut used);
                let mut mid = c.clone();
                self.run(6, &mut mid, high, hs, sr, &mut used);
                self.run(3, &mut c, high, hs, sr, &mut used);
                out.b = [lo, mid, c];
                out.produced = [true; 3];
            }
            (true, false) => {
                let mut lo = x.to_vec();
                self.run(0, &mut lo, low, ls, sr, &mut used);
                let mut mid = x.to_vec();
                self.run(2, &mut mid, low, ls, sr, &mut used);
                out.b[LOW] = lo;
                out.b[MID] = mid;
                out.produced = [true, true, false];
            }
            (false, true) => {
                let mut mid = x.to_vec();
                self.run(6, &mut mid, high, hs, sr, &mut used);
                let mut hi = x.to_vec();
                self.run(3, &mut hi, high, hs, sr, &mut used);
                out.b[MID] = mid;
                out.b[HIGH] = hi;
                out.produced = [false, true, true];
            }
        }
        self.used = used;
    }
}

/// Multiband::process: the sum of the produced bands, or one band with
/// solo. With both crossovers inactive the mid band always passes.
pub fn multiband_sum(
    bands: &Bands,
    low: f32,
    high: f32,
    solo: bool,
    selected: usize,
    out: &mut [[f32; 2]],
) {
    let (low_on, high_on) = active(low, high);
    if !low_on && !high_on {
        out.copy_from_slice(&bands.b[MID]);
        return;
    }
    // With the low band produced the sum starts from it (or from zeros under solo) and adds the
    // mid band; without it the sum starts as a copy of the mid band. The two differ only in the
    // sign of zero results.
    let mid = !solo || selected == MID;
    if low_on {
        if !solo || selected == LOW {
            out.copy_from_slice(&bands.b[LOW]);
        } else {
            out.fill([0.0; 2]);
        }
        if mid {
            for (o, m) in out.iter_mut().zip(&bands.b[MID]) {
                o[0] += m[0];
                o[1] += m[1];
            }
        }
    } else if mid {
        out.copy_from_slice(&bands.b[MID]);
    } else {
        out.fill([0.0; 2]);
    }
    if high_on && (!solo || selected == HIGH) {
        for (o, h) in out.iter_mut().zip(&bands.b[HIGH]) {
            o[0] += h[0];
            o[1] += h[1];
        }
    }
}
